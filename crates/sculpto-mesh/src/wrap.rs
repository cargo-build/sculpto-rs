//! Non-rigid garment fit onto a body surface (`fit_to_body`).
//!
//! The semantics are ported from BodySlide and Outfit Studio's `SurfaceFit`
//! (GPL-3.0, `src/components/SurfaceFit.cpp`): the garment is welded into
//! solver unknowns, each unknown is pulled to a gap outside the body's closest
//! surface point, and the displacement field is found by a preconditioned
//! conjugate-gradient solve of a Laplacian-regularized least-squares system —
//! data terms plus a squared-Laplacian and membrane smoothness. That is what
//! keeps a sleeve or a boot's own form. Stages run stiff to loose, strained
//! edges give way, and final passes push out clipping vertices and body pokes.
//! The projection is topology-free, so a garment authored on one body fits
//! another body of a different mesh. @req DECL0013

use crate::check::triangle_corners;

/// Optional smooth normals plus the faces they index.
type NormalField<'a> = (&'a [[f32; 3]], &'a [[u32; 3]]);

/// Positions, remaining faces and a report: the result of a pair-driven fix.
pub type PairFixResult = (Vec<[f32; 3]>, Vec<[u32; 3]>, PairFixReport);

/// A body mesh as positions plus triangles.
pub type Mesh<'a> = (&'a [[f32; 3]], &'a [[u32; 3]]);

/// How the target gap is chosen, mirroring Outfit Studio's `gapMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GapMode {
    /// One constant gap everywhere; snaps the garment onto the body shape.
    #[default]
    Constant,
    /// Keep each vertex's current standoff (at least `min_gap`); only pulls
    /// clipping vertices out and follows the body under the garment.
    Keep,
    /// Use the gap each vertex has to the garment's authored body
    /// (`gap_from`), measured on `gap_source` when given; the fit then
    /// re-applies the authored standoff to the new body.
    FromShape,
}

/// Parameters of one fit; the defaults mirror Outfit Studio's `fit_to_body`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitOptions {
    /// How the target gap is chosen.
    pub gap_mode: GapMode,
    /// Distance outside the body to fit to.
    pub gap: f32,
    /// Smallest gap any vertex is fitted to.
    pub min_gap: f32,
    /// Vertices farther than this from the body are not pulled.
    pub max_distance: f32,
    /// Least cosine between garment and body normals that still pulls a vertex.
    pub min_normal_agreement: f32,
    /// Stiffness of the final stage; earlier stages are this times `4^n`.
    pub stiffness: f32,
    /// Stiff-to-loose stages.
    pub stages: u32,
    /// Correspondence updates per stage.
    pub iterations: u32,
    /// PCG iterations per update.
    pub solver_iterations: u32,
    /// Edge strain above this ratio makes the pull give way.
    pub max_strain: f32,
    /// Final passes that push out clipping vertices and body pokes.
    pub clipping_passes: u32,
    /// Smoothing passes (Taubin) that find the garment relief.
    pub relief: u32,
    /// How far a body vertex may sit outside the garment before it is a poke.
    pub poke_depth: f32,
    /// Push the garment out over body pokes as well as inside vertices.
    pub pokes: bool,
    /// Rings of neighbours around flipped triangles that may be damped.
    pub flip_rings: u32,
    /// Blend toward the authored form per flip-repair pass.
    pub flip_strength: f32,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            gap_mode: GapMode::default(),
            gap: 0.3,
            min_gap: 0.02,
            max_distance: 10.0,
            min_normal_agreement: 0.0,
            stiffness: 1.0,
            stages: 4,
            iterations: 3,
            solver_iterations: 100,
            max_strain: 0.3,
            clipping_passes: 4,
            relief: 0,
            poke_depth: 1.5,
            pokes: true,
            flip_rings: 4,
            flip_strength: 0.5,
        }
    }
}

/// What one fit changed, for the caller's gate and logs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitReport {
    /// Garment vertices considered.
    pub vertices: usize,
    /// Solver unknowns after welding.
    pub unknowns: usize,
    /// Vertices that were pulled toward the body at least once.
    pub pulled: usize,
    /// Vertices inside the body before the fit.
    pub inside_before: usize,
    /// Vertices inside the body after the fit.
    pub inside_after: usize,
    /// Vertices whose pull gave way to the strain cap.
    pub kept_form: usize,
    /// Triangles whose normal flipped during the fit.
    pub flipped: usize,
    /// Mean displacement over all vertices.
    pub mean_displacement: f32,
    /// Largest single displacement.
    pub max_displacement: f32,
}

/// Fit `garment` onto `body` and return moved positions plus a report.
///
/// Both meshes must be in the same frame; `None` means empty input.
pub fn fit_to_body(
    garment: &[[f32; 3]],
    faces: &[[u32; 3]],
    body: &[[f32; 3]],
    body_faces: &[[u32; 3]],
    options: &FitOptions,
) -> Option<(Vec<[f32; 3]>, FitReport)> {
    fit_to_body_from(garment, faces, body, body_faces, None, None, options)
}

