use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{address, name};

/// Score bonus when other sources have filings at the same address.
pub const CORROBORATION_BONUS: u32 = 2;
/// Score bonus when other sources have filings under the same name.
pub const NAME_BONUS: u32 = 1;

/// Minimum stored score for a row to count as a corroborator (see
/// `db::all_addresses`): the Watch-bucket threshold, i.e. the row is a
/// signal on its own.
pub const CORROBORATOR_MIN_SCORE: u32 = 2;

/// Sources whose sub-threshold rows are never rescued by corroboration. A
/// score-1 business registration means "a DBA that isn't food or retail
/// registered somewhere in this building" — in a mixed-use tower that is
/// any office tenant, not a venue signal.
const NON_RESCUABLE_SOURCES: &[&str] = &["business"];

/// Whether a row below `min_score` may be lifted into the digest by
/// corroboration bonuses.
pub fn rescue_eligible(source: &str) -> bool {
    !NON_RESCUABLE_SOURCES.contains(&source)
}

/// Stored-score floor for selecting digest candidates: low enough that a row
/// which display-time corroboration could lift to `min_score` is selected,
/// but never below 1 (a score-0 row has no signal of its own to corroborate).
/// The final `min_score` filter is applied after corroboration, in `grouped`.
pub fn selection_floor(min_score: u32) -> u32 {
    min_score
        .saturating_sub(CORROBORATION_BONUS + NAME_BONUS)
        .max(1)
}

/// Sources whose `raw` row carries a permit description worth showing.
const DESCRIPTION_SOURCES: &[&str] = &["permit", "electrical", "plumbing", "planning"];
const SNIPPET_MAX: usize = 120;

#[derive(Debug, Clone)]
pub struct DigestEntry {
    pub source: String, // source key, e.g. "business" | "permit" | "health"
    pub id: String,
    pub name: String,
    pub address: String,
    pub date: String,
    pub neighborhood: String,
    pub score: u32,
    pub reasons: Vec<String>,
    pub description: Option<String>, // permit description snippet, if any
    pub url: String,                 // per-row page where the source has one, else ""
}

/// Per-row page for sources that have one. Built from the id alone so it
/// works for every stored row, not just ones fetched after this was added.
pub fn url_for(source: &str, id: &str) -> String {
    match source {
        "abc" => format!(
            "https://www.abc.ca.gov/licensing/license-lookup/single-license/?RPTTYPE=12&LICENSE={id}"
        ),
        "permit" => format!(
            "https://dbiweb02.sfgov.org/dbipts/default.aspx?page=Permit&PermitNumber={id}"
        ),
        "planning" => format!("https://sfplanninggis.org/pim?search={id}"),
        _ => String::new(),
    }
}

/// Extract a one-line description snippet from a stored `raw` JSON row.
/// Only for permit-type sources; whitespace-collapsed, truncated to ~120
/// chars with an ellipsis. None when missing or empty.
pub fn description_snippet(source: &str, raw_json: &str) -> Option<String> {
    if !DESCRIPTION_SOURCES.contains(&source) || raw_json.is_empty() {
        return None;
    }
    let row: serde_json::Value = serde_json::from_str(raw_json).ok()?;
    let desc = row.get("description")?.as_str()?;
    let collapsed = desc.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(truncate(&collapsed, SNIPPET_MAX))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max - 3).collect();
        format!("{}...", cut.trim_end())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Strong, // score >= 4
    Watch,  // score 2-3
    Low,    // score 0-1: only rendered when --min-score is below 2
}

impl Bucket {
    pub fn title(self) -> &'static str {
        match self {
            Bucket::Strong => "🔥 Strong signals",
            Bucket::Watch => "👀 Worth watching",
            Bucket::Low => "· Low signals",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Strong => "strong",
            Bucket::Watch => "watch",
            Bucket::Low => "low",
        }
    }
}

/// Display order, strongest first.
pub const BUCKETS: [Bucket; 3] = [Bucket::Strong, Bucket::Watch, Bucket::Low];

pub fn bucket_for(score: u32) -> Option<Bucket> {
    match score {
        4.. => Some(Bucket::Strong),
        2..=3 => Some(Bucket::Watch),
        _ => Some(Bucket::Low),
    }
}

