//! Rig systems in yello's schema, and the role based bone pairing built on them.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    L,
    R,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum System {
    Spine {
        pelvis: Option<String>,
        vertebrae: Vec<String>,
    },
    Arm {
        side: Side,
        parent: Option<String>,
        shoulder: Option<String>,
        upper_arm: String,
        forearm: String,
        hand: String,
    },
    Leg {
        side: Side,
        parent: Option<String>,
        upper_leg: String,
        lower_leg: String,
        foot: String,
        toe: Option<String>,
    },
    Head {
        parent: Option<String>,
        neck: Option<String>,
        head: String,
    },
    Finger {
        name: String,
        side: Side,
        parent: Option<String>,
        chain: Vec<String>,
    },
}

/// Validate classifier output against real bone names, dropping anything invented.
pub fn parse(data: &Value, bone_names: &[String]) -> Vec<System> {
    let lookup: HashMap<String, &String> = bone_names
        .iter()
        .map(|name| (name.trim().to_lowercase(), name))
        .collect();
    let resolve = |value: Option<&Value>| resolve(&lookup, value);
    let chain = |value: Option<&Value>| -> Vec<String> {
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.is_string())
            .filter_map(|item| resolve(Some(item)))
            .collect()
    };
    let entries = match data {
        Value::Array(entries) => entries.as_slice(),
        _ => data
            .get("systems")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    };
    let mut systems = Vec::new();
    for entry in entries {
        let entry = flatten(entry);
        let field = |key: &str| entry.get(key);
        let parent = field("parent").and_then(Value::as_str).map(str::to_owned);
        let side = side(field("side"));
        let system = match field("type").and_then(Value::as_str) {
            Some("spine") => {
                let pelvis = resolve(field("pelvis"));
                let vertebrae = chain(field("vertebrae"));
                (pelvis.is_some() || !vertebrae.is_empty())
                    .then_some(System::Spine { pelvis, vertebrae })
            }
            Some("arm") => match (
                resolve(field("upper_arm")),
                resolve(field("forearm")),
                resolve(field("hand")),
            ) {
                (Some(upper_arm), Some(forearm), Some(hand)) => Some(System::Arm {
                    side,
                    parent,
                    shoulder: resolve(field("shoulder")),
                    upper_arm,
                    forearm,
                    hand,
                }),
                _ => None,
            },
            Some("leg") => match (
                resolve(field("upper_leg")),
                resolve(field("lower_leg")),
                resolve(field("foot")),
            ) {
                (Some(upper_leg), Some(lower_leg), Some(foot)) => Some(System::Leg {
                    side,
                    parent,
                    upper_leg,
                    lower_leg,
                    foot,
                    toe: resolve(field("toe")),
                }),
                _ => None,
            },
            Some("head") => resolve(field("head")).map(|head| System::Head {
                parent,
                neck: resolve(field("neck")),
                head,
            }),
            Some("finger") => {
                let bones = chain(field("chain"));
                let name = field("name").and_then(Value::as_str).map(str::to_lowercase);
                match name {
                    Some(name) if !bones.is_empty() => Some(System::Finger {
                        name,
                        side,
                        parent,
                        chain: bones,
                    }),
                    _ => None,
                }
            }
            _ => None,
        };
        systems.extend(system);
    }
    systems
}

fn resolve(lookup: &HashMap<String, &String>, value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => lookup
            .get(&text.trim().to_lowercase())
            .map(|name| (*name).clone()),
        Value::Array(items) => items.iter().find_map(|item| resolve(lookup, Some(item))),
        _ => None,
    }
}

/// Accept `{"spine": {...}}` as well as `{"type": "spine", ...}`; local models emit both.
fn flatten(entry: &Value) -> Value {
    if let Some(object) = entry.as_object()
        && !object.contains_key("type")
        && object.len() == 1
        && let Some((key, Value::Object(fields))) = object.iter().next()
    {
        let mut flattened = fields.clone();
        flattened.insert("type".to_owned(), json!(key));
        return Value::Object(flattened);
    }
    entry.clone()
}

fn side(value: Option<&Value>) -> Side {
    match value
        .and_then(Value::as_str)
        .map(|text| text.trim().to_lowercase())
    {
        Some(text) if text.starts_with('r') => Side::R,
        _ => Side::L,
    }
}