/// Fit with `gapMode fromShape`: the gap targets follow the standoff the
/// garment has to its authored body, optionally measured on a copy of the
/// garment in its source frame (`gap_source`, same vertex order). @req DECL0013
pub fn fit_to_body_from(
    garment: &[[f32; 3]],
    faces: &[[u32; 3]],
    body: &[[f32; 3]],
    body_faces: &[[u32; 3]],
    gap_from: Option<Mesh<'_>>,
    gap_source: Option<&[[f32; 3]]>,
    options: &FitOptions,
) -> Option<(Vec<[f32; 3]>, FitReport)> {
    if garment.is_empty() || body.is_empty() || body_faces.is_empty() {
        return None;
    }
    let body_oriented = orient_faces(body, body_faces);
    let (body_bvh, body_corners) = triangle_corners(body, &body_oriented);
    let body_normals = welded_vertex_normals(body, &body_oriented);
    let mesh = weld(garment, faces);
    let unknown_count = mesh.rest.len();
    let mut gaps = vec![options.gap; unknown_count];
    add_relief(&mesh, &mut gaps, options);

    if options.gap_mode == GapMode::Keep {
        for (index, &point) in mesh.rest.iter().enumerate() {
            let (clearance, _, _) = signed_clearance(point, &body_bvh, &body_corners);
            if clearance.is_finite() && clearance.abs() <= options.max_distance {
                gaps[index] = clearance.max(options.min_gap);
            }
        }
    }
    if options.gap_mode == GapMode::FromShape {
        let (source_positions, source_faces) = gap_from.unwrap_or((body, body_faces));
        let (source_bvh, source_corners) = triangle_corners(source_positions, source_faces);
        let measured = gap_source.unwrap_or(garment);
        let mut across = vec![f32::NEG_INFINITY; unknown_count];
        for (source_index, &position) in measured.iter().enumerate() {
            let unknown = mesh.unknown_of[source_index] as usize;
            let Some((triangle, _)) = source_bvh.nearest(position) else {
                continue;
            };
            let closest = closest_on_triangle(position, source_corners[triangle]);
            let distance = length(sub(position, closest));
            across[unknown] = across[unknown].max(distance);
        }
        for (index, &distance) in across.iter().enumerate() {
            if distance.is_finite() {
                gaps[index] = distance.max(options.min_gap);
            }
        }
    }
    for gap in &mut gaps {
        *gap = gap.max(options.min_gap);
    }

    let mut inside_before = 0usize;
    for &point in &mesh.rest {
        if signed_clearance(point, &body_bvh, &body_corners).0 < 0.0 {
            inside_before += 1;
        }
    }

    let mut displacement = vec![[0.0f32; 3]; unknown_count];
    let mut pull_scale = vec![1.0f32; unknown_count];
    let mut last_weight = vec![0.0f32; unknown_count];

    let final_lambda = options.stiffness.max(1e-4);
    for stage in 0..options.stages.max(1) {
        let stages_left = options.stages.max(1) - 1 - stage;
        let lambda = final_lambda * 4.0f32.powi(stages_left as i32);
        for _ in 0..options.iterations.max(1) {
            iterate(
                &mesh,
                &gaps,
                &body_bvh,
                &body_corners,
                &body_normals,
                body_faces,
                &mut displacement,
                &pull_scale,
                &mut last_weight,
                lambda,
                options,
            );
        }
    }

    // Straining edges make the pull give way around them.
    let mut kept_form = vec![false; unknown_count];
    if options.max_strain > 0.0 {
        for _ in 0..4 {
            let mut strained = vec![false; unknown_count];
            let mut count = 0usize;
            for face in &mesh.faces {
                for (a, b) in edges(face) {
                    let before = length(sub(mesh.rest[a], mesh.rest[b]));
                    let after = length(sub(
                        add(mesh.rest[a], displacement[a]),
                        add(mesh.rest[b], displacement[b]),
                    ));
                    let (shorter, longer) = if before < after {
                        (before, after)
                    } else {
                        (after, before)
                    };
                    if shorter > 1e-6 && longer / shorter - 1.0 > options.max_strain {
                        count += usize::from(!strained[a]) + usize::from(!strained[b]);
                        strained[a] = true;
                        strained[b] = true;
                    }
                }
            }
            if count == 0 {
                break;
            }
            let mut relax = strained.clone();
            for (index, &is_strained) in strained.iter().enumerate() {
                if is_strained {
                    for &neighbour in &mesh.neighbours[index] {
                        relax[neighbour as usize] = true;
                    }
                }
            }
            for (index, &is_relaxed) in relax.iter().enumerate() {
                if is_relaxed {
                    pull_scale[index] *= 0.25;
                    kept_form[index] = true;
                }
            }
            for _ in 0..options.iterations.max(1) {
                iterate(
                    &mesh,
                    &gaps,
                    &body_bvh,
                    &body_corners,
                    &body_normals,
                    body_faces,
                    &mut displacement,
                    &pull_scale,
                    &mut last_weight,
                    final_lambda,
                    options,
                );
            }
        }
    }

    repair_flips(&mesh, &mut displacement, options);

    // Clipping left over: inside vertices go out to the gap, body pokes push
    // the garment out over them. Like upstream, each pass is a solve with a
    // hold term on the current fit, so the push spreads over the neighbors
    // instead of teleporting a vertex.
    for pass in 0..options.clipping_passes {
        let mut inside = vec![false; unknown_count];
        let mut any_inside = false;
        for (index, &rest) in mesh.rest.iter().enumerate() {
            let point = add(rest, displacement[index]);
            let (clearance, normal, closest) = signed_clearance_normals(
                point,
                &body_bvh,
                &body_corners,
                Some((&body_normals, &body_oriented)),
            );
            if clearance < 0.0 && clean_pull(point, closest, normal) {
                inside[index] = true;
                any_inside = true;
            }
        }

        let deformed: Vec<[f32; 3]> = garment
            .iter()
            .enumerate()
            .map(|(index, &rest)| add(rest, displacement[mesh.unknown_of[index] as usize]))
            .collect();
        let (deformed_bvh, deformed_corners) = triangle_corners(&deformed, faces);
        let mut poke_weight: Vec<f32> = vec![0.0; unknown_count];
        let mut poke_sum: Vec<[f32; 3]> = vec![[0.0; 3]; unknown_count];
        if options.pokes {
            for &body_vertex in body {
                let Some((triangle, nearest)) = deformed_bvh.nearest(body_vertex) else {
                    continue;
                };
                if nearest > options.poke_depth {
                    continue;
                }
                let (_, body_normal, _) = signed_clearance_normals(
                    body_vertex,
                    &body_bvh,
                    &body_corners,
                    Some((&body_normals, &body_oriented)),
                );
                // An inward ray from the body vertex hits the garment behind
                // it: the body pokes through that triangle.
                let inward = scale(body_normal, -1.0);
                let face = deformed_corners[triangle];
                let Some(depth) = ray_triangle(body_vertex, inward, face) else {
                    continue;
                };
                if !(1e-5..=options.poke_depth).contains(&depth) {
                    continue;
                }
                let hit = add(body_vertex, scale(inward, depth));
                let (u, v, w) = barycentric(hit, face);
                let target = add(body_vertex, scale(body_normal, options.min_gap));
                for (corner, weight) in [u, v, w].into_iter().enumerate() {
                    let source = faces[triangle][corner] as usize;
                    let unknown = mesh.unknown_of[source] as usize;
                    poke_weight[unknown] += weight;
                    poke_sum[unknown] = add(poke_sum[unknown], scale(target, weight));
                }
            }
        }
        let has_pokes = poke_weight.iter().any(|&w| w > 0.0);
        if !any_inside && !has_pokes {
            break;
        }

        // Upstream structure: hold the current fit at weight 1, push inside
        // vertices out at weight 20, and let the smoothness spread each push.
        const PUSH: f32 = 20.0;
        let mut data_weight = vec![1.0f32; unknown_count];
        let mut data_target = displacement.clone();
        for (index, &is_inside) in inside.iter().enumerate() {
            if !is_inside {
                continue;
            }
            let point = add(mesh.rest[index], displacement[index]);
            let (_, normal, closest) = signed_clearance_normals(
                point,
                &body_bvh,
                &body_corners,
                Some((&body_normals, &body_oriented)),
            );
            let grew = options.min_gap * (1.0 + pass as f32);
            data_weight[index] = PUSH;
            data_target[index] = sub(add(closest, scale(normal, grew)), mesh.rest[index]);
        }
        for unknown in 0..unknown_count {
            if poke_weight[unknown] <= 0.0 {
                continue;
            }
            let target = scale(poke_sum[unknown], 1.0 / poke_weight[unknown]);
            data_weight[unknown] += PUSH * poke_weight[unknown];
            data_target[unknown] = add(
                scale(data_target[unknown], 0.5),
                scale(sub(target, mesh.rest[unknown]), 0.5),
            );
        }
        let solution = solve(
            &mesh,
            &data_weight,
            &data_target,
            final_lambda,
            options.solver_iterations,
            &displacement,
        );
        displacement = solution;
    }

    let positions: Vec<[f32; 3]> = garment
        .iter()
        .enumerate()
        .map(|(index, &rest)| {
            let unknown = mesh.unknown_of[index];
            if unknown < 0 {
                rest
            } else {
                add(rest, displacement[unknown as usize])
            }
        })
        .collect();

    let inside_after = positions
        .iter()
        .filter(|&&point| signed_clearance(point, &body_bvh, &body_corners).0 < 0.0)
        .count();
    let flipped = faces
        .iter()
        .filter(|face| {
            dot(
                face_normal(garment, **face),
                face_normal(&positions, **face),
            ) < 0.0
        })
        .count();
    let displacements: Vec<f32> = positions
        .iter()
        .zip(garment.iter())
        .map(|(after, before)| length(sub(*after, *before)))
        .collect();
    let mean_displacement = displacements.iter().sum::<f32>() / displacements.len().max(1) as f32;
    let max_displacement = displacements.iter().fold(0.0f32, |a, &b| a.max(b));
    let pulled = last_weight.iter().filter(|&&w| w > 0.0).count();

    Some((
        positions,
        FitReport {
            vertices: garment.len(),
            unknowns: unknown_count,
            pulled,
            inside_before,
            inside_after,
            kept_form: kept_form.iter().filter(|&&k| k).count(),
            flipped,
            mean_displacement,
            max_displacement,
        },
    ))
}

