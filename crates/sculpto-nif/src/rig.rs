//! Raw source capture: the imported node graph, one placement and skin context
//! per shape, and the Y-up to source Z-up basis rule.
//!
//! [`capture_graph`] turns the importer's node list into the neutral [`Rig`].
//! [`capture_shape`] keeps object placement and the source global-to-skin
//! matrix `G` separate. [`NifDocument::bind_rest`] checks the captured
//! source data through the neutral rest proof.
//!
//! Joint ids are the imported node indexes, empty source names are captured as
//! unnamed, and `NifDocument::sources` is parallel to `NifDocument::shapes`.
//!
//! @req DECL0008

use std::fmt;

use byroredux_nif::import::{ImportedMesh, ImportedNode};
use sculpto_core::rig::rest::{RestBindError, RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigError, RigFrame, TransformDefect};

use crate::{NifDocument, NifError};

/// Raw source facts for one shape: where its object node sits and which skin
/// factors the source carried.
///
/// Positional: `NifDocument::sources[i]` describes `NifDocument::shapes[i]`.
/// Raw and unbound: [`Self::local`] is the object node's own transform, not a
/// skeleton placement, and [`Self::global_skin`] is the source `NiSkinData`
/// skin transform (`G`, global to skin) after the basis change below. Neither
/// value has been applied to a vertex or combined with an inverse bind.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeSource {
    /// Imported node the shape hangs under, when the file has one.
    pub parent: Option<JointId>,
    /// The shape object node's own local transform, in source Z-up.
    pub local: LocalTransform,
    /// Source `G` for a skinned shape, verbatim after the basis change.
    ///
    /// `None` for an unskinned shape, which is different from a skinned shape
    /// whose source `G` is the identity.
    pub global_skin: Option<[[f32; 4]; 4]>,
    /// Source skin-instance skeleton-root name, when the file resolves one.
    pub skeleton_root: Option<String>,
}

impl NifDocument {
    /// Resolve the source skeleton and prove rest reconstruction for every skin.
    ///
    /// Uses authored skin-local positions and G. Skinned object placement
    /// remains part of the source record. @req DECL0008
    pub fn bind_rest(&self) -> Result<RestBinding<'_>, NifRigError> {
        let rig = self.rig.as_ref().ok_or(NifRigError::MissingRig)?;
        self.bind_rest_on(rig)
    }

    /// Prove the authored skins against a supplied completed family rig.
    ///
    /// # Errors
    /// Refuses missing source records, unresolved skeleton roots and any
    /// frame, palette or rest mismatch reported by the checked binding.
    pub fn bind_rest_on<'a>(&'a self, rig: &'a Rig) -> Result<RestBinding<'a>, NifRigError> {
        if self.shapes.len() != self.sources.len() {
            return Err(NifRigError::SourceCount {
                shapes: self.shapes.len(),
                sources: self.sources.len(),
            });
        }
        let sources = self
            .shapes
            .iter()
            .zip(&self.sources)
            .enumerate()
            .filter_map(|(index, (shape, source))| {
                shape.skin.as_ref().map(|skin| (index, shape, source, skin))
            })
            .map(|(index, shape, source, skin)| {
                let root = source
                    .skeleton_root
                    .as_deref()
                    .ok_or(NifRigError::MissingSource {
                        shape: index,
                        field: "skeleton root",
                    })?;
                let global_to_skin = source.global_skin.ok_or(NifRigError::MissingSource {
                    shape: index,
                    field: "global-to-skin matrix",
                })?;
                let root = rig.joint_by_name(root).map_err(|error| NifRigError::Root {
                    shape: index,
                    error,
                })?;
                Ok(SkinRestSource {
                    shape: index,
                    root,
                    positions: &shape.positions,
                    skin,
                    global_to_skin,
                })
            })
            .collect::<Result<Vec<_>, NifRigError>>()?;
        RestBinding::prove(rig, RigFrame::SourceZUp, &sources).map_err(NifRigError::Rest)
    }
}

