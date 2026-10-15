# retarjet

Combine and retarget animations between GLB files from the command line.

## Install

```bash
cargo install --path .
```

Classifying a rig the first time needs [Ollama](https://ollama.com) running locally with the model pulled.

```bash
ollama pull qwen2.5:32b
```

## Commands

```
retarjet transfer <source> <destination> [--pose SOURCE [DESTINATION]] [--facing SOURCE [DESTINATION]] [--overwrite] [-o OUTPUT]
retarjet combine <source> <destination> [--overwrite] [-o OUTPUT]
retarjet pose <file> [--name t-pose] [--from bind|default] [--overwrite] [-o OUTPUT]
retarjet classify <file> [--force] [-o OUTPUT]
retarjet info <file>
```

Every command that writes updates the destination in place unless `-o` is given. A clip whose name already exists in the destination is skipped unless `--overwrite` is passed. Put options after the file arguments.

- `transfer` retargets every clip of the source onto the destination's rig, which can have different bone names, hierarchy, proportions and bone axes.
- `combine` copies clips between two files that share the same rig. It refuses rigs whose hierarchies differ and points you to `transfer`.
- `pose` writes a single frame clip to use as a reference pose, captured from the skin's bind pose or the nodes' default transforms.
- `classify` maps the rig's bones to anatomical roles and stores the result in the file.
- `info` prints the skeleton, the stored classification and the clips.

## How transfer works

### Reference poses

Motion is transferred relative to a reference pose on each rig. Without `--pose`, that is each file's default pose (the nodes' rest transforms), which works when both characters rest in the same stance.

When they don't (one in a T-pose and the other in an A-pose, for example), give each file a clip that shows its character in a shared stance and name it with `--pose`. One name applies to both files, as in `--pose t-pose`. Pass two when they differ, as in `--pose TPose t-pose`. `retarjet pose` writes such a clip from the bind pose, which you can then adjust in your DCC. Named reference clips are never transferred.

### Bone mapping

Bones are classified into rig systems (spine, arms, legs, head and fingers) with the same prompts and schema as the [yello](https://github.com/playsthetic/yello) Blender extension. The classification is stored as `extras.yello_systems` on the armature node, so it runs once per character and can be edited by hand. Two rigs are then paired role by role. Chains of different lengths, such as a three bone spine onto a two bone spine, are spread end to end.

### Retargeting

For every key, each mapped bone's world rotation change from the source reference pose is applied to the target bone's reference pose. Working in world space makes bone axis conventions and armature transforms irrelevant. Hips (and a root bone above them, when both rigs have one) also get their world displacement, scaled by the ratio of hip heights. Other bones keep their own lengths.

### Facing

`--facing` gives the direction each character faces in its file, one of `+x`, `-x`, `+z` or `-z`. The default `+z` is the glTF convention. Pass two values when the files disagree, as in `--facing +z -z`.

## Limitations

- Classification covers humanoids only.
- `transfer` writes rotations plus hips and root translation. Scale keys and morph target weights are not transferred.
- Replacing a clip leaves its old data unreferenced in the binary chunk.
- Sparse accessors, meshopt compression and external buffers are not supported yet.

## Development

```bash
cargo test
```

The test fixtures in `tests/fixtures` are characters stripped to their nodes, skins and animations, with a `t-pose` clip and a stored classification. Regenerate one with the following command, then run `retarjet pose` and `retarjet classify` on it.

```bash
python3 -I tests/fixtures/strip.py SOURCE.glb tests/fixtures/NAME.glb
```
