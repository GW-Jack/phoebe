use phoebe::capability_flow::{Action, Block, Edge, Graph, Slot, Target, verify};

fn edge(target: Target, actions: Vec<Action>) -> Edge {
    Edge { target, actions }
}

#[test]
fn branch_cannot_hide_a_live_right_at_join() {
    let graph = Graph {
        slots: 1,
        entry_live: vec![Slot(0)],
        blocks: vec![
            Block {
                actions: vec![],
                exits: vec![
                    edge(Target::Block(1), vec![]),
                    edge(Target::Block(2), vec![Action::Transfer(Slot(0))]),
                ],
            },
            Block {
                actions: vec![],
                exits: vec![edge(Target::Block(3), vec![])],
            },
            Block {
                actions: vec![],
                exits: vec![edge(Target::Block(3), vec![])],
            },
            Block {
                actions: vec![],
                exits: vec![edge(Target::Return, vec![])],
            },
        ],
    };
    assert!(
        verify(&graph)
            .unwrap_err()
            .contains("differs across incoming edges")
    );
}

#[test]
fn edge_close_reconciles_branch_ownership() {
    let graph = Graph {
        slots: 1,
        entry_live: vec![Slot(0)],
        blocks: vec![
            Block {
                actions: vec![],
                exits: vec![
                    edge(Target::Block(1), vec![Action::Close(Slot(0))]),
                    edge(Target::Block(2), vec![Action::Transfer(Slot(0))]),
                ],
            },
            Block {
                actions: vec![],
                exits: vec![edge(Target::Block(3), vec![])],
            },
            Block {
                actions: vec![],
                exits: vec![edge(Target::Block(3), vec![])],
            },
            Block {
                actions: vec![],
                exits: vec![edge(Target::Return, vec![])],
            },
        ],
    };
    assert!(verify(&graph).unwrap().failures.is_empty());
}

#[test]
fn normal_return_requires_explicit_disposition() {
    let graph = Graph {
        slots: 1,
        entry_live: vec![Slot(0)],
        blocks: vec![Block {
            actions: vec![],
            exits: vec![edge(Target::Return, vec![])],
        }],
    };
    assert!(verify(&graph).unwrap_err().contains("normal return leaks"));
}

#[test]
fn loop_backedge_must_restore_header_ownership() {
    let graph = Graph {
        slots: 1,
        entry_live: vec![Slot(0)],
        blocks: vec![
            Block {
                actions: vec![],
                exits: vec![edge(Target::Block(1), vec![])],
            },
            Block {
                actions: vec![],
                exits: vec![
                    edge(Target::Block(2), vec![]),
                    edge(Target::Return, vec![Action::Close(Slot(0))]),
                ],
            },
            Block {
                actions: vec![],
                exits: vec![edge(Target::Block(1), vec![Action::Close(Slot(0))])],
            },
        ],
    };
    assert!(
        verify(&graph)
            .unwrap_err()
            .contains("differs across incoming edges")
    );
}

#[test]
fn failure_edge_yields_exact_cleanup_slots() {
    let graph = Graph {
        slots: 3,
        entry_live: vec![Slot(0), Slot(1)],
        blocks: vec![Block {
            actions: vec![Action::Move {
                from: Slot(1),
                to: Slot(2),
            }],
            exits: vec![
                edge(Target::Failure, vec![]),
                edge(
                    Target::Return,
                    vec![Action::Close(Slot(0)), Action::Transfer(Slot(2))],
                ),
            ],
        }],
    };
    let facts = verify(&graph).unwrap();
    assert_eq!(facts.failures.len(), 1);
    assert_eq!(facts.failures[0].live, vec![Slot(0), Slot(2)]);
}

#[test]
fn suspension_records_retained_frame_capabilities() {
    let graph = Graph {
        slots: 2,
        entry_live: vec![Slot(0), Slot(1)],
        blocks: vec![
            Block {
                actions: vec![],
                exits: vec![
                    edge(Target::Suspend(1), vec![]),
                    edge(Target::Block(1), vec![]),
                ],
            },
            Block {
                actions: vec![Action::Require(Slot(1))],
                exits: vec![edge(
                    Target::Return,
                    vec![Action::Close(Slot(0)), Action::Transfer(Slot(1))],
                )],
            },
        ],
    };
    let facts = verify(&graph).unwrap();
    assert_eq!(facts.suspensions.len(), 1);
    assert_eq!(facts.suspensions[0].resume, 1);
    assert_eq!(facts.suspensions[0].live, vec![Slot(0), Slot(1)]);
}

#[test]
fn capability_cannot_be_duplicated_or_consumed_twice() {
    let mut graph = Graph {
        slots: 1,
        entry_live: vec![Slot(0)],
        blocks: vec![Block {
            actions: vec![Action::Create(Slot(0))],
            exits: vec![edge(Target::Return, vec![Action::Close(Slot(0))])],
        }],
    };
    assert!(verify(&graph).unwrap_err().contains("already live"));
    graph.blocks[0].actions = vec![Action::Close(Slot(0)), Action::Close(Slot(0))];
    assert!(verify(&graph).unwrap_err().contains("not live"));
}

#[test]
fn fallible_pair_creation_changes_only_the_success_edge() {
    let pair = Action::CreatePair {
        first: Slot(1),
        second: Slot(2),
    };
    let graph = Graph {
        slots: 3,
        entry_live: vec![Slot(0)],
        blocks: vec![
            Block {
                actions: vec![],
                exits: vec![
                    edge(Target::Block(1), vec![pair]),
                    edge(Target::Block(2), vec![]),
                ],
            },
            Block {
                actions: vec![Action::Require(Slot(1)), Action::Require(Slot(2))],
                exits: vec![edge(
                    Target::Return,
                    vec![
                        Action::Close(Slot(1)),
                        Action::Close(Slot(2)),
                        Action::Close(Slot(0)),
                    ],
                )],
            },
            Block {
                actions: vec![Action::Require(Slot(0))],
                exits: vec![edge(Target::Return, vec![Action::Close(Slot(0))])],
            },
        ],
    };
    assert!(verify(&graph).is_ok());

    let mut missing_endpoint = graph.clone();
    missing_endpoint.blocks[0].exits[0].actions = vec![Action::Create(Slot(1))];
    assert!(verify(&missing_endpoint).unwrap_err().contains("not live"));

    let mut partial_failure = graph;
    partial_failure.blocks[0].exits[1].actions = vec![Action::Create(Slot(1))];
    assert!(
        verify(&partial_failure)
            .unwrap_err()
            .contains("normal return leaks")
    );
}

#[test]
fn paired_creation_requires_distinct_empty_slots() {
    let mut graph = Graph {
        slots: 2,
        entry_live: vec![Slot(0)],
        blocks: vec![Block {
            actions: vec![Action::CreatePair {
                first: Slot(1),
                second: Slot(1),
            }],
            exits: vec![edge(Target::Return, vec![Action::Close(Slot(0))])],
        }],
    };
    assert!(verify(&graph).unwrap_err().contains("aliases a slot"));
    graph.blocks[0].actions[0] = Action::CreatePair {
        first: Slot(0),
        second: Slot(1),
    };
    assert!(verify(&graph).unwrap_err().contains("already live"));
}
