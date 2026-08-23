use std::path::PathBuf;

use anyhow::Result;
use chrono::{Duration, Local};
use clap::{Parser, Subcommand};

use sf_radar::{abc, config, db, digest, score, socrata, sources};

const FULL_BACKFILL_DAYS: i64 = 90;

#[derive(Parser)]
#[command(
    name = "sf-radar",
    about = "Scan SF open data for early signs of new restaurants and stores opening"
)]
struct Cli {
    /// Path to the SQLite database
    #[arg(long, global = true, value_name = "PATH")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the database and schema
    Init,
    /// Pull new rows from all Socrata sources
    Fetch {
        /// Fetch rows on/after this date (YYYY-MM-DD), overriding the stored watermark
        #[arg(long, value_name = "YYYY-MM-DD")]
        since: Option<String>,
        /// Backfill the last 90 days
        #[arg(long)]
        full: bool,
    },
    /// Print a digest of likely openings and mark them seen
    Digest {
        /// Look back this many days
        #[arg(long, default_value_t = 7)]
        days: u32,
        /// Minimum score to include
        #[arg(long, default_value_t = 2)]
        min_score: u32,
        /// Only include rows whose neighborhood contains this text (case-insensitive)
        #[arg(long, value_name = "NAME")]
        neighborhood: Option<String>,
        /// Emit markdown instead of plain text
        #[arg(long)]
        md: bool,
        /// Emit machine-readable JSON instead of prose
        #[arg(long, conflicts_with = "md")]
        json: bool,
        /// Print the digest without marking signals seen or archiving stale ones
        #[arg(long)]
        dry_run: bool,
    },
}

fn default_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/share/sf-radar/radar.db")
}

fn days_ago(n: i64) -> String {
    (Local::now() - Duration::days(n))
        .format("%Y-%m-%dT00:00:00")
        .to_string()
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db.clone().unwrap_or_else(default_db_path);

    match cli.command {
        Command::Init => {
            db::open(&db_path)?;
            println!("Initialized database at {}", db_path.display());
        }
        Command::Fetch { since, full } => fetch(&db_path, since, full)?,
        Command::Digest {
            days,
            min_score,
            neighborhood,
            md,
            json,
            dry_run,
        } => digest_cmd(
            &db_path,
            days,
            min_score,
            neighborhood.as_deref(),
            md,
            json,
            dry_run,
        )?,
    }
    Ok(())
}

fn fetch(db_path: &std::path::Path, since: Option<String>, full: bool) -> Result<()> {
    let conn = db::open(db_path)?;
    let cfg = config::load(db_path);
    let client =
        socrata::SocrataClient::new(config::app_token(std::env::var("SOCRATA_APP_TOKEN").ok(), &cfg))?;
    if client.has_app_token() {
        println!("Using Socrata app token.");
    }

    let today = today();
    let mut total = 0usize;
    // One source failing (dataset outage, schema change) must not stop the
    // others from fetching; failures are collected and reported at the end.
    let mut failures: Vec<String> = Vec::new();
    for source in sources::all() {
        match fetch_source(&conn, &client, &source, since.as_deref(), full, &today) {
            Ok(stored) => total += stored,
            Err(e) => {
                eprintln!("  {} failed, skipping: {e:#}", source.key);
                failures.push(source.key.to_string());
            }
        }
    }

    // ABC liquor licenses come from a scraped HTML report, not Socrata.
    match fetch_abc(&conn, since.as_deref(), &today) {
        Ok(stored) => total += stored,
        Err(e) => {
            eprintln!("  abc failed, skipping: {e:#}");
            failures.push("abc".to_string());
        }
    }

    println!("\nDone — {total} signals stored in {}", db_path.display());
    if !failures.is_empty() {
        anyhow::bail!(
            "{} source(s) failed this run: {} (other sources were fetched and stored)",
            failures.len(),
            failures.join(", ")
        );
    }
    println!("\nTip: keep the radar warm with cron (`crontab -e`):");
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "sf-radar".to_string());
    println!("  0 */6 * * * {exe} fetch --db {} >> /tmp/sf-radar.log 2>&1", db_path.display());
    Ok(())
}

