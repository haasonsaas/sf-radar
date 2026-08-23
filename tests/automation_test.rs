use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use sf_radar::{config, db, digest};
use sf_radar::digest::{apply_corroboration, AddressIndex, DigestEntry, NameIndex};

fn entry(source: &str, id: &str, score: u32) -> DigestEntry {
    DigestEntry {
        source: source.to_string(),
        id: id.to_string(),
        name: format!("Name {id}"),
        address: "1 Main St".to_string(),
        date: "2026-07-01".to_string(),
        neighborhood: "Mission".to_string(),
        score,
        reasons: vec!["test reason".to_string()],
        description: None,
        url: String::new(),
    }
}

#[test]
fn json_shape_has_exact_keys() {
    let mut e = entry("business", "B1", 5);
    e.name = "Meek Coffee".to_string();
    e.address = "2360 3rd St".to_string();
    e.neighborhood = "Potrero Hill".to_string();
    e.date = "2026-07-09".to_string();

    let out = digest::render_json(&[e], 2, 30, 3);
    let v: Value = serde_json::from_str(&out).unwrap();

    let top: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        top,
        // serde_json::Value sorts object keys; the emitted field order is
        // covered by the exact-match entry assertions below.
        ["archived", "entries", "generated_at", "min_score", "tool", "window_days"]
    );
    assert_eq!(v["tool"], "sf-radar");
    assert_eq!(v["window_days"], 30);
    assert_eq!(v["min_score"], 2);
    assert_eq!(v["archived"], 3);
    // RFC3339 UTC
    assert!(v["generated_at"].as_str().unwrap().ends_with('Z'));

    let entry = &v["entries"][0];
    let keys: Vec<&str> = entry.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "address", "bucket", "date", "description", "id", "name", "neighborhood",
            "reasons", "score", "source", "url"
        ]
    );
    assert_eq!(entry["source"], "business");
    assert_eq!(entry["id"], "B1");
    assert_eq!(entry["name"], "Meek Coffee");
    assert_eq!(entry["address"], "2360 3rd St");
    assert_eq!(entry["neighborhood"], "Potrero Hill");
    assert_eq!(entry["date"], "2026-07-09");
    assert_eq!(entry["score"], 5);
    assert_eq!(entry["bucket"], "strong");
    assert_eq!(entry["reasons"], serde_json::json!(["test reason"]));
    assert_eq!(entry["url"], "");
    assert_eq!(entry["description"], "");
}

#[test]
fn json_and_prose_include_entry_url() {
    let mut e = entry("abc", "681355", 3);
    e.url = digest::url_for("abc", "681355");

    let out = digest::render_json(&[e.clone()], 2, 30, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["entries"][0]["url"],
        "https://www.abc.ca.gov/licensing/license-lookup/single-license/?RPTTYPE=12&LICENSE=681355"
    );

    let plain = digest::render(&[e.clone()], 2, false, 7);
    assert!(plain.contains("\n    https://www.abc.ca.gov/"), "plain text lists the url on its own line");
    let md = digest::render(&[e], 2, true, 7);
    assert!(md.contains("[Name 681355](https://www.abc.ca.gov/"), "markdown links the name");
}

#[test]
fn json_bucket_uses_post_bonus_score() {
    // Score 3 (watch) + corroboration bonus +2 -> 5 (strong).
    let rows = vec![(
        "permit".to_string(),
        "Permit 1".to_string(),
        "1 Main St".to_string(),
        "2026-06-15".to_string(),
    )];
    let names = NameIndex::build(&rows);
    let addresses = AddressIndex::build(rows);

    let mut entries = vec![entry("business", "B1", 3)];
    apply_corroboration(&mut entries, &addresses, &names);
    assert_eq!(entries[0].score, 5);

    let v: Value = serde_json::from_str(&digest::render_json(&entries, 2, 7, 0)).unwrap();
    assert_eq!(v["entries"][0]["bucket"], "strong");
    assert_eq!(v["entries"][0]["score"], 5);
}

#[test]
fn json_entries_ordered_best_first_like_prose() {
    let strong = entry("business", "strong", 4);
    let watch = entry("permit", "watch", 2);
    let below = entry("health", "below", 1); // filtered out at min_score 2
    // Deliberately worst-first input; output must be best-first.
    let entries = vec![below, watch, strong];

    let v: Value = serde_json::from_str(&digest::render_json(&entries, 2, 7, 0)).unwrap();
    let ids: Vec<&str> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["strong", "watch"]);
    assert_eq!(v["entries"][0]["bucket"], "strong");
    assert_eq!(v["entries"][1]["bucket"], "watch");
}

#[test]
fn json_description_snippet_included() {
    let mut e = entry("permit", "P1", 4);
    e.description = Some("Tenant improvement for new cafe".to_string());
    let v: Value = serde_json::from_str(&digest::render_json(&[e], 2, 7, 0)).unwrap();
    assert_eq!(v["entries"][0]["description"], "Tenant improvement for new cafe");
}

