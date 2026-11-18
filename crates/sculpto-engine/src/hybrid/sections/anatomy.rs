//! Use A-pose branch identity for limb/digit membership and joint planes for segment boundaries.
use super::super::{SkinAsset, fail};
use crate::CommandError;
use glam::DVec3;

pub(super) struct Part {
    pub name: String,
    pub slot: usize,
    pub root: DVec3,
    pub tip: DVec3,
    pub depth: f64,
}
impl Part {
    fn dir(&self) -> DVec3 {
        (self.tip - self.root).normalize()
    }
    fn axis_distance(&self, p: DVec3) -> f64 {
        let v = self.tip - self.root;
        let t = ((p - self.root).dot(v) / v.length_squared()).clamp(0., 1.);
        (p - self.root - t * v).length()
    }
}
struct Side {
    arm: usize,
    fore: usize,
    hand: usize,
    shin: usize,
    ankle: DVec3,
    upperplane: DVec3,
    up: DVec3,
    medial_reach: f64,
    kneeplane: DVec3,
    ankleplane: DVec3,
    wristplane: DVec3,
    digits: Vec<[usize; 3]>,
    prefix: String,
}
pub(super) struct Layout {
    pub parts: Vec<Part>,
    sides: Vec<Side>,
    pub ankles: Vec<(usize, DVec3, f64)>,
}
impl Layout {
    pub fn new(a: &SkinAsset, body: &[[u32; 3]]) -> Result<Self, CommandError> {
        let mut parts = Vec::new();
        let mut sides = Vec::new();
        let mut ankles = Vec::new();
        for side in ["Left", "Right"] {
            let world = |suffix: &str| {
                a.mix_slot(&format!("{side}{suffix}"))
                    .map(|i| a.world(i).w_axis.truncate())
            };
            let mut add = |tag: &str, child: &str| -> Result<usize, CommandError> {
                let name = format!("{side}{tag}");
                let slot = a.mix_slot(&name)?;
                let root = world(tag)?;
                let tip = world(child)?;
                if root.distance(tip) < 1e-6 {
                    return Err(fail(format!("degenerate material axis at {name}")));
                }
                let i = parts.len();
                parts.push(Part {
                    name,
                    slot,
                    root,
                    tip,
                    depth: 0.,
                });
                Ok(i)
            };
            let arm = add("Arm", "ForeArm")?;
            let fore = add("ForeArm", "Hand")?;
            let hand = add("Hand", "HandMiddle1")?;
            let shin = add("Leg", "Foot")?;
            let mut digits = Vec::new();
            for digit in ["Thumb", "Index", "Middle", "Ring", "Pinky"] {
                let mut chain = [0; 3];
                for (k, part) in chain.iter_mut().enumerate() {
                    *part = add(
                        &format!("Hand{digit}{}", k + 1),
                        &format!("Hand{digit}{}", k + 2),
                    )?;
                }
                digits.push(chain);
            }
            let ankle = world("Foot")?;
            let kneeplane =
                ((world("Leg")? - world("UpLeg")?).normalize() + parts[shin].dir()).normalize();
            // The ankle cut crosses the shin axis at the malleoli, above the heel and instep.
            let ankleplane = parts[shin].dir();
            let wristplane = (parts[fore].dir() + parts[hand].dir()).normalize();
            let clavicle = world("Arm")? - world("Shoulder")?;
            let upperplane = clavicle.normalize();
            let up = (a.world(a.mix_slot("Neck")?).w_axis.truncate()
                - a.world(a.mix_slot("Hips")?).w_axis.truncate())
            .normalize();
            sides.push(Side {
                arm,
                fore,
                hand,
                shin,
                ankle,
                upperplane,
                up,
                medial_reach: 0.5 * clavicle.length(),
                kneeplane,
                ankleplane,
                wristplane,
                digits,
                prefix: side.into(),
            });
            ankles.push((a.mix_slot(&format!("{side}Foot"))?, ankle, 0.));
        }
        let rest: Vec<_> = a
            .pool
            .vertices
            .iter()
            .map(|p| p.map(|x| x as f32))
            .collect();
        let centres: Vec<_> = parts
            .iter()
            .map(|s| s.root.as_vec3().to_array())
            .chain(ankles.iter().map(|(_, p, _)| p.as_vec3().to_array()))
            .collect();
        let depths = sculpto_mesh::check::nearest_distances(&centres, &rest, body);
        for (part, d) in parts.iter_mut().zip(&depths) {
            part.depth = f64::from(*d);
        }
        for (ankle, d) in ankles.iter_mut().zip(&depths[parts.len()..]) {
            ankle.2 = f64::from(*d);
        }
        if depths.iter().any(|v| !v.is_finite() || *v <= 1e-5) {
            return Err(fail(
                "joint clearance cannot support an enclosed articulation",
            ));
        }
        Ok(Self {
            parts,
            sides,
            ankles,
        })
    }

