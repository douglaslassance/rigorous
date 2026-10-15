//! The joints of a character and the poses they can be read in.

use crate::glb::{Glb, as_usize};
use crate::scene::{Scene, Transform};
use anyhow::Result;
use glam::Mat4;
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

/// Local transforms that put every skinned joint in its bind pose.
pub fn bind_pose(glb: &Glb, scene: &Scene) -> Result<Vec<Transform>> {
    let default_world = scene.world_matrices(&scene.defaults);
    let mut bind_world: HashMap<usize, Mat4> = HashMap::new();
    for (skin_index, skin) in glb.array("skins").iter().enumerate() {
        let Some(matrices) = as_usize(skin.get("inverseBindMatrices")) else {
            continue;
        };
        let matrices = glb.read_accessor(matrices)?;
        let joints: Vec<(usize, Mat4)> = skin
            .get("joints")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, joint)| {
                let values = matrices.values.get(index * 16..index * 16 + 16)?;
                Some((
                    as_usize(Some(joint))?,
                    Mat4::from_cols_slice(values).inverse(),
                ))
            })
            .filter(|&(joint, _)| joint < scene.names.len())
            .collect();
        let mesh_world = skinned_mesh_node(glb, skin_index)
            .map(|node| default_world[node])
            .unwrap_or(Mat4::IDENTITY);
        // Exporters disagree on whether inverse bind matrices are in world or skinned mesh space.
        let in_mesh_space = !mesh_world.abs_diff_eq(Mat4::IDENTITY, 1e-6) && {
            let distance = |space: Mat4| -> f32 {
                joints
                    .iter()
                    .map(|&(joint, matrix)| {
                        ((space * matrix).w_axis - default_world[joint].w_axis).length()
                    })
                    .sum()
            };
            distance(mesh_world) < distance(Mat4::IDENTITY)
        };
        for (joint, matrix) in joints {
            let world = if in_mesh_space {
                mesh_world * matrix
            } else {
                matrix
            };
            bind_world.entry(joint).or_insert(world);
        }
    }
    let mut locals = scene.defaults.clone();
    let mut world = vec![Mat4::IDENTITY; scene.names.len()];
    for &node in &scene.order {
        let parent_world = scene.parents[node]
            .map(|parent| world[parent])
            .unwrap_or(Mat4::IDENTITY);
        world[node] = match bind_world.get(&node) {
            Some(&matrix) => {
                locals[node] = Transform::from_matrix(parent_world.inverse() * matrix);
                matrix
            }
            None => parent_world * locals[node].matrix(),
        };
    }
    Ok(locals)
}

fn skinned_mesh_node(glb: &Glb, skin: usize) -> Option<usize> {
    glb.array("nodes")
        .iter()
        .position(|node| as_usize(node.get("skin")) == Some(skin))
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
    use serde_json::json;

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

    #[test]
    fn bind_pose_recovers_joint_world_from_inverse_bind_matrices() {
        let bind = Mat4::from_translation(glam::Vec3::new(0.0, 1.0, 0.0)).inverse();
        let bin: Vec<u8> = bind
            .to_cols_array()
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let glb = Glb {
            json: json!({
                "nodes": [{"name": "Armature", "children": [1], "scale": [0.5, 0.5, 0.5]}, {"name": "Hips", "translation": [0.0, 7.0, 0.0]}],
                "skins": [{"joints": [1], "inverseBindMatrices": 0}],
                "buffers": [{"byteLength": 64}],
                "bufferViews": [{"buffer": 0, "byteLength": 64}],
                "accessors": [{"bufferView": 0, "componentType": 5126, "count": 1, "type": "MAT4"}],
            }),
            bin,
        };
        let scene = Scene::new(&glb.json).unwrap();
        let pose = bind_pose(&glb, &scene).unwrap();
        assert!(
            pose[1]
                .translation
                .abs_diff_eq(glam::Vec3::new(0.0, 2.0, 0.0), 1e-5)
        );
        let skeleton = Skeleton::new(&glb, &scene);
        assert_eq!(skeleton.joints, vec![1]);
    }

    #[test]
    fn bind_pose_detects_matrices_in_skinned_mesh_space() {
        let bind = Mat4::from_translation(glam::Vec3::new(0.0, 100.0, 0.0)).inverse();
        let bin: Vec<u8> = bind
            .to_cols_array()
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let glb = Glb {
            json: json!({
                "nodes": [{"name": "Hips", "translation": [0.0, 1.0, 0.0]}, {"name": "Body", "skin": 0, "scale": [0.01, 0.01, 0.01]}],
                "skins": [{"joints": [0], "inverseBindMatrices": 0}],
                "buffers": [{"byteLength": 64}],
                "bufferViews": [{"buffer": 0, "byteLength": 64}],
                "accessors": [{"bufferView": 0, "componentType": 5126, "count": 1, "type": "MAT4"}],
            }),
            bin,
        };
        let scene = Scene::new(&glb.json).unwrap();
        let pose = bind_pose(&glb, &scene).unwrap();
        assert!(
            pose[0]
                .translation
                .abs_diff_eq(glam::Vec3::new(0.0, 1.0, 0.0), 1e-5)
        );
        assert!(pose[0].scale.abs_diff_eq(glam::Vec3::splat(0.01), 1e-6));
    }
}