fn neighborhood_key(neighborhood: &str) -> String {
    if neighborhood.is_empty() {
        "Unknown neighborhood".to_string()
    } else {
        neighborhood.to_string()
    }
}

/* ---------- venue clustering ---------- */

/// Which source's name to show for a venue: DBAs (abc, health, business…)
/// before synthetic names ("Permit 2026…", planning's address-as-name).
const NAME_PRIORITY: &[&str] = &[
    "abc",
    "health",
    "business",
    "tables_chairs",
    "entertainment",
    "mobile_food",
    "vending",
    "planning",
    "fire",
    "permit",
    "electrical",
    "plumbing",
];
/// Earlier filings (from the full-history index) listed per venue.
const HISTORY_CAP: usize = 5;

fn source_rank(source: &str) -> usize {
    NAME_PRIORITY
        .iter()
        .position(|s| *s == source)
        .unwrap_or(NAME_PRIORITY.len())
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// One venue: the digest entries at one building (or, for entries with no
/// usable address, under one name), plus earlier filings from history.
/// This is the unit the digest displays and marks seen.
#[derive(Debug, Clone)]
pub struct Venue<'a> {
    pub name: String,
    pub address: String,
    pub neighborhood: String,
    pub score: u32,  // best member score, corroboration included
    pub date: String, // newest member date
    pub entries: Vec<&'a DigestEntry>, // newest first
    pub history: Vec<Corroborator>,    // earlier filings not among `entries`
}

/// Cluster entries into venues. Entries sharing a normalized address are
/// one venue. Name links only attach an entry that has no usable address
/// (e.g. a food-truck applicant) to one that does — two entries at
/// different addresses stay separate venues even under the same name, so
/// a chain's locations don't collapse into one.
pub fn cluster<'a>(entries: &'a [DigestEntry], addresses: &AddressIndex) -> Vec<Venue<'a>> {
    let addr_keys: Vec<String> = entries
        .iter()
        .map(|e| {
            let k = address::normalize(&e.address);
            if k.len() >= 5 { k } else { String::new() }
        })
        .collect();
    let name_keys: Vec<String> = entries
        .iter()
        .map(|e| {
            let k = name::match_key(&e.name);
            if name::is_matchable(&k) { k } else { String::new() }
        })
        .collect();

    let mut uf = UnionFind::new(entries.len());
    let mut first_at_addr: HashMap<&str, usize> = HashMap::new();
    for (i, k) in addr_keys.iter().enumerate() {
        if k.is_empty() {
            continue;
        }
        match first_at_addr.get(k.as_str()) {
            Some(&j) => uf.union(i, j),
            None => {
                first_at_addr.insert(k, i);
            }
        }
    }
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, k) in name_keys.iter().enumerate() {
        if !k.is_empty() {
            by_name.entry(k.as_str()).or_default().push(i);
        }
    }
    for idxs in by_name.values() {
        match idxs.iter().find(|&&i| !addr_keys[i].is_empty()) {
            Some(&anchor) => {
                for &i in idxs {
                    if addr_keys[i].is_empty() {
                        uf.union(i, anchor);
                    }
                }
            }
            None => {
                for &i in &idxs[1..] {
                    uf.union(i, idxs[0]);
                }
            }
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..entries.len() {
        groups.entry(uf.find(i)).or_default().push(i);
    }
    groups
        .into_values()
        .map(|idxs| build_venue(entries, &idxs, addresses))
        .collect()
}

fn build_venue<'a>(entries: &'a [DigestEntry], idxs: &[usize], addresses: &AddressIndex) -> Venue<'a> {
    let mut members: Vec<&DigestEntry> = idxs.iter().map(|&i| &entries[i]).collect();
    members.sort_by(|a, b| b.date.cmp(&a.date).then(b.score.cmp(&a.score)));

    let namer = members
        .iter()
        .copied()
        .filter(|e| !e.name.is_empty())
        .min_by_key(|e| (source_rank(&e.source), std::cmp::Reverse(e.score)))
        .unwrap_or(members[0]);
    let address = if !namer.address.is_empty() {
        namer.address.clone()
    } else {
        members
            .iter()
            .find(|e| !e.address.is_empty())
            .map(|e| e.address.clone())
            .unwrap_or_default()
    };
    let neighborhood = members
        .iter()
        .find(|e| !e.neighborhood.is_empty())
        .map(|e| e.neighborhood.clone())
        .unwrap_or_default();
    let score = members.iter().map(|e| e.score).max().unwrap_or(0);
    let date = members.iter().map(|e| e.date.clone()).max().unwrap_or_default();
    let history: Vec<Corroborator> = addresses
        .filings_at(&address)
        .into_iter()
        .filter(|h| {
            !members
                .iter()
                .any(|m| m.source == h.source && m.name == h.name && m.date == h.date)
        })
        .take(HISTORY_CAP)
        .cloned()
        .collect();

    let name = if !namer.name.is_empty() {
        namer.name.clone()
    } else if !address.is_empty() {
        address.clone()
    } else {
        "(unnamed)".to_string()
    };

    Venue {
        name,
        address,
        neighborhood,
        score,
        date,
        entries: members,
        history,
    }
}

