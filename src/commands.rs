//! The CLI commands.

use crate::animation::{self, Channel, Clip, Interpolation, Property};
use crate::classify;
use crate::glb::{Glb, as_usize};
use crate::retarget::{self, Mapping, Rig};
use crate::scene::{Scene, Transform};
use crate::skeleton::{self, Skeleton};
use crate::systems::{self, System};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// Report progress on stdout without failing when the reader has gone away, so the file still gets written.
macro_rules! say {
    ($($argument:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), $($argument)*);
    }};
}

/// Combined rigs may differ by at most this share of animated nodes.
const COMBINE_MISSING_TOLERANCE: f32 = 0.1;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum PoseSource {
    Bind,
    Default,
}

pub struct TransferOptions<'a> {
    pub poses: &'a [String],
    pub facings: &'a [String],
    pub overwrite: bool,
    pub output: Option<&'a Path>,
    pub model: &'a str,
}

pub fn combine(
    source_path: &Path,
    destination_path: &Path,
    overwrite: bool,
    output: Option<&Path>,
) -> Result<()> {
    let source = Glb::read(source_path)?;
    let mut destination = Glb::read(destination_path)?;
    let source_scene = Scene::new(&source.json)?;
    let destination_scene = Scene::new(&destination.json)?;
    let clips = animation::read_clips(&source)?;
    if clips.is_empty() {
        bail!("{} has no animations", source_path.display());
    }
    let lookup = NodeLookup::new(&destination_scene);
    let animated: BTreeSet<usize> = clips
        .iter()
        .flat_map(|clip| clip.channels.iter().map(|channel| channel.node))
        .collect();
    let mut node_map = HashMap::new();
    let mut missing = Vec::new();
    for &node in &animated {
        match lookup.find(&source_scene.names[node]) {
            Some(target) => {
                node_map.insert(node, target);
            }
            None => missing.push(source_scene.names[node].clone()),
        }
    }
    if missing.len() as f32 > animated.len() as f32 * COMBINE_MISSING_TOLERANCE {
        bail!(
            "the rigs differ. {} of {} animated nodes have no match in {} ({}). Use `retarjet transfer` instead.",
            missing.len(),
            animated.len(),
            destination_path.display(),
            preview(&missing)
        );
    }
    let mut reparented: Vec<String> = node_map
        .iter()
        .filter(|&(&node, &target)| {
            let Some(parent) =
                source_scene.parents[node].filter(|parent| animated.contains(parent))
            else {
                return false;
            };
            lookup.find(&source_scene.names[parent]) != destination_scene.parents[target]
        })
        .map(|(&node, _)| source_scene.names[node].clone())
        .collect();
    reparented.sort();
    if !reparented.is_empty() {
        bail!(
            "the rigs differ. {} have a different parent in {}. Use `retarjet transfer` instead.",
            preview(&reparented),
            destination_path.display()
        );
    }
    if !missing.is_empty() {
        eprintln!(
            "warning: skipping channels of nodes missing from the destination ({})",
            preview(&missing)
        );
    }
    let mut existing = animation::clip_names(&destination);
    let mut changed = false;
    for clip in &clips {
        let channels: Vec<Channel> = clip
            .channels
            .iter()
            .filter_map(|channel| {
                let node = *node_map.get(&channel.node)?;
                let compatible = channel.property != Property::Weights
                    || morph_target_count(&destination, node) == Some(channel.width);
                compatible.then(|| Channel {
                    node,
                    ..channel.clone()
                })
            })
            .collect();
        changed |= place(
            &mut destination,
            &mut existing,
            &clip.name,
            &channels,
            overwrite,
        )?;
    }
    finish(&mut destination, destination_path, output, changed)
}

