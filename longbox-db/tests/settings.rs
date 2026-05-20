mod common;

use common::fresh_pool;
use longbox_db::{settings_repo, KEY_LIBRARY_ROOT_PATH, KEY_MATCH_CONFIDENCE_THRESHOLD};

#[tokio::test]
async fn get_unset_returns_none() {
    let pool = fresh_pool().await;
    let v = settings_repo::get(&pool, KEY_LIBRARY_ROOT_PATH)
        .await
        .unwrap();
    assert!(v.is_none());
}

#[tokio::test]
async fn set_then_get_returns_value() {
    let pool = fresh_pool().await;
    settings_repo::set(&pool, KEY_LIBRARY_ROOT_PATH, "/comics")
        .await
        .unwrap();
    let v = settings_repo::get(&pool, KEY_LIBRARY_ROOT_PATH)
        .await
        .unwrap();
    assert_eq!(v.as_deref(), Some("/comics"));
}

#[tokio::test]
async fn set_twice_keeps_latest_value() {
    let pool = fresh_pool().await;
    settings_repo::set(&pool, KEY_LIBRARY_ROOT_PATH, "/a")
        .await
        .unwrap();
    settings_repo::set(&pool, KEY_LIBRARY_ROOT_PATH, "/b")
        .await
        .unwrap();
    let v = settings_repo::get(&pool, KEY_LIBRARY_ROOT_PATH)
        .await
        .unwrap();
    assert_eq!(v.as_deref(), Some("/b"));
}

#[tokio::test]
async fn seed_threshold_is_present() {
    let pool = fresh_pool().await;
    let v = settings_repo::get(&pool, KEY_MATCH_CONFIDENCE_THRESHOLD)
        .await
        .unwrap();
    assert_eq!(v.as_deref(), Some("0.85"));
}

#[tokio::test]
async fn get_or_default_returns_default_for_missing() {
    let pool = fresh_pool().await;
    let v: f64 = settings_repo::get_or_default(&pool, KEY_LIBRARY_ROOT_PATH, 0.85_f64)
        .await
        .unwrap();
    assert!((v - 0.85).abs() < 1e-9);
}

#[tokio::test]
async fn get_or_default_parses_stored_value() {
    let pool = fresh_pool().await;
    let v: f64 = settings_repo::get_or_default(&pool, KEY_MATCH_CONFIDENCE_THRESHOLD, 0.50_f64)
        .await
        .unwrap();
    assert!((v - 0.85).abs() < 1e-9);
}

#[tokio::test]
async fn get_or_default_falls_back_on_unparseable() {
    let pool = fresh_pool().await;
    settings_repo::set(&pool, "weird_key", "not_a_number")
        .await
        .unwrap();
    let v: f64 = settings_repo::get_or_default(&pool, "weird_key", 0.50_f64)
        .await
        .unwrap();
    assert!((v - 0.50).abs() < 1e-9);
}
