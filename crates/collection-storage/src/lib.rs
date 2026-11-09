//! Owned scalar sequence storage for the first collection implementation slice.
//! A checked plan selects a concrete type; operations have no runtime layout tag.

use phoebe_collection_plan::{Extent, Plan, Representation};

#[derive(Debug, PartialEq, Eq)]
pub enum StorageError {
    UnsupportedRepresentation,
    InvalidPlan,
    Limit,
    Allocation,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PushError {
    Limit(i64),
    Allocation(i64),
}

pub trait ReadI64 {
    fn as_slice(&self) -> &[i64];

    fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    fn get(&self, index: usize) -> Option<i64> {
        self.as_slice().get(index).copied()
    }
}

/// Exactly N inline scalar words. The native backend will flatten this shape.
#[derive(Debug)]
pub struct FixedInlineI64<const N: usize> {
    words: [i64; N],
}

impl<const N: usize> FixedInlineI64<N> {
    pub fn new(plan: Plan, words: [i64; N]) -> Result<Self, StorageError> {
        if plan.representation != Representation::FixedInline
            || plan.extent != Extent::Static(N)
            || plan.element_bytes != std::mem::size_of::<i64>()
            || plan.effects != Default::default()
            || plan.frozen_after_build
        {
            return Err(StorageError::InvalidPlan);
        }
        Ok(Self { words })
    }
}

impl<const N: usize> ReadI64 for FixedInlineI64<N> {
    fn as_slice(&self) -> &[i64] {
        &self.words
    }
}

/// Runtime length fixed after construction. This Rust host implementation
/// retains Vec metadata for fallible allocation; native lowering must not.
#[derive(Debug)]
pub struct FixedRegionI64 {
    words: Vec<i64>,
}

impl FixedRegionI64 {
    pub fn from_values(plan: Plan, values: &[i64]) -> Result<Self, StorageError> {
        Self::from_values_bounded(plan, values, usize::MAX)
    }

    pub fn from_values_bounded(
        plan: Plan,
        values: &[i64],
        max_elements: usize,
    ) -> Result<Self, StorageError> {
        if plan.representation != Representation::FixedRegion
            || plan.element_bytes != std::mem::size_of::<i64>()
            || matches!(plan.extent, Extent::Static(count) if count != values.len())
            || !plan.effects.reserve
            || plan.effects.grow
            || plan.effects.relocate_elements
            || plan.effects.shift_elements
            || plan.effects.allocate_chunk
            || plan.frozen_after_build
        {
            return Err(StorageError::InvalidPlan);
        }
        if values.len() > max_elements {
            return Err(StorageError::Limit);
        }
        let mut words = Vec::new();
        words
            .try_reserve_exact(values.len())
            .map_err(|_| StorageError::Allocation)?;
        words.extend_from_slice(values);
        Ok(Self { words })
    }
}

impl ReadI64 for FixedRegionI64 {
    fn as_slice(&self) -> &[i64] {
        &self.words
    }
}

/// Owned contiguous buffer. Rust's exclusive borrow prevents append during
/// a live element reference. Growth is fallible and preserves the input value.
#[derive(Debug)]
pub struct AppendI64 {
    words: Vec<i64>,
}

impl AppendI64 {
    pub fn new(plan: Plan) -> Result<Self, StorageError> {
        if plan.representation != Representation::Contiguous
            || plan.extent != Extent::Static(0)
            || plan.element_bytes != std::mem::size_of::<i64>()
            || !plan.effects.reserve
            || !plan.effects.grow
            || !plan.effects.relocate_elements
            || plan.effects.shift_elements
            || plan.effects.allocate_chunk
        {
            return Err(StorageError::InvalidPlan);
        }
        Ok(Self { words: Vec::new() })
    }

    pub fn push(&mut self, value: i64) -> Result<(), PushError> {
        self.push_bounded(value, usize::MAX)
    }

    pub fn push_bounded(&mut self, value: i64, max_elements: usize) -> Result<(), PushError> {
        if self.words.len() >= max_elements {
            return Err(PushError::Limit(value));
        }
        self.words
            .try_reserve(1)
            .map_err(|_| PushError::Allocation(value))?;
        self.words.push(value);
        Ok(())
    }

