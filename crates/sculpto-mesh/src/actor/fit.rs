//! Rest-frame fit of the extremity chains to the flesh they dominate. @req DECL0013
//!
//! The shipped actors carry the SSE rest frames wherever the skeleton put them:
//! the finger chains sit 8–20° off the axis of the flesh they weight, Emily's
//! ulnar chains sit laterally on the neighbouring digit (only 57.0 % of her
//! pinky flesh is geometrically closest to the pinky chain), the toe pivot rests
//! on the sole instead of inside the forefoot, and the head pivot sits at the
//! occiput instead of the skull base. None of that is a weight defect: the pivot
//! is in the wrong place, and no weight map moves a pivot.
//!
//! The pivot of every extremity joint is measured from the mesh, not estimated:
//! the vertices whose weights **blend the two bones the joint separates** form
//! the articulation band, and the band's centroid is the pivot. That band is
//! where the bind itself says the surface changes bone, so it is the same
//! correspondence the skin already carries:
//!
//! * knuckle ≈ centroid of the Hand↔Finger10 blend band;
//! * ankle ≈ centroid of the Calf↔Foot band; ball ≈ centroid of Foot↔Toe0;
//! * knee ≈ centroid of the Thigh↔Calf band; skull base ≈ Spine2↔Head band.
//!
//! A joint whose band is too thin to measure falls back, in this order, to the
//! boundary between the two bones' dominated flesh along the chain's own flesh
//! line, then to the authored placement projected onto that line. Which source
//! placed each pivot is reported per joint, so an unmeasured pivot stays
//! visible instead of looking fitted.
//!
//! The rest orientation is corrected by the minimal arc that takes the joint's
//! authored bone axis onto the fitted segment, so the authored roll survives and
//! a positive local-X rotation keeps meaning what the rig authored. The angle
//! between that axis and the palm/sole plane is reported as evidence.
//!
//! Rest geometry is untouched: every joint's inverse bind matrix is recomputed
//! as `inverse(new_world) * skin_base`, a constant the fit proves over the whole
//! skin first, so the same vertices sit in the same place and only pivots move.

use std::collections::BTreeMap;

use glam::{DMat3, DMat4, DQuat, DVec3};

use super::{Actor, ActorError, animation, geometry, invalid};

/// How a joint that has no measurable blend band is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthMode {
    /// The two bones' dominated flesh decides the boundary; proportions come
    /// from the mesh.
    Flesh,
    /// The authored segment length is kept and only the direction and the
    /// lateral offset are fitted.
    Preserve,
}

impl LengthMode {
    /// Parse the CLI spelling.
    ///
    /// # Errors
    /// A name that is neither `flesh` nor `preserve`.
    pub fn parse(value: &str) -> Result<Self, ActorError> {
        match value {
            "flesh" => Ok(Self::Flesh),
            "preserve" => Ok(Self::Preserve),
            _ => Err(invalid(
                "fit lengths",
                format!("expected flesh or preserve, found {value:?}"),
            )),
        }
    }

    /// Stable report spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flesh => "flesh",
            Self::Preserve => "preserve",
        }
    }
}

/// What a pivot measurement came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    /// Centroid of the two bones' blend band: the measured articulation.
    BlendBand,
    /// Boundary between the two bones' dominated flesh along the flesh line.
    FleshBoundary,
    /// The authored position, moved onto the flesh line only.
    AuthoredOnLine,
    /// The authored position, untouched.
    Authored,
}

impl Placed {
    /// Stable report spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BlendBand => "blend_band",
            Self::FleshBoundary => "flesh_boundary",
            Self::AuthoredOnLine => "authored_on_line",
            Self::Authored => "authored",
        }
    }

    fn measured(self) -> bool {
        matches!(self, Self::BlendBand | Self::FleshBoundary)
    }
}

/// The direction a positive local-X rotation moves a chain's tip toward, as the
/// rig authors it; the fit keeps that roll and reports the agreement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flexion {
    /// Fingers: curl toward the palm.
    Palmar,
    /// Foot and toes: lift toward the dorsum.
    Dorsal,
    /// Knee: swing the shin backward.
    Posterior,
    /// Neck: nod forward.
    Anterior,
}

impl Flexion {
    /// Stable report spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Palmar => "palmar",
            Self::Dorsal => "dorsal",
            Self::Posterior => "posterior",
            Self::Anterior => "anterior",
        }
    }
}

/// One fitted chain of joints that share a flesh region.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainFit {
    /// Report label, e.g. `L ring` or `L foot`.
    pub label: String,
    /// Chain class, for the report.
    pub kind: &'static str,
    pub flexion: Flexion,
    /// Unit direction the chain was fitted to.
    pub axis: [f64; 3],
    /// Flesh vertices after the span clip and the trims.
    pub flesh_vertices: usize,
    /// Chain direction against the flesh axis, before the fit, degrees.
    pub axis_before_degrees: f64,
    /// Chain direction against the flesh axis after the fit, degrees.
    pub axis_after_degrees: f64,
    /// Rotation of the root joint's local X against the flexion plane, before
    /// and after, degrees: 0 means the flexion axis lies in the palm/sole plane.
    pub flexion_axis_off_plane_before_degrees: f64,
    pub flexion_axis_off_plane_after_degrees: f64,
    pub joints: Vec<JointFit>,
}

