//! Shared safety helpers for ECS hierarchy traversals.

use super::components::Children;
use super::storage::EntityId;
use super::World;

/// Bounds one hierarchy walk using the current entity population and child
/// references.
///
/// A valid parent/children tree visits at most one entry per live entity. The
/// child-reference term leaves room for duplicated edges while still making a
/// malformed cyclic graph finite. Callers additionally use a visited set so a
/// cycle is rejected before it can multiply the pending work.
#[derive(Debug)]
pub struct HierarchyTraversalGuard {
    remaining: usize,
}

impl HierarchyTraversalGuard {
    /// Creates a guard for a walk over the current hierarchy snapshot.
    pub fn new(entity_count: usize, child_reference_count: usize) -> Self {
        Self {
            remaining: entity_count
                .saturating_add(child_reference_count)
                .saturating_add(1),
        }
    }

    /// Consumes one traversal step, returning `false` after the finite budget
    /// is exhausted.
    pub fn step(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        true
    }
}

/// Collect a root's whole `Children` subtree — root included — with the
/// project's hierarchy-walk discipline (#3700 / #4572 / #4769 / #5513):
/// a visited set plus a [`HierarchyTraversalGuard`] budget, so a cyclic
/// or duplicated-edge `Children` graph yields a finite, de-duplicated
/// list instead of hanging the calling system with no diagnostic. A
/// child listed under two parents appears once.
///
/// This is the one shared collector every whole-subtree walk routes
/// through (cinematic adoption, loading-cover setup/retirement, cell
/// teardown retention, gear release); order is unspecified (DFS).
pub fn collect_subtree(world: &World, root: EntityId) -> Vec<EntityId> {
    let mut seen = std::collections::HashSet::new();
    let mut guard = HierarchyTraversalGuard::new(world.next_entity_id() as usize, 0);
    let mut stack = vec![root];
    let mut out = Vec::new();
    while let Some(entity) = stack.pop() {
        if !seen.insert(entity) {
            continue;
        }
        if !guard.step() {
            log::error!(
                "subtree collection from entity {root} aborted after exhausting \
                 the hierarchy traversal budget at entity {entity} — cyclic or \
                 corrupt Children state"
            );
            break;
        }
        out.push(entity);
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.0.iter().copied());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #5513 — a cyclic `Children` graph must terminate and return each
    /// entity once. The hand-rolled walks this collector replaced pushed
    /// every child id onto the stack unconditionally, so the same graph
    /// hung the streaming adoption / loading-cover steps.
    #[test]
    fn collect_subtree_terminates_on_a_cycle() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        let c = world.spawn();
        world.insert(a, Children(vec![b]));
        world.insert(b, Children(vec![c, a])); // back-edge to the root
        world.insert(c, Children(vec![b])); // second back-edge
        let mut subtree = collect_subtree(&world, a);
        subtree.sort_unstable();
        assert_eq!(subtree, vec![a, b, c], "each cycle member appears once");
    }

    /// A child listed under two parents is visited once (the duplicated
    /// `Children` edge case the guard budget tolerates).
    #[test]
    fn collect_subtree_dedupes_shared_children() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        let shared = world.spawn();
        world.insert(a, Children(vec![shared]));
        world.insert(b, Children(vec![shared]));
        let mut subtree = collect_subtree(&world, a);
        subtree.sort_unstable();
        assert_eq!(subtree, vec![a, shared]);
    }

    /// A leaf root (no `Children` at all) collects to just itself.
    #[test]
    fn collect_subtree_root_only() {
        let mut world = World::new();
        let a = world.spawn();
        assert_eq!(collect_subtree(&world, a), vec![a]);
    }
}
