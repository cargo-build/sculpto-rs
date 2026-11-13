//! OBJ helpers shared by the argv surface and the engine handlers.

use std::fmt;
use std::io::Write;
use std::path::Path;

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_nif::NifShape;

use crate::{CommandError, ErrorKind, errors};

/// A Wavefront OBJ mesh: display-frame positions and triangulated faces.
pub type ObjMesh = (Vec<[f32; 3]>, Vec<[u32; 3]>);

/// A file that could not be read, or that is not a usable OBJ mesh.
///
/// The two causes keep their own contract: an unreadable file stays an I/O
/// failure, and malformed content stays an invalid value.
#[derive(Debug)]
#[non_exhaustive]
pub enum ObjError {
    /// The file could not be read; the message keeps the path and the cause.
    Io(String),
    /// The content is unusable; the message keeps the path and the cause.
    Malformed(String),
}

impl fmt::Display for ObjError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) | Self::Malformed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ObjError {}

/// Read an OBJ mesh as authored: `v` positions in the file's own frame and `f`
/// corners fanned to triangles.
///
/// The argv surface and the collision-scope reader share this parser, so both
/// validate the same faces. Positions-only consumers use [`read_obj_positions`].
///
/// # Errors
/// Reports unreadable files, malformed vertices and faces, and empty or
/// out-of-range geometry with the offending value.
pub fn read_obj_mesh(path: &Path) -> Result<ObjMesh, ObjError> {
    let text = read_text(path)?;
    let positions = positions_of(&text, path)?;
    let mut triangles = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("f ") {
            triangles.extend(read_face(rest, path)?);
        }
    }
    if let Some(vertex) = triangles
        .iter()
        .flatten()
        .find(|&&index| index as usize >= positions.len())
    {
        return Err(malformed(format!(
            "{}: face references vertex {}, but only {} vertices exist",
            path.display(),
            u64::from(*vertex) + 1,
            positions.len()
        )));
    }
    Ok((positions, triangles))
}

/// Positions of a source-frame OBJ, in the shared display convention.
///
/// Face records are ignored: this is the positions-only contract the baked-body
/// and follow inputs have always had, so relative face syntax stays acceptable
/// here.
///
/// # Errors
/// Refuses unreadable files as I/O failures and malformed vertices or empty
/// files as invalid values.
pub fn read_obj_positions(path: &Path) -> Result<Vec<[f32; 3]>, CommandError> {
    let text = std::fs::read_to_string(path).map_err(|error| errors::io(path, error))?;
    positions_of(&text, path)
        .map(|positions| positions.into_iter().map(display_to_source).collect())
        .map_err(obj_error)
}

/// Positions and triangles of an OBJ in the NIF source scene frame.
///
/// # Errors
/// Refuses unreadable files and malformed or out-of-range geometry.
pub(crate) fn read_obj_surface(path: &Path) -> Result<ObjMesh, CommandError> {
    read_obj_mesh(path)
        .map(|(positions, triangles)| {
            (
                positions.into_iter().map(display_to_source).collect(),
                triangles,
            )
        })
        .map_err(obj_error)
}

fn read_text(path: &Path) -> Result<String, ObjError> {
    std::fs::read_to_string(path)
        .map_err(|error| ObjError::Io(format!("{}: {error}", path.display())))
}

