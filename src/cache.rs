//! Rig classifications cached on this machine, keyed by skeleton, so no GLB ever carries them.

use crate::systems::{self, System};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Overrides the cache directory, mainly so tests stay isolated.
pub const DIRECTORY_VARIABLE: &str = "RETARJET_CACHE";

pub type Bones = [(String, Option<String>)];

pub fn directory() -> Option<PathBuf> {
    match std::env::var_os(DIRECTORY_VARIABLE) {
        Some(directory) => Some(PathBuf::from(directory)),
        None => dirs::cache_dir().map(|directory| directory.join("retarjet")),
    }
}

/// Stable across runs and bone order, and changes whenever a bone is renamed or reparented.
pub fn key(bones: &Bones) -> String {
    let mut lines: Vec<String> = bones
        .iter()
        .map(|(name, parent)| format!("{name}\t{}", parent.as_deref().unwrap_or_default()))
        .collect();
    lines.sort();
    format!("{:016x}", fnv1a(lines.join("\n").as_bytes()))
}

pub fn path_in(directory: &Path, bones: &Bones) -> PathBuf {
    directory
        .join("classifications")
        .join(format!("{}.json", key(bones)))
}

/// The cached classification, minus any bone the skeleton no longer has.
pub fn load_from(directory: &Path, bones: &Bones) -> Option<Vec<System>> {
    let text = fs::read_to_string(path_in(directory, bones)).ok()?;
    let data: Value = serde_json::from_str(&text).ok()?;
    let names: Vec<String> = bones.iter().map(|(name, _)| name.clone()).collect();
    let systems = systems::parse(&data, &names);
    (!systems.is_empty()).then_some(systems)
}

pub fn store_in(directory: &Path, bones: &Bones, systems: &[System]) -> Result<PathBuf> {
    let path = path_in(directory, bones);
    let parent = path.parent().context("cache path has no directory")?;
    fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    let names: Vec<&str> = bones.iter().map(|(name, _)| name.as_str()).collect();
    let text = serde_json::to_string_pretty(&json!({"bones": names, "systems": systems}))?;
    fs::write(&path, text + "\n").with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

pub fn load(bones: &Bones) -> Option<Vec<System>> {
    load_from(&directory()?, bones)
}

pub fn store(bones: &Bones, systems: &[System]) -> Result<PathBuf> {
    let directory = directory().context("no cache directory on this system")?;
    store_in(&directory, bones, systems)
}

/// 64 bit FNV-1a, whose output never changes between Rust versions unlike the standard hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bones(list: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        list.iter()
            .map(|(name, parent)| (name.to_string(), parent.map(str::to_owned)))
            .collect()
    }

    #[test]
    fn key_ignores_bone_order_but_not_hierarchy() {
        let skeleton = bones(&[("Hips", None), ("Head", Some("Hips"))]);
        let reordered = bones(&[("Head", Some("Hips")), ("Hips", None)]);
        let reparented = bones(&[("Hips", None), ("Head", None)]);
        assert_eq!(key(&skeleton), key(&reordered));
        assert_ne!(key(&skeleton), key(&reparented));
    }

    #[test]
    fn store_then_load_round_trips() {
        let directory = std::env::temp_dir().join(format!("retarjet-cache-{}", std::process::id()));
        let skeleton = bones(&[("Neck", None), ("Head", Some("Neck"))]);
        let systems = vec![System::Head {
            parent: None,
            neck: Some("Neck".into()),
            head: "Head".into(),
        }];
        store_in(&directory, &skeleton, &systems).unwrap();
        assert_eq!(load_from(&directory, &skeleton), Some(systems));
        assert_eq!(load_from(&directory, &bones(&[("Head", None)])), None);
        fs::remove_dir_all(directory).unwrap();
    }
}