/* ---------- grouping + rendering ---------- */

/// Buckets of neighborhood-name → venue groups, in display order.
type Grouped<'a> = Vec<(Bucket, Vec<(String, Vec<Venue<'a>>)>)>;

/// Cluster, drop venues below `min_score`, and group for display: buckets
/// strong-first, neighborhood groups ordered by their best venue (score
/// desc, date desc), venues within a neighborhood the same way.
fn grouped<'a>(entries: &'a [DigestEntry], min_score: u32, addresses: &AddressIndex) -> Grouped<'a> {
    let by_score = |a: &Venue, b: &Venue| b.score.cmp(&a.score).then(b.date.cmp(&a.date));

    let venues = cluster(entries, addresses);
    let mut out = Vec::new();
    for bucket in BUCKETS {
        let mut by_hood: BTreeMap<String, Vec<Venue<'a>>> = BTreeMap::new();
        for venue in venues
            .iter()
            .filter(|v| v.score >= min_score && bucket_for(v.score) == Some(bucket))
        {
            by_hood
                .entry(neighborhood_key(&venue.neighborhood))
                .or_default()
                .push(venue.clone());
        }
        if by_hood.is_empty() {
            continue;
        }
        let mut groups: Vec<(String, Vec<Venue<'a>>)> = by_hood.into_iter().collect();
        for group in groups.iter_mut().map(|(_, g)| g) {
            group.sort_by(by_score);
        }
        groups.sort_by(|a, b| by_score(&a.1[0], &b.1[0]).then_with(|| a.0.cmp(&b.0)));
        out.push((bucket, groups));
    }
    out
}

/// Every entry the digest shows, flattened in display order: venue by venue,
/// each venue's filings newest first. Every filing of a displayed venue is
/// included, even ones below `min_score` on their own — that's what gets
/// marked seen.
pub fn ordered_with<'a>(entries: &'a [DigestEntry], min_score: u32, addresses: &AddressIndex) -> Vec<&'a DigestEntry> {
    grouped(entries, min_score, addresses)
        .into_iter()
        .flat_map(|(_, groups)| groups.into_iter().flat_map(|(_, venues)| venues))
        .flat_map(|v| v.entries)
        .collect()
}

/// `ordered_with` without history (venues cluster within `entries` only).
pub fn ordered(entries: &[DigestEntry], min_score: u32) -> Vec<&DigestEntry> {
    ordered_with(entries, min_score, &AddressIndex::build(Vec::new()))
}

