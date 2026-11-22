use super::descriptor::Material;
use bevy::prelude::{Quat, Vec3};
use rapier3d::prelude as rp;

pub(super) const DT: f32 = 1.0 / 60.0;

#[derive(Clone, Copy)]
pub(super) struct CapsulePose {
    pub a: Vec3,
    pub b: Vec3,
}
impl CapsulePose {
    fn pose(self) -> rp::Pose {
        let d = self.b - self.a;
        let rotation = if d.length_squared() > 1e-12 {
            Quat::from_rotation_arc(Vec3::Y, d.normalize())
        } else {
            Quat::IDENTITY
        };
        rp::Pose::from_parts(
            vector((self.a + self.b) * 0.5),
            rp::Rotation::from_array(rotation.to_array()),
        )
    }
}
fn vector(v: Vec3) -> rp::Vector {
    rp::Vector::from_array(v.to_array())
}

pub(super) struct Solver {
    world: rp::PhysicsWorld,
    cloth: rp::SoftBodyHandle,
    obstacles: Vec<MovingCapsule>,
    pins: Vec<u32>,
    pub steps: u64,
}

struct MovingCapsule {
    body: rp::RigidBodyHandle,
    collider: rp::ColliderHandle,
    half_height: f32,
    radius: f32,
}

fn capsule_shape(half_height: f32, radius: f32) -> rp::SharedShape {
    if half_height < 1e-6 {
        rp::SharedShape::ball(radius)
    } else {
        rp::SharedShape::capsule_y(half_height, radius)
    }
}

impl Solver {
    pub fn new(
        rest: &[Vec3],
        triangles: &[[u32; 3]],
        pins: &[u32],
        material: Material,
        obstacles: &[(CapsulePose, f32)],
        initial: &[Vec3],
    ) -> Result<Self, String> {
        let mut world = rp::PhysicsWorld::new();
        world.gravity = vector(Vec3::NEG_Y * 9.81);
        world.integration_parameters.dt = DT;
        let edge = rp::SpringCoefficients::new(material.stretch_hz, material.damping_ratio);
        let bend = rp::SpringCoefficients::new(material.bend_hz, material.damping_ratio);
        let fabric = rp::SoftBodyMaterial {
            bend_softness: bend,
            ..rp::SoftBodyMaterial::uniform(edge)
        };
        let builder = rp::SoftBodyBuilder::trimesh(
            rest.iter().copied().map(vector).collect(),
            triangles.to_vec(),
        )
        .ok_or("Rapier rejected the cloth triangles")?
        .material(fabric)
        .pinned_particles(pins.iter().copied())
        .linear_damping(material.air_damping)
        .particle_mass(material.particle_mass)
        .particle_radius(material.thickness)
        .shape_matching(false)
        .self_contacts(true)
        .surface_collider(rp::ColliderBuilder::ball(material.thickness).friction(0.65))
        .can_sleep(false);
        let cloth = world.insert_soft_body(builder);
        let mut bodies = Vec::with_capacity(obstacles.len());
        for &(shape, radius) in obstacles {
            let h = (shape.b - shape.a).length() * 0.5;
            let (body, collider) = world.insert(
                rp::RigidBodyBuilder::kinematic_position_based().pose(shape.pose()),
                rp::ColliderBuilder::new(capsule_shape(h, radius)).friction(0.65),
            );
            bodies.push(MovingCapsule {
                body,
                collider,
                half_height: h,
                radius,
            });
        }
        let mut result = Self {
            world,
            cloth,
            obstacles: bodies,
            pins: pins.to_vec(),
            steps: 0,
        };
        result.place(initial);
        Ok(result)
    }
    pub fn place(&mut self, points: &[Vec3]) {
        let cloth = &mut self.world.soft_bodies[self.cloth];
        for (i, &point) in points.iter().enumerate() {
            cloth.set_particle_position(i, vector(point));
            cloth.set_particle_velocity(i, rp::Vector::ZERO);
        }
    }
    pub fn step(&mut self, targets: &[Vec3], capsules: &[CapsulePose]) {
        for (&idx, &point) in self.pins.iter().zip(targets) {
            self.world.soft_bodies[self.cloth]
                .set_particle_kinematic_target(idx as usize, vector(point));
        }
        for (obstacle, &pose) in self.obstacles.iter_mut().zip(capsules) {
            let half_height = pose.a.distance(pose.b) * 0.5;
            // Length scales on animated props change even when their collision radius stays fixed.
            if (obstacle.half_height - half_height).abs() > obstacle.radius * 0.001 {
                self.world.colliders[obstacle.collider]
                    .set_shape(capsule_shape(half_height, obstacle.radius));
                obstacle.half_height = half_height;
            }
            self.world.bodies[obstacle.body].set_next_kinematic_position(pose.pose());
        }
        self.world.step();
        self.steps += 1;
    }
    pub fn positions(&self) -> Vec<Vec3> {
        self.world.soft_bodies[self.cloth]
            .particle_positions()
            .map(|p| Vec3::from_array(p.to_array()))
            .collect()
    }
    pub fn contacts(&self) -> usize {
        self.world
            .narrow_phase
            .contact_pairs()
            .filter(|p| p.has_any_active_contact())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn free_hem_settles_under_gravity_with_stationary_shoulders() {
        let rest = [
            Vec3::ZERO,
            Vec3::X * 0.12,
            Vec3::new(0., 0., 0.1),
            Vec3::new(0.12, 0., 0.1),
        ];
        let m = Material {
            stretch_hz: 75.,
            bend_hz: 5.,
            damping_ratio: 1.,
            air_damping: 2.,
            particle_mass: 0.003,
            thickness: 0.003,
        };
        let obstacle = CapsulePose {
            a: Vec3::X,
            b: Vec3::X,
        };
        let extended = CapsulePose {
            a: Vec3::X,
            b: Vec3::X + 0.5 * Vec3::Y,
        };
        let mut sim = Solver::new(
            &rest,
            &[[0, 2, 1], [1, 2, 3]],
            &[0, 1],
            m,
            &[(obstacle, 0.01)],
            &rest,
        )
        .unwrap();
        for _ in 0..18 {
            sim.step(&rest[..2], &[extended]);
        }
        let points = sim.positions();
        assert_eq!(sim.steps, 18);
        assert!(points[0].distance(rest[0]) < 1e-4 && points[1].distance(rest[1]) < 1e-4);
        assert!(points[2].y < -0.025 && points[3].is_finite());
        let actual = sim.world.colliders[sim.obstacles[0].collider]
            .shape()
            .as_capsule()
            .unwrap();
        assert!((actual.segment.a.distance(actual.segment.b) - 0.5).abs() < 1e-5);
    }
}
