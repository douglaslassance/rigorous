//! End to end tests on the goblin fixtures, which carry a `t-pose` clip and a classification next to them.

use glam::{Mat4, Quat};
use retarjet::animation::{self, Clip};
use retarjet::cache;
use retarjet::glb::Glb;
use retarjet::scene::{Scene, rotation_of};
use retarjet::skeleton::Skeleton;
use retarjet::systems;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Largest angle, in radians, allowed between expected and retargeted world rotations.
const ANGLE_TOLERANCE: f32 = 2e-3;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

struct Workspace {
    paths: Vec<PathBuf>,
    cache: PathBuf,
}

/// Copy fixtures into a directory owned by one test, with their classifications in its own cache.
fn workspace(test: &str, names: &[&str]) -> Workspace {
    let directory = std::env::temp_dir().join(format!("retarjet-{}-{test}", std::process::id()));
    let cache = directory.join("cache");
    std::fs::create_dir_all(&directory).unwrap();
    let paths = names
        .iter()
        .map(|name| {
            let path = directory.join(name);
            std::fs::copy(fixture(name), &path).unwrap();
            let glb = Glb::read(&path).unwrap();
            let scene = Scene::new(&glb.json).unwrap();
            let skeleton = Skeleton::new(&glb, &scene);
            let classification = fixture(name).with_extension("classification.json");
            let data =
                serde_json::from_str(&std::fs::read_to_string(classification).unwrap()).unwrap();
            let systems = systems::parse(&data, &skeleton.names(&scene));
            cache::store_in(&cache, &skeleton.bones_with_parents(&scene), &systems).unwrap();
            path
        })
        .collect();
    Workspace { paths, cache }
}

impl Workspace {
    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_retarjet"))
            .env(cache::DIRECTORY_VARIABLE, &self.cache)
            .args(arguments)
            .output()
            .unwrap()
    }

    fn character(&self, path: &Path) -> Character {
        Character::load(path, &self.cache)
    }
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

struct Character {
    scene: Scene,
    clips: Vec<Clip>,
    skeleton: Skeleton,
    systems: Vec<systems::System>,
}

impl Character {
    fn load(path: &Path, cache: &Path) -> Self {
        let glb = Glb::read(path).unwrap();
        let scene = Scene::new(&glb.json).unwrap();
        let skeleton = Skeleton::new(&glb, &scene);
        let systems = cache::load_from(cache, &skeleton.bones_with_parents(&scene))
            .expect("the workspace caches every fixture's classification");
        let clips = animation::read_clips(&glb).unwrap();
        Self {
            scene,
            clips,
            skeleton,
            systems,
        }
    }

    fn clip(&self, name: &str) -> &Clip {
        self.clips
            .iter()
            .find(|clip| clip.name == name)
            .unwrap_or_else(|| panic!("no clip named {name}"))
    }

    fn world(&self, clip: &str, time: f32) -> Vec<Mat4> {
        self.scene
            .world_matrices(&self.clip(clip).pose(&self.scene.defaults, time))
    }

    /// World matrices of the named pose clip, or of the default pose without a name.
    fn reference(&self, pose: Option<&str>) -> Vec<Mat4> {
        match pose {
            Some(pose) => self.world(pose, 0.0),
            None => self.scene.world_matrices(&self.scene.defaults),
        }
    }

    fn node(&self, name: &str) -> usize {
        self.skeleton.find(&self.scene, name).unwrap()
    }
}

fn rotation(matrix: Mat4) -> Quat {
    rotation_of(matrix)
}

/// Every mapped bone must turn in world space exactly as its source did from the reference pose.
fn assert_world_motion_matches(
    source: &Character,
    destination: &Character,
    clip: &str,
    pose: Option<&str>,
    facing: Quat,
) {
    let source_reference = source.reference(pose);
    let destination_reference = destination.reference(pose);
    let pairs: Vec<(usize, usize)> = systems::pair(&source.systems, &destination.systems)
        .iter()
        .map(|(source_bone, destination_bone)| {
            (source.node(source_bone), destination.node(destination_bone))
        })
        .collect();
    assert!(pairs.len() >= 20, "only {} bones mapped", pairs.len());
    for time in source.clip(clip).times() {
        let source_world = source.world(clip, time);
        let destination_world = destination.world(clip, time);
        for &(source_node, destination_node) in &pairs {
            let source_delta = rotation(source_world[source_node])
                * rotation(source_reference[source_node]).inverse();
            let expected = facing * source_delta * facing.inverse();
            let actual = rotation(destination_world[destination_node])
                * rotation(destination_reference[destination_node]).inverse();
            let angle = expected.angle_between(actual);
            assert!(
                angle < ANGLE_TOLERANCE,
                "{} at {time:.3}s is off by {angle:.4} radians",
                destination.scene.names[destination_node]
            );
        }
    }
}

