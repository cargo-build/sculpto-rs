//! `body assemble`: the prepared body built from its declared parts.
//!
//! A prepared body is one mesh holding a torso, feet, a head and hands in one
//! frame, with the neck and wrists seated and sealed. Which part goes where and
//! in which frame used to live in a Python fixture; the manifest carries that
//! data and this module applies exactly what it declares. Nothing is inferred: a
//! part moves only by its own `rotate`/`translate`/`drop`, and a rim is bridged
//! only where a joint is declared. The seal itself stays the mesh crate's
//! [`joint_seal`] path, so the assembly and `body seal-joints` cannot drift.
//!
//! The seat is signed per part: `translate` carries the full source-frame offset,
//! so a family whose head must move **up** or sideways is expressible, while
//! `drop` stays the convenient non-negative downward case. A manifest that
//! declares no seat is not given one; the seal refuses the joint instead.
//!
//! Ankles are not a joint role here: the feet are accepted as authored, and the
//! seal's explicit ankle pair mispairs the leg chain (BODY-01 0020 §3), so a
//! manifest cannot declare one.
//! @req BODY-01

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use sculpto_mesh::joint_seal::{self, JointRequest, SealOptions, SealOutcome};
use sculpto_mesh::parts::{self, ShellIdentity};

use crate::obj::{ObjMesh, read_obj_object, read_obj_surface_typed, write_obj};
use crate::ops::file_sha256;
use crate::seal::{joint_row, seal_error};
use crate::{CommandError, ErrorKind};

/// The only manifest schema this build accepts.
const MANIFEST_VERSION: u32 = 1;

