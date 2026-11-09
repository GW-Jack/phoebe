//! A bounded, pure representation planner for future checked collection IR.
//! It consumes facts that a source/Core analysis has already established;
//! it neither parses source nor proves that those facts are sound.

mod flow;
pub use flow::{Block, GraphError, summarize_graph};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    Static(usize),
    Runtime,
}

impl Extent {
    pub fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Static(left), Self::Static(right)) if left == right => self,
            _ => Self::Runtime,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mutation {
    None,
    Append,
    Structural,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Access {
    pub scan: bool,
    pub index: bool,
    pub stable_handle: bool,
}

impl Access {
    fn join(self, other: Self) -> Self {
        Self {
            scan: self.scan || other.scan,
            index: self.index || other.index,
            stable_handle: self.stable_handle || other.stable_handle,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requirements {
    extent: Extent,
    mutation: Mutation,
    appends: bool,
    access: Access,
    loan_spans_append: bool,
    frozen_after_build: bool,
    escapes: bool,
}

impl Requirements {
    pub fn new(extent: Extent) -> Self {
        Self {
            extent,
            mutation: Mutation::None,
            appends: false,
            access: Access::default(),
            loan_spans_append: false,
            frozen_after_build: false,
            escapes: false,
        }
    }

    /// Conservative path merge. A phase guarantee survives only if it holds
    /// on every incoming path; required operations accumulate by union.
    pub fn join(self, other: Self) -> Self {
        Self {
            extent: self.extent.join(other.extent),
            mutation: self.mutation.max(other.mutation),
            appends: self.appends || other.appends,
            access: self.access.join(other.access),
            loan_spans_append: self.loan_spans_append || other.loan_spans_append,
            frozen_after_build: self.frozen_after_build && other.frozen_after_build,
            escapes: self.escapes || other.escapes,
        }
    }

    fn record(&mut self, operation: Use) -> Result<(), AnalysisError> {
        match operation {
            Use::Append => {
                if self.frozen_after_build {
                    return Err(AnalysisError::MutationAfterFreeze);
                }
                self.mutation = self.mutation.max(Mutation::Append);
                self.appends = true;
            }
            Use::RemoveByIndex | Use::RemoveByHandle => {
                if self.frozen_after_build {
                    return Err(AnalysisError::MutationAfterFreeze);
                }
                self.mutation = Mutation::Structural;
                if matches!(operation, Use::RemoveByHandle) {
                    self.access.stable_handle = true;
                } else {
                    self.access.index = true;
                }
            }
            Use::Scan => self.access.scan = true,
            Use::Index => self.access.index = true,
            Use::LoanSpansAppend => {
                if self.frozen_after_build {
                    return Err(AnalysisError::MutationAfterFreeze);
                }
                self.mutation = self.mutation.max(Mutation::Append);
                self.appends = true;
                self.loan_spans_append = true;
            }
            Use::Freeze => self.frozen_after_build = true,
            Use::Escape => self.escapes = true,
            Use::Call(effects) => {
                if effects.appends {
                    self.record(Use::Append)?;
                }
                if effects.removes_by_index {
                    self.record(Use::RemoveByIndex)?;
                }
                if effects.removes_by_handle {
                    self.record(Use::RemoveByHandle)?;
                }
                if effects.scans {
                    self.record(Use::Scan)?;
                }
                if effects.indexes {
                    self.record(Use::Index)?;
                }
                if effects.loan_spans_append {
                    self.record(Use::LoanSpansAppend)?;
                }
                if effects.escapes {
                    self.record(Use::Escape)?;
                }
            }
            Use::UnknownCall => return Err(AnalysisError::UnknownCall),
        }
        Ok(())
    }
}

/// Events in one already-checked path. `LoanSpansAppend` is supplied by the
/// loan analysis; the selector does not independently establish loan safety.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Append,
    RemoveByIndex,
    RemoveByHandle,
    Scan,
    Index,
    LoanSpansAppend,
    Freeze,
    Escape,
    /// Effects from a checked callee summary for this collection argument.
    Call(CallEffects),
    /// A call without a checked summary cannot justify a representation.
    UnknownCall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CallEffects {
    pub appends: bool,
    pub removes_by_index: bool,
    pub removes_by_handle: bool,
    pub scans: bool,
    pub indexes: bool,
    pub loan_spans_append: bool,
    pub escapes: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisError {
    MutationAfterFreeze,
    UnknownCall,
}

pub fn summarize_path(extent: Extent, uses: &[Use]) -> Result<Requirements, AnalysisError> {
    let mut requirements = Requirements::new(extent);
    for operation in uses {
        requirements.record(*operation)?;
    }
    Ok(requirements)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Representation {
    FixedInline,
    FixedRegion,
    Contiguous,
    Segmented,
    RegionLinked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Effects {
    pub reserve: bool,
    pub grow: bool,
    pub relocate_elements: bool,
    pub shift_elements: bool,
    pub allocate_chunk: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub representation: Representation,
    pub extent: Extent,
    pub element_bytes: usize,
    pub effects: Effects,
    /// A validated freeze permits growth in the build phase, never afterward.
    pub frozen_after_build: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constraints {
    pub element_bytes: usize,
    pub max_inline_bytes: usize,
    /// A public ABI or unknown caller cannot depend on a private local choice.
    pub representation_may_escape: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionError {
    NoEligibleRepresentation,
    UnknownEscapeLayout,
}

/// A constant-size rule table. Profiling may later choose another eligible
/// layout, but ordinary compilation does no search or benchmarking.
pub fn select(
    requirements: Requirements,
    constraints: Constraints,
) -> Result<Plan, SelectionError> {
    if requirements.escapes && !constraints.representation_may_escape {
        return Err(SelectionError::UnknownEscapeLayout);
    }

    let representation = if requirements.mutation == Mutation::None {
        match requirements.extent {
            Extent::Static(count)
                if count
                    .checked_mul(constraints.element_bytes)
                    .is_some_and(|bytes| bytes <= constraints.max_inline_bytes) =>
            {
                Representation::FixedInline
            }
            _ => Representation::FixedRegion,
        }
    } else if requirements.access.stable_handle {
        if requirements.access.index {
            return Err(SelectionError::NoEligibleRepresentation);
        }
        Representation::RegionLinked
    } else if requirements.loan_spans_append {
        if requirements.mutation == Mutation::Structural {
            return Err(SelectionError::NoEligibleRepresentation);
        }
        Representation::Segmented
    } else {
        Representation::Contiguous
    };

    let effects = match representation {
        Representation::FixedInline => Effects::default(),
        Representation::FixedRegion => Effects {
            reserve: true,
            ..Effects::default()
        },
        Representation::Contiguous => Effects {
            reserve: true,
            grow: requirements.appends,
            relocate_elements: requirements.appends,
            shift_elements: requirements.mutation == Mutation::Structural,
            ..Effects::default()
        },
        Representation::Segmented | Representation::RegionLinked => Effects {
            reserve: true,
            grow: requirements.appends,
            allocate_chunk: requirements.appends,
            ..Effects::default()
        },
    };
    Ok(Plan {
        representation,
        extent: requirements.extent,
        element_bytes: constraints.element_bytes,
        effects,
        frozen_after_build: requirements.frozen_after_build,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: Constraints = Constraints {
        element_bytes: 8,
        max_inline_bytes: 256,
        representation_may_escape: false,
    };

    #[test]
    fn fixed_extent_omits_growth_and_large_or_runtime_extent_uses_a_region() {
        let small = select(Requirements::new(Extent::Static(4)), LOCAL).unwrap();
        assert_eq!(small.representation, Representation::FixedInline);
        assert_eq!(small.effects, Effects::default());
        let large = select(Requirements::new(Extent::Static(33)), LOCAL).unwrap();
        assert_eq!(large.representation, Representation::FixedRegion);
        assert!(!large.effects.grow);
        let runtime = select(Requirements::new(Extent::Runtime), LOCAL).unwrap();
        assert_eq!(runtime.representation, Representation::FixedRegion);
        assert!(!runtime.effects.relocate_elements);
    }

    #[test]
    fn append_only_defaults_to_contiguous_and_stable_loan_requires_segments() {
        let ordinary =
            summarize_path(Extent::Static(0), &[Use::Append, Use::Scan, Use::Index]).unwrap();
        let plan = select(ordinary, LOCAL).unwrap();
        assert_eq!(plan.representation, Representation::Contiguous);
        assert!(plan.effects.relocate_elements);

        let stable = summarize_path(
            Extent::Static(0),
            &[Use::Append, Use::LoanSpansAppend, Use::Index],
        )
        .unwrap();
        let plan = select(stable, LOCAL).unwrap();
        assert_eq!(plan.representation, Representation::Segmented);
        assert!(!plan.effects.relocate_elements);
        assert!(plan.effects.allocate_chunk);
    }

    #[test]
    fn handle_removal_needs_linked_nodes_and_conflicting_indexing_is_rejected() {
        let handles = summarize_path(
            Extent::Static(0),
            &[Use::Append, Use::RemoveByHandle, Use::Scan],
        )
        .unwrap();
        assert_eq!(
            select(handles, LOCAL).unwrap().representation,
            Representation::RegionLinked
        );
        let indexed = handles.join(summarize_path(Extent::Static(0), &[Use::Index]).unwrap());
        assert_eq!(
            select(indexed, LOCAL),
            Err(SelectionError::NoEligibleRepresentation)
        );
    }

    #[test]
    fn removal_only_does_not_acquire_a_growth_effect() {
        let removal = summarize_path(Extent::Static(10), &[Use::RemoveByIndex]).unwrap();
        let plan = select(removal, LOCAL).unwrap();
        assert_eq!(plan.representation, Representation::Contiguous);
        assert!(!plan.effects.grow);
        assert!(!plan.effects.relocate_elements);
        assert!(plan.effects.shift_elements);
    }

    #[test]
    fn freeze_ends_growth_and_path_join_loses_a_nonuniversal_guarantee() {
        let frozen = summarize_path(Extent::Static(0), &[Use::Append, Use::Freeze]).unwrap();
        assert!(select(frozen, LOCAL).unwrap().frozen_after_build);
        assert_eq!(
            summarize_path(Extent::Static(0), &[Use::Freeze, Use::Append]),
            Err(AnalysisError::MutationAfterFreeze)
        );
        let other = summarize_path(Extent::Static(1), &[Use::Append]).unwrap();
        let joined = frozen.join(other);
        assert_eq!(joined.extent, Extent::Runtime);
        assert!(!joined.frozen_after_build);
    }

    #[test]
    fn unknown_escape_and_size_overflow_are_conservative() {
        let escape = summarize_path(Extent::Static(1), &[Use::Escape]).unwrap();
        assert_eq!(
            select(escape, LOCAL),
            Err(SelectionError::UnknownEscapeLayout)
        );
        let huge = select(Requirements::new(Extent::Static(usize::MAX)), LOCAL).unwrap();
        assert_eq!(huge.representation, Representation::FixedRegion);
    }

    #[test]
    fn path_join_is_order_independent_and_idempotent() {
        let a = summarize_path(Extent::Static(4), &[Use::Append, Use::Scan]).unwrap();
        let b = summarize_path(Extent::Static(6), &[Use::Index, Use::Freeze]).unwrap();
        let c = summarize_path(Extent::Runtime, &[Use::LoanSpansAppend, Use::Escape]).unwrap();
        assert_eq!(a.join(a), a);
        assert_eq!(a.join(b), b.join(a));
        assert_eq!(a.join(b).join(c), a.join(b.join(c)));
        let joined = a.join(b).join(c);
        assert_eq!(joined.extent, Extent::Runtime);
        assert!(joined.loan_spans_append);
        assert!(joined.escapes);
        assert!(!joined.frozen_after_build);
    }
}
