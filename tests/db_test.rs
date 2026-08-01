use serde_json::json;
use sf_radar::{db, sources};

fn test_conn() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    conn
}

fn signal(source: &'static str, id: &str, neighborhood: &str) -> db::Signal {
    db::Signal {
        source,
        external_id: id.to_string(),
        name: format!("Name {id}"),
        address: "1 Main St".to_string(),
        date: "2026-07-01".to_string(),
        neighborhood: neighborhood.to_string(),
        raw: "{}".to_string(),
        score: 3,
        reasons: vec!["test".to_string()],
        first_seen: "2026-07-01".to_string(),
        seen: false,
    }
}

#[test]
fn neighborhood_filter_matches_substring() {
    let conn = test_conn();
    db::upsert_signal(&conn, &signal("business", "1", "Mission Bay")).unwrap();
    db::upsert_signal(&conn, &signal("business", "2", "Marina")).unwrap();

    let hits = db::unseen_signals(&conn, "2026-01-01", "2026-12-31", 2, Some("mission bay")).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "1");
}

#[test]
fn no_neighborhood_filter_returns_all() {
    let conn = test_conn();
    db::upsert_signal(&conn, &signal("business", "1", "Mission Bay")).unwrap();
    db::upsert_signal(&conn, &signal("permit", "2", "Marina")).unwrap();
    db::upsert_signal(&conn, &signal("mobile_food", "3", "")).unwrap();

    let hits = db::unseen_signals(&conn, "2026-01-01", "2026-12-31", 2, None).unwrap();
    assert_eq!(hits.len(), 3);
}

#[test]
fn unseen_signals_respects_score_cutoff_and_seen() {
    let conn = test_conn();
    let mut low = signal("permit", "low", "Mission");
    low.score = 1;
    db::upsert_signal(&conn, &low).unwrap();
    db::upsert_signal(&conn, &signal("permit", "high", "Mission")).unwrap();

    let hits = db::unseen_signals(&conn, "2026-01-01", "2026-12-31", 2, None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "high");

    db::mark_seen(&conn, &hits).unwrap();
    assert!(db::unseen_signals(&conn, "2026-01-01", "2026-12-31", 2, None)
        .unwrap()
        .is_empty());
}

#[test]
fn health_composite_id() {
    let health = sources::all()
        .into_iter()
        .find(|s| s.key == "health")
        .unwrap();
    let row = json!({
        "permit_number": "P12345",
        "inspection_date": "2026-07-01T00:00:00.000",
    });
    assert_eq!(
        (health.external_id)(&row),
        "P12345-2026-07-01T00:00:00.000"
    );
}

#[test]
fn health_first_inspection_scores_then_zero() {
    let conn = test_conn();
    let first = json!({
        "permit_number": "P1",
        "inspection_date": "2026-01-01T00:00:00.000",
        "dba": "Pho Real",
    });
    // Nothing stored yet: first inspection on record scores 3.
    let (score, reasons) = sources::score_health(&first, &conn);
    assert_eq!(score, 3);
    assert!(reasons.iter().any(|r| r.contains("first health inspection")));

    // Once stored, a later inspection for the same permit scores 0.
    let mut s = signal("health", "P1-2026-01-01T00:00:00.000", "Mission");
    s.score = 3;
    db::upsert_signal(&conn, &s).unwrap();

    let second = json!({
        "permit_number": "P1",
        "inspection_date": "2026-02-01T00:00:00.000",
        "dba": "Pho Real",
    });
    assert_eq!(sources::score_health(&second, &conn).0, 0);

    // A different permit still scores as a first inspection.
    let other = json!({
        "permit_number": "P2",
        "inspection_date": "2026-02-01T00:00:00.000",
        "dba": "New Place",
    });
    assert_eq!(sources::score_health(&other, &conn).0, 3);
}

