use serde_json::json;
use sf_radar::digest::{bucket_for, render, Bucket, DigestEntry};
use sf_radar::score::{
    score_business, score_electrical, score_entertainment, score_mobile_food, score_permit,
    score_plumbing,
};

#[test]
fn business_food_naics_scores_two() {
    let row = json!({
        "dba_name": "MISSION TACOS",
        "ownership_name": "MISSION TACOS",
        "lic_code_description": "Some Other License",
        "self_reported_naics_code": "722511",
    });
    let (score, reasons) = score_business(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("food service")));
}

#[test]
fn business_food_license_keyword_scores_two() {
    let row = json!({
        "dba_name": "",
        "ownership_name": "",
        "lic_code_description": "Catering Food Facility",
        "self_reported_naics_code": "999999",
    });
    let (score, reasons) = score_business(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("food service")));
}

#[test]
fn business_retail_naics_scores_two() {
    for naics in ["441110", "459999"] {
        let row = json!({
            "dba_name": "",
            "ownership_name": "",
            "lic_code_description": "General",
            "self_reported_naics_code": naics,
        });
        let (score, reasons) = score_business(&row);
        assert_eq!(score, 2, "NAICS {naics} should be retail");
        assert!(reasons.iter().any(|r| r.contains("retail")));
    }
}

#[test]
fn business_unrelated_naics_scores_zero() {
    let row = json!({
        "dba_name": "",
        "ownership_name": "",
        "lic_code_description": "General Contractor",
        "self_reported_naics_code": "236220",
    });
    let (score, _) = score_business(&row);
    assert_eq!(score, 0);
}

#[test]
fn business_dba_different_from_owner_adds_one() {
    let row = json!({
        "dba_name": "Bob's Burgers",
        "ownership_name": "Belcher Holdings LLC",
        "lic_code_description": "Restaurant",
        "self_reported_naics_code": "",
    });
    let (score, reasons) = score_business(&row);
    assert_eq!(score, 3);
    assert!(reasons.iter().any(|r| r.contains("differs from owner")));
}

#[test]
fn permit_keyword_hit_scores_two() {
    let row = json!({
        "description": "Interior remodel for existing office",
        "proposed_use": "",
        "estimated_cost": "",
    });
    let (score, _) = score_permit(&row);
    assert_eq!(score, 0);

    let row = json!({
        "description": "Interior remodel for a new coffee shop",
        "proposed_use": "",
        "estimated_cost": "",
    });
    let (score, reasons) = score_permit(&row);
    assert!(score >= 2);
    assert!(reasons.iter().any(|r| r.contains("coffee")));
}

#[test]
fn permit_signals_stack() {
    let row = json!({
        "description": "Tenant improvement: change of use to restaurant",
        "proposed_use": "Restaurant",
        "estimated_cost": "250000.0",
    });
    let (score, reasons) = score_permit(&row);
    // keyword +2, change of use +1, tenant improvement +1, proposed use +1, cost +1
    assert_eq!(score, 6);
    assert_eq!(reasons.len(), 5);
}

#[test]
fn permit_estimated_cost_threshold() {
    let row = json!({
        "description": "minor repairs",
        "proposed_use": "",
        "estimated_cost": "100000.0",
    });
    assert_eq!(score_permit(&row).0, 0, "exactly 100k should not score");

    let row = json!({
        "description": "minor repairs",
        "proposed_use": "",
        "estimated_cost": "100000.01",
    });
    assert_eq!(score_permit(&row).0, 1);
}

#[test]
fn bucket_boundaries() {
    assert_eq!(bucket_for(0), None);
    assert_eq!(bucket_for(1), None);
    assert_eq!(bucket_for(2), Some(Bucket::Watch));
    assert_eq!(bucket_for(3), Some(Bucket::Watch));
    assert_eq!(bucket_for(4), Some(Bucket::Strong));
    assert_eq!(bucket_for(9), Some(Bucket::Strong));
}

fn entry(name: &str, hood: &str, date: &str, score: u32) -> DigestEntry {
    DigestEntry {
        source: "business".to_string(),
        id: name.to_string(),
        name: name.to_string(),
        address: "1 Market St".to_string(),
        date: date.to_string(),
        neighborhood: hood.to_string(),
        score,
        reasons: vec!["test reason".to_string()],
    }
}

