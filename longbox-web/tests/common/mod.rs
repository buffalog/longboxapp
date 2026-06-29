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
use longbox_metron::{MetronClient, MetronClientConfig};
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
    /// Temp directory backing `config.opds_covers_dir` so the OPDS cover
    /// proxy's write-behind cache lands somewhere disposable.
    #[allow(dead_code)]
    pub covers_dir: TempDir,
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

    /// Replace `config.host_library_path` and rebuild the router.
    /// Used by the Show-in-Finder tests to exercise the env-var
    /// fallback path that consumers of the `host_library_path`
    /// setting use when the DB row is absent.
    #[allow(dead_code)]
    pub fn set_host_library_path_fallback(&mut self, path: Option<String>) {
        let mut config = (*self.state.config).clone();
        config.host_library_path = path;
        self.state.config = Arc::new(config);
        self.router = build_router(self.state.clone());
    }

    /// Override `config.opds_base_url` and rebuild the router. Used to
    /// exercise the empty-base default (relative hrefs).
    #[allow(dead_code)]
    pub fn set_opds_base_url(&mut self, base: &str) {
        let mut config = (*self.state.config).clone();
        config.opds_base_url = base.to_owned();
        self.state.config = Arc::new(config);
        self.router = build_router(self.state.clone());
    }

    /// Toggle the OPDS cover-proxy SSRF guard and rebuild the router.
    /// The guard is relaxed by default in tests (the mock CDN binds
    /// loopback); the SSRF test flips it on to exercise the refusal path.
    #[allow(dead_code)]
    pub fn set_allow_private_cover_hosts(&mut self, allow: bool) {
        let mut config = (*self.state.config).clone();
        config.opds_allow_private_cover_hosts = allow;
        self.state.config = Arc::new(config);
        self.router = build_router(self.state.clone());
    }

    /// Wire up a Metron client backed by `server` and rebuild the
    /// router from the modified state. Subsequent requests route
    /// through the new client. Item A v2 piece-3 tests use this to
    /// stand up a `state.metron = Some(...)` configuration without
    /// duplicating the full `build_test_app` setup body.
    #[allow(dead_code)]
    pub fn enable_metron(&mut self, server: &MockServer) {
        let metron = MetronClient::new(MetronClientConfig {
            username: "test_user".into(),
            password: "test_pass".into(),
            base_url: format!("{}/", server.uri()),
            timeout: Duration::from_secs(2),
            connect_timeout: Duration::from_secs(1),
            rate_limit_per_hour: 360_000,
            max_wait_for_slot: Duration::from_secs(1),
            user_agent: "longbox-test/0.0".into(),
        })
        .unwrap();
        self.state.metron = Some(Arc::new(metron));
        self.router = build_router(self.state.clone());
    }
}

pub async fn build_test_app() -> TestApp {
    let pool: Pool = longbox_db::open(":memory:").await.unwrap();
    let library_dir = TempDir::new().unwrap();
    let library_path = library_dir.path().to_string_lossy().to_string();
    let covers_dir = TempDir::new().unwrap();

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
        connect_timeout: Duration::from_secs(1),
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
        metron_api_user: None,
        metron_api_password: None,
        pull_schedule_time: time::macros::time!(05:00),
        scan_schedule_time: time::macros::time!(03:00),
        host_library_path: None,
        opds_base_url: "http://opds.test".to_owned(),
        opds_covers_dir: covers_dir.path().to_path_buf(),
        // Tests' mock CDN binds loopback; allow it. The dedicated SSRF
        // test drives `is_safe_cover_url` directly with the guard on.
        opds_allow_private_cover_hosts: true,
        opds_port: 8096,
    };

    // The pull + scan schedulers run in every test app; with their
    // default slots they just sleep — tests never wait on them. The
    // pull handle is what the `/pull/check` route exercises; the scan
    // scheduler gets a no-op closure.
    let pull = longbox_pull::start(longbox_pull::PullConfig::default(), pool.clone());
    let pull_search = longbox_pull::PullSearchHandle::new(pool.clone());
    let scan_scheduler = longbox_scan_scheduler::start(
        longbox_scan_scheduler::ScanSchedulerConfig::default(),
        || async {},
    );
    let cv_arc = Arc::new(cv);
    // Tests use the same wiremock server for cv_direct — rate limit is moot in tests.
    let cv_direct = ComicVineClient::new(ComicVineClientConfig {
        api_key: "test-key".to_string(),
        base_url: format!("{}/", cv_server.uri()),
        max_wait_for_slot: Duration::from_secs(3),
        ..Default::default()
    })
    .unwrap();
    let cv_direct_arc = Arc::new(cv_direct);
    let enrichment = longbox_cv_enrichment::spawn(
        pool.clone(),
        Arc::clone(&cv_arc),
        std::path::PathBuf::from("/tmp/longbox-test-library"),
    );

    let state = AppState {
        db: pool,
        cv: cv_arc,
        cv_direct: cv_direct_arc,
        // Item A v2: tests don't exercise the Metron path. Disabled is
        // the default — piece 3 will add tests that explicitly construct
        // a Some(client) backed by a wiremock server for forward-week
        // calendar behavior.
        metron: None,
        scanner: Arc::new(scanner),
        config: Arc::new(config),
        scan_status: Arc::new(RwLock::new(ScanStatus::default())),
        library_root_id,
        pending_cache: Arc::new(longbox_postprocess::PendingInterventionsCache::new()),
        pull,
        pull_search,
        scan_scheduler,
        enrichment,
        start_time: time::OffsetDateTime::now_utc(),
        cv_resolve_gate: Arc::new(tokio::sync::Semaphore::new(3)),
        subscribe_status: Arc::new(RwLock::new(longbox_web::SubscribeStatus::default())),
    };

    let router = build_router(state.clone());

    TestApp {
        router,
        state,
        cv_server,
        library_dir,
        covers_dir,
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

#[allow(dead_code)]
pub async fn response_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}