/// One joint's before/after measurement inside a fitted chain.
#[derive(Clone, Debug, PartialEq)]
pub struct JointFit {
    pub name: String,
    pub node: usize,
    pub placed_by: Placed,
    /// Vertices whose weights blend the parent-side bone and this one.
    pub band_vertices: usize,
    /// Longest extent of that blend band: a band wider than the flesh it bounds
    /// is a gradual blend, not an articulation.
    pub band_span: f64,
    pub before_head: [f64; 3],
    pub after_head: [f64; 3],
    /// Distance from the flesh axis before the fit.
    pub perpendicular_before: f64,
    /// Distance from the flesh axis after the fit.
    pub perpendicular_after: f64,
    /// Distance from the joint to the centroid of the vertices it dominates.
    pub centroid_before: Option<f64>,
    pub centroid_after: Option<f64>,
    /// Segment that starts at this joint.
    pub segment_before: Option<f64>,
    pub segment_after: Option<f64>,
    /// Vertices this joint dominates on its own.
    pub dominated: usize,
    /// Whether this chain moved the joint; a joint another chain owns is a
    /// reference for the segment only.
    pub owned: bool,
}

/// A chain the fit had to leave alone, with the measured reason.
#[derive(Clone, Debug, PartialEq)]
pub struct UnfittedChain {
    pub label: String,
    pub reason: String,
    pub dominated: usize,
}

/// One palette joint's new rest frame, in the actor's own units.
#[derive(Clone, Debug, PartialEq)]
pub struct FittedFrame {
    pub node: usize,
    pub name: String,
    /// Rest position after the fit.
    pub translation: [f64; 3],
    /// Rest rotation after the fit, quaternion `x, y, z, w`.
    pub rotation: [f64; 4],
    /// Uniform local scale, unchanged from the actor.
    pub scale: f64,
    /// Local transform relative to the parent.
    pub local_translation: [f64; 3],
    pub local_rotation: [f64; 4],
    /// The joint's new inverse bind matrix, column-major.
    pub inverse_bind: [[f64; 4]; 4],
    /// Whether the fit moved this joint's rest world.
    pub moved: bool,
    /// Whether the joint's local transform must be rewritten: a joint the fit
    /// did not move keeps its world, so its local transform absorbs a fitted
    /// parent's change.
    pub rewritten: bool,
}

/// Every fitted chain plus the frames and binds the writer needs.
#[derive(Clone, Debug, PartialEq)]
pub struct FitOutcome {
    pub chains: Vec<ChainFit>,
    pub unfitted: Vec<UnfittedChain>,
    /// One entry per joint of the actor's skin, in skin order.
    pub frames: Vec<FittedFrame>,
    pub moved_joints: usize,
    /// Largest distance any rest pivot moved.
    pub max_pivot_shift: f64,
    /// Largest disagreement of `world * inverse_bind` across the skin before the
    /// fit, as a matrix element.
    pub skin_base_spread: f64,
    /// Unique sampled vertices.
    pub vertex_count: usize,
}

/// One chain's joints, the plane it bends in, and which joints it owns.
struct ChainSpec {
    label: String,
    kind: &'static str,
    flexion: Flexion,
    joints: Vec<usize>,
    /// `owned[position]` — a joint another chain owns is a length reference only.
    owned: Vec<bool>,
    /// Joint that supplies the flexion-plane normal (`None` uses the body axes).
    plane_from: Option<usize>,
}

/// One unique vertex's influence row, in world space.
#[derive(Clone)]
struct Vertex {
    position: DVec3,
    /// `(joint node, weight)` for every positive influence.
    influences: Vec<(usize, f64)>,
}

impl Vertex {
    fn weight(&self, node: usize) -> f64 {
        self.influences
            .iter()
            .find(|(joint, _)| *joint == node)
            .map(|(_, weight)| *weight)
            .unwrap_or(0.0)
    }

    fn dominant(&self) -> Option<usize> {
        self.influences
            .iter()
            .fold(
                None,
                |best: Option<(usize, f64)>, (node, weight)| match best {
                    Some((_, best_weight)) if best_weight >= *weight => best,
                    _ => Some((*node, *weight)),
                },
            )
            .map(|(node, _)| node)
    }
}

/// One joint's decided pivot.
struct PlacedJoint {
    node: usize,
    pivot: DVec3,
    placed_by: Placed,
    band_vertices: usize,
    band_span: f64,
}