fn format_filing(entry: &DigestEntry, venue: &Venue, markdown: bool) -> String {
    // Corroboration reasons restate filings the venue block already shows
    // (as members or on the `earlier:` line); they stay in the JSON output.
    let reasons = entry
        .reasons
        .iter()
        .filter(|r| !r.starts_with("corroborated by ") && !r.starts_with("name also in "))
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let desc = entry.description.as_deref().unwrap_or("");
    let mut out = if markdown {
        let name = if entry.url.is_empty() {
            entry.name.clone()
        } else {
            format!("[{}]({})", entry.name, entry.url)
        };
        format!("  - {} [{}] {}: {}", entry.date, entry.source, name, reasons)
    } else {
        // Skip the name when it's just the venue name again.
        let name = if entry.name == venue.name { String::new() } else { format!("{} — ", entry.name) };
        format!("    {} [{}] {}{}", entry.date, entry.source, name, reasons)
    };
    if !desc.is_empty() {
        if markdown {
            out.push_str(&format!("\n    - *{desc}*"));
        } else {
            out.push_str(&format!("\n      {desc}"));
        }
    }
    if !markdown && !entry.url.is_empty() {
        out.push_str(&format!("\n      {}", entry.url));
    }
    out
}

fn format_venue(venue: &Venue, markdown: bool) -> String {
    let address = if venue.address.is_empty() { "no address" } else { &venue.address };
    let n = venue.entries.len();
    let filings = format!("{n} filing{}", if n == 1 { "" } else { "s" });
    let mut out = if markdown {
        format!("- **{}** — {} — score {} — {}", venue.name, address, venue.score, filings)
    } else {
        format!("  {} — {} · score {} · {}", venue.name, address, venue.score, filings)
    };
    for entry in &venue.entries {
        out.push('\n');
        out.push_str(&format_filing(entry, venue, markdown));
    }
    if !venue.history.is_empty() {
        let earlier = list(&venue.history.iter().collect::<Vec<_>>());
        if markdown {
            out.push_str(&format!("\n  - earlier: {earlier}"));
        } else {
            out.push_str(&format!("\n    earlier: {earlier}"));
        }
    }
    out
}

/// Render the digest: buckets ordered strong-first, one block per venue
/// with its filings as a timeline (see `grouped`).
pub fn render_with(entries: &[DigestEntry], min_score: u32, markdown: bool, days: u32, addresses: &AddressIndex) -> String {
    let mut out = String::new();
    let header = format!("SF Opening Radar — last {days} days");
    if markdown {
        out.push_str(&format!("# {header}\n"));
    } else {
        out.push_str(&format!("{header}\n"));
    }

    let mut signals = 0usize;
    let mut venues = 0usize;
    for (bucket, groups) in grouped(entries, min_score, addresses) {
        out.push('\n');
        if markdown {
            out.push_str(&format!("## {}\n", bucket.title()));
        } else {
            out.push_str(&format!("{}\n", bucket.title()));
        }

        for (hood, group) in groups {
            out.push('\n');
            if markdown {
                out.push_str(&format!("### {hood}\n\n"));
            } else {
                out.push_str(&format!("{hood}\n"));
            }
            for venue in group {
                venues += 1;
                signals += venue.entries.len();
                out.push_str(&format_venue(&venue, markdown));
                out.push('\n');
            }
        }
    }

    if signals == 0 {
        out.push_str("\nNo new signals.\n");
    } else {
        out.push_str(&format!("\n{signals} new signal(s) at {venues} venue(s).\n"));
    }
    out
}

/// `render_with` without history.
pub fn render(entries: &[DigestEntry], min_score: u32, markdown: bool, days: u32) -> String {
    render_with(entries, min_score, markdown, days, &AddressIndex::build(Vec::new()))
}

/// Machine-readable form of one digest entry. Field set and order are part
/// of the tool's JSON contract — a sibling project implements the same shape.
#[derive(Debug, Clone, serde::Serialize)]
pub struct JsonEntry {
    pub source: String,
    pub id: String,
    pub name: String,
    pub address: String,
    pub neighborhood: String,
    pub date: String,
    pub score: u32,
    pub bucket: String,
    pub reasons: Vec<String>,
    pub url: String,
    pub description: String,
}

/// An earlier filing at a venue's address, from history.
#[derive(Debug, serde::Serialize)]
pub struct JsonFiling {
    pub source: String,
    pub name: String,
    pub date: String,
}

/// One venue in `--json`: the clustered view of `entries`.
#[derive(Debug, serde::Serialize)]
pub struct JsonVenue {
    pub name: String,
    pub address: String,
    pub neighborhood: String,
    pub date: String,
    pub score: u32,
    pub bucket: String,
    pub filings: Vec<JsonEntry>,
    pub history: Vec<JsonFiling>,
}

