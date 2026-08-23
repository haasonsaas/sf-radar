use serde_json::Value;

/// Get a trimmed string field from a Socrata row ("" if missing or not a string).
pub fn field<'a>(row: &'a Value, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

/// Get a numeric field, tolerating Socrata's habit of returning numbers as strings.
pub fn field_num(row: &Value, key: &str) -> Option<f64> {
    match row.get(key) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Building-permit description keywords (+2). "bar" is NOT in this list — as
/// a substring it matches "rebar", "grab bar", and "wet bar"; it's handled by
/// `has_bar_keyword` as a standalone word instead.
const PERMIT_KEYWORDS: &[&str] = &[
    "restaurant",
    "cafe",
    "coffee",
    "bakery",
    "boba",
    "tea shop",
    "ice cream",
    "gelato",
    "taqueria",
    "pizzeria",
    "pizza",
    "ramen",
    "sushi",
    "izakaya",
    "poke",
    "deli",
    "bistro",
    "brewery",
    "taproom",
    "brewpub",
    "winery",
    "tasting room",
    "food hall",
    "juice",
    "grocery",
    "butcher",
    "retail",
    "storefront",
    "salon",
    "gym",
];

const FOOD_LIC_KEYWORDS: &[&str] = &["RESTAURANT", "FOOD", "CAFE", "CATERING", "BAR"];

/// Score a registered business location row. Returns (score, human-readable reasons).
pub fn score_business(row: &Value) -> (u32, Vec<String>) {
    let mut score = 0;
    let mut reasons = Vec::new();

    let lic = field(row, "lic_code_description");
    let naics = field(row, "self_reported_naics_code");
    let lic_upper = lic.to_uppercase();

    let is_food =
        naics.starts_with("722") || FOOD_LIC_KEYWORDS.iter().any(|k| lic_upper.contains(k));
    // 3121 = beverage manufacturing: breweries, wineries, distilleries —
    // in SF these almost always open with a taproom or tasting room.
    let is_beverage = naics.starts_with("3121");
    let is_retail = naics.starts_with("44") || naics.starts_with("45");

    if is_food {
        score += 2;
        reasons.push(format!("food service (lic: {lic}, NAICS: {naics})"));
    } else if is_beverage {
        score += 2;
        reasons.push(format!("beverage producer (NAICS: {naics})"));
    } else if is_retail {
        score += 2;
        reasons.push(format!("retail (NAICS: {naics})"));
    }

    let dba = field(row, "dba_name");
    let ownership = field(row, "ownership_name");
    if !dba.is_empty() && !dba.eq_ignore_ascii_case(ownership) {
        score += 1;
        reasons.push(format!("new DBA \"{dba}\" differs from owner \"{ownership}\""));
    }

    (score, reasons)
}

/// Score a building permit row. Returns (score, human-readable reasons).
pub fn score_permit(row: &Value) -> (u32, Vec<String>) {
    let mut score = 0;
    let mut reasons = Vec::new();

    let desc = field(row, "description").to_lowercase();

    let mut hits: Vec<&str> = PERMIT_KEYWORDS
        .iter()
        .copied()
        .filter(|k| desc.contains(k))
        .collect();
    if has_bar_keyword(&desc) {
        hits.push("bar");
    }
    if !hits.is_empty() {
        score += 2;
        reasons.push(format!("keyword: {}", hits.join(", ")));
    }

    if desc.contains("change of use") {
        score += 1;
        reasons.push("change of use".to_string());
    }
    if desc.contains("tenant improvement") {
        score += 1;
        reasons.push("tenant improvement".to_string());
    }
    // Word-boundary "new": "new restaurant..." yes, "newly renovated..." no.
    if desc.split_whitespace().next() == Some("new") || desc.contains("new business") {
        score += 1;
        reasons.push("new business".to_string());
    }

    let proposed_use = field(row, "proposed_use").to_lowercase();
    if ["restaurant", "retail", "store"]
        .iter()
        .any(|k| proposed_use.contains(k))
    {
        score += 1;
        reasons.push(format!("proposed use: {}", field(row, "proposed_use")));
    }

    if let Some(cost) = field_num(row, "estimated_cost")
        && cost > 100_000.0
    {
        score += 1;
        reasons.push(format!("estimated cost ${cost:.0}"));
    }

    (score, reasons)
}

/// Score a Places of Entertainment row (snapshot dataset — every row is a venue).
pub fn score_entertainment(row: &Value) -> (u32, Vec<String>) {
    let mut score = 2;
    let mut reasons = vec!["new entertainment venue".to_string()];
    let license_type = field(row, "license_type");
    if !license_type.is_empty() {
        score += 1;
        reasons.push(format!("license type: {license_type}"));
    }
    (score, reasons)
}

/// Score a Shared Spaces sidewalk registration (tables & chairs / display
/// merchandise). Sidewalk seating is a food-service tell; a merchandise
/// display is retail. Booleans arrive as real JSON booleans, not strings.
pub fn score_tables_chairs(row: &Value) -> (u32, Vec<String>) {
    let flag = |key: &str| row.get(key).and_then(Value::as_bool).unwrap_or(false);
    if flag("tablesandchairs") {
        (
            3,
            vec!["sidewalk tables & chairs registration — outdoor seating".to_string()],
        )
    } else if flag("displaymerchandise") {
        (
            2,
            vec!["sidewalk merchandise display registration".to_string()],
        )
    } else {
        (2, vec!["Shared Spaces sidewalk registration".to_string()])
    }
}

/// Score a Mobile Food Facility permit row.
pub fn score_mobile_food(row: &Value) -> (u32, Vec<String>) {
    let mut score = 2;
    let mut reasons = vec!["new mobile food permit".to_string()];
    if field(row, "facilitytype").eq_ignore_ascii_case("truck") {
        score += 1;
        reasons.push("food truck".to_string());
    }
    (score, reasons)
}

/// Strong keywords in trade-permit descriptions: near-certain food/retail
/// buildouts (+2). "commercial kitchen" is strong; "kitchen" alone is not.
/// "bar" is handled separately — as a substring it matches "wet bar",
/// "grab bar", and "towel bar" in residential remodels.
const TRADE_STRONG_KEYWORDS: &[&str] = &[
    "restaurant",
    "food service",
    "cafe",
    "cafeteria",
    "bakery",
    "commercial kitchen",
    "espresso",
    "brewery",
    "taproom",
    "pizza oven",
    // Commercial cold storage / hood tells. "walk-in" alone would match
    // "walk-in closet"; "hood" alone matches residential range hoods.
    "walk-in cooler",
    "walk-in freezer",
    "type 1 hood",
];

/// Plumbing adds "grease" — a grease trap/interceptor is a restaurant tell.
const PLUMBING_STRONG_KEYWORDS: &[&str] = &[
    "restaurant",
    "food service",
    "cafe",
    "cafeteria",
    "bakery",
    "commercial kitchen",
    "espresso",
    "brewery",
    "taproom",
    "pizza oven",
    "walk-in cooler",
    "walk-in freezer",
    "type 1 hood",
    "grease",
];

/// Weak keyword (+1): "kitchen" alone is usually a residential remodel.
const TRADE_WEAK_KEYWORD: &str = "kitchen";

/// "bar" counts only as a standalone word that isn't part of a residential
/// compound: "wet bar", "grab bar", "towel bar", "handle bar".
fn has_bar_keyword(desc: &str) -> bool {
    let words: Vec<&str> = desc
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().enumerate().any(|(i, w)| {
        *w == "bar"
            && !matches!(
                i.checked_sub(1).map(|j| words[j]),
                Some("wet" | "grab" | "towel" | "handle")
            )
    })
}

fn score_trade_permit(row: &Value, strong_keywords: &[&str], cost_field: &str) -> (u32, Vec<String>) {
    let mut score = 0;
    let mut reasons = Vec::new();

    let desc = field(row, "description").to_lowercase();
    let mut hits: Vec<&str> = strong_keywords
        .iter()
        .copied()
        .filter(|k| desc.contains(k))
        .collect();
    if has_bar_keyword(&desc) {
        hits.push("bar");
    }
    if !hits.is_empty() {
        score += 2;
        reasons.push(format!("keyword: {}", hits.join(", ")));
    } else if desc.contains(TRADE_WEAK_KEYWORD) {
        score += 1;
        reasons.push(format!("keyword: {TRADE_WEAK_KEYWORD}"));
    }

    if let Some(valuation) = field_num(row, cost_field)
        && valuation > 50_000.0
    {
        score += 1;
        reasons.push(format!("valuation ${valuation:.0}"));
    }

    (score, reasons)
}

/// Score an Electrical Permit row.
pub fn score_electrical(row: &Value) -> (u32, Vec<String>) {
    score_trade_permit(row, TRADE_STRONG_KEYWORDS, "permit_valuation")
}

/// Score a Plumbing Permit row.
pub fn score_plumbing(row: &Value) -> (u32, Vec<String>) {
    score_trade_permit(row, PLUMBING_STRONG_KEYWORDS, "valuation")
}
