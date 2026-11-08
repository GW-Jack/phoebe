//! Compact immediate-dominator tree for verified reachable Core CFGs.

/// Compute immediate dominators in reverse postorder, then number the
/// dominator tree so each dominance query is a constant-time interval test.
pub(super) struct Dominators {
    entry: Vec<usize>,
    exit: Vec<usize>,
}

impl Dominators {
    pub(super) fn new(successors: &[Vec<usize>], predecessors: &[Vec<usize>]) -> Self {
        let mut seen = vec![false; successors.len()];
        let mut postorder = Vec::with_capacity(successors.len());
        let mut stack = vec![(0, 0)];
        seen[0] = true;
        while let Some((node, next)) = stack.last_mut() {
            if let Some(&target) = successors[*node].get(*next) {
                *next += 1;
                if !seen[target] {
                    seen[target] = true;
                    stack.push((target, 0));
                }
            } else {
                postorder.push(*node);
                stack.pop();
            }
        }
        postorder.reverse();
        let mut order = vec![0; successors.len()];
        for (index, &node) in postorder.iter().enumerate() {
            order[node] = index;
        }
        let mut idom = vec![None; successors.len()];
        idom[0] = Some(0);
        let intersect = |mut left: usize, mut right: usize, idom: &[Option<usize>]| {
            while left != right {
                while order[left] > order[right] {
                    left = idom[left].expect("visited predecessor has a dominator");
                }
                while order[right] > order[left] {
                    right = idom[right].expect("visited predecessor has a dominator");
                }
            }
            left
        };
        loop {
            let mut changed = false;
            for &node in postorder.iter().skip(1) {
                let mut known = predecessors[node]
                    .iter()
                    .copied()
                    .filter(|&pred| idom[pred].is_some());
                let mut parent = known
                    .next()
                    .expect("reachable block has a known predecessor");
                for pred in known {
                    parent = intersect(parent, pred, &idom);
                }
                if idom[node] != Some(parent) {
                    idom[node] = Some(parent);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut children = vec![Vec::new(); successors.len()];
        for (node, parent) in idom.into_iter().enumerate().skip(1) {
            children[parent.expect("reachable block has a dominator")].push(node);
        }
        let mut entry = vec![0; successors.len()];
        let mut exit = vec![0; successors.len()];
        let mut time = 0;
        let mut stack = vec![(0, false)];
        while let Some((node, leaving)) = stack.pop() {
            time += 1;
            if leaving {
                exit[node] = time;
            } else {
                entry[node] = time;
                stack.push((node, true));
                for &child in children[node].iter().rev() {
                    stack.push((child, false));
                }
            }
        }
        Self { entry, exit }
    }

    pub(super) fn dominates(&self, definition: usize, use_block: usize) -> bool {
        self.entry[definition] <= self.entry[use_block]
            && self.exit[use_block] <= self.exit[definition]
    }
}

#[cfg(test)]
mod tests {
    use super::Dominators;

    #[test]
    fn single_entry_block_dominates_itself() {
        let tree = Dominators::new(&[vec![]], &[vec![]]);
        assert!(tree.dominates(0, 0));
    }

    #[test]
    fn compact_dominators_agree_with_set_intersection_on_generated_graphs() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..64 {
            let count = 24;
            let mut successors = vec![Vec::new(); count];
            for (node, edges) in successors.iter_mut().enumerate().take(count - 1) {
                edges.push(node + 1);
            }
            for _ in 0..72 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let from = (seed as usize) % count;
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let to = 1 + (seed as usize) % (count - 1);
                successors[from].push(to);
            }
            let mut predecessors = vec![Vec::new(); count];
            for (from, edges) in successors.iter().enumerate() {
                for &to in edges {
                    predecessors[to].push(from);
                }
            }
            let compact = Dominators::new(&successors, &predecessors);
            let mut reference = vec![vec![true; count]; count];
            reference[0] = (0..count).map(|id| id == 0).collect();
            loop {
                let mut changed = false;
                for node in 1..count {
                    let next = (0..count)
                        .map(|candidate| {
                            candidate == node
                                || predecessors[node]
                                    .iter()
                                    .all(|&pred| reference[pred][candidate])
                        })
                        .collect::<Vec<_>>();
                    if reference[node] != next {
                        reference[node] = next;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            for (use_block, expected) in reference.iter().enumerate() {
                for (definition, &dominates) in expected.iter().enumerate() {
                    assert_eq!(
                        compact.dominates(definition, use_block),
                        dominates,
                        "dominance of {definition} over {use_block}"
                    );
                }
            }
        }
    }
}
