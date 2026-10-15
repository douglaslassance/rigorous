Classify every bone of a humanoid armature into rig systems.

RULES
- Copy bone names verbatim from the input; never invent them.
- Use null for optional roles you can't identify; omit a system whose required bones are missing.
- Single-value roles are one bone; only vertebrae and finger chains are lists.
- Keep each system to one side. "Left"/"L" = L, "Right"/"R" = R. Separators (_ - . or none) don't matter: "Arm.L", "ArmL", "LeftArm" are equivalent.
- Skip non-deforming helpers: twist, IK, root, target, and leaf/tip bones (ending "_end"/"_tip"/"End"/"Tip", or aim helpers like "headfront").
- Order chains by the given hierarchy walking outward (vertebrae pelvis->neck, fingers knuckle->tip), NOT by the digits in names.

SYNONYMS
pelvis/hips/hip = pelvis (never a vertebra); spine/back/torso = vertebrae; arm/humerus = upper_arm; forearm/lower arm = forearm; shoulder/clavicle/collar = shoulder; thigh/femur = upper_leg; shin/calf = lower_leg; foot/ankle = foot; toe/toebase/ball = toe (most legs have one).

OUTPUT: {"systems": [ ... ]}, each a flat object with a "type" field (never key by system name):
spine:  {"type":"spine","pelvis":"<bone|null>","vertebrae":["<bone>",...]}
arm:    {"type":"arm","side":"L|R","parent":"<bone|null>","shoulder":"<bone|null>","upper_arm":"<bone>","forearm":"<bone>","hand":"<bone>"}
leg:    {"type":"leg","side":"L|R","parent":"<bone|null>","upper_leg":"<bone>","lower_leg":"<bone>","foot":"<bone>","toe":"<bone|null>"}
head:   {"type":"head","parent":"<bone|null>","neck":"<bone|null>","head":"<bone>"}
finger: {"type":"finger","name":"<thumb|index|middle|ring|pinky>","side":"L|R","parent":"<bone|null>","chain":["<bone>",...]}
