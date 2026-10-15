//! The joints of a character and the poses they can be read in.

use crate::glb::{Glb, as_usize};
use crate::scene::Scene;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub struct Skeleton {
    /// Joint node indices, parents before children.
    pub joints: Vec<usize>,
    joint_set: HashSet<usize>,
}

impl Skeleton {
    /// Skin joints plus animated nodes below them, or every animated node when the file has no skin.
    pub fn new(glb: &Glb, scene: &Scene) -> Self {
        let skin_joints: HashSet<usize> = glb
            .array("skins")
            .iter()
            .flat_map(|skin| {
                skin.get("joints")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|joint| as_usize(Some(joint)))
            .filter(|&joint| joint < scene.names.len())
            .collect();
        let animated = glb
            .array("animations")
            .iter()
            .flat_map(|animation| {
                animation
                    .get("channels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter(|channel| {
                channel.pointer("/target/path").and_then(Value::as_str) != Some("weights")
            })
            .filter_map(|channel| as_usize(channel.pointer("/target/node")))
            .filter(|&node| node < scene.names.len());
        let below_skin_joint = |node: usize| {
            let mut current = scene.parents[node];
            while let Some(ancestor) = current {
                if skin_joints.contains(&ancestor) {
                    return true;
                }
                current = scene.parents[ancestor];
            }
            false
        };
        let joint_set: HashSet<usize> = if skin_joints.is_empty() {
            animated.collect()
        } else {
            animated
                .filter(|&node| below_skin_joint(node))
                .chain(skin_joints.iter().copied())
                .collect()
        };
        let joints = scene
            .order
            .iter()
            .copied()
            .filter(|joint| joint_set.contains(joint))
            .collect();
        Self { joints, joint_set }
    }

    pub fn contains(&self, node: usize) -> bool {
        self.joint_set.contains(&node)
    }

    /// Nearest ancestor that is also a joint.
    pub fn joint_parent(&self, scene: &Scene, joint: usize) -> Option<usize> {
        let mut current = scene.parents[joint];
        while let Some(node) = current {
            if self.contains(node) {
                return Some(node);
            }
            current = scene.parents[node];
        }
        None
    }

    pub fn find(&self, scene: &Scene, name: &str) -> Option<usize> {
        self.joints
            .iter()
            .copied()
            .find(|&joint| scene.names[joint] == name)
    }

    pub fn names(&self, scene: &Scene) -> Vec<String> {
        self.joints
            .iter()
            .map(|&joint| scene.names[joint].clone())
            .collect()
    }

    pub fn bones_with_parents(&self, scene: &Scene) -> Vec<(String, Option<String>)> {
        self.joints
            .iter()
            .map(|&joint| {
                (
                    scene.names[joint].clone(),
                    self.joint_parent(scene, joint)
                        .map(|parent| scene.names[parent].clone()),
                )
            })
            .collect()
    }

    /// Topmost joint above `joint`, such as a root motion bone above the hips.
    pub fn root_above(&self, scene: &Scene, joint: usize) -> Option<usize> {
        let mut root = None;
        let mut current = self.joint_parent(scene, joint);
        while let Some(node) = current {
            root = Some(node);
            current = self.joint_parent(scene, node);
        }
        root
    }
}

/// Indented tree of bone names, the format the classifier prompt expects.
pub fn format_hierarchy(bones: &[(String, Option<String>)]) -> String {
    let names: HashSet<&str> = bones.iter().map(|(name, _)| name.as_str()).collect();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut roots = Vec::new();
    for (name, parent) in bones {
        match parent.as_deref().filter(|parent| names.contains(parent)) {
            Some(parent) => children.entry(parent).or_default().push(name),
            None => roots.push(name.as_str()),
        }
    }
    let mut lines = Vec::new();
    let mut stack: Vec<(&str, usize)> = roots.into_iter().rev().map(|root| (root, 0)).collect();
    while let Some((name, depth)) = stack.pop() {
        lines.push(format!("{}- {name}", "  ".repeat(depth)));
        if let Some(kids) = children.get(name) {
            stack.extend(kids.iter().rev().map(|&child| (child, depth + 1)));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_indents_children_under_parents() {
        let bones = vec![
            ("Hips".to_owned(), None),
            ("Spine02".to_owned(), Some("Hips".to_owned())),
            ("Spine01".to_owned(), Some("Spine02".to_owned())),
            ("LeftUpLeg".to_owned(), Some("Hips".to_owned())),
        ];
        assert_eq!(
            format_hierarchy(&bones),
            "- Hips\n  - Spine02\n    - Spine01\n  - LeftUpLeg"
        );
    }
}