/// Fit every extremity chain this actor exposes and return the new frames.
///
/// # Errors
/// A part selection or sampling refusal of the actor reader, a skin without
/// inverse bind matrices, or a skin whose `world * inverse_bind` is not one
/// constant matrix (the rewrite could not keep the rest geometry).
pub fn fit(actor: &Actor, parts: &[&str], lengths: LengthMode) -> Result<FitOutcome, ActorError> {
    let selected = geometry::select(&actor.document, parts)?;
    let worlds = animation::compose_worlds(actor, &actor.transforms)?;
    let binds = skin_matrices(actor, &worlds)?;
    let base = skin_base(&binds)?;
    let vertices = sampled_vertices(actor, &selected, &base)?;
    let names = node_names(actor);
    let specs = chain_specs(&names)?;
    let flexion = flexion_normals(&vertices, &worlds, &names, &specs);

    let mut pivots: BTreeMap<usize, DVec3> = BTreeMap::new();
    let mut chains = Vec::new();
    let mut unfitted = Vec::new();
    for spec in &specs {
        let reference = flexion
            .get(spec.label.as_str())
            .copied()
            .unwrap_or(DVec3::Z);
        let context = FitContext {
            names: &names,
            parents: &actor.parents,
            vertices: &vertices,
            worlds: &worlds,
            pivots: &pivots,
            lengths,
        };
        match fit_chain(&context, spec, reference) {
            Ok(Some((chain, placed, reference_axis))) => {
                for joint in &placed {
                    pivots.insert(joint.node, joint.pivot);
                }
                chains.push((chain, placed, reference_axis, reference));
            }
            Ok(None) => unfitted.push(UnfittedChain {
                label: spec.label.clone(),
                reason: "no joint of the chain dominates flesh and no blend band reaches it"
                    .to_owned(),
                dominated: 0,
            }),
            Err(reason) => unfitted.push(UnfittedChain {
                label: spec.label.clone(),
                reason,
                dominated: 0,
            }),
        }
    }

    // Frames: minimal-arc correction of the authored bone axis onto the fitted
    // segment, so the authored roll survives.
    let mut heads: BTreeMap<usize, DVec3> = BTreeMap::new();
    let mut rotations: BTreeMap<usize, DQuat> = BTreeMap::new();
    for (_, placed, _axis, _) in &chains {
        for (position, joint) in placed.iter().enumerate() {
            let next = placed.get(position + 1).map(|joint| joint.pivot);
            heads.insert(joint.node, joint.pivot);
            let Some(next) = next else {
                // A chain tip has no next segment: its pivot moves, its aim stays
                // as authored, because aiming the tip is a roll question.
                continue;
            };
            let direction = (next - joint.pivot).normalize_or_zero();
            let (_, authored_rotation, _) = worlds[joint.node].to_scale_rotation_translation();
            let authored_bone =
                (DMat3::from_quat(authored_rotation) * DVec3::Z).normalize_or(DVec3::Z);
            let correction = DQuat::from_rotation_arc(authored_bone, direction);
            rotations.insert(joint.node, (correction * authored_rotation).normalize());
        }
    }

    let frames = build_frames(actor, &names, &worlds, &binds, &base, &heads, &rotations)?;
    let moved_joints = frames.iter().filter(|frame| frame.moved).count();
    let max_pivot_shift = frames
        .iter()
        .map(|frame| {
            let rest = worlds[frame.node].transform_point3(DVec3::ZERO);
            rest.distance(DVec3::from_array(frame.translation))
        })
        .fold(0.0_f64, f64::max);

    let mut chains: Vec<ChainFit> = chains
        .into_iter()
        .map(|(mut chain, placed, _, reference)| {
            let flexed = chain
                .joints
                .first()
                .and_then(|joint| rotations.get(&joint.node))
                .map(|rotation| flexion_off_plane(*rotation, reference))
                .unwrap_or(0.0);
            chain.flexion_axis_off_plane_after_degrees = flexed;
            chain
                .joints
                .iter_mut()
                .zip(&placed)
                .for_each(|(joint, placed)| {
                    joint.placed_by = placed.placed_by;
                    joint.band_vertices = placed.band_vertices;
                });
            chain
        })
        .collect();
    chains.sort_by(|left, right| left.label.cmp(&right.label));

    Ok(FitOutcome {
        chains,
        unfitted,
        frames,
        moved_joints,
        max_pivot_shift,
        skin_base_spread: base.spread,
        vertex_count: vertices.len(),
    })
}

/// Diagonal of the actor's own bounds, the scale a residual is judged against.
///
/// # Errors
/// Any part selection or sampling refusal of the actor reader.
pub fn extent(actor: &Actor, parts: &[&str]) -> Result<f64, ActorError> {
    let selected = geometry::select(&actor.document, parts)?;
    let worlds = animation::compose_worlds(actor, &actor.transforms)?;
    let sample = geometry::sample(actor, &selected, &worlds)?;
    let mut low = DVec3::splat(f64::INFINITY);
    let mut high = DVec3::splat(f64::NEG_INFINITY);
    for point in &sample.vertices {
        let point = DVec3::from_array(*point);
        low = low.min(point);
        high = high.max(point);
    }
    Ok((high - low).length())
}

/// Angle, in degrees, between a joint's local X axis and the flexion plane the
/// palm or sole spans; 0 means the local X is the flexion axis.
fn flexion_off_plane(rotation: DQuat, plane_normal: DVec3) -> f64 {
    let axis = (DMat3::from_quat(rotation) * DVec3::X).normalize_or_zero();
    let off = 90.0
        - axis
            .dot(plane_normal)
            .abs()
            .clamp(0.0, 1.0)
            .acos()
            .to_degrees();
    off.abs()
}

/// The skin's one base matrix: `world * inverse_bind`, which must agree over the
/// whole skin or a rewritten frame would move the rest geometry.
pub(super) struct SkinBase {
    pub(super) matrix: DMat4,
    pub(super) spread: f64,
}

/// `world * inverse_bind` for every joint of the actor's skin.
pub(super) fn skin_matrices(
    actor: &Actor,
    worlds: &[DMat4],
) -> Result<Vec<(usize, DMat4)>, ActorError> {
    let node = actor
        .document
        .nodes()
        .find(|node| node.skin().is_some())
        .ok_or_else(|| invalid("actor fit", "the actor carries no skinned mesh"))?;
    let skin = node
        .skin()
        .ok_or_else(|| invalid(format!("node {}", node.index()), "missing skin"))?;
    let joints: Vec<usize> = skin.joints().map(|joint| joint.index()).collect();
    let products = geometry::joint_matrices(actor, skin, worlds)?;
    Ok(joints.into_iter().zip(products).collect())
}

pub(super) fn skin_base(binds: &[(usize, DMat4)]) -> Result<SkinBase, ActorError> {
    let first = binds
        .first()
        .map(|(_, product)| *product)
        .ok_or_else(|| invalid("actor fit", "the skin has no joints"))?;
    let inverse = first.inverse();
    let spread = binds
        .iter()
        .map(|(_, product)| {
            let delta = (*product * inverse - DMat4::IDENTITY).to_cols_array();
            delta
                .iter()
                .fold(0.0_f64, |max, value| max.max(value.abs()))
        })
        .fold(0.0_f64, f64::max);
    if !spread.is_finite() || spread > 1e-4 {
        return Err(invalid(
            "actor fit",
            format!(
                "the skin's inverse bind matrices disagree with its rest frames by {spread:.3e}; \
                 rewriting a frame would move the rest geometry"
            ),
        ));
    }
    Ok(SkinBase {
        matrix: first,
        spread,
    })
}

