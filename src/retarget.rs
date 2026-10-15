//! World space retargeting relative to a reference pose on each rig.

use crate::animation::{Channel, Clip, Interpolation, Property};
use crate::scene::{Scene, Transform, rotation_of};
use anyhow::{Result, bail};
use glam::{Mat4, Quat, Vec3};

/// A rig evaluated in its reference pose, its default pose or a T-pose clip for example.
pub struct Rig<'a> {
    pub scene: &'a Scene,
    pub reference_rotations: Vec<Quat>,
    pub reference_positions: Vec<Vec3>,
}

impl<'a> Rig<'a> {
    pub fn new(scene: &'a Scene, reference: &[Transform]) -> Self {
        let reference_rotations = scene.world_rotations(reference);
        let reference_positions = scene
            .world_matrices(reference)
            .iter()
            .map(|matrix| matrix.w_axis.truncate())
            .collect();
        Self {
            scene,
            reference_rotations,
            reference_positions,
        }
    }

    /// Height of `pelvis` above the lowest of `feet` in the reference pose, Y up.
    pub fn hip_height(&self, pelvis: usize, feet: &[usize]) -> f32 {
        let ground = feet
            .iter()
            .map(|&foot| self.reference_positions[foot].y)
            .fold(f32::INFINITY, f32::min);
        let ground = if ground.is_finite() { ground } else { 0.0 };
        self.reference_positions[pelvis].y - ground
    }
}

/// Bone pairs as (source node, target node).
#[derive(Default)]
pub struct Mapping {
    pub rotations: Vec<(usize, usize)>,
    pub translations: Vec<(usize, usize)>,
}

pub fn parse_facing(text: &str) -> Result<Vec3> {
    Ok(match text.trim().to_lowercase().as_str() {
        "+x" | "x" => Vec3::X,
        "-x" => Vec3::NEG_X,
        "+z" | "z" => Vec3::Z,
        "-z" => Vec3::NEG_Z,
        other => bail!("invalid facing '{other}', expected +x, -x, +z or -z"),
    })
}

/// Yaw that turns the source facing direction onto the target facing direction.
pub fn facing_rotation(source: Vec3, target: Vec3) -> Quat {
    let yaw = |direction: Vec3| direction.x.atan2(direction.z);
    Quat::from_rotation_y(yaw(target) - yaw(source))
}

