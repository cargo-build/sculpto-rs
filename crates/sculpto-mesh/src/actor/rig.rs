//! Reduced skin correspondence for explicitly requested joint nodes. @req DECL0013

use std::collections::{HashMap, HashSet};

use glam::DMat4;

use super::{
    Actor, ActorError, Influence, JointCandidate, JointDisambiguation, Pose, RequestedJoint,
    RigNode, RigRole, RigSample, Sample, animation, geometry, invalid, unsupported,
};

/// A requested joint name resolved to its unique graph node.
pub(super) struct Resolved {
    pub(super) name: String,
    pub(super) node: usize,
}

/// Every node the selected parts' skins reference: the set that may break a
/// duplicate-name tie. An export that ships a duplicate rig copy alongside
/// the animation armature duplicates names outside this set only.
fn skin_electorate(actor: &Actor, selected: &[usize]) -> HashSet<usize> {
    selected
        .iter()
        .filter_map(|&node| {
            actor
                .document
                .nodes()
                .nth(node)
                .and_then(|node| node.skin())
        })
        .flat_map(|skin| skin.joints().map(|joint| joint.index()))
        .collect()
}

/// Sample the parts, then describe skin correspondence in a reduced rig.
/// @req DECL0013
pub(super) fn sample(
    actor: &Actor,
    parts: &[&str],
    pose: Pose<'_>,
    joints: &[&str],
) -> Result<RigSample, ActorError> {
    let selected = geometry::select(&actor.document, parts)?;
    let electorate = skin_electorate(actor, &selected);
    let (requested, disambiguated) = resolve_with(actor, joints, &electorate)?;
    let (worlds, morphs) = animation::evaluated(actor, &selected, pose)?;
    let sample = geometry::sample_with_morphs(actor, &selected, &worlds, Some(&morphs))?;
    let (rig, index) = graph(actor, &requested, &worlds)?;
    let requested = requested
        .into_iter()
        .map(|joint| RequestedJoint {
            rig: index[&joint.node],
            node: joint.node,
            name: joint.name,
        })
        .collect::<Vec<_>>();
    let influences = influence_rows(actor, &sample, &index)?;
    Ok(RigSample {
        sample,
        rig,
        requested,
        influences,
        disambiguated,
    })
}

/// Resolve joint names against every skin the document carries: the
/// electorate for verbs that drive joints without sampling parts. The
/// resolved records only; the tie-break notes stay with [`resolve_with`].
pub(super) fn resolve(actor: &Actor, joints: &[&str]) -> Result<Vec<Resolved>, ActorError> {
    let skin_joints: HashSet<usize> = actor
        .document
        .skins()
        .flat_map(|skin| skin.joints().map(|joint| joint.index()))
        .collect();
    let (resolved, _) = resolve_with(actor, joints, &skin_joints)?;
    Ok(resolved)
}

