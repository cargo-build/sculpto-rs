//! Real-time garment dynamics for annotated glTF primitives.
mod descriptor;
mod physics;
mod skin;
mod surface;
mod worker;

use crate::ReviewModel;
use bevy::ecs::system::SystemParam;
use bevy::gltf::GltfExtras;
use bevy::mesh::morph::{MeshMorphWeights, MorphWeights};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use descriptor::{ClothSpec, Frame};
use physics::{CapsulePose, Solver, DT};
use serde_json::{json, Value};
use skin::SkinSample;
use surface::Surface;
use worker::{Goal, Sample, Worker};

pub(super) struct LiveClothPlugin;
impl Plugin for LiveClothPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClothStatus>().add_systems(
            PostUpdate,
            (discover, attach, simulate)
                .chain()
                .after(bevy::transform::TransformSystems::Propagate),
        );
    }
}
#[derive(Component)]
struct Pending(ClothSpec);
#[derive(Component)]
pub(super) struct LiveSurface;
#[derive(Resource, Default)]
pub(super) struct ClothStatus(pub Vec<Value>);

type NewClothMesh = (With<Mesh3d>, Added<GltfExtras>);

fn discover(mut cmd: Commands, meshes: Query<(Entity, &GltfExtras), NewClothMesh>) {
    for (entity, extras) in &meshes {
        let Ok(data) = serde_json::from_str::<Value>(&extras.value) else {
            continue;
        };
        let Some(cloth) = data.get("sculpto_cloth") else {
            continue;
        };
        match serde_json::from_value::<ClothSpec>(cloth.clone()) {
            Ok(spec) => {
                cmd.entity(entity).insert(Pending(spec));
            }
            Err(err) => warn!("invalid live cloth descriptor on {entity}: {err}"),
        }
    }
}

#[derive(SystemParam)]
struct Nodes<'w, 's> {
    names: Query<'w, 's, (Entity, &'static Name), Without<Mesh3d>>,
    parents: Query<'w, 's, &'static ChildOf>,
    roots: Query<'w, 's, Entity, With<ReviewModel>>,
    globals: Query<'w, 's, &'static GlobalTransform>,
    weights: Query<'w, 's, &'static MorphWeights>,
    sources: Query<
        'w,
        's,
        (
            &'static ChildOf,
            &'static Mesh3d,
            &'static SkinnedMesh,
            Option<&'static MeshMorphWeights>,
        ),
    >,
}
impl Nodes<'_, '_> {
    fn root(&self, e: Entity) -> Option<Entity> {
        self.parents
            .iter_ancestors(e)
            .find(|&p| self.roots.contains(p))
    }
    fn named(&self, root: Entity, name: &str) -> Option<Entity> {
        self.names
            .iter()
            .find(|(e, n)| n.as_str() == name && self.root(*e) == Some(root))
            .map(|(e, _)| e)
    }
}
enum Driver {
    Joint(usize),
    Node(Entity),
}
struct Obstacle {
    driver: Driver,
    a: Vec3,
    b: Vec3,
    radius: f32,
}
impl Obstacle {
    fn pose(&self, joints: &[Mat4], scene: &Nodes) -> Option<CapsulePose> {
        let matrix = match self.driver {
            Driver::Joint(i) => *joints.get(i)?,
            Driver::Node(e) => scene.globals.get(e).ok()?.to_matrix(),
        };
        Some(CapsulePose {
            a: matrix.transform_point3(self.a),
            b: matrix.transform_point3(self.b),
        })
    }
}
struct Rig {
    joints: Vec<(Entity, Mat4)>,
    weights: Option<Entity>,
    samples: Vec<SkinSample>,
    seam: Vec<(usize, SkinSample)>,
    colliders: Vec<Obstacle>,
}
impl Rig {
    fn matrices(&self, nodes: &Nodes) -> Option<Vec<Mat4>> {
        self.joints
            .iter()
            .map(|(e, bind)| Some(nodes.globals.get(*e).ok()?.to_matrix() * *bind))
            .collect()
    }
    fn weights<'a>(&self, nodes: &'a Nodes) -> &'a [f32] {
        self.weights
            .and_then(|e| nodes.weights.get(e).ok())
            .map_or(&[], MorphWeights::weights)
    }
}
#[derive(Component)]
struct LiveCloth {
    worker: Worker,
    last: Option<Sample>,
    surface: Surface,
    rig: Rig,
    spec: ClothSpec,
    handle: Handle<Mesh>,
    pins: Vec<Vec3>,
    positions: Vec<Vec3>,
    reset_distance: f32,
}

