//! Shared test harness: in-memory pool + tempdir library + wiremock CV +
//! Scanner, composed into an AppState and a Router we can drive with
//! tower::ServiceExt::oneshot.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use longbox_comicvine::{ComicVineClient, ComicVineClientConfig};
use longbox_db::{library_root_repo, NewLibraryRoot, Pool};
use longbox_scanner::{Scanner, ScannerConfig};
use longbox_web::{build_router, AppConfig, AppState, ScanStatus};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wiremock::MockServer;

/// One self-contained app instance for a test. Holds the TempDir alive so
/// the library doesn't get reaped while the test is running.
pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub cv_server: MockServer,
    #[allow(dead_code)]
    pub library_dir: TempDir,
    pub library_root_id: i64,
}

impl TestApp {
    pub async fn request(&self, req: Request<Body>) -> axum::response::Response {
        // Cloning the Router is cheap (Arc internals).
        self.router.clone().oneshot(req).await.unwrap()
    }

    pub fn library_path(&self) -> PathBuf {
        self.library_dir.path().to_path_buf()
    }
}

pub async fn build_test_app() -> TestApp {
    let pool: Pool = longbox_db::open(":memory:").await.unwrap();
    let library_dir = TempDir::new().unwrap();
    let library_path = library_dir.path().to_string_lossy().to_string();

    let library_root_id = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: library_path.clone(),
        },
    )
    .await
    .unwrap()
    .id;

    let cv_server = MockServer::start().await;
    let cv = ComicVineClient::new(ComicVineClientConfig {
        api_key: "test-key".into(),
        base_url: format!("{}/", cv_server.uri()),
        timeout: Duration::from_secs(2),
        // Effectively unbounded for tests.
        rate_limit_per_hour: 360_000,
        max_wait_for_slot: Duration::from_secs(1),
        user_agent: "longbox-test/0.0".into(),
    })
    .unwrap();

    let scanner = Scanner::new(pool.clone(), ScannerConfig::default());

    let config = AppConfig {
        comicvine_api_key: "test-key".into(),
        library_root_path: library_path,
        database_url: "sqlite::memory:".into(),
        bind_addr: "0.0.0.0:0".into(),
        log_level: "info".into(),
        match_threshold: 0.85,
        cors_permissive: false,
        download_watch_path: None,
        pull_schedule_time: time::macros::time!(05:00),
        scan_schedule_time: time::macros::time!(03:00),
    };

    // The pull + scan schedulers run in every test app; with their
    // default slots they just sleep — tests never wait on them. The
    // pull handle is what the `/pull/check` route exercises; the scan
    // scheduler gets a no-op closure.
    let pull = longbox_pull::start(longbox_pull::PullConfig::default(), pool.clone());
    let scan_scheduler = longbox_scan_scheduler::start(
        longbox_scan_scheduler::ScanSchedulerConfig::default(),
        || async {},
    );
    let cv_arc = Arc::new(cv);
    let enrichment = longbox_cv_enrichment::spawn(pool.clone(), Arc::clone(&cv_arc));

    let state = AppState {
        db: pool,
        cv: cv_arc,
        scanner: Arc::new(scanner),
        config: Arc::new(config),
        scan_status: Arc::new(RwLock::new(ScanStatus::default())),
        library_root_id,
        pending_cache: Arc::new(longbox_postprocess::PendingInterventionsCache::new()),
        pull,
        scan_scheduler,
        enrichment,
    };

    let router = build_router(state.clone());

    TestApp {
        router,
        state,
        cv_server,
        library_dir,
        library_root_id,
    }
}

/// Build an HTTP request with a JSON body.
pub fn json_request<S: AsRef<str>>(method: &str, uri: &str, body: S) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.as_ref().to_owned()))
        .unwrap()
}

pub fn empty_request(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

pub async fn response_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