/// A source NIF did not contain enough consistent data for checked binding.
#[derive(Debug, Clone, PartialEq)]
pub enum NifRigError {
    /// The file has no authored graph.
    MissingRig,
    /// There is no one-to-one shape-to-source mapping.
    SourceCount { shapes: usize, sources: usize },
    /// A skinned shape lacks required source context.
    MissingSource { shape: usize, field: &'static str },
    /// A required skeleton root could not be resolved.
    Root { shape: usize, error: RigError },
    /// The rest proof refused the authored binding.
    Rest(RestBindError),
}

impl fmt::Display for NifRigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRig => write!(formatter, "NIF has no source rig"),
            Self::SourceCount { shapes, sources } => write!(
                formatter,
                "{shapes} NIF shapes have {sources} source records"
            ),
            Self::MissingSource { shape, field } => {
                write!(formatter, "shape {shape} has no {field}")
            }
            Self::Root { shape, error } => {
                write!(formatter, "shape {shape} skeleton root: {error}")
            }
            Self::Rest(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for NifRigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Root { error, .. } => Some(error),
            Self::Rest(error) => Some(error),
            _ => None,
        }
    }
}

/// One imported node reduced to the fields the capture consumes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NodeView<'a> {
    pub(crate) name: Option<&'a str>,
    pub(crate) translation: [f32; 3],
    pub(crate) rotation: [f32; 4],
    pub(crate) scale: f32,
    pub(crate) parent: Option<usize>,
}

impl<'a> From<&'a ImportedNode> for NodeView<'a> {
    fn from(node: &'a ImportedNode) -> Self {
        Self {
            name: node.name.as_deref(),
            translation: node.translation,
            rotation: node.rotation,
            scale: node.scale,
            parent: node.parent_node,
        }
    }
}

/// One imported shape reduced to the fields the capture consumes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeshView<'a> {
    pub(crate) name: &'a str,
    pub(crate) translation: [f32; 3],
    pub(crate) rotation: [f32; 4],
    pub(crate) scale: f32,
    pub(crate) parent: Option<usize>,
    pub(crate) skin: Option<MeshSkinView<'a>>,
}

/// The skin fields one imported shape carries into the raw capture.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeshSkinView<'a> {
    pub(crate) skeleton_root: Option<&'a str>,
    pub(crate) global_skin: &'a [[f32; 4]; 4],
}

impl<'a> From<&'a ImportedMesh> for MeshView<'a> {
    fn from(mesh: &'a ImportedMesh) -> Self {
        Self {
            name: mesh.name.as_deref().unwrap_or_default(),
            translation: mesh.translation,
            rotation: mesh.rotation,
            scale: mesh.scale,
            parent: mesh.parent_node,
            skin: mesh.skin.as_ref().map(|skin| MeshSkinView {
                skeleton_root: skin.skeleton_root.as_deref(),
                global_skin: &skin.global_skin_transform,
            }),
        }
    }
}

