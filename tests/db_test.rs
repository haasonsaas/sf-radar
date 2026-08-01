use serde_json::json;
use sf_radar::db;

fn test_conn() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    conn
}

fn business_row(id: &str, neighborhood: &str) -> serde_json::Value {
    json!({
        "uniqueid": id,
        "dba_name": format!("DBA {id}"),
        "ownership_name": "Owner LLC",
        "dba_start_date": "2026-07-01T00:00:00.000",
        "lic_code_description": "RESTAURANT",
        "self_reported_naics_code": "722511",
        "neighborhoods_analysis_boundaries": neighborhood,
        "full_business_address": "1 Main St",
    })
}

#[test]
fn neighborhood_filter_matches_substring() {
    let conn = test_conn();
    db::upsert_business(&conn, &business_row("1", "Mission Bay"), 3, &[]).unwrap();
    db::upsert_business(&conn, &business_row("2", "Marina"), 3, &[]).unwrap();

    let hits = db::unseen_businesses(&conn, "2026-01-01", 2, Some("mission bay")).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "1");
}

#[test]
fn no_neighborhood_filter_returns_all() {
    let conn = test_conn();
    db::upsert_business(&conn, &business_row("1", "Mission Bay"), 3, &[]).unwrap();
    db::upsert_business(&conn, &business_row("2", "Marina"), 3, &[]).unwrap();
    db::upsert_business(&conn, &business_row("3", ""), 3, &[]).unwrap();

    let hits = db::unseen_businesses(&conn, "2026-01-01", 2, None).unwrap();
    assert_eq!(hits.len(), 3);
}

#[test]
fn neighborhood_filter_applies_to_permits() {
    let conn = test_conn();
    let row = json!({
        "permit_number": "P1",
        "description": "new cafe tenant improvement",
        "filed_date": "2026-07-01T00:00:00.000",
        "neighborhoods_analysis_boundaries": "Dogpatch",
        "street_number": "100",
        "street_name": "Minnesota",
    });
    db::upsert_permit(&conn, &row, 4, &[]).unwrap();

    assert_eq!(
        db::unseen_permits(&conn, "2026-01-01", 2, Some("Dogpatch"))
            .unwrap()
            .len(),
        1
    );
    assert!(
        db::unseen_permits(&conn, "2026-01-01", 2, Some("Marina"))
            .unwrap()
            .is_empty()
    );
}