#[allow(clippy::too_many_arguments)]
fn bind(
    entity: Entity,
    owner: &GlobalTransform,
    render: &Mesh,
    spec: &ClothSpec,
    nodes: &Nodes,
    meshes: &Assets<Mesh>,
    inverse: &Assets<SkinnedMeshInverseBindposes>,
) -> Result<Option<(LiveCloth, Mesh)>, String> {
    let Some(root) = nodes.root(entity) else {
        return Ok(None);
    };
    let Some(source) = nodes.named(root, &spec.source_node) else {
        return Ok(None);
    };
    let Some((_, handle, skin, morph)) = nodes
        .sources
        .iter()
        .find(|(parent, _, _, _)| parent.parent() == source)
    else {
        return Ok(None);
    };
    let (Some(mesh), Some(bindposes)) = (meshes.get(handle), inverse.get(&skin.inverse_bindposes))
    else {
        return Ok(None);
    };
    spec.validate(render.count_vertices())?;
    if skin.joints.len() != bindposes.len() {
        return Err("cloth inverse bind count does not match the source rig".into());
    }
    let n = skin.joints.len();
    let read = |&i: &usize| SkinSample::read(mesh, i, n);
    let samples = spec
        .particles
        .iter()
        .map(read)
        .collect::<Result<Vec<_>, _>>()?;
    let seam = spec
        .seam
        .iter()
        .map(|&[i, src]| Ok((i, read(&src)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let mut colliders = Vec::new();
    for c in &spec.colliders {
        let driver = match &c.frame {
            Frame::Joint(i) if *i < n => Driver::Joint(*i),
            Frame::Joint(_) => return Err("cloth collider joint is outside the source rig".into()),
            Frame::Node(name) => {
                let Some(e) = nodes.named(root, name) else {
                    return Ok(None);
                };
                Driver::Node(e)
            }
        };
        colliders.push(Obstacle {
            driver,
            a: Vec3::from_array(c.a),
            b: Vec3::from_array(c.b),
            radius: c.radius,
        });
    }
    let weights = match morph {
        Some(MeshMorphWeights::Reference(e)) => Some(*e),
        None => None,
        Some(MeshMorphWeights::Value { .. }) => {
            return Err("expected glTF node morph weights".into())
        }
    };
    let rig = Rig {
        joints: skin
            .joints
            .iter()
            .copied()
            .zip(bindposes.iter().copied())
            .collect(),
        weights,
        samples,
        seam,
        colliders,
    };
    let Some(joints) = rig.matrices(nodes) else {
        return Ok(None);
    };
    let weights = rig.weights(nodes);
    let initial: Vec<_> = rig
        .samples
        .iter()
        .map(|s| s.world(&joints, weights))
        .collect();
    let rest: Vec<_> = rig
        .samples
        .iter()
        .map(|s| owner.transform_point(s.rest))
        .collect();
    let Some(caps) = rig
        .colliders
        .iter()
        .map(|c| c.pose(&joints, nodes))
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(None);
    };
    let scale = owner.compute_transform().scale.abs().max_element();
    let obstacles: Vec<_> = caps
        .iter()
        .zip(&rig.colliders)
        .map(|(&p, c)| (p, c.radius * scale))
        .collect();
    let mut material = spec.material;
    material.thickness *= scale;
    let solver = Solver::new(
        &rest,
        &spec.triangles,
        &spec.pinned,
        material,
        &obstacles,
        &initial,
    )?;
    let pins: Vec<_> = spec.pinned.iter().map(|&i| initial[i as usize]).collect();
    let local_rest: Vec<_> = rig.samples.iter().map(|s| s.rest).collect();
    let surface = Surface::new(render, &local_rest, spec)?;
    let low = rest
        .iter()
        .copied()
        .fold(Vec3::splat(f32::INFINITY), Vec3::min);
    let high = rest
        .iter()
        .copied()
        .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
    let worker = Worker::start(
        solver,
        Goal {
            pins: pins.clone(),
            caps,
            reset_to: None,
        },
        spec.pinned.clone(),
    )?;
    Ok(Some((
        LiveCloth {
            worker,
            last: None,
            surface,
            rig,
            spec: spec.clone(),
            handle: Handle::default(),
            pins,
            positions: initial,
            reset_distance: low.distance(high) * 0.45,
        },
        render.clone(),
    )))
}

fn attach(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    inverse: Res<Assets<SkinnedMeshInverseBindposes>>,
    nodes: Nodes,
    pending: Query<(Entity, &GlobalTransform, &Mesh3d, &Pending)>,
) {
    for (entity, transform, handle, desc) in &pending {
        let Some(render) = meshes.get(&handle.0) else {
            continue;
        };
        match bind(
            entity, transform, render, &desc.0, &nodes, &meshes, &inverse,
        ) {
            Ok(Some((mut cloth, mesh))) => {
                let live = meshes.add(mesh);
                cloth.handle = live.clone();
                info!(
                    "viewer: Rapier live cloth attached: {} particles, {} pins, {} colliders",
                    cloth.positions.len(),
                    cloth.pins.len(),
                    cloth.rig.colliders.len()
                );
                commands.entity(entity).remove::<Pending>().insert((
                    Mesh3d(live),
                    LiveSurface,
                    cloth,
                    bevy::camera::visibility::NoFrustumCulling,
                ));
            }
            Ok(None) => {}
            Err(err) => {
                warn!("could not attach live cloth: {err}");
                commands.entity(entity).remove::<Pending>();
            }
        }
    }
}

fn simulate(
    nodes: Nodes,
    mut instances: Query<(&GlobalTransform, &mut LiveCloth)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut status: ResMut<ClothStatus>,
) {
    status.0.clear();
    for (transform, mut cloth) in &mut instances {
        let cloth = &mut *cloth;
        let Some(joints) = cloth.rig.matrices(&nodes) else {
            continue;
        };
        let w = cloth.rig.weights(&nodes);
        let pins: Vec<_> = cloth
            .spec
            .pinned
            .iter()
            .map(|&i| cloth.rig.samples[i as usize].world(&joints, w))
            .collect();
        let Some(caps) = cloth
            .rig
            .colliders
            .iter()
            .map(|c| c.pose(&joints, &nodes))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let discontinuity = cloth
            .pins
            .iter()
            .zip(&pins)
            .any(|(a, b)| a.distance(*b) > cloth.reset_distance);
        let reset_to = discontinuity.then(|| {
            cloth
                .rig
                .samples
                .iter()
                .map(|s| s.world(&joints, w))
                .collect()
        });
        cloth.worker.submit(Goal {
            pins: pins.clone(),
            caps,
            reset_to,
        });
        if let Some(sample) = cloth.worker.poll() {
            cloth.last = Some(sample);
        }
        let Some(last) = &cloth.last else {
            continue;
        };
        let t = (last.finished.elapsed().as_secs_f32() / DT).min(1.);
        let mut motion = 0f32;
        for ((p, a), b) in cloth
            .positions
            .iter_mut()
            .zip(&last.previous)
            .zip(&last.positions)
        {
            let current = a.lerp(*b, t);
            motion = motion.max(p.distance(current));
            *p = current;
        }
        for (&idx, &p) in cloth.spec.pinned.iter().zip(&pins) {
            cloth.positions[idx as usize] = p;
        }
        let inv = transform.affine().inverse();
        let local: Vec<_> = cloth
            .positions
            .iter()
            .map(|&p| inv.transform_point3(p))
            .collect();
        let seam = cloth
            .rig
            .seam
            .iter()
            .map(|(i, s)| (*i, inv.transform_point3(s.world(&joints, w))));
        let Some(mut mesh) = meshes.get_mut(&cloth.handle) else {
            continue;
        };
        let lit_vertices = cloth.surface.update(&mut mesh, &local, &cloth.spec, seam);
        status.0.push(json!({"solver":"Rapier SoftBody","fixed_hz":60,"steps":last.steps,
             "particles":cloth.positions.len(),"pins":pins.len(),"capsules":cloth.rig.colliders.len(),
             "contacts":last.contacts,"max_pin_error_m":last.pin_error,"max_particle_motion_m":motion,
             "render_vertices":mesh.count_vertices(),"unit_normals":lit_vertices,
             "step_ms":last.step_ms,"witness":cloth.positions[0].to_array(),"pin":pins[0].to_array()}));
        cloth.pins = pins;
    }
}