#[test]
fn digest_groups_by_bucket_then_neighborhood() {
    let entries = vec![
        entry("weak", "Mission", "2026-07-01", 1), // filtered out
        entry("watch-sunset", "Sunset", "2026-07-02", 2),
        entry("strong-mission", "Mission", "2026-07-03", 5),
        entry("watch-mission", "Mission", "2026-07-04", 3),
    ];

    let text = render(&entries, 2, false, 7);
    let strong = text.find("🔥 Strong signals").unwrap();
    let watch = text.find("👀 Worth watching").unwrap();
    assert!(strong < watch, "strong bucket should come first");
    assert!(!text.contains("weak —"), "below min-score entries are filtered");

    // within the watch bucket, Mission sorts before Sunset
    let watch_section = &text[watch..];
    assert!(watch_section.find("Mission").unwrap() < watch_section.find("Sunset").unwrap());
    assert!(text.contains("3 new signal(s)."));

    let md = render(&entries, 2, true, 7);
    assert!(md.contains("## 🔥 Strong signals"));
    assert!(md.contains("### Mission"));
    assert!(md.contains("- **[business] strong-mission**"));
}

#[test]
fn digest_min_score_filter() {
    let entries = vec![entry("low", "Mission", "2026-07-01", 2)];
    let text = render(&entries, 4, false, 7);
    assert!(text.contains("No new signals."));
}

#[test]
fn entertainment_scores_base_plus_license() {
    let row = json!({"dba_name": "Club X", "license_type": ""});
    let (score, reasons) = score_entertainment(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("entertainment venue")));

    let row = json!({"dba_name": "Club X", "license_type": "Limited Live Performance"});
    let (score, reasons) = score_entertainment(&row);
    assert_eq!(score, 3);
    assert!(reasons.iter().any(|r| r.contains("Limited Live Performance")));
}

#[test]
fn mobile_food_scores_truck_bonus() {
    let row = json!({"applicant": "Taco Cart", "facilitytype": "Push Cart"});
    assert_eq!(score_mobile_food(&row).0, 2);

    let row = json!({"applicant": "Taco Truck", "facilitytype": "Truck"});
    let (score, reasons) = score_mobile_food(&row);
    assert_eq!(score, 3);
    assert!(reasons.iter().any(|r| r.contains("food truck")));
}

#[test]
fn electrical_keyword_and_valuation() {
    let row = json!({"description": "office wiring", "permit_valuation": "10000"});
    assert_eq!(score_electrical(&row).0, 0);

    let row = json!({"description": "new kitchen circuits for restaurant", "permit_valuation": "75000"});
    let (score, reasons) = score_electrical(&row);
    assert_eq!(score, 3); // strong keyword +2, valuation > 50k +1
    assert!(reasons.iter().any(|r| r.contains("restaurant")));
    assert!(reasons.iter().any(|r| r.contains("valuation")));

    let row = json!({"description": "bakery panel upgrade", "permit_valuation": "50000"});
    assert_eq!(score_electrical(&row).0, 2, "exactly 50k should not add valuation point");
}

#[test]
fn trade_kitchen_alone_is_weak() {
    // Residential remodel noise: "kitchen" alone scores 1, below the storage threshold.
    let row = json!({"description": "kitchen remodel", "permit_valuation": ""});
    let (score, reasons) = score_electrical(&row);
    assert_eq!(score, 1);
    assert!(reasons.iter().any(|r| r.contains("keyword: kitchen")));

    // ... but kitchen + a big valuation still reaches the storage threshold.
    let row = json!({"description": "kitchen remodel", "permit_valuation": "90000"});
    assert_eq!(score_electrical(&row).0, 2);

    // "commercial kitchen" is a strong hit.
    let row = json!({"description": "install hood in commercial kitchen", "permit_valuation": ""});
    let (score, reasons) = score_plumbing(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("commercial kitchen")));
}

#[test]
fn plumbing_grease_keyword() {
    let row = json!({"description": "install grease interceptor", "valuation": ""});
    let (score, reasons) = score_plumbing(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("grease")));

    let row = json!({"description": "install grease trap for new cafe", "valuation": "60000"});
    assert_eq!(score_plumbing(&row).0, 3);

    let row = json!({"description": "water heater replacement", "valuation": "80000"});
    let (score, _) = score_plumbing(&row);
    assert_eq!(score, 1, "valuation alone scores 1, below the storage threshold");
}

#[test]
fn trade_bar_excludes_residential_compounds() {
    // "wet bar" / "grab bar" are residential remodel noise.
    for desc in ["add 2 bedrooms, new wet bar", "install grab bar in bathroom", "towel bar replacement"] {
        let row = json!({"description": desc, "permit_valuation": ""});
        assert_eq!(score_electrical(&row).0, 0, "{desc:?} should not score");
    }

    // Real bars still hit: "wine bar", "juice bar", bare "bar".
    let row = json!({"description": "wine bar buildout", "permit_valuation": ""});
    let (score, reasons) = score_electrical(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("bar")));

    let row = json!({"description": "bar and restaurant light remodel", "valuation": ""});
    assert_eq!(score_plumbing(&row).0, 2);
}
