//! Per-user OPDS account repo + the credential-carry-over migration.

mod common;

use common::fresh_pool;
use longbox_db::{opds_users_repo, DbError};
use sqlx::sqlite::SqlitePoolOptions;

/// bcrypt-shaped placeholder. The repo never inspects the hash; auth-path
/// verification lives in longbox-opds, so a literal stand-in is fine here.
const HASH_A: &str = "$2b$12$aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "$2b$12$bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[tokio::test]
async fn create_then_list_roundtrips() {
    let pool = fresh_pool().await;
    let created = opds_users_repo::create(&pool, "Judd", HASH_A).await.unwrap();
    assert_eq!(created.username, "Judd");
    assert!(created.enabled);
    assert!(created.last_seen_at.is_none());

    let all = opds_users_repo::list(&pool).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0], created);
    // The row type carries no hash field — it can't leak.
}

#[tokio::test]
async fn list_is_ordered_case_insensitively() {
    let pool = fresh_pool().await;
    for name in ["thomas", "Brandon", "cody"] {
        opds_users_repo::create(&pool, name, HASH_A).await.unwrap();
    }
    let names: Vec<_> = opds_users_repo::list(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.username)
        .collect();
    assert_eq!(names, ["Brandon", "cody", "thomas"]);
}

#[tokio::test]
async fn duplicate_username_is_a_unique_violation_case_insensitive() {
    let pool = fresh_pool().await;
    opds_users_repo::create(&pool, "Reader", HASH_A).await.unwrap();
    let err = opds_users_repo::create(&pool, "reader", HASH_B)
        .await
        .unwrap_err();
    assert!(
        matches!(err, DbError::UniqueViolation { field: "opds_username" }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn find_enabled_for_auth_respects_the_enabled_flag() {
    let pool = fresh_pool().await;
    let u = opds_users_repo::create(&pool, "reader", HASH_A).await.unwrap();

    // Enabled + case-insensitive lookup returns the hash.
    let found = opds_users_repo::find_enabled_for_auth(&pool, "READER")
        .await
        .unwrap()
        .expect("enabled user found");
    assert_eq!(found.id, u.id);
    assert_eq!(found.password_hash, HASH_A);

    // Disabled → invisible to the auth path.
    opds_users_repo::set_enabled(&pool, u.id, false).await.unwrap();
    assert!(opds_users_repo::find_enabled_for_auth(&pool, "reader")
        .await
        .unwrap()
        .is_none());

    // Unknown username → None.
    assert!(opds_users_repo::find_enabled_for_auth(&pool, "ghost")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn set_enabled_and_delete_report_not_found() {
    let pool = fresh_pool().await;
    assert!(matches!(
        opds_users_repo::set_enabled(&pool, 999, false).await.unwrap_err(),
        DbError::NotFound
    ));
    assert!(matches!(
        opds_users_repo::delete(&pool, 999).await.unwrap_err(),
        DbError::NotFound
    ));

    let u = opds_users_repo::create(&pool, "temp", HASH_A).await.unwrap();
    opds_users_repo::delete(&pool, u.id).await.unwrap();
    assert!(opds_users_repo::list(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn touch_last_seen_sets_the_timestamp() {
    let pool = fresh_pool().await;
    let u = opds_users_repo::create(&pool, "reader", HASH_A).await.unwrap();
    assert!(u.last_seen_at.is_none());

    opds_users_repo::touch_last_seen(&pool, u.id).await.unwrap();
    let seen = opds_users_repo::list(&pool).await.unwrap()[0]
        .last_seen_at
        .clone();
    assert!(seen.is_some(), "last_seen_at stamped after touch");

    // A missing id is a silent no-op, not an error.
    opds_users_repo::touch_last_seen(&pool, 999).await.unwrap();
}

/// The migration's credential carry-over: a configured single credential in
/// `settings` must land as the first enabled `opds_users` row, preserving
/// access across the upgrade. Exercised against a hand-built pre-migration
/// fixture (the standard harness runs every migration, so the old rows are
/// already gone by the time a test sees the pool).
#[tokio::test]
async fn migration_carries_over_an_existing_credential() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();

    // Reproduce the pre-migration shape: a settings table with the old rows.
    sqlx::query("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    for (k, v) in [
        ("opds_username", "jeremy"),
        ("opds_password_hash", HASH_A),
        ("opds_api_token", "deadbeef"),
        ("opds_enabled", "true"),
    ] {
        sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
            .bind(k)
            .bind(v)
            .execute(&pool)
            .await
            .unwrap();
    }

    // Run the migration verbatim.
    sqlx::raw_sql(include_str!(
        "../migrations/20260628010000_opds_users.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    // The credential moved over, enabled.
    let user = opds_users_repo::find_enabled_for_auth(&pool, "jeremy")
        .await
        .unwrap()
        .expect("existing credential carried into opds_users");
    assert_eq!(user.password_hash, HASH_A);

    // Old rows gone; the global toggle stays.
    let leftover: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM settings WHERE key IN ('opds_username','opds_password_hash','opds_api_token')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(leftover, 0);
    let enabled: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'opds_enabled'")
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(enabled.as_deref(), Some("true"));
}

/// The carry-over must NOT fire when the old credential was unconfigured
/// (empty strings — how the seed migration left it). No phantom account.
#[tokio::test]
async fn migration_skips_an_unconfigured_credential() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    for (k, v) in [("opds_username", ""), ("opds_password_hash", "")] {
        sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
            .bind(k)
            .bind(v)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::raw_sql(include_str!(
        "../migrations/20260628010000_opds_users.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM opds_users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "no account created from an empty credential");
}
