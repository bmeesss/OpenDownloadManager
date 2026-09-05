//! The protocol-neutral download manager.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use odm_core::{
    Backend, BackendKind, BackendTask, DownloadId, DownloadProgress, DownloadState, Error, Event,
    ProgressSink, Result,
};
use serde_json::Value;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use url::Url;

use crate::config::{ManagerConfig, RecoveryPolicy};
use crate::events::EventBus;
use crate::persistence::{Download, Persistence};
use crate::queue::Queue;
use crate::scheduler;

/// A request to enqueue a new download.
#[derive(Debug, Clone)]
pub struct DownloadSpec {
    /// Source URL.
    pub url: Url,
    /// Final on-disk destination.
    pub destination: std::path::PathBuf,
    /// Which backend executes the transfer.
    pub backend: BackendKind,
    /// Whether an existing destination may be overwritten.
    pub overwrite: bool,
    /// Opaque, protocol-specific metadata.
    pub backend_meta: Value,
}

impl DownloadSpec {
    /// Creates a spec for `url` written to `destination` using the HTTP
    /// backend, without overwrite.
    #[must_use]
    pub fn new(url: Url, destination: std::path::PathBuf) -> Self {
        Self {
            url,
            destination,
            backend: BackendKind::Http,
            overwrite: false,
            backend_meta: Value::Null,
        }
    }

    /// Sets the overwrite flag.
    #[must_use]
    pub fn with_overwrite(mut self, overwrite: bool) -> Self {
        self.overwrite = overwrite;
        self
    }
}

/// One download as tracked by the manager: its persisted record plus the
/// runtime handles needed to drive and stop it.
struct ManagedDownload {
    info: Download,
    cancel: Option<Arc<Notify>>,
    dispose: Option<Arc<Notify>>,
    handle: Option<JoinHandle<()>>,
}

impl ManagedDownload {
    fn new(info: Download) -> Self {
        Self {
            info,
            cancel: None,
            dispose: None,
            handle: None,
        }
    }
}

struct ManagerInner {
    config: ManagerConfig,
    backends: HashMap<BackendKind, Arc<dyn Backend>>,
    queue: Mutex<Queue>,
    downloads: Mutex<HashMap<DownloadId, ManagedDownload>>,
    persistence: Persistence,
    events: EventBus,
    pause_intents: Mutex<HashSet<DownloadId>>,
}

/// The protocol-neutral download manager.
///
/// Owns the queue, scheduler, lifecycle state machine, persistence, event bus
/// and bandwidth policy, and drives protocol-specific backends through the
/// [`Backend`] boundary. It never uses a transport directly.
pub struct DownloadManager {
    inner: Arc<ManagerInner>,
}

impl Clone for DownloadManager {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl DownloadManager {
    /// Opens the manager, loading existing downloads from `db_path` and
    /// recovering any that were in flight when the process last exited.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] if the database cannot be opened or
    /// recovered.
    pub fn open(
        config: ManagerConfig,
        db_path: &Path,
        backends: Vec<Arc<dyn Backend>>,
    ) -> Result<Self> {
        let persistence = Persistence::open(db_path)?;
        let mut backend_map: HashMap<BackendKind, Arc<dyn Backend>> = HashMap::new();
        for b in backends {
            backend_map.insert(b.kind(), b);
        }
        let inner = Arc::new(ManagerInner {
            config,
            backends: backend_map,
            queue: Mutex::new(Queue::new()),
            downloads: Mutex::new(HashMap::new()),
            persistence,
            events: EventBus::new(),
            pause_intents: Mutex::new(HashSet::new()),
        });
        let mgr = Self { inner };
        mgr.recover()?;
        Ok(mgr)
    }

