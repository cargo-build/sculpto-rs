//! Neutral densify as an engine operation: OBJ in, densified OBJ out, and the
//! measured diagnostic report. The mesh owner is
//! [`sculpto_mesh::densify`]; this module is the file and JSON surface.

use std::fs;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde_json::{Value, json};

use sculpto_mesh::densify::{DensifyError, DensifyParams as StageParams, DensifyReport, densify};

use crate::obj::{display_to_source, read_obj_mesh, write_obj};
use crate::{CommandError, ErrorKind, errors};

use super::dress::obj_error;

/// Everything `mesh.densify` needs.
///
/// The command is the product surface over [`sculpto_mesh::densify`]: the
/// authored mesh with more triangles, optionally with interior smoothing that
/// never moves a boundary loop. It has no body, band or clearance parameter on
/// purpose — it cannot answer a fit question, and the report says so.
/// Absent parameters keep the module's documented defaults. @req DECL0013
#[derive(Debug, Clone, JsonSchema)]
pub struct DensifyParams {
    /// OBJ mesh to densify: positions and faces; multi-object files read
    /// whole and their parts stay separate components.
    pub r#in: PathBuf,
    /// Output OBJ: the densified mesh, same frame as the input.
    pub out: PathBuf,
    /// Midpoint subdivision rounds, `0..=2` (default 1).
    pub cuts: Option<u32>,
    /// Laplacian passes over the interior, `0..=20`; boundary loops never
    /// move (default 0: pure densify).
    pub smooth_iterations: Option<u32>,
    /// Hard cumulative per-vertex displacement bound of smoothing, in input
    /// units; exceeded refuses and leaves `out` unwritten (default: unbounded).
    pub smooth_max_shift: Option<f32>,
    /// Optional report path: the same JSON object the command returns.
    pub report: Option<PathBuf>,
}

/// One densified mesh and what the stage measured.
#[derive(Debug, Clone, PartialEq)]
pub struct DensifyOutcome {
    /// Written OBJ.
    pub out: PathBuf,
    /// Mesh that was densified.
    pub r#in: PathBuf,
    /// The mesh and its measured invariants.
    pub mesh: DensifyReport,
}

impl DensifyOutcome {
    /// The JSON body `mesh.densify` returns and writes to `report`.
    ///
    /// `diagnostic` is the no-fit marker: the result has no band, clearance or
    /// fit field anywhere, and the note says why. `uv` and `material` name the
    /// attributes the OBJ round trip does not carry, the way the seal report
    /// does. @req DECL0013
    pub fn to_json(&self) -> Value {
        let mesh = &self.mesh;
        let smoothing = &mesh.smoothing;
        json!({
            "out": self.out.display().to_string(),
            "in": self.r#in.display().to_string(),
            "source_vertices": mesh.source_vertices,
            "source_triangles": mesh.source_triangles,
            "cuts": mesh.cuts,
            "vertices": mesh.vertices,
            "triangles": mesh.triangles,
            "components": {
                "indexed": {
                    "before": mesh.components_indexed_before,
                    "after": mesh.components_indexed_after,
                },
                "exact_position": {
                    "before": mesh.components_exact_position_before,
                    "after": mesh.components_exact_position_after,
                },
            },
            "openings": {
                "before": openings(&mesh.openings_before),
                "after": openings(&mesh.openings_after),
            },
            "boundary_edges": {
                "before": mesh.boundary_edges_before,
                "after": mesh.boundary_edges_after,
            },
            "boundary_vertices": {
                "before": mesh.boundary_vertices_before,
                "after": mesh.boundary_vertices_after,
            },
            "coincident_groups": {
                "before": mesh.coincident_groups_before,
                "after": mesh.coincident_groups_after,
            },
            "boundary_max_drift": mesh.boundary_max_drift,
            "smoothing": {
                "enabled": smoothing.enabled,
                "iterations": smoothing.iterations,
                "max_shift": smoothing.max_shift,
                "max_displacement": smoothing.max_displacement,
                "boundary_max_displacement": smoothing.boundary_max_displacement,
                "changed_vertices": smoothing.changed_vertices,
                "flipped_triangles": smoothing.flipped_triangles,
            },
            "uv": "absent",
            "material": "not_carried",
            "diagnostic": true,
            "note": "diagnostic densified geometry: authored vertex positions of the \
                     first source vertices are preserved, nothing was fitted, and \
                     this output is not a fit result",
        })
    }
}

/// Boundary-loop openings as report rows.
fn openings(rows: &[sculpto_mesh::intake::Opening]) -> Vec<Value> {
    rows.iter()
        .map(|opening| {
            json!({
                "edges": opening.edges,
                "center": opening.center,
            })
        })
        .collect()
}

/// Densify one OBJ mesh and write it.
///
/// # Errors
///
/// Refuses unreadable or malformed OBJs as their own classified failures, a
/// topology subdivision would change as `densify_degenerate_face` /
/// `densify_non_manifold_edge`, every out-of-range parameter as
/// `densify_parameter`, and a smoothing stage beyond its displacement bound as
/// `densify_smooth_shift`; no output file is written on any failure.
pub fn densify_mesh(params: &DensifyParams) -> Result<DensifyOutcome, CommandError> {
    let source =
        read_obj_mesh(&params.r#in).map_err(|error| obj_error("in", &params.r#in, &error))?;
    let defaults = StageParams::default();
    let stage = StageParams {
        cuts: params.cuts.unwrap_or(defaults.cuts),
        smooth_iterations: params
            .smooth_iterations
            .unwrap_or(defaults.smooth_iterations),
        smooth_max_shift: params.smooth_max_shift.or(defaults.smooth_max_shift),
    };
    let (positions, triangles, report) =
        densify(&source.0, &source.1, &stage).map_err(|error| densify_refused(&error))?;
    if let Some(parent) = params.out.parent() {
        fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
    }
    write_obj(
        &params.out,
        &positions
            .iter()
            .map(|&point| display_to_source(point))
            .collect::<Vec<_>>(),
        &triangles,
    )
    .map_err(|error| errors::io(&params.out, error))?;
    let outcome = DensifyOutcome {
        out: params.out.clone(),
        r#in: params.r#in.clone(),
        mesh: report,
    };
    if let Some(path) = &params.report {
        let text = serde_json::to_string_pretty(&outcome.to_json())
            .expect("the densify report is a JSON value");
        fs::write(path, text).map_err(|error| errors::io(path, error))?;
    }
    Ok(outcome)
}

/// The domain refusal, classified by the variant that produced it.
fn densify_refused(error: &DensifyError) -> CommandError {
    let (kind, code) = match error {
        DensifyError::EmptyMesh
        | DensifyError::EmptyFaces
        | DensifyError::NonFinitePosition { .. }
        | DensifyError::FaceIndex { .. } => (ErrorKind::Refused, "densify_input"),
        DensifyError::DegenerateFace { .. } => (ErrorKind::Refused, "densify_degenerate_face"),
        DensifyError::NonManifoldEdge { .. } => (ErrorKind::Refused, "densify_non_manifold_edge"),
        DensifyError::Cuts(_)
        | DensifyError::SmoothIterations(_)
        | DensifyError::SmoothMaxShift(_) => (ErrorKind::InvalidParams, "densify_parameter"),
        DensifyError::ShiftLimit { .. } => (ErrorKind::Refused, "densify_smooth_shift"),
        DensifyError::NonFiniteResult { .. } => (ErrorKind::Unsupported, "densify_non_finite"),
    };
    CommandError::new(kind, code, error.to_string())
}
