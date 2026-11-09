//! Raw garment intake for generated exports (G01–G03): read a `.glb`,
//! `.gltf` or `.obj` as generated — arbitrary scale, at most one material,
//! no rig — and normalise it to the declared body scale (metres).
//!
//! The source stays untouched: positions are copied, never repaired, and no
//! axis conversion happens here (`DECL0013` records the coordinate transform
//! in the recipe, so intake keeps the file's own frame).
//!
//! Scale rule: the largest bounding-box dimension is matched against the
//! garment band in metres. Coordinates inside the band read as metres; far
//! below it they read as ~1/100-scale content shipped in metre units (the
//! common generated-export defect), far above it as explicit centimetres or
//! millimetres. The matched rule and its factor are part of
//! [`RawGarment::scale`], and the same file always yields the same rule and
//! factor. Detection is a declared heuristic, not a proof: a genuine
//! sub-decimetre miniature is misread as shrunk content, which the reported
//! factor makes visible instead of silent.
//!
//! [`RawGarment::into_fit_input`] is the G05 hand-off: it narrows the read into
//! the mesh the conform path consumes, in intake vertex order. Its refusals are
//! the fit path's own (see [`HandoffError`]); intake defects such as an open
//! boundary are reported by the audit, never invented here as a fit rule.
//! [`RawGarment::classify`] is the G06 kind gate: one
//! [`classify`](crate::classify) call with the fitted path's rules, and an
//! undecided kind is a typed refusal carrying the measurements, never a guess.
//!
//! [`RawGarment::defects`] is the G04 defect report: the measured topology
//! counts, never repaired, and [`DefectLimits`] is the G07 gate over them.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use sculpto_core::mesh::MaterialSlot;

use crate::classify::{ClassifyError, Features, GarmentKind};

/// Largest dimension still read as metres; above it the file is
/// unit-labelled instead.
const METRE_LIMIT: f32 = 10.0;
/// Below this largest dimension (in metres) the coordinates cannot be a
/// garment-sized model shipped in true metres.
const UNDERSIZED_LIMIT: f32 = 0.1;
/// Largest dimension read as centimetres; above it, millimetres.
const CENTIMETRE_LIMIT: f32 = 500.0;

/// One raw garment read from a generated export.
#[derive(Debug, Clone, PartialEq)]
pub struct RawGarment {
    /// Mesh name, as authored; empty when the file names none.
    pub name: String,
    /// Vertex positions in metres; the scale factor is already applied.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals; empty when the file carries none.
    pub normals: Vec<[f32; 3]>,
    /// Vertex UVs; empty when the file carries none. An OBJ UV seam collapses
    /// to the last face corner's value per source vertex; render vertices are
    /// not split, so source vertex/face correspondence is preserved.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle indices into [`Self::positions`].
    pub triangles: Vec<[u32; 3]>,
    /// The captured material; `name` is `"default"` and paths are empty when
    /// the file declares no material or embeds its textures.
    pub material: MaterialSlot,
    /// The detected source scale and the factor applied to reach metres.
    pub scale: ScaleReport,
}

/// The scale rule that matched the source extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleRule {
    /// Extent in the garment band: the coordinates already are metres.
    Metres,
    /// Extent far below any plausible garment: metre-unit coordinates
    /// carrying ~1/100-scale content, so the factor multiplies by 100.
    UndersizedMetres,
    /// Extent above the metre band but under [`CENTIMETRE_LIMIT`]:
    /// the coordinates are centimetres.
    Centimetres,
    /// Extent from [`CENTIMETRE_LIMIT`] up: the coordinates are millimetres.
    Millimetres,
}

impl ScaleRule {
    /// Multiplier that maps source coordinates to metres.
    pub fn factor(self) -> f32 {
        match self {
            Self::Metres => 1.0,
            Self::UndersizedMetres => 100.0,
            Self::Centimetres => 0.01,
            Self::Millimetres => 0.001,
        }
    }

    /// Stable label for reports and errors.
    pub fn label(self) -> &'static str {
        match self {
            Self::Metres => "metres",
            Self::UndersizedMetres => "undersized metre coordinates",
            Self::Centimetres => "centimetres",
            Self::Millimetres => "millimetres",
        }
    }
}

