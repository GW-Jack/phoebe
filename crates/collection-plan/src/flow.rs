//! A finite forward analysis over already-checked collection uses.
//! The two flow states distinguish construction from a frozen collection.

use super::{AnalysisError, Extent, Requirements, Use};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug)]
pub struct Block<'a> {
    pub uses: &'a [Use],
    pub successors: &'a [usize],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphError {
    InvalidEntry(usize),
    InvalidSuccessor { block: usize, successor: usize },
    MutationAfterFreeze { block: usize },
    UnknownCall { block: usize },
}

/// Infer the union of requirements on every reachable path from `entry`.
/// A freeze guarantee holds only when every reachable exit is frozen. A loop
/// reaches at most two states per block, so the worklist terminates without
/// solver search, including for graphs with no exit.
pub fn summarize_graph(
    extent: Extent,
    blocks: &[Block<'_>],
    entry: usize,
) -> Result<Requirements, GraphError> {
    if entry >= blocks.len() {
        return Err(GraphError::InvalidEntry(entry));
    }
    for (block, body) in blocks.iter().enumerate() {
        for &successor in body.successors {
            if successor >= blocks.len() {
                return Err(GraphError::InvalidSuccessor { block, successor });
            }
        }
    }

    let mut reached = vec![[false; 2]; blocks.len()];
    let mut pending = VecDeque::from([(entry, false)]);
    let mut summary = Requirements::new(extent);
    let mut exits = 0usize;
    let mut all_exits_frozen = true;
    while let Some((block, frozen)) = pending.pop_front() {
        let state = usize::from(frozen);
        if reached[block][state] {
            continue;
        }
        reached[block][state] = true;

        let mut local = Requirements::new(extent);
        local.frozen_after_build = frozen;
        for &operation in blocks[block].uses {
            if let Err(error) = local.record(operation) {
                return Err(match error {
                    AnalysisError::MutationAfterFreeze => GraphError::MutationAfterFreeze { block },
                    AnalysisError::UnknownCall => GraphError::UnknownCall { block },
                });
            }
        }
        let next_frozen = local.frozen_after_build;
        summary = summary.join(local);
        if blocks[block].successors.is_empty() {
            exits += 1;
            all_exits_frozen &= next_frozen;
        } else {
            for &successor in blocks[block].successors {
                if !reached[successor][usize::from(next_frozen)] {
                    pending.push_back((successor, next_frozen));
                }
            }
        }
    }
    summary.frozen_after_build = exits > 0 && all_exits_frozen;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallEffects, Constraints, Representation, SelectionError, select};

    const LOCAL: Constraints = Constraints {
        element_bytes: 8,
        max_inline_bytes: 256,
        representation_may_escape: false,
    };

    #[test]
    fn branches_join_to_append_only_without_removal_machinery() {
        let graph = [
            Block {
                uses: &[],
                successors: &[1, 2],
            },
            Block {
                uses: &[Use::Append],
                successors: &[3],
            },
            Block {
                uses: &[Use::Scan],
                successors: &[3],
            },
            Block {
                uses: &[Use::Index],
                successors: &[],
            },
        ];
        let requirements = summarize_graph(Extent::Static(0), &graph, 0).unwrap();
        let plan = select(requirements, LOCAL).unwrap();
        assert_eq!(plan.representation, Representation::Contiguous);
        assert!(plan.effects.grow);
        assert!(!plan.effects.shift_elements);
        assert!(!plan.frozen_after_build);
    }

    #[test]
    fn freeze_on_all_paths_survives_join_and_forbids_later_append() {
        let graph = [
            Block {
                uses: &[],
                successors: &[1, 2],
            },
            Block {
                uses: &[Use::Append, Use::Freeze],
                successors: &[3],
            },
            Block {
                uses: &[Use::Freeze],
                successors: &[3],
            },
            Block {
                uses: &[Use::Index],
                successors: &[],
            },
        ];
        let requirements = summarize_graph(Extent::Static(0), &graph, 0).unwrap();
        assert!(select(requirements, LOCAL).unwrap().frozen_after_build);

        let mut invalid = graph;
        invalid[3] = Block {
            uses: &[Use::Append],
            successors: &[],
        };
        assert_eq!(
            summarize_graph(Extent::Static(0), &invalid, 0),
            Err(GraphError::MutationAfterFreeze { block: 3 })
        );
    }

    #[test]
    fn one_frozen_path_rejects_mutation_after_join() {
        let graph = [
            Block {
                uses: &[],
                successors: &[1, 2],
            },
            Block {
                uses: &[Use::Freeze],
                successors: &[3],
            },
            Block {
                uses: &[],
                successors: &[3],
            },
            Block {
                uses: &[Use::Append],
                successors: &[],
            },
        ];
        assert_eq!(
            summarize_graph(Extent::Runtime, &graph, 0),
            Err(GraphError::MutationAfterFreeze { block: 3 })
        );
    }

    #[test]
    fn loops_terminate_and_union_loan_requirements() {
        let graph = [
            Block {
                uses: &[],
                successors: &[1],
            },
            Block {
                uses: &[Use::Append],
                successors: &[1, 2],
            },
            Block {
                uses: &[Use::LoanSpansAppend, Use::Index],
                successors: &[],
            },
        ];
        let requirements = summarize_graph(Extent::Runtime, &graph, 0).unwrap();
        let plan = select(requirements, LOCAL).unwrap();
        assert_eq!(plan.representation, Representation::Segmented);
        assert!(!plan.effects.relocate_elements);
    }

    #[test]
    fn unreachable_mutation_and_escape_do_not_affect_choice() {
        let graph = [
            Block {
                uses: &[Use::Scan],
                successors: &[],
            },
            Block {
                uses: &[Use::RemoveByHandle, Use::Escape],
                successors: &[],
            },
        ];
        let requirements = summarize_graph(Extent::Static(4), &graph, 0).unwrap();
        assert_eq!(
            select(requirements, LOCAL).unwrap().representation,
            Representation::FixedInline
        );
        let escaped = summarize_graph(Extent::Static(4), &graph, 1).unwrap();
        assert_eq!(
            select(escaped, LOCAL),
            Err(SelectionError::UnknownEscapeLayout)
        );
    }

    #[test]
    fn invalid_edges_are_reported_before_analysis() {
        let graph = [Block {
            uses: &[],
            successors: &[2],
        }];
        assert_eq!(
            summarize_graph(Extent::Runtime, &graph, 1),
            Err(GraphError::InvalidEntry(1))
        );
        assert_eq!(
            summarize_graph(Extent::Runtime, &graph, 0),
            Err(GraphError::InvalidSuccessor {
                block: 0,
                successor: 2
            })
        );
    }

    #[test]
    fn checked_call_effects_join_with_local_uses() {
        let graph = [
            Block {
                uses: &[Use::Call(CallEffects {
                    appends: true,
                    scans: true,
                    ..CallEffects::default()
                })],
                successors: &[1],
            },
            Block {
                uses: &[Use::Index],
                successors: &[],
            },
        ];
        let plan = select(
            summarize_graph(Extent::Static(0), &graph, 0).unwrap(),
            LOCAL,
        )
        .unwrap();
        assert_eq!(plan.representation, Representation::Contiguous);
        assert!(plan.effects.grow);
        assert!(!plan.effects.shift_elements);
    }

    #[test]
    fn unknown_call_and_mutating_call_after_freeze_are_rejected() {
        let unknown = [Block {
            uses: &[Use::UnknownCall],
            successors: &[],
        }];
        assert_eq!(
            summarize_graph(Extent::Static(4), &unknown, 0),
            Err(GraphError::UnknownCall { block: 0 })
        );

        let frozen = [Block {
            uses: &[
                Use::Freeze,
                Use::Call(CallEffects {
                    appends: true,
                    ..CallEffects::default()
                }),
            ],
            successors: &[],
        }];
        assert_eq!(
            summarize_graph(Extent::Static(4), &frozen, 0),
            Err(GraphError::MutationAfterFreeze { block: 0 })
        );
    }
}