/// Top-level `--json` output (see `render_json`).
#[derive(Debug, serde::Serialize)]
pub struct JsonDigest {
    pub tool: String,
    pub generated_at: String,
    pub window_days: u32,
    pub min_score: u32,
    pub archived: usize,
    pub entries: Vec<JsonEntry>,
    pub venues: Vec<JsonVenue>,
}

fn json_entry(e: &DigestEntry) -> JsonEntry {
    JsonEntry {
        source: e.source.clone(),
        id: e.id.clone(),
        name: e.name.clone(),
        address: e.address.clone(),
        neighborhood: e.neighborhood.clone(),
        date: e.date.clone(),
        score: e.score,
        bucket: bucket_for(e.score).map_or("low", Bucket::as_str).to_string(),
        reasons: e.reasons.clone(),
        url: e.url.clone(),
        description: e.description.clone().unwrap_or_default(),
    }
}

/// Machine-readable digest. `entries` is every displayed filing in display
/// order (venue by venue), the same flat shape as before venues existed;
/// `venues` is the clustered view. Buckets use the post-corroboration
/// score. `url` is the per-row page where the source has one (see
/// `url_for`), else ""; `description` is the permit snippet or "".
pub fn render_json_with(entries: &[DigestEntry], min_score: u32, days: u32, archived: usize, addresses: &AddressIndex) -> String {
    let venues: Vec<JsonVenue> = grouped(entries, min_score, addresses)
        .into_iter()
        .flat_map(|(_, groups)| groups.into_iter().flat_map(|(_, venues)| venues))
        .map(|v| JsonVenue {
            name: v.name.clone(),
            address: v.address.clone(),
            neighborhood: v.neighborhood.clone(),
            date: v.date.clone(),
            score: v.score,
            bucket: bucket_for(v.score).map_or("low", Bucket::as_str).to_string(),
            filings: v.entries.iter().map(|e| json_entry(e)).collect(),
            history: v
                .history
                .iter()
                .map(|h| JsonFiling {
                    source: h.source.clone(),
                    name: h.name.clone(),
                    date: h.date.clone(),
                })
                .collect(),
        })
        .collect();
    let flat: Vec<JsonEntry> = venues.iter().flat_map(|v| v.filings.iter().cloned()).collect();
    let digest = JsonDigest {
        tool: "sf-radar".to_string(),
        generated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        window_days: days,
        min_score,
        archived,
        entries: flat,
        venues,
    };
    serde_json::to_string_pretty(&digest).expect("JsonDigest serialization cannot fail")
}

/// `render_json_with` without history.
pub fn render_json(entries: &[DigestEntry], min_score: u32, days: u32, archived: usize) -> String {
    render_json_with(entries, min_score, days, archived, &AddressIndex::build(Vec::new()))
}

#[derive(Debug, Clone)]
pub struct Corroborator {
    pub source: String,
    pub name: String,
    pub date: String,
}

/// Newest-first, deduped by (source, name, date), capped, self excluded.
fn corroborators<'a>(
    rows: Option<&'a Vec<Corroborator>>,
    source: &str,
    cap: usize,
) -> Vec<&'a Corroborator> {
    let mut hits: Vec<&Corroborator> = rows
        .map(|rows| rows.iter().filter(|r| r.source != source).collect())
        .unwrap_or_default();
    hits.sort_by(|a, b| b.date.cmp(&a.date));
    let mut seen = HashSet::new();
    hits.retain(|h| seen.insert((&h.source, &h.name, &h.date)));
    hits.truncate(cap);
    hits
}