/// Detected source scale and the factor applied to reach metres. A pure
/// function of the source geometry, so it is identical on every re-run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleReport {
    /// The rule that matched.
    pub rule: ScaleRule,
    /// Largest bounding-box dimension of the source geometry, before scaling.
    pub source_extent: f32,
    /// Multiplier applied to [`RawGarment::positions`] to reach metres.
    pub factor: f32,
}

/// Why a raw garment export cannot be read.
#[derive(Debug)]
pub enum IntakeError {
    /// The file could not be read.
    Io {
        /// The file that failed.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// The extension is not one of `.glb`, `.gltf`, `.obj`.
    UnsupportedExtension {
        /// The refused extension, lowercased; empty when there was none.
        extension: String,
    },
    /// The file has no bytes.
    Empty {
        /// The file that was empty.
        path: PathBuf,
    },
    /// The glTF/GLB structure could not be parsed or names an unsupported
    /// feature.
    Decode {
        /// The file format that failed.
        format: &'static str,
        /// What was wrong.
        detail: String,
    },
    /// The geometry carries no vertices.
    MissingVertices {
        /// The file without vertices.
        path: PathBuf,
    },
    /// A glTF primitive has no index accessor.
    MissingIndices {
        /// The mesh whose primitive lacks indices.
        mesh: usize,
    },
    /// The glTF file has more than one mesh; the one-garment contract needs
    /// exactly one.
    MultipleMeshes {
        /// The mesh count.
        count: usize,
    },
    /// The glTF file carries a skin (rig); generated garments are not rigged.
    Rigged {
        /// The skin count.
        skins: usize,
    },
    /// The glTF file declares more than one material.
    TooManyMaterials {
        /// The material count.
        count: usize,
    },
    /// A `.gltf` buffer is a `data:` URI; only the GLB `BIN` chunk and
    /// relative file buffers are supported.
    UnsupportedBufferUri {
        /// The refused URI.
        uri: String,
    },
    /// An OBJ face references a vertex outside the parsed geometry.
    FaceIndexOutOfRange {
        /// The 1-based face counter.
        face: usize,
        /// The offending index, as authored.
        vertex: i64,
    },
    /// An OBJ line could not be parsed.
    Malformed {
        /// The file format that failed.
        format: &'static str,
        /// The 1-based line number.
        line: usize,
        /// What was wrong.
        detail: String,
    },
}

impl fmt::Display for IntakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::UnsupportedExtension { extension } => {
                write!(f, "unsupported file extension `{extension}`")
            }
            Self::Empty { path } => write!(f, "{}: file is empty", path.display()),
            Self::Decode { format, detail } => write!(f, "{format}: {detail}"),
            Self::MissingVertices { path } => write!(f, "{}: no vertex lines", path.display()),
            Self::MissingIndices { mesh } => {
                write!(f, "glTF: mesh {mesh} primitive has no indices")
            }
            Self::MultipleMeshes { count } => {
                write!(f, "glTF: {count} meshes; one garment file needs one")
            }
            Self::Rigged { skins } => write!(f, "glTF: rigged file ({skins} skins)"),
            Self::TooManyMaterials { count } => {
                write!(
                    f,
                    "glTF: {count} materials; one garment file needs at most one"
                )
            }
            Self::UnsupportedBufferUri { uri } => {
                write!(f, "glTF: data URI buffers unsupported: {uri}")
            }
            Self::FaceIndexOutOfRange { face, vertex } => {
                write!(
                    f,
                    "OBJ: face {face} references vertex {vertex} out of range"
                )
            }
            Self::Malformed {
                format,
                line,
                detail,
            } => write!(f, "{format}: line {line}: {detail}"),
        }
    }
}

impl std::error::Error for IntakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Read a raw garment export: `.glb`, `.gltf` or `.obj`, as generated.
pub fn read_raw(path: &Path) -> Result<RawGarment, IntakeError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    match extension.as_deref() {
        Some("glb" | "gltf") => read_gltf_file(path),
        Some("obj") => read_obj_file(path),
        _ => Err(IntakeError::UnsupportedExtension {
            extension: extension.unwrap_or_default(),
        }),
    }
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, IntakeError> {
    std::fs::read(path).map_err(|source| IntakeError::Io {
        path: path.to_owned(),
        source,
    })
}