/// Fetch one Socrata source since its watermark (or the backfill window),
/// score and store its rows, and advance the watermark. Returns rows stored.
fn fetch_source(
    conn: &rusqlite::Connection,
    client: &socrata::SocrataClient,
    source: &sources::Source,
    since: Option<&str>,
    full: bool,
    today: &str,
) -> Result<usize> {
    let watermark_key = format!("watermark:{}", source.key);
    let stored_watermark = db::get_watermark(conn, &watermark_key)?;
    let backfill_from = || {
        source
            .backfill_start
            .map(str::to_string)
            .unwrap_or_else(|| days_ago(FULL_BACKFILL_DAYS))
    };
    // Quiet backfill: only on the very first fetch of this source (no
    // stored watermark) and not for explicit --since runs.
    let quiet = source.quiet_backfill && stored_watermark.is_none() && since.is_none();

    let (rows, effective_since) = match source.date_field {
        Some(date_field) => {
            let effective_since = if let Some(s) = since {
                s.to_string()
            } else if full {
                backfill_from()
            } else {
                stored_watermark.clone().unwrap_or_else(backfill_from)
            };
            println!(
                "Fetching {} since {} ...",
                source.key,
                effective_since.get(..10).unwrap_or(&effective_since)
            );
            let rows = client.fetch_since(source.dataset, date_field, &effective_since)?;
            (rows, Some(effective_since))
        }
        None => {
            println!("Fetching {} (snapshot, full dataset) ...", source.key);
            (client.fetch_all(source.dataset)?, None)
        }
    };

    let mut max_date = effective_since
        .as_deref()
        .map(db::normalize_date)
        .unwrap_or_default();
    let mut stored = 0usize;
    for row in &rows {
        // Advance the watermark over every fetched row, even ones we don't store.
        if let Some(date_field) = source.date_field {
            let d = score::field(row, date_field);
            if !d.is_empty() {
                let d = db::normalize_date(d);
                if d > max_date {
                    max_date = d;
                }
            }
        }

        let external_id = (source.external_id)(row);
        if external_id.trim_matches('-').is_empty() {
            continue;
        }
        let (sc, reasons) = (source.score)(row, conn);
        if sc < source.min_store_score {
            continue;
        }
        let date = match source.date_field {
            Some(date_field) => db::normalize_date(score::field(row, date_field)),
            None => today.to_string(), // snapshot: date = first_seen
        };
        db::upsert_signal(
            conn,
            &db::Signal {
                source: source.key,
                external_id,
                name: (source.name)(row),
                address: (source.address)(row),
                date,
                neighborhood: (source.neighborhood)(row),
                raw: row.to_string(),
                score: sc,
                reasons,
                first_seen: today.to_string(),
                seen: quiet,
            },
        )?;
        stored += 1;
    }

    if effective_since.is_some() {
        // Bad source data can carry future dates; never let a watermark
        // move past today or incremental fetches would skip everything.
        if max_date.as_str() > today {
            max_date = today.to_string();
        }
        db::set_watermark(conn, &watermark_key, &max_date)?;
    }
    println!("  {} rows fetched, {stored} stored", rows.len());
    Ok(stored)
}

