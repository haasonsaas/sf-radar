use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{address, name};

/// Score bonus when other sources have filings at the same address.
pub const CORROBORATION_BONUS: u32 = 2;
/// Score bonus when other sources have filings under the same name.
pub const NAME_BONUS: u32 = 1;

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
}

impl Bucket {
    pub fn title(self) -> &'static str {
        match self {
            Bucket::Strong => "🔥 Strong signals",
            Bucket::Watch => "👀 Worth watching",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Strong => "strong",
            Bucket::Watch => "watch",
        }
    }
}

pub fn bucket_for(score: u32) -> Option<Bucket> {
    match score {
        4.. => Some(Bucket::Strong),
        2..=3 => Some(Bucket::Watch),
        _ => None,
    }
}

fn neighborhood_key(entry: &DigestEntry) -> String {
    if entry.neighborhood.is_empty() {
        "Unknown neighborhood".to_string()
    } else {
        entry.neighborhood.clone()
    }
}

fn format_entry(entry: &DigestEntry, markdown: bool) -> String {
    let reasons = entry.reasons.join("; ");
    let address = if entry.address.is_empty() {
        "no address"
    } else {
        &entry.address
    };
    let desc = entry.description.as_deref().unwrap_or("");
    let mut out = if markdown {
        format!(
            "- **[{}] {}** — {} — {} — score {}\n  - {}",
            entry.source, entry.name, address, entry.date, entry.score, reasons
        )
    } else {
        format!(
            "  [{}] {} — {}\n    {} · score {} · {}",
            entry.source, entry.name, address, entry.date, entry.score, reasons
        )
    };
    if !desc.is_empty() {
        if markdown {
            out.push_str(&format!("\n  - *{desc}*"));
        } else {
            out.push_str(&format!("\n    {desc}"));
        }
    }
    out
}

/// Buckets of neighborhood-name → entries groups, in display order.
type Grouped<'a> = Vec<(Bucket, Vec<(String, Vec<&'a DigestEntry>)>)>;

/// Group entries for display: buckets strong-first, neighborhood groups
/// ordered by their best entry (score desc, date desc), and entries within a
/// neighborhood the same way, so the strongest signals float to the top.
/// Entries below `min_score` are filtered out.
fn grouped(entries: &[DigestEntry], min_score: u32) -> Grouped<'_> {
    let by_score = |a: &&DigestEntry, b: &&DigestEntry| {
        b.score.cmp(&a.score).then(b.date.cmp(&a.date))
    };

    let mut out = Vec::new();
    for bucket in [Bucket::Strong, Bucket::Watch] {
        let mut by_hood: BTreeMap<String, Vec<&DigestEntry>> = BTreeMap::new();
        for entry in entries
            .iter()
            .filter(|e| e.score >= min_score && bucket_for(e.score) == Some(bucket))
        {
            by_hood.entry(neighborhood_key(entry)).or_default().push(entry);
        }
        if by_hood.is_empty() {
            continue;
        }

        let mut groups: Vec<(String, Vec<&DigestEntry>)> = by_hood.into_iter().collect();
        for group in groups.iter_mut().map(|(_, g)| g) {
            group.sort_by(by_score);
        }
        groups.sort_by(|a, b| {
            by_score(&a.1[0], &b.1[0]).then_with(|| a.0.cmp(&b.0))
        });
        out.push((bucket, groups));
    }
    out
}

/// Entries flattened in display order (best first) — the same order `render`
/// prints them and `render_json` emits them.
pub fn ordered(entries: &[DigestEntry], min_score: u32) -> Vec<&DigestEntry> {
    grouped(entries, min_score)
        .into_iter()
        .flat_map(|(_, groups)| groups.into_iter().flat_map(|(_, group)| group))
        .collect()
}

/// Render the digest: buckets ordered strong-first (see `grouped`).
pub fn render(entries: &[DigestEntry], min_score: u32, markdown: bool, days: u32) -> String {
    let mut out = String::new();
    let header = format!("SF Opening Radar — last {days} days");
    if markdown {
        out.push_str(&format!("# {header}\n"));
    } else {
        out.push_str(&format!("{header}\n"));
    }

    let mut total = 0usize;
    for (bucket, groups) in grouped(entries, min_score) {
        out.push('\n');
        if markdown {
            out.push_str(&format!("## {}\n", bucket.title()));
        } else {
            out.push_str(&format!("{}\n", bucket.title()));
        }

        for (hood, group) in groups {
            total += group.len();
            out.push('\n');
            if markdown {
                out.push_str(&format!("### {hood}\n\n"));
            } else {
                out.push_str(&format!("{hood}\n"));
            }
            for entry in group {
                out.push_str(&format_entry(entry, markdown));
                out.push('\n');
            }
        }
    }

    if total == 0 {
        out.push_str("\nNo new signals.\n");
    } else {
        out.push_str(&format!("\n{total} new signal(s).\n"));
    }
    out
}

/// Machine-readable form of one digest entry. Field set and order are part
/// of the tool's JSON contract — a sibling project implements the same shape.
#[derive(Debug, serde::Serialize)]
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

/// Top-level `--json` output (see `render_json`).
#[derive(Debug, serde::Serialize)]
pub struct JsonDigest {
    pub tool: String,
    pub generated_at: String,
    pub window_days: u32,
    pub min_score: u32,
    pub archived: usize,
    pub entries: Vec<JsonEntry>,
}

/// Machine-readable digest, same entry order as the prose digest (best
/// first). Buckets use the post-corroboration score. `url` is always empty
/// (this tool has no per-row URL); `description` is the permit snippet or "".
pub fn render_json(entries: &[DigestEntry], min_score: u32, days: u32, archived: usize) -> String {
    let entries = ordered(entries, min_score)
        .into_iter()
        .map(|e| JsonEntry {
            source: e.source.clone(),
            id: e.id.clone(),
            name: e.name.clone(),
            address: e.address.clone(),
            neighborhood: e.neighborhood.clone(),
            date: e.date.clone(),
            score: e.score,
            bucket: bucket_for(e.score).map_or("watch", Bucket::as_str).to_string(),
            reasons: e.reasons.clone(),
            url: String::new(),
            description: e.description.clone().unwrap_or_default(),
        })
        .collect();
    let digest = JsonDigest {
        tool: "sf-radar".to_string(),
        generated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        window_days: days,
        min_score,
        archived,
        entries,
    };
    serde_json::to_string_pretty(&digest).expect("JsonDigest serialization cannot fail")
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
            let key = name::normalize_name(row_name);
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
        let key = name::normalize_name(row_name);
        if !name::is_matchable(&key) {
            return Vec::new();
        }
        corroborators(self.by_name.get(&key), source, 2)
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