/// Capture the complete imported node graph, or `None` when the file has none.
///
/// Every node is kept, including unnamed and non-palette ancestors. A parent
/// index, a cycle or a rest transform the neutral [`Rig`] cannot represent is a
/// contextual refusal; it is never replaced by an identity.
pub(crate) fn capture_graph(nodes: &[NodeView<'_>]) -> Result<Option<Rig>, NifError> {
    if nodes.is_empty() {
        return Ok(None);
    }
    let joints = nodes.iter().map(joint).collect::<Result<Vec<_>, _>>()?;
    Rig::new(joints, RigFrame::SourceZUp)
        .map(Some)
        .map_err(graph_error)
}

/// Capture one shape's object placement and skin context.
///
/// `nodes` is the captured graph length, so a parent index that addresses no
/// node is refused here rather than carried into a later stage.
pub(crate) fn capture_shape(mesh: &MeshView<'_>, nodes: usize) -> Result<ShapeSource, NifError> {
    let parent = match mesh.parent {
        Some(index) if index >= nodes => {
            return Err(NifError::Malformed(format!(
                "shape `{}`: parent node {index} is outside the {nodes} imported nodes",
                mesh.name
            )));
        }
        Some(index) => Some(JointId::new(node_id(index)?)),
        None => None,
    };
    let local = yup_transform_to_zup(mesh.translation, mesh.rotation, mesh.scale);
    local
        .validate()
        .map_err(|defect| placement_error(mesh.name, defect))?;
    let (global_skin, skeleton_root) = match &mesh.skin {
        Some(skin) => {
            let global_skin = yup_matrix_to_zup(skin.global_skin);
            match global_skin.iter().flatten().all(|value| value.is_finite()) {
                true => {}
                false => {
                    return Err(NifError::Malformed(format!(
                        "shape `{}`: global skin transform is not finite",
                        mesh.name
                    )));
                }
            }
            (Some(global_skin), skin.skeleton_root.map(str::to_owned))
        }
        None => (None, None),
    };
    Ok(ShapeSource {
        parent,
        local,
        global_skin,
        skeleton_root,
    })
}

/// Convert a column-major Y-up matrix back to the source Z-up frame.
///
/// The importer applies the basis change `C` (`(x, y, z) -> (x, z, -y)`), so the
/// source frame is recovered with `C^T * M * C`. Used for every per-bone inverse
/// bind and for the per-shape global skin transform, so both keep one basis rule.
pub fn yup_matrix_to_zup(matrix: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let right = [
        matrix[0],
        [-matrix[2][0], -matrix[2][1], -matrix[2][2], -matrix[2][3]],
        matrix[1],
        matrix[3],
    ];
    right.map(|column| [column[0], -column[2], column[1], column[3]])
}

/// Convert an imported Y-up local transform to the source Z-up frame.
///
/// The translation follows `(x, y, z) -> (x, -z, y)` and the rotation is
/// conjugated by the basis change, `q_zup = q_C^-1 * q_yup * q_C`, so the
/// uniform scale passes through unchanged. The result is not validated here;
/// callers validate, because a refusal has to name its shape or joint.
pub fn yup_transform_to_zup(
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: f32,
) -> LocalTransform {
    LocalTransform {
        translation: [translation[0], -translation[2], translation[1]],
        // Conjugating by this fixed basis permutes the quaternion's vector
        // part exactly; two f32 Hamilton products would perturb even identity.
        rotation: [rotation[0], -rotation[2], rotation[1], rotation[3]],
        scale,
    }
}

/// One joint from one imported node; the joint id is the imported node index.
fn joint(node: &NodeView<'_>) -> Result<Joint, NifError> {
    Ok(Joint {
        name: node.name.map(str::to_owned).filter(|name| !name.is_empty()),
        parent: match node.parent {
            Some(parent) => Some(JointId::new(node_id(parent)?)),
            None => None,
        },
        rest: yup_transform_to_zup(node.translation, node.rotation, node.scale),
    })
}

/// Name a node index as a joint id, refusing an index a joint id cannot address.
fn node_id(index: usize) -> Result<u32, NifError> {
    u32::try_from(index).map_err(|_| {
        NifError::Malformed(format!("imported node {index} is above the joint id range"))
    })
}

fn graph_error(error: RigError) -> NifError {
    NifError::Malformed(format!("imported node graph: {error}"))
}

fn placement_error(shape: &str, defect: TransformDefect) -> NifError {
    NifError::Malformed(format!("shape `{shape}`: placement is invalid: {defect}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_1_SQRT_2;

    const FRAC: f32 = FRAC_1_SQRT_2;

    fn node(name: Option<&str>, parent: Option<usize>) -> NodeView<'_> {
        NodeView {
            name,
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
            parent,
        }
    }

    fn mesh<'a>(skin: Option<MeshSkinView<'a>>) -> MeshView<'a> {
        MeshView {
            name: "shape",
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
            parent: None,
            skin,
        }
    }

    fn close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "{what}: {actual} != {expected}"
        );
    }

    fn assert_quaternion(actual: [f32; 4], expected: [f32; 4]) {
        let same = actual
            .iter()
            .zip(expected)
            .all(|(left, right)| (left - right).abs() < 1e-6);
        let flipped = actual
            .iter()
            .zip(expected)
            .all(|(left, right)| (left + right).abs() < 1e-6);
        assert!(
            same || flipped,
            "quaternion {actual:?} is neither {expected:?} nor its negation"
        );
    }

    #[test]
    fn a_yup_translation_moves_into_source_axes() {
        let transform = yup_transform_to_zup([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], 2.0);
        assert_eq!(transform.translation, [1.0, -3.0, 2.0]);
        assert_eq!(transform.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(transform.scale, 2.0);
        assert_eq!(transform.validate(), Ok(()));
    }

    #[test]
    fn a_yup_quarter_turn_about_y_becomes_a_z_turn() {
        // Y-up is the source Z axis, so +90° about Y-up is +90° about source Z.
        let transform = yup_transform_to_zup([0.0; 3], [0.0, FRAC, 0.0, FRAC], 1.0);
        assert_quaternion(transform.rotation, [0.0, 0.0, FRAC, FRAC]);
        assert_eq!(transform.validate(), Ok(()));
    }

    #[test]
    fn the_basis_change_round_trips() {
        let source = [0.3, 0.4, 0.0, 0.866_025_4_f32];
        // Paint the same physical rotation in Y-up, then convert it back.
        let yup = [0.3, 0.0, -0.4, 0.866_025_4_f32];
        let transform = yup_transform_to_zup([0.0; 3], yup, 1.0);
        assert_quaternion(transform.rotation, source);
        assert_eq!(transform.validate(), Ok(()));
    }

    #[test]
    fn the_identity_transform_stays_identity() {
        let transform = yup_transform_to_zup([0.0; 3], [0.0, 0.0, 0.0, 1.0], 1.0);
        assert_eq!(transform, LocalTransform::default());
    }

    #[test]
    fn a_general_transform_matches_matrix_basis_conversion() {
        let yup = LocalTransform {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.3, 0.0, -0.4, 0.866_025_4],
            scale: 2.0,
        };
        let expected = yup_matrix_to_zup(
            &yup.matrix()
                .expect("valid source")
                .map(|column| column.map(|value| value as f32)),
        );
        let actual = yup_transform_to_zup(yup.translation, yup.rotation, yup.scale)
            .matrix()
            .expect("valid conversion");
        actual
            .iter()
            .flatten()
            .zip(expected.iter().flatten())
            .for_each(|(actual, expected)| {
                assert!((actual - f64::from(*expected)).abs() < 1e-6);
            });
    }

    #[test]
    fn a_yup_matrix_swap_matches_the_documented_basis() {
        // A pure +5 translation up the Y-up axis is +5 along source Z.
        let mut matrix = [[0.0f32; 4]; 4];
        matrix[0][0] = 1.0;
        matrix[1][1] = 1.0;
        matrix[2][2] = 1.0;
        matrix[3] = [0.0, 5.0, 0.0, 1.0];

        let source = yup_matrix_to_zup(&matrix);
        assert_eq!([source[3][0], source[3][1], source[3][2]], [0.0, 0.0, 5.0]);
        source
            .iter()
            .take(3)
            .enumerate()
            .for_each(|(axis, column)| {
                column.iter().take(3).enumerate().for_each(|(lane, value)| {
                    let expected = f32::from(u8::from(axis == lane));
                    close(*value, expected, "rotation part stays identity");
                });
            });
    }

    #[test]
    fn graph_keeps_unnamed_ancestors_and_orders_parents_first() {
        // The parent has a higher index than one of its children, so the order
        // cannot come from the imported sequence.
        let nodes = [
            node(None, Some(1)),
            node(None, None),
            node(Some("spine"), Some(1)),
        ];
        let rig = capture_graph(&nodes)
            .expect("a valid graph")
            .expect("three imported nodes are a graph");

        assert_eq!(rig.joints().len(), 3);
        assert_eq!(rig.roots(), &[JointId::new(1)]);
        assert!(rig.joint(JointId::new(0)).expect("child").name.is_none());
        assert_eq!(
            rig.joint(JointId::new(2)).expect("spine").name.as_deref(),
            Some("spine")
        );
        let order: Vec<usize> = rig
            .topological_order()
            .iter()
            .map(|joint| joint.index())
            .collect();
        assert_eq!(order[0], 1, "the root is placed before its children");
        assert!(order.iter().position(|id| *id == 1) < order.iter().position(|id| *id == 0));
        assert!(order.iter().position(|id| *id == 1) < order.iter().position(|id| *id == 2));
    }

    #[test]
    fn an_empty_node_name_is_captured_as_unnamed() {
        let nodes = [node(Some(""), None)];
        let rig = capture_graph(&nodes)
            .expect("an empty NIF name denotes an unnamed node")
            .expect("one imported node is a graph");
        assert!(rig.joint(JointId::new(0)).expect("joint").name.is_none());
    }

    #[test]
    fn a_whitespace_node_name_is_refused() {
        let error = capture_graph(&[node(Some("   "), None)]).expect_err("invalid authored name");
        assert!(matches!(error, NifError::Malformed(detail) if detail.contains("blank name")));
    }

    #[test]
    fn no_imported_nodes_capture_no_graph() {
        assert_eq!(capture_graph(&[]), Ok(None));
    }

    #[test]
    fn a_malformed_parent_is_refused() {
        let nodes = [node(None, Some(7))];
        let error = capture_graph(&nodes).expect_err("a dangling parent is invalid");
        assert!(
            matches!(&error, NifError::Malformed(detail) if detail.starts_with("imported node graph:")),
            "{error:?}"
        );
    }

    #[test]
    fn a_malformed_transform_is_refused() {
        let mut zero_scale = node(None, None);
        zero_scale.scale = 0.0;
        let error = capture_graph(&[zero_scale]).expect_err("a zero scale is invalid");
        assert!(
            matches!(&error, NifError::Malformed(detail) if detail.contains("scale 0 is not positive")),
            "{error:?}"
        );

        let mut zero_rotation = node(None, None);
        zero_rotation.rotation = [0.0, 0.0, 0.0, 0.0];
        let error = capture_graph(&[zero_rotation]).expect_err("a zero quaternion is invalid");
        assert!(
            matches!(&error, NifError::Malformed(detail) if detail.contains("rotation length squared")),
            "{error:?}"
        );
        assert_eq!(
            yup_transform_to_zup([0.0; 3], [0.0, 0.0, 0.0, 0.0], 1.0).validate(),
            Err(TransformDefect::RotationNotUnit {
                length_squared: 0.0
            })
        );
    }

    #[test]
    fn a_parent_cycle_is_refused() {
        let nodes = [node(None, Some(1)), node(None, Some(0))];
        let error = capture_graph(&nodes).expect_err("a cycle is invalid");
        assert!(
            matches!(&error, NifError::Malformed(detail) if detail.contains("parent cycle")),
            "{error:?}"
        );
    }

    #[test]
    fn shape_capture_keeps_placement_and_global_skin_apart() {
        let mut placement = mesh(None);
        placement.translation = [1.0, 2.0, 3.0];
        placement.rotation = [0.0, FRAC, 0.0, FRAC];
        placement.parent = Some(0);

        let mut global = [[0.0f32; 4]; 4];
        global[0][0] = 1.0;
        global[1][1] = 1.0;
        global[2][2] = 1.0;
        global[3] = [0.0, 0.0, 4.0, 1.0];
        let mut skinned = mesh(Some(MeshSkinView {
            skeleton_root: Some("NPC Root"),
            global_skin: &global,
        }));
        skinned.translation = [9.0, 8.0, 7.0];

        let source = capture_shape(&placement, 1).expect("a placement inside the graph");
        assert_eq!(source.parent, Some(JointId::new(0)));
        assert_eq!(source.local.translation, [1.0, -3.0, 2.0]);
        assert_eq!(source.global_skin, None);
        assert_eq!(source.skeleton_root, None);

        let source = capture_shape(&skinned, 1).expect("an unparented shape");
        assert_eq!(source.parent, None);
        assert_eq!(source.local.translation, [9.0, -7.0, 8.0]);
        let captured = source.global_skin.expect("a skinned shape carries G");
        assert_eq!(
            [captured[3][0], captured[3][1], captured[3][2]],
            [0.0, -4.0, 0.0]
        );
        assert_ne!(captured, global, "G is converted, not copied band-for-band");
        assert_eq!(source.skeleton_root.as_deref(), Some("NPC Root"));
    }

    #[test]
    fn shape_capture_refuses_a_parent_outside_the_graph() {
        let mut outside = mesh(None);
        outside.parent = Some(3);
        let error = capture_shape(&outside, 2).expect_err("a dangling parent is invalid");
        assert!(
            matches!(&error, NifError::Malformed(detail) if detail.contains("outside the 2 imported nodes")),
            "{error:?}"
        );
    }

    #[test]
    fn shape_capture_refuses_an_invalid_placement() {
        let mut broken = mesh(None);
        broken.scale = 0.0;
        let error = capture_shape(&broken, 1).expect_err("a zero scale is invalid");
        assert!(
            matches!(&error, NifError::Malformed(detail) if detail.contains("placement is invalid")),
            "{error:?}"
        );
    }

    #[test]
    fn shape_capture_refuses_a_non_finite_global_skin() {
        let mut global = [[0.0f32; 4]; 4];
        global[3][1] = f32::NAN;
        let broken = mesh(Some(MeshSkinView {
            skeleton_root: None,
            global_skin: &global,
        }));
        let error = capture_shape(&broken, 1).expect_err("a non-finite G is invalid");
        assert!(
            matches!(&error, NifError::Malformed(detail) if detail.contains("global skin transform is not finite")),
            "{error:?}"
        );
    }
}
