use serde_json::json;
use sf_radar::digest::{bucket_for, render, Bucket, DigestEntry};
use sf_radar::score::{
    score_business, score_electrical, score_entertainment, score_fire, score_mobile_food,
    score_permit, score_planning, score_plumbing, score_tables_chairs, score_vending,
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
fn business_beverage_naics_scores_two() {
    // 3121 = beverage manufacturing (breweries, wineries, distilleries).
    let row = json!({
        "dba_name": "",
        "ownership_name": "",
        "lic_code_description": "General",
        "self_reported_naics_code": "312120",
    });
    let (score, reasons) = score_business(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("beverage producer")));
}

#[test]
fn permit_bar_excludes_residential_compounds() {
    // Substring matches that used to fire: rebar, grab bar, wet bar.
    for desc in [
        "install rebar reinforcement at foundation",
        "install grab bar in bathroom",
        "add wet bar to living room",
    ] {
        let row = json!({"description": desc, "proposed_use": "", "estimated_cost": ""});
        assert_eq!(score_permit(&row).0, 0, "{desc:?} should not score");
    }

    // Real bars still hit.
    let row = json!({"description": "buildout for wine bar", "proposed_use": "", "estimated_cost": ""});
    let (score, reasons) = score_permit(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("bar")));
}

#[test]
fn permit_expanded_food_keywords() {
    for desc in [
        "taqueria tenant buildout",
        "convert space to pizzeria",
        "new ramen shop interior",
        "brewery taproom fit-out",
    ] {
        let row = json!({"description": desc, "proposed_use": "", "estimated_cost": ""});
        assert!(score_permit(&row).0 >= 2, "{desc:?} should score");
    }
}

#[test]
fn permit_new_requires_word_boundary() {
    // "newly renovated" is not a new business.
    let row = json!({"description": "newly renovated office suite", "proposed_use": "", "estimated_cost": ""});
    assert_eq!(score_permit(&row).0, 0);

    let row = json!({"description": "new storefront for retail", "proposed_use": "", "estimated_cost": ""});
    let (score, reasons) = score_permit(&row);
    assert_eq!(score, 3); // keyword +2, leading "new" +1
    assert!(reasons.iter().any(|r| r.contains("new business")));
}

#[test]
fn trade_commercial_kitchen_equipment_keywords() {
    let row = json!({"description": "install walk-in cooler and compressor", "permit_valuation": ""});
    assert_eq!(score_electrical(&row).0, 2);

    let row = json!({"description": "type 1 hood exhaust connection", "valuation": ""});
    assert_eq!(score_plumbing(&row).0, 2);

    // "walk-in closet" must not score.
    let row = json!({"description": "wiring for walk-in closet", "permit_valuation": ""});
    assert_eq!(score_electrical(&row).0, 0);
}

#[test]
fn tables_chairs_scoring() {
    let row = json!({"dbaname": "CAFE X", "tablesandchairs": true, "displaymerchandise": false});
    let (score, reasons) = score_tables_chairs(&row);
    assert_eq!(score, 3);
    assert!(reasons.iter().any(|r| r.contains("outdoor seating")));

    let row = json!({"dbaname": "SHOP Y", "tablesandchairs": false, "displaymerchandise": true});
    let (score, reasons) = score_tables_chairs(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("merchandise display")));

    // Missing flags still store as a generic registration.
    let row = json!({"dbaname": "Z"});
    assert_eq!(score_tables_chairs(&row).0, 2);
}

#[test]
fn planning_food_description_scores() {
    let row = json!({
        "description": "The project proposes re-establishing the previous Full Service Restaurant use.",
    });
    let (score, reasons) = score_planning(&row);
    assert_eq!(score, 2);
    assert!(reasons.iter().any(|r| r.contains("restaurant")));

    // Land-use vocabulary the permit list doesn't have.
    let row = json!({"description": "Establish outdoor dining and takeout food service."});
    assert_eq!(score_planning(&row).0, 2);

    // Change of use stacks.
    let row = json!({"description": "Change of use from office to cafe."});
    assert_eq!(score_planning(&row).0, 3);

    // Housing projects score 0 (and are dropped at ingest).
    let row = json!({"description": "New construction of a 24-unit residential building."});
    assert_eq!(score_planning(&row).0, 0);

    // "rebar" must not trip the bar keyword here either.
    let row = json!({"description": "Structural upgrade with new rebar."});
    assert_eq!(score_planning(&row).0, 0);
}

#[test]
fn fire_place_of_assembly_scoring() {
    let row = json!({"permit_type_description": "place of assembly, operation"});
    let (score, reasons) = score_fire(&row);
    assert_eq!(score, 3);
    assert!(reasons.iter().any(|r| r.contains("place of assembly")));

    let row = json!({"permit_type_description": "outdoor place of assembly"});
    assert_eq!(score_fire(&row).0, 3);

    // Temporary/special-event variants are noise.
    for ty in [
        "place of assembly, temporary / special, operation",
        "open flame, use, temporary",
        "hot work operations, welder, cut, weld, grind, braze, solder, conduct",
    ] {
        let row = json!({"permit_type_description": ty});
        assert_eq!(score_fire(&row).0, 0, "{ty:?} should not score");
    }
}