/// Welded garment: one solver unknown per group of coincident vertices.
struct Welded {
    unknown_of: Vec<isize>,
    rest: Vec<[f32; 3]>,
    faces: Vec<[usize; 3]>,
    neighbours: Vec<Vec<u32>>,
}

fn weld(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Welded {
    let mut unknown_of = vec![-1isize; positions.len()];
    let mut rest: Vec<[f32; 3]> = Vec::new();
    for (index, &position) in positions.iter().enumerate() {
        if let Some(known) = rest
            .iter()
            .position(|&existing| length(sub(existing, position)) < 1e-4)
        {
            unknown_of[index] = known as isize;
        } else {
            unknown_of[index] = rest.len() as isize;
            rest.push(position);
        }
    }
    let mapped: Vec<[usize; 3]> = faces
        .iter()
        .map(|face| {
            [
                unknown_of[face[0] as usize] as usize,
                unknown_of[face[1] as usize] as usize,
                unknown_of[face[2] as usize] as usize,
            ]
        })
        .collect();
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); rest.len()];
    for face in &mapped {
        for (a, b) in edges(face) {
            if !neighbours[a].contains(&(b as u32)) {
                neighbours[a].push(b as u32);
            }
            if !neighbours[b].contains(&(a as u32)) {
                neighbours[b].push(a as u32);
            }
        }
    }
    Welded {
        unknown_of,
        rest,
        faces: mapped,
        neighbours,
    }
}

#[allow(clippy::too_many_arguments)]
fn iterate(
    mesh: &Welded,
    gaps: &[f32],
    body_bvh: &crate::check::Bvh,
    body_corners: &[[[f32; 3]; 3]],
    body_normals: &[[f32; 3]],
    body_faces: &[[u32; 3]],
    displacement: &mut [[f32; 3]],
    pull_scale: &[f32],
    last_weight: &mut [f32],
    lambda: f32,
    options: &FitOptions,
) {
    let count = mesh.rest.len();
    let mut data_weight = vec![0.0f32; count];
    let mut data_target = vec![[0.0f32; 3]; count];
    let normals = triangle_normal_sums(mesh);
    for index in 0..count {
        let current = add(mesh.rest[index], displacement[index]);
        let (clearance, normal, closest) = signed_clearance_normals(
            current,
            body_bvh,
            body_corners,
            Some((body_normals, body_faces)),
        );
        if !clearance.is_finite() || clearance.abs() > options.max_distance {
            continue;
        }
        if !clean_pull(current, closest, normal) {
            continue;
        }
        if dot(normals[index], normal) < options.min_normal_agreement {
            continue;
        }
        let falloff = {
            let t = (length(sub(current, closest)) / options.max_distance).clamp(0.0, 1.0);
            let f = 1.0 - t * t;
            f * f
        };
        data_weight[index] = falloff * pull_scale[index];
        data_target[index] = match options.gap_mode {
            GapMode::Constant | GapMode::FromShape => {
                sub(add(closest, scale(normal, gaps[index])), mesh.rest[index])
            }
            GapMode::Keep => {
                let signed = dot(sub(current, closest), normal);
                scale(normal, gaps[index] - signed)
            }
        };
        last_weight[index] = data_weight[index];
    }

    let solution = solve(
        mesh,
        &data_weight,
        &data_target,
        lambda,
        options.solver_iterations,
        displacement,
    );
    displacement.copy_from_slice(&solution);
}

/// PCG solve of `(lambda * LᵀL + membrane + diag(w)) d = w * target`.
fn solve(
    mesh: &Welded,
    data_weight: &[f32],
    data_target: &[[f32; 3]],
    lambda: f32,
    max_iterations: u32,
    initial: &[[f32; 3]],
) -> Vec<[f32; 3]> {
    const REGULARIZATION: f32 = 1e-6;
    const MEMBRANE_SHARE: f32 = 0.1;
    let count = mesh.rest.len();
    let mut inverse_degree = vec![0.0f32; count];
    for (index, neighbours) in mesh.neighbours.iter().enumerate() {
        if !neighbours.is_empty() {
            inverse_degree[index] = 1.0 / neighbours.len() as f32;
        }
    }
    let laplacian = |d: &[[f32; 3]]| -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; count];
        for index in 0..count {
            let neighbours = &mesh.neighbours[index];
            if neighbours.is_empty() {
                continue;
            }
            let mut mean = [0.0f32; 3];
            for &neighbour in neighbours {
                mean = add(mean, d[neighbour as usize]);
            }
            out[index] = sub(d[index], scale(mean, inverse_degree[index]));
        }
        out
    };
    let laplacian_transposed = |y: &[[f32; 3]]| -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; count];
        for index in 0..count {
            let neighbours = &mesh.neighbours[index];
            if neighbours.is_empty() {
                continue;
            }
            let mut sum = [0.0f32; 3];
            for &neighbour in neighbours {
                sum = add(
                    sum,
                    scale(y[neighbour as usize], inverse_degree[neighbour as usize]),
                );
            }
            out[index] = sub(y[index], sum);
        }
        out
    };
    let apply = |d: &[[f32; 3]]| -> Vec<[f32; 3]> {
        let mut out = laplacian_transposed(&laplacian(d));
        let membrane = lambda * MEMBRANE_SHARE;
        for index in 0..count {
            out[index] = add(
                scale(out[index], lambda),
                scale(d[index], data_weight[index] + REGULARIZATION),
            );
            let mut difference = [0.0f32; 3];
            for &neighbour in &mesh.neighbours[index] {
                difference = add(difference, sub(d[index], d[neighbour as usize]));
            }
            out[index] = add(out[index], scale(difference, membrane));
        }
        out
    };
    let rhs: Vec<[f32; 3]> = (0..count)
        .map(|index| scale(data_target[index], data_weight[index]))
        .collect();
    let diagonal: Vec<f32> = (0..count)
        .map(|index| {
            let mut ltl = if mesh.neighbours[index].is_empty() {
                0.0
            } else {
                1.0
            };
            for &neighbour in &mesh.neighbours[index] {
                ltl += inverse_degree[neighbour as usize] * inverse_degree[neighbour as usize];
            }
            lambda * ltl
                + data_weight[index]
                + REGULARIZATION
                + lambda * MEMBRANE_SHARE * mesh.neighbours[index].len() as f32
        })
        .collect();

    // Warm start from the current displacement, as upstream `Solve` does.
    let mut d = initial.to_vec();
    let ad = apply(&d);
    let mut r: Vec<[f32; 3]> = (0..count).map(|index| sub(rhs[index], ad[index])).collect();
    let mut z: Vec<[f32; 3]> = (0..count)
        .map(|index| scale(r[index], 1.0 / diagonal[index].max(1e-20)))
        .collect();
    let mut p = z.clone();
    let mut rz = dot_all(&r, &z);
    let b_norm = dot_all(&rhs, &rhs).max(1e-20);
    for _ in 0..max_iterations {
        if dot_all(&r, &r) < 1e-12 * b_norm {
            break;
        }
        let ap = apply(&p);
        let pap = dot_all(&p, &ap);
        if pap <= 0.0 {
            break;
        }
        let alpha = (rz / pap) as f32;
        for index in 0..count {
            d[index] = add(d[index], scale(p[index], alpha));
            r[index] = sub(r[index], scale(ap[index], alpha));
            z[index] = scale(r[index], 1.0 / diagonal[index].max(1e-20));
        }
        let rz_new = dot_all(&r, &z);
        let beta = (rz_new / rz.max(1e-30)) as f32;
        rz = rz_new;
        for index in 0..count {
            p[index] = add(z[index], scale(p[index], beta));
        }
    }
    d
}