pub fn transfer(
    source_path: &Path,
    destination_path: &Path,
    options: &TransferOptions,
) -> Result<()> {
    let poses = per_file(options.poses);
    let (source_pose, destination_pose) = (
        poses.map(|(source, _)| source),
        poses.map(|(_, destination)| destination),
    );
    let (source_facing, destination_facing) = per_file(options.facings).unwrap_or(("+z", "+z"));
    let facing = retarget::facing_rotation(
        retarget::parse_facing(source_facing)?,
        retarget::parse_facing(destination_facing)?,
    );

    let source = Glb::read(source_path)?;
    let mut destination = Glb::read(destination_path)?;
    let source_scene = Scene::new(&source.json)?;
    let destination_scene = Scene::new(&destination.json)?;
    let source_skeleton = skeleton_of(&source, &source_scene, source_path)?;
    let destination_skeleton = skeleton_of(&destination, &destination_scene, destination_path)?;

    let source_clips = animation::read_clips(&source)?;
    let destination_clips = animation::read_clips(&destination)?;
    let source_reference = reference_pose(&source_clips, &source_scene, source_pose, source_path)?;
    let destination_reference = reference_pose(
        &destination_clips,
        &destination_scene,
        destination_pose,
        destination_path,
    )?;

    let (source_systems, source_classified) = systems_for(
        &source,
        &source_scene,
        &source_skeleton,
        options.model,
        source_path,
    )?;
    if source_classified {
        eprintln!(
            "hint: run `retarjet classify {}` to store this classification and skip it next time",
            source_path.display()
        );
    }
    let (destination_systems, destination_classified) = systems_for(
        &destination,
        &destination_scene,
        &destination_skeleton,
        options.model,
        destination_path,
    )?;

    let source_rig = Rig::new(&source_scene, &source_reference);
    let destination_rig = Rig::new(&destination_scene, &destination_reference);
    let source_side = Side {
        scene: &source_scene,
        skeleton: &source_skeleton,
        systems: &source_systems,
        rig: &source_rig,
    };
    let destination_side = Side {
        scene: &destination_scene,
        skeleton: &destination_skeleton,
        systems: &destination_systems,
        rig: &destination_rig,
    };
    let (mapping, scale) = build_mapping(&source_side, &destination_side)?;
    say!("Mapped {} bones, scale {scale:.3}", mapping.rotations.len());

    let mut changed = false;
    if destination_classified {
        let host = destination_skeleton
            .host(&destination_scene)
            .context("destination has no skeleton")?;
        systems::store(&mut destination, host, &destination_systems)?;
        changed = true;
    }
    let mut existing = animation::clip_names(&destination);
    for clip in &source_clips {
        if Some(clip.name.as_str()) == source_pose {
            continue;
        }
        if Some(clip.name.as_str()) == destination_pose {
            say!(
                "Skipped '{}' (it is the destination reference pose)",
                clip.name
            );
            continue;
        }
        let channels =
            retarget::retarget(clip, &source_rig, &destination_rig, &mapping, facing, scale);
        changed |= place(
            &mut destination,
            &mut existing,
            &clip.name,
            &channels,
            options.overwrite,
        )?;
    }
    finish(&mut destination, destination_path, options.output, changed)
}

pub fn pose(
    path: &Path,
    name: &str,
    from: PoseSource,
    overwrite: bool,
    output: Option<&Path>,
) -> Result<()> {
    let mut glb = Glb::read(path)?;
    let scene = Scene::new(&glb.json)?;
    let skeleton = skeleton_of(&glb, &scene, path)?;
    let pose = match from {
        PoseSource::Bind => {
            if !glb
                .array("skins")
                .iter()
                .any(|skin| skin.get("inverseBindMatrices").is_some())
            {
                bail!(
                    "{} has no bind pose. Use `--from default` instead.",
                    path.display()
                );
            }
            skeleton::bind_pose(&glb, &scene)?
        }
        PoseSource::Default => scene.defaults.clone(),
    };
    let key = |node: usize, property: Property, values: Vec<f32>| Channel {
        node,
        property,
        interpolation: Interpolation::Linear,
        times: vec![0.0],
        width: values.len(),
        values,
    };
    let channels: Vec<Channel> = skeleton
        .joints
        .iter()
        .flat_map(|&joint| {
            let transform = pose[joint];
            [
                key(
                    joint,
                    Property::Translation,
                    transform.translation.to_array().to_vec(),
                ),
                key(
                    joint,
                    Property::Rotation,
                    transform.rotation.to_array().to_vec(),
                ),
                key(joint, Property::Scale, transform.scale.to_array().to_vec()),
            ]
        })
        .collect();
    let mut existing = animation::clip_names(&glb);
    let changed = place(&mut glb, &mut existing, name, &channels, overwrite)?;
    finish(&mut glb, path, output, changed)
}

