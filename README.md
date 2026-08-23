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
| `tables_chairs` | `dpch-7nr4` Table and Chairs Registrations (Shared Spaces) | Sidewalk seating/display registrations — outdoor tables mean food service |
| `planning` | `qvu5-m3a2` Planning Department Records - Projects | Planning applications with food/retail descriptions — the earliest signal, months before permits |
| `fire` | `893e-xam6` Fire Permits | Standing place-of-assembly permits — venues licensing 50+ occupancy |
| `vending` | `34ws-kyf6` Street Vending Permits | New sidewalk vendors (snapshot dataset, no date field) |
| `abc` | abc.ca.gov daily "Report of New Applications" (scraped, not Socrata) | New liquor-license applications — a type 41/47 filing is a near-certain restaurant, earlier than any city record |
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

Data is stored in `~/.local/share/sf-radar/radar.db` (override with `--db`). Transient Socrata errors (429 rate limits, 5xx) are retried twice with backoff; if you hit rate limits persistently, get a free app token from data.sfgov.org and export `SOCRATA_APP_TOKEN` — or put it in `config.toml` next to the database (see Automation).

## Commands

| Command | What it does |
| --- | --- |
| `sf-radar fetch [--full] [--since DATE]` | Incrementally pull all sources; `--full` backfills 90 days. A source that fails is skipped (the others still store) and the run exits non-zero listing the failures |
| `sf-radar digest [--days N] [--min-score N] [--neighborhood NAME] [--md] [--json] [--dry-run]` | Print unseen signals and mark them seen |
| `sf-radar init` | Create the database (auto-run by other commands) |

## Automation

`digest --json` emits the digest as machine-readable JSON on stdout (conflicts with `--md`). Entries are ordered best-first, same as the prose digest; `score` and `bucket` include the display-time corroboration/name bonuses. `url` is the per-row page where the source has one — ABC license lookup for `abc`, the DBI permit tracker for `permit`, the Planning PIM page for `planning` — and `""` otherwise. `description` is the permit-description snippet, or `""` when the source has none. The prose digest prints the URL on its own line; the markdown digest links the name.

```json
{
  "tool": "sf-radar",
  "generated_at": "2026-08-02T03:13:00Z",
  "window_days": 30,
  "min_score": 2,
  "archived": 0,
  "entries": [
    {
      "source": "business",
      "id": "1234567-2026",
      "name": "Meek Coffee",
      "address": "2360 3rd St",
      "neighborhood": "Potrero Hill",
      "date": "2026-07-09",
      "score": 5,
      "bucket": "strong",
      "reasons": ["food-service NAICS", "corroborated by permit: Permit 202607123456 (2026-07-10)"],
      "url": "",
      "description": ""
    }
  ]
}
```

`--json` still marks entries seen and archives stale rows, so the next run only reports new signals. Pass `--dry-run` to print the digest (prose or JSON) without touching the database — useful for polling the same window repeatedly:

```bash
sf-radar digest --days 30 --json --dry-run | jq '.entries[] | select(.bucket == "strong")'
```

The Socrata app token can also live in `config.toml` in the same directory as the database (e.g. `~/.local/share/sf-radar/config.toml`):

```toml
socrata_app_token = "your-token-here"
```

The `SOCRATA_APP_TOKEN` environment variable wins over the config file. A missing config file is fine; an invalid one logs a warning on stderr and is ignored.

Cron example — fetch every 6 hours, mail a weekly JSON digest of strong signals:

```
0 */6 * * * /path/to/sf-radar fetch >> /tmp/sf-radar.log 2>&1
0 8 * * 1 /path/to/sf-radar digest --days 7 --json | jq -c '.entries[]' >> /tmp/sf-radar-digest.jsonl
```

## How scoring works