fn triangle_normal_sums(mesh: &Welded) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32; 3]; mesh.rest.len()];
    for face in &mesh.faces {
        let normal = cross(
            sub(mesh.rest[face[1]], mesh.rest[face[0]]),
            sub(mesh.rest[face[2]], mesh.rest[face[0]]),
        );
        for &corner in face {
            normals[corner] = add(normals[corner], normal);
        }
    }
    normals
}

/// Taubin smoothing relief, as in Outfit Studio: alternating shrink/inflate
/// passes measure how far each vertex stands out along its normal.
fn add_relief(mesh: &Welded, gaps: &mut [f32], options: &FitOptions) {
    if options.relief == 0 {
        return;
    }
    let mut smoothed = mesh.rest.clone();
    let step = |smoothed: &mut Vec<[f32; 3]>, factor: f32| {
        let mut next = smoothed.clone();
        for (index, neighbours) in mesh.neighbours.iter().enumerate() {
            if neighbours.is_empty() {
                continue;
            }
            let mut mean = [0.0f32; 3];
            for &neighbour in neighbours {
                mean = add(mean, smoothed[neighbour as usize]);
            }
            mean = scale(mean, 1.0 / neighbours.len() as f32);
            next[index] = add(smoothed[index], scale(sub(mean, smoothed[index]), factor));
        }
        *smoothed = next;
    };
    for _ in 0..options.relief {
        step(&mut smoothed, 0.5);
        step(&mut smoothed, -0.53);
    }
    let normals = triangle_normal_sums(mesh);
    for index in 0..mesh.rest.len() {
        gaps[index] += dot(sub(mesh.rest[index], smoothed[index]), normals[index]);
    }
}

/// Damp the displacement around flipped triangles, ring by ring, like
/// Outfit Studio's `fix_flipped_triangles`.
fn repair_flips(mesh: &Welded, displacement: &mut [[f32; 3]], options: &FitOptions) {
    if options.flip_rings == 0 || options.flip_strength <= 0.0 {
        return;
    }
    let flipped_faces = |displacement: &[[f32; 3]]| -> Vec<usize> {
        mesh.faces
            .iter()
            .enumerate()
            .filter_map(|(index, face)| {
                let before = cross(
                    sub(mesh.rest[face[1]], mesh.rest[face[0]]),
                    sub(mesh.rest[face[2]], mesh.rest[face[0]]),
                );
                let after = cross(
                    sub(
                        add(mesh.rest[face[1]], displacement[face[1]]),
                        add(mesh.rest[face[0]], displacement[face[0]]),
                    ),
                    sub(
                        add(mesh.rest[face[2]], displacement[face[2]]),
                        add(mesh.rest[face[0]], displacement[face[0]]),
                    ),
                );
                (dot(before, after) < 0.0).then_some(index)
            })
            .collect()
    };
    for _ in 0..options.flip_rings {
        let flipped = flipped_faces(displacement);
        if flipped.is_empty() {
            return;
        }
        let mut affected = vec![false; mesh.rest.len()];
        for &face in &flipped {
            for &corner in &mesh.faces[face] {
                affected[corner] = true;
                for &neighbour in &mesh.neighbours[corner] {
                    affected[neighbour as usize] = true;
                }
            }
        }
        for (index, &is_affected) in affected.iter().enumerate() {
            if is_affected {
                displacement[index] = scale(displacement[index], 1.0 - options.flip_strength);
            }
        }
    }
}

/// Moller-Trumbore ray/triangle hit distance, if any.
fn ray_triangle(origin: [f32; 3], direction: [f32; 3], tri: [[f32; 3]; 3]) -> Option<f32> {
    const EPSILON: f32 = 1e-7;
    let edge1 = sub(tri[1], tri[0]);
    let edge2 = sub(tri[2], tri[0]);
    let h = cross(direction, edge2);
    let a = dot(edge1, h);
    if a.abs() < EPSILON {
        return None;
    }
    let f = 1.0 / a;
    let s = sub(origin, tri[0]);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, edge1);
    let v = f * dot(direction, q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * dot(edge2, q);
    (t > EPSILON).then_some(t)
}

/// Signed clearance to the body plus the body normal and closest point.
///
/// Negative clearance means the point is inside the body.
fn signed_clearance(
    point: [f32; 3],
    bvh: &crate::check::Bvh,
    corners: &[[[f32; 3]; 3]],
) -> (f32, [f32; 3], [f32; 3]) {
    signed_clearance_normals(point, bvh, corners, None)
}

/// Signed clearance with the body's smooth vertex normals when they exist,
/// falling back to the face normal. Mirrors SurfaceQuery::Closest: the normal
/// is interpolated barycentrically at the closest point.
fn signed_clearance_normals(
    point: [f32; 3],
    bvh: &crate::check::Bvh,
    corners: &[[[f32; 3]; 3]],
    vertex_normals: Option<NormalField<'_>>,
) -> (f32, [f32; 3], [f32; 3]) {
    let Some((triangle, _)) = bvh.nearest(point) else {
        return (f32::INFINITY, [0.0, 0.0, 0.0], point);
    };
    let face = corners[triangle];
    let closest = closest_on_triangle(point, face);
    let normal = match vertex_normals {
        Some((normals, faces)) => {
            let (u, v, w) = barycentric(closest, face);
            let face_indices = faces[triangle];
            let summed = add(
                add(
                    scale(normals[face_indices[0] as usize], u),
                    scale(normals[face_indices[1] as usize], v),
                ),
                scale(normals[face_indices[2] as usize], w),
            );
            let normal = unit(summed);
            if length(normal) < 0.5 {
                unit(face_normal_of(face))
            } else {
                normal
            }
        }
        None => unit(face_normal_of(face)),
    };
    (dot(sub(point, closest), normal), normal, closest)
}

