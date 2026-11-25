//! Minimal UI language switch. Only the interface chrome is translated;
//! document content (room / furniture / material names) stays as the user typed it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    #[serde(rename = "zh")]
    Zh,
    #[serde(rename = "en")]
    En,
}

impl Lang {
    pub fn toggled(self) -> Self {
        match self {
            Self::Zh => Self::En,
            Self::En => Self::Zh,
        }
    }

    /// Short label shown on the switch button.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Zh => "中文",
            Self::En => "EN",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct T(pub Lang);

impl T {
    pub fn s(self, zh: &'static str, en: &'static str) -> &'static str {
        match self.0 {
            Lang::Zh => zh,
            Lang::En => en,
        }
    }

    pub fn f(self, zh: &str, en: &str) -> String {
        match self.0 {
            Lang::Zh => zh.to_owned(),
            Lang::En => en.to_owned(),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct PrefsFile {
    #[serde(default)]
    lang: Lang,
}

pub fn prefs_path() -> PathBuf {
    let mut path = crate::scene::user_plan_path();
    path.set_file_name("prefs.json");
    path
}

pub fn load_lang() -> Lang {
    std::fs::read_to_string(prefs_path())
        .ok()
        .and_then(|text| serde_json::from_str::<PrefsFile>(&text).ok())
        .map(|file| file.lang)
        .unwrap_or_default()
}

pub fn save_lang(lang: Lang) {
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = serde_json::to_string_pretty(&PrefsFile { lang }).unwrap_or_default();
    let _ = std::fs::write(path, body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_between_two_languages() {
        assert_eq!(Lang::Zh.toggled(), Lang::En);
        assert_eq!(Lang::En.toggled(), Lang::Zh);
        assert_eq!(Lang::Zh.tag(), "中文");
        assert_eq!(Lang::En.tag(), "EN");
    }

    #[test]
    fn picks_the_right_string() {
        assert_eq!(T(Lang::Zh).s("保存", "Save"), "保存");
        assert_eq!(T(Lang::En).s("保存", "Save"), "Save");
        assert_eq!(T(Lang::En).f("仓库", "warehouse"), "warehouse");
    }
}
