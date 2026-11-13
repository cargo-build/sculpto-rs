//! Default replaceable head slots per configured body (SHP-02, decision D3).
//!
//! Each configured default body carries one explicit head slot. The slot is
//! resolved from the fixture paths in `config/bodies.json`; a missing or
//! incompatible head is a named refusal, never a silent substitute. A
//! [`CompatibilityReport`] records what was checked — neck seam, rig/bone order
//! against the body's skin palette, UV set count and material names — and what
//! cannot be verified headlessly. Replacing the head swaps only the head shape:
//! body, hands and feet geometry and skin travel unchanged.
//!
//! This module owns the typed body/head settings and their validation
//! (config-first). The command surface that exposes these operations is SHP-05;
//! here they are the shared operations the acceptance tests and that later
//! surface consume.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use sculpto_core::mesh::{MeshAsset, validate_bundle};
use sculpto_core::role::{ShapeRole, validate_required_roles};

use crate::obj::{ObjError, pick_shape, read_obj_asset};
use crate::{CommandError, ErrorKind, errors};

/// One configured default body and its replaceable head slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyDescriptor {
    /// Display name, unique within the config.
    pub name: String,
    /// Body family label (for example `3bbb`, `himbo`).
    pub family: String,
    /// Body mesh source (`.obj` or `.nif`).
    pub body: PathBuf,
    /// Default head source (`.obj` or `.nif`) for this body's head slot.
    pub head: PathBuf,
}

/// The configured default bodies, each with its head slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodiesConfig {
    /// One descriptor per configured default body.
    pub bodies: Vec<BodyDescriptor>,
}

#[derive(Deserialize)]
struct RawBodies {
    bodies: Vec<RawBody>,
}

#[derive(Deserialize)]
struct RawBody {
    name: String,
    family: String,
    body: String,
    head: String,
}

/// Load and validate the body/head config, expanding `~` and `${VAR}` paths.
///
/// Private fixture roots stay out of the committed file: the config references
/// them through environment variables, and an unset variable is a named
/// refusal rather than a guessed path.
///
/// # Errors
/// Refuses unreadable or malformed JSON, empty or duplicate body names, empty
/// fields, and any `${VAR}` the environment does not define.
pub fn load_bodies_config(path: &Path) -> Result<BodiesConfig, CommandError> {
    let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
    let raw: RawBodies = serde_json::from_slice(&bytes).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "config_invalid",
            format!("{}: {error}", path.display()),
        )
    })?;
    if raw.bodies.is_empty() {
        return Err(errors::invalid("bodies", "config declares no default body"));
    }
    let mut bodies = Vec::new();
    let mut names = BTreeSet::new();
    for raw_body in raw.bodies {
        if raw_body.name.trim().is_empty() {
            return Err(errors::invalid("bodies[].name", "body name is required"));
        }
        if !names.insert(raw_body.name.clone()) {
            return Err(errors::invalid(
                "bodies[].name",
                format!("duplicate body name `{}`", raw_body.name),
            ));
        }
        if raw_body.family.trim().is_empty() {
            return Err(errors::invalid(
                "bodies[].family",
                format!("body `{}` needs a family label", raw_body.name),
            ));
        }
        let body = expand_path(&raw_body.body).map_err(|missing| {
            config_unresolved(&raw_body.name, "body", &raw_body.body, &missing)
        })?;
        let head = expand_path(&raw_body.head).map_err(|missing| {
            config_unresolved(&raw_body.name, "head", &raw_body.head, &missing)
        })?;
        bodies.push(BodyDescriptor {
            name: raw_body.name,
            family: raw_body.family,
            body,
            head,
        });
    }
    Ok(BodiesConfig { bodies })
}

fn config_unresolved(body: &str, field: &str, raw: &str, var: &str) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidParams,
        "config_unresolved",
        format!("body `{body}` {field} path `{raw}` references unset environment variable `{var}`"),
    )
}