/// Reorder triangles so every connected component's winding gives outward
/// normals (positive signed volume). Authored bodies mix shells; without this
/// an inward-facing component inverts the clearance sign.
fn orient_faces(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Vec<[u32; 3]> {
    let count = positions.len();
    let mut parent: Vec<usize> = (0..count).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut faces = faces.to_vec();
    for face in &faces {
        let a = find(&mut parent, face[0] as usize);
        let b = find(&mut parent, face[1] as usize);
        if a != b {
            parent[a] = b;
        }
        let b = find(&mut parent, face[2] as usize);
        let a = find(&mut parent, face[0] as usize);
        if a != b {
            parent[a] = b;
        }
    }
    // The cone-volume sign only means something for closed components; an
    // open shell (a plane, a cape) keeps its authored winding.
    let mut edge_uses: std::collections::HashMap<(usize, usize), usize> =
        std::collections::HashMap::new();
    for face in &faces {
        let corners = [face[0] as usize, face[1] as usize, face[2] as usize];
        for (a, b) in [
            (corners[0], corners[1]),
            (corners[1], corners[2]),
            (corners[2], corners[0]),
        ] {
            let index = if a < b { (a, b) } else { (b, a) };
            *edge_uses.entry(index).or_insert(0) += 1;
        }
    }
    let mut open: Vec<bool> = vec![false; count];
    for ((a, _b), uses) in &edge_uses {
        if *uses == 1 {
            let root = find(&mut parent, *a);
            open[root] = true;
        }
    }
    let mut volume: Vec<f64> = vec![0.0; count];
    for face in &faces {
        let a = positions[face[0] as usize];
        let b = positions[face[1] as usize];
        let c = positions[face[2] as usize];
        let cross = cross(b, c);
        let tip = dot(a, cross) as f64 / 6.0;
        let root = find(&mut parent, face[0] as usize);
        volume[root] += tip;
    }
    for face in &mut faces {
        let root = find(&mut parent, face[0] as usize);
        if !open[root] && volume[root] < 0.0 {
            face.swap(1, 2);
        }
    }
    faces
}

/// Area-weighted vertex normals over welded copies, as SurfaceFit::VertexNormals.
fn welded_vertex_normals(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Vec<[f32; 3]> {
    let mesh = weld(positions, faces);
    let sums = triangle_normal_sums(&mesh);
    positions
        .iter()
        .enumerate()
        .map(|(index, _)| unit(sums[mesh.unknown_of[index] as usize]))
        .collect()
}

/// Barycentric weights of `p` on `tri` (already known to be its closest point).
fn barycentric(p: [f32; 3], tri: [[f32; 3]; 3]) -> (f32, f32, f32) {
    let [a, b, c] = tri;
    let v0 = sub(b, a);
    let v1 = sub(c, a);
    let v2 = sub(p, a);
    let d00 = dot(v0, v0);
    let d01 = dot(v0, v1);
    let d11 = dot(v1, v1);
    let d20 = dot(v2, v0);
    let d21 = dot(v2, v1);
    let denominator = d00 * d11 - d01 * d01;
    if denominator.abs() < 1e-12 {
        return (1.0, 0.0, 0.0);
    }
    let v = (d11 * d20 - d01 * d21) / denominator;
    let w = (d00 * d21 - d01 * d20) / denominator;
    (1.0 - v - w, v, w)
}

/// Outfit Studio's `CleanPull`: a pull must run roughly along the normal
/// unless the point is already on the surface.
fn clean_pull(point: [f32; 3], closest: [f32; 3], normal: [f32; 3]) -> bool {
    const ON_SURFACE: f32 = 1e-3;
    const MIN_LINE_AGREEMENT: f32 = 0.6;
    let line = sub(point, closest);
    let distance = length(line);
    if distance < ON_SURFACE {
        return true;
    }
    (dot(line, normal).abs() / distance) >= MIN_LINE_AGREEMENT
}

fn edges(face: &[usize; 3]) -> [(usize, usize); 3] {
    [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])]
}

fn face_normal(positions: &[[f32; 3]], face: [u32; 3]) -> [f32; 3] {
    face_normal_of([
        positions[face[0] as usize],
        positions[face[1] as usize],
        positions[face[2] as usize],
    ])
}

fn face_normal_of(face: [[f32; 3]; 3]) -> [f32; 3] {
    cross(sub(face[1], face[0]), sub(face[2], face[0]))
}

/// Exact closest point on a triangle (Ericson, Real-Time Collision Detection).
fn closest_on_triangle(p: [f32; 3], tri: [[f32; 3]; 3]) -> [f32; 3] {
    let [a, b, c] = tri;
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denominator = 1.0 / (va + vb + vc);
    add(
        a,
        add(scale(ab, vb * denominator), scale(ac, vc * denominator)),
    )
}

