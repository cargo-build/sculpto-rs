//! OBJ helpers shared by the argv surface and the engine handlers.

use std::io::Write;
use std::path::Path;

use sculpto_nif::NifShape;

use crate::CommandError;
use crate::errors;

/// Source NIF positions are Z-up; every OBJ here uses the `(x, z, -y)` display
/// frame, so garment, reference shape and baked body stay comparable.
pub fn read_obj_positions(path: &Path) -> Result<Vec<[f32; 3]>, CommandError> {
    let text = std::fs::read_to_string(path).map_err(|error| errors::io(path, error))?;
    let mut positions = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("v ") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let mut coordinate = || -> Result<f32, CommandError> {
            let value = parts.next().ok_or_else(|| {
                errors::invalid("obj", format!("{}: incomplete vertex line", path.display()))
            })?;
            value.parse::<f32>().map_err(|error| {
                errors::invalid("obj", format!("{}: `{value}`: {error}", path.display()))
            })
        };
        let [x, y, z] = [coordinate()?, coordinate()?, coordinate()?];
        positions.push([x, -z, y]);
    }
    if positions.is_empty() {
        return Err(errors::invalid(
            "obj",
            format!("{}: no vertex lines", path.display()),
        ));
    }
    Ok(positions)
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
