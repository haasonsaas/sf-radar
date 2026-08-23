//! California ABC liquor-license applications, scraped from the daily
//! "Report of New Applications" at abc.ca.gov (there is no Socrata dataset
//! for this). A type 41/47 application at an address is a near-certain
//! restaurant signal that predates even health inspections.
//!
//! Flow: GET the report page to extract the WordPress form nonce, then POST
//! one request per report date since the stored watermark. The response is a
//! statewide HTML table; rows are filtered to San Francisco (county code 38)
//! and parsed with plain string scanning — no HTML-parser dependency.

use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Local, NaiveDate};

const REPORT_PAGE: &str = "https://www.abc.ca.gov/licensing/licensing-reports/new-applications/";
const REPORT_POST: &str = "https://www.abc.ca.gov/wp-admin/admin-post.php";
/// County code for San Francisco in ABC reports.
const SF_COUNTY_CODE: &str = "38";
/// Days of daily reports to pull when there is no watermark (each day is one
/// HTTP request, so this is deliberately shorter than the Socrata backfill).
pub const BACKFILL_DAYS: i64 = 14;
/// Never fetch more than this many report days in one run, even after a gap.
const MAX_DAYS_PER_RUN: i64 = 30;

#[derive(Debug, Clone, PartialEq)]
pub struct AbcApplication {
    pub license_number: String,
    pub status: String,
    pub license_type: u32,
    pub dba: String,   // "" when the application has no DBA line
    pub owner: String,
    pub street: String,
    pub city: String,
    pub zip: String,
}

impl AbcApplication {
    /// Display name: DBA when present, owner otherwise.
    pub fn name(&self) -> &str {
        if self.dba.is_empty() { &self.owner } else { &self.dba }
    }
}

/// Score an application by license type. On-sale eating places and bars are
/// strong signals; off-sale (shops) are moderate; everything else (wholesale,
/// importers, caterers, events) is dropped.
pub fn score_license_type(license_type: u32) -> (u32, Vec<String>) {
    match license_type {
        // 41 = on-sale beer & wine eating place, 47 = on-sale general eating
        // place, 75 = brewpub-restaurant.
        41 | 47 | 75 => (
            3,
            vec![format!("liquor license application: type {license_type} (restaurant)")],
        ),
        // 40/61 = on-sale beer, 42 = on-sale beer & wine public premises,
        // 48 = on-sale general public premises (bar/nightclub).
        40 | 42 | 48 | 61 => (
            3,
            vec![format!("liquor license application: type {license_type} (bar)")],
        ),
        // 20 = off-sale beer & wine, 21 = off-sale general (shops).
        20 | 21 => (
            2,
            vec![format!("liquor license application: type {license_type} (off-sale shop)")],
        ),
        _ => (0, Vec::new()),
    }
}

/* ---------- HTML scanning helpers (no parser dependency) ---------- */

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&#038;", "&")
        .replace("&#8217;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    unescape(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The `<td>` cell bodies of one `<tr>` chunk, tags stripped.
fn row_cells(row: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut rest = row;
    while let Some(start) = rest.find("<td") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else { break };
        let body = &after[open_end + 1..];
        let Some(close) = body.find("</td>") else { break };
        cells.push(strip_tags(&body[..close]));
        rest = &body[close + 5..];
    }
    cells
}

/// The DBA (if any) and owner from the raw owner/premises cell, whose lines
/// are `<br/>`-separated: ["DBA: X", owner, street..., "city, CA zip"].
fn dba_and_owner(row: &str) -> (String, String) {
    // Owner cell is the 4th <td>; re-scan raw HTML for its body.
    let mut rest = row;
    for _ in 0..3 {
        let Some(start) = rest.find("<td") else { return (String::new(), String::new()) };
        let Some(close) = rest[start..].find("</td>") else { return (String::new(), String::new()) };
        rest = &rest[start + close + 5..];
    }
    let Some(start) = rest.find("<td") else { return (String::new(), String::new()) };
    let after = &rest[start..];
    let Some(open_end) = after.find('>') else { return (String::new(), String::new()) };
    let body = &after[open_end + 1..];
    let Some(close) = body.find("</td>") else { return (String::new(), String::new()) };

    let lines: Vec<String> = body[..close]
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n")
        .split('\n')
        .map(strip_tags)
        .filter(|l| !l.is_empty())
        .collect();
    match lines.first() {
        Some(first) if first.to_uppercase().starts_with("DBA:") => (
            first[4..].trim().to_string(),
            lines.get(1).cloned().unwrap_or_default(),
        ),
        Some(first) => (String::new(), first.clone()),
        None => (String::new(), String::new()),
    }
}