fn read_gltf_file(path: &Path) -> Result<RawGarment, IntakeError> {
    let bytes = read_bytes(path)?;
    if bytes.is_empty() {
        return Err(IntakeError::Empty {
            path: path.to_owned(),
        });
    }
    let decoded = gltf::Gltf::from_slice(&bytes).map_err(|error| IntakeError::Decode {
        format: "glTF",
        detail: error.to_string(),
    })?;
    let document = decoded.document;
    if let Some(extension) = document.extensions_required().next() {
        return Err(IntakeError::Decode {
            format: "glTF",
            detail: format!("required extension {extension}"),
        });
    }
    let skins = document.skins().count();
    if skins > 0 {
        return Err(IntakeError::Rigged { skins });
    }
    let materials = document.materials().count();
    if materials > 1 {
        return Err(IntakeError::TooManyMaterials { count: materials });
    }
    let meshes = document.meshes().count();
    if meshes > 1 {
        return Err(IntakeError::MultipleMeshes { count: meshes });
    }
    let material = material_slot(&document);
    let buffers = resolve_buffers(path, &document, decoded.blob.as_deref())?;

    let mut name = String::new();
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut triangles = Vec::new();
    for (mesh_index, mesh) in document.meshes().enumerate() {
        if name.is_empty() && mesh.name().is_some_and(|value| !value.is_empty()) {
            name = mesh.name().unwrap_or_default().to_owned();
        }
        for primitive in mesh.primitives() {
            let reader = primitive
                .reader(|buffer| buffers.get(buffer.index()).map(|bytes| bytes.as_slice()));
            let Some(primitive_positions) = reader.read_positions() else {
                return Err(IntakeError::Decode {
                    format: "glTF",
                    detail: format!("mesh {mesh_index} primitive has no POSITION"),
                });
            };
            let base = positions.len() as u32;
            positions.extend(primitive_positions);
            if let Some(primitive_normals) = reader.read_normals() {
                normals.extend(primitive_normals);
            }
            if let Some(primitive_uvs) = reader.read_tex_coords(0) {
                uvs.extend(primitive_uvs.into_f32());
            }
            let Some(indices) = reader.read_indices() else {
                return Err(IntakeError::MissingIndices { mesh: mesh_index });
            };
            let index_list: Vec<u32> = indices.into_u32().collect();
            let (chunks, remainder) = index_list.as_chunks::<3>();
            if !remainder.is_empty() {
                return Err(IntakeError::Decode {
                    format: "glTF",
                    detail: format!("mesh {mesh_index} index count is not a triangle multiple"),
                });
            }
            triangles.extend(
                chunks
                    .iter()
                    .map(|&[a, b, c]| [a + base, b + base, c + base]),
            );
        }
    }
    if positions.is_empty() {
        return Err(IntakeError::MissingVertices {
            path: path.to_owned(),
        });
    }
    match_aligned(&normals, positions.len(), "normals")?;
    match_aligned(&uvs, positions.len(), "uvs")?;
    Ok(finalise(name, positions, normals, uvs, triangles, material))
}

fn match_aligned<T>(
    values: &[T],
    length: usize,
    attribute: &'static str,
) -> Result<(), IntakeError> {
    if !values.is_empty() && values.len() != length {
        return Err(IntakeError::Decode {
            format: "glTF",
            detail: format!(
                "{attribute} cover {} vertices but the mesh has {length}",
                values.len()
            ),
        });
    }
    Ok(())
}

/// Resolve every glTF buffer into bytes: the GLB `BIN` chunk, or a file
/// relative to the `.gltf`. `data:` URIs are refused.
fn resolve_buffers(
    path: &Path,
    document: &gltf::Document,
    blob: Option<&[u8]>,
) -> Result<Vec<Vec<u8>>, IntakeError> {
    document
        .buffers()
        .map(|buffer| match buffer.source() {
            gltf::buffer::Source::Bin => {
                blob.map(<[u8]>::to_vec).ok_or_else(|| IntakeError::Decode {
                    format: "glTF",
                    detail: "buffer requires the BIN chunk".to_string(),
                })
            }
            gltf::buffer::Source::Uri(uri) => {
                if uri.starts_with("data:") {
                    return Err(IntakeError::UnsupportedBufferUri {
                        uri: uri.to_owned(),
                    });
                }
                let file = path.parent().unwrap_or(Path::new(".")).join(uri);
                std::fs::read(&file).map_err(|source| IntakeError::Io { path: file, source })
            }
        })
        .collect()
}

