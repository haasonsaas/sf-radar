# sf-radar

A CLI that scans San Francisco public filings for early signs of new restaurants, cafes, and stores opening — before they show up on Yelp or Google Maps.

Rows from multiple SF open-data sources are deduped into a local SQLite database (one generic `signals` table), scored with simple heuristics, and rendered as a digest grouped by signal strength and neighborhood.

## Data sources

| Key | Socrata dataset | What it signals |
| --- | --- | --- |
| `business` | `g8m3-pdis` Registered Business Locations | New DBAs, food/retail NAICS codes |
| `permit` | `i98e-djp9` Building Permits | Tenant improvements, change-of-use, buildout costs |
| `entertainment` | `76g9-59eq` Places of Entertainment | New venues (snapshot dataset, no date field — new rows surface on first appearance) |
| `health` | `tvy3-wexg` Health Inspections 2024+ | A business's first health inspection since 2024 — usually means it's about to open |
| `mobile_food` | `rqzj-sfat` Mobile Food Facility Permits | New food trucks and carts |
| `electrical` | `ftty-kx6y` Electrical Permits | Kitchen/restaurant wiring buildouts |
| `plumbing` | `a6aw-rudh` Plumbing Permits | Grease traps and restaurant plumbing buildouts |

## Usage

```bash
# First run: backfill the last 90 days
cargo run -- fetch --full

# See what's opening
cargo run -- digest --days 30

# Markdown output, higher signal threshold
cargo run -- digest --days 7 --min-score 3 --md

# Only one neighborhood (case-insensitive substring match)
cargo run -- digest --days 30 --neighborhood "Mission Bay"
```

Weekly cron example:

```
0 8 * * 1 cd /path/to/sf-radar && cargo run --release -q -- fetch && cargo run --release -q -- digest
```

Data is stored in `~/.local/share/sf-radar/radar.db` (override with `--db`). If you hit Socrata rate limits, get a free app token from data.sfgov.org and export `SOCRATA_APP_TOKEN`.

## Commands

| Command | What it does |
| --- | --- |
| `sf-radar fetch [--full] [--since DATE]` | Incrementally pull all sources; `--full` backfills 90 days |
| `sf-radar digest [--days N] [--min-score N] [--neighborhood NAME] [--md]` | Print unseen signals and mark them seen |
| `sf-radar init` | Create the database (auto-run by other commands) |

## How scoring works

- **Business registrations**: +2 for food-service (NAICS 722x) or retail (NAICS 44/45) filings, +1 when the DBA name differs from the ownership entity
- **Building permits**: +2 for description keywords (restaurant, cafe, coffee, bakery, bar, boba, retail, storefront…), +1 for "change of use" / "tenant improvement", +1 for a restaurant/retail proposed use, +1 for buildout cost over $100k
- **Entertainment venues**: +2 base, +1 when a license type is listed
- **Health inspections**: +3 for a permit number's first inspection since 2024 (routine re-inspections are dropped at ingest). The initial 2024–now backfill is stored pre-seen — every existing facility has a "first since 2024", so only first-inspections found by incremental fetches after the radar is live alert
- **Mobile food**: +2 base, +1 for trucks
- **Electrical / plumbing permits**: +2 for strong buildout keywords (restaurant, food service, cafe, cafeteria, bakery, commercial kitchen — plus "grease" for plumbing, and "bar" as a standalone word excluding wet/grab/towel bar), +1 for "kitchen" alone, +1 for valuation over $50k; only rows scoring ≥ 2 are stored

**Corroboration**: at digest time, an entry gets +2 when other sources have filings at the same address (addresses are normalized across formats — case, whitespace, ST/STREET, 1ST/FIRST, unit designators stripped), and +1 when other sources have filings under the same name (names normalized — case, punctuation, entity suffixes like LLC/INC dropped; minimum length 4). Name matches catch pairs whose addresses don't normalize equal, like a food truck's commissary vs its storefront. Corroboration matches against full DB history including already-seen rows, and the bonuses are display-only — stored scores are untouched.

Digest buckets: 🔥 score ≥ 4, 👀 score 2–3. Entries are labeled with their source, e.g. `[health]`. Permit-type entries show a one-line snippet of the actual permit description (from the stored raw row, truncated to ~120 chars). Within a bucket, neighborhoods and entries are ordered by score then date, so the strongest signals come first. After printing, the digest also archives (marks seen) every unseen row older than the lookback window and reports the count — stale backfill rows don't linger forever.

## Adding a source

Sources are declarative configs in `src/sources.rs`. To add one, add a `Source` entry to the `all()` registry:

```rust
Source {
    key: "my_source",                       // DB/watermark key and digest label
    dataset: "abcd-1234",                   // Socrata dataset id
    date_field: Some("filed_date"),         // or None for snapshot datasets
    min_store_score: 0,                     // rows scoring below this are dropped at ingest
    external_id: |r| f(r, "permit_number"), // dedupe key (composite ok)
    name: |r| f(r, "dba_name"),
    address: |r| f(r, "street_address"),
    neighborhood: |r| f(r, "analysis_neighborhood"),
    score: |r, _conn| score::score_my_source(r),
}
```

`fetch` and `digest` pick it up automatically: incremental fetching uses the per-source watermark (90-day backfill on first run), snapshot sources (`date_field: None`) are fetched whole each run and new rows surface because they weren't in `signals` before.