#[test]
fn config_env_wins_over_file_then_blank_falls_through() {
    let cfg = config::Config {
        socrata_app_token: Some("file-token".to_string()),
    };
    assert_eq!(
        config::app_token(Some("env-token".to_string()), &cfg),
        Some("env-token".to_string())
    );
    assert_eq!(
        config::app_token(None, &cfg),
        Some("file-token".to_string())
    );
    // Blank/whitespace tokens count as unset at both levels.
    assert_eq!(
        config::app_token(Some("  ".to_string()), &cfg),
        Some("file-token".to_string())
    );
    let blank_cfg = config::Config {
        socrata_app_token: Some("".to_string()),
    };
    assert_eq!(config::app_token(None, &blank_cfg), None);
    assert_eq!(config::app_token(None, &config::Config::default()), None);
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sf-radar-test-{}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn config_load_reads_db_sibling_and_ignores_invalid_toml() {
    let dir = temp_dir("config-load");
    let db_path = dir.join("radar.db");

    // Missing file: default config, no token.
    assert!(config::load(&db_path).socrata_app_token.is_none());

    // Valid file.
    std::fs::write(dir.join("config.toml"), "socrata_app_token = \"tok123\"\n").unwrap();
    assert_eq!(
        config::load(&db_path).socrata_app_token.as_deref(),
        Some("tok123")
    );

    // Invalid TOML: ignored (warning on stderr), back to default.
    std::fs::write(dir.join("config.toml"), "this is [ not toml\n").unwrap();
    assert!(config::load(&db_path).socrata_app_token.is_none());

    std::fs::remove_dir_all(&dir).ok();
}

fn seed_db(db_path: &std::path::Path) {
    let conn = db::open(db_path).unwrap();
    let signal = |id: &str, date: &str| db::Signal {
        source: "business",
        external_id: id.to_string(),
        name: format!("Name {id}"),
        address: "1 Main St".to_string(),
        date: date.to_string(),
        neighborhood: "Mission".to_string(),
        raw: "{}".to_string(),
        score: 3,
        reasons: vec!["test".to_string()],
        first_seen: date.to_string(),
        seen: false,
    };
    // In-window row (would be marked seen) and a stale row (would be archived).
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    db::upsert_signal(&conn, &signal("recent", &today)).unwrap();
    db::upsert_signal(&conn, &signal("stale", "2020-01-01")).unwrap();
}

fn seen_flags(db_path: &std::path::Path) -> (u32, u32) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let get = |id: &str| -> u32 {
        conn.query_row(
            "SELECT seen FROM signals WHERE external_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    };
    (get("recent"), get("stale"))
}

/// A permit stored at score 1 (below min-score 2) must surface when an abc
/// application at the same address corroborates it, and get marked seen.
/// A lone score-1 row stays unseen — it was never displayed.
#[test]
fn corroboration_rescues_sub_threshold_rows() {
    let dir = temp_dir("rescue");
    let db_path = dir.join("radar.db");
    {
        let conn = db::open(&db_path).unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let signal = |source: &'static str, id: &str, address: &str, score: u32, hood: &str| db::Signal {
            source,
            external_id: id.to_string(),
            name: format!("Name {id}"),
            address: address.to_string(),
            date: today.clone(),
            neighborhood: hood.to_string(),
            raw: "{}".to_string(),
            score,
            reasons: vec!["test".to_string()],
            first_seen: today.clone(),
            seen: false,
        };
        db::upsert_signal(&conn, &signal("permit", "weak-corroborated", "88 Spear St", 1, "FiDi")).unwrap();
        db::upsert_signal(&conn, &signal("abc", "abc-app", "88 SPEAR ST.", 3, "")).unwrap();
        db::upsert_signal(&conn, &signal("permit", "weak-alone", "1 Nowhere Rd", 1, "Sunset")).unwrap();
    }

    let bin = env!("CARGO_BIN_EXE_sf-radar");
    let out = Command::new(bin)
        .args(["--db"])
        .arg(&db_path)
        .args(["digest", "--days", "30", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = v["entries"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"weak-corroborated"), "rescued by +2 address bonus: {ids:?}");
    assert!(ids.contains(&"abc-app"));
    assert!(!ids.contains(&"weak-alone"), "still below threshold: {ids:?}");
    let abc = v["entries"].as_array().unwrap().iter().find(|e| e["id"] == "abc-app").unwrap();
    assert_eq!(abc["neighborhood"], "FiDi", "abc inherits the permit's neighborhood");

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let seen = |id: &str| -> u32 {
        conn.query_row("SELECT seen FROM signals WHERE external_id=?1", [id], |r| r.get(0)).unwrap()
    };
    assert_eq!(seen("weak-corroborated"), 1);
    assert_eq!(seen("abc-app"), 1);
    assert_eq!(seen("weak-alone"), 0, "undisplayed rows must not be marked seen");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dry_run_leaves_seen_flags_untouched() {
    let dir = temp_dir("dry-run");
    let db_path = dir.join("radar.db");
    seed_db(&db_path);

    let bin = env!("CARGO_BIN_EXE_sf-radar");

    // --dry-run --json: prints JSON, mutates nothing.
    let out = Command::new(bin)
        .args(["--db"])
        .arg(&db_path)
        .args(["digest", "--days", "30", "--json", "--dry-run"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    assert_eq!(v["entries"][0]["id"], "recent");
    assert_eq!(v["archived"], 0);
    assert_eq!(seen_flags(&db_path), (0, 0), "dry-run must not touch seen");

    // Real run: marks the entry seen and archives the stale row.
    let out = Command::new(bin)
        .args(["--db"])
        .arg(&db_path)
        .args(["digest", "--days", "30", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["archived"], 1);
    assert_eq!(seen_flags(&db_path), (1, 1));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn json_conflicts_with_md() {
    let bin = env!("CARGO_BIN_EXE_sf-radar");
    let out = Command::new(bin)
        .args(["digest", "--json", "--md"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"));
}
