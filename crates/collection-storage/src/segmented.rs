//! Host-side stable-address scalar chunks. This does not implement Phoebe loans.

use super::{PushError, StorageError};
use phoebe_collection_plan::{Extent, Plan, Representation};

/// An append-only sequence whose existing chunk buffers never reallocate.
/// Directory growth moves only Vec headers, not initialized elements.
#[derive(Debug)]
pub struct SegmentedI64<const CHUNK: usize> {
    chunks: Vec<Vec<i64>>,
    len: usize,
}

impl<const CHUNK: usize> SegmentedI64<CHUNK> {
    pub fn new(plan: Plan) -> Result<Self, StorageError> {
        if CHUNK == 0
            || plan.representation != Representation::Segmented
            || plan.extent != Extent::Static(0)
            || plan.element_bytes != std::mem::size_of::<i64>()
            || !plan.effects.reserve
            || !plan.effects.grow
            || plan.effects.relocate_elements
            || plan.effects.shift_elements
            || !plan.effects.allocate_chunk
        {
            return Err(StorageError::InvalidPlan);
        }
        Ok(Self {
            chunks: Vec::new(),
            len: 0,
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<i64> {
        if index >= self.len {
            return None;
        }
        self.chunks[index / CHUNK].get(index % CHUNK).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = i64> + '_ {
        self.chunks.iter().flat_map(|chunk| chunk.iter().copied())
    }

    pub fn push(&mut self, value: i64) -> Result<(), PushError> {
        self.push_bounded(value, usize::MAX)
    }

    pub fn push_bounded(&mut self, value: i64, max_elements: usize) -> Result<(), PushError> {
        if self.len >= max_elements {
            return Err(PushError::Limit(value));
        }
        if let Some(last) = self.chunks.last_mut()
            && last.len() < CHUNK
        {
            // Every chunk reserves CHUNK slots before publication.
            last.push(value);
            self.len += 1;
            return Ok(());
        }

        // Reserve both allocations before publishing a new chunk. If either
        // fails, the incoming value and logical sequence remain untouched.
        self.chunks
            .try_reserve(1)
            .map_err(|_| PushError::Allocation(value))?;
        let mut chunk = Vec::new();
        chunk
            .try_reserve_exact(CHUNK)
            .map_err(|_| PushError::Allocation(value))?;
        chunk.push(value);
        self.chunks.push(chunk);
        self.len += 1;
        Ok(())
    }

    /// Consumes the builder without copying any chunk or element.
    pub fn freeze(self) -> FrozenSegmentedI64<CHUNK> {
        FrozenSegmentedI64 {
            chunks: self.chunks,
            len: self.len,
        }
    }

    #[cfg(test)]
    fn element_address(&self, index: usize) -> Option<usize> {
        if index >= self.len {
            return None;
        }
        Some(
            self.chunks[index / CHUNK]
                .as_ptr()
                .wrapping_add(index % CHUNK) as usize,
        )
    }
}

/// Read-only owner of the original chunks.
#[derive(Debug)]
pub struct FrozenSegmentedI64<const CHUNK: usize> {
    chunks: Vec<Vec<i64>>,
    len: usize,
}

impl<const CHUNK: usize> FrozenSegmentedI64<CHUNK> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<i64> {
        if index >= self.len {
            return None;
        }
        self.chunks[index / CHUNK].get(index % CHUNK).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = i64> + '_ {
        self.chunks.iter().flat_map(|chunk| chunk.iter().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoebe_collection_plan::{Constraints, Use, select, summarize_path};

    const LOCAL: Constraints = Constraints {
        element_bytes: 8,
        max_inline_bytes: 256,
        representation_may_escape: false,
    };

    fn plan() -> Plan {
        select(
            summarize_path(Extent::Static(0), &[Use::LoanSpansAppend, Use::Index]).unwrap(),
            LOCAL,
        )
        .unwrap()
    }

    #[test]
    fn stable_addresses_survive_chunk_and_directory_growth() {
        let mut values = SegmentedI64::<3>::new(plan()).unwrap();
        for value in 0..3 {
            values.push(value).unwrap();
        }
        let first = values.element_address(0).unwrap();
        let third = values.element_address(2).unwrap();
        for value in 3..257 {
            values.push(value).unwrap();
            assert_eq!(values.element_address(0), Some(first));
            assert_eq!(values.element_address(2), Some(third));
        }
        assert_eq!(values.len(), 257);
        assert_eq!(values.get(256), Some(256));
        assert_eq!(values.get(257), None);
        assert_eq!(values.iter().sum::<i64>(), (0..257).sum());
        let frozen = values.freeze();
        assert_eq!(frozen.chunks[0].as_ptr() as usize, first);
        assert_eq!(frozen.get(256), Some(256));
        assert_eq!(frozen.iter().sum::<i64>(), (0..257).sum());
    }

    #[test]
    fn limit_failure_preserves_values_and_incoming_owner() {
        let mut values = SegmentedI64::<2>::new(plan()).unwrap();
        values.push_bounded(7, 2).unwrap();
        values.push_bounded(11, 2).unwrap();
        let first = values.element_address(0);
        assert_eq!(values.push_bounded(13, 2), Err(PushError::Limit(13)));
        assert_eq!(values.len(), 2);
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![7, 11]);
        assert_eq!(values.element_address(0), first);
    }

    #[test]
    fn allocation_failure_before_first_chunk_preserves_value() {
        let mut values = SegmentedI64::<{ usize::MAX }>::new(plan()).unwrap();
        assert_eq!(values.push(17), Err(PushError::Allocation(17)));
        assert!(values.is_empty());
        assert_eq!(values.get(0), None);
    }

    #[test]
    fn constructor_rejects_wrong_plan_or_zero_chunk() {
        assert_eq!(
            SegmentedI64::<0>::new(plan()).unwrap_err(),
            StorageError::InvalidPlan
        );
        let contiguous = select(
            summarize_path(Extent::Static(0), &[Use::Append]).unwrap(),
            LOCAL,
        )
        .unwrap();
        assert_eq!(
            SegmentedI64::<4>::new(contiguous).unwrap_err(),
            StorageError::InvalidPlan
        );
    }
}
