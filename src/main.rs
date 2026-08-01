use std::path::PathBuf;

use anyhow::Result;
use chrono::{Duration, Local};
use clap::{Parser, Subcommand};

use sf_radar::{db, digest, score, socrata, sources};

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
        } => digest_cmd(&db_path, days, min_score, neighborhood.as_deref(), md)?,
    }
    Ok(())
}

fn fetch(db_path: &std::path::Path, since: Option<String>, full: bool) -> Result<()> {
    let conn = db::open(db_path)?;
    let client = socrata::SocrataClient::new()?;
    if client.has_app_token() {
        println!("Using SOCRATA_APP_TOKEN.");
    }

    let today = today();
    let mut total = 0usize;
    for source in sources::all() {
        let watermark_key = format!("watermark:{}", source.key);
        let stored_watermark = db::get_watermark(&conn, &watermark_key)?;
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
                let effective_since = if let Some(s) = &since {
                    s.clone()
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
            let (sc, reasons) = (source.score)(row, &conn);
            if sc < source.min_store_score {
                continue;
            }
            let date = match source.date_field {
                Some(date_field) => db::normalize_date(score::field(row, date_field)),
                None => today.clone(), // snapshot: date = first_seen
            };
            db::upsert_signal(
                &conn,
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
                    first_seen: today.clone(),
                    seen: quiet,
                },
            )?;
            stored += 1;
        }

        if effective_since.is_some() {
            // Bad source data can carry future dates; never let a watermark
            // move past today or incremental fetches would skip everything.
            if max_date > today {
                max_date.clone_from(&today);
            }
            db::set_watermark(&conn, &watermark_key, &max_date)?;
        }
        println!(
            "  {} rows fetched, {stored} stored",
            rows.len(),
        );
        total += stored;
    }

    println!("\nDone — {total} signals stored in {}", db_path.display());
    println!("\nTip: keep the radar warm with cron (`crontab -e`):");
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "sf-radar".to_string());
    println!("  0 */6 * * * {exe} fetch --db {} >> /tmp/sf-radar.log 2>&1", db_path.display());
    Ok(())
}

fn digest_cmd(
    db_path: &std::path::Path,
    days: u32,
    min_score: u32,
    neighborhood: Option<&str>,
    md: bool,
) -> Result<()> {
    let conn = db::open(db_path)?;
    let cutoff = (Local::now() - Duration::days(days as i64))
        .format("%Y-%m-%d")
        .to_string();

    let entries = {
        let mut entries = db::unseen_signals(&conn, &cutoff, &today(), min_score, neighborhood)?;
        // Display-time corroboration: +2 for other sources at the same
        // address, +1 for other sources under the same name. Not persisted.
        let rows = db::all_addresses(&conn)?;
        let names = digest::NameIndex::build(&rows);
        let addresses = digest::AddressIndex::build(rows);
        digest::apply_corroboration(&mut entries, &addresses, &names);
        entries
    };

    print!("{}", digest::render(&entries, min_score, md, days));

    db::mark_seen(&conn, &entries)?;
    let archived = db::archive_before(&conn, &cutoff)?;
    if archived > 0 {
        println!("(archived {archived} signals older than the window)");
    }
    Ok(())
}