/// Capture the one declared material; no material means a `"default"` slot
/// with empty paths. Embedded (GLB view) textures have no path to capture.
fn material_slot(document: &gltf::Document) -> MaterialSlot {
    let Some(material) = document.materials().next() else {
        return default_material();
    };
    let diffuse = material
        .pbr_metallic_roughness()
        .base_color_texture()
        .and_then(|info| texture_uri(info.texture()));
    let normal = material
        .normal_texture()
        .and_then(|texture| texture_uri(texture.texture()));
    MaterialSlot {
        name: material.name().unwrap_or("default").to_owned(),
        diffuse,
        normal,
        ..default_material()
    }
}

fn default_material() -> MaterialSlot {
    MaterialSlot {
        name: "default".to_owned(),
        ..MaterialSlot::default()
    }
}

fn texture_uri(texture: gltf::Texture<'_>) -> Option<String> {
    match texture.source().source() {
        gltf::image::Source::Uri { uri, .. } => Some(uri.to_owned()),
        gltf::image::Source::View { .. } => None,
    }
}

/// The OBJ state: source positions, the raw `vt`/`vn` lists, per-vertex
/// attribute slots filled from face corners, and the material references.
#[derive(Default)]
struct ObjSource {
    positions: Vec<[f32; 3]>,
    vt_values: Vec<[f32; 2]>,
    vn_values: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    material_name: Option<String>,
    material_library: Option<String>,
}