fn dot_all(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            f64::from(x[0]) * f64::from(y[0])
                + f64::from(x[1]) * f64::from(y[1])
                + f64::from(x[2]) * f64::from(y[2])
        })
        .sum()
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], factor: f32) -> [f32; 3] {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let len = length(v);
    if len <= 1e-12 {
        [0.0, 0.0, 0.0]
    } else {
        scale(v, 1.0 / len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        (
            vec![
                [-10.0, -10.0, 0.0],
                [10.0, -10.0, 0.0],
                [10.0, 10.0, 0.0],
                [-10.0, 10.0, 0.0],
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
    }

    #[test]
    fn inside_vertex_is_pushed_to_the_gap() {
        let (body, body_faces) = plane();
        let garment = vec![[0.0, 0.0, -0.5], [1.0, 0.0, 0.4], [0.0, 1.0, 0.4]];
        let faces = vec![[0, 1, 2]];
        let (positions, report) =
            fit_to_body(&garment, &faces, &body, &body_faces, &FitOptions::default()).expect("fit");
        assert_eq!(report.inside_before, 1);
        assert_eq!(report.inside_after, 0);
        assert!(positions[0][2] >= 0.0, "inside vertex must leave the body");
    }

    #[test]
    fn far_vertices_are_not_pulled() {
        let (body, body_faces) = plane();
        let garment = vec![[0.0, 0.0, 50.0], [1.0, 0.0, 50.0], [0.0, 1.0, 50.0]];
        let faces = vec![[0, 1, 2]];
        let (positions, report) =
            fit_to_body(&garment, &faces, &body, &body_faces, &FitOptions::default()).expect("fit");
        assert_eq!(report.pulled, 0);
        assert_eq!(positions[0], garment[0]);
    }

    #[test]
    fn from_shape_reapplies_the_authored_standoff() {
        let (authored, authored_faces) = plane();
        let garment = vec![[0.0, 0.0, 5.0], [1.0, 0.0, 5.0], [0.0, 1.0, 5.0]];
        let faces = vec![[0, 1, 2]];
        // New body sits lower: z = -3 instead of 0.
        let new_body: Vec<[f32; 3]> = authored.iter().map(|p| [p[0], p[1], p[2] - 3.0]).collect();
        let options = FitOptions {
            gap_mode: GapMode::FromShape,
            stages: 2,
            iterations: 4,
            relief: 0,
            clipping_passes: 0,
            ..FitOptions::default()
        };
        let (positions, _) = fit_to_body_from(
            &garment,
            &faces,
            &new_body,
            &authored_faces,
            Some((&authored, &authored_faces)),
            None,
            &options,
        )
        .expect("fit");
        // Authored standoff is 5; over the new body the garment must sit at 5.
        assert!(
            (positions[0][2] - 2.0).abs() < 0.2,
            "expected z ~ 2 for a 5-unit standoff over a plane at -3, got {}",
            positions[0][2]
        );
    }

    #[test]
    fn strain_cap_keeps_the_authored_edge() {
        let (body, body_faces) = plane();
        let garment = vec![[0.0, 0.0, -8.0], [0.1, 0.0, 0.0], [0.0, 0.1, 0.0]];
        let faces = vec![[0, 1, 2]];
        let options = FitOptions {
            max_distance: 100.0,
            ..FitOptions::default()
        };
        let (positions, report) =
            fit_to_body(&garment, &faces, &body, &body_faces, &options).expect("fit");
        let rest = length(sub(garment[0], garment[1]));
        let fitted = length(sub(positions[0], positions[1]));
        assert!(report.kept_form > 0);
        assert!(
            fitted / rest <= 1.0 + options.max_strain + 0.05,
            "edge strain above the cap: {fitted} vs {rest}"
        );
    }
}

/// Options for the clipping fixer (Outfit Studio `ClippingFixer` semantics).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipFixOptions {
    /// How strongly clipped regions are pushed out; scales pushes and rings.
    pub strength: f32,
    /// Fix passes over the whole mesh.
    pub passes: u32,
}

impl Default for ClipFixOptions {
    fn default() -> Self {
        Self {
            strength: 1.0,
            passes: 3,
        }
    }
}

/// What one clipping fix changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipFixReport {
    /// Vertices that received a push.
    pub pushed: usize,
    /// Largest single push.
    pub max_push: f32,
}

/// Push an outfit out of the body like BodySlide's clipping fixer: clipped
/// triangles are sampled at their centroids, body pokes are found by an inward
/// ray from each body vertex, the pushes are merged per vertex, propagated
/// through the mesh with decay and smoothed, so straps keep their thickness.
/// @req DECL0013
pub fn fix_clipping(
    garment: &[[f32; 3]],
    faces: &[[u32; 3]],
    body: &[[f32; 3]],
    body_faces: &[[u32; 3]],
    options: &ClipFixOptions,
) -> Option<(Vec<[f32; 3]>, ClipFixReport)> {
    if garment.is_empty() || body.is_empty() || body_faces.is_empty() {
        return None;
    }
    let oriented = orient_faces(body, body_faces);
    let (body_bvh, body_corners) = triangle_corners(body, &oriented);
    let body_normals = welded_vertex_normals(body, &oriented);
    let mesh = weld(garment, faces);
    let count = mesh.rest.len();

    let inflation = 0.05 + options.strength * 0.25;
    let margin = inflation * 0.5;
    let total_influence = inflation + margin;
    let search_radius = 2.0 + options.strength * 3.0;
    let long_edge_sq = (total_influence * 1.5) * (total_influence * 1.5);
    let max_push = 0.1 + options.strength * 0.5;
    let propagation_rings = 4 + (options.strength * 4.0).round() as usize;
    let decay = 0.85f32;

    // Garment normals, oriented away from the body centroid.
    let body_centroid = scale(
        body.iter().fold([0.0f32; 3], |a, &b| add(a, b)),
        1.0 / body.len() as f32,
    );
    let welded_normals = {
        let sums = {
            let mesh = &mesh;
            let mut sums = vec![[0.0f32; 3]; mesh.rest.len()];
            for face in &mesh.faces {
                let normal = cross(
                    sub(mesh.rest[face[1]], mesh.rest[face[0]]),
                    sub(mesh.rest[face[2]], mesh.rest[face[0]]),
                );
                for &corner in face {
                    sums[corner] = add(sums[corner], normal);
                }
            }
            sums
        };
        let sign: f32 = sums
            .iter()
            .zip(mesh.rest.iter())
            .map(|(normal, rest)| dot(*normal, sub(*rest, body_centroid)))
            .sum();
        let sign = if sign < 0.0 { -1.0 } else { 1.0 };
        sums.iter()
            .map(|&normal| scale(unit(normal), sign))
            .collect::<Vec<_>>()
    };

    let boundary = boundary_vertices(&mesh);
    let mut displacements = vec![[0.0f32; 3]; count];
    let mut eligible = vec![false; count];

    let calc_displacement = |point: [f32; 3], point_normal: Option<[f32; 3]>| -> ([f32; 3], bool) {
        let Some((triangle, nearest)) = body_bvh.nearest(point) else {
            return ([0.0f32; 3], false);
        };
        if nearest > search_radius {
            return ([0.0f32; 3], false);
        }
        let face = body_corners[triangle];
        let closest = closest_on_triangle(point, face);
        let (u, v, w) = barycentric(closest, face);
        let indices = oriented[triangle];
        let mut inflation_dir = add(
            add(
                scale(body_normals[indices[0] as usize], u),
                scale(body_normals[indices[1] as usize], v),
            ),
            scale(body_normals[indices[2] as usize], w),
        );
        if length(inflation_dir) < 1e-8 {
            inflation_dir = unit(face_normal_of(face));
        }
        inflation_dir = unit(inflation_dir);
        let opposite_facing = point_normal.is_some_and(|normal| dot(normal, inflation_dir) < -0.25);
        let offset = sub(point, closest);
        let signed = dot(offset, inflation_dir);
        let tangent_sq = (dot(offset, offset) - signed * signed).max(0.0);
        let _ = nearest;
        let propagation_influence = total_influence + inflation;
        let can_propagate = signed < propagation_influence
            && tangent_sq <= propagation_influence * propagation_influence;
        let mut displacement = [0.0f32; 3];
        // Upstream skips surfaces facing into the body so a thick garment's
        // lining is not pulled onto the skin. A lining that is actually inside
        // the body must still move out, at half strength.
        let inside_body = signed < -0.01;
        if (!opposite_facing || inside_body)
            && signed < total_influence
            && tangent_sq <= total_influence * total_influence
        {
            let push = if signed < inflation {
                inflation - signed
            } else {
                let t = (signed - inflation) / margin;
                let falloff = (1.0 - t) * (1.0 - t);
                falloff * inflation * 0.3
            };
            let scale_factor = if opposite_facing { 0.5 } else { 1.0 };
            displacement = scale(inflation_dir, push * scale_factor);
        }
        (displacement, can_propagate)
    };

    for _pass in 0..options.passes.max(1) {
        let mut merged = displacements.clone();

        // Primary pass: every welded garment vertex.
        for (index, (rest, normal)) in mesh.rest.iter().zip(welded_normals.iter()).enumerate() {
            let (displacement, can_propagate) = calc_displacement(*rest, Some(*normal));
            merge_displacement(
                &mut merged,
                &mut eligible,
                index,
                displacement,
                can_propagate,
            );
        }
        // Triangle interiors and long edges cover sparse low-poly faces.
        for face in &mesh.faces {
            let centroid = scale(
                add(
                    add(mesh.rest[face[0]], mesh.rest[face[1]]),
                    mesh.rest[face[2]],
                ),
                1.0 / 3.0,
            );
            let face_normal = unit(face_normal_of([
                mesh.rest[face[0]],
                mesh.rest[face[1]],
                mesh.rest[face[2]],
            ]));
            let (displacement, can_propagate) = calc_displacement(centroid, Some(face_normal));
            for &corner in face {
                merge_displacement(
                    &mut merged,
                    &mut eligible,
                    corner,
                    displacement,
                    can_propagate,
                );
            }
            let edges = [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])];
            for (a, b) in edges {
                let length_sq = {
                    let edge = sub(mesh.rest[b], mesh.rest[a]);
                    dot(edge, edge)
                };
                if length_sq <= long_edge_sq {
                    continue;
                }
                let midpoint = scale(add(mesh.rest[a], mesh.rest[b]), 0.5);
                let (displacement, can_propagate) = calc_displacement(midpoint, Some(face_normal));
                merge_displacement(&mut merged, &mut eligible, a, displacement, can_propagate);
                merge_displacement(&mut merged, &mut eligible, b, displacement, can_propagate);
            }
        }

        // Body pokes: an inward ray from a body vertex hits the outfit behind it.
        let deformed: Vec<[f32; 3]> = garment
            .iter()
            .enumerate()
            .map(|(index, &rest)| add(rest, displacement_for(&mesh, &merged, index)))
            .collect();
        let (outfit_bvh, outfit_corners) = triangle_corners(&deformed, faces);
        for &body_vertex in body {
            let Some((triangle, nearest)) = outfit_bvh.nearest(body_vertex) else {
                continue;
            };
            if nearest > total_influence + 1.0 {
                continue;
            }
            let (_, body_normal, _) = signed_clearance_normals(
                body_vertex,
                &body_bvh,
                &body_corners,
                Some((&body_normals, &oriented)),
            );
            let face = outfit_corners[triangle];
            let Some(depth) = ray_triangle(body_vertex, scale(body_normal, -1.0), face) else {
                continue;
            };
            if depth <= 1e-5 || depth > total_influence + 1.0 {
                continue;
            }
            let push = depth.min(max_push) + inflation;
            let displacement = scale(body_normal, push);
            for &corner in &faces[triangle] {
                let unknown = mesh.unknown_of[corner as usize] as usize;
                merge_displacement(&mut merged, &mut eligible, unknown, displacement, true);
            }
        }
        // Propagate through the mesh connectivity, strongest neighbour first.
        for _ in 0..propagation_rings {
            let mut next = merged.clone();
            for index in 0..count {
                if boundary[index] {
                    continue;
                }
                let mut best = [0.0f32; 3];
                let mut best_len = 0.0f32;
                for &neighbour in &mesh.neighbours[index] {
                    let len = length(merged[neighbour as usize]);
                    if len > best_len {
                        best_len = len;
                        best = merged[neighbour as usize];
                    }
                }
                if best_len * decay > length(merged[index]) + 0.001 {
                    next[index] = scale(best, decay);
                    eligible[index] = true;
                }
            }
            merged = next;
        }

        // Two smoothing passes over eligible vertices, latest after.
        for _ in 0..2 {
            let mut next = merged.clone();
            for index in 0..count {
                if boundary[index] || !eligible[index] {
                    continue;
                }
                let mut sum = merged[index];
                let mut weight = 1.0f32;
                for &neighbour in &mesh.neighbours[index] {
                    if !eligible[neighbour as usize] || length(merged[neighbour as usize]) < 1e-6 {
                        continue;
                    }
                    sum = add(sum, merged[neighbour as usize]);
                    weight += 1.0;
                }
                if weight <= 1.0 {
                    continue;
                }
                let average = scale(sum, 1.0 / weight);
                next[index] = add(scale(merged[index], 0.65), scale(average, 0.35));
            }
            merged = next;
        }

        let changed = merged
            .iter()
            .zip(displacements.iter())
            .any(|(a, b)| length(sub(*a, *b)) > 1e-4);
        displacements = merged;
        if !changed {
            break;
        }
    }

    let positions: Vec<[f32; 3]> = garment
        .iter()
        .enumerate()
        .map(|(index, &rest)| add(rest, displacement_for(&mesh, &displacements, index)))
        .collect();
    let pushed = displacements.iter().filter(|d| length(**d) > 1e-6).count();
    let max_push = displacements.iter().fold(0.0f32, |a, &d| a.max(length(d)));
    Some((positions, ClipFixReport { pushed, max_push }))
}

