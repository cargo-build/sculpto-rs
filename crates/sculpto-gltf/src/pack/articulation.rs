//! Append deform helpers and rigid, joint-parented mesh sections to an existing actor.
use super::{PackError, PackWriter};
use serde_json::{Value, json};

/// Local TRS and joint-parent index in the output node graph. @req DECL0013
pub struct HelperJoint {
    pub name: String,
    pub parent: usize,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// Independent cut vertices, shading data, skin if flexible, and two disjoint triangle groups.
#[derive(Default)]
pub struct CutGeometry {
    pub positions: Vec<[f32; 3]>,
    pub attributes: Vec<(String, usize, Vec<f32>)>,
    pub rows: Vec<Vec<(u16, f32)>>,
    pub outer: Vec<[u32; 3]>,
    pub transitions: Vec<[u32; 3]>,
    pub band_uv: Vec<f32>,
    pub caps: Vec<[u32; 3]>,
}

impl PackWriter {
    fn child(&mut self, parent: usize, node: Value) -> Result<usize, PackError> {
        let nodes = self
            .json
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| PackError::invalid("joint child", "missing nodes"))?;
        let i = nodes.len();
        let children = nodes
            .get_mut(parent)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| PackError::invalid("joint child", "parent is absent"))?
            .entry("children")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| PackError::invalid("joint child", "invalid children"))?;
        children.push(json!(i));
        nodes.push(node);
        Ok(i)
    }

    /// Extend a skin in palette order and preserve the old inverse-bind rows. @req DECL0013
    pub fn extend_skin(
        &mut self,
        skin: usize,
        helpers: &[HelperJoint],
        binds: &[[[f32; 4]; 4]],
    ) -> Result<(), PackError> {
        let old = self
            .element("skins", skin)?
            .get("joints")
            .and_then(Value::as_array)
            .ok_or_else(|| PackError::invalid("deform helpers", "invalid joint list"))?
            .len();
        if binds.len() != old + helpers.len()
            || binds.iter().flatten().flatten().any(|v| !v.is_finite())
        {
            return Err(PackError::invalid(
                "deform helpers",
                "invalid inverse-bind rows",
            ));
        }
        let mut added = Vec::with_capacity(helpers.len());
        for h in helpers {
            let node = json!({"name":h.name,"translation":h.translation,"rotation":h.rotation,"scale":h.scale});
            added.push(json!(self.child(h.parent, node)?));
        }
        let bytes: Vec<_> = binds
            .iter()
            .flatten()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let view = self.view(&bytes, &json!({}))?;
        let id = self.append(
            "accessors",
            json!({"bufferView":view,"componentType":5126,"count":binds.len(),"type":"MAT4"}),
        )?;
        let target = self.element("skins", skin)?;
        target["joints"]
            .as_array_mut()
            .ok_or_else(|| PackError::invalid("deform helpers", "invalid palette"))?
            .extend(added);
        target["inverseBindMatrices"] = json!(id);
        Ok(())
    }

    fn triangle_indices(&mut self, faces: &[[u32; 3]]) -> Result<usize, PackError> {
        if faces.is_empty() {
            return Err(PackError::invalid(
                "articulation triangles",
                "empty triangle set",
            ));
        }
        let data: Vec<_> = faces
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let view = self.view(&data, &json!({"target":34963}))?;
        self.append(
            "accessors",
            json!({"bufferView":view,"componentType":5125,"count":faces.len()*3,"type":"SCALAR"}),
        )
    }

    /// Write a compact cut surface. Only skin-bearing surfaces receive JOINTS and WEIGHTS. @req DECL0013
    fn cut_primitive(
        &mut self,
        data: &CutGeometry,
        faces: &[[u32; 3]],
        material: Option<usize>,
    ) -> Result<Value, PackError> {
        if data.positions.is_empty()
            || faces
                .iter()
                .flatten()
                .any(|&i| i as usize >= data.positions.len())
        {
            return Err(PackError::invalid(
                "cut geometry",
                "invalid compact vertex indices",
            ));
        }
        let mut attrs = serde_json::Map::new();
        attrs.insert(
            "POSITION".into(),
            json!(self.morph_vectors(&data.positions)?),
        );
        for (name, width, values) in &data.attributes {
            if values.len() != data.positions.len() * width || values.iter().any(|v| !v.is_finite())
            {
                return Err(PackError::invalid(
                    "cut attributes",
                    "invalid attribute layout",
                ));
            }
            attrs.insert(
                name.clone(),
                json!(self.vec_floats(values, *width, data.positions.len())?),
            );
        }
        if !data.rows.is_empty() {
            if data.rows.len() != data.positions.len() {
                return Err(PackError::invalid("cut skin", "weight count differs"));
            }
            let (j, w) = self.skin_accessors(&data.rows)?;
            attrs.insert("JOINTS_0".into(), json!(j));
            attrs.insert("WEIGHTS_0".into(), json!(w));
        }
        let mut pr = json!({"attributes":attrs,"mode":4,"indices":self.triangle_indices(faces)?});
        if let Some(m) = material {
            pr["material"] = json!(m);
        }
        Ok(pr)
    }

    /// Replace the body primitive by its flexible exterior and the tissue connecting rigid cut ends. @req DECL0013
    pub fn flexible_surface(
        &mut self,
        mesh: usize,
        part: usize,
        data: &CutGeometry,
        mark: bool,
    ) -> Result<(), PackError> {
        let material = self
            .primitive(mesh, part)?
            .get("material")
            .and_then(Value::as_u64)
            .map(|i| i as usize);
        let pr = self.cut_primitive(data, &data.outer, material)?;
        let mut transition = pr.clone();
        transition["indices"] = json!(self.triangle_indices(&data.transitions)?);
        if mark {
            if data.band_uv.len() != data.positions.len() * 2 {
                return Err(PackError::invalid("transition stripes", "invalid UV count"));
            }
            let uv = self.vec_floats(&data.band_uv, 2, data.positions.len())?;
            transition["attributes"]["TEXCOORD_0"] = json!(uv);
            transition["material"] = json!(self.stripe_material()?);
        }
        *self.primitive(mesh, part)? = pr;
        self.mesh(mesh)?["primitives"]
            .as_array_mut()
            .ok_or_else(|| PackError::invalid("cut skin", "invalid primitives"))?
            .push(transition);
        Ok(())
    }

    fn stripe_material(&mut self) -> Result<usize, PackError> {
        let view = self.view(include_bytes!("transition-stripes.png"), &json!({}))?;
        let image = self.append(
            "images",
            json!({"name":"Joint transition stripes","mimeType":"image/png","bufferView":view}),
        )?;
        let sampler = self.append(
            "samplers",
            json!({"magFilter":9728,"minFilter":9728,"wrapS":10497,"wrapT":33071}),
        )?;
        let tex = self.append("textures", json!({"sampler":sampler,"source":image}))?;
        self.append("materials",json!({"name":"Striped transition tissue","pbrMetallicRoughness":{"baseColorTexture":{"index":tex},"metallicFactor":0.0,"roughnessFactor":0.85},
            "emissiveTexture":{"index":tex},"emissiveFactor":[0.3,0.3,0.3]}))
    }

    /// Write one closed joint-local surface with private geometry, outer material, and interior cap material. @req DECL0013
    pub fn cut_piece(
        &mut self,
        data: &CutGeometry,
        material: Option<usize>,
        interior: usize,
        parent: usize,
        name: &str,
    ) -> Result<(), PackError> {
        let outer = self.cut_primitive(data, &data.outer, material)?;
        let mut cap = outer.clone();
        cap["indices"] = json!(self.triangle_indices(&data.caps)?);
        cap["material"] = json!(interior);
        let mesh = self.append("meshes", json!({"name":name,"primitives":[outer,cap]}))?;
        self.child(parent, json!({"name":name,"mesh":mesh}))?;
        Ok(())
    }

    /// Interior faces have their own untextured material so a source atlas stays on the exterior. @req DECL0013
    pub fn core_material(&mut self) -> Result<usize, PackError> {
        self.append("materials",json!({"name":"Joint interior","pbrMetallicRoughness":{"baseColorFactor":[0.58,0.52,0.43,1.0],"metallicFactor":0.0,"roughnessFactor":0.85}}))
    }

    /// Store one smooth triangulated unit sphere shared by the covered joints. @req DECL0013
    pub fn joint_core_mesh(&mut self, material: usize) -> Result<usize, PackError> {
        let sectors = 20_u32;
        let rings = 12_u32;
        let mut points = vec![[0_f32, 1., 0.]];
        for lat in 1..rings {
            let a = std::f32::consts::PI * lat as f32 / rings as f32;
            for lon in 0..sectors {
                let b = std::f32::consts::TAU * lon as f32 / sectors as f32;
                points.push([a.sin() * b.cos(), a.cos(), a.sin() * b.sin()]);
            }
        }
        let bottom = points.len() as u32;
        points.push([0., -1., 0.]);
        let index = |row: u32, col: u32| 1 + (row - 1) * sectors + col % sectors;
        let mut faces = Vec::new();
        for i in 0..sectors {
            faces.push([0, index(1, i + 1), index(1, i)]);
            for row in 1..rings - 1 {
                let (a, b, c, d) = (
                    index(row, i),
                    index(row, i + 1),
                    index(row + 1, i),
                    index(row + 1, i + 1),
                );
                faces.extend([[a, b, c], [b, d, c]]);
            }
            faces.push([bottom, index(rings - 1, i), index(rings - 1, i + 1)]);
        }
        let xyz = self.morph_vectors(&points)?;
        let idx = self.triangle_indices(&faces)?;
        self.append("meshes",json!({"name":"Articulation core","primitives":[{"attributes":{"POSITION":xyz,"NORMAL":xyz},"indices":idx,"material":material,"mode":4}]}))
    }

    /// Instance a rigid cover in the selected joint's frame. @req DECL0013
    pub fn joint_core(
        &mut self,
        mesh: usize,
        parent: usize,
        name: &str,
        local: [f32; 16],
    ) -> Result<(), PackError> {
        self.child(parent, json!({"name":name,"mesh":mesh,"matrix":local}))?;
        Ok(())
    }
}
