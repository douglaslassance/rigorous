//! Reading, sampling and writing glTF animation clips.

use crate::glb::{Cleanup, Glb, as_usize};
use crate::scene::Transform;
use anyhow::{Context, Result, bail};
use glam::{Quat, Vec3};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const TIME_EPSILON: f32 = 1e-5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    Translation,
    Rotation,
    Scale,
    Weights,
}

impl Property {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "translation" => Some(Self::Translation),
            "rotation" => Some(Self::Rotation),
            "scale" => Some(Self::Scale),
            "weights" => Some(Self::Weights),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Translation => "translation",
            Self::Rotation => "rotation",
            Self::Scale => "scale",
            Self::Weights => "weights",
        }
    }

    fn accessor_type(self) -> &'static str {
        match self {
            Self::Translation | Self::Scale => "VEC3",
            Self::Rotation => "VEC4",
            Self::Weights => "SCALAR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
    CubicSpline,
}

impl Interpolation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Step => "STEP",
            Self::Linear => "LINEAR",
            Self::CubicSpline => "CUBICSPLINE",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub node: usize,
    pub property: Property,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    /// Flat output values; cubic spline keys hold in tangent, value and out tangent.
    pub values: Vec<f32>,
    pub width: usize,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    pub channels: Vec<Channel>,
}

pub fn read_clips(glb: &Glb) -> Result<Vec<Clip>> {
    let mut clips = Vec::new();
    for (index, animation) in glb.array("animations").iter().enumerate() {
        let name = animation
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("animation_{index}"));
        let samplers = animation
            .get("samplers")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut channels = Vec::new();
        for channel in animation
            .get("channels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(node) = as_usize(channel.pointer("/target/node")) else {
                continue;
            };
            let Some(property) = channel
                .pointer("/target/path")
                .and_then(Value::as_str)
                .and_then(Property::parse)
            else {
                continue;
            };
            let sampler = as_usize(channel.get("sampler"))
                .and_then(|sampler| samplers.get(sampler))
                .with_context(|| {
                    format!("animation '{name}' has a channel without a valid sampler")
                })?;
            let interpolation = match sampler.get("interpolation").and_then(Value::as_str) {
                Some("STEP") => Interpolation::Step,
                Some("CUBICSPLINE") => Interpolation::CubicSpline,
                _ => Interpolation::Linear,
            };
            let input = as_usize(sampler.get("input")).context("sampler has no input")?;
            let output = as_usize(sampler.get("output")).context("sampler has no output")?;
            let times = glb.read_accessor(input)?.values;
            let output = glb.read_accessor(output)?;
            let parts = if interpolation == Interpolation::CubicSpline {
                3
            } else {
                1
            };
            let width = match property {
                Property::Weights => output.values.len() / (times.len() * parts).max(1),
                _ => output.width,
            };
            if times.is_empty() || width == 0 || output.values.len() != times.len() * parts * width
            {
                bail!("animation '{name}' has a channel whose key counts do not match");
            }
            channels.push(Channel {
                node,
                property,
                interpolation,
                times,
                values: output.values,
                width,
            });
        }
        clips.push(Clip { name, channels });
    }
    Ok(clips)
}

impl Channel {
    fn element(&self, key: usize, part: usize) -> &[f32] {
        let index = match self.interpolation {
            Interpolation::CubicSpline => key * 3 + part,
            _ => key,
        };
        &self.values[index * self.width..(index + 1) * self.width]
    }

    pub fn sample(&self, time: f32) -> Vec<f32> {
        let last = self.times.len() - 1;
        if time <= self.times[0] {
            return self.element(0, 1).to_vec();
        }
        if time >= self.times[last] {
            return self.element(last, 1).to_vec();
        }
        let next = self.times.partition_point(|&key_time| key_time <= time);
        let key = next - 1;
        let span = self.times[next] - self.times[key];
        let factor = if span > 0.0 {
            (time - self.times[key]) / span
        } else {
            0.0
        };
        match self.interpolation {
            Interpolation::Step => self.element(key, 1).to_vec(),
            Interpolation::Linear if self.property == Property::Rotation => {
                let start = Quat::from_slice(self.element(key, 1)).normalize();
                let end = Quat::from_slice(self.element(next, 1)).normalize();
                start.slerp(end, factor).to_array().to_vec()
            }
            Interpolation::Linear => {
                let start = self.element(key, 1);
                let end = self.element(next, 1);
                start
                    .iter()
                    .zip(end)
                    .map(|(a, b)| a + (b - a) * factor)
                    .collect()
            }
            Interpolation::CubicSpline => {
                let squared = factor * factor;
                let cubed = squared * factor;
                let start = self.element(key, 1);
                let start_out = self.element(key, 2);
                let end = self.element(next, 1);
                let end_in = self.element(next, 0);
                let mut values: Vec<f32> = (0..self.width)
                    .map(|i| {
                        (2.0 * cubed - 3.0 * squared + 1.0) * start[i]
                            + (cubed - 2.0 * squared + factor) * span * start_out[i]
                            + (-2.0 * cubed + 3.0 * squared) * end[i]
                            + (cubed - squared) * span * end_in[i]
                    })
                    .collect();
                if self.property == Property::Rotation {
                    values = Quat::from_slice(&values).normalize().to_array().to_vec();
                }
                values
            }
        }
    }

    pub fn sample_vec3(&self, time: f32) -> Vec3 {
        Vec3::from_slice(&self.sample(time))
    }

    pub fn sample_quat(&self, time: f32) -> Quat {
        Quat::from_slice(&self.sample(time)).normalize()
    }
}

