use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Optional settings read from `config.toml` next to the database.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub socrata_app_token: Option<String>,
}

/// The config file lives in the same directory as the database, so a custom
/// `--db` path brings its own config along.
pub fn path_for(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("config.toml")
}

pub fn parse(content: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(content)
}

/// Load the config next to `db_path`. A missing file is fine; an unreadable
/// or invalid file warns on stderr and is ignored.
pub fn load(db_path: &Path) -> Config {
    let path = path_for(db_path);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            eprintln!("Warning: cannot read config {}: {e}", path.display());
            return Config::default();
        }
    };
    match parse(&content) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Warning: ignoring invalid config {}: {e}", path.display());
            Config::default()
        }
    }
}

fn non_empty(token: Option<String>) -> Option<String> {
    token.filter(|t| !t.trim().is_empty())
}

/// Resolve the Socrata app token: the `SOCRATA_APP_TOKEN` env var wins over
/// the config file.
pub fn app_token(env: Option<String>, config: &Config) -> Option<String> {
    non_empty(env).or_else(|| non_empty(config.socrata_app_token.clone()))
}
