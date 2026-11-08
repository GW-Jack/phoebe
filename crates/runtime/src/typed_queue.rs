//! Preallocated fixed-width FIFO storage for concrete channel messages.

use super::{REQUEST_I64_MESSAGE_TYPE_ID, ReplyToken, ResourceError};

mod wire;
pub(crate) use wire::{MAX_WORDS as NATIVE_PLAN_MAX_WORDS, PlanDecodeError};

/// Runtime ownership path for the first transferable typed-message right.
/// The compiler will derive this from the ABI close plan. Closed messages use
/// `Empty`; each tagged arm visits only its active payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypedRightPlan {
    Empty,
    ReplyTo {
        part: usize,
    },
    Sequence(Vec<TypedRightPlan>),
    Tagged {
        tag_part: usize,
        arms: Vec<TypedRightPlan>,
    },
}

impl TypedRightPlan {
    fn max_rights(&self, width: usize, depth: usize) -> Result<usize, String> {
        if depth >= 64 {
            return Err("typed right plan exceeds runtime depth limit".into());
        }
        match self {
            Self::Empty => Ok(0),
            Self::ReplyTo { part } if *part < width => Ok(1),
            Self::ReplyTo { .. } => Err("typed right part exceeds message width".into()),
            Self::Sequence(plans) => plans.iter().try_fold(0usize, |count, plan| {
                count
                    .checked_add(plan.max_rights(width, depth + 1)?)
                    .ok_or_else(|| "typed right count overflow".into())
            }),
            Self::Tagged { tag_part, arms } if *tag_part < width && !arms.is_empty() => {
                arms.iter().try_fold(0, |maximum, arm| {
                    Ok(maximum.max(arm.max_rights(width, depth + 1)?))
                })
            }
            Self::Tagged { .. } => Err("typed message tag layout is invalid".into()),
        }
    }

    fn visit(
        &self,
        words: &[u64],
        visit: &mut impl FnMut(ReplyToken) -> Result<(), String>,
    ) -> Result<(), String> {
        match self {
            Self::Empty => Ok(()),
            Self::ReplyTo { part } => {
                let word = *words.get(*part).ok_or("typed right part is missing")?;
                let token = usize::try_from(word).map_err(|_| "typed reply token overflow")?;
                visit(ReplyToken(token))
            }
            Self::Sequence(plans) => {
                for plan in plans {
                    plan.visit(words, visit)?;
                }
                Ok(())
            }
            Self::Tagged { tag_part, arms } => {
                let tag = *words.get(*tag_part).ok_or("typed message tag is missing")?;
                let arm = usize::try_from(tag)
                    .ok()
                    .and_then(|index| arms.get(index))
                    .ok_or("typed message tag is out of range")?;
                arm.visit(words, visit)
            }
        }
    }
}

pub(super) struct TypedQueue {
    type_id: u64,
    width: usize,
    capacity: usize,
    words: Vec<u64>,
    plan: TypedRightPlan,
    scratch: Vec<ReplyToken>,
    head: usize,
    len: usize,
}

impl TypedQueue {
    pub(super) fn new(type_id: u64, width: usize, capacity: usize) -> Result<Self, ResourceError> {
        Self::with_plan(type_id, width, capacity, TypedRightPlan::Empty)
    }

    pub(super) fn with_plan(
        type_id: u64,
        width: usize,
        capacity: usize,
        plan: TypedRightPlan,
    ) -> Result<Self, ResourceError> {
        if capacity == 0 {
            return Err(ResourceError::InvalidChannelCapacity);
        }
        let max_rights = plan
            .max_rights(width, 0)
            .map_err(|_| ResourceError::InvalidChannelCapacity)?;
        let count = width
            .checked_mul(capacity)
            .ok_or(ResourceError::InvalidChannelCapacity)?;
        let mut words = Vec::new();
        words
            .try_reserve_exact(count)
            .map_err(|_| ResourceError::Allocation)?;
        words.resize(count, 0);
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(max_rights)
            .map_err(|_| ResourceError::Allocation)?;
        Ok(Self {
            type_id,
            width,
            capacity,
            words,
            plan,
            scratch,
            head: 0,
            len: 0,
        })
    }

    pub(super) fn matches(&self, type_id: u64, width: usize) -> bool {
        self.type_id == type_id && self.width == width
    }

    pub(super) fn matches_request_i64(&self) -> bool {
        self.matches(REQUEST_I64_MESSAGE_TYPE_ID, 2)
            && self.plan == TypedRightPlan::ReplyTo { part: 1 }
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn capacity(&self) -> usize {
        self.capacity
    }

    pub(super) fn is_full(&self) -> bool {
        self.len == self.capacity
    }

    fn collect(
        plan: &TypedRightPlan,
        words: &[u64],
        scratch: &mut Vec<ReplyToken>,
    ) -> Result<(), String> {
        scratch.clear();
        plan.visit(words, &mut |token| {
            if scratch.contains(&token) {
                return Err("typed message duplicates a reply right".into());
            }
            scratch.push(token);
            Ok(())
        })
    }

    pub(super) fn collect_send_rights(&mut self, words: &[u64]) -> Result<&[ReplyToken], String> {
        Self::collect(&self.plan, words, &mut self.scratch)?;
        Ok(&self.scratch)
    }

    pub(super) fn collect_front_rights(&mut self) -> Result<&[ReplyToken], String> {
        if self.len == 0 {
            return Err("typed queue is empty".into());
        }
        let start = self.head * self.width;
        Self::collect(
            &self.plan,
            &self.words[start..start + self.width],
            &mut self.scratch,
        )?;
        Ok(&self.scratch)
    }

    pub(super) fn visit_queued_rights(
        &self,
        mut visit: impl FnMut(ReplyToken) -> Result<(), String>,
    ) -> Result<(), String> {
        for offset in 0..self.len {
            let index = if self.head >= self.capacity - offset {
                self.head - (self.capacity - offset)
            } else {
                self.head + offset
            };
            let start = index * self.width;
            self.plan
                .visit(&self.words[start..start + self.width], &mut visit)?;
        }
        Ok(())
    }

    pub(super) fn scratch_right(&self, index: usize) -> ReplyToken {
        self.scratch[index]
    }

    pub(super) fn discard_front(&mut self) {
        debug_assert!(self.len > 0);
        self.head = (self.head + 1) % self.capacity;
        self.len -= 1;
    }

    pub(super) fn push(&mut self, value: &[u64]) {
        debug_assert_eq!(value.len(), self.width);
        debug_assert!(!self.is_full());
        let tail = if self.head >= self.capacity - self.len {
            self.head - (self.capacity - self.len)
        } else {
            self.head + self.len
        };
        let start = tail * self.width;
        self.words[start..start + self.width].copy_from_slice(value);
        self.len += 1;
    }

    pub(super) fn pop(&mut self, output: &mut [u64]) -> bool {
        debug_assert_eq!(output.len(), self.width);
        if self.len == 0 {
            return false;
        }
        let start = self.head * self.width;
        output.copy_from_slice(&self.words[start..start + self.width]);
        self.head = (self.head + 1) % self.capacity;
        self.len -= 1;
        true
    }

    pub(super) fn verify(&self) -> bool {
        self.capacity > 0
            && self.head < self.capacity
            && self.len <= self.capacity
            && self.scratch.capacity() >= self.plan.max_rights(self.width, 0).unwrap_or(usize::MAX)
            && self
                .width
                .checked_mul(self.capacity)
                .is_some_and(|count| count == self.words.len())
    }
}

#[cfg(test)]
mod tests;
