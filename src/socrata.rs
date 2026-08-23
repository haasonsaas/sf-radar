use anyhow::Result;
use serde_json::Value;
use thiserror::Error;

const PAGE_SIZE: usize = 5000;
/// Attempts per page for rate-limit (429) and server (5xx) errors, with
/// exponential backoff: 1s, 2s between tries.
const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Error)]
pub enum SocrataError {
    #[error("Socrata API error (HTTP {status}): {message}")]
    Api { status: u16, message: String },
    #[error("unexpected Socrata response (expected a JSON array): {0}")]
    BadShape(String),
}

pub struct SocrataClient {
    client: reqwest::blocking::Client,
    app_token: Option<String>,
}

impl SocrataClient {
    /// `app_token` is the resolved Socrata app token (see `config::app_token`);
    /// sent as the X-App-Token header when present.
    pub fn new(app_token: Option<String>) -> reqwest::Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("sf-radar/0.1")
            .timeout(std::time::Duration::from_secs(120))
            .build()?;
        Ok(Self { client, app_token })
    }

    pub fn has_app_token(&self) -> bool {
        self.app_token.is_some()
    }

    /// Fetch every row of `dataset` with `date_field >= since` (ISO date/timestamp),
    /// paging with LIMIT/OFFSET until a short page.
    pub fn fetch_since(&self, dataset: &str, date_field: &str, since: &str) -> Result<Vec<Value>> {
        self.fetch_pages(
            dataset,
            Some(format!("{date_field} >= '{since}'")),
            date_field,
        )
    }

    /// Fetch the entire dataset (for snapshot sources without a date field),
    /// paging on the system row id for stability.
    pub fn fetch_all(&self, dataset: &str) -> Result<Vec<Value>> {
        self.fetch_pages(dataset, None, ":id")
    }

    fn fetch_pages(
        &self,
        dataset: &str,
        where_clause: Option<String>,
        order: &str,
    ) -> Result<Vec<Value>> {
        let url = format!("https://data.sfgov.org/resource/{dataset}.json");
        let mut all = Vec::new();
        let mut offset = 0usize;

        loop {
            let mut query = vec![
                ("$order", order.to_string()),
                ("$limit", PAGE_SIZE.to_string()),
                ("$offset", offset.to_string()),
            ];
            if let Some(w) = &where_clause {
                query.push(("$where", w.clone()));
            }
            let text = self.get_page(&url, &query)?;
            let body: Value = serde_json::from_str(&text)?;
            let page = body
                .as_array()
                .ok_or_else(|| SocrataError::BadShape(text.chars().take(300).collect()))?;
            let n = page.len();
            all.extend(page.iter().cloned());
            offset += n;
            if n < PAGE_SIZE {
                break;
            }
        }

        Ok(all)
    }

    /// GET one page, retrying 429s and 5xxs (transient rate limits / blips)
    /// with exponential backoff. Other errors fail immediately.
    fn get_page(&self, url: &str, query: &[(&str, String)]) -> Result<String> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut req = self.client.get(url).query(query);
            if let Some(token) = &self.app_token {
                req = req.header("X-App-Token", token);
            }

            let outcome = match req.send() {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text()?;
                    if status.is_success() {
                        return Ok(text);
                    }
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    (
                        retryable,
                        SocrataError::Api {
                            status: status.as_u16(),
                            message: text.chars().take(300).collect(),
                        }
                        .into(),
                    )
                }
                // Connection-level failures (reset, timeout) are worth a retry.
                Err(e) => (true, e.into()),
            };

            let (retryable, err): (bool, anyhow::Error) = outcome;
            if !retryable || attempt >= MAX_ATTEMPTS {
                return Err(err);
            }
            let wait = std::time::Duration::from_secs(1 << (attempt - 1));
            eprintln!("  retrying in {}s: {err}", wait.as_secs());
            std::thread::sleep(wait);
        }
    }
}