/// Resample `clip` onto the target rig, keying rotations and root translations only.
pub fn retarget(
    clip: &Clip,
    source: &Rig,
    target: &Rig,
    mapping: &Mapping,
    facing: Quat,
    scale: f32,
) -> Vec<Channel> {
    let times = clip.times();
    let count = target.scene.names.len();
    let mut rotation_source = vec![None; count];
    for &(source_node, target_node) in &mapping.rotations {
        rotation_source[target_node] = Some(source_node);
    }
    let mut translation_source = vec![None; count];
    for &(source_node, target_node) in &mapping.translations {
        translation_source[target_node] = Some(source_node);
    }
    let mut rotation_values = vec![Vec::new(); count];
    let mut translation_values = vec![Vec::new(); count];
    let mut previous: Vec<Option<Quat>> = vec![None; count];
    for &time in &times {
        let source_matrices = source
            .scene
            .world_matrices(&clip.pose(&source.scene.defaults, time));
        let source_rotations: Vec<Quat> =
            source_matrices.iter().copied().map(rotation_of).collect();
        let mut local = target.scene.defaults.clone();
        let mut world_matrices = vec![Mat4::IDENTITY; count];
        for &node in &target.scene.order {
            let parent_matrix = target.scene.parents[node]
                .map(|parent| world_matrices[parent])
                .unwrap_or(Mat4::IDENTITY);
            let parent_rotation = rotation_of(parent_matrix);
            if let Some(source_node) = rotation_source[node] {
                let delta = source_rotations[source_node]
                    * source.reference_rotations[source_node].inverse();
                let desired = facing * delta * facing.inverse() * target.reference_rotations[node];
                let mut rotation = (parent_rotation.inverse() * desired).normalize();
                if previous[node].is_some_and(|previous| previous.dot(rotation) < 0.0) {
                    rotation = -rotation;
                }
                previous[node] = Some(rotation);
                local[node].rotation = rotation;
                rotation_values[node].extend(rotation.to_array());
            }
            if let Some(source_node) = translation_source[node] {
                let offset = source_matrices[source_node].w_axis.truncate()
                    - source.reference_positions[source_node];
                let desired = target.reference_positions[node] + facing * (offset * scale);
                local[node].translation = parent_matrix.inverse().transform_point3(desired);
                translation_values[node].extend(local[node].translation.to_array());
            }
            world_matrices[node] = parent_matrix * local[node].matrix();
        }
    }
    let channel = |node: usize, property: Property, values: Vec<f32>, width: usize| Channel {
        node,
        property,
        interpolation: Interpolation::Linear,
        times: times.clone(),
        values,
        width,
    };
    let mut channels = Vec::new();
    for node in 0..count {
        if !translation_values[node].is_empty() {
            channels.push(channel(
                node,
                Property::Translation,
                std::mem::take(&mut translation_values[node]),
                3,
            ));
        }
        if !rotation_values[node].is_empty() {
            channels.push(channel(
                node,
                Property::Rotation,
                std::mem::take(&mut rotation_values[node]),
                4,
            ));
        }
    }
    channels
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::f32::consts::FRAC_PI_2;

    /// Hips with a straight arm pointing along +X, under an armature node.
    fn rig_json(arm_rotation: Quat, armature_scale: f32, hips_height: f32) -> serde_json::Value {
        json!({"nodes": [
            {"name": "Armature", "children": [1], "scale": [armature_scale, armature_scale, armature_scale]},
            {"name": "Hips", "children": [2], "translation": [0.0, hips_height / armature_scale, 0.0]},
            {"name": "Arm", "children": [3], "rotation": arm_rotation.to_array()},
            {"name": "Hand", "translation": [0.0, 0.5 / armature_scale, 0.0]},
        ]})
    }

    fn clip(rotations: &[Quat], hips: &[Vec3]) -> Clip {
        let times: Vec<f32> = (0..rotations.len()).map(|index| index as f32).collect();
        Clip {
            name: "Test".into(),
            channels: vec![
                Channel {
                    node: 2,
                    property: Property::Rotation,
                    interpolation: Interpolation::Linear,
                    times: times.clone(),
                    values: rotations
                        .iter()
                        .flat_map(|rotation| rotation.to_array())
                        .collect(),
                    width: 4,
                },
                Channel {
                    node: 1,
                    property: Property::Translation,
                    interpolation: Interpolation::Linear,
                    times,
                    values: hips
                        .iter()
                        .flat_map(|position| position.to_array())
                        .collect(),
                    width: 3,
                },
            ],
        }
    }

    fn mapping() -> Mapping {
        Mapping {
            rotations: vec![(1, 1), (2, 2)],
            translations: vec![(1, 1)],
        }
    }

    #[test]
    fn same_rig_reproduces_the_clip() {
        let scene = Scene::new(&rig_json(Quat::IDENTITY, 1.0, 1.0)).unwrap();
        let rig = Rig::new(&scene, &scene.defaults);
        let raised = Quat::from_rotation_z(0.7);
        let clip = clip(
            &[Quat::IDENTITY, raised],
            &[Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.8, 0.3)],
        );
        let channels = retarget(&clip, &rig, &rig, &mapping(), Quat::IDENTITY, 1.0);
        let arm = channels.iter().find(|channel| channel.node == 2).unwrap();
        assert!(Quat::from_slice(&arm.values[4..8]).abs_diff_eq(raised, 1e-5));
        let hips = channels
            .iter()
            .find(|channel| channel.node == 1 && channel.property == Property::Translation)
            .unwrap();
        assert!(Vec3::from_slice(&hips.values[3..6]).abs_diff_eq(Vec3::new(0.0, 0.8, 0.3), 1e-5));
    }

    #[test]
    fn different_bone_axes_and_units_give_the_same_world_motion() {
        let source_scene = Scene::new(&rig_json(Quat::IDENTITY, 1.0, 1.0)).unwrap();
        let target_scene =
            Scene::new(&rig_json(Quat::from_rotation_y(FRAC_PI_2), 0.01, 2.0)).unwrap();
        let source = Rig::new(&source_scene, &source_scene.defaults);
        let target = Rig::new(&target_scene, &target_scene.defaults);
        let raised = Quat::from_rotation_z(0.7);
        let clip = clip(
            &[Quat::IDENTITY, raised],
            &[Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.5)],
        );
        let scale = target.hip_height(1, &[]) / source.hip_height(1, &[]);
        let channels = retarget(&clip, &source, &target, &mapping(), Quat::IDENTITY, scale);
        let mut pose = target_scene.defaults.clone();
        for channel in &channels {
            match channel.property {
                Property::Rotation => {
                    pose[channel.node].rotation = Quat::from_slice(&channel.values[4..8])
                }
                Property::Translation => {
                    pose[channel.node].translation = Vec3::from_slice(&channel.values[3..6])
                }
                _ => {}
            }
        }
        let world = target_scene.world_rotations(&pose);
        let expected = raised * target.reference_rotations[2];
        assert!(world[2].abs_diff_eq(expected, 1e-5) || world[2].abs_diff_eq(-expected, 1e-5));
        let hips = target_scene.world_matrices(&pose)[1].w_axis.truncate();
        assert!(hips.abs_diff_eq(Vec3::new(0.0, 2.0, 1.0), 1e-4));
    }

    #[test]
    fn opposite_facing_turns_motion_around() {
        let rotation = facing_rotation(Vec3::Z, Vec3::NEG_Z);
        assert!((rotation * Vec3::new(0.0, 0.0, 1.0)).abs_diff_eq(Vec3::NEG_Z, 1e-6));
        assert!(parse_facing("+y").is_err());
    }
}