fn read_obj_file(path: &Path) -> Result<RawGarment, IntakeError> {
    let bytes = read_bytes(path)?;
    if bytes.is_empty() {
        return Err(IntakeError::Empty {
            path: path.to_owned(),
        });
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut source = ObjSource::default();
    let mut seen_uvs = false;
    let mut seen_normals = false;
    let mut face = 0usize;
    for (number, line) in text.lines().enumerate() {
        let number = number + 1;
        let malformed = |detail: String| IntakeError::Malformed {
            format: "OBJ",
            line: number,
            detail,
        };
        if let Some(rest) = line.strip_prefix("v ") {
            source
                .positions
                .push(parse_coordinates::<3>(rest, 3, malformed)?);
        } else if let Some(rest) = line.strip_prefix("vt ") {
            source
                .vt_values
                .push(parse_coordinates::<2>(rest, 1, malformed)?);
        } else if let Some(rest) = line.strip_prefix("vn ") {
            source
                .vn_values
                .push(parse_coordinates::<3>(rest, 3, malformed)?);
        } else if let Some(rest) = line.strip_prefix("f ") {
            face += 1;
            let corners = parse_face(rest, &source, face, &malformed)?;
            for &(vertex, uv, normal) in &corners {
                if let Some(uv) = uv {
                    seen_uvs = true;
                    copy_or_grow(&mut source.uvs, vertex, uv);
                }
                if let Some(normal) = normal {
                    seen_normals = true;
                    copy_or_grow(&mut source.normals, vertex, normal);
                }
            }
            fan_triangulate(&corners, &mut source.triangles);
        } else if let Some(rest) = line.strip_prefix("usemtl ") {
            source.material_name = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("mtllib ") {
            source
                .material_library
                .get_or_insert(rest.trim().to_owned());
        }
    }
    if source.positions.is_empty() {
        return Err(IntakeError::MissingVertices {
            path: path.to_owned(),
        });
    }
    // Per-vertex slots carry UVs/normals only when the file authors them: an
    // unauthored layer stays empty as [`RawGarment`] documents, a partially
    // authored one is padded to the arena with defaults.
    if seen_uvs {
        align(&mut source.uvs, source.positions.len());
    } else {
        source.uvs.clear();
    }
    if seen_normals {
        align(&mut source.normals, source.positions.len());
    } else {
        source.normals.clear();
    }

    let mut material = match source.material_name.as_deref() {
        Some(name) => MaterialSlot {
            name: name.to_owned(),
            ..default_material()
        },
        None => default_material(),
    };
    if let Some(library) = source.material_library.as_deref() {
        capture_mtl(path, library, &mut material);
    }
    Ok(finalise(
        String::new(),
        source.positions,
        source.normals,
        source.uvs,
        source.triangles,
        material,
    ))
}

fn parse_coordinates<const SLOTS: usize>(
    rest: &str,
    minimum: usize,
    malformed: impl Fn(String) -> IntakeError,
) -> Result<[f32; SLOTS], IntakeError> {
    let mut values = [0.0; SLOTS];
    let mut count = 0;
    for (slot, token) in rest.split_whitespace().enumerate() {
        if slot >= SLOTS {
            break;
        }
        values[slot] = token
            .parse::<f32>()
            .map_err(|error| malformed(format!("`{token}`: {error}")))?;
        count = slot + 1;
    }
    if count < minimum {
        return Err(malformed(format!(
            "expected at least {minimum} coordinates, found {count}"
        )));
    }
    Ok(values)
}

/// One OBJ face corner: position index plus optional UV and normal values.
type Corner = (u32, Option<[f32; 2]>, Option<[f32; 3]>);

fn parse_face(
    rest: &str,
    source: &ObjSource,
    face: usize,
    malformed: &impl Fn(String) -> IntakeError,
) -> Result<Vec<Corner>, IntakeError> {
    rest.split_whitespace()
        .map(|corner| {
            let mut fields = corner.split('/');
            let position_token = fields.next().unwrap_or_default();
            let uv_token = fields.next();
            let normal_token = fields.next();
            let vertex = obj_index(position_token, source.positions.len(), face, malformed)?;
            let uv = match uv_token.filter(|value| !value.is_empty()) {
                Some(token) => {
                    let index = obj_index(token, source.vt_values.len(), face, malformed)? as usize;
                    Some(source.vt_values[index])
                }
                None => None,
            };
            let normal = match normal_token.filter(|value| !value.is_empty()) {
                Some(token) => {
                    let index = obj_index(token, source.vn_values.len(), face, malformed)? as usize;
                    Some(source.vn_values[index])
                }
                None => None,
            };
            Ok((vertex, uv, normal))
        })
        .collect()
}

fn obj_index(
    token: &str,
    count: usize,
    face: usize,
    malformed: &impl Fn(String) -> IntakeError,
) -> Result<u32, IntakeError> {
    let authored: i64 = token
        .parse()
        .map_err(|error| malformed(format!("`{token}`: {error}")))?;
    let resolved = if authored < 0 {
        count as i64 + authored
    } else {
        authored - 1
    };
    if resolved < 0 || resolved >= count as i64 {
        return Err(IntakeError::FaceIndexOutOfRange {
            face,
            vertex: authored,
        });
    }
    Ok(resolved as u32)
}

fn copy_or_grow<T: Copy + Default>(values: &mut Vec<T>, index: u32, value: T) {
    if index as usize >= values.len() {
        values.resize(index as usize + 1, T::default());
    }
    values[index as usize] = value;
}

fn align<T: Copy + Default>(values: &mut Vec<T>, length: usize) {
    values.resize(length, T::default());
}

fn fan_triangulate(corners: &[Corner], triangles: &mut Vec<[u32; 3]>) {
    if corners.len() < 3 {
        return;
    }
    let positions = corners
        .iter()
        .map(|&(vertex, _, _)| vertex)
        .collect::<Vec<_>>();
    for window in positions[1..].windows(2) {
        triangles.push([positions[0], window[0], window[1]]);
    }
}

/// Read the declared `.mtl` sidecar for the captured material's texture
/// paths; a missing or nameless sidecar keeps the paths empty. Non-fatal by
/// contract: a texture-less OBJ is a valid raw garment.
fn capture_mtl(obj_path: &Path, library: &str, material: &mut MaterialSlot) {
    let Ok(text) =
        std::fs::read_to_string(obj_path.parent().unwrap_or(Path::new(".")).join(library))
    else {
        return;
    };
    let mut in_block = false;
    for line in text.lines() {
        let line = line.trim();
        let mut statement = line.splitn(2, char::is_whitespace);
        match statement.next() {
            Some("newmtl") => {
                in_block = statement.next().map(str::trim) == Some(material.name.as_str())
            }
            Some("map_Kd") if in_block => material.diffuse = texture_path(statement.next()),
            Some("map_Bump" | "bump" | "map_bump") if in_block => {
                material.normal = texture_path(statement.next())
            }
            _ => {}
        }
    }
}

/// First non-option token of an MTL texture statement, so sampler flags such
/// as `-clamp on` are skipped.
fn texture_path(rest: Option<&str>) -> Option<String> {
    rest.and_then(|rest| {
        rest.split_whitespace()
            .find(|token| !token.starts_with('-'))
            .map(str::to_owned)
    })
}

fn finalise(
    name: String,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    triangles: Vec<[u32; 3]>,
    material: MaterialSlot,
) -> RawGarment {
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for position in &positions {
        for (axis, value) in position.iter().enumerate() {
            minimum[axis] = minimum[axis].min(*value);
            maximum[axis] = maximum[axis].max(*value);
        }
    }
    let source_extent = (maximum[0] - minimum[0])
        .max(maximum[1] - minimum[1])
        .max(maximum[2] - minimum[2]);
    let rule = scale_rule(source_extent);
    let factor = rule.factor();
    let positions = positions
        .into_iter()
        .map(|position| position.map(|value| value * factor))
        .collect();
    RawGarment {
        name,
        positions,
        normals,
        uvs,
        triangles,
        material,
        scale: ScaleReport {
            rule,
            source_extent,
            factor,
        },
    }
}

fn scale_rule(extent: f32) -> ScaleRule {
    match extent {
        value if value < UNDERSIZED_LIMIT => ScaleRule::UndersizedMetres,
        value if value < METRE_LIMIT => ScaleRule::Metres,
        value if value < CENTIMETRE_LIMIT => ScaleRule::Centimetres,
        _ => ScaleRule::Millimetres,
    }
}

/// The measured topology defects of one raw garment, as read. Intake never
/// repairs; these counts are the caller's decision input. @req DECL0013
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TopologyDefects {
    /// Edges carried by more than two triangles.
    pub non_manifold_edges: usize,
    /// Triangles with zero area: repeated, coincident or collinear corners.
    pub degenerate_triangles: usize,
    /// Position entries beyond the first at one exactly coincident location
    /// (the bit-exact rule the topology stage welds by).
    pub duplicate_positions: usize,
    /// Vertices without a UV: the whole arena when the file carries no UV
    /// layer, the uncovered remainder of a partial one.
    pub missing_uvs: usize,
    /// Edges carried by exactly one triangle. Openings at collar, cuffs and
    /// hem are normal cloth; the count only feeds the declared limits.
    pub open_boundary_edges: usize,
}

/// Acceptable defect maxima for one raw garment. A count above its limit is
/// a refusal; there is no bypass flag — the caller decides with the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefectLimits {
    /// Largest accepted [`TopologyDefects::non_manifold_edges`] count.
    pub non_manifold_edges: usize,
    /// Largest accepted [`TopologyDefects::degenerate_triangles`] count.
    pub degenerate_triangles: usize,
    /// Largest accepted [`TopologyDefects::duplicate_positions`] count.
    pub duplicate_positions: usize,
    /// Largest accepted [`TopologyDefects::missing_uvs`] count.
    pub missing_uvs: usize,
    /// Largest accepted [`TopologyDefects::open_boundary_edges`] count.
    pub open_boundary_edges: usize,
}

impl Default for DefectLimits {
    /// Generated exports must arrive manifold, non-degenerate, weld-free and
    /// fully UV-covered, so those limits are zero. Open boundary edges are
    /// normal cloth (hem, collar, cuffs): a garment-resolution torso keeps
    /// those openings far below this budget, while a shredded or fractured
    /// mesh crosses it.
    fn default() -> Self {
        Self {
            non_manifold_edges: 0,
            degenerate_triangles: 0,
            duplicate_positions: 0,
            missing_uvs: 0,
            open_boundary_edges: 512,
        }
    }
}

impl DefectLimits {
    /// The typed refusal when any measured count exceeds its limit.
    pub fn check(&self, counts: TopologyDefects) -> Result<(), DefectOverLimit> {
        let over = counts.non_manifold_edges > self.non_manifold_edges
            || counts.degenerate_triangles > self.degenerate_triangles
            || counts.duplicate_positions > self.duplicate_positions
            || counts.missing_uvs > self.missing_uvs
            || counts.open_boundary_edges > self.open_boundary_edges;
        match over {
            true => Err(DefectOverLimit {
                counts,
                limits: *self,
            }),
            false => Ok(()),
        }
    }
}

/// Why a raw garment was refused: the measured counts against the limits
/// that were applied. The display names every exceeded count and its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefectOverLimit {
    /// The measured counts.
    pub counts: TopologyDefects,
    /// The limits the garment was measured against.
    pub limits: DefectLimits,
}