    pub fn fields(&self, a: &SkinAsset) -> (Vec<Option<usize>>, Vec<f64>) {
        let mut owners = Vec::new();
        let mut widths = Vec::new();
        let stations: Vec<_> = self
            .parts
            .iter()
            .map(|p| (p.root, (0.54 * p.depth).min(0.18 * p.root.distance(p.tip))))
            .chain(self.ankles.iter().map(|(_, p, d)| (*p, 0.54 * d)))
            .collect();
        for (&point, row) in a.pool.vertices.iter().zip(&a.pool.rows) {
            let p = DVec3::from_array(point);
            let dom = row
                .iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .and_then(|v| a.names[usize::from(v.0)].rsplit(':').next());
            let owner = dom.and_then(|n| self.sides.iter().find_map(|s| self.owner(s, p, n, row)));
            let nearest = stations
                .iter()
                .min_by(|(a, _), (b, _)| a.distance_squared(p).total_cmp(&b.distance_squared(p)))
                .expect("limb stations");
            owners.push(owner);
            widths.push(nearest.1);
        }
        (owners, widths)
    }

    fn owner(&self, s: &Side, p: DVec3, dominant: &str, row: &[(u16, f32)]) -> Option<usize> {
        let short = dominant.strip_prefix(&s.prefix);
        let (arm, fore, palm, shin) = (
            &self.parts[s.arm],
            &self.parts[s.fore],
            &self.parts[s.hand],
            &self.parts[s.shin],
        );
        if short.is_some_and(|n| matches!(n, "UpLeg" | "Leg" | "Foot" | "ToeBase" | "Toe_End")) {
            return (((p - shin.root).dot(s.kneeplane) >= 0.)
                && ((p - s.ankle).dot(s.ankleplane) < 0.))
                .then_some(s.shin);
        }
        let arm_family =
            short.is_some_and(|n| n == "Arm" || n == "ForeArm" || n.starts_with("Hand"));
        if p.distance(arm.root) < 0.5 * arm.root.distance(arm.tip) {
            let support = row
                .iter()
                .any(|&(slot, w)| usize::from(slot) == arm.slot && w > 0.03);
            let offset = p - arm.root;
            let h = offset.dot(s.up) / arm.depth;
            let blend = ((h + 0.65) / 1.15).clamp(0., 1.);
            let superior = blend * blend * (3. - 2. * blend);
            let lateral = offset.dot(s.upperplane);
            let cap = h > -0.4
                && offset.dot(s.upperplane.cross(s.up).normalize()).abs() < 1.55 * arm.depth;
            if (!arm_family && !support && short != Some("Shoulder") && !cap)
                || lateral < -s.medial_reach * superior
            {
                return None;
            }
        } else if !arm_family {
            return None;
        }
        if (p - fore.root).dot((arm.dir() + fore.dir()).normalize()) < 0. {
            return Some(s.arm);
        }
        if (p - palm.root).dot(s.wristplane) < 0. {
            return Some(s.fore);
        }
        let mut picked = None;
        let mut best = f64::INFINITY;
        for chain in &s.digits {
            let [one, two, three] = chain.map(|i| &self.parts[i]);
            if (p - one.root).dot(one.dir()) < 0. {
                continue;
            }
            let belongs = [one, two, three].iter().any(|g| g.name == dominant);
            let dist = one
                .axis_distance(p)
                .min(two.axis_distance(p))
                .min(three.axis_distance(p));
            let radius = (one.depth + two.depth + three.depth) / 3.;
            if !belongs && dist > 1.55 * radius {
                continue;
            }
            let score = if belongs { -1. } else { dist / radius };
            if score < best {
                best = score;
                picked = Some(
                    if (p - two.root).dot((one.dir() + two.dir()).normalize()) < 0. {
                        chain[0]
                    } else if (p - three.root).dot((two.dir() + three.dir()).normalize()) < 0. {
                        chain[1]
                    } else {
                        chain[2]
                    },
                );
            }
        }
        picked.or(Some(s.hand))
    }
}
