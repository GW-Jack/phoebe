//! Opaque shared storage for closed I64 sequences. Every raw pointer handed to
//! generated code owns one reference; retain and release change that count.

use std::alloc::{Layout, alloc, dealloc};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicUsize, Ordering, fence};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordsError {
    Allocation,
    Bounds,
    RefcountOverflow,
}

#[repr(C)]
pub struct ManagedWords {
    owners: AtomicUsize,
    len: usize,
}

fn storage_layout(len: usize) -> Result<(Layout, usize), WordsError> {
    let elements = Layout::array::<i64>(len).map_err(|_| WordsError::Allocation)?;
    let (layout, offset) = Layout::new::<ManagedWords>()
        .extend(elements)
        .map_err(|_| WordsError::Allocation)?;
    Ok((layout.pad_to_align(), offset))
}

fn words_offset() -> usize {
    storage_layout(0).expect("header layout fits").1
}

unsafe fn data(pointer: NonNull<ManagedWords>) -> *mut i64 {
    // SAFETY: the allocation includes a trailing I64 slice at this offset.
    unsafe {
        pointer
            .as_ptr()
            .cast::<u8>()
            .add(words_offset())
            .cast::<i64>()
    }
}

/// One owner of an immutable buffer. Mutation of a shared buffer first makes
/// an independent copy; mutation of a sole owner writes in place.
pub struct SharedWords(NonNull<ManagedWords>);

// SAFETY: all shared reads are immutable, reference counts are atomic, and a
// mutable write needs &mut SharedWords plus a sole owner count.
unsafe impl Send for SharedWords {}
unsafe impl Sync for SharedWords {}

impl SharedWords {
    pub fn try_new(words: &[i64]) -> Result<Self, WordsError> {
        // SAFETY: the global allocator returns a fresh block for the layout.
        unsafe { Self::try_new_with(words, |layout| alloc(layout)) }
    }

    /// # Safety
    /// `allocate` must return null or a fresh global-allocator block with the
    /// supplied layout. This hook supports allocation-failure tests.
    unsafe fn try_new_with(
        words: &[i64],
        allocate: impl FnOnce(Layout) -> *mut u8,
    ) -> Result<Self, WordsError> {
        let (layout, _) = storage_layout(words.len())?;
        let pointer =
            NonNull::new(allocate(layout).cast::<ManagedWords>()).ok_or(WordsError::Allocation)?;
        // SAFETY: `pointer` owns `layout`, and its trailing slice has room for
        // exactly `words.len()` initialized I64 values.
        unsafe {
            pointer.as_ptr().write(ManagedWords {
                owners: AtomicUsize::new(1),
                len: words.len(),
            });
            ptr::copy_nonoverlapping(words.as_ptr(), data(pointer), words.len());
        }
        Ok(Self(pointer))
    }

    pub fn as_slice(&self) -> &[i64] {
        let len = unsafe { self.0.as_ref().len };
        // SAFETY: this owner keeps the allocation live. Shared storage is only
        // mutated after it has been copied, so the slice remains immutable.
        unsafe { std::slice::from_raw_parts(data(self.0), len) }
    }

    pub fn owners(&self) -> usize {
        unsafe { self.0.as_ref().owners.load(Ordering::Acquire) }
    }

    pub fn shares_storage_with(&self, other: &Self) -> bool {
        self.0 == other.0
    }

