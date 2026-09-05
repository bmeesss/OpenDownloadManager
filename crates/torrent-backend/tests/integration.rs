use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use odm_core::{
    Backend, BackendKind, BackendTask, DownloadProgress, DownloadState, Error, ProgressSink,
};
use odm_download_manager::{BandwidthPolicy, DownloadManager, DownloadSpec, ManagerConfig};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use url::Url;

use librqbit::{
    AddTorrent, AddTorrentOptions, ListenerOptions, Session, SessionOptions, TorrentStatsState,
};
use odm_torrent_backend::{BackendMeta, TorrentBackend};

const WAIT: Duration = Duration::from_secs(45);

struct Progress(Arc<std::sync::Mutex<Vec<DownloadProgress>>>);

impl ProgressSink for Progress {
    fn on_progress(&self, progress: DownloadProgress) {
        self.0.lock().unwrap().push(progress);
    }
}

struct Fixture {
    _root: TempDir,
    seed: Arc<Session>,
    torrent: Vec<u8>,
    info_hash: String,
    peers: Vec<std::net::SocketAddr>,
    files: Vec<(PathBuf, Vec<u8>)>,
}

async fn fixture(multi_file: bool) -> Fixture {
    let root = TempDir::new().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let files = if multi_file {
        vec![
            (PathBuf::from("one.bin"), bytes(96 * 1024, 11)),
            (PathBuf::from("nested/two.bin"), bytes(96 * 1024, 29)),
        ]
    } else {
        vec![(PathBuf::from("single.bin"), bytes(192 * 1024, 7))]
    };
    for (name, data) in &files {
        let path = source.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    let torrent_path = if multi_file {
        source.clone()
    } else {
        source.join(&files[0].0)
    };
    let torrent = librqbit::create_torrent(
        &torrent_path,
        librqbit::CreateTorrentOptions {
            piece_length: Some(16 * 1024),
            ..Default::default()
        },
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let torrent_bytes = torrent.as_bytes().unwrap().to_vec();
    let info_hash = torrent.info_hash().as_string();

    let seed = Session::new_with_opts(
        root.path().join("seed-session"),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            listen: Some(ListenerOptions {
                listen_addr: ([127, 0, 0, 1], 0).into(),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let seed_output = source.clone();
    let seed_handle = seed
        .add_torrent(
            AddTorrent::from_bytes(torrent_bytes.clone()),
            Some(AddTorrentOptions {
                overwrite: true,
                output_folder: Some(seed_output.to_string_lossy().into_owned()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();
    tokio::time::timeout(WAIT, seed_handle.wait_until_completed())
        .await
        .unwrap()
        .unwrap();
    Fixture {
        peers: vec![seed.listen_addr().unwrap()],
        _root: root,
        seed,
        torrent: torrent_bytes,
        info_hash,
        files,
    }
}

fn bytes(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

fn torrent_url(fixture: &Fixture) -> (TempDir, Url) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("fixture.torrent");
    std::fs::write(&path, &fixture.torrent).unwrap();
    (dir, Url::from_file_path(path).unwrap())
}

fn task(url: Url, progress: Option<Arc<dyn ProgressSink>>) -> BackendTask {
    BackendTask {
        id: odm_core::DownloadId::new(),
        url,
        destination: PathBuf::from("unused"),
        overwrite: true,
        backend_meta: serde_json::Value::Null,
        progress,
        cancel: Some(Arc::new(Notify::new())),
        rate_limiter: None,
        dispose: Some(Arc::new(Notify::new())),
        global_max_bytes_per_sec: Some(32 * 1024),
    }
}

fn raw_torrent(files: &[(&str, u64)]) -> Vec<u8> {
    let mut info = Vec::new();
    info.extend_from_slice(b"d5:filesl");
    for (path, length) in files {
        info.extend_from_slice(b"d6:lengthi");
        info.extend_from_slice(length.to_string().as_bytes());
        info.extend_from_slice(b"e4:pathl");
        info.extend_from_slice(path.len().to_string().as_bytes());
        info.push(b':');
        info.extend_from_slice(path.as_bytes());
        info.extend_from_slice(b"eee");
    }
    info.extend_from_slice(b"4:name4:test12:piece lengthi16384e6:pieces20:");
    info.extend_from_slice(&[0; 20]);
    info.push(b'e');
    let mut torrent = b"d4:info".to_vec();
    torrent.extend_from_slice(info.len().to_string().as_bytes());
    torrent.push(b':');
    torrent.extend_from_slice(&info);
    torrent.push(b'e');
    torrent
}

async fn expect_rejected_torrent(bytes: Vec<u8>) {
    let root = TempDir::new().unwrap();
    let torrent_file = root.path().join("invalid.torrent");
    std::fs::write(&torrent_file, bytes).unwrap();
    let backend = TorrentBackend::new(root.path().join("output"))
        .await
        .unwrap();
    let result = backend
        .run(task(Url::from_file_path(torrent_file).unwrap(), None))
        .await;
    assert!(matches!(result, Err(Error::Internal(_))));
}

async fn backend(fixture: &Fixture) -> (TorrentBackend, TempDir) {
    let output = TempDir::new().unwrap();
    (
        TorrentBackend::new_with_initial_peers(output.path().to_path_buf(), fixture.peers.clone())
            .await
            .unwrap(),
        output,
    )
}

async fn local_tracker(peer: std::net::SocketAddr) -> (Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = Url::parse(&format!(
        "http://{}/announce",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await;
            let port = peer.port().to_be_bytes();
            let mut body = b"d8:intervali1e5:peers6:\x7f\x00\x00\x01".to_vec();
            body.extend_from_slice(&port);
            body.push(b'e');
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(&body).await;
        }
    });
    (url, handle)
}

fn assert_files(fixture: &Fixture, output: &Path) {
    for (name, expected) in &fixture.files {
        assert_eq!(
            std::fs::read(
                output
                    .join(format!("odm-{}/", fixture.info_hash))
                    .join(name)
            )
            .unwrap(),
            *expected
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn single_file_torrent_downloads() {
    let fixture = fixture(false).await;
    let (backend, output) = backend(&fixture).await;
    let (_torrent_dir, url) = torrent_url(&fixture);
    let outcome = tokio::time::timeout(WAIT, backend.run(task(url, None)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.downloaded_bytes, fixture.files[0].1.len() as u64);
    assert_files(&fixture, output.path());
    assert!(!backend.has_active_torrent(&fixture.info_hash));
    fixture.seed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_file_torrent_downloads() {
    let fixture = fixture(true).await;
    let (backend, output) = backend(&fixture).await;
    let (_torrent_dir, url) = torrent_url(&fixture);
    tokio::time::timeout(WAIT, backend.run(task(url, None)))
        .await
        .unwrap()
        .unwrap();
    assert_files(&fixture, output.path());
    fixture.seed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn magnet_list_only_resolves_and_adds_once() {
    let fixture = fixture(false).await;
    let output = TempDir::new().unwrap();
    let backend =
        TorrentBackend::new_with_initial_peers(output.path().to_path_buf(), fixture.peers.clone())
            .await
            .unwrap();
    let (tracker, tracker_task) = local_tracker(fixture.peers[0]).await;
    let mut magnet = Url::parse(&format!("magnet:?xt=urn:btih:{}", fixture.info_hash)).unwrap();
    magnet.query_pairs_mut().append_pair("tr", tracker.as_str());
    let outcome = tokio::time::timeout(WAIT, backend.run(task(magnet, None)))
        .await
        .unwrap()
        .unwrap();
    let meta: BackendMeta = serde_json::from_value(outcome.backend_meta).unwrap();
    assert_eq!(meta.source, odm_torrent_backend::TorrentSource::Magnet);
    assert!(!backend.has_active_torrent(&fixture.info_hash));
    tracker_task.abort();
    fixture.seed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_keeps_partial_data() {
    let fixture = fixture(false).await;
    let (backend, output) = backend(&fixture).await;
    let (_torrent_dir, url) = torrent_url(&fixture);
    let progress = Arc::new(std::sync::Mutex::new(Vec::new()));
    let task = task(url, Some(Arc::new(Progress(progress.clone()))));
    let cancel = task.cancel.clone().unwrap();
    let backend = Arc::new(backend);
    let run = tokio::spawn({
        let backend = backend.clone();
        async move { backend.run(task).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.notify_one();
    assert!(matches!(
        tokio::time::timeout(WAIT, run).await.unwrap().unwrap(),
        Err(Error::Cancelled)
    ));
    assert!(backend.has_active_torrent(&fixture.info_hash));
    assert!(!matches!(
        backend.torrent_state(&fixture.info_hash),
        Some(TorrentStatsState::Live)
    ));
    assert!(output
        .path()
        .join(format!("odm-{}/single.bin", fixture.info_hash))
        .exists());
    fixture.seed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_cancel_keeps_data_and_marks_cancelled() {
    let fixture = fixture(false).await;
    let (backend, output) = backend(&fixture).await;
    let backend = Arc::new(backend);
    let (_torrent_dir, url) = torrent_url(&fixture);
    let db = output.path().join("manager.sqlite");
    let manager = DownloadManager::open(
        ManagerConfig {
            max_concurrent_downloads: 1,
            bandwidth: BandwidthPolicy::new(Some(16 * 1024)),
            ..Default::default()
        },
        &db,
        vec![backend.clone()],
    )
    .unwrap();
    let id = manager
        .enqueue(DownloadSpec {
            url,
            destination: output.path().join("unused"),
            backend: BackendKind::Torrent,
            overwrite: true,
            backend_meta: serde_json::Value::Null,
        })
        .unwrap();
    wait_state_any(
        &manager,
        id,
        &[DownloadState::Starting, DownloadState::Downloading],
    )
    .await;
    manager.cancel(id).unwrap();
    wait_state(&manager, id, DownloadState::Cancelled).await;
    assert!(output
        .path()
        .join(format!("odm-{}/single.bin", fixture.info_hash))
        .exists());
    assert!(!matches!(
        backend.torrent_state(&fixture.info_hash),
        Some(TorrentStatsState::Live)
    ));
    fixture.seed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_deletes_owned_files_only() {
    let fixture = fixture(false).await;
    let (backend, output) = backend(&fixture).await;
    let (_torrent_dir, url) = torrent_url(&fixture);
    let task = task(url, None);
    let dispose = task.dispose.clone().unwrap();
    let backend = Arc::new(backend);
    let run = tokio::spawn({
        let backend = backend.clone();
        async move { backend.run(task).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let folder = output.path().join(format!("odm-{}", fixture.info_hash));
    let unrelated = folder.join("unrelated.txt");
    std::fs::write(&unrelated, b"keep me").unwrap();
    dispose.notify_one();
    assert!(matches!(
        tokio::time::timeout(WAIT, run).await.unwrap().unwrap(),
        Err(Error::Cancelled)
    ));
    assert!(!folder.join("single.bin").exists());
    assert_eq!(std::fs::read(unrelated).unwrap(), b"keep me");
    assert!(!backend.has_active_torrent(&fixture.info_hash));
    fixture.seed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_remove_disposes_files_and_database_row() {
    let fixture = fixture(false).await;
    let (backend, output) = backend(&fixture).await;
    let backend = Arc::new(backend);
    let (_torrent_dir, url) = torrent_url(&fixture);
    let db = output.path().join("manager.sqlite");
    let manager = DownloadManager::open(
        ManagerConfig {
            bandwidth: BandwidthPolicy::new(Some(16 * 1024)),
            ..Default::default()
        },
        &db,
        vec![backend.clone()],
    )
    .unwrap();
    let id = manager
        .enqueue(DownloadSpec {
            url,
            destination: output.path().join("unused"),
            backend: BackendKind::Torrent,
            overwrite: true,
            backend_meta: serde_json::Value::Null,
        })
        .unwrap();
    wait_state_any(
        &manager,
        id,
        &[DownloadState::Starting, DownloadState::Downloading],
    )
    .await;
    let folder = output.path().join(format!("odm-{}", fixture.info_hash));
    tokio::time::timeout(WAIT, async {
        while !folder.join("single.bin").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let unrelated = folder.join("unrelated.txt");
    std::fs::write(&unrelated, b"keep").unwrap();
    manager.remove(id).unwrap();
    assert!(manager.get(id).is_none());
    let reopened = DownloadManager::open(ManagerConfig::default(), &db, Vec::new()).unwrap();
    assert!(reopened.get(id).is_none());
    tokio::time::timeout(WAIT, async {
        loop {
            if !folder.join("single.bin").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read(unrelated).unwrap(), b"keep");
    assert!(!backend.has_active_torrent(&fixture.info_hash));
    fixture.seed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_pause_cancel_and_completion_lifecycle() {
    let fixture = fixture(false).await;
    let (backend, output) = backend(&fixture).await;
    let (_torrent_dir, url) = torrent_url(&fixture);
    let db = output.path().join("manager.sqlite");
    let backend = Arc::new(backend);
    let manager = DownloadManager::open(
        ManagerConfig {
            max_concurrent_downloads: 1,
            bandwidth: BandwidthPolicy::new(Some(16 * 1024)),
            ..Default::default()
        },
        &db,
        vec![backend.clone()],
    )
    .unwrap();
    let id = manager
        .enqueue(DownloadSpec {
            url,
            destination: output.path().join("ignored"),
            backend: BackendKind::Torrent,
            overwrite: true,
            backend_meta: serde_json::Value::Null,
        })
        .unwrap();
    wait_state_any(
        &manager,
        id,
        &[DownloadState::Starting, DownloadState::Downloading],
    )
    .await;
    manager.pause(id).unwrap();
    wait_state(&manager, id, DownloadState::Paused).await;
    assert!(!matches!(
        backend.torrent_state(&fixture.info_hash),
        Some(TorrentStatsState::Live)
    ));
    assert!(output
        .path()
        .join(format!("odm-{}/single.bin", fixture.info_hash))
        .exists());
    manager.resume(id).unwrap();
    wait_state(&manager, id, DownloadState::Completed).await;
    assert!(!backend.has_active_torrent(&fixture.info_hash));
    fixture.seed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_shutdown_leaves_no_active_torrent() {
    let fixture = fixture(false).await;
    let (backend, output) = backend(&fixture).await;
    let backend = Arc::new(backend);
    let (_torrent_dir, url) = torrent_url(&fixture);
    let manager = DownloadManager::open(
        ManagerConfig::default(),
        &output.path().join("shutdown.sqlite"),
        vec![backend.clone()],
    )
    .unwrap();
    let _id = manager
        .enqueue(DownloadSpec {
            url,
            destination: output.path().join("unused"),
            backend: BackendKind::Torrent,
            overwrite: true,
            backend_meta: serde_json::Value::Null,
        })
        .unwrap();
    tokio::time::timeout(WAIT, manager.shutdown())
        .await
        .unwrap();
    assert!(!matches!(
        backend.torrent_state(&fixture.info_hash),
        Some(TorrentStatsState::Live)
    ));
    fixture.seed.stop().await;
}

async fn wait_state(manager: &DownloadManager, id: odm_core::DownloadId, state: DownloadState) {
    tokio::time::timeout(WAIT, async {
        loop {
            if manager.get(id).map(|d| d.state) == Some(state) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for {state:?}, current={:?}",
            manager.get(id).map(|d| (d.state, d.error))
        )
    });
}

async fn wait_state_any(
    manager: &DownloadManager,
    id: odm_core::DownloadId,
    states: &[DownloadState],
) {
    tokio::time::timeout(WAIT, async {
        loop {
            if manager.get(id).is_some_and(|d| states.contains(&d.state)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn backend_metadata_round_trips_and_reuses_complete_data() {
    let fixture = fixture(false).await;
    let (backend, output) = backend(&fixture).await;
    let (_torrent_dir, url) = torrent_url(&fixture);
    let outcome = tokio::time::timeout(WAIT, backend.run(task(url.clone(), None)))
        .await
        .unwrap()
        .unwrap();
    let meta: BackendMeta = serde_json::from_value(outcome.backend_meta).unwrap();
    let db = output.path().join("meta.sqlite");
    let manager = DownloadManager::open(
        ManagerConfig {
            max_concurrent_downloads: 0,
            ..Default::default()
        },
        &db,
        vec![Arc::new(backend)],
    )
    .unwrap();
    let id = manager
        .enqueue(DownloadSpec {
            url,
            destination: output.path().join("unused"),
            backend: BackendKind::Torrent,
            overwrite: true,
            backend_meta: serde_json::to_value(&meta).unwrap(),
        })
        .unwrap();
    let backend =
        TorrentBackend::new_with_initial_peers(output.path().to_path_buf(), fixture.peers.clone())
            .await
            .unwrap();
    let reopened =
        DownloadManager::open(ManagerConfig::default(), &db, vec![Arc::new(backend)]).unwrap();
    let loaded = reopened.get(id).unwrap();
    let loaded_meta: BackendMeta = serde_json::from_value(loaded.backend_meta).unwrap();
    assert_eq!(loaded_meta.info_hash, fixture.info_hash);
    assert_eq!(
        loaded_meta.source,
        odm_torrent_backend::TorrentSource::TorrentFile
    );
    reopened.start(id).unwrap();
    wait_state(&reopened, id, DownloadState::Completed).await;
    fixture.seed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn initialization_hash_check_does_not_report_network_progress() {
    let fixture = fixture(false).await;
    let output = TempDir::new().unwrap();
    let folder = output.path().join(format!("odm-{}", fixture.info_hash));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join(".odm-owned"),
        format!("odm-torrent-v1:{}\n", fixture.info_hash),
    )
    .unwrap();
    std::fs::write(folder.join("single.bin"), &fixture.files[0].1).unwrap();
    let backend =
        TorrentBackend::new_with_initial_peers(output.path().to_path_buf(), fixture.peers.clone())
            .await
            .unwrap();
    let (_torrent_dir, url) = torrent_url(&fixture);
    let progress = Arc::new(std::sync::Mutex::new(Vec::new()));
    tokio::time::timeout(
        WAIT,
        backend.run(task(url, Some(Arc::new(Progress(progress.clone()))))),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(progress.lock().unwrap().is_empty());
    fixture.seed.stop().await;
}

#[tokio::test]
async fn unsafe_torrent_paths_are_rejected() {
    expect_rejected_torrent(raw_torrent(&[("../escape.bin", 1)])).await;
    expect_rejected_torrent(raw_torrent(&[("/absolute.bin", 1)])).await;
    expect_rejected_torrent(raw_torrent(&[("CON.txt", 1)])).await;
    expect_rejected_torrent(raw_torrent(&[("A.bin", 1), ("a.bin", 1)])).await;
}

#[cfg(unix)]
#[tokio::test]
async fn backend_rejects_symlink_output_escape() {
    let fixture = fixture(false).await;
    let output = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let folder = output.path().join(format!("odm-{}", fixture.info_hash));
    std::os::unix::fs::symlink(outside.path(), &folder).unwrap();
    let backend =
        TorrentBackend::new_with_initial_peers(output.path().to_path_buf(), fixture.peers.clone())
            .await
            .unwrap();
    let (_torrent_dir, url) = torrent_url(&fixture);
    assert!(matches!(
        backend.run(task(url, None)).await,
        Err(Error::Internal(_))
    ));
    fixture.seed.stop().await;
}

#[cfg(windows)]
#[tokio::test]
async fn backend_rejects_junction_output_escape() {
    let fixture = fixture(false).await;
    let output = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let folder = output.path().join(format!("odm-{}", fixture.info_hash));
    if std::os::windows::fs::symlink_dir(outside.path(), &folder).is_err() {
        return;
    }
    let backend =
        TorrentBackend::new_with_initial_peers(output.path().to_path_buf(), fixture.peers.clone())
            .await
            .unwrap();
    let (_torrent_dir, url) = torrent_url(&fixture);
    assert!(matches!(
        backend.run(task(url, None)).await,
        Err(Error::Internal(_))
    ));
    fixture.seed.stop().await;
}
