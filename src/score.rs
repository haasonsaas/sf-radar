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

const PERMIT_KEYWORDS: &[&str] = &[
    "restaurant",
    "cafe",
    "coffee",
    "bakery",
    "bar",
    "boba",
    "tea shop",
    "ice cream",
    "gelato",
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
    let is_retail = naics.starts_with("44") || naics.starts_with("45");

    if is_food {
        score += 2;
        reasons.push(format!("food service (lic: {lic}, NAICS: {naics})"));
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

    let hits: Vec<&str> = PERMIT_KEYWORDS
        .iter()
        .copied()
        .filter(|k| desc.contains(k))
        .collect();
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
    if desc.starts_with("new") || desc.contains("new business") {
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
