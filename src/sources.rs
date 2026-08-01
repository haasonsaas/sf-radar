use rusqlite::Connection;
use serde_json::Value;

use crate::db;
use crate::score::{self, field};

/// Declarative config for one Socrata data source. Adding a source means
/// adding an entry to `all()` below — fetch/digest pick it up automatically.
pub struct Source {
    /// Short key used in the DB, watermarks, and digest labels (e.g. "[permit]").
    pub key: &'static str,
    /// Socrata dataset id.
    pub dataset: &'static str,
    /// Date field for incremental fetches. `None` = snapshot dataset: fetch
    /// the whole thing every run; new rows surface because they weren't in
    /// `signals` before (their date is set to first_seen on insert).
    pub date_field: Option<&'static str>,
    /// Only store rows scoring at least this (huge datasets filter at ingest).
    pub min_store_score: u32,
    pub external_id: fn(&Value) -> String,
    pub name: fn(&Value) -> String,
    pub address: fn(&Value) -> String,
    pub neighborhood: fn(&Value) -> String,
    /// Scores a row. Gets the DB connection so scorers can look at already-
    /// stored signals (health's "first inspection on record" needs this).
    pub score: fn(&Value, &Connection) -> (u32, Vec<String>),
}

fn f(row: &Value, key: &str) -> String {
    field(row, key).to_string()
}

fn street(row: &Value) -> String {
    let mut parts: Vec<&str> = vec![
        field(row, "street_number"),
        field(row, "street_name"),
        field(row, "street_suffix"),
    ];
    parts.retain(|p| !p.is_empty());
    parts.join(" ")
}

fn street_with_unit(row: &Value) -> String {
    let mut addr = street(row);
    let unit = field(row, "unit");
    if !unit.is_empty() {
        addr.push_str(&format!(", unit {unit}"));
    }
    addr
}

fn permit_name(row: &Value) -> String {
    format!("Permit {}", field(row, "permit_number"))
}

fn business_name(row: &Value) -> String {
    let dba = field(row, "dba_name");
    if dba.is_empty() {
        f(row, "ownership_name")
    } else {
        dba.to_string()
    }
}

fn no_neighborhood(_row: &Value) -> String {
    String::new()
}

/// Health inspections: +3 only when this is the first inspection on record
/// for the permit_number (i.e. nothing stored for it yet — ingest runs in
/// inspection_date order, so any stored row for the permit is earlier).
pub fn score_health(row: &Value, conn: &Connection) -> (u32, Vec<String>) {
    let permit = field(row, "permit_number");
    let has_earlier =
        db::has_signal_with_id_prefix(conn, "health", &format!("{permit}-")).unwrap_or(false);
    if has_earlier {
        (0, Vec::new())
    } else {
        (3, vec!["first health inspection on record".to_string()])
    }
}

fn health_id(row: &Value) -> String {
    format!(
        "{}-{}",
        field(row, "permit_number"),
        field(row, "inspection_date")
    )
}

pub fn all() -> Vec<Source> {
    vec![
        Source {
            key: "business",
            dataset: "g8m3-pdis",
            date_field: Some("dba_start_date"),
            min_store_score: 0,
            external_id: |r| f(r, "uniqueid"),
            name: business_name,
            address: |r| f(r, "full_business_address"),
            neighborhood: |r| f(r, "neighborhoods_analysis_boundaries"),
            score: |r, _conn| score::score_business(r),
        },
        Source {
            key: "permit",
            dataset: "i98e-djp9",
            date_field: Some("filed_date"),
            min_store_score: 0,
            external_id: |r| f(r, "permit_number"),
            name: permit_name,
            address: street_with_unit,
            neighborhood: |r| f(r, "neighborhoods_analysis_boundaries"),
            score: |r, _conn| score::score_permit(r),
        },
        Source {
            key: "entertainment",
            dataset: "76g9-59eq",
            date_field: None, // snapshot
            min_store_score: 0,
            external_id: |r| f(r, "permit_number"),
            name: |r| f(r, "dba_name"),
            address: |r| f(r, "street_address"),
            neighborhood: |r| f(r, "analysis_neighborhood"),
            score: |r, _conn| score::score_entertainment(r),
        },
        Source {
            key: "health",
            dataset: "tvy3-wexg",
            date_field: Some("inspection_date"),
            min_store_score: 1, // only store first-inspection signals
            external_id: health_id,
            name: |r| f(r, "dba"),
            address: |r| f(r, "street_address"),
            neighborhood: |r| f(r, "analysis_neighborhood"),
            score: score_health,
        },
        Source {
            key: "mobile_food",
            dataset: "rqzj-sfat",
            date_field: Some("received"),
            min_store_score: 0,
            external_id: |r| f(r, "objectid"),
            name: |r| f(r, "applicant"),
            address: |r| f(r, "address"),
            neighborhood: no_neighborhood,
            score: |r, _conn| score::score_mobile_food(r),
        },
        Source {
            key: "electrical",
            dataset: "ftty-kx6y",
            date_field: Some("filed_date"),
            min_store_score: 2,
            external_id: |r| f(r, "permit_number"),
            name: permit_name,
            address: street,
            neighborhood: |r| f(r, "neighborhoods_analysis_boundaries"),
            score: |r, _conn| score::score_electrical(r),
        },
        Source {
            key: "plumbing",
            dataset: "a6aw-rudh",
            date_field: Some("filed_date"),
            min_store_score: 2,
            external_id: |r| f(r, "permit_number"),
            name: permit_name,
            address: street,
            neighborhood: |r| f(r, "neighborhoods_analysis_boundaries"),
            score: |r, _conn| score::score_plumbing(r),
        },
    ]
}
