//! One-off catalog data fix: re-normalize every series `sort_title`
//! against `longbox_core::normalize_title`. Surfaces every row whose
//! stored sort_title differs from the canonical form (~324 in the live
//! catalog as of this fix) and updates them in a single transaction.
//!
//! Defensive runtime: prints a summary before applying when run
//! without `--apply`; requires the flag explicitly to write. The
//! caller is responsible for stopping the longbox process so no
//! writer races this tool.
//!
//! Usage:
//!   cargo run -p longbox-db --example fix_sort_titles -- \
//!     sqlite:/path/to/longbox.db [--apply]

use std::env;
use std::process::ExitCode;

use longbox_core::normalize_title;
use sqlx::{sqlite::SqlitePoolOptions, Row};

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let db_url = match args.next() {
        Some(s) => s,
        None => {
            eprintln!("usage: fix_sort_titles <sqlite-url> [--apply]");
            return ExitCode::from(2);
        }
    };
    let apply = args.any(|a| a == "--apply");

    let pool = match SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("connect failed: {e}");
            return ExitCode::from(1);
        }
    };

    let rows = match sqlx::query("SELECT id, title, sort_title FROM series ORDER BY id")
        .fetch_all(&pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("query failed: {e}");
            return ExitCode::from(1);
        }
    };

    let mut to_update: Vec<(i64, String, String, String)> = Vec::new();
    for r in &rows {
        let id: i64 = r.get("id");
        let title: String = r.get("title");
        let stored: String = r.get("sort_title");
        let canonical = normalize_title(&title);
        if canonical != stored {
            to_update.push((id, title, stored, canonical));
        }
    }

    println!(
        "scanned {} series; {} sort_titles differ from normalize_title(title)",
        rows.len(),
        to_update.len()
    );

    let sample = to_update.iter().take(20);
    for (id, title, stored, canonical) in sample {
        println!(
            "  id={id:5} title={title:?}\n    stored:    {stored:?}\n    canonical: {canonical:?}"
        );
    }
    if to_update.len() > 20 {
        println!("  ... ({} more)", to_update.len() - 20);
    }

    if !apply {
        println!("\ndry run (no --apply). Re-run with --apply to write changes.");
        return ExitCode::SUCCESS;
    }

    // Apply in a single transaction so a mid-flight failure rolls back
    // cleanly rather than leaving the catalog half-fixed.
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("begin failed: {e}");
            return ExitCode::from(1);
        }
    };
    let mut updated = 0_u64;
    for (id, _, _, canonical) in &to_update {
        let r = sqlx::query("UPDATE series SET sort_title = ? WHERE id = ?")
            .bind(canonical)
            .bind(id)
            .execute(&mut *tx)
            .await;
        match r {
            Ok(res) => updated += res.rows_affected(),
            Err(e) => {
                eprintln!("update id={id} failed: {e} — rolling back");
                return ExitCode::from(1);
            }
        }
    }
    if let Err(e) = tx.commit().await {
        eprintln!("commit failed: {e}");
        return ExitCode::from(1);
    }
    println!("applied: {updated} rows updated");
    ExitCode::SUCCESS
}
