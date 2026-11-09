//! Finite unions of field subtrees. A path is a sequence of layout field
//! indices; a subtree denotes that path and every descendant of it.
//!
//! This is a regular language: each term is `path · Σ*`. Keeping only the
//! shortest prefixes gives a canonical antichain. Intersection is nonempty
//! exactly when one of the two prefixes extends the other. There is no
//! depth cutoff, regex parsing, or search in the compilation path.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathLanguage {
    prefixes: Vec<Vec<usize>>,
}

impl PathLanguage {
    pub fn subtrees_overlap(left: &[usize], right: &[usize]) -> bool {
        left.starts_with(right) || right.starts_with(left)
    }

    pub fn subtree(path: impl Into<Vec<usize>>) -> Self {
        Self {
            prefixes: vec![path.into()],
        }
    }

    pub fn union(&mut self, other: &Self) {
        for path in &other.prefixes {
            if self.prefixes.iter().any(|prefix| path.starts_with(prefix)) {
                continue;
            }
            self.prefixes.retain(|prefix| !prefix.starts_with(path));
            self.prefixes.push(path.clone());
        }
        self.prefixes.sort();
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.prefixes.iter().any(|left| {
            other
                .prefixes
                .iter()
                .any(|right| Self::subtrees_overlap(left, right))
        })
    }

    pub fn overlaps_subtree(&self, path: &[usize]) -> bool {
        self.prefixes
            .iter()
            .any(|prefix| Self::subtrees_overlap(prefix, path))
    }
}

#[cfg(test)]
mod tests {
    use super::PathLanguage;

    #[test]
    fn parent_and_child_overlap_but_siblings_do_not() {
        let parent = PathLanguage::subtree(vec![1]);
        let left = PathLanguage::subtree(vec![1, 0]);
        let right = PathLanguage::subtree(vec![1, 1]);
        assert!(parent.overlaps(&left));
        assert!(left.overlaps(&parent));
        assert!(!left.overlaps(&right));
        assert!(PathLanguage::subtree(vec![]).overlaps(&right));
    }

    #[test]
    fn union_normalizes_subsumed_paths() {
        let mut paths = PathLanguage::subtree(vec![1, 0]);
        paths.union(&PathLanguage::subtree(vec![2]));
        paths.union(&PathLanguage::subtree(vec![1]));
        assert_eq!(paths.prefixes, vec![vec![1], vec![2]]);
        assert!(!paths.overlaps(&PathLanguage::subtree(vec![0])));
        assert!(paths.overlaps(&PathLanguage::subtree(vec![2, 3, 4])));
    }

    #[test]
    fn bounded_enumeration_agrees_with_language_intersection() {
        let all = [
            vec![],
            vec![0],
            vec![1],
            vec![0, 0],
            vec![0, 1],
            vec![1, 0],
            vec![1, 1],
        ];
        for left_mask in 0..(1 << all.len()) {
            for right_mask in 0..(1 << all.len()) {
                let mut left = PathLanguage::default();
                let mut right = PathLanguage::default();
                for (index, path) in all.iter().enumerate() {
                    if left_mask & (1 << index) != 0 {
                        left.union(&PathLanguage::subtree(path.clone()));
                    }
                    if right_mask & (1 << index) != 0 {
                        right.union(&PathLanguage::subtree(path.clone()));
                    }
                }
                let brute = all.iter().any(|word| {
                    left.prefixes.iter().any(|prefix| word.starts_with(prefix))
                        && right.prefixes.iter().any(|prefix| word.starts_with(prefix))
                });
                assert_eq!(left.overlaps(&right), brute, "{left:?} vs {right:?}");
            }
        }
    }
}