    /// Enqueues a new download and, if capacity allows, starts it.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] if no backend is registered for the
    /// spec's protocol, or if persistence fails.
    pub fn enqueue(&self, spec: DownloadSpec) -> Result<DownloadId> {
        if !self.inner.backends.contains_key(&spec.backend) {
            return Err(Error::Internal(format!(
                "no backend registered for {}",
                spec.backend
            )));
        }
        let now = SystemTime::now();
        let id = DownloadId::new();
        let dl = Download {
            id,
            url: spec.url,
            destination: spec.destination,
            backend: spec.backend,
            state: DownloadState::Queued,
            overwrite: spec.overwrite,
            total_bytes: None,
            downloaded_bytes: 0,
            created_at: now,
            updated_at: now,
            started_at: None,
            completed_at: None,
            error: None,
            backend_meta: spec.backend_meta,
        };
        self.inner.persistence.save(&dl)?;
        self.inner
            .downloads
            .lock()
            .unwrap()
            .insert(id, ManagedDownload::new(dl));
        self.inner.queue.lock().unwrap().enqueue(id);
        self.inner.events.publish(Event::Queued(id));
        self.schedule();
        Ok(id)
    }

    /// Starts a queued download now, if concurrency capacity allows.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] if the download is unknown or not
    /// currently queued.
    pub fn start(&self, id: DownloadId) -> Result<()> {
        let state = self.get(id).map(|d| d.state);
        if state != Some(DownloadState::Queued) {
            return Err(Error::Internal(format!(
                "download {id} is not queued (state: {state:?})"
            )));
        }
        if self.count_active() >= self.inner.config.max_concurrent_downloads {
            return Ok(());
        }
        self.inner.queue.lock().unwrap().remove(&id);
        self.begin_download(id);
        Ok(())
    }

    /// Pauses an in-flight download.
    ///
    /// The running transfer is cancelled; on completion the manager marks it
    /// [`DownloadState::Paused`] rather than cancelled, so it can be resumed.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] if the download is not currently
    /// transferring.
    pub fn pause(&self, id: DownloadId) -> Result<()> {
        let state = self.get(id).map(|d| d.state);
        if !matches!(
            state,
            Some(DownloadState::Starting) | Some(DownloadState::Downloading)
        ) {
            return Err(Error::Internal(format!(
                "cannot pause download {id} in state {state:?}"
            )));
        }
        self.inner.pause_intents.lock().unwrap().insert(id);
        let cancel = self
            .inner
            .downloads
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|md| md.cancel.clone());
        if let Some(c) = cancel {
            c.notify_one();
        }
        Ok(())
    }

    /// Resumes a paused download by re-queuing it.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] if the download is not paused.
    pub fn resume(&self, id: DownloadId) -> Result<()> {
        let state = self.get(id).map(|d| d.state);
        if state != Some(DownloadState::Paused) {
            return Err(Error::Internal(format!(
                "cannot resume download {id} in state {state:?}"
            )));
        }
        self.transition_state(id, DownloadState::Queued, None)?;
        self.inner.queue.lock().unwrap().enqueue(id);
        self.inner.events.publish(Event::Resumed(id));
        self.schedule();
        Ok(())
    }