#[test]
fn combine_matches_world_motion_between_rigs() {
    let workspace = workspace("between_rigs", &["blue_goblin.glb", "green_goblin.glb"]);
    let paths = &workspace.paths;
    let output = workspace.run(&["combine", text(&paths[0]), text(&paths[1])]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Added 'Squat'"));
    assert_world_motion_matches(
        &workspace.character(&paths[0]),
        &workspace.character(&paths[1]),
        "Squat",
        None,
        Quat::IDENTITY,
    );
}

#[test]
fn combine_matches_world_motion_from_a_partially_keyed_rig() {
    let workspace = workspace("partially_keyed", &["red_goblin.glb", "blue_goblin.glb"]);
    let paths = &workspace.paths;
    let output = workspace.run(&[
        "combine",
        text(&paths[0]),
        text(&paths[1]),
        "--pose",
        "t-pose",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_world_motion_matches(
        &workspace.character(&paths[0]),
        &workspace.character(&paths[1]),
        "Lift_Left_Leg",
        Some("t-pose"),
        Quat::IDENTITY,
    );
}

#[test]
fn combine_turns_motion_to_the_destination_facing() {
    let workspace = workspace("facing", &["blue_goblin.glb", "red_goblin.glb"]);
    let paths = &workspace.paths;
    let output = workspace.run(&[
        "combine",
        text(&paths[0]),
        text(&paths[1]),
        "--facing",
        "+z",
        "-z",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let half_turn = Quat::from_rotation_y(std::f32::consts::PI);
    assert_world_motion_matches(
        &workspace.character(&paths[0]),
        &workspace.character(&paths[1]),
        "Squat",
        None,
        half_turn,
    );
}

#[test]
fn combine_scales_root_motion_by_hip_height() {
    let workspace = workspace("root_motion", &["blue_goblin.glb", "green_goblin.glb"]);
    let paths = &workspace.paths;
    let output = stdout(&workspace.run(&["combine", text(&paths[0]), text(&paths[1])]));
    let mapped = output
        .lines()
        .find(|line| line.starts_with("Mapped"))
        .unwrap();
    let scale: f32 = mapped.rsplit(' ').next().unwrap().parse().unwrap();
    let source = workspace.character(&paths[0]);
    let destination = workspace.character(&paths[1]);
    let (source_hips, destination_hips) = (source.node("Hips"), destination.node("Hips"));
    let source_reference = source.reference(None)[source_hips].w_axis.truncate();
    let destination_reference = destination.reference(None)[destination_hips]
        .w_axis
        .truncate();
    for time in source.clip("Squat").times() {
        let source_offset =
            source.world("Squat", time)[source_hips].w_axis.truncate() - source_reference;
        let destination_offset = destination.world("Squat", time)[destination_hips]
            .w_axis
            .truncate()
            - destination_reference;
        assert!(
            destination_offset.abs_diff_eq(source_offset * scale, 1e-3),
            "hips at {time:.3}s"
        );
    }
}

#[test]
fn combine_skips_existing_clips_unless_overwriting() {
    let workspace = workspace("overwrite", &["blue_goblin.glb", "red_goblin.glb"]);
    let paths = &workspace.paths;
    let (source, destination) = (text(&paths[0]), text(&paths[1]));
    let pose = ["--pose", "t-pose"];
    assert!(
        workspace
            .run(&[&["combine", source, destination][..], &pose].concat())
            .status
            .success()
    );
    let count = animation::clip_names(&Glb::read(&paths[1]).unwrap()).len();
    let again = stdout(&workspace.run(&[&["combine", source, destination][..], &pose].concat()));
    assert!(again.contains("Skipped 'Squat' (already exists"));
    assert!(again.contains("Nothing to write"));
    let replaced = stdout(
        &workspace.run(&[&["combine", source, destination, "--overwrite"][..], &pose].concat()),
    );
    assert!(replaced.contains("Replaced 'Squat'"));
    let names = animation::clip_names(&Glb::read(&paths[1]).unwrap());
    assert_eq!(names.len(), count);
    assert_eq!(names.iter().filter(|name| *name == "t-pose").count(), 1);
    assert_no_unused_data(&paths[1]);
}

#[test]
fn combine_without_pose_treats_pose_clips_as_regular_clips() {
    let workspace = workspace("no_pose", &["blue_goblin.glb", "red_goblin.glb"]);
    let paths = &workspace.paths;
    let output = stdout(&workspace.run(&["combine", text(&paths[0]), text(&paths[1])]));
    assert!(output.contains("Skipped 't-pose' (already exists"));
    assert!(output.contains("Added 'Squat'"));
}

#[test]
fn combine_leaves_the_rest_of_the_destination_untouched() {
    let workspace = workspace("untouched", &["blue_goblin.glb", "green_goblin.glb"]);
    let paths = &workspace.paths;
    let before = Glb::read(&paths[1]).unwrap();
    assert!(
        workspace
            .run(&["combine", text(&paths[0]), text(&paths[1])])
            .status
            .success()
    );
    let after = Glb::read(&paths[1]).unwrap();
    assert_eq!(before.json["nodes"], after.json["nodes"]);
    assert_eq!(before.json["skins"], after.json["skins"]);
    assert_eq!(before.bin[..], after.bin[..before.bin.len()]);
}

#[test]
fn combine_explains_how_to_add_a_missing_reference_pose() {
    let workspace = workspace("missing_pose", &["blue_goblin.glb", "green_goblin.glb"]);
    let paths = &workspace.paths;
    let output = workspace.run(&[
        "combine",
        text(&paths[0]),
        text(&paths[1]),
        "--pose",
        "a-pose",
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--name a-pose"));
}

/// Every accessor and buffer view in the file is still used by something.
fn assert_no_unused_data(path: &Path) {
    let glb = Glb::read(path).unwrap();
    let mut accessors = std::collections::BTreeSet::new();
    for skin in glb.array("skins") {
        accessors.extend(skin["inverseBindMatrices"].as_u64());
    }
    for animation in glb.array("animations") {
        for sampler in animation["samplers"].as_array().unwrap() {
            accessors.extend(sampler["input"].as_u64());
            accessors.extend(sampler["output"].as_u64());
        }
    }
    assert_eq!(accessors.len(), glb.array("accessors").len());
    assert_eq!(glb.array("bufferViews").len(), glb.array("accessors").len());
}

#[test]
fn replace_keeps_only_the_source_animations() {
    let workspace = workspace("replace", &["blue_goblin.glb", "red_goblin.glb"]);
    let paths = &workspace.paths;
    let output = workspace.run(&[
        "replace",
        text(&paths[0]),
        text(&paths[1]),
        "--pose",
        "t-pose",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Removed 'Lift_Left_Leg'"));
    let names = animation::clip_names(&Glb::read(&paths[1]).unwrap());
    assert_eq!(names, ["t-pose", "Idle_Pop_Loop", "Squat"]);
    assert_no_unused_data(&paths[1]);
    assert_world_motion_matches(
        &workspace.character(&paths[0]),
        &workspace.character(&paths[1]),
        "Squat",
        Some("t-pose"),
        Quat::IDENTITY,
    );
}

#[test]
fn classify_reads_the_cache_and_leaves_the_file_alone() {
    let workspace = workspace("classify", &["red_goblin.glb"]);
    let before = std::fs::read(&workspace.paths[0]).unwrap();
    let output = stdout(&workspace.run(&["classify", text(&workspace.paths[0])]));
    assert!(output.contains("(cached, pass --force"));
    assert!(output.contains(text(&workspace.cache)));
    assert_eq!(std::fs::read(&workspace.paths[0]).unwrap(), before);
}

#[test]
fn pose_replaces_an_existing_clip_only_when_asked() {
    let workspace = workspace("pose", &["blue_goblin.glb"]);
    let paths = &workspace.paths;
    assert!(stdout(&workspace.run(&["pose", text(&paths[0])])).contains("Skipped 't-pose'"));
    assert!(
        stdout(&workspace.run(&["pose", text(&paths[0]), "--overwrite"]))
            .contains("Replaced 't-pose'")
    );
}

#[test]
fn info_lists_skeleton_classification_and_clips() {
    let workspace = workspace("info", &["green_goblin.glb"]);
    let printed = stdout(&workspace.run(&["info", text(&workspace.paths[0])]));
    assert!(
        printed.contains("24 joints"),
        "the animated Spine node between skin joints counts"
    );
    assert!(printed.contains("Classified as spine(3)"));
    assert!(printed.contains("JumpingJack_Phase_1"));
}
