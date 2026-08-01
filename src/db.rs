use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::digest::DigestEntry;

/// One scored row on its way into the `signals` table.
pub struct Signal {
    pub source: &'static str,
    pub external_id: String,
    pub name: String,
    pub address: String,
    pub date: String, // YYYY-MM-DD
    pub neighborhood: String,
    pub raw: String, // raw JSON row
    pub score: u32,
    pub reasons: Vec<String>,
    pub first_seen: String, // YYYY-MM-DD, set by us on first insert
}

/// Open (creating if needed) the database, ensure the schema exists,
/// and migrate any legacy per-source tables.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    init_schema(&conn)?;
    migrate_legacy(&conn)?;
    Ok(conn)
}

pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS signals (
            source        TEXT NOT NULL,
            external_id   TEXT NOT NULL,
            name          TEXT NOT NULL DEFAULT '',
            address       TEXT NOT NULL DEFAULT '',
            date          TEXT NOT NULL DEFAULT '',
            neighborhood  TEXT NOT NULL DEFAULT '',
            raw           TEXT NOT NULL DEFAULT '',
            score         INTEGER NOT NULL DEFAULT 0,
            reasons       TEXT NOT NULL DEFAULT '',
            first_seen    TEXT NOT NULL DEFAULT '',
            seen          INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (source, external_id)
        );
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT ''
        );
        ",
    )?;
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    Ok(stmt.exists(params![name])?)
}

/// One-time migration from the legacy `businesses`/`permits` tables into
/// `signals` (preserving score/reasons/seen; first_seen = date), then drops
/// the old tables and renames their watermark keys to the new source keys.
pub fn migrate_legacy(conn: &Connection) -> Result<()> {
    if table_exists(conn, "businesses")? {
        conn.execute_batch(
            "
            INSERT OR IGNORE INTO signals
                (source, external_id, name, address, date, neighborhood, raw,
                 score, reasons, first_seen, seen)
            SELECT 'business', uniqueid,
                   CASE WHEN dba_name = '' THEN ownership_name ELSE dba_name END,
                   address, dba_start_date, neighborhood, '',
                   score, reasons, dba_start_date, seen
            FROM businesses;
            DROP TABLE businesses;
            UPDATE meta SET key = 'watermark:business' WHERE key = 'watermark:businesses';
            ",
        )?;
    }
    if table_exists(conn, "permits")? {
        conn.execute_batch(
            "
            INSERT OR IGNORE INTO signals
                (source, external_id, name, address, date, neighborhood, raw,
                 score, reasons, first_seen, seen)
            SELECT 'permit', permit_number, 'Permit ' || permit_number,
                   address, filed_date, neighborhood, '',
                   score, reasons, filed_date, seen
            FROM permits;
            DROP TABLE permits;
            UPDATE meta SET key = 'watermark:permit' WHERE key = 'watermark:permits';
            ",
        )?;
    }
    Ok(())
}

/// First 10 chars of an ISO timestamp -> YYYY-MM-DD, so string compares work.
pub fn date_only(s: &str) -> &str {
    s.get(..10).unwrap_or(s)
}

/// Normalize a Socrata date/timestamp to YYYY-MM-DD. Handles ISO timestamps
/// ("2026-07-31T00:00:00.000") and compact dates ("20260731", used by some
/// datasets like mobile food's `received`).
pub fn normalize_date(s: &str) -> String {
    let s = s.trim();
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..8])
    } else {
        date_only(s).to_string()
    }
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

/// Insert a signal, or refresh it on conflict. `date`, `first_seen`, and
/// `seen` are set on first insert only — re-fetches never move a signal's
/// date forward (matters for snapshot sources) or un-see it.
pub fn upsert_signal(conn: &Connection, s: &Signal) -> Result<()> {
    conn.execute(
        "INSERT INTO signals (source, external_id, name, address, date, neighborhood,
                              raw, score, reasons, first_seen, seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)
         ON CONFLICT(source, external_id) DO UPDATE SET
            name = excluded.name,
            address = excluded.address,
            neighborhood = excluded.neighborhood,
            raw = excluded.raw,
            score = excluded.score,
            reasons = excluded.reasons",
        params![
            s.source,
            s.external_id,
            s.name,
            s.address,
            s.date,
            s.neighborhood,
            s.raw,
            s.score,
            join_reasons(&s.reasons),
            s.first_seen,
        ],
    )?;
    Ok(())
}

/// Does `source` already have a signal whose external_id starts with
/// `id_prefix`? Used by the health source to detect first inspections.
pub fn has_signal_with_id_prefix(conn: &Connection, source: &str, id_prefix: &str) -> Result<bool> {
    let escaped = id_prefix
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let mut stmt = conn.prepare(
        "SELECT 1 FROM signals WHERE source = ?1 AND external_id LIKE ?2 ESCAPE '\\' LIMIT 1",
    )?;
    Ok(stmt.exists(params![source, format!("{escaped}%")])?)
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

pub fn unseen_signals(
    conn: &Connection,
    cutoff: &str,
    max_date: &str,
    min_score: u32,
    neighborhood: Option<&str>,
) -> Result<Vec<DigestEntry>> {
    let mut stmt = conn.prepare(
        "SELECT source, external_id, name, address, date, neighborhood, score, reasons
         FROM signals
         WHERE seen = 0 AND score >= ?1 AND date >= ?2 AND date <= ?3
           AND neighborhood LIKE ?4
         ORDER BY date DESC",
    )?;
    let rows = stmt.query_map(
        params![min_score, cutoff, max_date, neighborhood_pattern(neighborhood)],
        |r| {
            Ok(DigestEntry {
                source: r.get(0)?,
                id: r.get(1)?,
                name: r.get(2)?,
                address: r.get(3)?,
                date: r.get(4)?,
                neighborhood: r.get(5)?,
                score: r.get(6)?,
                reasons: split_reasons(&r.get::<_, String>(7)?),
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Mark digest entries as seen so future digests don't resurface them.
pub fn mark_seen(conn: &Connection, entries: &[DigestEntry]) -> Result<()> {
    let mut by_source: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in entries {
        by_source.entry(&e.source).or_default().push(&e.id);
    }
    for (source, ids) in by_source {
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql =
            format!("UPDATE signals SET seen = 1 WHERE source = ? AND external_id IN ({placeholders})");
        let mut query_params: Vec<&dyn rusqlite::ToSql> = vec![&source];
        query_params.extend(ids.iter().map(|id| id as &dyn rusqlite::ToSql));
        conn.execute(&sql, query_params.as_slice())?;
    }
    Ok(())
}