/// Fetch ABC liquor-license applications (scraped daily reports). One HTTP
/// request per report day since the watermark; SF rows scoring >= 2 are
/// stored under source "abc".
fn fetch_abc(conn: &rusqlite::Connection, since: Option<&str>, today: &str) -> Result<usize> {
    let watermark = db::get_watermark(conn, "watermark:abc")?;
    let dates = abc::dates_to_fetch(watermark.as_deref(), since, abc::newest_report_day());
    if dates.is_empty() {
        println!("Fetching abc (liquor licenses): up to date");
        return Ok(0);
    }
    println!(
        "Fetching abc (liquor licenses), {} report day(s) from {} ...",
        dates.len(),
        dates[0]
    );

    let client = abc::AbcClient::new()?;
    let mut stored = 0usize;
    // Each report day yields two requests: new applications (the earliest
    // signal) and issued licenses (opening imminent). An issued license is
    // stored as its own signal ("<license>-issued") so the venue timeline
    // shows both lifecycle events.
    let reports = [
        (abc::ReportKind::NewApplications, "", abc::score_license_type as fn(u32) -> (u32, Vec<String>)),
        (abc::ReportKind::IssuedLicenses, "-issued", abc::score_issued_license_type),
    ];
    for date in &dates {
        for (kind, id_suffix, score) in reports {
            for app in client.report(kind, *date)? {
                let (sc, reasons) = score(app.license_type);
                if sc < 2 {
                    continue;
                }
                db::upsert_signal(
                    conn,
                    &db::Signal {
                        source: "abc",
                        external_id: format!("{}{id_suffix}", app.license_number),
                        name: app.name().to_string(),
                        address: app.street.clone(),
                        date: date.format("%Y-%m-%d").to_string(),
                        neighborhood: String::new(),
                        raw: serde_json::json!({
                            "report": format!("{kind:?}"),
                            "license_number": app.license_number,
                            "status": app.status,
                            "license_type": app.license_type,
                            "dba": app.dba,
                            "owner": app.owner,
                            "street": app.street,
                            "city": app.city,
                            "zip": app.zip,
                        })
                        .to_string(),
                        score: sc,
                        reasons,
                        first_seen: today.to_string(),
                        seen: false,
                    },
                )?;
                stored += 1;
            }
            // Be polite to the report endpoint between requests.
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
    db::set_watermark(
        conn,
        "watermark:abc",
        &dates.last().unwrap().format("%Y-%m-%d").to_string(),
    )?;
    println!("  {stored} stored");
    Ok(stored)
}

fn digest_cmd(
    db_path: &std::path::Path,
    days: u32,
    min_score: u32,
    neighborhood: Option<&str>,
    md: bool,
    json: bool,
    dry_run: bool,
) -> Result<()> {
    let conn = db::open(db_path)?;
    let cutoff = (Local::now() - Duration::days(days as i64))
        .format("%Y-%m-%d")
        .to_string();

    let (entries, addresses) = {
        // Select below min_score so corroboration can lift sub-threshold rows;
        // the neighborhood filter is applied after inheritance, below.
        let floor = digest::selection_floor(min_score);
        let mut entries = db::unseen_signals(&conn, &cutoff, &today(), floor, None)?;
        // Display-time corroboration: +2 for other sources at the same
        // address, +1 for other sources under the same name. Not persisted.
        let rows = db::all_addresses(&conn)?;
        let names = digest::NameIndex::build(&rows);
        let addresses = digest::AddressIndex::build(rows);
        digest::apply_corroboration(&mut entries, &addresses, &names);
        digest::NeighborhoodIndex::build(db::neighborhoods_by_address(&conn)?).fill(&mut entries);
        if let Some(filter) = neighborhood {
            let filter = filter.to_lowercase();
            entries.retain(|e| e.neighborhood.to_lowercase().contains(&filter));
        }
        (entries, addresses)
    };
    // Only what the digest actually shows gets marked seen; rows selected
    // under the floor but still below min_score stay unseen for a later run.
    let shown: Vec<digest::DigestEntry> = digest::ordered_with(&entries, min_score, &addresses)
        .into_iter()
        .cloned()
        .collect();

    // --dry-run prints without mutating the database at all.
    let archived = if dry_run {
        0
    } else {
        db::mark_seen(&conn, &shown)?;
        db::archive_before(&conn, &cutoff, min_score)?
    };

    if json {
        println!("{}", digest::render_json_with(&entries, min_score, days, archived, &addresses));
    } else {
        print!("{}", digest::render_with(&entries, min_score, md, days, &addresses));
        if archived > 0 {
            println!("(archived {archived} signals older than the window)");
        }
    }
    Ok(())
}