/// Expand a leading `~` and every `${VAR}`; the first unset variable is named.
fn expand_path(raw: &str) -> Result<PathBuf, String> {
    let mut text = raw.to_string();
    if let Some(rest) = text.strip_prefix('~') {
        let home = std::env::var_os("HOME")
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_default();
        text = format!("{home}{rest}");
    }
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unterminated `${{` in `{raw}`"))?;
        let var = &after[..end];
        let value = std::env::var(var).map_err(|_| var.to_string())?;
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    if out.trim().is_empty() {
        return Err(String::new());
    }
    Ok(PathBuf::from(out))
}

/// Load one body or head source into the export model.
///
/// `.obj` keeps per-vertex UVs; `.nif` keeps skin, UVs and normals. Both land
/// in the source Z-up frame so parts assemble coherently.
///
/// # Errors
/// Refuses unreadable files, unsupported extensions and malformed geometry.
pub fn load_shape(path: &Path) -> Result<MeshAsset, CommandError> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(ext) if ext == "obj" => read_obj_asset(path).map_err(obj_error),
        Some(ext) if ext == "nif" => {
            let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
            let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
            let shape = pick_shape(&document.shapes, None)?;
            Ok(MeshAsset::from(shape))
        }
        _ => Err(errors::invalid(
            "shape",
            format!("{}: expected a .obj or .nif source", path.display()),
        )),
    }
}

fn obj_error(error: ObjError) -> CommandError {
    match error {
        ObjError::Io(message) => CommandError::new(ErrorKind::Io, "io_error", message),
        ObjError::Malformed(message) => errors::invalid("obj", message),
    }
}

/// Resolve a body's configured default head, refusing a missing file by name.
///
/// # Errors
/// [`head_missing`](CommandError) when the configured head does not exist, so a
/// missing asset never becomes a silent substitute or an untextured fallback.
pub fn resolve_default_head(descriptor: &BodyDescriptor) -> Result<MeshAsset, CommandError> {
    if !descriptor.head.exists() {
        return Err(CommandError::new(
            ErrorKind::NotFound,
            "head_missing",
            format!(
                "configured default head for body `{}` is missing: {}",
                descriptor.name,
                descriptor.head.display()
            ),
        ));
    }
    load_shape(&descriptor.head)
}

/// Outcome of one compatibility check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    /// The check ran and passed.
    Pass,
    /// The check ran and failed; the head is incompatible.
    Fail,
    /// The check cannot be verified headlessly from the loaded sources.
    NotVerifiable,
}

/// One named compatibility check and its evidence.
#[derive(Debug, Clone)]
pub struct CompatibilityCheck {
    /// Stable check name.
    pub name: &'static str,
    /// Outcome.
    pub status: CheckStatus,
    /// Human-readable evidence, including what could not be verified.
    pub detail: String,
}

/// What was checked between a head and its body, and what could not be.
#[derive(Debug, Clone)]
pub struct CompatibilityReport {
    /// Every check, in a stable order.
    pub checks: Vec<CompatibilityCheck>,
}

impl CompatibilityReport {
    /// A head is compatible when no check failed; unverifiable checks are
    /// recorded but do not by themselves refuse the head.
    pub fn is_compatible(&self) -> bool {
        !self
            .checks
            .iter()
            .any(|check| check.status == CheckStatus::Fail)
    }

    /// The failing checks, each naming its defect.
    pub fn failures(&self) -> Vec<&CompatibilityCheck> {
        self.checks
            .iter()
            .filter(|check| check.status == CheckStatus::Fail)
            .collect()
    }
}

