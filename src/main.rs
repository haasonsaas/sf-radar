use std::path::PathBuf;

use anyhow::Result;
use chrono::{Duration, Local};
use clap::{Parser, Subcommand};

use sf_radar::{db, digest, score, socrata};

const BUSINESS_DATASET: &str = "g8m3-pdis";
const PERMIT_DATASET: &str = "i98e-djp9";
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
    /// Pull new rows from both Socrata datasets
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

    let mut total = 0usize;
    for source in [
        SourceSpec {
            name: "businesses",
            dataset: BUSINESS_DATASET,
            date_field: "dba_start_date",
            id_field: "uniqueid",
        },
        SourceSpec {
            name: "permits",
            dataset: PERMIT_DATASET,
            date_field: "filed_date",
            id_field: "permit_number",
        },
    ] {
        let watermark_key = format!("watermark:{}", source.name);
        let effective_since = if full {
            days_ago(FULL_BACKFILL_DAYS)
        } else if let Some(s) = &since {
            s.clone()
        } else {
            db::get_watermark(&conn, &watermark_key)?.unwrap_or_else(|| days_ago(FULL_BACKFILL_DAYS))
        };

        println!(
            "Fetching {} since {} ...",
            source.name,
            effective_since.get(..10).unwrap_or(&effective_since)
        );
        let rows = client.fetch_since(source.dataset, source.date_field, &effective_since)?;

        let mut max_date = effective_since.clone();
        let mut upserted = 0usize;
        for row in &rows {
            if score::field(row, source.id_field).is_empty() {
                continue;
            }
            let (sc, reasons) = match source.name {
                "businesses" => score::score_business(row),
                _ => score::score_permit(row),
            };
            match source.name {
                "businesses" => db::upsert_business(&conn, row, sc, &reasons)?,
                _ => db::upsert_permit(&conn, row, sc, &reasons)?,
            }
            upserted += 1;

            let d = score::field(row, source.date_field);
            if !d.is_empty() && d > max_date.as_str() {
                max_date = d.to_string();
            }
        }
        db::set_watermark(&conn, &watermark_key, &max_date)?;
        println!("  {upserted} {} upserted (watermark -> {max_date})", source.name);
        total += upserted;
    }

    println!("\nDone — {total} rows upserted into {}", db_path.display());
    println!("\nTip: keep the radar warm with cron (`crontab -e`):");
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "sf-radar".to_string());
    println!("  0 */6 * * * {exe} fetch --db {} >> /tmp/sf-radar.log 2>&1", db_path.display());
    Ok(())
}

struct SourceSpec {
    name: &'static str,
    dataset: &'static str,
    date_field: &'static str,
    id_field: &'static str,
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

    let mut entries = db::unseen_businesses(&conn, &cutoff, min_score, neighborhood)?;
    entries.extend(db::unseen_permits(&conn, &cutoff, min_score, neighborhood)?);

    print!("{}", digest::render(&entries, min_score, md, days));

    let business_ids: Vec<String> = entries
        .iter()
        .filter(|e| e.source == "business")
        .map(|e| e.id.clone())
        .collect();
    let permit_ids: Vec<String> = entries
        .iter()
        .filter(|e| e.source == "permit")
        .map(|e| e.id.clone())
        .collect();
    db::mark_seen(&conn, "businesses", "uniqueid", &business_ids)?;
    db::mark_seen(&conn, "permits", "permit_number", &permit_ids)?;
    Ok(())
}