pub(super) fn node_names(actor: &Actor) -> Vec<Option<String>> {
    actor
        .document
        .nodes()
        .map(|node| node.name().map(str::to_owned))
        .collect()
}

/// Every unique vertex with its influence row, in the joint frame the rest
/// matrices are expressed in.
///
/// A vertex is read from its POSITION accessor and moved by the skin's base
/// matrix (`world * inverse_bind`, one constant over the skin), which is the
/// space the joint rest worlds live in. Sampling the posed mesh instead would
/// mix that space with the rendered frame.
fn sampled_vertices(
    actor: &Actor,
    selected: &[usize],
    base: &SkinBase,
) -> Result<Vec<Vertex>, ActorError> {
    let mut rows = Vec::new();
    let mut seen: std::collections::HashSet<([u64; 3], u64)> = std::collections::HashSet::new();
    for node in selected {
        let node = actor
            .document
            .nodes()
            .nth(*node)
            .ok_or_else(|| invalid(format!("node {node}"), "missing selected node"))?;
        let context = format!("node {}", node.index());
        let mesh = node
            .mesh()
            .ok_or_else(|| invalid(&context, "missing selected mesh"))?;
        let skin = node
            .skin()
            .ok_or_else(|| super::unsupported(&context, "unskinned mesh"))?;
        let joints: Vec<usize> = skin.joints().map(|joint| joint.index()).collect();
        for primitive in mesh.primitives() {
            let inside = format!(
                "node {} mesh {} primitive {}",
                node.index(),
                mesh.index(),
                primitive.index()
            );
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let positions = primitive
                .get(&gltf::mesh::Semantic::Positions)
                .ok_or_else(|| invalid(&inside, "missing POSITION"))?;
            super::decode::accessor(
                &positions,
                gltf::accessor::Dimensions::Vec3,
                &[gltf::accessor::DataType::F32],
                &inside,
            )?;
            let reader = primitive.reader(|buffer| actor.buffer(buffer));
            let points: Vec<[f32; 3]> = reader
                .read_positions()
                .ok_or_else(|| invalid(&inside, "unreadable POSITION"))?
                .collect();
            let skinning =
                geometry::skin_rows(actor, &primitive, joints.len(), points.len(), &inside)?;
            for (point, row) in points.into_iter().zip(skinning) {
                let influences: Vec<(usize, f64)> = row
                    .joints
                    .iter()
                    .zip(&row.weights)
                    .filter(|(_, weight)| **weight > 0.0)
                    .map(|(joint, weight)| (joints[usize::from(*joint)], *weight))
                    .collect();
                // One primitive per part repeats the same vertex arena; fit each
                // vertex once. The signature keeps distinct vertices that share a
                // position but not a weight row.
                let mut signature: Vec<(usize, u64)> = influences
                    .iter()
                    .map(|(node, weight)| (*node, weight.to_bits()))
                    .collect();
                signature.sort_unstable();
                let hash = signature.iter().fold(0_u64, |hash, (node, weight)| {
                    (hash.rotate_left(7) ^ (*node as u64)).rotate_left(13) ^ weight.rotate_left(3)
                });
                let key = (point.map(|value| f64::from(value).to_bits()), hash);
                if !seen.insert(key) {
                    continue;
                }
                let position = base
                    .matrix
                    .transform_point3(DVec3::from_array(point.map(f64::from)));
                rows.push(Vertex {
                    position,
                    influences,
                });
            }
        }
    }
    Ok(rows)
}

/// Split a palette bone name into its side and base name: `L` and `Finger10`.
/// Torso and neck bones carry no side and are reported as `C`.
pub(super) fn parse_name(name: &str) -> Option<(char, &str)> {
    let rest = name.strip_prefix("NPC ")?;
    let base = rest.split(" [").next()?.trim();
    if base.is_empty() {
        return None;
    }
    match base.strip_prefix("L ") {
        Some(tail) => Some(('L', tail.trim())),
        None => match base.strip_prefix("R ") {
            Some(tail) => Some(('R', tail.trim())),
            None => Some(('C', base)),
        },
    }
}