pub fn classify(path: &Path, force: bool, output: Option<&Path>, model: &str) -> Result<()> {
    let mut glb = Glb::read(path)?;
    let scene = Scene::new(&glb.json)?;
    let skeleton = skeleton_of(&glb, &scene, path)?;
    if !force && let Some(systems) = systems::load(&glb, &skeleton.names(&scene)) {
        say!(
            "{} (already stored, pass --force to classify again)",
            systems::summary(&systems)
        );
        return Ok(());
    }
    eprintln!("Classifying the rig in {} with {model}...", path.display());
    let systems = classify::classify(&skeleton.bones_with_parents(&scene), model)?;
    say!("{}", systems::summary(&systems));
    let host = skeleton
        .host(&scene)
        .context("no skeleton to store the classification on")?;
    systems::store(&mut glb, host, &systems)?;
    finish(&mut glb, path, output, true)
}

pub fn info(path: &Path) -> Result<()> {
    let glb = Glb::read(path)?;
    let scene = Scene::new(&glb.json)?;
    let skeleton = Skeleton::new(&glb, &scene);
    say!("{} joints", skeleton.joints.len());
    say!(
        "{}",
        skeleton::format_hierarchy(&skeleton.bones_with_parents(&scene))
    );
    match systems::load(&glb, &skeleton.names(&scene)) {
        Some(systems) => say!("\nClassified as {}", systems::summary(&systems)),
        None => say!("\nNot classified"),
    }
    let clips = animation::read_clips(&glb)?;
    say!("\n{} clips", clips.len());
    for clip in &clips {
        let times = clip.times();
        let duration = times
            .last()
            .zip(times.first())
            .map(|(last, first)| last - first)
            .unwrap_or(0.0);
        say!(
            "  {} ({duration:.2}s, {} keys, {} channels)",
            clip.name,
            times.len(),
            clip.channels.len()
        );
    }
    Ok(())
}

struct Side<'a> {
    scene: &'a Scene,
    skeleton: &'a Skeleton,
    systems: &'a [System],
    rig: &'a Rig<'a>,
}

impl Side<'_> {
    fn find(&self, name: &str) -> Option<usize> {
        self.skeleton.find(self.scene, name)
    }

    fn pelvis(&self) -> Option<usize> {
        self.systems.iter().find_map(|system| match system {
            System::Spine {
                pelvis: Some(pelvis),
                ..
            } => self.find(pelvis),
            _ => None,
        })
    }

    fn feet(&self) -> Vec<usize> {
        systems::roles(self.systems)
            .into_iter()
            .filter(|(role, _)| role.ends_with(".foot") || role.ends_with(".toe"))
            .flat_map(|(_, bones)| bones)
            .filter_map(|bone| self.find(&bone))
            .collect()
    }
}

fn build_mapping(source: &Side, destination: &Side) -> Result<(Mapping, f32)> {
    let mut mapping = Mapping::default();
    for (source_bone, destination_bone) in systems::pair(source.systems, destination.systems) {
        match (
            source.find(&source_bone),
            destination.find(&destination_bone),
        ) {
            (Some(source_node), Some(destination_node)) => {
                mapping.rotations.push((source_node, destination_node))
            }
            _ => eprintln!(
                "warning: cannot find the bones of pair {source_bone} -> {destination_bone}"
            ),
        }
    }
    let (Some(source_pelvis), Some(destination_pelvis)) = (source.pelvis(), destination.pelvis())
    else {
        bail!("both rigs need a classified pelvis to retarget");
    };
    mapping
        .translations
        .push((source_pelvis, destination_pelvis));
    let source_root = source.skeleton.root_above(source.scene, source_pelvis);
    let destination_root = destination
        .skeleton
        .root_above(destination.scene, destination_pelvis);
    if let (Some(source_root), Some(destination_root)) = (source_root, destination_root) {
        mapping.rotations.push((source_root, destination_root));
        mapping.translations.push((source_root, destination_root));
    }
    let source_height = source.rig.hip_height(source_pelvis, &source.feet());
    let destination_height = destination
        .rig
        .hip_height(destination_pelvis, &destination.feet());
    let scale = if source_height > 1e-6 && destination_height > 1e-6 {
        destination_height / source_height
    } else {
        1.0
    };
    Ok((mapping, scale))
}