impl fmt::Display for DefectOverLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let exceeded = [
            (
                "non_manifold_edges",
                self.counts.non_manifold_edges,
                self.limits.non_manifold_edges,
            ),
            (
                "degenerate_triangles",
                self.counts.degenerate_triangles,
                self.limits.degenerate_triangles,
            ),
            (
                "duplicate_positions",
                self.counts.duplicate_positions,
                self.limits.duplicate_positions,
            ),
            (
                "missing_uvs",
                self.counts.missing_uvs,
                self.limits.missing_uvs,
            ),
            (
                "open_boundary_edges",
                self.counts.open_boundary_edges,
                self.limits.open_boundary_edges,
            ),
        ]
        .into_iter()
        .filter(|&(_, measured, limit)| measured > limit);
        write!(formatter, "garment defects exceed the declared limits")?;
        for (name, measured, limit) in exceeded {
            write!(formatter, "; {name} {measured} > {limit}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DefectOverLimit {}

/// The garment the conform fit path consumes
/// ([`sculpto_mesh::conform`](crate::conform)).
///
/// This is [`RawGarment`] minus the fields the fit has no use for. The render
/// vertex arena, its normals and the triangles stay in intake order, so
/// `Conform::bind(&fit.positions, &body.positions)` pins vertex *i* exactly as
/// the intake read it: no welding, no reordering, no normal recomputation.
#[derive(Debug, Clone, PartialEq)]
pub struct FitInput {
    /// Mesh name, as authored; empty when the file names none.
    pub name: String,
    /// Vertex positions in metres, in intake order.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals in the same order; empty when the file carries none.
    pub normals: Vec<[f32; 3]>,
    /// Triangle indices into [`Self::positions`].
    pub triangles: Vec<[u32; 3]>,
}

/// Why an intake cannot be handed to the fit path.
///
/// The rules are the fit path's own, not a new garment policy:
/// [`Conform::bind`](crate::conform::Conform::bind) refuses an empty garment,
/// and the fitted mesh keeps the intake triangles, so an index outside the
/// vertex arena could not be applied. Open boundaries and non-manifold edges
/// are **not** refusals: conform pins every vertex to its nearest reference
/// vertex and never reads a triangle, so those defects stay with the intake
/// audit instead of blocking a fit that the vertex binding can perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffError {
    /// The garment has no vertices, which `Conform::bind` refuses as
    /// `ConformError::EmptyGarment` for the same positions.
    EmptyGarment,
    /// The normals cover a different vertex count than the positions, so they
    /// cannot be carried in intake order.
    NormalCount {
        /// Vertex count of the positions.
        positions: usize,
        /// Vertex count of the normals.
        normals: usize,
    },
    /// A triangle references a vertex outside the arena; the fitted mesh would
    /// read past the last position.
    TriangleVertexOutOfRange {
        /// The 0-based triangle index.
        triangle: usize,
        /// The offending vertex index, as read.
        vertex: u32,
    },
}

impl fmt::Display for HandoffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGarment => write!(f, "the garment has no vertices"),
            Self::NormalCount { positions, normals } => write!(
                f,
                "{normals} normals cover {positions} garment vertices; the fit needs one per vertex or none"
            ),
            Self::TriangleVertexOutOfRange { triangle, vertex } => write!(
                f,
                "triangle {triangle} references vertex {vertex} outside the garment"
            ),
        }
    }
}

