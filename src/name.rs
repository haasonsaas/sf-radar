//! Name normalization for cross-source corroboration. The same business
//! appears as "Grasslands Bar & Lounge" (business DBA), "GRASSLANDS BAR &
//! LOUNGE" (health dba), or "Senor Sisig LLC" (mobile_food applicant).

/// Trailing entity suffixes dropped when matching names.
const ENTITY_SUFFIXES: &[&str] = &[
    "LLC",
    "INC",
    "CORP",
    "CORPORATION",
    "CO",
    "COMPANY",
    "LP",
    "LLP",
    "LTD",
];

/// Minimum normalized length to be matchable — avoids "ZOE"-grade false
/// positives from very short names.
pub const MIN_MATCH_LEN: usize = 4;

/// Normalize a business name: uppercase, drop everything non-alphanumeric
/// (keeping spaces), collapse whitespace, remove trailing entity suffixes
/// (repeatedly, so "ACME HOLDINGS CO LLC" -> "ACME HOLDINGS").
pub fn normalize_name(name: &str) -> String {
    let upper = name.to_uppercase();
    let cleaned: String = upper
        .chars()
        .filter(|c| *c != '\'' && *c != '\u{2019}') // possessives: "Domino's" -> "DOMINOS"
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let mut tokens: Vec<&str> = cleaned.split_whitespace().collect();
    while let Some(last) = tokens.last() {
        if ENTITY_SUFFIXES.contains(last) {
            tokens.pop();
        } else {
            break;
        }
    }
    tokens.join(" ")
}

/// Key for cross-source name matching: `normalize_name` plus plural folding,
/// so "SUPER DUPER BURGERS" (business DBA) matches "SUPER DUPER BURGER"
/// (health dba). Tokens of 4+ letters lose a trailing S unless it's "SS".
pub fn match_key(name: &str) -> String {
    normalize_name(name)
        .split(' ')
        .map(fold_plural)
        .collect::<Vec<_>>()
        .join(" ")
}

fn fold_plural(token: &str) -> &str {
    if token.len() > 3 && token.ends_with('S') && !token.ends_with("SS") {
        &token[..token.len() - 1]
    } else {
        token
    }
}

/// Whether a normalized name is long enough to corroborate on.
pub fn is_matchable(normalized: &str) -> bool {
    normalized.len() >= MIN_MATCH_LEN
}
