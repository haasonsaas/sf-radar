use chrono::NaiveDate;
use sf_radar::abc::{dates_to_fetch, extract_nonce, parse_report, score_issued_license_type, score_license_type};

/// Two rows in the real report layout: an SF type-47 application with a DBA,
/// and an out-of-county row that must be filtered.
const SAMPLE: &str = r#"
<table id="license_report">
<tr><th>License Number</th><th>Status</th></tr>
<tr>
  <th><a href="/licensing/license-lookup/single-license/?RPTTYPE=12&LICENSE=681355">681355</a></th>
  <td>PEND</td>
  <td>47 | 2 </td>
  <td>11/30/2026 </td>
  <td>DBA: ALTO 88<br/> PLEASE SPEAR, LLC<br/> 88 SPEAR ST.,<br/> SAN FRANCISCO, CA  94105</td>
  <td style="white-space:nowrap;"> ONE SANSOME ST., SUITE 725<br/> SAN FRANCISCO, CA  94104</td>
  <td>PER/PRM</td>
  <td></td>
  <td>ESCROW CO 890 3RD ST</td>
  <td>24</td>
  <td>3800</td>
  <td>88 SPEAR ST.</td>
  <td>SAN FRANCISCO</td>
  <td>38</td>
  <td>94105</td>
  <td>ONE SANSOME ST., SUITE 725</td>
  <td>SAN FRANCISCO</td>
  <td>94104</td>
  <td>CA</td>
</tr>
<tr>
  <th><a href="/licensing/license-lookup/single-license/?RPTTYPE=12&LICENSE=999999">999999</a></th>
  <td>PEND</td>
  <td>21 | 1 </td>
  <td>11/30/2026 </td>
  <td>CORNER MART LLC<br/> 1 MAIN ST,<br/> SOUTH GATE, CA 90280</td>
  <td> 1 MAIN ST</td>
  <td>PER</td>
  <td></td>
  <td></td>
  <td>19</td>
  <td>1000</td>
  <td>1 MAIN ST</td>
  <td>SOUTH GATE</td>
  <td>19</td>
  <td>90280</td>
  <td>1 MAIN ST</td>
  <td>SOUTH GATE</td>
  <td>90280</td>
  <td>CA</td>
</tr>
</table>
"#;

#[test]
fn parses_sf_rows_and_filters_other_counties() {
    let apps = parse_report(SAMPLE);
    assert_eq!(apps.len(), 1, "only the county-38 row should survive");
    let a = &apps[0];
    assert_eq!(a.license_number, "681355");
    assert_eq!(a.status, "PEND");
    assert_eq!(a.license_type, 47);
    assert_eq!(a.action, "PER/PRM");
    assert_eq!(a.dba, "ALTO 88");
    assert_eq!(a.owner, "PLEASE SPEAR, LLC");
    assert_eq!(a.street, "88 SPEAR ST.");
    assert_eq!(a.city, "SAN FRANCISCO");
    assert_eq!(a.zip, "94105");
    assert_eq!(a.name(), "ALTO 88");
}

#[test]
fn owner_without_dba_line() {
    // Same layout but the owner cell has no "DBA:" prefix.
    let html = SAMPLE.replace("DBA: ALTO 88<br/> PLEASE SPEAR, LLC", "PLEASE SPEAR, LLC");
    let apps = parse_report(&html);
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].dba, "");
    assert_eq!(apps[0].owner, "PLEASE SPEAR, LLC");
    assert_eq!(apps[0].name(), "PLEASE SPEAR, LLC");
}

#[test]
fn license_type_scoring() {
    for t in [41, 47, 75] {
        let (score, reasons) = score_license_type(t, "ORI");
        assert_eq!(score, 3, "type {t} is a restaurant license");
        assert!(reasons[0].contains("restaurant"));
    }
    for t in [40, 42, 48, 61] {
        let (score, reasons) = score_license_type(t, "ORI");
        assert_eq!(score, 3, "type {t} is a bar license");
        assert!(reasons[0].contains("bar"));
    }
    for t in [20, 21] {
        assert_eq!(score_license_type(t, "ORI").0, 2, "type {t} is off-sale");
    }
    // Wholesale/importer/caterer types are dropped.
    for t in [9, 14, 17, 58, 77] {
        assert_eq!(score_license_type(t, "ORI").0, 0, "type {t} should not score");
    }
}

#[test]
fn issued_license_scores_one_above_application() {
    for t in [41, 47, 75, 40, 42, 48, 61] {
        let (score, reasons) = score_issued_license_type(t, "ORI");
        assert_eq!(score, 4, "type {t} issued is a strong signal");
        assert!(reasons[0].starts_with("liquor license issued:"), "{reasons:?}");
    }
    assert_eq!(score_issued_license_type(20, "ORI").0, 3);
    assert_eq!(score_issued_license_type(58, "ORI").0, 0, "non-venue types still dropped");
}

#[test]
fn nonce_extraction() {
    let page = r#"<input type="hidden" id="abclqs_daily_report" name="abclqs_daily_report" value="ea2a54ebea" />"#;
    assert_eq!(extract_nonce(page).as_deref(), Some("ea2a54ebea"));
    assert_eq!(extract_nonce("<html></html>"), None);
}

#[test]
fn date_window_logic() {
    let end = NaiveDate::from_ymd_opt(2026, 8, 22).unwrap();

    // No watermark: backfill window ending at `end`.
    let dates = dates_to_fetch(None, None, end);
    assert_eq!(dates.len() as i64, sf_radar::abc::BACKFILL_DAYS + 1);
    assert_eq!(*dates.last().unwrap(), end);

    // Watermark yesterday: fetch just `end`.
    let dates = dates_to_fetch(Some("2026-08-21"), None, end);
    assert_eq!(dates, vec![end]);

    // Watermark current: nothing to fetch.
    assert!(dates_to_fetch(Some("2026-08-22"), None, end).is_empty());

    // A long gap is capped at 30 days, oldest first.
    let dates = dates_to_fetch(Some("2020-01-01"), None, end);
    assert_eq!(dates.len(), 31);
    assert_eq!(*dates.first().unwrap(), NaiveDate::from_ymd_opt(2026, 7, 23).unwrap());

    // Explicit --since wins over the watermark.
    let dates = dates_to_fetch(Some("2026-08-01"), Some("2026-08-20"), end);
    assert_eq!(dates.len(), 3);
}

#[test]
fn action_codes_adjust_scores() {
    // Original license and premises transfers are new venues: full score.
    assert_eq!(score_license_type(47, "ORI").0, 3);
    assert_eq!(score_license_type(47, "PER/PRM").0, 3);
    assert_eq!(score_license_type(47, "PRM").0, 3);
    assert!(score_license_type(47, "ORI").1[0].ends_with("(restaurant), new license"));
    assert!(score_license_type(47, "PER/PRM").1[0].ends_with("premises transfer"));

    // Person-to-person transfer at the same premises: ownership change, -1.
    let (score, reasons) = score_license_type(47, "PER");
    assert_eq!(score, 2);
    assert!(reasons[0].contains("ownership transfer at existing premises"));
    assert_eq!(score_issued_license_type(48, "PER").0, 3);
    assert_eq!(score_license_type(20, "PER").0, 1, "off-sale transfer drops below storage");

    // Unknown/empty action: no adjustment, no note.
    let (score, reasons) = score_license_type(41, "");
    assert_eq!(score, 3);
    assert!(reasons[0].ends_with("(restaurant)"));
}
