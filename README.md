# retarjet

Retarget animations between GLB files from the command line.

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
retarjet retarget <source> <destination> [--mode replace|add|update] [--pose SOURCE [DESTINATION]] [--facing SOURCE [DESTINATION]] [-o OUTPUT]
retarjet pose <file> [--name t-pose] [--from bind|default] [--overwrite] [-o OUTPUT]
retarjet classify <file> [--force]
retarjet info <file>
```

Commands that write update the file in place unless `-o` is given. Put options after the file arguments.

- `retarget` retargets every clip of the source onto the destination's rig, which can have different bone names, hierarchy, proportions and bone axes. `--mode` decides what happens to the destination's own animations.
  - `replace` (the default) removes them, so the result only has the source's.
  - `add` keeps them and skips source clips whose name is taken.
  - `update` keeps them and overwrites the ones a source clip shares a name with.
- `pose` writes a single frame clip to use as a reference pose, captured from the skin's bind pose or the nodes' default transforms.
- `classify` maps the rig's bones to anatomical roles, caches the result and prints where the cache file is.
- `info` prints the skeleton, the cached classification and the clips.

retarjet only ever writes animation clips into a GLB.

## How retargeting works

### Reference poses

Motion is transferred relative to a reference pose on each rig. Without `--pose`, that is each file's default pose (the nodes' rest transforms), which works when both characters rest in the same stance.

When they don't (one in a T-pose and the other in an A-pose, for example), give each file a clip that shows its character in a shared stance and name it with `--pose`. One name applies to both files, as in `--pose t-pose`. Pass two when they differ, as in `--pose TPose t-pose`. `retarjet pose` writes such a clip from the bind pose, which you can then adjust in your DCC. Named reference clips are never retargeted, replaced or overwritten.

### Bone mapping

Bones are classified into rig systems (spine, arms, legs, head and fingers) with the same prompts and schema as the [yello](https://github.com/playsthetic/yello) Blender extension. Two rigs are then paired role by role. Chains of different lengths, such as a three bone spine onto a two bone spine, are spread end to end.

Classifications are cached on your machine (`~/Library/Caches/retarjet` on macOS, `~/.cache/retarjet` on Linux, or `RETARJET_CACHE`), keyed by the skeleton's bone names and hierarchy. Each rig is classified once, and renaming or reparenting a bone classifies it again. Edit a cache file to fix a wrong mapping.

### Retargeting

For every key, each mapped bone's world rotation change from the source reference pose is applied to the target bone's reference pose. Working in world space makes bone axis conventions and armature transforms irrelevant. Hips (and a root bone above them, when both rigs have one) also get their world displacement, scaled by the ratio of hip heights. Other bones keep their own lengths.

### Facing

`--facing` gives the direction each character faces in its file, one of `+x`, `-x`, `+z` or `-z`. The default `+z` is the glTF convention. Pass two values when the files disagree, as in `--facing +z -z`.

## Limitations

- Classification covers humanoids only.
- Retargeted clips hold rotations plus hips and root translation. Scale keys and morph target weights are not carried over.
- Removed clips leave their data in the file when it uses an extension whose references retarjet cannot follow.
- Sparse accessors, meshopt compression and external buffers are not supported yet.

## Development

```bash
cargo test
```

The test fixtures in `tests/fixtures` are characters stripped to their nodes, skins and animations, with a `t-pose` clip. Each has a `.classification.json` next to it that the tests load into a private cache, so they never need Ollama. Regenerate a fixture with the following command, run `retarjet pose` on it, then copy the file `retarjet classify` prints next to it.

```bash
python3 -I tests/fixtures/strip.py SOURCE.glb tests/fixtures/NAME.glb
```