/// Check a head against the body it will attach to.
///
/// Checks, in order: neck seam present on the head; head bone palette equal to
/// the body's skin palette in order; head UV set count; head material names.
/// Rig order and material names are reported as not verifiable when the loaded
/// sources carry no skin or no materials (OBJ geometry, or the NIF projection).
pub fn head_compatibility(body: &MeshAsset, head: &MeshAsset) -> CompatibilityReport {
    let mut checks = Vec::new();

    let (boundary, neck_ring) = neck_seam(head);
    checks.push(CompatibilityCheck {
        name: "neck_seam",
        status: if neck_ring >= 3 {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: format!(
            "{boundary} boundary vertices, {neck_ring} in the neck ring at the head's base"
        ),
    });

    let rig = match (body.skin.as_ref(), head.skin.as_ref()) {
        (Some(body_skin), Some(head_skin)) => {
            if body_skin.bones == head_skin.bones {
                CompatibilityCheck {
                    name: "rig_bone_order",
                    status: CheckStatus::Pass,
                    detail: format!(
                        "{} bones match the body palette order",
                        head_skin.bones.len()
                    ),
                }
            } else {
                CompatibilityCheck {
                    name: "rig_bone_order",
                    status: CheckStatus::Fail,
                    detail: format!(
                        "head palette {:?} differs from body palette {:?}",
                        head_skin.bones, body_skin.bones
                    ),
                }
            }
        }
        (body_skin, head_skin) => CompatibilityCheck {
            name: "rig_bone_order",
            status: CheckStatus::NotVerifiable,
            detail: format!(
                "cannot compare bone order headlessly: body skin {}, head skin {}",
                presence(body_skin.is_some()),
                presence(head_skin.is_some())
            ),
        },
    };
    checks.push(rig);

    let uv_sets = usize::from(!head.uvs.is_empty());
    checks.push(CompatibilityCheck {
        name: "uv_set_count",
        status: if uv_sets >= 1 {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: format!("{uv_sets} UV set(s), {} head UVs", head.uvs.len()),
    });

    let material_names: Vec<&str> = head
        .materials
        .iter()
        .map(|slot| slot.name.as_str())
        .collect();
    checks.push(CompatibilityCheck {
        name: "material_names",
        status: if material_names.is_empty() {
            CheckStatus::NotVerifiable
        } else {
            CheckStatus::Pass
        },
        detail: if material_names.is_empty() {
            "no material names exposed by the reader; texture assignment cannot be verified headlessly"
                .to_string()
        } else {
            format!("materials: {}", material_names.join(", "))
        },
    });

    CompatibilityReport { checks }
}

fn presence(present: bool) -> &'static str {
    if present {
        "present"
    } else {
        "absent (OBJ geometry carries no skin)"
    }
}

/// Boundary-vertex count and the neck-ring count at the head's base.
///
/// A boundary vertex lies on an edge used by exactly one triangle. The neck
/// ring is the boundary band in the lowest 15% of the head's Z extent (source
/// Z-up frame); a watertight head has no seam to attach.
fn neck_seam(head: &MeshAsset) -> (usize, usize) {
    let mut edge_count: std::collections::BTreeMap<(u32, u32), u32> =
        std::collections::BTreeMap::new();
    for triangle in &head.triangles {
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            let edge = if a < b { (a, b) } else { (b, a) };
            *edge_count.entry(edge).or_insert(0) += 1;
        }
    }
    let mut boundary = BTreeSet::new();
    for ((a, b), count) in edge_count {
        if count == 1 {
            boundary.insert(a);
            boundary.insert(b);
        }
    }
    if boundary.is_empty() || head.positions.is_empty() {
        return (0, 0);
    }
    let zmin = head
        .positions
        .iter()
        .map(|p| p[2])
        .fold(f32::INFINITY, f32::min);
    let zmax = head
        .positions
        .iter()
        .map(|p| p[2])
        .fold(f32::NEG_INFINITY, f32::max);
    let band = zmin + 0.15 * (zmax - zmin);
    let neck_ring = boundary
        .iter()
        .filter(|&&vertex| head.positions[vertex as usize][2] <= band)
        .count();
    (boundary.len(), neck_ring)
}