/// Bones keyed by anatomical role, such as `arm.L.forearm` or `finger.R.index`.
pub fn roles(systems: &[System]) -> BTreeMap<String, Vec<String>> {
    let mut roles = BTreeMap::new();
    let mut add = |role: String, bones: Vec<String>| {
        if !bones.is_empty() {
            roles.insert(role, bones);
        }
    };
    for system in systems {
        match system {
            System::Spine { pelvis, vertebrae } => {
                add("spine.pelvis".into(), pelvis.iter().cloned().collect());
                add("spine.vertebrae".into(), vertebrae.clone());
            }
            System::Arm {
                side,
                shoulder,
                upper_arm,
                forearm,
                hand,
                ..
            } => {
                add(
                    format!("arm.{side:?}.shoulder"),
                    shoulder.iter().cloned().collect(),
                );
                add(format!("arm.{side:?}.upper_arm"), vec![upper_arm.clone()]);
                add(format!("arm.{side:?}.forearm"), vec![forearm.clone()]);
                add(format!("arm.{side:?}.hand"), vec![hand.clone()]);
            }
            System::Leg {
                side,
                upper_leg,
                lower_leg,
                foot,
                toe,
                ..
            } => {
                add(format!("leg.{side:?}.upper_leg"), vec![upper_leg.clone()]);
                add(format!("leg.{side:?}.lower_leg"), vec![lower_leg.clone()]);
                add(format!("leg.{side:?}.foot"), vec![foot.clone()]);
                add(format!("leg.{side:?}.toe"), toe.iter().cloned().collect());
            }
            System::Head { neck, head, .. } => {
                add("head.neck".into(), neck.iter().cloned().collect());
                add("head.head".into(), vec![head.clone()]);
            }
            System::Finger {
                name, side, chain, ..
            } => add(format!("finger.{side:?}.{name}"), chain.clone()),
        }
    }
    roles
}

/// (source, target) bone pairs for every role both rigs have.
pub fn pair(source: &[System], target: &[System]) -> Vec<(String, String)> {
    let source_roles = roles(source);
    let mut pairs = Vec::new();
    for (role, target_chain) in roles(target) {
        let Some(source_chain) = source_roles.get(&role) else {
            continue;
        };
        for (index, target_bone) in target_chain.iter().enumerate() {
            let source_index = chain_index(index, target_chain.len(), source_chain.len());
            pairs.push((source_chain[source_index].clone(), target_bone.clone()));
        }
    }
    pairs
}

/// Spread a chain of one length over another, keeping both ends aligned.
fn chain_index(index: usize, target_length: usize, source_length: usize) -> usize {
    if target_length <= 1 {
        return source_length - 1;
    }
    (index as f32 * (source_length - 1) as f32 / (target_length - 1) as f32).round() as usize
}

pub fn summary(systems: &[System]) -> String {
    let labels: Vec<String> = systems
        .iter()
        .map(|system| match system {
            System::Spine { vertebrae, .. } => format!("spine({})", vertebrae.len()),
            System::Arm { side, .. } => format!("arm.{side:?}"),
            System::Leg { side, toe, .. } => {
                format!("leg.{side:?}{}", if toe.is_some() { "(toe)" } else { "" })
            }
            System::Head { .. } => "head".to_owned(),
            System::Finger { name, side, .. } => format!("finger.{side:?}.{name}"),
        })
        .collect();
    labels.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn parse_resolves_case_and_drops_invented_bones() {
        let bones = names(&["Hips", "Spine", "LeftArm", "LeftForeArm", "LeftHand"]);
        let data = json!({"systems": [
            {"type": "spine", "pelvis": "hips", "vertebrae": ["Spine", "Ghost"]},
            {"type": "arm", "side": "L", "upper_arm": " LEFTARM ", "forearm": "LeftForeArm", "hand": "LeftHand"},
            {"type": "arm", "side": "R", "upper_arm": "RightArm", "forearm": "RightForeArm", "hand": "RightHand"},
        ]});
        let systems = parse(&data, &bones);
        assert_eq!(systems.len(), 2);
        assert_eq!(
            systems[0],
            System::Spine {
                pelvis: Some("Hips".into()),
                vertebrae: vec!["Spine".into()]
            }
        );
        assert!(matches!(&systems[1], System::Arm { upper_arm, .. } if upper_arm == "LeftArm"));
    }

    #[test]
    fn parse_flattens_entries_keyed_by_system_name() {
        let data = json!({"systems": [{"head": {"neck": "Neck", "head": "Head"}}]});
        let systems = parse(&data, &names(&["Neck", "Head"]));
        assert_eq!(
            systems,
            vec![System::Head {
                parent: None,
                neck: Some("Neck".into()),
                head: "Head".into()
            }]
        );
    }

    #[test]
    fn serializes_in_yello_shape() {
        let text = serde_json::to_string(&[System::Head {
            parent: None,
            neck: None,
            head: "Head".into(),
        }])
        .unwrap();
        assert_eq!(
            text,
            r#"[{"type":"head","parent":null,"neck":null,"head":"Head"}]"#
        );
    }

    #[test]
    fn pairs_chains_of_different_lengths_end_to_end() {
        let source = vec![System::Spine {
            pelvis: Some("Hips".into()),
            vertebrae: names(&["S1", "S2", "S3"]),
        }];
        let target = vec![System::Spine {
            pelvis: Some("pelvis".into()),
            vertebrae: names(&["spine_01", "spine_02"]),
        }];
        let pairs = pair(&source, &target);
        assert!(pairs.contains(&("Hips".into(), "pelvis".into())));
        assert!(pairs.contains(&("S1".into(), "spine_01".into())));
        assert!(pairs.contains(&("S3".into(), "spine_02".into())));
    }
}