#[test]
fn vending_food_bonus() {
    let row = json!({"dbaname": "X", "whatwillyoubeselling": "merchandise",
                     "describewhatyouwillsell": "selling new and used books"});
    assert_eq!(score_vending(&row).0, 2);

    let row = json!({"dbaname": "Y", "whatwillyoubeselling": "",
                     "describewhatyouwillsell": "hot dogs and drinks"});
    let (score, reasons) = score_vending(&row);
    assert_eq!(score, 3);
    assert!(reasons.iter().any(|r| r.contains("food vendor")));
}

#[test]
fn bucket_boundaries() {
    assert_eq!(bucket_for(0), Some(Bucket::Low));
    assert_eq!(bucket_for(1), Some(Bucket::Low));
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
        // Distinct address per entry: same-address entries cluster into one venue.
        address: format!("1 {name} St"),
        date: date.to_string(),
        neighborhood: hood.to_string(),
        score,
        reasons: vec!["test reason".to_string()],
        description: None,
        url: String::new(),
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
    assert!(text.contains("3 new signal(s) at 3 venue(s)."));

    let md = render(&entries, 2, true, 7);
    assert!(md.contains("## 🔥 Strong signals"));
    assert!(md.contains("### Mission"));
    assert!(md.contains("- **strong-mission** — 1 strong-mission St — score 5 — 1 filing"));
    assert!(md.contains("  - 2026-07-03 [business] strong-mission: test reason"));
}

#[test]
fn digest_min_score_filter() {
    let entries = vec![entry("low", "Mission", "2026-07-01", 2)];
    let text = render(&entries, 4, false, 7);
    assert!(text.contains("No new signals."));
}

#[test]
fn low_bucket_renders_only_below_default_min_score() {
    let entries = vec![entry("faint", "Mission", "2026-07-01", 1)];
    // Default threshold: nothing.
    assert!(render(&entries, 2, false, 7).contains("No new signals."));
    // --min-score 1: the Low bucket appears, after Strong/Watch.
    let text = render(&entries, 1, false, 7);
    assert!(text.contains("· Low signals\n"), "{text}");
    assert!(text.contains("faint — 1 faint St · score 1"), "{text}");
    assert!(!text.contains("Worth watching"), "empty buckets are not printed");
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

use sf_radar::digest::description_snippet;

#[test]
fn description_snippet_extraction_and_truncation() {
    // Only permit-type sources.
    assert_eq!(description_snippet("business", r#"{"description":"x"}"#), None);
    assert_eq!(description_snippet("permit", ""), None);
    assert_eq!(description_snippet("permit", "not json"), None);
    assert_eq!(description_snippet("permit", r#"{"description":"  "}"#), None);

    let raw = r#"{"description":"Tenant  improvement\n for   new cafe"}"#;
    assert_eq!(
        description_snippet("electrical", raw).as_deref(),
        Some("Tenant improvement for new cafe")
    );

    let long = "a".repeat(200);
    let raw = format!(r#"{{"description":"{long}"}}"#);
    let snippet = description_snippet("plumbing", &raw).unwrap();
    assert_eq!(snippet.chars().count(), 120);
    assert!(snippet.ends_with("..."));

    let exact = "b".repeat(120);
    let raw = format!(r#"{{"description":"{exact}"}}"#);
    assert_eq!(description_snippet("permit", &raw).as_deref(), Some(exact.as_str()));
}

#[test]
fn digest_orders_groups_and_entries_by_score() {
    let mut entries = vec![
        entry("alpha-low", "Alamo", "2026-07-01", 4),
        entry("zulu-high", "Zoo Heights", "2026-07-02", 5),
        entry("alpha-high", "Alamo", "2026-06-30", 6),
    ];
    entries[0].source = "permit".into();
    entries[1].source = "permit".into();
    entries[2].source = "permit".into();

    let text = render(&entries, 2, false, 7);
    let strong = &text[text.find("🔥 Strong signals").unwrap()..];
    // Alamo's best entry (score 6) beats Zoo Heights (score 5) despite the alphabet.
    assert!(strong.find("Alamo").unwrap() < strong.find("Zoo Heights").unwrap());
    // Within Alamo: score 6 before score 4.
    assert!(strong.find("alpha-high").unwrap() < strong.find("alpha-low").unwrap());
}

#[test]
fn digest_renders_description_lines() {
    let mut e = entry("with-desc", "Mission", "2026-07-01", 4);
    e.description = Some("Tenant improvement for new cafe".to_string());
    let plain = render(&[e.clone()], 2, false, 7);
    assert!(plain.contains("\n      Tenant improvement for new cafe\n"));
    let md = render(&[e], 2, true, 7);
    assert!(md.contains("\n    - *Tenant improvement for new cafe*\n"));
}
