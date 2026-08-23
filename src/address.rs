//! Address normalization for cross-source corroboration. SF open data writes
//! the same building as "418 Sutter St", "418   SUTTER ST", or
//! "100 FIRST ST STE 165" vs "100 1st St" — normalize all to one form so
//! string equality works at the building level.

/// Unit designators: cut the address at the first one (building-level match).
const UNIT_TOKENS: &[&str] = &["APT", "UNIT", "STE", "SUITE", "RM", "FL", "#"];

/// Trailing street-suffix canonicalization. Covers the spelled-out forms,
/// USPS abbreviations, and DBI's two-letter permit abbreviations ("Bl",
/// "Wy", "Hy", "Tr") — ABC and business registrations use the long forms.
fn canonical_suffix(token: &str) -> &str {
    match token {
        "STREET" => "ST",
        "AVENUE" | "AV" => "AVE",
        "BOULEVARD" | "BL" => "BLVD",
        "DRIVE" => "DR",
        "ROAD" => "RD",
        "LANE" => "LN",
        "PLACE" => "PL",
        "COURT" => "CT",
        "TERRACE" | "TR" => "TER",
        "HIGHWAY" | "HY" => "HWY",
        "WY" => "WAY",
        "CIRCLE" | "CI" => "CIR",
        "PARKWAY" | "PW" => "PKWY",
        "ALLEY" | "AL" => "ALY",
        other => other,
    }
}

/// House-number canonicalization for the leading token: spelled-out numbers
/// ("ONE SANSOME ST" vs "1 Sansome St") and hyphenated ranges ("88-90 SPEAR
/// ST" vs "88 Spear St") both collapse to the first digit form.
fn canonical_house_number(token: &str) -> &str {
    match token {
        "ONE" => "1",
        "TWO" => "2",
        "THREE" => "3",
        "FOUR" => "4",
        "FIVE" => "5",
        "SIX" => "6",
        "SEVEN" => "7",
        "EIGHT" => "8",
        "NINE" => "9",
        "TEN" => "10",
        other => {
            let bytes = other.as_bytes();
            // "88-90" -> "88"; requires digits on both sides of the hyphen.
            if let Some(dash) = other.find('-')
                && dash > 0
                && bytes[..dash].iter().all(u8::is_ascii_digit)
                && bytes[dash + 1..].iter().all(u8::is_ascii_digit)
                && dash + 1 < bytes.len()
            {
                &other[..dash]
            } else {
                other
            }
        }
    }
}

/// Word ordinals -> digit form, so "100 FIRST ST" matches "100 1st St".
/// Covers the numbered streets/avenues where word forms actually appear.
fn canonical_ordinal(token: &str) -> &str {
    match token {
        "FIRST" => "1ST",
        "SECOND" => "2ND",
        "THIRD" => "3RD",
        "FOURTH" => "4TH",
        "FIFTH" => "5TH",
        "SIXTH" => "6TH",
        "SEVENTH" => "7TH",
        "EIGHTH" => "8TH",
        "NINTH" => "9TH",
        "TENTH" => "10TH",
        "ELEVENTH" => "11TH",
        "TWELFTH" => "12TH",
        "THIRTEENTH" => "13TH",
        "FOURTEENTH" => "14TH",
        "FIFTEENTH" => "15TH",
        "SIXTEENTH" => "16TH",
        "SEVENTEENTH" => "17TH",
        "EIGHTEENTH" => "18TH",
        "NINETEENTH" => "19TH",
        "TWENTIETH" => "20TH",
        other => other,
    }
}

/// Normalize an address: uppercase, strip commas/periods, collapse whitespace,
/// cut at the first unit designator, canonicalize ordinals and the trailing
/// street suffix. Returns "" for empty input.
pub fn normalize(addr: &str) -> String {
    let cleaned: String = addr
        .to_uppercase()
        .chars()
        .filter(|c| *c != ',' && *c != '.')
        .collect();
    let tokens: Vec<&str> = cleaned.split_whitespace().collect();

    let unit_at = tokens
        .iter()
        .position(|t| UNIT_TOKENS.contains(t))
        .unwrap_or(tokens.len());
    // A unit token as the very first token isn't a designator ("UNITED ...").
    let unit_at = if unit_at == 0 { tokens.len() } else { unit_at };

    let mut tokens: Vec<&str> = tokens[..unit_at].to_vec();
    for t in tokens.iter_mut() {
        *t = canonical_ordinal(t);
    }
    if let Some(first) = tokens.first_mut() {
        *first = canonical_house_number(first);
    }
    if let Some(last) = tokens.last_mut() {
        *last = canonical_suffix(last);
    }
    tokens.join(" ")
}