fn list(hits: &[&Corroborator]) -> String {
    hits.iter()
        .map(|h| format!("{}: {} ({})", h.source, h.name, h.date))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Index of every stored signal by normalized address, used to find
/// cross-source corroboration at digest time (display-only, never persisted).
pub struct AddressIndex {
    by_address: HashMap<String, Vec<Corroborator>>,
}

impl AddressIndex {
    /// `rows` is (source, name, address, date) for every signal with an address.
    pub fn build(rows: Vec<(String, String, String, String)>) -> Self {
        let mut by_address: HashMap<String, Vec<Corroborator>> = HashMap::new();
        for (source, name, addr, date) in rows {
            let key = address::normalize(&addr);
            if key.len() < 5 {
                continue; // too vague to corroborate anything
            }
            by_address
                .entry(key)
                .or_default()
                .push(Corroborator { source, name, date });
        }
        Self { by_address }
    }

    /// Up to 3 corroborators from OTHER sources at the same address.
    pub fn corroborators(&self, source: &str, addr: &str) -> Vec<&Corroborator> {
        corroborators(self.by_address.get(&address::normalize(addr)), source, 3)
    }

    /// Every filing at the same address, any source, newest first (deduped,
    /// capped) — the venue timeline's history.
    pub fn filings_at(&self, addr: &str) -> Vec<&Corroborator> {
        corroborators(self.by_address.get(&address::normalize(addr)), "", 10)
    }
}

/// Index of every stored signal by normalized name — catches cross-source
/// pairs whose addresses don't normalize equal (e.g. a food truck's
/// commissary address vs the storefront).
pub struct NameIndex {
    by_name: HashMap<String, Vec<Corroborator>>,
}

impl NameIndex {
    /// `rows` is (source, name, address, date) for every signal with an address.
    pub fn build(rows: &[(String, String, String, String)]) -> Self {
        let mut by_name: HashMap<String, Vec<Corroborator>> = HashMap::new();
        for (source, row_name, _, date) in rows {
            let key = name::match_key(row_name);
            if !name::is_matchable(&key) {
                continue;
            }
            by_name.entry(key).or_default().push(Corroborator {
                source: source.clone(),
                name: row_name.clone(),
                date: date.clone(),
            });
        }
        Self { by_name }
    }

    /// Up to 2 corroborators from OTHER sources with the same name.
    pub fn corroborators(&self, source: &str, row_name: &str) -> Vec<&Corroborator> {
        let key = name::match_key(row_name);
        if !name::is_matchable(&key) {
            return Vec::new();
        }
        corroborators(self.by_name.get(&key), source, 2)
    }
}

/// Normalized address -> neighborhood, from every stored signal that has
/// both. Sources without a neighborhood field (abc, fire, planning, vending)
/// inherit one from any other filing at the same building.
pub struct NeighborhoodIndex {
    by_address: HashMap<String, String>,
}

impl NeighborhoodIndex {
    /// `rows` is (address, neighborhood) with both non-empty.
    pub fn build(rows: Vec<(String, String)>) -> Self {
        let mut by_address = HashMap::new();
        for (addr, hood) in rows {
            let key = address::normalize(&addr);
            if key.len() >= 5 && !hood.trim().is_empty() {
                by_address.entry(key).or_insert(hood);
            }
        }
        Self { by_address }
    }

    pub fn lookup(&self, addr: &str) -> Option<&str> {
        self.by_address
            .get(&address::normalize(addr))
            .map(String::as_str)
    }

    /// Fill empty neighborhoods in place from same-address signals.
    pub fn fill(&self, entries: &mut [DigestEntry]) {
        for e in entries.iter_mut() {
            if e.neighborhood.is_empty()
                && let Some(hood) = self.lookup(&e.address)
            {
                e.neighborhood = hood.to_string();
            }
        }
    }
}

/// Apply corroboration bonuses in memory (display-only — stored scores are
/// not touched): +2 for other sources at the same address, then +1 for
/// other sources under the same name.
pub fn apply_corroboration(
    entries: &mut [DigestEntry],
    addresses: &AddressIndex,
    names: &NameIndex,
) {
    for entry in entries.iter_mut() {
        let hits = addresses.corroborators(&entry.source, &entry.address);
        if !hits.is_empty() {
            entry.score += CORROBORATION_BONUS;
            entry.reasons.push(format!("corroborated by {}", list(&hits)));
        }
        let hits = names.corroborators(&entry.source, &entry.name);
        if !hits.is_empty() {
            entry.score += NAME_BONUS;
            entry.reasons.push(format!("name also in {}", list(&hits)));
        }
    }
}
