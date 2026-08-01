use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::Value;

use crate::digest::DigestEntry;
use crate::score::field;

/// Open (creating if needed) the database and ensure the schema exists.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    init_schema(&conn)?;
    Ok(conn)
}

pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS businesses (
            uniqueid              TEXT PRIMARY KEY,
            dba_name              TEXT NOT NULL DEFAULT '',
            ownership_name        TEXT NOT NULL DEFAULT '',
            dba_start_date        TEXT NOT NULL DEFAULT '',
            location_start_date   TEXT NOT NULL DEFAULT '',
            lic_code_description  TEXT NOT NULL DEFAULT '',
            naics                 TEXT NOT NULL DEFAULT '',
            neighborhood          TEXT NOT NULL DEFAULT '',
            address               TEXT NOT NULL DEFAULT '',
            corridor              TEXT NOT NULL DEFAULT '',
            score                 INTEGER NOT NULL DEFAULT 0,
            reasons               TEXT NOT NULL DEFAULT '',
            seen                  INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS permits (
            permit_number   TEXT PRIMARY KEY,
            description     TEXT NOT NULL DEFAULT '',
            filed_date      TEXT NOT NULL DEFAULT '',
            issued_date     TEXT NOT NULL DEFAULT '',
            proposed_use    TEXT NOT NULL DEFAULT '',
            estimated_cost  TEXT NOT NULL DEFAULT '',
            neighborhood    TEXT NOT NULL DEFAULT '',
            address         TEXT NOT NULL DEFAULT '',
            score           INTEGER NOT NULL DEFAULT 0,
            reasons         TEXT NOT NULL DEFAULT '',
            seen            INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT ''
        );
        ",
    )?;
    Ok(())
}

/// First 10 chars of an ISO timestamp -> YYYY-MM-DD, so string compares work.
fn date_only(s: &str) -> &str {
    s.get(..10).unwrap_or(s)
}

fn join_reasons(reasons: &[String]) -> String {
    reasons.join("; ")
}

fn split_reasons(s: &str) -> Vec<String> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split("; ").map(str::to_string).collect()
    }
}

pub fn upsert_business(conn: &Connection, row: &Value, score: u32, reasons: &[String]) -> Result<()> {
    conn.execute(
        "INSERT INTO businesses (uniqueid, dba_name, ownership_name, dba_start_date,
                                 location_start_date, lic_code_description, naics,
                                 neighborhood, address, corridor, score, reasons)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(uniqueid) DO UPDATE SET
            dba_name = excluded.dba_name,
            ownership_name = excluded.ownership_name,
            dba_start_date = excluded.dba_start_date,
            location_start_date = excluded.location_start_date,
            lic_code_description = excluded.lic_code_description,
            naics = excluded.naics,
            neighborhood = excluded.neighborhood,
            address = excluded.address,
            corridor = excluded.corridor,
            score = excluded.score,
            reasons = excluded.reasons",
        params![
            field(row, "uniqueid"),
            field(row, "dba_name"),
            field(row, "ownership_name"),
            date_only(field(row, "dba_start_date")),
            date_only(field(row, "location_start_date")),
            field(row, "lic_code_description"),
            field(row, "self_reported_naics_code"),
            field(row, "neighborhoods_analysis_boundaries"),
            field(row, "full_business_address"),
            field(row, "business_corridor"),
            score,
            join_reasons(reasons),
        ],
    )?;
    Ok(())
}