    /// Cancels a download.
    ///
    /// An in-flight download is stopped via its cancellation signal; a queued,
    /// paused or failed download is moved straight to
    /// [`DownloadState::Cancelled`].
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] if the download is unknown or already in
    /// a terminal state that cannot be cancelled.
    pub fn cancel(&self, id: DownloadId) -> Result<()> {
        let state = self.get(id).map(|d| d.state);
        match state {
            Some(DownloadState::Starting) | Some(DownloadState::Downloading) => {
                let cancel = self
                    .inner
                    .downloads
                    .lock()
                    .unwrap()
                    .get(&id)
                    .and_then(|md| md.cancel.clone());
                if let Some(c) = cancel {
                    c.notify_one();
                }
            }
            Some(DownloadState::Queued)
            | Some(DownloadState::Paused)
            | Some(DownloadState::Failed) => {
                self.inner.queue.lock().unwrap().remove(&id);
                self.transition_state(
                    id,
                    DownloadState::Cancelled,
                    Some("cancelled by user".into()),
                )?;
                self.inner.events.publish(Event::Cancelled(id));
            }
            Some(DownloadState::Completed) | Some(DownloadState::Cancelled) => {
                return Err(Error::Internal(format!(
                    "cannot cancel download {id} in state {state:?}"
                )));
            }
            None => {
                return Err(Error::Internal(format!("unknown download {id}")));
            }
        }
        Ok(())
    }

    /// Re-enqueues a failed download.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] if the download is not failed.
    pub fn retry(&self, id: DownloadId) -> Result<()> {
        let state = self.get(id).map(|d| d.state);
        if state != Some(DownloadState::Failed) {
            return Err(Error::Internal(format!(
                "cannot retry download {id} in state {state:?}"
            )));
        }
        self.transition_state(id, DownloadState::Queued, None)?;
        self.inner.queue.lock().unwrap().enqueue(id);
        self.schedule();
        Ok(())
    }

    /// Removes a download from the manager and the database. A running
    /// transfer is signalled to stop first.
    ///
    /// Unlike [`cancel`], this also signals the backend that transfer data
    /// should be disposed.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] if persistence fails.
    pub fn remove(&self, id: DownloadId) -> Result<()> {
        let (dispose, paused_task) = {
            let g = self.inner.downloads.lock().unwrap();
            let Some(md) = g.get(&id) else {
                return Err(Error::Internal(format!("unknown download {id}")));
            };
            let task = if md.handle.is_none() && md.info.state == DownloadState::Paused {
                Some(self.task_for_info(&md.info, Arc::new(Notify::new()), Arc::new(Notify::new())))
            } else {
                None
            };
            (md.dispose.clone(), task)
        };
        if let Some(c) = &dispose {
            c.notify_one();
        }
        if let Some(task) = paused_task {
            let backend = {
                let g = self.inner.downloads.lock().unwrap();
                g.get(&id)
                    .and_then(|md| self.inner.backends.get(&md.info.backend).cloned())
            };
            if let Some(backend) = backend {
                tokio::spawn(async move {
                    let _ = backend.dispose(task).await;
                });
            }
        }
        self.inner.pause_intents.lock().unwrap().remove(&id);
        self.inner.queue.lock().unwrap().remove(&id);
        self.inner.downloads.lock().unwrap().remove(&id);
        self.inner.persistence.remove(id)?;
        self.schedule();
        Ok(())
    }

    /// Returns a snapshot of one download, if known.
    #[must_use]
    pub fn get(&self, id: DownloadId) -> Option<Download> {
        self.inner
            .downloads
            .lock()
            .unwrap()
            .get(&id)
            .map(|md| md.info.clone())
    }

    /// Returns snapshots of all known downloads.
    #[must_use]
    pub fn list(&self) -> Vec<Download> {
        self.inner
            .downloads
            .lock()
            .unwrap()
            .values()
            .map(|md| md.info.clone())
            .collect()
    }

    /// Subscribes to lifecycle events.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// Gracefully shuts down all in-flight transfers.
    ///
    /// Signals every active download to cancel, waits for them to finish
    /// with a bounded timeout, and aborts any that do not respond as a
    /// final fallback.
    pub async fn shutdown(&self) {
        let cancels: Vec<Arc<Notify>> = {
            let g = self.inner.downloads.lock().unwrap();
            g.values().filter_map(|md| md.cancel.clone()).collect()
        };
        for c in cancels {
            c.notify_one();
        }

        let mut handles: Vec<JoinHandle<()>> = {
            let mut g = self.inner.downloads.lock().unwrap();
            g.values_mut().filter_map(|md| md.handle.take()).collect()
        };

        let graceful = async {
            for h in &mut handles {
                let _ = h.await;
            }
        };

        if tokio::time::timeout(std::time::Duration::from_secs(5), graceful)
            .await
            .is_err()
        {
            for h in &handles {
                h.abort();
            }
            for h in handles {
                let _ = h.await;
            }
        }
    }

    fn count_active(&self) -> usize {
        self.inner
            .downloads
            .lock()
            .unwrap()
            .values()
            .filter(|md| {
                matches!(
                    md.info.state,
                    DownloadState::Starting | DownloadState::Downloading
                )
            })
            .count()
    }

    fn schedule(&self) {
        loop {
            let to_start = {
                let active = self.count_active();
                let queue = self.inner.queue.lock().unwrap();
                scheduler::select_for_start(
                    &queue,
                    active,
                    self.inner.config.max_concurrent_downloads,
                )
                .into_iter()
                .next()
            };
            match to_start {
                Some(id) => {
                    self.inner.queue.lock().unwrap().remove(&id);
                    self.begin_download(id);
                }
                None => break,
            }
        }
    }

    fn begin_download(&self, id: DownloadId) {
        if self
            .transition_state(id, DownloadState::Starting, None)
            .is_err()
        {
            return;
        }
        self.inner.events.publish(Event::Started(id));
        let cancel = Arc::new(Notify::new());
        let dispose = Arc::new(Notify::new());
        {
            let mut g = self.inner.downloads.lock().unwrap();
            if let Some(md) = g.get_mut(&id) {
                md.cancel = Some(cancel.clone());
                md.dispose = Some(dispose.clone());
            }
        }
        let mgr = DownloadManager {
            inner: self.inner.clone(),
        };
        let handle = tokio::spawn(async move { mgr.run_download(id, cancel, dispose).await });
        {
            let mut g = self.inner.downloads.lock().unwrap();
            if let Some(md) = g.get_mut(&id) {
                md.handle = Some(handle);
            }
        }
    }

    fn transition_state(
        &self,
        id: DownloadId,
        next: DownloadState,
        error: Option<String>,
    ) -> Result<()> {
        let now = SystemTime::now();
        let (from, to) = {
            let mut g = self.inner.downloads.lock().unwrap();
            let md = g
                .get_mut(&id)
                .ok_or_else(|| Error::Internal(format!("unknown download {id}")))?;
            let from = md.info.state;
            let to = from
                .transition(next)
                .map_err(|e| Error::Internal(e.to_string()))?;
            md.info.state = to;
            md.info.updated_at = now;
            match to {
                DownloadState::Downloading => {
                    if md.info.started_at.is_none() {
                        md.info.started_at = Some(now);
                    }
                }
                DownloadState::Completed => {
                    md.info.completed_at = Some(now);
                    md.info.error = None;
                }
                DownloadState::Failed | DownloadState::Cancelled => {
                    md.info.error = error;
                }
                _ => {
                    if error.is_some() {
                        md.info.error = error;
                    }
                }
            }
            (from, to)
        };
        let dl = self
            .inner
            .downloads
            .lock()
            .unwrap()
            .get(&id)
            .map(|md| md.info.clone());
        if let Some(dl) = dl {
            self.inner.persistence.save(&dl)?;
        }
        self.inner
            .events
            .publish(Event::StateChanged { id, from, to });
        Ok(())
    }

    fn on_progress(&self, id: DownloadId, p: DownloadProgress) {
        let now = SystemTime::now();
        let mut state_changed = None;
        let dl = {
            let mut g = self.inner.downloads.lock().unwrap();
            let Some(md) = g.get_mut(&id) else {
                return;
            };
            if md.info.state == DownloadState::Starting {
                md.info.state = DownloadState::Downloading;
                if md.info.started_at.is_none() {
                    md.info.started_at = Some(now);
                }
                state_changed = Some((DownloadState::Starting, DownloadState::Downloading));
            }
            md.info.downloaded_bytes = p.downloaded_bytes;
            if p.total_bytes.is_some() {
                md.info.total_bytes = p.total_bytes;
            }
            md.info.updated_at = now;
            md.info.clone()
        };
        let _ = self.inner.persistence.save(&dl);
        if let Some((from, to)) = state_changed {
            self.inner
                .events
                .publish(Event::StateChanged { id, from, to });
        }
        self.inner.events.publish(Event::Progress {
            id,
            downloaded_bytes: p.downloaded_bytes,
            total_bytes: p.total_bytes,
        });
    }

    async fn run_download(&self, id: DownloadId, cancel: Arc<Notify>, dispose: Arc<Notify>) {
        let (dl, backend) = {
            let g = self.inner.downloads.lock().unwrap();
            let md = match g.get(&id) {
                Some(md)
                    if matches!(
                        md.info.state,
                        DownloadState::Starting | DownloadState::Downloading
                    ) =>
                {
                    md
                }
                _ => return,
            };
            let dl = md.info.clone();
            let backend = self.inner.backends.get(&dl.backend).cloned();
            (dl, backend)
        };
        let rate = self.inner.config.bandwidth.limiter_for(self.count_active());
        let backend = match backend {
            Some(b) => b,
            None => {
                let _ = self.transition_state(
                    id,
                    DownloadState::Failed,
                    Some("no backend available for download".into()),
                );
                self.schedule();
                return;
            }
        };
        let mut task = self.task_for_info(&dl, cancel, dispose);
        task.progress = Some(Arc::new(ManagerProgressSink {
            mgr: DownloadManager {
                inner: self.inner.clone(),
            },
            id,
        }));
        task.rate_limiter = rate;

        let result = backend.run(task).await;
        match result {
            Ok(outcome) => {
                {
                    let mut g = self.inner.downloads.lock().unwrap();
                    if let Some(md) = g.get_mut(&id) {
                        md.info.downloaded_bytes = outcome.downloaded_bytes;
                        md.info.total_bytes = outcome.total_bytes.or(md.info.total_bytes);
                        md.info.backend_meta = outcome.backend_meta;
                        md.info.updated_at = SystemTime::now();
                    }
                }
                let _ = self.transition_state(id, DownloadState::Completed, None);
                self.inner.events.publish(Event::Completed(id));
            }
            Err(e) => {
                let paused = self.inner.pause_intents.lock().unwrap().remove(&id);
                if matches!(e, Error::Cancelled) && paused {
                    let _ = self.transition_state(
                        id,
                        DownloadState::Paused,
                        Some("paused by user".into()),
                    );
                    self.inner.events.publish(Event::Paused(id));
                } else if matches!(e, Error::Cancelled) {
                    let _ = self.transition_state(
                        id,
                        DownloadState::Cancelled,
                        Some("cancelled by user".into()),
                    );
                    self.inner.events.publish(Event::Cancelled(id));
                } else {
                    let err = e.to_string();
                    let _ = self.transition_state(id, DownloadState::Failed, Some(err.clone()));
                    self.inner.events.publish(Event::Failed { id, error: err });
                }
            }
        }
        if let Some(md) = self.inner.downloads.lock().unwrap().get_mut(&id) {
            md.handle = None;
            md.cancel = None;
            md.dispose = None;
        }
        self.inner.pause_intents.lock().unwrap().remove(&id);
        self.schedule();
    }

    fn task_for_info(
        &self,
        dl: &Download,
        cancel: Arc<Notify>,
        dispose: Arc<Notify>,
    ) -> BackendTask {
        BackendTask {
            id: dl.id,
            url: dl.url.clone(),
            destination: dl.destination.clone(),
            overwrite: dl.overwrite,
            backend_meta: dl.backend_meta.clone(),
            progress: None,
            cancel: Some(cancel),
            rate_limiter: None,
            dispose: Some(dispose),
            global_max_bytes_per_sec: self.inner.config.bandwidth.max_bytes_per_sec,
        }
    }

    fn recover(&self) -> Result<()> {
        let downloads = self.inner.persistence.list()?;
        for dl in downloads {
            let recovered = match self.inner.config.recover_interrupted {
                RecoveryPolicy::Failed => match dl.state {
                    DownloadState::Starting | DownloadState::Downloading => {
                        let mut d = dl.clone();
                        d.state = DownloadState::Failed;
                        d.error = Some("interrupted by manager restart".to_string());
                        d.updated_at = SystemTime::now();
                        Some(d)
                    }
                    _ => None,
                },
                RecoveryPolicy::Queued => match dl.state {
                    DownloadState::Starting | DownloadState::Downloading => {
                        let mut d = dl.clone();
                        d.state = DownloadState::Queued;
                        d.started_at = None;
                        d.error = None;
                        d.updated_at = SystemTime::now();
                        Some(d)
                    }
                    _ => None,
                },
            };
            match recovered {
                Some(d) => {
                    let from = dl.state;
                    let to = d.state;
                    let id = d.id;
                    self.inner.persistence.save(&d)?;
                    self.inner
                        .events
                        .publish(Event::StateChanged { id, from, to });
                    if to == DownloadState::Failed {
                        let err = d.error.clone().unwrap_or_default();
                        self.inner.events.publish(Event::Failed { id, error: err });
                    } else if to == DownloadState::Queued {
                        self.inner.events.publish(Event::Queued(id));
                    }
                    self.inner
                        .downloads
                        .lock()
                        .unwrap()
                        .insert(id, ManagedDownload::new(d));
                    if to == DownloadState::Queued {
                        self.inner.queue.lock().unwrap().enqueue(id);
                    }
                }
                None => {
                    let id = dl.id;
                    let state = dl.state;
                    self.inner
                        .downloads
                        .lock()
                        .unwrap()
                        .insert(id, ManagedDownload::new(dl));
                    if state == DownloadState::Queued {
                        self.inner.queue.lock().unwrap().enqueue(id);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Bridges [`ProgressSink`] calls from a backend into the manager.
struct ManagerProgressSink {
    mgr: DownloadManager,
    id: DownloadId,
}

impl ProgressSink for ManagerProgressSink {
    fn on_progress(&self, p: DownloadProgress) {
        self.mgr.on_progress(self.id, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ManagerConfig;
    use odm_core::{BackendOutcome, BackendTask};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;

    struct FakeBackend {
        delay: Duration,
        fail_once: Arc<AtomicBool>,
    }

    struct StubbornBackend {
        running: Arc<AtomicBool>,
    }

    struct RunningGuard(Arc<AtomicBool>);

    impl Drop for RunningGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl Backend for StubbornBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Http
        }

        async fn run(&self, _task: BackendTask) -> Result<BackendOutcome> {
            self.running.store(true, Ordering::SeqCst);
            let _guard = RunningGuard(self.running.clone());
            std::future::pending().await
        }
    }

    impl FakeBackend {
        fn new(delay: Duration, fail: bool) -> Self {
            Self {
                delay,
                fail_once: Arc::new(AtomicBool::new(fail)),
            }
        }
    }

    #[async_trait::async_trait]
    impl Backend for FakeBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Http
        }

        async fn run(&self, task: BackendTask) -> Result<BackendOutcome> {
            if let Some(rl) = &task.rate_limiter {
                rl.acquire(1024).await;
            }
            tokio::select! {
                _ = tokio::time::sleep(self.delay) => {}
                _ = async {
                    if let Some(c) = &task.cancel {
                        c.notified().await;
                    }
                } => return Err(Error::Cancelled),
            }
            if let Some(p) = &task.progress {
                p.on_progress(DownloadProgress {
                    downloaded_bytes: 1024,
                    total_bytes: Some(1024),
                    at: SystemTime::now(),
                });
            }
            if self.fail_once.swap(false, Ordering::Relaxed) {
                return Err(Error::Network("forced failure".into()));
            }
            Ok(BackendOutcome {
                downloaded_bytes: 1024,
                total_bytes: Some(1024),
                backend_meta: Value::Null,
            })
        }
    }

    fn tmp_db() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("db.sqlite");
        (dir, path)
    }

    fn manager(delay: Duration, fail: bool, limit: usize) -> (DownloadManager, TempDir) {
        let (_dir, path) = tmp_db();
        let cfg = ManagerConfig {
            max_concurrent_downloads: limit,
            ..ManagerConfig::default()
        };
        let backend: Arc<dyn Backend> = Arc::new(FakeBackend::new(delay, fail));
        let mgr = DownloadManager::open(cfg, &path, vec![backend]).unwrap();
        (mgr, _dir)
    }

    fn spec(url: &str) -> DownloadSpec {
        DownloadSpec::new(
            Url::parse(url).unwrap(),
            std::path::PathBuf::from(format!("{url}.bin")),
        )
    }

    async fn wait_for<F: Fn(DownloadState) -> bool>(
        mgr: &DownloadManager,
        id: DownloadId,
        pred: F,
        seconds: u64,
    ) {
        for _ in 0..(seconds * 20) {
            if let Some(d) = mgr.get(id) {
                if pred(d.state) {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for download {id} state");
    }

    #[tokio::test]
    async fn scheduler_respects_concurrency_limit() {
        let (mgr, _dir) = manager(Duration::from_millis(200), false, 2);
        let ids: Vec<_> = (0..5)
            .map(|i| mgr.enqueue(spec(&format!("http://a.test/{i}"))).unwrap())
            .collect();

        // Right after enqueue, exactly the concurrency limit should be active
        // and the rest still queued (the scheduler starts synchronously).
        let active = mgr
            .list()
            .iter()
            .filter(|d| {
                matches!(
                    d.state,
                    DownloadState::Starting | DownloadState::Downloading
                )
            })
            .count();
        assert_eq!(active, 2, "exactly the concurrency limit should be active");
        let queued = mgr
            .list()
            .iter()
            .filter(|d| d.state == DownloadState::Queued)
            .count();
        assert_eq!(queued, 3);

        let mut rx = mgr.subscribe();
        let mut completed = 0;
        while completed < 5 {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Ok(Event::Completed(_))) => completed += 1,
                Ok(Ok(_)) => {}
                _ => break,
            }
        }
        assert_eq!(completed, 5);
        for id in &ids {
            assert_eq!(mgr.get(*id).unwrap().state, DownloadState::Completed);
        }
    }

    #[tokio::test]
    async fn queued_events_fire_in_order_with_zero_limit() {
        let (mgr, _dir) = manager(Duration::from_millis(50), false, 0);
        let mut rx = mgr.subscribe();
        let ids: Vec<_> = (0..3)
            .map(|i| mgr.enqueue(spec(&format!("http://a.test/{i}"))).unwrap())
            .collect();
        let mut queued = Vec::new();
        for _ in 0..3 {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Ok(Event::Queued(id))) => queued.push(id),
                _ => break,
            }
        }
        assert_eq!(queued, ids);
        // With a zero limit nothing ever started.
        assert!(mgr.list().iter().all(|d| d.state == DownloadState::Queued));
    }

    #[tokio::test]
    async fn pause_resume_requeues_and_completes() {
        let (mgr, _dir) = manager(Duration::from_millis(200), false, 2);
        let id = mgr.enqueue(spec("http://a.test/x")).unwrap();
        wait_for(
            &mgr,
            id,
            |s| matches!(s, DownloadState::Starting | DownloadState::Downloading),
            2,
        )
        .await;
        mgr.pause(id).unwrap();
        wait_for(&mgr, id, |s| s == DownloadState::Paused, 2).await;
        mgr.resume(id).unwrap();
        wait_for(&mgr, id, |s| s == DownloadState::Completed, 5).await;
    }

    #[tokio::test]
    async fn cancel_active_download() {
        let (mgr, _dir) = manager(Duration::from_millis(200), false, 2);
        let id = mgr.enqueue(spec("http://a.test/x")).unwrap();
        wait_for(
            &mgr,
            id,
            |s| matches!(s, DownloadState::Starting | DownloadState::Downloading),
            2,
        )
        .await;
        mgr.cancel(id).unwrap();
        wait_for(&mgr, id, |s| s == DownloadState::Cancelled, 2).await;
    }

    #[tokio::test]
    async fn failed_download_can_be_retried() {
        let (mgr, _dir) = manager(Duration::from_millis(50), true, 2);
        let id = mgr.enqueue(spec("http://a.test/x")).unwrap();
        wait_for(&mgr, id, |s| s == DownloadState::Failed, 2).await;
        mgr.retry(id).unwrap();
        wait_for(&mgr, id, |s| s == DownloadState::Completed, 5).await;
    }

    #[tokio::test]
    async fn shutdown_aborts_backend_that_ignores_cancellation() {
        let (_dir, path) = tmp_db();
        let running = Arc::new(AtomicBool::new(false));
        let backend: Arc<dyn Backend> = Arc::new(StubbornBackend {
            running: running.clone(),
        });
        let mgr = DownloadManager::open(ManagerConfig::default(), &path, vec![backend]).unwrap();
        mgr.enqueue(spec("http://a.test/stubborn")).unwrap();
        for _ in 0..20 {
            if running.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(running.load(Ordering::SeqCst));
        mgr.shutdown().await;
        assert!(!running.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn remove_from_queue_deletes_download() {
        let (mgr, _dir) = manager(Duration::from_millis(50), false, 0);
        let id = mgr.enqueue(spec("http://a.test/x")).unwrap();
        assert!(mgr.get(id).is_some());
        mgr.remove(id).unwrap();
        assert!(mgr.get(id).is_none());
    }

    #[tokio::test]
    async fn restart_recovers_interrupted_downloads_as_failed() {
        let (_dir, path) = tmp_db();
        let p = Persistence::open(&path).unwrap();
        p.save(&Download {
            id: DownloadId::new(),
            url: Url::parse("http://a.test/x").unwrap(),
            destination: std::path::PathBuf::from("x"),
            backend: BackendKind::Http,
            state: DownloadState::Downloading,
            overwrite: false,
            total_bytes: None,
            downloaded_bytes: 0,
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            started_at: Some(SystemTime::now()),
            completed_at: None,
            error: None,
            backend_meta: Value::Null,
        })
        .unwrap();
        p.save(&Download {
            id: DownloadId::new(),
            url: Url::parse("http://a.test/y").unwrap(),
            destination: std::path::PathBuf::from("y"),
            backend: BackendKind::Http,
            state: DownloadState::Queued,
            overwrite: false,
            total_bytes: None,
            downloaded_bytes: 0,
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            error: None,
            backend_meta: Value::Null,
        })
        .unwrap();
        drop(p);

        let backend: Arc<dyn Backend> =
            Arc::new(FakeBackend::new(Duration::from_millis(10), false));
        let mgr = DownloadManager::open(ManagerConfig::default(), &path, vec![backend]).unwrap();

        let list = mgr.list();
        let interrupted = list
            .iter()
            .find(|d| d.error.as_deref() == Some("interrupted by manager restart"));
        assert!(
            interrupted.is_some(),
            "in-flight download must be recovered"
        );
        assert!(
            mgr.list()
                .iter()
                .all(|d| d.state != DownloadState::Downloading),
            "no download may remain in-flight after recovery"
        );
        assert!(
            mgr.list().iter().any(|d| d.state == DownloadState::Queued),
            "a queued download must survive recovery"
        );
    }
}
