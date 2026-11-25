//! Favourite furniture asset ids, stored beside the language preference.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
struct FavoritesFile {
    #[serde(default)]
    ids: Vec<String>,
}

fn store_path() -> PathBuf {
    let mut path = crate::scene::user_plan_path();
    path.set_file_name("favorites.json");
    path
}

pub fn load() -> Vec<String> {
    fs::read_to_string(store_path())
        .ok()
        .and_then(|text| serde_json::from_str::<FavoritesFile>(&text).ok())
        .map(|file| file.ids)
        .unwrap_or_default()
}

pub fn toggle(asset_id: &str) -> Vec<String> {
    let mut ids = load();
    toggle_in(&mut ids, asset_id);
    save(&ids);
    ids
}

pub fn save(ids: &[String]) {
    let path = store_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let body =
        serde_json::to_string_pretty(&FavoritesFile { ids: ids.to_vec() }).unwrap_or_default();
    let _ = fs::write(path, body);
}

pub fn contains(ids: &[String], asset_id: &str) -> bool {
    ids.iter().any(|id| id == asset_id)
}

/// Toggle in memory so the ordering rule is testable without touching disk.
pub fn toggle_in(ids: &mut Vec<String>, asset_id: &str) {
    if let Some(pos) = ids.iter().position(|id| id == asset_id) {
        ids.remove(pos);
    } else {
        ids.insert(0, asset_id.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_puts_new_favourites_first_and_removes_existing() {
        let mut ids = vec!["sofa".into(), "bed".into()];
        toggle_in(&mut ids, "desk");
        assert_eq!(ids, vec!["desk", "sofa", "bed"]);
        toggle_in(&mut ids, "sofa");
        assert_eq!(ids, vec!["desk", "bed"]);
    }
}