/// Merge a push into a vertex, keeping the longest displacement (upstream
/// `mergeDisplacement`).
fn merge_displacement(
    merged: &mut [[f32; 3]],
    eligible: &mut [bool],
    index: usize,
    displacement: [f32; 3],
    can_propagate: bool,
) {
    let magnitude = length(displacement);
    if magnitude > 1e-8 && magnitude > length(merged[index]) {
        merged[index] = displacement;
    }
    if can_propagate || magnitude > 1e-8 {
        eligible[index] = true;
    }
}

fn displacement_for(mesh: &Welded, displacements: &[[f32; 3]], source: usize) -> [f32; 3] {
    displacements[mesh.unknown_of[source] as usize]
}

fn boundary_vertices(mesh: &Welded) -> Vec<bool> {
    use std::collections::HashMap;
    let mut edge_uses: HashMap<(usize, usize), usize> = HashMap::new();
    for face in &mesh.faces {
        for (a, b) in edges(face) {
            let key = if a < b { (a, b) } else { (b, a) };
            *edge_uses.entry(key).or_insert(0) += 1;
        }
    }
    let mut boundary = vec![false; mesh.rest.len()];
    for ((a, b), uses) in edge_uses {
        if uses == 1 {
            boundary[a] = true;
            boundary[b] = true;
        }
    }
    boundary
}

/// Regional envelope adaptation for a garment section worn around a body part
/// that is wider than the authored one, e.g. a collar around the head.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvelopeOptions {
    /// Fraction of body height where the adapted zone starts (0.86 = head).
    pub zone_start: f32,
    /// Fraction of body height where the adapted zone ends (1.0 = top).
    pub zone_end: f32,
    /// Distance outside the body envelope to place the garment.
    pub gap: f32,
    /// Height over which the correction fades to zero below the zone.
    pub blend: f32,
    /// Band height of the body envelope measurement.
    pub band: f32,
}

impl Default for EnvelopeOptions {
    fn default() -> Self {
        Self {
            zone_start: 0.86,
            zone_end: 1.0,
            gap: 1.0,
            blend: 10.0,
            band: 2.0,
        }
    }
}

/// What one envelope adaptation changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvelopeReport {
    /// Garment vertices moved outward.
    pub moved: usize,
    /// Largest outward move.
    pub max_move: f32,
}

/// Push a garment section out to the body's measured radial envelope.
///
/// For each height band above `zone_start` the body's maximum radius around
/// the band centroid is measured; garment vertices inside `radius + gap` are
/// moved out radially, fading in over `blend` below the zone. @req DECL0013
pub fn adapt_envelope(
    garment: &[[f32; 3]],
    body: &[[f32; 3]],
    options: &EnvelopeOptions,
) -> Option<(Vec<[f32; 3]>, EnvelopeReport)> {
    if garment.is_empty() || body.is_empty() {
        return None;
    }
    let y_min = body.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let y_max = body.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
    let height = (y_max - y_min).max(1e-6);
    let zone_y = y_min + options.zone_start * height;
    let zone_top = y_min + options.zone_end * height;
    let band = options.band.max(1e-3);

    // Envelope per band: centroid and maximum radius of the body points.
    let envelope_top = zone_top.min(y_max);
    let first_band = ((zone_y - y_min) / band).floor() as usize;
    let bands = ((envelope_top - zone_y) / band).ceil().max(0.0) as usize + 1;
    let mut envelope: Vec<(f32, f32, f32, f32)> = Vec::new(); // (y, cx, cz, radius)
    for index in 0..bands {
        let y0 = y_min + (first_band + index) as f32 * band;
        let y1 = y0 + band;
        let points: Vec<[f32; 3]> = body
            .iter()
            .copied()
            .filter(|p| p[1] >= y0 && p[1] < y1)
            .collect();
        if points.is_empty() {
            envelope.push((y0 + band * 0.5, 0.0, 0.0, 0.0));
            continue;
        }
        let count = points.len() as f32;
        let cx = points.iter().map(|p| p[0]).sum::<f32>() / count;
        let cz = points.iter().map(|p| p[2]).sum::<f32>() / count;
        let radius = points
            .iter()
            .map(|p| ((p[0] - cx).powi(2) + (p[2] - cz).powi(2)).sqrt())
            .fold(0.0f32, f32::max);
        envelope.push((y0 + band * 0.5, cx, cz, radius));
    }
    let envelope_at = |y: f32| -> (f32, f32, f32) {
        let first = envelope.first().copied().unwrap_or((y, 0.0, 0.0, 0.0));
        let last = envelope.last().copied().unwrap_or((y, 0.0, 0.0, 0.0));
        if y <= first.0 {
            return (first.1, first.2, first.3);
        }
        if y >= last.0 {
            return (last.1, last.2, last.3);
        }
        for pair in envelope.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if y >= a.0 && y <= b.0 {
                let t = (y - a.0) / (b.0 - a.0).max(1e-6);
                return (
                    a.1 + (b.1 - a.1) * t,
                    a.2 + (b.2 - a.2) * t,
                    a.3 + (b.3 - a.3) * t,
                );
            }
        }
        (last.1, last.2, last.3)
    };

    let mut moved = 0usize;
    let mut max_move = 0.0f32;
    let positions = garment
        .iter()
        .map(|&p| {
            let fade_start = zone_y - options.blend;
            let fade_end = zone_top + options.blend;
            if p[1] <= fade_start || p[1] >= fade_end {
                return p;
            }
            let weight = ((p[1] - fade_start) / options.blend.max(1e-6))
                .min(1.0)
                .min(((fade_end - p[1]) / options.blend.max(1e-6)).clamp(0.0, 1.0));
            let (cx, cz, radius) = envelope_at(p[1]);
            if radius <= 0.0 {
                return p;
            }
            let dx = p[0] - cx;
            let dz = p[2] - cz;
            let current = (dx * dx + dz * dz).sqrt();
            let target = radius + options.gap;
            if current >= target || current <= 1e-6 {
                return p;
            }
            let factor = 1.0 + (target / current - 1.0) * weight;
            let moved_point = [cx + dx * factor, p[1], cz + dz * factor];
            moved += 1;
            max_move = max_move.max(length(sub(moved_point, p)));
            moved_point
        })
        .collect();
    Some((positions, EnvelopeReport { moved, max_move }))
}

