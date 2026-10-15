//! The CLI commands.

use crate::animation::{self, Clip};
use crate::cache;
use crate::classify;
use crate::glb::{Cleanup, Glb};
use crate::retarget::{self, Mapping, Rig};
use crate::scene::{Scene, Transform};
use crate::skeleton::{self, Skeleton};
use crate::systems::{self, System};
use anyhow::{Context, Result, bail};
use std::path::Path;

/// Report progress on stdout without failing when the reader has gone away, so the file still gets written.
macro_rules! say {
    ($($argument:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), $($argument)*);
    }};
}

/// What happens to the animations the destination already has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// Remove them, so the result only has the source's animations
    #[default]
    Replace,
    /// Keep them, and skip source animations whose name is taken
    Add,
    /// Keep them, and overwrite the ones a source animation shares a name with
    Update,
}

pub struct RetargetOptions<'a> {
    pub poses: &'a [String],
    pub facings: &'a [String],
    pub mode: Mode,
    pub output: Option<&'a Path>,
    pub model: &'a str,
}

pub fn retarget(
    source_path: &Path,
    destination_path: &Path,
    options: &RetargetOptions,
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

    let source_systems = systems_for(&source_scene, &source_skeleton, options.model, source_path)?;
    let destination_systems = systems_for(
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
    let existing = animation::clip_names(&destination);
    let mut removals: Vec<String> = match options.mode {
        Mode::Replace => existing
            .iter()
            .filter(|name| Some(name.as_str()) != destination_pose)
            .cloned()
            .collect(),
        Mode::Add | Mode::Update => Vec::new(),
    };
    let mut additions = Vec::new();
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
        if channels.is_empty() {
            say!(
                "Skipped '{}' (no channel maps onto the destination)",
                clip.name
            );
            continue;
        }
        let replaced = match options.mode {
            Mode::Replace => false,
            Mode::Add | Mode::Update => {
                let exists = existing.contains(&clip.name);
                if exists && options.mode == Mode::Add {
                    say!(
                        "Skipped '{}' (already exists, use --mode update to overwrite it)",
                        clip.name
                    );
                    continue;
                }
                if exists {
                    removals.push(clip.name.clone());
                }
                exists
            }
        };
        additions.push((&clip.name, channels, replaced));
    }
    if options.mode == Mode::Replace {
        for name in &removals {
            say!("Removed '{name}'");
        }
    }
    changed |= remove(&mut destination, &removals)?;
    for (name, channels, replaced) in &additions {
        animation::add_clip(&mut destination, name, channels)?;
        say!("{} '{name}'", if *replaced { "Replaced" } else { "Added" });
        changed = true;
    }
    finish(&mut destination, destination_path, options.output, changed)
}

pub fn classify(path: &Path, force: bool, model: &str) -> Result<()> {
    let glb = Glb::read(path)?;
    let scene = Scene::new(&glb.json)?;
    let skeleton = skeleton_of(&glb, &scene, path)?;
    let bones = skeleton.bones_with_parents(&scene);
    let directory = cache::directory().context("no cache directory on this system")?;
    if !force && let Some(systems) = cache::load_from(&directory, &bones) {
        say!(
            "{} (cached, pass --force to classify again)",
            systems::summary(&systems)
        );
        say!("{}", cache::path_in(&directory, &bones).display());
        return Ok(());
    }
    classify_and_cache(&bones, model, path)?;
    say!("{}", cache::path_in(&directory, &bones).display());
    Ok(())
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
    match cache::load(&skeleton.bones_with_parents(&scene)) {
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
            "{} has no '{label}' clip to use as the reference pose. Add a single frame clip with that name.",
            path.display()
        )
    })?;
    Ok(clip.pose(&scene.defaults, clip.start()))
}

/// The cached classification, or a fresh one that gets cached for next time.
fn systems_for(
    scene: &Scene,
    skeleton: &Skeleton,
    model: &str,
    path: &Path,
) -> Result<Vec<System>> {
    let bones = skeleton.bones_with_parents(scene);
    match cache::load(&bones) {
        Some(systems) => Ok(systems),
        None => classify_and_cache(&bones, model, path),
    }
}

fn classify_and_cache(bones: &cache::Bones, model: &str, path: &Path) -> Result<Vec<System>> {
    eprintln!("Classifying the rig in {} with {model}...", path.display());
    let systems = classify::classify(bones, model)?;
    say!(
        "{} classified as {}",
        path.display(),
        systems::summary(&systems)
    );
    if let Err(error) = cache::store(bones, &systems) {
        eprintln!("warning: cannot cache the classification ({error:#})");
    }
    Ok(systems)
}

/// Remove clips and their data, warning when the data has to stay.
fn remove(glb: &mut Glb, names: &[String]) -> Result<bool> {
    if names.is_empty() {
        return Ok(false);
    }
    if let Cleanup::Skipped(extension) = animation::remove_clips(glb, names)? {
        eprintln!("warning: the removed clips' data stays in the file because it uses {extension}");
    }
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
