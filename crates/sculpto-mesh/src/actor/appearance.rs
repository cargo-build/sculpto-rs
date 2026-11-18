//! Read continuous rendering attributes for barycentric surface cuts without crossing UV charts.
use super::{Actor, ActorError, invalid, unsupported};
use gltf::Semantic;

/// Decoded per-vertex data in shader order. The extra lanes are padding.
pub struct SurfaceAttribute {
    pub semantic: String,
    pub width: usize,
    pub values: Vec<[f32; 4]>,
}

impl Actor {
    fn surface_primitive(
        &self,
        node: usize,
        part: usize,
    ) -> Result<gltf::Primitive<'_>, ActorError> {
        self.document
            .nodes()
            .nth(node)
            .and_then(|n| n.mesh())
            .and_then(|m| m.primitives().nth(part))
            .ok_or_else(|| invalid("surface attributes", "missing mesh or primitive"))
    }

    /// Read encoded bind positions so a partial repair can copy untouched rows exactly. @req DECL0013
    pub fn bind_positions(&self, node: usize, part: usize) -> Result<Vec<[f32; 3]>, ActorError> {
        let p = self.surface_primitive(node, part)?;
        Ok(p.reader(|b| self.buffer(b))
            .read_positions()
            .ok_or_else(|| invalid("bind positions", "unreadable"))?
            .collect())
    }

    /// Return normals, tangents, UV sets and vertex colors of a selected primitive. @req DECL0013
    pub fn surface_attributes(
        &self,
        node: usize,
        part: usize,
    ) -> Result<Vec<SurfaceAttribute>, ActorError> {
        let p = self.surface_primitive(node, part)?;
        let reader = p.reader(|b| self.buffer(b));
        let count = p
            .get(&Semantic::Positions)
            .ok_or_else(|| invalid("surface attributes", "missing position"))?
            .count();
        let mut out = Vec::new();
        for (s, _) in p.attributes() {
            let (semantic, width, values): (String, usize, Vec<[f32; 4]>) = match s {
                Semantic::Positions | Semantic::Joints(0) | Semantic::Weights(0) => continue,
                Semantic::Normals => (
                    "NORMAL".into(),
                    3,
                    reader
                        .read_normals()
                        .ok_or_else(|| invalid("cut normals", "unreadable"))?
                        .map(|[x, y, z]| [x, y, z, 0.])
                        .collect(),
                ),
                Semantic::Tangents => (
                    "TANGENT".into(),
                    4,
                    reader
                        .read_tangents()
                        .ok_or_else(|| invalid("cut tangents", "unreadable"))?
                        .collect(),
                ),
                Semantic::TexCoords(i) => (
                    format!("TEXCOORD_{i}"),
                    2,
                    reader
                        .read_tex_coords(i)
                        .ok_or_else(|| invalid("cut UV", "unreadable"))?
                        .into_f32()
                        .map(|[u, v]| [u, v, 0., 0.])
                        .collect(),
                ),
                Semantic::Colors(i) => (
                    format!("COLOR_{i}"),
                    4,
                    reader
                        .read_colors(i)
                        .ok_or_else(|| invalid("cut colors", "unreadable"))?
                        .into_rgba_f32()
                        .collect(),
                ),
                other => return Err(unsupported("cut attribute", format!("{other:?}"))),
            };
            if values.len() != count || values.iter().flatten().any(|v| !v.is_finite()) {
                return Err(invalid("surface attributes", "invalid count or value"));
            }
            out.push(SurfaceAttribute {
                semantic,
                width,
                values,
            });
        }
        Ok(out)
    }
}