    pub fn try_retain(&self) -> Result<Self, WordsError> {
        let owners = unsafe { &self.0.as_ref().owners };
        owners
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count > 0 && count < isize::MAX as usize).then_some(count + 1)
            })
            .map_err(|_| WordsError::RefcountOverflow)?;
        Ok(Self(self.0))
    }

    /// Returns whether the write copied shared storage. On allocation failure
    /// the receiver and every other owner keep their original bytes.
    pub fn try_set(&mut self, index: usize, value: i64) -> Result<bool, WordsError> {
        // SAFETY: the global allocator returns a fresh block for the layout.
        unsafe { self.try_set_with(index, value, |layout| alloc(layout)) }
    }

    /// # Safety
    /// `allocate` has the same contract as in `try_new_with`.
    unsafe fn try_set_with(
        &mut self,
        index: usize,
        value: i64,
        allocate: impl FnOnce(Layout) -> *mut u8,
    ) -> Result<bool, WordsError> {
        if index >= self.as_slice().len() {
            return Err(WordsError::Bounds);
        }
        if self.owners() == 1 {
            // SAFETY: no other owner exists, and `&mut self` excludes local
            // aliases. The index was checked against the initialized length.
            unsafe { data(self.0).add(index).write(value) };
            return Ok(false);
        }
        // SAFETY: the allocation hook obeys the stated contract. The old
        // owner is not released until the new allocation and write succeed.
        let replacement = unsafe { Self::try_new_with(self.as_slice(), allocate) }?;
        unsafe { data(replacement.0).add(index).write(value) };
        let old = std::mem::replace(self, replacement);
        drop(old);
        Ok(true)
    }

    pub fn into_raw(self) -> *mut ManagedWords {
        let pointer = self.0.as_ptr();
        std::mem::forget(self);
        pointer
    }

    /// # Safety
    /// `pointer` must be a live owning reference from `into_raw` or a
    /// successful retain. Each such pointer may be reconstructed only once.
    pub unsafe fn from_raw(pointer: *mut ManagedWords) -> Self {
        Self(NonNull::new(pointer).expect("managed storage pointer is non-null"))
    }
}

impl Clone for SharedWords {
    fn clone(&self) -> Self {
        self.try_retain()
            .expect("managed storage reference count overflow")
    }
}

impl Drop for SharedWords {
    fn drop(&mut self) {
        let header = unsafe { self.0.as_ref() };
        if header.owners.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire);
        let (layout, _) = storage_layout(header.len).expect("allocated layout remains valid");
        // SAFETY: this was the final owner, and `layout` is the one used for
        // the original global-allocator allocation.
        unsafe { dealloc(self.0.as_ptr().cast::<u8>(), layout) };
    }
}

/// Return null on allocation failure. A non-null result owns one reference.
///
/// # Safety
/// `source` must point to `len` initialized I64 words (or may be null when
/// `len` is zero). The caller must release every successful returned owner.
#[unsafe(export_name = "phoebe_rt_v1_words_new")]
pub unsafe extern "C" fn phoebe_words_new(source: *const i64, len: usize) -> *mut ManagedWords {
    if source.is_null() && len != 0 {
        return ptr::null_mut();
    }
    let words = if len == 0 {
        &[]
    } else {
        // SAFETY: required by the caller contract above.
        unsafe { std::slice::from_raw_parts(source, len) }
    };
    SharedWords::try_new(words).map_or(ptr::null_mut(), SharedWords::into_raw)
}

/// Return 0 after adding one owner, or -1 on reference-count overflow.
///
/// # Safety
/// `pointer` must name live managed storage. The caller must eventually
/// release the additional owner on success.
#[unsafe(export_name = "phoebe_rt_v1_words_retain")]
pub unsafe extern "C" fn phoebe_words_retain(pointer: *mut ManagedWords) -> i64 {
    let Some(pointer) = NonNull::new(pointer) else {
        return -1;
    };
    let owner = std::mem::ManuallyDrop::new(SharedWords(pointer));
    match owner.try_retain() {
        Ok(retained) => {
            retained.into_raw();
            0
        }
        Err(_) => -1,
    }
}

/// Release one raw owner. Null is accepted for optional owner slots.
///
/// # Safety
/// A non-null pointer must be one live owning reference and must not be
/// released twice.
#[unsafe(export_name = "phoebe_rt_v1_words_release")]
pub unsafe extern "C" fn phoebe_words_release(pointer: *mut ManagedWords) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: required by the caller contract above.
    drop(unsafe { SharedWords::from_raw(pointer) });
}