/// One value for both files, or one per file.
fn per_file(values: &[String]) -> Option<(&str, &str)> {
    let first = values.first()?.as_str();
    Some((first, values.get(1).map(String::as_str).unwrap_or(first)))
}

fn skeleton_of(glb: &Glb, scene: &Scene, path: &Path) -> Result<Skeleton> {
    let skeleton = Skeleton::new(glb, scene);
    if skeleton.joints.is_empty() {
        bail!("{} has no skeleton", path.display());
    }
    Ok(skeleton)
}

/// The labelled clip's first frame, or the nodes' default transforms without a label.
fn reference_pose(
    clips: &[Clip],
    scene: &Scene,
    label: Option<&str>,
    path: &Path,
) -> Result<Vec<Transform>> {
    let Some(label) = label else {
        return Ok(scene.defaults.clone());
    };
    let clip = clips.iter().find(|clip| clip.name == label).with_context(|| {
        format!(
            "{} has no '{label}' clip to use as the reference pose. Create one with `retarjet pose {} --name {label}`.",
            path.display(),
            path.display()
        )
    })?;
    Ok(clip.pose(&scene.defaults, clip.start()))
}

fn systems_for(
    glb: &Glb,
    scene: &Scene,
    skeleton: &Skeleton,
    model: &str,
    path: &Path,
) -> Result<(Vec<System>, bool)> {
    if let Some(systems) = systems::load(glb, &skeleton.names(scene)) {
        return Ok((systems, false));
    }
    eprintln!("Classifying the rig in {} with {model}...", path.display());
    let systems = classify::classify(&skeleton.bones_with_parents(scene), model)?;
    say!(
        "{} classified as {}",
        path.display(),
        systems::summary(&systems)
    );
    Ok((systems, true))
}

/// Add a clip under the overwrite rules, reporting what happened.
fn place(
    glb: &mut Glb,
    existing: &mut Vec<String>,
    name: &str,
    channels: &[Channel],
    overwrite: bool,
) -> Result<bool> {
    if channels.is_empty() {
        say!("Skipped '{name}' (no channel maps onto the destination)");
        return Ok(false);
    }
    let exists = existing.iter().any(|existing_name| existing_name == name);
    if exists && !overwrite {
        say!("Skipped '{name}' (already exists, pass --overwrite to replace it)");
        return Ok(false);
    }
    if exists {
        animation::remove_clip(glb, name);
    } else {
        existing.push(name.to_owned());
    }
    animation::add_clip(glb, name, channels)?;
    say!("{} '{name}'", if exists { "Replaced" } else { "Added" });
    Ok(true)
}

fn finish(glb: &mut Glb, destination: &Path, output: Option<&Path>, changed: bool) -> Result<()> {
    if !changed && output.is_none() {
        say!("Nothing to write");
        return Ok(());
    }
    let path = output.unwrap_or(destination);
    glb.write(path)?;
    say!("Wrote {}", path.display());
    Ok(())
}

fn morph_target_count(glb: &Glb, node: usize) -> Option<usize> {
    let mesh = as_usize(glb.array("nodes").get(node)?.get("mesh"))?;
    let targets = glb
        .array("meshes")
        .get(mesh)?
        .pointer("/primitives/0/targets")?
        .as_array()?;
    Some(targets.len())
}

fn preview(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
    let more = names.len().saturating_sub(shown.len());
    if more > 0 {
        format!("{} and {more} more", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

/// Finds nodes by exact name, then ignoring namespaces like `mixamorig:` and case.
struct NodeLookup {
    exact: HashMap<String, usize>,
    loose: HashMap<String, usize>,
}

impl NodeLookup {
    fn new(scene: &Scene) -> Self {
        let mut exact = HashMap::new();
        let mut loose = HashMap::new();
        for (index, name) in scene.names.iter().enumerate() {
            exact.entry(name.clone()).or_insert(index);
            loose.entry(loose_name(name)).or_insert(index);
        }
        Self { exact, loose }
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.exact
            .get(name)
            .or_else(|| self.loose.get(&loose_name(name)))
            .copied()
    }
}

fn loose_name(name: &str) -> String {
    name.rsplit([':', '|'])
        .next()
        .unwrap_or(name)
        .to_lowercase()
}