    /// Consume the builder without copying or reallocating its elements.
    pub fn freeze(self) -> FrozenI64 {
        FrozenI64 { words: self.words }
    }
}

impl ReadI64 for AppendI64 {
    fn as_slice(&self) -> &[i64] {
        &self.words
    }
}

/// A read-only owner with no growth operation in its public surface.
#[derive(Debug)]
pub struct FrozenI64 {
    words: Vec<i64>,
}

impl ReadI64 for FrozenI64 {
    fn as_slice(&self) -> &[i64] {
        &self.words
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoebe_collection_plan::{Constraints, Extent, Requirements, Use, select, summarize_path};

    const LOCAL: Constraints = Constraints {
        element_bytes: 8,
        max_inline_bytes: 256,
        representation_may_escape: false,
    };

    #[test]
    fn fixed_inline_uses_exactly_its_static_words() {
        let plan = select(Requirements::new(Extent::Static(3)), LOCAL).unwrap();
        let values = FixedInlineI64::new(plan, [3, 5, 8]).unwrap();
        assert_eq!(
            std::mem::size_of_val(&values),
            3 * std::mem::size_of::<i64>()
        );
        assert_eq!(values.len(), 3);
        assert_eq!(values.get(2), Some(8));
        assert_eq!(values.get(3), None);
    }

    #[test]
    fn runtime_fixed_extent_has_no_growth_method() {
        let plan = select(Requirements::new(Extent::Runtime), LOCAL).unwrap();
        let values = FixedRegionI64::from_values(plan, &[1, 2, 3]).unwrap();
        assert_eq!(values.get(1), Some(2));
        assert_eq!(values.len(), 3);
    }

    #[test]
    fn append_then_freeze_keeps_the_same_elements() {
        let uses = summarize_path(Extent::Static(0), &[Use::Append, Use::Freeze]).unwrap();
        let plan = select(uses, LOCAL).unwrap();
        let mut values = AppendI64::new(plan).unwrap();
        assert!(values.is_empty());
        values.push(21).unwrap();
        values.push(34).unwrap();
        let before = values.as_slice().as_ptr();
        let frozen = values.freeze();
        assert_eq!(frozen.as_slice().as_ptr(), before);
        assert_eq!(frozen.as_slice(), &[21, 34]);
    }

    #[test]
    fn a_stable_append_plan_cannot_use_moving_storage() {
        let uses = summarize_path(Extent::Static(0), &[Use::LoanSpansAppend]).unwrap();
        let plan = select(uses, LOCAL).unwrap();
        assert_eq!(AppendI64::new(plan).unwrap_err(), StorageError::InvalidPlan);
    }

    #[test]
    fn checked_plan_binds_element_shape_and_initial_extent() {
        let fixed = select(Requirements::new(Extent::Static(3)), LOCAL).unwrap();
        assert_eq!(
            FixedInlineI64::new(fixed, [1, 2]).unwrap_err(),
            StorageError::InvalidPlan
        );
        let different_element = select(
            Requirements::new(Extent::Static(3)),
            Constraints {
                element_bytes: 4,
                ..LOCAL
            },
        )
        .unwrap();
        assert_eq!(
            FixedInlineI64::new(different_element, [1, 2, 3]).unwrap_err(),
            StorageError::InvalidPlan
        );
        let region = select(Requirements::new(Extent::Static(33)), LOCAL).unwrap();
        assert_eq!(
            FixedRegionI64::from_values(region, &[1, 2]).unwrap_err(),
            StorageError::InvalidPlan
        );
    }

    #[test]
    fn bounded_growth_rejects_before_changing_owned_values() {
        let uses = summarize_path(Extent::Static(0), &[Use::Append]).unwrap();
        let plan = select(uses, LOCAL).unwrap();
        let mut values = AppendI64::new(plan).unwrap();
        values.push_bounded(7, 2).unwrap();
        values.push_bounded(11, 2).unwrap();
        assert_eq!(values.push_bounded(13, 2), Err(PushError::Limit(13)));
        assert_eq!(values.as_slice(), &[7, 11]);
        assert_eq!(
            FixedRegionI64::from_values_bounded(
                select(Requirements::new(Extent::Runtime), LOCAL).unwrap(),
                &[7, 11],
                1
            )
            .unwrap_err(),
            StorageError::Limit
        );
    }
}
