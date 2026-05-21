//! Live detection test: stand up the watcher against a real tempdir,
//! drop files into it, verify the consumer sees them. Bridges the unit
//! tests (pure logic) and the production deploy (Docker bind mount).
//!
//! Approach: replace the standard consumer with a test sink that
//! forwards detected paths to a channel the test can read. The setup
//! mirrors `start()` but parameterized so the test owns the sink.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use longbox_postprocess::skip;
use notify::{RecursiveMode, Watcher};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Re-implementation of the production wiring, with the consumer task
/// piped into `detected_tx` so the test can observe.
async fn spawn_live_watcher(
    watch_path: PathBuf,
    detected_tx: mpsc::Sender<PathBuf>,
) -> Result<notify::RecommendedWatcher, notify::Error> {
    let tx = Arc::new(detected_tx);
    let tx_for_cb = Arc::clone(&tx);

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let event = match res {
            Ok(e) => e,
            Err(_) => return,
        };
        use notify::EventKind;
        let paths = match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => event.paths.clone(),
            _ => Vec::new(),
        };
        for path in paths {
            if skip::should_skip(&path).is_some() {
                continue;
            }
            let _ = tx_for_cb.try_send(path);
        }
    })?;
    watcher.watch(&watch_path, RecursiveMode::Recursive)?;
    Ok(watcher)
}

#[tokio::test]
async fn watcher_detects_cbz_created_after_start() {
    let tmp = TempDir::new().unwrap();
    let (detected_tx, mut detected_rx) = mpsc::channel::<PathBuf>(16);
    // Keep the watcher alive for the duration of the test by binding
    // it to a local — dropping it stops the watch.
    let _watcher = spawn_live_watcher(tmp.path().to_path_buf(), detected_tx)
        .await
        .unwrap();

    // notify can drop events that arrive synchronously with watcher
    // setup on some platforms; small grace period before producing.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let target = tmp.path().join("Saga 001.cbz");
    std::fs::write(&target, b"PK\x03\x04").unwrap(); // tiny ZIP signature

    // Wait up to 2s for the watcher to fire. notify is async on every
    // platform; 2s is generous on macOS / Linux and matches the
    // production polling-fallback interval ceiling.
    let detected = timeout(Duration::from_secs(2), detected_rx.recv()).await;
    assert!(detected.is_ok(), "watcher did not fire within 2s");
    let path = detected.unwrap().expect("channel closed unexpectedly");
    assert!(
        path.ends_with("Saga 001.cbz"),
        "expected to detect Saga 001.cbz, got {path:?}"
    );
}

#[tokio::test]
async fn watcher_ignores_in_progress_and_dotfiles() {
    let tmp = TempDir::new().unwrap();
    let (detected_tx, mut detected_rx) = mpsc::channel::<PathBuf>(16);
    let _watcher = spawn_live_watcher(tmp.path().to_path_buf(), detected_tx)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Drop a noise file + a real candidate. The noise file should be
    // skipped at the watcher boundary; the candidate should arrive.
    std::fs::write(tmp.path().join(".DS_Store"), []).unwrap();
    std::fs::write(tmp.path().join("Foo.cbz.partial"), []).unwrap();
    std::fs::write(tmp.path().join("Real Series 001.cbz"), b"PK\x03\x04").unwrap();

    let detected = timeout(Duration::from_secs(2), detected_rx.recv())
        .await
        .expect("watcher did not fire within 2s")
        .expect("channel closed");
    assert!(detected.ends_with("Real Series 001.cbz"));

    // Drain any subsequent events for up to 250ms. None should be the
    // noise files; if they were, the skip filter regressed.
    while let Ok(Some(extra)) = timeout(Duration::from_millis(250), detected_rx.recv()).await {
        let name = extra.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !name.starts_with('.') && !name.ends_with(".partial"),
            "noise file leaked through skip filter: {name}"
        );
    }
}

#[tokio::test]
async fn watcher_detects_files_in_nested_subdirectory() {
    // SAB creates a per-job subfolder under the watch root; the
    // recursive mode must follow into it.
    let tmp = TempDir::new().unwrap();
    let subdir = tmp.path().join("SAB-job-12345");
    std::fs::create_dir(&subdir).unwrap();

    let (detected_tx, mut detected_rx) = mpsc::channel::<PathBuf>(16);
    let _watcher = spawn_live_watcher(tmp.path().to_path_buf(), detected_tx)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    std::fs::write(subdir.join("Nested 001.cbz"), b"PK\x03\x04").unwrap();

    let detected = timeout(Duration::from_secs(2), detected_rx.recv())
        .await
        .expect("watcher did not fire within 2s")
        .expect("channel closed");
    assert!(detected.ends_with("Nested 001.cbz"));
    // And the parent should be the per-job subfolder.
    assert!(
        detected.to_string_lossy().contains("SAB-job-12345"),
        "expected nested path, got {detected:?}"
    );
}