/// The measured torso shell name; the socket every joint pairs against.
const TORSO: &str = "torso";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn name(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Rotate {
    /// Source axis the part turns about, through the origin.
    axis: Axis,
    /// Degrees, right-handed about that axis.
    degrees: f32,
}

/// The body region one part supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum Role {
    /// The torso; exactly one part, and it comes first.
    Body,
    /// Feet; repeatable, one part per source file.
    Feet,
    /// The head.
    Head,
    /// Both hands in one file; the seat pairs them across the wrists.
    Hands,
    /// One hand in its own file.
    HandLeft,
    /// One hand in its own file.
    HandRight,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Feet => "feet",
            Self::Head => "head",
            Self::Hands => "hands",
            Self::HandLeft => "hand-left",
            Self::HandRight => "hand-right",
        }
    }

    /// May the manifest declare this role more than once?
    fn repeats(self) -> bool {
        self == Self::Feet
    }

    /// Is this role a hand part the seat may move?
    fn is_hand(self) -> bool {
        matches!(self, Self::Hands | Self::HandLeft | Self::HandRight)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PartSpec {
    /// OBJ the part is read from.
    path: PathBuf,
    /// Body region this part supplies.
    role: Role,
    /// `o` block inside a multi-object OBJ; absent reads the whole file.
    object: Option<String>,
    /// Signed per-axis scale about the origin, applied first: a negative lane
    /// is a declared mirror of that axis and rewinds the part's faces so the
    /// surface keeps facing outward. Nothing is mirrored unless declared.
    scale: Option<[f32; 3]>,
    /// Rotation about one source axis through the origin, applied after the
    /// scale.
    rotate: Option<Rotate>,
    /// Signed translation in source units (source axes, +Z up), applied last:
    /// the place to express an upward or lateral seat, which `drop` cannot.
    translate: Option<[f32; 3]>,
    /// Non-negative downward seat along the source up axis (+Z), in source
    /// units: the explicit vertical seat `body seal-joints --head-drop` applies.
    /// An upward or lateral seat is [`translate`](Self::translate), which is
    /// signed; this field exists so the common case stays readable.
    drop: Option<f32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct JointSpec {
    /// Part role whose shells are bridged.
    part: Role,
    /// Socket role the shells pair against (the torso).
    socket: Role,
    /// Seat the part before bridging: the hands path, never the neck.
    seat: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SealSpec {
    /// Laplacian passes over the seam band (default 12).
    iterations: Option<u32>,
    /// Ring distance from the seam that may move (default 2).
    band: Option<u32>,
    /// Hard displacement bound per vertex, in source units.
    max_shift: Option<f32>,
    /// Candidate-rim proximity band; derived from the socket ring when absent.
    arc_band: Option<f64>,
    /// Smooth the seam after bridging (default true).
    smooth: Option<bool>,
    /// Merge exactly coincident boundary copies beyond the pinch (default false).
    weld_coincident: Option<bool>,
}

/// The versioned assembly manifest, as `body assemble` reads it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// Manifest schema version; this build accepts 1.
    version: u32,
    /// Parts in assembly order; the `body` part must come first.
    parts: Vec<PartSpec>,
    /// Joints to seat and seal; an empty list leaves the concatenation unsealed.
    #[serde(default)]
    joints: Vec<JointSpec>,
    /// Seal settings; absent members keep the seal's measured defaults.
    #[serde(default)]
    seal: SealSpec,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Params {
    /// Versioned part/joint manifest.
    manifest: PathBuf,
    /// Assembled and sealed body OBJ.
    out: PathBuf,
    /// Also write the JSON report here.
    report: Option<PathBuf>,
}

/// One part read into the shared frame, with the offsets it occupies.
struct PlacedPart {
    order: usize,
    role: Role,
    path: PathBuf,
    object: Option<String>,
    sha256: String,
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    vertex_offset: usize,
    triangle_offset: usize,
    placement: Value,
}

/// A declared joint, resolved to the shells the seal pairs.
struct ResolvedJoint {
    part: Role,
    socket: Role,
    seat: bool,
    shells: Vec<(String, String)>,
}

/// The command input schema. The manifest is a *file* here, so its own
/// versioned shape is documented in the CLI help and in this module rather than
/// nested in the registry row; [`Manifest`] stays the single source for both.
pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(Params))
}

pub(crate) fn assemble(input: &Value) -> Result<Value, CommandError> {
    let params: Params = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "assemble_input",
            error.to_string(),
        )
    })?;
    let manifest = read_manifest(&params.manifest)?;
    let parts = read_parts(&manifest)?;
    let (positions, triangles) = concatenate(&parts);
    let identities = parts::shell_identities(&positions, &triangles);
    let (requests, joints) = resolve_joints(&manifest, &parts, &identities)?;
    let sealed = if requests.is_empty() {
        None
    } else {
        Some(
            joint_seal::seal_joints(
                &positions,
                &triangles,
                &requests,
                &seal_options(&manifest.seal),
            )
            .map_err(seal_failure)?,
        )
    };
    let (out_positions, out_triangles) = sealed
        .as_ref()
        .map_or((positions.as_slice(), triangles.as_slice()), |outcome| {
            (outcome.positions.as_slice(), outcome.triangles.as_slice())
        });
    write_obj(&params.out, out_positions, out_triangles).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "assemble_out",
            format!("{}: {error}", params.out.display()),
        )
    })?;
    let report = render(&params, &manifest, &parts, &joints, sealed.as_ref());
    if let Some(path) = &params.report {
        write_report(path, &report)?;
    }
    Ok(report)
}

fn read_manifest(path: &Path) -> Result<Manifest, CommandError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "assemble_manifest_io",
            format!("{}: {error}", path.display()),
        )
    })?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidRequest,
            "assemble_manifest",
            format!("{}: {error}", path.display()),
        )
    })?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Refuse a manifest the assembly cannot honour, before any part is read.
