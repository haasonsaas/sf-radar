use serde_json::Value;
use sf_radar::digest::{cluster, ordered_with, render_json_with, render_with, AddressIndex, DigestEntry};

fn entry(source: &str, id: &str, name: &str, address: &str, date: &str, score: u32) -> DigestEntry {
    DigestEntry {
        source: source.to_string(),
        id: id.to_string(),
        name: name.to_string(),
        address: address.to_string(),
        date: date.to_string(),
        neighborhood: "FiDi".to_string(),
        score,
        reasons: vec![format!("{source} reason")],
        description: None,
        url: String::new(),
    }
}

fn no_history() -> AddressIndex {
    AddressIndex::build(Vec::new())
}

#[test]
fn same_address_clusters_into_one_venue_named_by_dba_source() {
    let entries = vec![
        entry("electrical", "E1", "Permit EW1", "88 Spear St", "2026-08-10", 4),
        entry("abc", "681355", "ALTO 88", "88 SPEAR ST.", "2026-08-21", 5),
        entry("permit", "P1", "Permit P1", "88-90 Spear Street", "2026-07-01", 2),
        entry("business", "B9", "Elsewhere Cafe", "1 Other Ave", "2026-08-01", 3),
    ];
    let venues = cluster(&entries, &no_history());
    assert_eq!(venues.len(), 2);
    let alto = venues.iter().find(|v| v.name == "ALTO 88").expect("abc name wins over Permit names");
    assert_eq!(alto.entries.len(), 3);
    assert_eq!(alto.score, 5, "best member score");
    assert_eq!(alto.date, "2026-08-21", "newest filing");
    assert_eq!(alto.address, "88 SPEAR ST.", "address from the naming entry");
    // Timeline newest first.
    let ids: Vec<&str> = alto.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["681355", "E1", "P1"]);
}

#[test]
fn name_links_only_attach_addressless_entries() {
    // A food truck applicant with no address joins the storefront by name…
    let entries = vec![
        entry("business", "B1", "Senor Sisig LLC", "990 Valencia St", "2026-08-01", 3),
        entry("mobile_food", "M1", "Senor Sisig", "", "2026-08-05", 3),
    ];
    let venues = cluster(&entries, &no_history());
    assert_eq!(venues.len(), 1);
    assert_eq!(venues[0].address, "990 Valencia St");

    // …but two locations of a chain stay separate venues.
    let entries = vec![
        entry("business", "B1", "Super Duper Burgers", "235 Stockton St", "2026-08-01", 3),
        entry("health", "H1", "SUPER DUPER BURGER", "2304 Market St", "2026-08-05", 3),
    ];
    assert_eq!(cluster(&entries, &no_history()).len(), 2);
}

#[test]
fn history_lists_earlier_filings_not_among_members() {
    let history = AddressIndex::build(vec![
        ("permit".to_string(), "Permit OLD".to_string(), "88 Spear St".to_string(), "2026-05-01".to_string()),
        ("abc".to_string(), "ALTO 88".to_string(), "88 Spear St".to_string(), "2026-08-21".to_string()), // a member
    ]);
    let entries = vec![entry("abc", "681355", "ALTO 88", "88 SPEAR ST.", "2026-08-21", 5)];
    let venues = cluster(&entries, &history);
    assert_eq!(venues[0].history.len(), 1, "member filing excluded from history");
    assert_eq!(venues[0].history[0].name, "Permit OLD");

    let text = render_with(&entries, 2, false, 7, &history);
    assert!(text.contains("    earlier: permit: Permit OLD (2026-05-01)"), "{text}");
}

#[test]
fn displayed_venue_includes_sub_threshold_members() {
    // The permit alone (score 1) is below min-score 2, but it belongs to a
    // venue whose best filing scores 5, so it's shown and marked seen.
    let entries = vec![
        entry("permit", "P1", "Permit P1", "88 Spear St", "2026-07-01", 1),
        entry("abc", "A1", "ALTO 88", "88 SPEAR ST.", "2026-08-21", 5),
        entry("permit", "P2", "Permit P2", "1 Nowhere Rd", "2026-07-02", 1),
    ];
    let shown: Vec<&str> = ordered_with(&entries, 2, &no_history()).iter().map(|e| e.id.as_str()).collect();
    assert_eq!(shown, ["A1", "P1"], "venue members newest first; lone sub-threshold row excluded");
}

#[test]
fn json_has_venues_and_flat_entries_agree() {
    let entries = vec![
        entry("abc", "A1", "ALTO 88", "88 Spear St", "2026-08-21", 5),
        entry("electrical", "E1", "Permit EW1", "88 SPEAR ST", "2026-08-10", 4),
        entry("business", "B1", "Solo Cafe", "1 Other Ave", "2026-08-01", 3),
    ];
    let v: Value = serde_json::from_str(&render_json_with(&entries, 2, 7, 0, &no_history())).unwrap();
    let venues = v["venues"].as_array().unwrap();
    assert_eq!(venues.len(), 2);
    assert_eq!(venues[0]["name"], "ALTO 88");
    assert_eq!(venues[0]["score"], 5);
    assert_eq!(venues[0]["bucket"], "strong");
    assert_eq!(venues[0]["filings"].as_array().unwrap().len(), 2);
    assert_eq!(venues[0]["filings"][1]["id"], "E1");
    assert_eq!(venues[0]["history"].as_array().unwrap().len(), 0);
    let keys: Vec<&str> = venues[0].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["address", "bucket", "date", "filings", "history", "name", "neighborhood", "score"]);

    // Flat entries are the venues' filings in the same order.
    let flat: Vec<&str> = v["entries"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(flat, ["A1", "E1", "B1"]);
}

#[test]
fn prose_renders_venue_block() {
    let entries = vec![
        entry("abc", "A1", "ALTO 88", "88 Spear St", "2026-08-21", 5),
        entry("electrical", "E1", "Permit EW1", "88 SPEAR ST", "2026-08-10", 4),
    ];
    let mut entries = entries;
    entries[0].reasons.push("corroborated by electrical: Permit EW1 (2026-08-10)".to_string());
    let text = render_with(&entries, 2, false, 7, &no_history());
    assert!(text.contains("  ALTO 88 — 88 Spear St · score 5 · 2 filings\n"), "{text}");
    assert!(!text.contains("corroborated by"), "prose drops corroboration reasons: {text}");
    assert!(text.contains("    2026-08-21 [abc] abc reason\n"), "venue-named filing omits the name: {text}");
    assert!(text.contains("    2026-08-10 [electrical] Permit EW1 — electrical reason\n"), "{text}");
    assert!(text.contains("2 new signal(s) at 1 venue(s)."));
}