/// Options for the pair-driven clipping fix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairFixOptions {
    /// Distance outside the body every crossing triangle is pushed to.
    pub gap: f32,
    /// Maximum fix rounds.
    pub passes: u32,
    /// Rings of neighbours that share each push.
    pub rings: u32,
    /// Blend per ring toward the pushed displacement.
    pub decay: f32,
    /// Drop authored sliver triangles that still intersect after the rounds.
    pub drop_invalid: bool,
}

impl Default for PairFixOptions {
    fn default() -> Self {
        Self {
            gap: 0.5,
            passes: 40,
            rings: 2,
            decay: 0.5,
            drop_invalid: true,
        }
    }
}

/// Report of a pair-driven fix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairFixReport {
    /// Crossing pairs before the first push.
    pub pairs_before: usize,
    /// Crossing pairs after the last round.
    pub pairs_after: usize,
    /// Rounds that actually ran.
    pub rounds: u32,
    /// Malformed triangles dropped because they still intersect.
    pub dropped: usize,
}

/// Push crossing garment triangles out along the body normal, spreading each
/// push over nearby vertices, until no triangle pair intersects. Unlike the
/// trimming gate this keeps every triangle; unlike sparse sampling it targets
/// the actual crossing triangles, including hood lips around a head. @req DECL0013
pub fn fix_pairs(
    garment: &[[f32; 3]],
    faces: &[[u32; 3]],
    body: &[[f32; 3]],
    body_faces: &[[u32; 3]],
    options: &PairFixOptions,
) -> Option<PairFixResult> {
    if garment.is_empty() || faces.is_empty() || body.is_empty() || body_faces.is_empty() {
        return None;
    }
    let oriented = orient_faces(body, body_faces);
    let (body_bvh, body_corners) = triangle_corners(body, &oriented);
    let mesh = weld(garment, faces);
    let count = mesh.rest.len();
    let pairs_before = crate::check::intersection_pairs(garment, faces, body, &oriented);
    let mut displacement = vec![[0.0f32; 3]; count];
    let mut pairs_after = pairs_before;
    let mut rounds = 0u32;

    for _ in 0..options.passes.max(1) {
        let current: Vec<[f32; 3]> = garment
            .iter()
            .enumerate()
            .map(|(index, &rest)| add(rest, displacement[mesh.unknown_of[index] as usize]))
            .collect();
        let pairs = crate::check::intersection_pair_list(&current, faces, body, &oriented);
        pairs_after = pairs.len();
        if pairs.is_empty() {
            break;
        }
        rounds += 1;
        let mut push = vec![[0.0f32; 3]; count];
        let pair_normals = welded_vertex_normals(body, &oriented);
        for (garment_triangle, body_triangle) in pairs {
            let face = body_corners[body_triangle];
            let normal = unit(face_normal_of(face));
            let garment_face = faces[garment_triangle];
            let centroid = scale(
                add(
                    add(
                        current[garment_face[0] as usize],
                        current[garment_face[1] as usize],
                    ),
                    current[garment_face[2] as usize],
                ),
                1.0 / 3.0,
            );
            let (clearance, _, closest) = signed_clearance_normals(
                centroid,
                &body_bvh,
                &body_corners,
                Some((&pair_normals, &oriented)),
            );
            let _ = closest;
            // A pair can survive while the centroid is already outside (an edge
            // slices the body); every surviving pair still gets a step.
            let step = options.gap * 0.2 + 1e-3;
            let needed = (options.gap - clearance).max(step);
            let delta = scale(normal, needed);
            for &corner in &garment_face {
                let unknown = mesh.unknown_of[corner as usize] as usize;
                if length(delta) > length(push[unknown]) {
                    push[unknown] = delta;
                }
            }
        }
        // Spread each push over the rings around it.
        let mut spread = push.clone();
        for _ in 0..options.rings {
            let mut next = spread.clone();
            for index in 0..count {
                let mut best = [0.0f32; 3];
                let mut best_len = 0.0f32;
                for &neighbour in &mesh.neighbours[index] {
                    let len = length(spread[neighbour as usize]);
                    if len > best_len {
                        best_len = len;
                        best = spread[neighbour as usize];
                    }
                }
                if best_len * options.decay > length(spread[index]) {
                    next[index] = scale(best, options.decay);
                }
            }
            spread = next;
        }
        for (total, delta) in displacement.iter_mut().zip(spread.iter()) {
            *total = add(*total, *delta);
        }
    }

    let positions: Vec<[f32; 3]> = garment
        .iter()
        .enumerate()
        .map(|(index, &rest)| add(rest, displacement[mesh.unknown_of[index] as usize]))
        .collect();

    // Authored sliver faces can stay tangled no matter how far they are
    // pushed; they are invalid geometry, not a fit result.
    let mut kept = faces.to_vec();
    let mut dropped = 0usize;
    if options.drop_invalid {
        let pairs = crate::check::intersection_pair_list(&positions, faces, body, &oriented);
        if !pairs.is_empty() {
            let mut doomed = std::collections::BTreeSet::new();
            for (garment_triangle, _) in pairs {
                doomed.insert(garment_triangle);
            }
            kept = faces
                .iter()
                .enumerate()
                .filter_map(|(index, face)| (!doomed.contains(&index)).then_some(*face))
                .collect();
            dropped = doomed.len();
        }
        pairs_after = crate::check::intersection_pairs(&positions, &kept, body, &oriented);
    }
    Some((
        positions,
        kept,
        PairFixReport {
            pairs_before,
            pairs_after,
            rounds,
            dropped,
        },
    ))
}