/// Read an OBJ into the export model, keeping per-vertex UVs.
///
/// The bundle and head-compatibility paths need the UV set the position-only
/// readers drop. Positions are converted to the source NIF frame (Z-up) and
/// UVs to the NIF top-left convention, so an OBJ part and a NIF part assemble
/// in one frame. The flat model stores one UV per position, so an OBJ with
/// split UV seams (a corner whose `vt` index differs from its `v` index) is
/// refused rather than silently collapsed. OBJ carries no skin, normals or
/// materials here; those stay empty and the compatibility report says so.
///
/// # Errors
/// Refuses unreadable files, malformed vertices/UVs/faces, empty geometry,
/// out-of-range indices and split-UV seams the model cannot represent.
pub fn read_obj_asset(path: &Path) -> Result<MeshAsset, ObjError> {
    let text = read_text(path)?;
    let mut positions = Vec::new();
    let mut uvs_raw: Vec<[f32; 2]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("v ") {
            positions.push(display_to_source(read_position(rest, line, path)?));
        } else if let Some(rest) = line.strip_prefix("vt ") {
            uvs_raw.push(read_uv(rest, line, path)?);
        } else if let Some(rest) = line.strip_prefix("f ") {
            triangles.extend(read_face_with_uv(rest, path)?);
        }
    }
    if positions.is_empty() {
        return Err(malformed(format!("{}: no vertex lines", path.display())));
    }
    if let Some(vertex) = triangles
        .iter()
        .flatten()
        .find(|&&index| index as usize >= positions.len())
    {
        return Err(malformed(format!(
            "{}: face references vertex {}, but only {} vertices exist",
            path.display(),
            u64::from(*vertex) + 1,
            positions.len()
        )));
    }
    let uvs = if uvs_raw.is_empty() {
        Vec::new()
    } else if uvs_raw.len() == positions.len() {
        uvs_raw.iter().map(|[u, v]| [*u, 1.0 - v]).collect()
    } else {
        return Err(malformed(format!(
            "{}: {} UVs for {} vertices; the model needs one UV per vertex",
            path.display(),
            uvs_raw.len(),
            positions.len()
        )));
    };
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let triangle_count = triangles.len();
    Ok(MeshAsset {
        name,
        positions,
        uvs,
        triangles,
        triangle_partitions: vec![PartitionId(0); triangle_count],
        ..MeshAsset::default()
    })
}

fn read_uv(rest: &str, line: &str, path: &Path) -> Result<[f32; 2], ObjError> {
    let mut parts = rest.split_whitespace();
    let mut coordinate = || -> Result<f32, ObjError> {
        let value = parts
            .next()
            .ok_or_else(|| malformed(format!("{}: incomplete UV line `{line}`", path.display())))?;
        value
            .parse::<f32>()
            .map_err(|error| malformed(format!("{}: `{value}`: {error}", path.display())))
    };
    Ok([coordinate()?, coordinate()?])
}

/// Fan one face to triangles, capturing position indices and checking that any
/// UV index matches its position index (the flat model has no split UV seams).
fn read_face_with_uv(rest: &str, path: &Path) -> Result<Vec<[u32; 3]>, ObjError> {
    let mut corners = Vec::new();
    for corner in rest.split_whitespace() {
        let mut fields = corner.split('/');
        let vertex = fields
            .next()
            .ok_or_else(|| malformed(format!("{}: empty face corner", path.display())))?;
        let one_based = vertex.parse::<u32>().map_err(|_| {
            malformed(format!(
                "{}: face corner `{vertex}` is not a vertex index",
                path.display()
            ))
        })?;
        let index = one_based.checked_sub(1).ok_or_else(|| {
            malformed(format!(
                "{}: face corner `{vertex}` is not 1-based",
                path.display()
            ))
        })?;
        if let Some(uv) = fields.next().filter(|field| !field.is_empty()) {
            let uv_one_based = uv.parse::<u32>().map_err(|_| {
                malformed(format!(
                    "{}: UV index `{uv}` is not a number",
                    path.display()
                ))
            })?;
            let uv_index = uv_one_based.checked_sub(1).ok_or_else(|| {
                malformed(format!(
                    "{}: UV index `{uv}` is not 1-based",
                    path.display()
                ))
            })?;
            if uv_index != index {
                return Err(malformed(format!(
                    "{}: corner `{corner}` splits UVs (v{} vs vt{}); the model stores one UV per vertex",
                    path.display(),
                    one_based,
                    uv_one_based
                )));
            }
        }
        corners.push(index);
    }
    if corners.len() < 3 {
        return Err(malformed(format!(
            "{}: face has fewer than three corners",
            path.display()
        )));
    }
    Ok(corners[1..]
        .windows(2)
        .map(|edge| [corners[0], edge[0], edge[1]])
        .collect())
}

fn positions_of(text: &str, path: &Path) -> Result<Vec<[f32; 3]>, ObjError> {
    let mut positions = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("v ") {
            positions.push(read_position(rest, line, path)?);
        }
    }
    if positions.is_empty() {
        return Err(malformed(format!("{}: no vertex lines", path.display())));
    }
    Ok(positions)
}

