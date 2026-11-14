//! Plan scene nodes once for both native targets. @req DECL0008

use sculpto_core::rig::rest::{BoundRestSkin, RestBinding};
use sculpto_core::rig::{JointId, Rig};

use crate::ShapeSource;

use super::header::NifWriteError;
use super::skin;
use super::strings::StringTable;

pub(super) struct CheckedScene<'a> {
    rig: &'a Rig,
    skin: &'a BoundRestSkin<'a>,
    parent: JointId,
    basis: NativeTransform,
    pub(super) placement: NativeTransform,
}

impl<'a> CheckedScene<'a> {
    pub(super) fn new(
        binding: &'a RestBinding<'_>,
        source: &ShapeSource,
        shape: usize,
    ) -> Result<Self, NifWriteError> {
        let fail = |reason| NifWriteError::RigLayout { shape, reason };
        let rig = binding.rig();
        let skin = binding
            .skins()
            .iter()
            .find(|skin| skin.source().shape == shape)
            .ok_or_else(|| fail("a checked shape needs a source skin"))?;
        let parent = source
            .parent
            .filter(|&parent| rig.joint(parent).is_some())
            .ok_or_else(|| fail("the source shape has no resolved parent"))?;
        let basis = NativeTransform::from_matrix(binding.basis()).ok_or_else(|| {
            fail("the common rest basis needs a positive uniform native transform")
        })?;
        let local = source
            .local
            .matrix()
            .map_err(|_| fail("the source shape placement is invalid"))?;
        let placement = NativeTransform::from_matrix(local)
            .ok_or_else(|| fail("the source shape placement exceeds native precision"))?;
        Ok(Self {
            rig,
            skin,
            parent,
            basis,
            placement,
        })
    }

    pub(super) fn global_skin(&self) -> [[f32; 4]; 4] {
        self.skin.source().global_to_skin
    }

    fn plan(&self, start: i32, strings: &mut StringTable) -> Result<SkinNodes, NifWriteError> {
        let scene_parent = self.basis != NativeTransform::IDENTITY || self.rig.roots().len() > 1;
        let first_joint = start + i32::from(scene_parent);
        let order = self.rig.topological_order();
        let count = i32::try_from(order.len())
            .ok()
            .filter(|count| *count <= i32::MAX - first_joint - 3)
            .ok_or(NifWriteError::RigLayout {
                shape: self.skin.source().shape,
                reason: "too many joint blocks for native references",
            })?;
        let mut references = vec![-1; count as usize];
        for (index, id) in order.iter().enumerate() {
            references[id.index()] = first_joint + index as i32;
        }
        let block = |id: JointId| references[id.index()];
        let mut nodes = Vec::with_capacity(count as usize + usize::from(scene_parent));
        if scene_parent {
            nodes.push(Node {
                name: -1,
                local: self.basis,
                children: self.rig.roots().iter().copied().map(block).collect(),
            });
        }
        for &id in order {
            let joint = &self.rig.joints()[id.index()];
            let local = joint
                .rest
                .matrix()
                .ok()
                .and_then(NativeTransform::from_matrix)
                .ok_or(NifWriteError::RigLayout {
                    shape: self.skin.source().shape,
                    reason: "a joint local exceeds native transform precision",
                })?;
            let name = joint
                .name
                .as_deref()
                .map_or(-1, |name| strings.intern(name) as i32);
            nodes.push(Node {
                name,
                local,
                children: Vec::new(),
            });
            if let Some(parent) = joint.parent {
                nodes[(block(parent) - start) as usize]
                    .children
                    .push(block(id));
            }
        }
        nodes[(block(self.parent) - start) as usize]
            .children
            .push(0);
        Ok(SkinNodes {
            root: block(self.skin.source().root),
            bones: self.skin.palette().iter().copied().map(block).collect(),
            nodes,
        })
    }
}

pub(super) struct SkinNodes {
    pub(super) root: i32,
    pub(super) bones: Vec<i32>,
    nodes: Vec<Node>,
}

impl SkinNodes {
    pub(super) fn plan(
        checked: Option<&CheckedScene<'_>>,
        names: &[String],
        start: i32,
        strings: &mut StringTable,
    ) -> Result<Self, NifWriteError> {
        if let Some(checked) = checked {
            return checked.plan(start, strings);
        }
        let bones = (0..names.len())
            .map(|index| start + 1 + index as i32)
            .collect::<Vec<_>>();
        // The scene root owns the shape and every bone node: a skin instance's
        // bone refs are names, but native tools clone the graph by walking
        // children, so an unattached bone node loses its palette on re-export.
        let mut children = vec![0];
        children.extend(&bones);
        let mut nodes = vec![Node {
            name: -1,
            local: NativeTransform::IDENTITY,
            children,
        }];
        nodes.extend(names.iter().map(|name| Node {
            name: strings.intern(name) as i32,
            local: NativeTransform::IDENTITY,
            children: Vec::new(),
        }));
        Ok(Self {
            root: start,
            bones,
            nodes,
        })
    }

    pub(super) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(super) fn encode(&self, root_extra: &[i32]) -> Vec<Vec<u8>> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let extra = if index == 0 { root_extra } else { &[] };
                skin::node(node.name, &node.children, extra, node.local)
            })
            .collect()
    }
}

struct Node {
    name: i32,
    local: NativeTransform,
    children: Vec<i32>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct NativeTransform {
    translation: [f32; 3],
    rotation: [[f32; 3]; 3],
    scale: f32,
}

impl NativeTransform {
    pub(super) const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        scale: 1.0,
    };

    fn from_matrix(matrix: [[f64; 4]; 4]) -> Option<Self> {
        if matrix
            .into_iter()
            .zip([0.0, 0.0, 0.0, 1.0])
            .any(|(column, expected)| (column[3] - expected).abs() > 1e-12)
            || !matrix.iter().flatten().all(|value| value.is_finite())
        {
            return None;
        }
        let scale = matrix[0][0].hypot(matrix[0][1]).hypot(matrix[0][2]);
        let rotation = [0, 1, 2].map(|row| [0, 1, 2].map(|column| matrix[column][row] / scale));
        let dot = |a: [f64; 3], b: [f64; 3]| a.into_iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
        let [x, y, z] = rotation;
        let cross = [
            x[1] * y[2] - x[2] * y[1],
            x[2] * y[0] - x[0] * y[2],
            x[0] * y[1] - x[1] * y[0],
        ];
        let residuals = [
            dot(x, x) - 1.0,
            dot(y, y) - 1.0,
            dot(z, z) - 1.0,
            dot(x, y),
            dot(x, z),
            dot(y, z),
            dot(cross, z) - 1.0,
        ];
        let translation = [0, 1, 2].map(|axis| matrix[3][axis] as f32);
        let scale = scale as f32;
        if scale <= 0.0
            || !scale.is_finite()
            || !translation.iter().all(|value| value.is_finite())
            || !residuals.iter().all(|value| value.abs() <= 1e-5)
        {
            return None;
        }
        Some(Self {
            translation,
            rotation: rotation.map(|row| row.map(|value| value as f32)),
            scale,
        })
    }

    pub(super) fn write_av(self, bytes: &mut Vec<u8>) {
        for value in self
            .translation
            .into_iter()
            .chain(self.rotation.into_iter().flatten())
            .chain([self.scale])
        {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
}