impl std::error::Error for HandoffError {}

/// Why the intake cannot name a garment kind.
#[derive(Debug, Clone, PartialEq)]
pub enum KindError {
    /// The classifier's evidence left the kind undecided
    /// ([`GarmentKind::Unknown`]); the measurements name what was measured so
    /// the caller can split the parts or pass an explicit kind upstream.
    Undecided {
        /// The measured shape facts the verdict rests on.
        features: Features,
        /// One line per decision branch taken.
        evidence: Vec<String>,
    },
    /// Classification could not run: the fitted path's own
    /// [`ClassifyError`] (empty or flat body, empty garment, garment
    /// without triangles, triangle index outside the garment).
    Classified(ClassifyError),
}

impl fmt::Display for KindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undecided { features, .. } => write!(
                f,
                "garment kind is undecided: bottom={bottom:.2} top={top:.2} \
                 coverage={coverage:.2} columns={columns} leg_columns={leg_columns} \
                 body_columns={body_columns} merged={merged} \
                 radial_ratio={radial_ratio:.2} top_ratio={top_ratio:.2} \
                 angular={angular:.2} components={components} vertices={vertices}",
                bottom = features.bottom,
                top = features.top,
                coverage = features.coverage,
                columns = features.columns,
                leg_columns = features.leg_columns,
                body_columns = features.body_columns,
                merged = features.merged,
                radial_ratio = features.radial_ratio,
                top_ratio = features.top_ratio,
                angular = features.angular,
                components = features.components,
                vertices = features.vertices,
            ),
            Self::Classified(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for KindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Classified(error) => Some(error),
            Self::Undecided { .. } => None,
        }
    }
}