fn validate(manifest: &Manifest) -> Result<(), CommandError> {
    if manifest.version != MANIFEST_VERSION {
        return Err(CommandError::new(
            ErrorKind::InvalidParams,
            "assemble_version",
            format!(
                "manifest version {} is not supported; this build reads {MANIFEST_VERSION}",
                manifest.version
            ),
        ));
    }
    let Some(first) = manifest.parts.first() else {
        return Err(CommandError::new(
            ErrorKind::InvalidParams,
            "assemble_parts_empty",
            "the manifest declares no parts",
        ));
    };
    let bodies = manifest
        .parts
        .iter()
        .filter(|part| part.role == Role::Body)
        .count();
    if bodies != 1 {
        return Err(CommandError::new(
            ErrorKind::InvalidParams,
            "assemble_body_role",
            format!("exactly one part must have role `body`; the manifest declares {bodies}"),
        ));
    }
    if first.role != Role::Body {
        return Err(CommandError::new(
            ErrorKind::InvalidParams,
            "assemble_body_not_first",
            format!(
                "the first part must have role `body`, not `{}`: the body keeps vertex offset 0",
                first.role.name()
            ),
        ));
    }
    let mut seen: Vec<Role> = Vec::new();
    for part in &manifest.parts {
        if part.role.repeats() {
            continue;
        }
        if seen.contains(&part.role) {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "assemble_role_duplicate",
                format!(
                    "role `{}` is declared twice; only `feet` may repeat",
                    part.role.name()
                ),
            ));
        }
        seen.push(part.role);
    }
    for part in &manifest.parts {
        if part.drop.is_some_and(|drop| drop < 0.0) {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "assemble_part_placement",
                format!(
                    "part `{}` declares a negative `drop`; `drop` is downward-only, use `translate` for an upward seat",
                    part.role.name()
                ),
            ));
        }
        if part
            .scale
            .is_some_and(|scale| scale.iter().any(|lane| !lane.is_finite() || *lane == 0.0))
        {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "assemble_part_scale",
                format!(
                    "part `{}` declares a zero or non-finite `scale`; every lane must be a finite non-zero factor",
                    part.role.name()
                ),
            ));
        }
        let finite = part
            .translate
            .is_none_or(|value| value.iter().all(|lane| lane.is_finite()))
            && part.drop.is_none_or(f32::is_finite)
            && part.rotate.is_none_or(|rotate| rotate.degrees.is_finite());
        if !finite {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "assemble_part_placement",
                format!(
                    "part `{}` declares a non-finite placement",
                    part.role.name()
                ),
            ));
        }
    }
    for joint in &manifest.joints {
        for role in [joint.part, joint.socket] {
            if !manifest.parts.iter().any(|part| part.role == role) {
                return Err(CommandError::new(
                    ErrorKind::InvalidParams,
                    "assemble_joint_role",
                    format!(
                        "joint names role `{}`, which no part declares; declared: [{}]",
                        role.name(),
                        manifest
                            .parts
                            .iter()
                            .map(|part| part.role.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }
        if joint.part == joint.socket {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "assemble_joint_same_role",
                format!(
                    "joint names `{}` as both part and socket",
                    joint.part.name()
                ),
            ));
        }
        if joint.part == Role::Feet || joint.socket == Role::Feet {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "assemble_feet_joint",
                "feet are accepted as authored: the seal's explicit ankle pair mispairs the leg chain, so the prepared body declares no ankle joint",
            ));
        }
        if joint.seat.unwrap_or(false) && !joint.part.is_hand() {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "assemble_seat_role",
                format!(
                    "`seat` is the hands path; part `{}` is not a hand",
                    joint.part.name()
                ),
            ));
        }
    }
    Ok(())
}

fn read_parts(manifest: &Manifest) -> Result<Vec<PlacedPart>, CommandError> {
    let mut parts: Vec<PlacedPart> = Vec::with_capacity(manifest.parts.len());
    let mut vertex_offset = 0usize;
    let mut triangle_offset = 0usize;
    for (order, spec) in manifest.parts.iter().enumerate() {
        let (mut positions, mut triangles) = read_part(spec)?;
        if mirrored(spec) {
            rewind(&mut triangles);
        }
        if positions.is_empty() || triangles.is_empty() {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "assemble_part_empty",
                format!(
                    "part `{}` has {} vertices and {} triangles",
                    spec.role.name(),
                    positions.len(),
                    triangles.len()
                ),
            ));
        }
        place(&mut positions, spec);
        let sha256 = file_sha256(&spec.path).map_err(|error| part_hash_failure(spec, error))?;
        parts.push(PlacedPart {
            order,
            role: spec.role,
            path: spec.path.clone(),
            object: spec.object.clone(),
            sha256,
            positions,
            triangles,
            vertex_offset,
            triangle_offset,
            placement: placement_row(spec),
        });
        vertex_offset += parts.last().expect("just pushed").positions.len();
        triangle_offset += parts.last().expect("just pushed").triangles.len();
    }
    Ok(parts)
}

