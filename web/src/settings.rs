//! The one thing this device remembers that the engine does not.
//!
//! Which folder to keep files in is a question the engine answers only by being
//! told, once, at construction. Somebody has to remember the answer between
//! runs, and it is not the engine's business: it holds an identity, a catalog
//! and a block store, and where a person likes their files is none of those.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Absent until a folder has been chosen, which is what makes the default
    /// a default rather than a value written down on first run.
    pub sync_dir: Option<String>,
}

fn file(base_dir: &str) -> String {
    format!("{base_dir}/settings.json")
}

/// Unreadable, missing and malformed are the same answer: nothing was chosen.
/// A settings file is not worth refusing to start over.
pub fn load(base_dir: &str) -> Settings {
    std::fs::read_to_string(file(base_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(base_dir: &str, settings: &Settings) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(settings).unwrap_or_default();
    std::fs::write(file(base_dir), text)
}