fn read_position(rest: &str, line: &str, path: &Path) -> Result<[f32; 3], ObjError> {
    let mut parts = rest.split_whitespace();
    let mut coordinate = || -> Result<f32, ObjError> {
        let value = parts.next().ok_or_else(|| {
            malformed(format!(
                "{}: incomplete vertex line `{line}`",
                path.display()
            ))
        })?;
        value
            .parse::<f32>()
            .map_err(|error| malformed(format!("{}: `{value}`: {error}", path.display())))
    };
    Ok([coordinate()?, coordinate()?, coordinate()?])
}

fn read_face(rest: &str, path: &Path) -> Result<Vec<[u32; 3]>, ObjError> {
    let mut corners = Vec::new();
    for corner in rest.split_whitespace() {
        let index = corner
            .split('/')
            .next()
            .ok_or_else(|| malformed(format!("{}: empty face corner", path.display())))?;
        let one_based = index.parse::<u32>().map_err(|_| {
            malformed(format!(
                "{}: face corner `{index}` is not a vertex index",
                path.display()
            ))
        })?;
        corners.push(one_based.checked_sub(1).ok_or_else(|| {
            malformed(format!(
                "{}: face corner `{index}` is not 1-based",
                path.display()
            ))
        })?);
    }
    if corners.len() < 3 {
        return Err(malformed(format!(
            "{}: face has fewer than three corners",
            path.display()
        )));
    }
    Ok(corners[1..]
        .windows(2)
        .map(|edge| [corners[0], edge[0], edge[1]])
        .collect())
}

fn malformed(message: String) -> ObjError {
    ObjError::Malformed(message)
}

fn obj_error(error: ObjError) -> CommandError {
    match error {
        ObjError::Io(message) => CommandError::new(ErrorKind::Io, "io_error", message),
        ObjError::Malformed(message) => errors::invalid("obj", message),
    }
}

/// Source NIF positions are Z-up; every OBJ here uses the `(x, z, -y)` display
/// frame, so this is the one conversion between them.
fn display_to_source([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, -z, y]
}

pub fn pick_shape<'a>(
    shapes: &'a [NifShape],
    name: Option<&str>,
) -> Result<&'a NifShape, CommandError> {
    pick_shape_index(shapes, name).map(|index| &shapes[index])
}

pub(crate) fn pick_shape_index(
    shapes: &[NifShape],
    name: Option<&str>,
) -> Result<usize, CommandError> {
    match name {
        Some(name) => shapes
            .iter()
            .position(|shape| shape.name == name)
            .ok_or_else(|| errors::not_found("shape", name)),
        None => shapes
            .iter()
            .enumerate()
            .max_by_key(|(_, shape)| shape.positions.len())
            .map(|(index, _)| index)
            .ok_or_else(|| errors::invalid("nif", "the file has no shapes")),
    }
}

/// Wavefront OBJ in a Y-up display frame: source positions are Z-up, so they are
/// converted as `(x, z, -y)` here. Both comparison exports use the same frame.
pub fn write_obj(
    path: &Path,
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
) -> std::io::Result<()> {
    write_obj_uv(path, positions, triangles, &[])
}

/// Like [`write_obj`], plus per-vertex UVs as `vt`/`v/vt` pairs when the
/// source shape authors them. NIF UVs are D3D-style (top-left origin), so
/// `v` is emitted as `1 - v` for OBJ's bottom-left convention.
pub fn write_obj_uv(
    path: &Path,
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    uvs: &[[f32; 2]],
) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(
        out,
        "# sculpto: Y-up display frame from source Z-up (x, z, -y)"
    )?;
    for [x, y, z] in positions {
        writeln!(out, "v {x} {z} {}", -y)?;
    }
    let with_uv = !uvs.is_empty() && uvs.len() == positions.len();
    if with_uv {
        for [u, v] in uvs {
            writeln!(out, "vt {u} {}", 1.0 - v)?;
        }
    }
    for [a, b, c] in triangles {
        if with_uv {
            let (ai, bi, ci) = (a + 1, b + 1, c + 1);
            writeln!(out, "f {ai}/{ai} {bi}/{bi} {ci}/{ci}")?;
        } else {
            writeln!(out, "f {} {} {}", a + 1, b + 1, c + 1)?;
        }
    }
    out.flush()
}