/// Read a checked index from a live managed buffer.
///
/// # Safety
/// `pointer` must be live and `index` must be smaller than its length.
#[unsafe(export_name = "phoebe_rt_v1_words_get")]
pub unsafe extern "C" fn phoebe_words_get(pointer: *mut ManagedWords, index: usize) -> i64 {
    let owner = std::mem::ManuallyDrop::new(unsafe { SharedWords::from_raw(pointer) });
    owner.as_slice()[index]
}

/// Return 0 for an in-place write, 1 for a copy-on-write, -1 for allocation
/// failure, or -2 for an invalid index. An error leaves `slot` unchanged.
///
/// # Safety
/// `slot` must be an exclusive writable slot containing one live raw owner.
#[unsafe(export_name = "phoebe_rt_v1_words_set")]
pub unsafe extern "C" fn phoebe_words_set(
    slot: *mut *mut ManagedWords,
    index: usize,
    value: i64,
) -> i64 {
    let Some(slot) = NonNull::new(slot) else {
        return -2;
    };
    let pointer = unsafe { *slot.as_ptr() };
    let Some(pointer) = NonNull::new(pointer) else {
        return -2;
    };
    let mut owner = std::mem::ManuallyDrop::new(SharedWords(pointer));
    let outcome = owner.try_set(index, value);
    unsafe { *slot.as_ptr() = owner.0.as_ptr() };
    match outcome {
        Ok(false) => 0,
        Ok(true) => 1,
        Err(WordsError::Allocation) => -1,
        Err(WordsError::Bounds) | Err(WordsError::RefcountOverflow) => -2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retain_and_copy_on_write_preserve_prior_value() {
        let mut current = SharedWords::try_new(&[40, 2]).unwrap();
        let prior = current.try_retain().unwrap();
        assert!(prior.shares_storage_with(&current));
        assert_eq!(current.owners(), 2);
        assert_eq!(current.try_set(0, 0), Ok(true));
        assert_eq!(prior.as_slice(), &[40, 2]);
        assert_eq!(current.as_slice(), &[0, 2]);
        assert!(!prior.shares_storage_with(&current));
        assert_eq!(current.try_set(0, 40), Ok(false));
        assert_eq!(current.as_slice(), &[40, 2]);
    }

    #[test]
    fn failed_cow_allocation_keeps_both_owners_and_bytes() {
        let mut current = SharedWords::try_new(&[40, 2]).unwrap();
        let prior = current.try_retain().unwrap();
        let result = unsafe { current.try_set_with(0, 0, |_| ptr::null_mut()) };
        assert_eq!(result, Err(WordsError::Allocation));
        assert!(current.shares_storage_with(&prior));
        assert_eq!(current.owners(), 2);
        assert_eq!(current.as_slice(), &[40, 2]);
        assert_eq!(current.try_set(2, 0), Err(WordsError::Bounds));
    }

    #[test]
    fn failed_initial_allocation_publishes_no_owner() {
        let result = unsafe { SharedWords::try_new_with(&[40, 2], |_| ptr::null_mut()) };
        assert!(matches!(result, Err(WordsError::Allocation)));
    }

    #[test]
    fn native_abi_obeys_owner_transfer_and_mutation_contract() {
        let input = [40, 2];
        let original = unsafe { phoebe_words_new(input.as_ptr(), input.len()) };
        assert!(!original.is_null());
        assert_eq!(unsafe { phoebe_words_retain(original) }, 0);
        let borrowed = std::mem::ManuallyDrop::new(unsafe { SharedWords::from_raw(original) });
        assert_eq!(borrowed.owners(), 2);
        let mut current = original;
        assert_eq!(unsafe { phoebe_words_set(&mut current, 0, 0) }, 1);
        assert_ne!(current, original);
        assert_eq!(borrowed.owners(), 1);
        assert_eq!(unsafe { phoebe_words_get(original, 0) }, 40);
        assert_eq!(unsafe { phoebe_words_get(current, 0) }, 0);
        unsafe {
            phoebe_words_release(original);
            phoebe_words_release(current);
        }
    }
}
