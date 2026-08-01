use std::collections::BTreeMap;

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
    if markdown {
        format!(
            "- **[{}] {}** — {} — {} — score {}\n  - {}",
            entry.source, entry.name, address, entry.date, entry.score, reasons
        )
    } else {
        format!(
            "  [{}] {} — {}\n    {} · score {} · {}",
            entry.source, entry.name, address, entry.date, entry.score, reasons
        )
    }
}

/// Render the digest: buckets ordered strong-first, neighborhoods alphabetical,
/// entries newest-first. Entries below `min_score` are filtered out.
pub fn render(entries: &[DigestEntry], min_score: u32, markdown: bool, days: u32) -> String {
    let mut out = String::new();
    let header = format!("SF Opening Radar — last {days} days");
    if markdown {
        out.push_str(&format!("# {header}\n"));
    } else {
        out.push_str(&format!("{header}\n"));
    }

    let mut total = 0usize;
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

        out.push('\n');
        if markdown {
            out.push_str(&format!("## {}\n", bucket.title()));
        } else {
            out.push_str(&format!("{}\n", bucket.title()));
        }

        for (hood, mut group) in by_hood {
            group.sort_by(|a, b| b.date.cmp(&a.date).then(b.score.cmp(&a.score)));
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