/// One shape together with the role it fills in the character.
#[derive(Debug, Clone)]
pub struct AssignedShape {
    /// The part this shape fills.
    pub role: ShapeRole,
    /// The shape geometry and rig.
    pub mesh: MeshAsset,
}

/// A character bundle with an explicit role per shape and one head slot.
#[derive(Debug, Clone)]
pub struct RoleBundle {
    /// Shapes in assembly order, each with its role.
    pub shapes: Vec<AssignedShape>,
}

impl RoleBundle {
    /// Validate role uniqueness and the SHP-01 bundle gate, then keep the
    /// assignment.
    ///
    /// The required-part contract is a separate gate
    /// ([`Self::validate_required_parts`]): a head slot can be resolved and
    /// replaced on a body+head bundle before the hands and feet are assembled.
    ///
    /// # Errors
    /// Names a repeated role, or the first bundle defect (duplicate shape
    /// name, palette, bone-list mismatch).
    pub fn new(shapes: Vec<AssignedShape>) -> Result<Self, CommandError> {
        let roles: Vec<ShapeRole> = shapes.iter().map(|shape| shape.role).collect();
        let mut seen = BTreeSet::new();
        for role in &roles {
            if !seen.insert(*role) {
                return Err(CommandError::new(
                    ErrorKind::Refused,
                    "duplicate_role",
                    format!("role `{role}` is assigned more than once"),
                ));
            }
        }
        let meshes: Vec<MeshAsset> = shapes.iter().map(|shape| shape.mesh.clone()).collect();
        validate_bundle(&meshes).map_err(errors::bundle)?;
        Ok(Self { shapes })
    }

    /// Gate the required-part contract: body, hands, feet and head must all be
    /// present before the bundle is a complete character.
    ///
    /// # Errors
    /// [`missing_required_roles`](CommandError) names every absent part.
    pub fn validate_required_parts(&self) -> Result<(), CommandError> {
        let roles: Vec<ShapeRole> = self.shapes.iter().map(|shape| shape.role).collect();
        validate_required_roles(&roles).map_err(errors::role)
    }

    /// The current head shape, when the bundle has a head slot.
    pub fn head(&self) -> Option<&MeshAsset> {
        self.shapes
            .iter()
            .find(|shape| shape.role == ShapeRole::Head)
            .map(|shape| &shape.mesh)
    }

    /// Replace the head through its explicit role, preserving every other
    /// shape's geometry and rig exactly.
    ///
    /// The compatibility report gates the swap: any failing check is a named
    /// refusal. The new head is stamped with the head role's default partition;
    /// body, hands, feet and garments are cloned unchanged.
    ///
    /// # Errors
    /// Refuses an incompatible head (naming each failed check), a bundle with
    /// no head slot, or a resulting bundle that fails validation.
    pub fn replace_head(
        &self,
        head: MeshAsset,
        report: &CompatibilityReport,
    ) -> Result<RoleBundle, CommandError> {
        if !report.is_compatible() {
            let failures: Vec<String> = report
                .failures()
                .iter()
                .map(|check| format!("{}: {}", check.name, check.detail))
                .collect();
            return Err(CommandError::new(
                ErrorKind::Refused,
                "head_incompatible",
                format!("head refused for body slot: {}", failures.join("; ")),
            ));
        }
        let mut shapes = Vec::new();
        let mut replaced = false;
        for shape in &self.shapes {
            if shape.role == ShapeRole::Head {
                let mut mesh = head.clone();
                mesh.triangle_partitions =
                    vec![ShapeRole::Head.default_partition(); mesh.triangles.len()];
                shapes.push(AssignedShape {
                    role: ShapeRole::Head,
                    mesh,
                });
                replaced = true;
            } else {
                shapes.push(shape.clone());
            }
        }
        if !replaced {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "head_slot_absent",
                "bundle has no head slot to replace",
            ));
        }
        RoleBundle::new(shapes)
    }
}
