# sf-radar

A CLI that scans San Francisco public filings for early signs of new restaurants, cafes, and stores opening — before they show up on Yelp or Google Maps.

It continuously pulls two SF open-data sources:

- **Registered Business Locations** (`g8m3-pdis`) — new DBAs, NAICS codes, neighborhoods
- **Building Permits** (`i98e-djp9`) — tenant improvements, change-of-use, buildout costs

Rows are deduped into a local SQLite database, scored with simple heuristics (food/retail NAICS codes, permit keywords like "restaurant" or "tenant improvement", buildout cost), and rendered as a digest grouped by signal strength and neighborhood.

## Usage

```bash
# First run: backfill the last 90 days
cargo run -- fetch --full

# See what's opening
cargo run -- digest --days 30

# Markdown output, higher signal threshold
cargo run -- digest --days 7 --min-score 3 --md
```

Weekly cron example:

```
0 8 * * 1 cd /path/to/sf-radar && cargo run --release -q -- fetch && cargo run --release -q -- digest
```

Data is stored in `~/.local/share/sf-radar/radar.db` (override with `--db`). If you hit Socrata rate limits, get a free app token from data.sfgov.org and export `SOCRATA_APP_TOKEN`.

## Commands

| Command | What it does |
| --- | --- |
| `sf-radar fetch [--full] [--since DATE]` | Incrementally pull new filings; `--full` backfills 90 days |
| `sf-radar digest [--days N] [--min-score N] [--md]` | Score unseen rows and print the digest |
| `sf-radar init` | Create the database (auto-run by other commands) |

## How scoring works

- **Business registrations**: +2 for food-service (NAICS 722x) or retail (NAICS 44/45) filings, +1 when the DBA name differs from the ownership entity
- **Permits**: +2 for description keywords (restaurant, cafe, coffee, bakery, bar, boba, retail, storefront…), +1 for "change of use" / "tenant improvement", +1 for a restaurant/retail proposed use, +1 for buildout cost over $100k

Digest buckets: 🔥 score ≥ 4, 👀 score 2–3.
