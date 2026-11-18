use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::OnceLock};

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Engine {
    DuckDuckGo,
    Brave,
    Google,
    Startpage,
    Kagi,
}

impl Engine {
    pub const ALL: [Engine; 5] = [Engine::DuckDuckGo, Engine::Brave, Engine::Google, Engine::Startpage, Engine::Kagi];

    pub fn name(self) -> &'static str {
        match self {
            Engine::DuckDuckGo => "DuckDuckGo",
            Engine::Brave => "Brave Search",
            Engine::Google => "Google",
            Engine::Startpage => "Startpage",
            Engine::Kagi => "Kagi",
        }
    }

    pub fn id(self) -> usize {
        Self::ALL.iter().position(|e| *e == self).unwrap_or(0)
    }

    pub fn suggest_url(self, q: &str) -> String {
        let base = match self {
            Engine::Google => "https://suggestqueries.google.com/complete/search?client=firefox&q=",
            Engine::Brave => "https://search.brave.com/api/suggest?q=",
            _ => "https://duckduckgo.com/ac/?type=list&q=",
        };
        format!("{base}{}", Self::encode(q))
    }

    fn encode(q: &str) -> String {
        q.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    pub fn url(self, q: &str) -> String {
        let base = match self {
            Engine::DuckDuckGo => "https://duckduckgo.com/?q=",
            Engine::Brave => "https://search.brave.com/search?q=",
            Engine::Google => "https://www.google.com/search?q=",
            Engine::Startpage => "https://www.startpage.com/do/search?q=",
            Engine::Kagi => "https://kagi.com/search?q=",
        };
        format!("{base}{}", Self::encode(q))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub engine: Engine,
    pub hardware_accel: bool,
    pub bookmarks_bar: bool,
    pub autofill: bool,
    pub links_in_new_tab: bool,
    pub clean_urls: bool,
    pub focus_mode: bool,
    pub auto_update_ext: bool,
    pub dl_popup: bool,
    pub resume_tabs: bool,
    pub memory_saver: bool,
    #[serde(default = "yes")]
    pub onboarded: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            engine: Engine::DuckDuckGo,
            hardware_accel: true,
            bookmarks_bar: true,
            autofill: true,
            links_in_new_tab: false,
            clean_urls: false,
            focus_mode: false,
            auto_update_ext: true,
            dl_popup: true,
            resume_tabs: false,
            memory_saver: true,
            onboarded: false,
        }
    }
}

pub fn dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::var_os("RB_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| dirs::config_dir().unwrap_or_default().join("Ferrum"));
        let _ = std::fs::create_dir_all(&d);
        d
    })
    .clone()
}

impl Settings {
    pub fn load() -> Self {
        let d = dir();
        Self::parse(std::fs::read(d.join("settings.json")).ok().as_deref(), d.join("webview").exists())
    }

    fn parse(raw: Option<&[u8]>, prior_run: bool) -> Self {
        match raw {
            Some(b) => serde_json::from_slice(b).unwrap_or_else(|_| Self { onboarded: true, ..Self::default() }),
            None => Self { onboarded: prior_run, ..Self::default() },
        }
    }

    pub fn save(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(dir().join("settings.json"), b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn fresh_install_shows_onboarding() {
        assert!(!Settings::parse(None, false).onboarded);
    }

    #[test]
    fn prior_evidence_skips_onboarding() {
        assert!(Settings::parse(None, true).onboarded);
        assert!(Settings::parse(Some(br#"{"engine":"DuckDuckGo"}"#), false).onboarded);
        assert!(Settings::parse(Some(b"{broken"), false).onboarded);
        assert!(Settings::parse(Some(b"{}"), false).onboarded);
    }

    #[test]
    fn explicit_flag_wins() {
        assert!(!Settings::parse(Some(br#"{"onboarded":false}"#), true).onboarded);
        assert!(Settings::parse(Some(br#"{"onboarded":true}"#), false).onboarded);
    }
}