#[test]
fn upsert_preserves_date_and_first_seen_on_conflict() {
    let conn = test_conn();
    // Snapshot source: date is set to first_seen on first insert ...
    let mut s = signal("entertainment", "E1", "Mission");
    s.date = "2026-08-01".to_string();
    s.first_seen = "2026-08-01".to_string();
    db::upsert_signal(&conn, &s).unwrap();

    // ... and a later re-fetch must not move date/first_seen forward,
    // though refreshed fields like score do update.
    let mut again = signal("entertainment", "E1", "Mission");
    again.date = "2026-08-15".to_string();
    again.first_seen = "2026-08-15".to_string();
    again.score = 3;
    db::upsert_signal(&conn, &again).unwrap();

    let (date, first_seen, score): (String, String, u32) = conn
        .query_row(
            "SELECT date, first_seen, score FROM signals WHERE source='entertainment' AND external_id='E1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(date, "2026-08-01");
    assert_eq!(first_seen, "2026-08-01");
    assert_eq!(score, 3);
}

#[test]
fn legacy_migration_moves_rows_and_drops_old_tables() {
    let conn = test_conn();
    conn.execute_batch(
        "
        CREATE TABLE businesses (
            uniqueid TEXT PRIMARY KEY,
            dba_name TEXT NOT NULL DEFAULT '',
            ownership_name TEXT NOT NULL DEFAULT '',
            dba_start_date TEXT NOT NULL DEFAULT '',
            location_start_date TEXT NOT NULL DEFAULT '',
            lic_code_description TEXT NOT NULL DEFAULT '',
            naics TEXT NOT NULL DEFAULT '',
            neighborhood TEXT NOT NULL DEFAULT '',
            address TEXT NOT NULL DEFAULT '',
            corridor TEXT NOT NULL DEFAULT '',
            score INTEGER NOT NULL DEFAULT 0,
            reasons TEXT NOT NULL DEFAULT '',
            seen INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE permits (
            permit_number TEXT PRIMARY KEY,
            description TEXT NOT NULL DEFAULT '',
            filed_date TEXT NOT NULL DEFAULT '',
            issued_date TEXT NOT NULL DEFAULT '',
            proposed_use TEXT NOT NULL DEFAULT '',
            estimated_cost TEXT NOT NULL DEFAULT '',
            neighborhood TEXT NOT NULL DEFAULT '',
            address TEXT NOT NULL DEFAULT '',
            score INTEGER NOT NULL DEFAULT 0,
            reasons TEXT NOT NULL DEFAULT '',
            seen INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO businesses (uniqueid, dba_name, ownership_name, dba_start_date,
                                neighborhood, address, score, reasons, seen)
        VALUES ('B1', 'Taco Place', 'Owner LLC', '2026-06-01', 'Mission', '1 Main St', 3, 'retail', 1);
        INSERT INTO permits (permit_number, filed_date, neighborhood, address, score, reasons, seen)
        VALUES ('P1', '2026-06-02', 'Marina', '2 Main St', 4, 'keyword: cafe', 0);
        INSERT INTO meta (key, value) VALUES ('watermark:businesses', '2026-06-01T00:00:00.000');
        INSERT INTO meta (key, value) VALUES ('watermark:permits', '2026-06-02T00:00:00.000');
        ",
    )
    .unwrap();

    db::migrate_legacy(&conn).unwrap();

    // Rows moved into signals with score/reasons/seen preserved, first_seen = date.
    let biz: (String, String, u32, String, u32, String) = conn
        .query_row(
            "SELECT name, date, score, reasons, seen, first_seen FROM signals
             WHERE source='business' AND external_id='B1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(biz, ("Taco Place".into(), "2026-06-01".into(), 3, "retail".into(), 1, "2026-06-01".into()));

    let permit_name: String = conn
        .query_row(
            "SELECT name FROM signals WHERE source='permit' AND external_id='P1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(permit_name, "Permit P1");

    // Old tables dropped.
    let remaining: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('businesses', 'permits')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);

    // Watermarks renamed to the new source keys.
    assert_eq!(
        db::get_watermark(&conn, "watermark:business").unwrap(),
        Some("2026-06-01T00:00:00.000".to_string())
    );
    assert_eq!(
        db::get_watermark(&conn, "watermark:permit").unwrap(),
        Some("2026-06-02T00:00:00.000".to_string())
    );
    assert_eq!(db::get_watermark(&conn, "watermark:businesses").unwrap(), None);

    // Migration is idempotent on a fresh/migrated DB.
    db::migrate_legacy(&conn).unwrap();
}

#[test]
fn normalize_date_handles_iso_and_compact() {
    assert_eq!(db::normalize_date("2026-07-31T15:18:57.000"), "2026-07-31");
    assert_eq!(db::normalize_date("20260731"), "2026-07-31");
    assert_eq!(db::normalize_date("2026-07-31"), "2026-07-31");
    assert_eq!(db::normalize_date(""), "");
}

#[test]
fn health_source_backfill_config() {
    let health = sources::all()
        .into_iter()
        .find(|s| s.key == "health")
        .unwrap();
    assert_eq!(health.backfill_start, Some("2024-01-01"));
    assert!(health.quiet_backfill);
    // Nobody else is quiet.
    for s in sources::all() {
        if s.key != "health" {
            assert!(!s.quiet_backfill, "{} should not be quiet", s.key);
            assert_eq!(s.backfill_start, None);
        }
    }
}

#[test]
fn quiet_backfill_rows_stored_seen_incremental_unseen() {
    let conn = test_conn();

    // Initial backfill insert: stored pre-seen.
    let mut backfill = signal("health", "P1-2024-03-01T00:00:00.000", "Mission");
    backfill.seen = true;
    db::upsert_signal(&conn, &backfill).unwrap();

    // Incremental insert after the radar is live: unseen.
    let incremental = signal("health", "P2-2026-08-01T00:00:00.000", "Mission");
    db::upsert_signal(&conn, &incremental).unwrap();

    let seen_of = |id: &str| -> u32 {
        conn.query_row(
            "SELECT seen FROM signals WHERE source='health' AND external_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(seen_of("P1-2024-03-01T00:00:00.000"), 1);
    assert_eq!(seen_of("P2-2026-08-01T00:00:00.000"), 0);

    // A later non-quiet re-fetch of a backfill row must not flip seen back.
    let mut refetch = signal("health", "P1-2024-03-01T00:00:00.000", "Mission");
    refetch.seen = false;
    db::upsert_signal(&conn, &refetch).unwrap();
    assert_eq!(seen_of("P1-2024-03-01T00:00:00.000"), 1);
}