fn chain_specs(names: &[Option<String>]) -> Result<Vec<ChainSpec>, ActorError> {
    let mut index: BTreeMap<(char, String), usize> = BTreeMap::new();
    for (node, name) in names.iter().enumerate() {
        let Some((side, base)) = name.as_deref().and_then(parse_name) else {
            continue;
        };
        index.entry((side, base.to_owned())).or_insert(node);
    }
    // A chain is fitted when its joints exist; an actor without a part is not a
    // refusal, it just has one chain fewer.
    let find =
        |side: char, base: &str| -> Option<usize> { index.get(&(side, base.to_owned())).copied() };

    let mut specs = Vec::new();
    for side in ['L', 'R'] {
        for (digit, label) in [
            ("0", "thumb"),
            ("1", "index"),
            ("2", "middle"),
            ("3", "ring"),
            ("4", "pinky"),
        ] {
            let joints: Option<Vec<usize>> = ["0", "1", "2"]
                .iter()
                .map(|joint| find(side, &format!("Finger{digit}{joint}")))
                .collect();
            let Some(joints) = joints else { continue };
            specs.push(ChainSpec {
                label: format!("{side} {label}"),
                kind: "finger",
                flexion: Flexion::Palmar,
                joints,
                owned: vec![true; 3],
                plane_from: index.get(&(side, "Hand".to_owned())).copied(),
            });
        }
        if let (Some(foot), Some(toe)) = (find(side, "Foot"), find(side, "Toe0")) {
            specs.push(ChainSpec {
                label: format!("{side} foot"),
                kind: "foot",
                flexion: Flexion::Dorsal,
                joints: vec![foot, toe],
                owned: vec![true, true],
                plane_from: None,
            });
        }
        if let (Some(calf), Some(foot)) = (find(side, "Calf"), find(side, "Foot")) {
            specs.push(ChainSpec {
                label: format!("{side} knee"),
                kind: "knee",
                flexion: Flexion::Posterior,
                joints: vec![calf, foot],
                // The foot chain already placed the ankle; this chain fits the knee.
                owned: vec![true, false],
                plane_from: None,
            });
        }
    }
    if specs.is_empty() {
        return Err(invalid(
            "actor fit",
            "the palette names no extremity chain (NPC side, finger, foot, knee or neck joints)",
        ));
    }
    if let (Some(&neck), Some(&head)) = (
        index.get(&('C', "Neck".to_owned())),
        index.get(&('C', "Head".to_owned())),
    ) {
        specs.push(ChainSpec {
            label: "neck".to_owned(),
            kind: "neck",
            flexion: Flexion::Anterior,
            joints: vec![neck, head],
            owned: vec![true, true],
            plane_from: None,
        });
    }
    Ok(specs)
}

/// The plane each chain bends in: the palm normal for fingers, the sole normal
/// for the foot, the body's own axes for the knee and neck.
fn flexion_normals(
    vertices: &[Vertex],
    worlds: &[DMat4],
    names: &[Option<String>],
    specs: &[ChainSpec],
) -> BTreeMap<String, DVec3> {
    let by_name = |side: char, base: &str| -> Option<usize> {
        names.iter().position(|name| {
            name.as_deref()
                .and_then(parse_name)
                .is_some_and(|key| key == (side, base))
        })
    };
    let pelvis = by_name('L', "Pelvis").map(|node| worlds[node].transform_point3(DVec3::ZERO));
    let forward = body_forward(worlds, &by_name);
    let mut normals = BTreeMap::new();
    for spec in specs {
        let normal = match spec.kind {
            "finger" => {
                let palmar = spec.plane_from.and_then(|hand| {
                    let cloud = dominated_cloud(vertices, hand);
                    let center = centroid(&cloud)?;
                    let mut normal = principal_axes(&cloud, center)[2];
                    if let Some(pelvis) = pelvis
                        && normal.dot(pelvis - center) < 0.0
                    {
                        normal = -normal;
                    }
                    Some(normal)
                });
                palmar.unwrap_or(DVec3::Z)
            }
            "foot" => {
                let cloud = cloud_of(vertices, &spec.joints);
                match (centroid(&cloud), cloud.len() >= 9) {
                    (Some(center), true) => {
                        let mut normal = principal_axes(&cloud, center)[2];
                        if normal.z < 0.0 {
                            normal = -normal;
                        }
                        normal
                    }
                    _ => DVec3::Z,
                }
            }
            "knee" => -forward,
            _ => forward,
        };
        normals.insert(spec.label.clone(), normal.normalize_or(DVec3::Z));
    }
    normals
}

/// The actor's forward direction: ankle toward ball, averaged over both feet.
fn body_forward(worlds: &[DMat4], by_name: &impl Fn(char, &str) -> Option<usize>) -> DVec3 {
    let mut sum = DVec3::ZERO;
    for side in ['L', 'R'] {
        if let (Some(ankle), Some(ball)) = (by_name(side, "Foot"), by_name(side, "Toe0")) {
            sum += (worlds[ball].transform_point3(DVec3::ZERO)
                - worlds[ankle].transform_point3(DVec3::ZERO))
            .normalize_or_zero();
        }
    }
    sum.normalize_or(DVec3::Y)
}

/// `start` or its nearest ancestor that dominates flesh: the bone whose side of
/// the articulation a joint closes.
fn weight_carrier(
    parents: &[Option<usize>],
    vertices: &[Vertex],
    start: Option<usize>,
) -> Option<usize> {
    let mut cursor = start;
    while let Some(ancestor) = cursor {
        // Any positive weight counts: a bone can hold the other side of a blend
        // without dominating a single vertex.
        if vertices.iter().any(|vertex| vertex.weight(ancestor) > 0.0) {
            return Some(ancestor);
        }
        cursor = parents.get(ancestor).copied().flatten();
    }
    None
}

fn cloud_of(vertices: &[Vertex], nodes: &[usize]) -> Vec<DVec3> {
    vertices
        .iter()
        .filter(|vertex| vertex.dominant().is_some_and(|node| nodes.contains(&node)))
        .map(|vertex| vertex.position)
        .collect()
}

fn dominated_cloud(vertices: &[Vertex], node: usize) -> Vec<DVec3> {
    cloud_of(vertices, &[node])
}

/// The vertices whose weights blend two bones: the articulation band.
fn blend_band(vertices: &[Vertex], first: usize, second: usize) -> Vec<DVec3> {
    vertices
        .iter()
        .filter(|vertex| vertex.weight(first) > 0.01 && vertex.weight(second) > 0.01)
        .map(|vertex| vertex.position)
        .collect()
}

/// Longest extent of a cloud along its own principal axis.
fn band_span(points: &[DVec3]) -> f64 {
    let Some(center) = centroid(points) else {
        return 0.0;
    };
    if points.len() < 3 {
        return points
            .iter()
            .map(|point| point.distance(center))
            .fold(0.0_f64, f64::max)
            * 2.0;
    }
    let axis = principal_axes(points, center)[0];
    let mut along: Vec<f64> = points
        .iter()
        .map(|point| (*point - center).dot(axis))
        .collect();
    along.sort_by(f64::total_cmp);
    percentile(&along, 0.95) - percentile(&along, 0.05)
}