/// Parse one daily report's HTML into San Francisco applications.
/// Column order (after the license-number `<th>`): status, type|dup, expiry,
/// owner+premises, mailing, action, conditions, escrow, district, geo,
/// prem street, city, county, zip, ...
pub fn parse_report(html: &str) -> Vec<AbcApplication> {
    let mut out = Vec::new();
    for row in html.split("<tr").skip(1) {
        let row = match row.find("</tr>") {
            Some(end) => &row[..end],
            None => row,
        };
        // License number lives in the row's <th><a ...>NUMBER</a></th>.
        let Some(license_number) = row
            .find("LICENSE=")
            .map(|i| &row[i + 8..])
            .and_then(|s| s.find('"').map(|e| s[..e].to_string()))
        else {
            continue; // header row or malformed
        };

        let cells = row_cells(row);
        if cells.len() < 14 || cells[12] != SF_COUNTY_CODE {
            continue;
        }
        let license_type = cells[1]
            .split('|')
            .next()
            .unwrap_or("")
            .trim()
            .parse::<u32>()
            .unwrap_or(0);
        let (dba, owner) = dba_and_owner(row);
        out.push(AbcApplication {
            license_number,
            status: cells[0].clone(),
            license_type,
            dba,
            owner,
            street: cells[10].clone(),
            city: cells[11].clone(),
            zip: cells[13].clone(),
        });
    }
    out
}

/* ---------- fetching ---------- */

pub struct AbcClient {
    client: reqwest::blocking::Client,
    nonce: String,
}

impl AbcClient {
    /// GET the report page and extract the form nonce the POST requires.
    pub fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("sf-radar/0.1")
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        let page = client.get(REPORT_PAGE).send()?.error_for_status()?.text()?;
        let nonce = extract_nonce(&page)
            .ok_or_else(|| anyhow!("abc.ca.gov report page has no form nonce — layout changed?"))?;
        Ok(Self { client, nonce })
    }

    /// Fetch and parse the new-applications report for one date.
    pub fn new_applications(&self, date: NaiveDate) -> Result<Vec<AbcApplication>> {
        let resp = self
            .client
            .post(REPORT_POST)
            .form(&[
                ("action", "abclqs_daily_report"),
                ("url", "/licensing/licensing-reports/new-applications/"),
                ("rpttype", "2"),
                ("abclqs_daily_report", &self.nonce),
                ("_wp_http_referer", "/licensing/licensing-reports/new-applications/"),
                ("abclqs-date", &date.format("%m/%d/%Y").to_string()),
            ])
            .send()?
            .error_for_status()
            .with_context(|| format!("abc report for {date}"))?;
        Ok(parse_report(&resp.text()?))
    }
}

pub fn extract_nonce(page: &str) -> Option<String> {
    let i = page.find("name=\"abclqs_daily_report\" value=\"")?;
    let rest = &page[i + 34..];
    let end = rest.find('"')?;
    let nonce = &rest[..end];
    (!nonce.is_empty()).then(|| nonce.to_string())
}

/// The report dates to fetch this run: from the day after `watermark` (or the
/// backfill window) through `end`, capped at MAX_DAYS_PER_RUN. `end` should
/// be yesterday — today's report may still be filling in.
pub fn dates_to_fetch(watermark: Option<&str>, since: Option<&str>, end: NaiveDate) -> Vec<NaiveDate> {
    let parse = |s: &str| NaiveDate::parse_from_str(&s[..10.min(s.len())], "%Y-%m-%d").ok();
    let start = match (since.and_then(parse), watermark.and_then(parse)) {
        (Some(s), _) => s,
        (None, Some(w)) => w + Duration::days(1),
        (None, None) => end - Duration::days(BACKFILL_DAYS),
    };
    let start = start.max(end - Duration::days(MAX_DAYS_PER_RUN));
    let mut dates = Vec::new();
    let mut d = start;
    while d <= end {
        dates.push(d);
        d += Duration::days(1);
    }
    dates
}

/// Yesterday — the newest report day fetched (today's may be incomplete).
pub fn newest_report_day() -> NaiveDate {
    Local::now().date_naive() - Duration::days(1)
}