/// Requested joints are exact node names. The name is the label; the node
/// index is the address. When several nodes carry one name, the skin breaks
/// the tie: exactly one skin-referenced match wins and the choice is
/// reported, every other shape stays a refusal that names the candidates.
/// @req DECL0013
pub(super) fn resolve_with(
    actor: &Actor,
    joints: &[&str],
    skin_joints: &HashSet<usize>,
) -> Result<(Vec<Resolved>, Vec<JointDisambiguation>), ActorError> {
    if joints.is_empty() {
        return Err(invalid(
            "actor rig selection",
            "at least one explicit joint is required",
        ));
    }
    let mut seen = HashSet::new();
    let mut disambiguated = Vec::new();
    let resolved = joints
        .iter()
        .map(|&name| {
            match seen.insert(name) {
                true => {}
                false => {
                    return Err(ActorError::DuplicateJoint {
                        name: name.to_owned(),
                    });
                }
            }
            let matches: Vec<_> = actor
                .document
                .nodes()
                .filter(|node| node.name() == Some(name))
                .map(|node| node.index())
                .collect();
            match matches.as_slice() {
                [node] => Ok(Resolved {
                    name: name.to_owned(),
                    node: *node,
                }),
                _ => {
                    let candidates: Vec<JointCandidate> = matches
                        .iter()
                        .map(|&node| JointCandidate {
                            node,
                            is_skin_joint: skin_joints.contains(&node),
                            parent_depth: chain(actor, node).len().saturating_sub(1),
                        })
                        .collect();
                    let mut skin = candidates
                        .iter()
                        .filter(|candidate| candidate.is_skin_joint);
                    match (skin.next(), skin.next()) {
                        (Some(chosen), None) => {
                            disambiguated.push(JointDisambiguation {
                                name: name.to_owned(),
                                chosen: chosen.clone(),
                                refused: candidates
                                    .iter()
                                    .filter(|candidate| !candidate.is_skin_joint)
                                    .cloned()
                                    .collect(),
                            });
                            Ok(Resolved {
                                name: name.to_owned(),
                                node: chosen.node,
                            })
                        }
                        // Two skin-referenced nodes with one name is a
                        // genuinely ambiguous file; no skin reference at all
                        // leaves the lookup without a tie-break.
                        _ => Err(ActorError::JointSelection {
                            name: name.to_owned(),
                            matches: matches.len(),
                            candidates,
                        }),
                    }
                }
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((resolved, disambiguated))
}

/// Retain every node on each requested path to the shared lowest ancestor.
fn retain(actor: &Actor, requested: &[Resolved]) -> Result<(Vec<usize>, usize), ActorError> {
    let paths: Vec<Vec<usize>> = requested
        .iter()
        .map(|joint| chain(actor, joint.node))
        .collect();
    // Paths run upward from the joint itself, so the first element shared by
    // every path is the deepest common ancestor; nothing above it is retained.
    let root = paths[0]
        .iter()
        .copied()
        .find(|node| paths.iter().all(|path| path.contains(node)))
        .ok_or(ActorError::DisconnectedRig)?;
    let mut retained = HashSet::new();
    for path in &paths {
        for node in path {
            retained.insert(*node);
            if *node == root {
                break;
            }
        }
    }
    let rank: HashMap<_, _> = actor
        .order
        .iter()
        .enumerate()
        .map(|(position, &node)| (node, position))
        .collect();
    let mut ordered: Vec<_> = retained.into_iter().collect();
    ordered.sort_by_key(|node| rank[node]);
    Ok((ordered, root))
}

/// The real parent chain from a node to its graph root, both inclusive.
fn chain(actor: &Actor, node: usize) -> Vec<usize> {
    let mut path = vec![node];
    let mut cursor = node;
    while let Some(parent) = actor.parents[cursor] {
        path.push(parent);
        cursor = parent;
    }
    path
}

/// Connect retained nodes parent-before-child with original identities.
fn graph(
    actor: &Actor,
    requested: &[Resolved],
    worlds: &[DMat4],
) -> Result<(Vec<RigNode>, HashMap<usize, usize>), ActorError> {
    let (ordered, root) = retain(actor, requested)?;
    let index: HashMap<_, _> = ordered
        .iter()
        .enumerate()
        .map(|(position, &node)| (node, position))
        .collect();
    let requested: HashSet<_> = requested.iter().map(|joint| joint.node).collect();
    let rig = ordered
        .into_iter()
        .map(|node| {
            let parent = match actor.parents[node] {
                None => None,
                Some(parent) => match index.get(&parent) {
                    Some(&position) => Some(position),
                    None => match node == root {
                        true => None,
                        false => {
                            return Err(invalid(
                                format!("node {node}"),
                                "retained joint path lost its parent",
                            ));
                        }
                    },
                },
            };
            let name = actor
                .document
                .nodes()
                .nth(node)
                .and_then(|resolved| resolved.name())
                .map(str::to_owned);
            Ok(RigNode {
                role: match requested.contains(&node) {
                    true => RigRole::Requested,
                    false => RigRole::Connector,
                },
                node,
                name,
                parent,
                world: worlds[node].to_cols_array_2d(),
            })
        })
        .collect::<Result<_, _>>()?;
    Ok((rig, index))
}

/// Collapse every source influence onto its nearest retained ancestor.
fn influence_rows(
    actor: &Actor,
    sample: &Sample,
    index: &HashMap<usize, usize>,
) -> Result<Vec<Vec<Influence>>, ActorError> {
    let mut rows = Vec::with_capacity(sample.vertices.len());
    sample.parts.iter().try_for_each(|part| {
        let context = format!(
            "part {:?} node {} mesh {} primitive {}",
            part.name, part.node, part.mesh, part.primitive
        );
        let node = actor
            .document
            .nodes()
            .nth(part.node)
            .ok_or_else(|| invalid(format!("node {}", part.node), "missing selected node"))?;
        let skin = node
            .skin()
            .ok_or_else(|| unsupported(&context, "unskinned mesh"))?;
        let joints: Vec<_> = skin.joints().map(|joint| joint.index()).collect();
        let primitive = node
            .mesh()
            .and_then(|mesh| mesh.primitives().nth(part.primitive))
            .ok_or_else(|| invalid(&context, "missing selected primitive"))?;
        let skinning = geometry::skin_rows(
            actor,
            &primitive,
            joints.len(),
            part.vertices.len(),
            &context,
        )?;
        skinning
            .into_iter()
            .enumerate()
            .try_for_each(|(vertex, row)| {
                let location = format!("{context} vertex {vertex}");
                let mut bindings: Vec<Influence> = Vec::new();
                row.joints
                    .into_iter()
                    .zip(row.weights)
                    .try_for_each(|(joint, weight)| {
                        if weight <= 0.0 {
                            return Ok(());
                        }
                        let mut cursor = joints[usize::from(joint)];
                        let rig = loop {
                            match index.get(&cursor) {
                                Some(&rig) => break rig,
                                None => match actor.parents[cursor] {
                                    Some(parent) => cursor = parent,
                                    None => {
                                        return Err(invalid(
                                            &location,
                                            format!("joint node {cursor} has no retained ancestor"),
                                        ));
                                    }
                                },
                            }
                        };
                        match bindings.iter_mut().find(|binding| binding.rig == rig) {
                            Some(binding) => binding.weight += weight,
                            None => bindings.push(Influence { rig, weight }),
                        }
                        Ok(())
                    })?;
                bindings.sort_by_key(|binding| binding.rig);
                rows.push(bindings);
                Ok(())
            })
    })?;
    Ok(rows)
}