fn centroid(points: &[DVec3]) -> Option<DVec3> {
    match points.is_empty() {
        true => None,
        false => Some(points.iter().copied().sum::<DVec3>() / points.len() as f64),
    }
}

/// Eigenvectors of a cloud's covariance, largest spread first.
fn principal_axes(points: &[DVec3], center: DVec3) -> [DVec3; 3] {
    let mut covariance = [[0.0_f64; 3]; 3];
    for point in points {
        let delta = *point - center;
        for row in 0..3 {
            for column in row..3 {
                covariance[row][column] += delta[row] * delta[column];
            }
        }
    }
    for row in 0..3 {
        for column in 0..row {
            covariance[row][column] = covariance[column][row];
        }
    }
    let (values, vectors) = jacobi(covariance);
    let mut order = [0usize, 1, 2];
    order.sort_by(|left, right| values[*right].total_cmp(&values[*left]));
    [
        DVec3::from_array(vectors[order[0]]),
        DVec3::from_array(vectors[order[1]]),
        DVec3::from_array(vectors[order[2]]),
    ]
}

/// Symmetric 3×3 Jacobi: eigenvalues on the diagonal, unit eigenvectors as the
/// columns of the returned matrix.
// Clippy's range-loop lint asks for iterators; a Jacobi sweep is addressed by
// row and column index, which is what the algorithm states.
#[allow(clippy::needless_range_loop)]
fn jacobi(mut matrix: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut vectors = [[1.0_f64, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..32 {
        let mut largest = 0.0_f64;
        let mut pivot = (0usize, 1usize);
        for row in 0..3 {
            for column in (row + 1)..3 {
                if matrix[row][column].abs() > largest {
                    largest = matrix[row][column].abs();
                    pivot = (row, column);
                }
            }
        }
        if largest < 1e-18 {
            break;
        }
        let (p, q) = pivot;
        let angle = 0.5 * (2.0 * matrix[p][q]).atan2(matrix[q][q] - matrix[p][p]);
        let (sin, cos) = angle.sin_cos();
        for row in 0..3 {
            let left = matrix[row][p];
            let right = matrix[row][q];
            matrix[row][p] = cos * left - sin * right;
            matrix[row][q] = sin * left + cos * right;
        }
        for column in 0..3 {
            let left = matrix[p][column];
            let right = matrix[q][column];
            matrix[p][column] = cos * left - sin * right;
            matrix[q][column] = sin * left + cos * right;
        }
        for row in 0..3 {
            let left = vectors[row][p];
            let right = vectors[row][q];
            vectors[row][p] = cos * left - sin * right;
            vectors[row][q] = sin * left + cos * right;
        }
    }
    let eigenvalues = [matrix[0][0], matrix[1][1], matrix[2][2]];
    let eigenvectors = [
        [vectors[0][0], vectors[1][0], vectors[2][0]],
        [vectors[0][1], vectors[1][1], vectors[2][1]],
        [vectors[0][2], vectors[1][2], vectors[2][2]],
    ];
    (eigenvalues, eigenvectors)
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    match sorted.is_empty() {
        true => 0.0,
        false => {
            let position = fraction.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
            let low = position.floor() as usize;
            let high = position.ceil() as usize;
            let weight = position - low as f64;
            sorted[low] * (1.0 - weight) + sorted[high] * weight
        }
    }
}

/// The flesh line: principal axis of the cloud with the farthest tenth dropped,
/// refitted twice. Returns `(center, axis, kept)`.
fn flesh_line(points: &[DVec3], direction: DVec3) -> Option<(DVec3, DVec3, Vec<DVec3>)> {
    if points.len() < 8 {
        return None;
    }
    let mut keep: Vec<DVec3> = points.to_vec();
    let mut center = centroid(&keep)?;
    let mut axis = direction;
    for _ in 0..3 {
        let found = principal_axes(&keep, center)[0];
        axis = match found.dot(direction) < 0.0 {
            true => -found,
            false => found,
        };
        let mut distances: Vec<f64> = keep
            .iter()
            .map(|point| perpendicular(*point, center, axis))
            .collect();
        distances.sort_by(f64::total_cmp);
        let limit = percentile(&distances, 0.9);
        let trimmed: Vec<DVec3> = keep
            .iter()
            .copied()
            .filter(|point| perpendicular(*point, center, axis) <= limit)
            .collect();
        if trimmed.len() < 8 {
            break;
        }
        keep = trimmed;
        center = centroid(&keep)?;
    }
    Some((center, axis, keep))
}

fn along_percentile(cloud: &[DVec3], center: DVec3, axis: DVec3, fraction: f64) -> f64 {
    let mut along: Vec<f64> = cloud
        .iter()
        .map(|point| (*point - center).dot(axis))
        .collect();
    along.sort_by(f64::total_cmp);
    percentile(&along, fraction)
}

fn perpendicular(point: DVec3, center: DVec3, axis: DVec3) -> f64 {
    let delta = point - center;
    (delta - axis * delta.dot(axis)).length()
}

/// A fitted chain: its record, the pivots it decided, and the axis it measured.
type ChainOutcome = (ChainFit, Vec<PlacedJoint>, DVec3);

/// Everything a chain fit reads, so the call states its inputs once.
struct FitContext<'a> {
    names: &'a [Option<String>],
    parents: &'a [Option<usize>],
    vertices: &'a [Vertex],
    worlds: &'a [DMat4],
    pivots: &'a BTreeMap<usize, DVec3>,
    lengths: LengthMode,
}

fn fit_chain(
    context: &FitContext<'_>,
    spec: &ChainSpec,
    plane: DVec3,
) -> Result<Option<ChainOutcome>, String> {
    let FitContext {
        names,
        parents,
        vertices,
        worlds,
        pivots,
        lengths,
    } = *context;
    let current: Vec<DVec3> = spec
        .joints
        .iter()
        .map(|node| {
            pivots
                .get(node)
                .copied()
                .unwrap_or_else(|| worlds[*node].transform_point3(DVec3::ZERO))
        })
        .collect();
    let direction = (current[current.len() - 1] - current[0]).normalize_or_zero();
    if direction == DVec3::ZERO {
        return Err("the chain has no direction to fit".to_owned());
    }
    let old_lengths: Vec<f64> = current
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .collect();
    let total: f64 = old_lengths.iter().sum();

    let owned_nodes: Vec<usize> = spec
        .joints
        .iter()
        .zip(&spec.owned)
        .filter(|(_, owned)| **owned)
        .map(|(node, _)| *node)
        .collect();
    let chain_cloud = cloud_of(vertices, &owned_nodes);
    // The chain's own line, then the span clip along it. Clipping along the
    // authored direction would tilt the fit whenever that direction is the defect
    // (Emily's foot bone runs to the sole at 31° from the foot's axis).
    let rough = match flesh_line(&chain_cloud, direction) {
        Some((_, axis, _)) => axis,
        None => direction,
    };
    // Clip to the chain's own span: a bone that over-owns a neighbour's region
    // (Emily's toe dominates most of the foot) must not be fitted to it.
    let pressed: Vec<DVec3> = chain_cloud
        .iter()
        .copied()
        .filter(|point| {
            let along = (*point - current[0]).dot(rough);
            along > -0.5 * total && along < total * 2.0
        })
        .collect();
    let use_cloud = match pressed.len() >= 8 {
        true => pressed,
        false => chain_cloud.clone(),
    };
    let line = flesh_line(&use_cloud, rough);

    let mut placed = Vec::with_capacity(spec.joints.len());
    for (position, node) in spec.joints.iter().enumerate() {
        if !spec.owned[position] {
            placed.push(PlacedJoint {
                node: *node,
                pivot: current[position],
                placed_by: Placed::Authored,
                band_vertices: 0,
                band_span: 0.0,
            });
            continue;
        }
        // The parent-side bone: the previous chain joint, or the nearest weighted
        // ancestor at the chain root.
        let side = match position {
            0 => parents.get(*node).copied().flatten(),
            _ => Some(spec.joints[position - 1]),
        };
        let parent_bone = weight_carrier(parents, vertices, side);
        let band = parent_bone
            .map(|parent| blend_band(vertices, parent, *node))
            .unwrap_or_default();
        // A band as large as the chain's whole flesh is a gradual weight blend,
        // not an articulation: it cannot say where inside it the joint is, so the
        // authored pivot stays and the joint is reported as authored rather than
        // moved on a guess. Mike's calf↔foot blend spans 282 mm and its centroid
        // sits 56 mm up his shin; Emily's spans 289 mm the same way.
        // Four points is the smallest band whose centroid is a region rather
        // than one vertex's position.
        if band.len() >= 4
            && let Some(center) = centroid(&band)
        {
            placed.push(PlacedJoint {
                node: *node,
                pivot: center,
                placed_by: Placed::BlendBand,
                band_vertices: band.len(),
                band_span: band_span(&band),
            });
            continue;
        }
        // No band: the two bones' dominated flesh decides the boundary along the
        // chain's own line, otherwise the authored position moves onto the line.
        let mut placed_by = Placed::Authored;
        let mut pivot = current[position];
        let own_flesh = dominated_cloud(vertices, *node).len();
        if own_flesh >= 8
            && let Some((center, axis, _)) = &line
        {
            let parent_cloud = parent_bone
                .map(|parent| dominated_cloud(vertices, parent))
                .unwrap_or_default();
            let own_cloud = dominated_cloud(vertices, *node);
            let along = match lengths {
                LengthMode::Flesh if parent_cloud.len() >= 8 && own_cloud.len() >= 8 => {
                    placed_by = Placed::FleshBoundary;
                    0.5 * (along_percentile(&parent_cloud, *center, *axis, 0.9)
                        + along_percentile(&own_cloud, *center, *axis, 0.1))
                }
                LengthMode::Flesh if own_cloud.len() >= 8 => {
                    placed_by = Placed::FleshBoundary;
                    along_percentile(&own_cloud, *center, *axis, 0.1)
                }
                _ => {
                    placed_by = Placed::AuthoredOnLine;
                    (current[position] - *center).dot(*axis)
                }
            };
            pivot = *center + *axis * along;
        }
        placed.push(PlacedJoint {
            node: *node,
            pivot,
            placed_by,
            band_vertices: band.len(),
            band_span: band_span(&band),
        });
    }

    let measured_any = placed.iter().any(|joint| joint.placed_by.measured());
    if !measured_any {
        return Ok(None);
    }
    let axis = match line {
        Some((_, axis, _)) => axis,
        None => {
            let first = placed[0].pivot;
            let last = placed[placed.len() - 1].pivot;
            (last - first).normalize_or(direction)
        }
    };
    let kept = match flesh_line(&use_cloud, direction) {
        Some((_, _, kept)) => kept.len(),
        None => 0,
    };

    let angle = |from: DVec3| -> f64 {
        from.normalize_or_zero()
            .dot(axis)
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    };
    let axis_before = angle(current[current.len() - 1] - current[0]);
    let axis_after = angle(placed[placed.len() - 1].pivot - placed[0].pivot);
    let (center, line_axis) = line
        .as_ref()
        .map(|(center, axis, _)| (*center, *axis))
        .unwrap_or((DVec3::ZERO, axis));

    let mut joints = Vec::with_capacity(spec.joints.len());
    for (position, node) in spec.joints.iter().enumerate() {
        let before = current[position];
        let after = placed[position].pivot;
        let cloud = dominated_cloud(vertices, *node);
        let center_of_mass = centroid(&cloud);
        joints.push(JointFit {
            name: names[*node]
                .clone()
                .unwrap_or_else(|| format!("node {node}")),
            node: *node,
            placed_by: placed[position].placed_by,
            band_vertices: placed[position].band_vertices,
            band_span: placed[position].band_span,
            before_head: before.to_array(),
            after_head: after.to_array(),
            perpendicular_before: perpendicular(before, center, line_axis),
            perpendicular_after: perpendicular(after, center, line_axis),
            centroid_before: center_of_mass.map(|point| point.distance(before)),
            centroid_after: center_of_mass.map(|point| point.distance(after)),
            segment_before: old_lengths.get(position).copied(),
            segment_after: placed
                .get(position + 1)
                .map(|next| next.pivot.distance(after)),
            dominated: cloud.len(),
            owned: spec.owned[position],
        });
    }

    // How far the authored root frame's local X sits out of the flexion plane.
    let reference = flexion_off_plane(
        worlds[spec.joints[0]].to_scale_rotation_translation().1,
        plane,
    );

    Ok(Some((
        ChainFit {
            label: spec.label.clone(),
            kind: spec.kind,
            flexion: spec.flexion,
            axis: axis.to_array(),
            flesh_vertices: kept,
            axis_before_degrees: axis_before,
            axis_after_degrees: axis_after,
            flexion_axis_off_plane_before_degrees: reference,
            flexion_axis_off_plane_after_degrees: reference,
            joints,
        },
        placed,
        axis,
    )))
}

/// Rebuild every skin joint's world, local transform and inverse bind.
pub(super) fn build_frames(
    actor: &Actor,
    names: &[Option<String>],
    worlds: &[DMat4],
    binds: &[(usize, DMat4)],
    base: &SkinBase,
    heads: &BTreeMap<usize, DVec3>,
    rotations: &BTreeMap<usize, DQuat>,
) -> Result<Vec<FittedFrame>, ActorError> {
    let mut fitted_worlds = worlds.to_vec();
    for (node, head) in heads {
        let (scale, rotation, _) = worlds[*node].to_scale_rotation_translation();
        let rotation = rotations.get(node).copied().unwrap_or(rotation);
        fitted_worlds[*node] = DMat4::from_scale_rotation_translation(scale, rotation, *head);
    }
    let mut frames = Vec::with_capacity(binds.len());
    for (node, _) in binds {
        let world = fitted_worlds[*node];
        let (_, rotation, translation) = world.to_scale_rotation_translation();
        let parent = match actor.parents[*node] {
            Some(parent) => fitted_worlds[parent],
            None => DMat4::IDENTITY,
        };
        let local = parent.inverse() * world;
        let (local_scale, local_rotation, local_translation) =
            local.to_scale_rotation_translation();
        let rest_local = match actor.parents[*node] {
            Some(parent) => worlds[parent].inverse() * worlds[*node],
            None => worlds[*node],
        }
        .to_scale_rotation_translation();
        let rest = worlds[*node].transform_point3(DVec3::ZERO);
        let rewritten = rest_local.2.distance(local_translation) > 1e-9
            || rest_local.1.dot(local_rotation).abs() < 1.0 - 1e-12;
        frames.push(FittedFrame {
            node: *node,
            name: names[*node]
                .clone()
                .unwrap_or_else(|| format!("node {node}")),
            translation: translation.to_array(),
            rotation: rotation.to_array(),
            scale: local_scale.x,
            local_translation: local_translation.to_array(),
            local_rotation: local_rotation.to_array(),
            inverse_bind: (world.inverse() * base.matrix).to_cols_array_2d(),
            moved: rest.distance(translation) > 1e-9,
            rewritten,
        });
    }
    Ok(frames)
}

/// Write the fitted frames and inverse binds back into the actor GLB.
///
/// # Errors
/// Any refusal of the frame writer: an unparsable GLB, a joint node addressed by
/// index that the document does not carry, or a skin whose inverse bind accessor
/// it cannot overwrite in place.
pub fn write_fit(glb: &[u8], outcome: &FitOutcome) -> Result<Vec<u8>, ActorError> {
    write_frames(glb, &outcome.frames)
}

/// Write rest frames and their inverse binds back into the actor GLB.
///
/// # Errors
/// Any refusal of the frame writer: an unparsable GLB, a joint node addressed by
/// index that the document does not carry, or a skin whose inverse bind accessor
/// it cannot overwrite in place.
pub fn write_frames(glb: &[u8], frames: &[FittedFrame]) -> Result<Vec<u8>, ActorError> {
    let rewrites: Vec<sculpto_gltf::frame::JointFrame> = frames
        .iter()
        .filter(|frame| frame.rewritten)
        .map(|frame| sculpto_gltf::frame::JointFrame {
            node: frame.node,
            translation: frame.local_translation,
            rotation: frame.local_rotation,
            scale: frame.scale,
        })
        .collect();
    let binds: Vec<sculpto_gltf::frame::JointBind> = frames
        .iter()
        .map(|frame| sculpto_gltf::frame::JointBind {
            node: frame.node,
            matrix: frame.inverse_bind,
        })
        .collect();
    sculpto_gltf::frame::rewrite_frames(glb, &rewrites, &binds)
        .map_err(|error| invalid("actor fit write", error.to_string()))
}