pub fn upsert_permit(conn: &Connection, row: &Value, score: u32, reasons: &[String]) -> Result<()> {
    let address = permit_address(row);
    conn.execute(
        "INSERT INTO permits (permit_number, description, filed_date, issued_date,
                              proposed_use, estimated_cost, neighborhood, address,
                              score, reasons)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(permit_number) DO UPDATE SET
            description = excluded.description,
            filed_date = excluded.filed_date,
            issued_date = excluded.issued_date,
            proposed_use = excluded.proposed_use,
            estimated_cost = excluded.estimated_cost,
            neighborhood = excluded.neighborhood,
            address = excluded.address,
            score = excluded.score,
            reasons = excluded.reasons",
        params![
            field(row, "permit_number"),
            field(row, "description"),
            date_only(field(row, "filed_date")),
            date_only(field(row, "issued_date")),
            field(row, "proposed_use"),
            field(row, "estimated_cost"),
            field(row, "neighborhoods_analysis_boundaries"),
            address,
            score,
            join_reasons(reasons),
        ],
    )?;
    Ok(())
}

fn permit_address(row: &Value) -> String {
    let mut parts: Vec<&str> = vec![
        field(row, "street_number"),
        field(row, "street_name"),
        field(row, "street_suffix"),
    ];
    parts.retain(|p| !p.is_empty());
    let mut addr = parts.join(" ");
    let unit = field(row, "unit");
    if !unit.is_empty() {
        addr.push_str(&format!(", unit {unit}"));
    }
    addr
}

pub fn get_watermark(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = stmt.query(params![key])?;
    Ok(rows.next()?.map(|r| r.get(0)).transpose()?)
}

pub fn set_watermark(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// LIKE pattern for a neighborhood substring filter; "%" matches everything,
/// including rows with an empty neighborhood.
fn neighborhood_pattern(neighborhood: Option<&str>) -> String {
    neighborhood
        .map(|n| format!("%{n}%"))
        .unwrap_or_else(|| "%".to_string())
}

pub fn unseen_businesses(
    conn: &Connection,
    cutoff: &str,
    min_score: u32,
    neighborhood: Option<&str>,
) -> Result<Vec<DigestEntry>> {
    let mut stmt = conn.prepare(
        "SELECT uniqueid, dba_name, ownership_name, address, dba_start_date,
                neighborhood, score, reasons
         FROM businesses
         WHERE seen = 0 AND score >= ?1 AND dba_start_date >= ?2
           AND neighborhood LIKE ?3
         ORDER BY dba_start_date DESC",
    )?;
    let rows = stmt.query_map(params![min_score, cutoff, neighborhood_pattern(neighborhood)], |r| {
        let dba: String = r.get(1)?;
        let ownership: String = r.get(2)?;
        Ok(DigestEntry {
            source: "business",
            id: r.get(0)?,
            name: if dba.is_empty() { ownership } else { dba },
            address: r.get(3)?,
            date: r.get(4)?,
            neighborhood: r.get(5)?,
            score: r.get(6)?,
            reasons: split_reasons(&r.get::<_, String>(7)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn unseen_permits(
    conn: &Connection,
    cutoff: &str,
    min_score: u32,
    neighborhood: Option<&str>,
) -> Result<Vec<DigestEntry>> {
    let mut stmt = conn.prepare(
        "SELECT permit_number, address, filed_date, neighborhood, score, reasons
         FROM permits
         WHERE seen = 0 AND score >= ?1 AND filed_date >= ?2
           AND neighborhood LIKE ?3
         ORDER BY filed_date DESC",
    )?;
    let rows = stmt.query_map(params![min_score, cutoff, neighborhood_pattern(neighborhood)], |r| {
        let permit_number: String = r.get(0)?;
        Ok(DigestEntry {
            source: "permit",
            name: format!("Permit {permit_number}"),
            id: permit_number,
            address: r.get(1)?,
            date: r.get(2)?,
            neighborhood: r.get(3)?,
            score: r.get(4)?,
            reasons: split_reasons(&r.get::<_, String>(5)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Mark rows as seen so future digests don't resurface them.
/// `table` is an internal constant ("businesses" or "permits").
pub fn mark_seen(conn: &Connection, table: &str, id_column: &str, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!("UPDATE {table} SET seen = 1 WHERE {id_column} IN ({placeholders})");
    let params: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    conn.execute(&sql, params.as_slice())?;
    Ok(())
}