fn read_part(spec: &PartSpec) -> Result<ObjMesh, CommandError> {
    let is_obj = spec
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("obj"));
    if !is_obj {
        return Err(CommandError::new(
            ErrorKind::InvalidParams,
            "assemble_part_format",
            format!(
                "part `{}` ({}) is not an OBJ; export the shape first (`sculpto export-obj`)",
                spec.role.name(),
                spec.path.display()
            ),
        ));
    }
    let read = match &spec.object {
        Some(name) => read_obj_object(&spec.path, name),
        None => read_obj_surface_typed(&spec.path),
    };
    read.map_err(|error| part_failure(spec, error))
}

/// Name the part in every read failure, so a missing or misnamed input is
/// never reported as an anonymous file error.
fn part_failure(spec: &PartSpec, error: crate::obj::ObjError) -> CommandError {
    use crate::obj::ObjError;
    let role = spec.role.name();
    let path = spec.path.display();
    match error {
        ObjError::Io(message) => CommandError::new(
            ErrorKind::Io,
            "assemble_part_io",
            format!("part `{role}` ({path}): {message}"),
        ),
        ObjError::Malformed(message) => CommandError::new(
            ErrorKind::InvalidParams,
            "assemble_part_malformed",
            format!("part `{role}` ({path}): {message}"),
        ),
        ObjError::MissingObject { name, available } => CommandError::new(
            ErrorKind::NotFound,
            "assemble_object_missing",
            format!(
                "part `{role}` ({path}): object `{name}` not found; available: [{}]",
                available.join(", ")
            ),
        ),
    }
}

fn part_hash_failure(spec: &PartSpec, error: CommandError) -> CommandError {
    let code = match error.kind() {
        ErrorKind::Io => "assemble_part_io",
        _ => "assemble_part_malformed",
    };
    CommandError::new(
        error.kind(),
        code,
        format!(
            "part `{}` ({}): {}",
            spec.role.name(),
            spec.path.display(),
            error.message()
        ),
    )
}
/// Apply the declared placement, in the order the manifest documents.
fn place(positions: &mut [[f32; 3]], spec: &PartSpec) {
    let translation = effective_translation(spec);
    let scale = spec.scale.unwrap_or([1.0; 3]);
    for position in positions.iter_mut() {
        let scaled = [
            position[0] * scale[0],
            position[1] * scale[1],
            position[2] * scale[2],
        ];
        let rotated = spec
            .rotate
            .map_or(scaled, |rotate| rotate_point(scaled, rotate));
        *position = [
            rotated[0] + translation[0],
            rotated[1] + translation[1],
            rotated[2] + translation[2],
        ];
    }
}

/// Does this part declare a mirror? An odd number of negative lanes reverses
/// handedness, so the faces must be rewound with it.
fn mirrored(spec: &PartSpec) -> bool {
    spec.scale
        .is_some_and(|scale| scale.iter().filter(|lane| **lane < 0.0).count() % 2 == 1)
}

/// Rewind the faces of a mirrored part so the surface keeps facing outward.
fn rewind(triangles: &mut [[u32; 3]]) {
    for triangle in triangles.iter_mut() {
        triangle.swap(1, 2);
    }
}

fn effective_translation(spec: &PartSpec) -> [f32; 3] {
    let mut translation = spec.translate.unwrap_or([0.0; 3]);
    translation[2] -= spec.drop.unwrap_or(0.0);
    translation
}