impl RawGarment {
    /// Count the topology defects the read carries — measurement only, no
    /// repair, no tolerance.
    ///
    /// Triangles referencing a missing vertex enter no count; the hand-off
    /// refuses them as [`HandoffError::TriangleVertexOutOfRange`]. Degenerate
    /// triangles are excluded from the edge census so one defect does not
    /// masquerade as another (the same rule [`crate::control`] applies when
    /// it builds its edge map).
    pub fn defects(&self) -> TopologyDefects {
        let arena = self.positions.len() as u32;
        let mut counts = TopologyDefects::default();
        let mut distinct = HashSet::new();
        counts.duplicate_positions = self
            .positions
            .iter()
            .filter(|position| !distinct.insert(crate::topology::key(**position)))
            .count();
        counts.missing_uvs = match self.uvs.is_empty() {
            true => self.positions.len(),
            false => self.positions.len().saturating_sub(self.uvs.len()),
        };
        let mut edge_faces: HashMap<[u32; 2], usize> = HashMap::new();
        for &[a, b, c] in &self.triangles {
            if a >= arena || b >= arena || c >= arena {
                continue;
            }
            let corners = [
                self.positions[a as usize],
                self.positions[b as usize],
                self.positions[c as usize],
            ];
            if crate::check::zero_area(corners[0], corners[1], corners[2]) {
                counts.degenerate_triangles += 1;
                continue;
            }
            for (left, right) in [(a, b), (b, c), (c, a)] {
                let edge = match left <= right {
                    true => [left, right],
                    false => [right, left],
                };
                *edge_faces.entry(edge).or_default() += 1;
            }
        }
        for faces in edge_faces.values() {
            match *faces {
                1 => counts.open_boundary_edges += 1,
                3.. => counts.non_manifold_edges += 1,
                _ => {}
            }
        }
        counts
    }

    /// Narrow the read into the fit input, preserving vertex order and normals.
    pub fn into_fit_input(self) -> Result<FitInput, HandoffError> {
        if self.positions.is_empty() {
            return Err(HandoffError::EmptyGarment);
        }
        if !self.normals.is_empty() && self.normals.len() != self.positions.len() {
            return Err(HandoffError::NormalCount {
                positions: self.positions.len(),
                normals: self.normals.len(),
            });
        }
        let arena = self.positions.len();
        if let Some((triangle, vertex)) =
            self.triangles
                .iter()
                .enumerate()
                .find_map(|(index, corners)| {
                    let vertex = corners
                        .iter()
                        .copied()
                        .find(|vertex| *vertex as usize >= arena)?;
                    Some((index, vertex))
                })
        {
            return Err(HandoffError::TriangleVertexOutOfRange { triangle, vertex });
        }
        Ok(FitInput {
            name: self.name,
            positions: self.positions,
            normals: self.normals,
            triangles: self.triangles,
        })
    }

    // TODO(G04-intake-check): wire this classification into the engine's
    // future `intake.check` command (G04 topology defect report, epic
    // 2026-11-09_01h40m00s); the command id does not exist yet, so the kind
    // has no command surface.
    /// Classify the raw garment against the body it must fit with the same
    /// rules the fitted path uses: one
    /// [`classify`](crate::classify::classify) call, the classifier's own
    /// taxonomy — no intake-specific kinds. The kind selects the corset-aura
    /// profile the fit consumes; an undecided kind
    /// ([`GarmentKind::Unknown`]) is a [`KindError::Undecided`] refusal
    /// carrying the measurements, never a guess.
    pub fn classify(
        &self,
        body: &[[f32; 3]],
        body_triangles: &[[u32; 3]],
    ) -> Result<GarmentKind, KindError> {
        let verdict =
            crate::classify::classify(body, body_triangles, &self.positions, &self.triangles)
                .map_err(KindError::Classified)?;
        if verdict.kind == GarmentKind::Unknown {
            return Err(KindError::Undecided {
                features: verdict.features,
                evidence: verdict.evidence,
            });
        }
        Ok(verdict.kind)
    }
}