- **Business registrations**: +2 for food-service (NAICS 722x), beverage-producer (NAICS 3121x — breweries/wineries/distilleries), or retail (NAICS 44/45) filings, +1 when the DBA name differs from the ownership entity
- **Building permits**: +2 for description keywords (restaurant, cafe, coffee, bakery, boba, taqueria, pizzeria, ramen, sushi, deli, bistro, brewery, taproom, winery, juice, grocery, retail, storefront…), +1 for "change of use" / "tenant improvement", +1 for a restaurant/retail proposed use, +1 for buildout cost over $100k. "bar" counts only as a standalone word (not "rebar", "grab bar", "wet bar"), and "new" only as a leading word (not "newly renovated")
- **Entertainment venues**: +2 base, +1 when a license type is listed
- **Health inspections**: +3 for a permit number's first inspection since 2024 (routine re-inspections are dropped at ingest). The initial 2024–now backfill is stored pre-seen — every existing facility has a "first since 2024", so only first-inspections found by incremental fetches after the radar is live alert
- **Mobile food**: +2 base, +1 for trucks
- **Tables & chairs (Shared Spaces)**: +3 for a sidewalk tables-and-chairs registration (outdoor seating means food service), +2 for a merchandise-display registration
- **Planning applications**: +2 for food/retail keywords in the project description (including planning vocabulary: food service, takeout, outdoor dining, formula retail), +1 for "change of use"; only rows scoring ≥ 2 are stored (most planning records are housing). Keyword matching can't tell "change of use to restaurant" from "change of use from retail to office" — read the description snippet
- **Fire permits**: +3 for a standing place-of-assembly or commercial-cooking permit; temporary/special-event permits score 0 and are not stored. Standing permits renew annually, so the first fetch cycle surfaces some renewals of existing venues alongside genuinely new ones
- **Street vending**: +2 base, +1 when the goods description reads as food
- **ABC liquor licenses**: +3 for restaurant types (41 on-sale beer & wine eating place, 47 on-sale general eating place, 75 brewpub) and bar types (40/61 on-sale beer, 42/48 public premises), +2 for off-sale shop types (20/21); wholesale/importer/caterer types are dropped. This source is scraped from abc.ca.gov's daily statewide report (one HTTP request per report day, filtered to SF county, watermarked like the Socrata sources, 14-day initial backfill). A scrape failure prints a warning and skips the run instead of failing the whole fetch
- **Electrical / plumbing permits**: +2 for strong buildout keywords (restaurant, food service, cafe, cafeteria, bakery, commercial kitchen, espresso, brewery, taproom, pizza oven, walk-in cooler/freezer, type 1 hood — plus "grease" for plumbing, and "bar" as a standalone word excluding wet/grab/towel bar), +1 for "kitchen" alone, +1 for valuation over $50k; only rows scoring ≥ 2 are stored

**Corroboration**: at digest time, an entry gets +2 when other sources have filings at the same address (addresses are normalized across formats — case, whitespace, ST/STREET and DBI's two-letter permit abbreviations like BL/WY/HY, 1ST/FIRST, ONE/1 house numbers, 88-90/88 ranges, unit designators stripped), and +1 when other sources have filings under the same name (names normalized — case, punctuation, entity suffixes like LLC/INC dropped, plurals folded so BURGERS matches BURGER; minimum length 4). Name matches catch pairs whose addresses don't normalize equal, like a food truck's commissary vs its storefront. Corroboration matches against full DB history including already-seen rows, and the bonuses are display-only — stored scores are untouched. Candidates are selected down to a stored-score floor of `--min-score` minus the maximum bonus (never below 1), so a permit stored at score 1 surfaces once another source files at its address; rows that stay below `--min-score` after corroboration are not shown and not marked seen. Sources whose datasets carry no neighborhood (`abc`, `fire`, `planning`, `vending`) inherit one from any other filing at the same normalized address, and `--neighborhood` filters after that inheritance.

Digest buckets: 🔥 score ≥ 4, 👀 score 2–3. Entries are labeled with their source, e.g. `[health]`. Permit-type entries show a one-line snippet of the actual permit description (from the stored raw row, truncated to ~120 chars). Within a bucket, neighborhoods and entries are ordered by score then date, so the strongest signals come first. After printing, the digest also archives (marks seen) every unseen row older than the lookback window with a score at or above `--min-score` — the set the digest would have shown, shifted out of the window — and reports the count. Rows below `--min-score` stay unseen so a later digest with a lower threshold can still surface them; that digest marks them seen itself.

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