/// Rotate one source-frame point about the named axis through the origin.
///
/// A quarter turn is an exact permutation of the coordinates: `sin 90°` is not
/// representable, and the authored neck cut and its coincident copies are
/// compared bit-exactly, so an inexact matrix would move them apart.
fn rotate_point(point: [f32; 3], rotate: Rotate) -> [f32; 3] {
    let [a, b, c] = point;
    let quarters = rotate.degrees / 90.0;
    if quarters.fract() == 0.0 {
        let turn = (quarters as i64).rem_euclid(4);
        return match (rotate.axis, turn) {
            (Axis::X, 1) => [a, -c, b],
            (Axis::X, 2) => [a, -b, -c],
            (Axis::X, 3) => [a, c, -b],
            (Axis::Y, 1) => [c, b, -a],
            (Axis::Y, 2) => [-a, b, -c],
            (Axis::Y, 3) => [-c, b, a],
            (Axis::Z, 1) => [-b, a, c],
            (Axis::Z, 2) => [-a, -b, c],
            (Axis::Z, 3) => [b, -a, c],
            _ => point,
        };
    }
    let (sin, cos) = rotate.degrees.to_radians().sin_cos();
    match rotate.axis {
        Axis::X => [a, b * cos - c * sin, b * sin + c * cos],
        Axis::Y => [a * cos + c * sin, b, c * cos - a * sin],
        Axis::Z => [a * cos - b * sin, a * sin + b * cos, c],
    }
}

fn placement_row(spec: &PartSpec) -> Value {
    json!({
        "scale": spec.scale,
        "mirrored": mirrored(spec),
        "rotate": spec.rotate.map(|rotate| json!({
            "axis": rotate.axis.name(),
            "degrees": rotate.degrees,
        })),
        "translate": spec.translate,
        "drop": spec.drop.unwrap_or(0.0),
        "translation": effective_translation(spec),
    })
}

/// Concatenate in assembly order: the torso first, then the manifest order.
///
/// [`validate`] guarantees the body is the first part, so the manifest order is
/// the assembly order and every offset below is a plain running total.
fn concatenate(parts: &[PlacedPart]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let vertices: usize = parts.iter().map(|part| part.positions.len()).sum();
    let triangles: usize = parts.iter().map(|part| part.triangles.len()).sum();
    let mut positions = Vec::with_capacity(vertices);
    let mut faces = Vec::with_capacity(triangles);
    for part in parts {
        let offset = positions.len() as u32;
        positions.extend_from_slice(&part.positions);
        faces.extend(
            part.triangles
                .iter()
                .map(|corners| corners.map(|index| index + offset)),
        );
    }
    (positions, faces)
}

/// Every shell that lies wholly inside one part's vertex range.
///
/// A part is concatenated without sharing vertices, so a shell belongs to a
/// part exactly when its members are inside that part's range. The classifier's
/// shell names are spatial; this mapping is topological, so a misnamed hand
/// (`sole`) still resolves to the part that supplied it.
fn shells_of(parts: &[PlacedPart], identities: &[ShellIdentity]) -> Vec<Vec<usize>> {
    let mut owned: Vec<Vec<usize>> = vec![Vec::new(); parts.len()];
    for (index, shell) in identities.iter().enumerate() {
        let Some(&first) = shell.members.first() else {
            continue;
        };
        let Some(owner) = parts.iter().position(|part| {
            first >= part.vertex_offset && first < part.vertex_offset + part.positions.len()
        }) else {
            continue;
        };
        let end = parts[owner].vertex_offset + parts[owner].positions.len();
        if shell.members.iter().all(|&member| member < end) {
            owned[owner].push(index);
        }
    }
    owned
}

