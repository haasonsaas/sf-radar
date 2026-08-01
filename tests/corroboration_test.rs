use sf_radar::address::normalize;
use sf_radar::digest::{apply_corroboration, AddressIndex, DigestEntry};

#[test]
fn normalize_whitespace_case_and_punctuation() {
    assert_eq!(normalize("418 Sutter St"), "418 SUTTER ST");
    assert_eq!(normalize("418   SUTTER ST"), "418 SUTTER ST");
    assert_eq!(normalize("905 Kearny St."), "905 KEARNY ST");
    assert_eq!(normalize("1850   CESAR CHAVEZ ST UNIT 1"), "1850 CESAR CHAVEZ ST");
    assert_eq!(normalize(""), "");
}

#[test]
fn normalize_suffixes() {
    assert_eq!(normalize("685 Harrison Street"), "685 HARRISON ST");
    assert_eq!(normalize("3251 20th Av"), "3251 20TH AVE");
    assert_eq!(normalize("3251 20th Avenue"), "3251 20TH AVE");
    assert_eq!(normalize("2675 Geary Bl"), "2675 GEARY BL"); // unknown suffix left alone
    assert_eq!(normalize("2675 Geary Boulevard"), "2675 GEARY BLVD");
    assert_eq!(normalize("10 Inverness Drive"), "10 INVERNESS DR");
    assert_eq!(normalize("10 Locksley Avenue"), "10 LOCKSLEY AVE");
}

#[test]
fn normalize_ordinals() {
    assert_eq!(normalize("100 FIRST ST"), normalize("100 1st St"));
    assert_eq!(normalize("4705 Third Street"), "4705 3RD ST");
    assert_eq!(normalize("120 02ND ST"), "120 02ND ST"); // digit form untouched
}

#[test]
fn normalize_strips_units() {
    assert_eq!(normalize("100 FIRST ST STE 165"), "100 1ST ST");
    assert_eq!(normalize("3053 Fillmore St Unit 237"), "3053 FILLMORE ST");
    assert_eq!(normalize("101 California St, Apt 5"), "101 CALIFORNIA ST");
    assert_eq!(normalize("1200 Howard St Rm 314"), "1200 HOWARD ST");
    assert_eq!(normalize("704 Larkin St # 702"), "704 LARKIN ST");
}

fn entry(source: &str, address: &str) -> DigestEntry {
    DigestEntry {
        source: source.to_string(),
        id: format!("{source}-1"),
        name: format!("{source} entry"),
        address: address.to_string(),
        date: "2026-07-01".to_string(),
        neighborhood: "Mission".to_string(),
        score: 2,
        reasons: Vec::new(),
    }
}

#[test]
fn corroboration_boosts_across_sources() {
    let index = AddressIndex::build(vec![
        (
            "permit".into(),
            "Permit 123".into(),
            "101 California St".into(),
            "2026-06-01".into(),
        ),
        (
            "plumbing".into(),
            "Permit PP9".into(),
            "101   CALIFORNIA ST".into(),
            "2026-06-17".into(),
        ),
        (
            "mobile_food".into(),
            "Senor Sisig".into(),
            "101 CALIFORNIA ST".into(),
            "2026-06-04".into(),
        ),
    ]);

    let mut entries = vec![entry("mobile_food", "101 California St")];
    apply_corroboration(&mut entries, &index);

    assert_eq!(entries[0].score, 4);
    assert_eq!(entries[0].reasons.len(), 1);
    let reason = &entries[0].reasons[0];
    // newest corroborator first
    assert_eq!(
        reason,
        "corroborated by plumbing: Permit PP9 (2026-06-17), permit: Permit 123 (2026-06-01)"
    );
    assert!(
        !reason.contains("mobile_food"),
        "the entry itself must not be listed as a corroborator"
    );
}

#[test]
fn corroboration_uses_seen_history_and_matches_units() {
    // Health row has a unit designator; business row doesn't — still a match.
    let index = AddressIndex::build(vec![
        (
            "health".into(),
            "Cafe".into(),
            "100 FIRST ST STE 165".into(),
            "2026-07-31".into(),
        ),
    ]);
    let mut entries = vec![entry("business", "100 1st St")];
    apply_corroboration(&mut entries, &index);
    assert_eq!(entries[0].score, 4);
    assert!(entries[0].reasons[0].contains("health: Cafe"));
}

#[test]
fn no_boost_when_only_match_is_same_source() {
    let index = AddressIndex::build(vec![
        (
            "permit".into(),
            "Permit 123".into(),
            "1 Market St".into(),
            "2026-06-01".into(),
        ),
    ]);
    let mut entries = vec![entry("permit", "1 MARKET ST")];
    apply_corroboration(&mut entries, &index);
    assert_eq!(entries[0].score, 2);
    assert!(entries[0].reasons.is_empty());
}

#[test]
fn corroboration_caps_at_three_listed() {
    let rows: Vec<(String, String, String, String)> = ["permit", "plumbing", "electrical", "health", "business"]
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                s.to_string(),
                format!("Filing {s}"),
                "500 Pine St".to_string(),
                format!("2026-06-0{}", i + 1),
            )
        })
        .collect();
    let index = AddressIndex::build(rows);

    let mut entries = vec![entry("mobile_food", "500 PINE ST")];
    apply_corroboration(&mut entries, &index);

    assert_eq!(entries[0].score, 4);
    let reason = &entries[0].reasons[0];
    assert_eq!(reason.matches(" (2026-").count(), 3, "at most 3 corroborators listed");
}

#[test]
fn corroborators_dedupe_same_source_name_date() {
    let index = AddressIndex::build(vec![
        ("health".into(), "RITZ".into(), "600 Stockton St".into(), "2025-04-16".into()),
        ("health".into(), "RITZ".into(), "600 Stockton St".into(), "2025-04-16".into()),
        ("health".into(), "RITZ".into(), "600 Stockton St".into(), "2025-04-24".into()),
        ("permit".into(), "Permit 1".into(), "600 Stockton St".into(), "2026-01-01".into()),
    ]);
    let mut entries = vec![entry("electrical", "600 STOCKTON ST")];
    apply_corroboration(&mut entries, &index);
    let reason = &entries[0].reasons[0];
    assert_eq!(reason.matches("health: RITZ").count(), 2, "identical rows collapse");
    assert!(reason.contains("permit: Permit 1"));
}