impl Clip {
    /// Every key time used by any channel, sorted and deduplicated.
    pub fn times(&self) -> Vec<f32> {
        let mut times: Vec<f32> = self
            .channels
            .iter()
            .flat_map(|channel| channel.times.iter().copied())
            .collect();
        times.sort_by(f32::total_cmp);
        times.dedup_by(|a, b| (*a - *b).abs() < TIME_EPSILON);
        times
    }

    pub fn start(&self) -> f32 {
        let start = self
            .channels
            .iter()
            .map(|channel| channel.times[0])
            .fold(f32::INFINITY, f32::min);
        if start.is_finite() { start } else { 0.0 }
    }

    /// Local transforms at `time`; nodes this clip does not animate keep `defaults`.
    pub fn pose(&self, defaults: &[Transform], time: f32) -> Vec<Transform> {
        let mut pose = defaults.to_vec();
        for channel in &self.channels {
            let Some(transform) = pose.get_mut(channel.node) else {
                continue;
            };
            match channel.property {
                Property::Translation => transform.translation = channel.sample_vec3(time),
                Property::Rotation => transform.rotation = channel.sample_quat(time),
                Property::Scale => transform.scale = channel.sample_vec3(time),
                Property::Weights => {}
            }
        }
        pose
    }
}

pub fn clip_names(glb: &Glb) -> Vec<String> {
    glb.array("animations")
        .iter()
        .enumerate()
        .map(|(index, animation)| {
            animation
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("animation_{index}"))
        })
        .collect()
}

/// Remove the clips with these names, along with the data only they used.
pub fn remove_clips(glb: &mut Glb, names: &[String]) -> Result<Cleanup> {
    let all = clip_names(glb);
    let mut candidates = BTreeSet::new();
    let mut index = 0;
    glb.array_mut("animations").retain(|animation| {
        index += 1;
        let remove = names.contains(&all[index - 1]);
        if remove {
            for sampler in animation
                .get("samplers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                candidates.extend(
                    ["input", "output"]
                        .iter()
                        .filter_map(|key| as_usize(sampler.get(*key))),
                );
            }
        }
        !remove
    });
    glb.remove_unused_accessors(&candidates)
}

pub fn add_clip(glb: &mut Glb, name: &str, channels: &[Channel]) -> Result<()> {
    let mut inputs: Vec<(&[f32], usize)> = Vec::new();
    let mut samplers = Vec::new();
    let mut targets = Vec::new();
    for channel in channels {
        let input = match inputs
            .iter()
            .find(|(times, _)| *times == channel.times.as_slice())
        {
            Some(&(_, input)) => input,
            None => {
                let input = glb.append_accessor(&channel.times, "SCALAR", true)?;
                inputs.push((&channel.times, input));
                input
            }
        };
        let output =
            glb.append_accessor(&channel.values, channel.property.accessor_type(), false)?;
        targets.push(json!({"sampler": samplers.len(), "target": {"node": channel.node, "path": channel.property.as_str()}}));
        samplers.push(json!({"input": input, "interpolation": channel.interpolation.as_str(), "output": output}));
    }
    glb.array_mut("animations")
        .push(json!({"channels": targets, "name": name, "samplers": samplers}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(
        property: Property,
        interpolation: Interpolation,
        times: Vec<f32>,
        values: Vec<f32>,
        width: usize,
    ) -> Channel {
        Channel {
            node: 0,
            property,
            interpolation,
            times,
            values,
            width,
        }
    }

    #[test]
    fn linear_translation_interpolates_and_clamps() {
        let channel = channel(
            Property::Translation,
            Interpolation::Linear,
            vec![0.0, 1.0],
            vec![0.0, 0.0, 0.0, 2.0, 4.0, 6.0],
            3,
        );
        assert_eq!(channel.sample_vec3(0.5), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(channel.sample_vec3(-1.0), Vec3::ZERO);
        assert_eq!(channel.sample_vec3(5.0), Vec3::new(2.0, 4.0, 6.0));
    }

    #[test]
    fn linear_rotation_slerps() {
        let end = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let mut values = Quat::IDENTITY.to_array().to_vec();
        values.extend(end.to_array());
        let channel = channel(
            Property::Rotation,
            Interpolation::Linear,
            vec![0.0, 1.0],
            values,
            4,
        );
        let half = channel.sample_quat(0.5);
        assert!(half.abs_diff_eq(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4), 1e-5));
    }

    #[test]
    fn cubic_spline_with_flat_tangents_eases() {
        let values = vec![0.0, 0.0, 0.0, 0.0, 10.0, 0.0];
        let channel = channel(
            Property::Weights,
            Interpolation::CubicSpline,
            vec![0.0, 1.0],
            values,
            1,
        );
        assert!((channel.sample(0.5)[0] - 5.0).abs() < 1e-5);
        assert!(channel.sample(0.25)[0] < 2.5);
    }

    #[test]
    fn clip_round_trips_through_glb() {
        let mut glb = Glb {
            json: json!({"asset": {"version": "2.0"}, "nodes": [{"name": "a"}]}),
            bin: Vec::new(),
        };
        let rotation = channel(
            Property::Rotation,
            Interpolation::Linear,
            vec![0.0, 1.0],
            [Quat::IDENTITY.to_array(), Quat::IDENTITY.to_array()].concat(),
            4,
        );
        add_clip(&mut glb, "Walk", &[rotation]).unwrap();
        let clips = read_clips(&glb).unwrap();
        assert_eq!(clips[0].name, "Walk");
        assert_eq!(clips[0].channels[0].times, vec![0.0, 1.0]);
        remove_clips(&mut glb, &["Walk".to_owned()]).unwrap();
        assert!(clip_names(&glb).is_empty());
    }
}