fn resolve_joints(
    manifest: &Manifest,
    parts: &[PlacedPart],
    identities: &[ShellIdentity],
) -> Result<(Vec<JointRequest>, Vec<ResolvedJoint>), CommandError> {
    let owned = shells_of(parts, identities);
    let mut requests = Vec::new();
    let mut resolved = Vec::with_capacity(manifest.joints.len());
    for joint in &manifest.joints {
        let seat = joint.seat.unwrap_or(false);
        let part_shells = named_shells(joint.part, parts, &owned, identities);
        if part_shells.is_empty() {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "assemble_part_unnamed",
                format!(
                    "joint part role `{}` contributes no shell the seal can address",
                    joint.part.name()
                ),
            ));
        }
        if joint.part != Role::Hands && part_shells.len() != 1 {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "assemble_part_shells",
                format!(
                    "joint part role `{}` resolves to {} addressable shells; exactly one is required",
                    joint.part.name(),
                    part_shells.len()
                ),
            ));
        }
        if joint.part == Role::Body && !part_shells.contains(&TORSO) {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "assemble_body_not_torso",
                format!(
                    "the body part carries no shell named `{TORSO}`; it has [{}]",
                    part_shells.join(", ")
                ),
            ));
        }
        let socket = socket_shell(
            joint.socket,
            &named_shells(joint.socket, parts, &owned, identities),
        )?;
        let mut shells = Vec::with_capacity(part_shells.len());
        for part_shell in &part_shells {
            requests.push(JointRequest {
                part: (*part_shell).to_string(),
                socket: (*socket).to_string(),
                seat,
            });
            shells.push(((*part_shell).to_string(), (*socket).to_string()));
        }
        resolved.push(ResolvedJoint {
            part: joint.part,
            socket: joint.socket,
            seat,
            shells,
        });
    }
    Ok((requests, resolved))
}

/// The one shell a joint pairs against: the measured torso when the role
/// carries one, otherwise the role's only shell.
///
/// A body family ships extra shells inside the torso file (the Bijin body has
/// detached leg shells next to the torso), so "the socket role" and "the socket
/// shell" are not the same set.
fn socket_shell<'a>(role: Role, shells: &[&'a str]) -> Result<&'a str, CommandError> {
    if let Some(torso) = shells.iter().find(|name| **name == TORSO) {
        return Ok(torso);
    }
    match shells {
        [only] => Ok(only),
        _ => Err(CommandError::new(
            ErrorKind::Refused,
            "assemble_socket_ambiguous",
            format!(
                "joint socket role `{}` resolves to {} addressable shells and none is the measured torso `{TORSO}`; available: [{}]",
                role.name(),
                shells.len(),
                shells.join(", ")
            ),
        )),
    }
}

/// The named (addressable) shells one role contributes, in assembly order.
fn named_shells<'a>(
    role: Role,
    parts: &[PlacedPart],
    owned: &[Vec<usize>],
    identities: &'a [ShellIdentity],
) -> Vec<&'a str> {
    parts
        .iter()
        .enumerate()
        .filter(|(_, part)| part.role == role)
        .flat_map(|(index, _)| owned[index].iter())
        .map(|&shell| identities[shell].name.as_str())
        .filter(|name| !name.is_empty())
        .collect()
}

fn seal_options(spec: &SealSpec) -> SealOptions {
    let defaults = SealOptions::default();
    SealOptions {
        iterations: spec.iterations.unwrap_or(defaults.iterations),
        band: spec.band.unwrap_or(defaults.band),
        max_shift: spec.max_shift,
        arc_band: spec.arc_band,
        // Placement owns the vertical seat: a per-joint drop would move every
        // declared part, not the one the manifest placed.
        head_drop: 0.0,
        smooth: spec.smooth.unwrap_or(defaults.smooth),
        weld_coincident: spec.weld_coincident.unwrap_or(defaults.weld_coincident),
    }
}

/// A part the seal cannot reach is an unplaced part, not a seam defect.
fn seal_failure(error: joint_seal::SealError) -> CommandError {
    match &error {
        joint_seal::SealError::RingTooFar { .. }
        | joint_seal::SealError::SeatUnreachable { .. } => CommandError::new(
            ErrorKind::Refused,
            "assemble_part_unplaced",
            format!("{error}; the manifest must declare the part's `rotate`/`translate`/`drop`"),
        ),
        _ => seal_error(error),
    }
}

fn write_report(path: &Path, report: &Value) -> Result<(), CommandError> {
    let text = serde_json::to_string_pretty(report).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "assemble_report",
            format!("{}: {error}", path.display()),
        )
    })?;
    std::fs::write(path, format!("{text}\n")).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "assemble_report",
            format!("{}: {error}", path.display()),
        )
    })
}

fn render(
    params: &Params,
    manifest: &Manifest,
    parts: &[PlacedPart],
    joints: &[ResolvedJoint],
    sealed: Option<&SealOutcome>,
) -> Value {
    let assembled_vertices: usize = parts.iter().map(|part| part.positions.len()).sum();
    let assembled_triangles: usize = parts.iter().map(|part| part.triangles.len()).sum();
    let (output_vertices, output_triangles) = sealed
        .map_or((assembled_vertices, assembled_triangles), |outcome| {
            (outcome.positions.len(), outcome.triangles.len())
        });
    let parts: Vec<Value> = parts
        .iter()
        .map(|part| {
            json!({
                "order": part.order,
                "role": part.role.name(),
                "path": part.path,
                "object": part.object,
                "sha256": format!("sha256:{}", part.sha256),
                "vertices": part.positions.len(),
                "triangles": part.triangles.len(),
                "vertex_offset": part.vertex_offset,
                "triangle_offset": part.triangle_offset,
                "placement": part.placement,
            })
        })
        .collect();
    let joints: Vec<Value> = joints
        .iter()
        .map(|joint| {
            json!({
                "part": joint.part.name(),
                "socket": joint.socket.name(),
                "seat": joint.seat,
                "shells": joint
                    .shells
                    .iter()
                    .map(|(part_shell, socket_shell)| json!({
                        "part": part_shell,
                        "socket": socket_shell,
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    let seal = sealed.map(|outcome| {
        let report = &outcome.report;
        json!({
            "joints": report.joints.iter().map(joint_row).collect::<Vec<_>>(),
            "components_before": report.components_before,
            "components_after": report.components_after,
            "input_digest": report.input_digest,
            "output_digest": report.output_digest,
            "vertices": outcome.positions.len(),
            "triangles": outcome.triangles.len(),
        })
    });
    json!({
        "manifest": params.manifest,
        "version": manifest.version,
        "out": params.out,
        "report": params.report,
        "units": "source",
        "frame": "source z-up (the frame body seal-joints reads and writes)",
        "uv": "absent",
        "material": "not_carried",
        "parts": parts,
        "joints": joints,
        "assembled": {
            "vertices": assembled_vertices,
            "triangles": assembled_triangles,
        },
        "output": {
            "vertices": output_vertices,
            "triangles": output_triangles,
        },
        "seal": seal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turns_are_exact_permutations() {
        let point = [1.0, 2.0, 3.0];
        // Right-handed quarter turns at -90: the axis lane is invariant.
        for (axis, expected) in [
            (Axis::X, [1.0, 3.0, -2.0]),
            (Axis::Y, [-3.0, 2.0, 1.0]),
            (Axis::Z, [2.0, -1.0, 3.0]),
        ] {
            assert_eq!(
                rotate_point(
                    point,
                    Rotate {
                        axis,
                        degrees: -90.0
                    }
                ),
                expected,
                "{} at -90",
                axis.name()
            );
        }
        assert_eq!(
            rotate_point(
                point,
                Rotate {
                    axis: Axis::X,
                    degrees: 180.0
                }
            ),
            [1.0, -2.0, -3.0]
        );
    }

    #[test]
    fn a_general_angle_keeps_the_axis_lane() {
        let turned = rotate_point(
            [1.0, 0.0, 0.0],
            Rotate {
                axis: Axis::Y,
                degrees: 90.0,
            },
        );
        assert_eq!(turned, [0.0, 0.0, -1.0]);
        let turned = rotate_point(
            [2.0, 0.0, 0.0],
            Rotate {
                axis: Axis::Z,
                degrees: 45.0,
            },
        );
        assert!((turned[0] - 2f32.sqrt()).abs() < 1e-6, "{turned:?}");
        assert!((turned[1] - 2f32.sqrt()).abs() < 1e-6, "{turned:?}");
        assert_eq!(turned[2], 0.0);
    }

    #[test]
    fn only_an_odd_negative_scale_is_a_mirror() {
        let spec = |scale: Option<[f32; 3]>| PartSpec {
            path: PathBuf::from("head.obj"),
            role: Role::Head,
            object: None,
            scale,
            rotate: None,
            translate: None,
            drop: None,
        };
        assert!(!mirrored(&spec(None)), "no scale is no mirror");
        assert!(!mirrored(&spec(Some([1.0, 1.0, 1.0]))));
        assert!(mirrored(&spec(Some([-71.4286, 71.4286, 71.4286]))));
        assert!(
            !mirrored(&spec(Some([-71.4286, -71.4286, 71.4286]))),
            "two negative lanes are a half turn, not a mirror"
        );
        assert!(!mirrored(&spec(Some([-1.0, -1.0, 1.0]))));
        let mut faces = vec![[0, 1, 2], [2, 3, 0]];
        rewind(&mut faces);
        assert_eq!(faces, vec![[0, 2, 1], [2, 0, 3]]);
    }

    #[test]
    fn scale_rotate_and_translate_apply_in_the_documented_order() {
        let mut positions = vec![[1.0f32, 0.0, 0.0]];
        let spec = PartSpec {
            path: PathBuf::from("part.obj"),
            role: Role::Head,
            object: None,
            scale: Some([2.0, 2.0, 2.0]),
            rotate: Some(Rotate {
                axis: Axis::Z,
                degrees: 90.0,
            }),
            translate: Some([1.0, 0.0, 0.0]),
            drop: None,
        };
        place(&mut positions, &spec);
        // 1 -> scale 2 -> quarter turn (x, y) -> translate.
        assert_eq!(positions, vec![[1.0, 2.0, 0.0]]);
    }

    #[test]
    fn the_drop_lands_on_the_source_up_axis() {
        let spec = PartSpec {
            path: PathBuf::from("x.obj"),
            role: Role::Head,
            object: None,
            scale: None,
            rotate: None,
            translate: Some([1.0, 2.0, 3.0]),
            drop: Some(2.0),
        };
        assert_eq!(effective_translation(&spec), [1.0, 2.0, 1.0]);
    }

    #[test]
    fn a_manifest_missing_the_body_or_its_place_is_refused() {
        let empty = Manifest {
            version: MANIFEST_VERSION,
            parts: Vec::new(),
            joints: Vec::new(),
            seal: SealSpec::default(),
        };
        assert_eq!(
            validate(&empty).expect_err("no parts").code(),
            "assemble_parts_empty"
        );
        let head_first = Manifest {
            version: MANIFEST_VERSION,
            parts: vec![part(Role::Head), part(Role::Body)],
            joints: Vec::new(),
            seal: SealSpec::default(),
        };
        assert_eq!(
            validate(&head_first).expect_err("head first").code(),
            "assemble_body_not_first"
        );
        let duplicate = Manifest {
            version: MANIFEST_VERSION,
            parts: vec![part(Role::Body), part(Role::Head), part(Role::Head)],
            joints: Vec::new(),
            seal: SealSpec::default(),
        };
        assert_eq!(
            validate(&duplicate).expect_err("duplicate head").code(),
            "assemble_role_duplicate"
        );
        let two_feet = Manifest {
            version: MANIFEST_VERSION,
            parts: vec![part(Role::Body), part(Role::Feet), part(Role::Feet)],
            joints: Vec::new(),
            seal: SealSpec::default(),
        };
        validate(&two_feet).expect("feet may repeat");
        let wrong_version = Manifest {
            version: 2,
            parts: vec![part(Role::Body)],
            joints: Vec::new(),
            seal: SealSpec::default(),
        };
        assert_eq!(
            validate(&wrong_version).expect_err("version").code(),
            "assemble_version"
        );
    }

    fn part(role: Role) -> PartSpec {
        PartSpec {
            path: PathBuf::from("part.obj"),
            role,
            object: None,
            scale: None,
            rotate: None,
            translate: None,
            drop: None,
        }
    }
}
