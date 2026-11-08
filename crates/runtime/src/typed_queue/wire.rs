//! Stable native ABI encoding of a typed message's structural right plan.
//! Word 0 is the version; the remaining words form one preorder tree.

use super::TypedRightPlan;

const VERSION: u64 = 1;
pub(crate) const MAX_WORDS: usize = 4096;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanDecodeError {
    Invalid,
    Allocation,
}

impl TypedRightPlan {
    pub fn encode_native_words(&self) -> Vec<u64> {
        fn append(plan: &TypedRightPlan, words: &mut Vec<u64>) {
            match plan {
                TypedRightPlan::Empty => words.push(0),
                TypedRightPlan::ReplyTo { part } => {
                    words.extend([1, *part as u64]);
                }
                TypedRightPlan::Sequence(children) => {
                    words.extend([2, children.len() as u64]);
                    for child in children {
                        append(child, words);
                    }
                }
                TypedRightPlan::Tagged { tag_part, arms } => {
                    words.extend([3, *tag_part as u64, arms.len() as u64]);
                    for arm in arms {
                        append(arm, words);
                    }
                }
            }
        }

        let mut words = vec![VERSION];
        append(self, &mut words);
        words
    }

    pub(crate) fn decode_native_words(words: &[u64]) -> Result<Self, PlanDecodeError> {
        fn read(words: &[u64], cursor: &mut usize) -> Result<u64, PlanDecodeError> {
            let word = *words.get(*cursor).ok_or(PlanDecodeError::Invalid)?;
            *cursor += 1;
            Ok(word)
        }

        fn children(
            words: &[u64],
            cursor: &mut usize,
            count: u64,
            depth: usize,
        ) -> Result<Vec<TypedRightPlan>, PlanDecodeError> {
            let count = usize::try_from(count).map_err(|_| PlanDecodeError::Invalid)?;
            if count > words.len().saturating_sub(*cursor) {
                return Err(PlanDecodeError::Invalid);
            }
            let mut plans = Vec::new();
            plans
                .try_reserve_exact(count)
                .map_err(|_| PlanDecodeError::Allocation)?;
            for _ in 0..count {
                plans.push(parse(words, cursor, depth + 1)?);
            }
            Ok(plans)
        }

        fn parse(
            words: &[u64],
            cursor: &mut usize,
            depth: usize,
        ) -> Result<TypedRightPlan, PlanDecodeError> {
            if depth >= MAX_DEPTH {
                return Err(PlanDecodeError::Invalid);
            }
            match read(words, cursor)? {
                0 => Ok(TypedRightPlan::Empty),
                1 => Ok(TypedRightPlan::ReplyTo {
                    part: usize::try_from(read(words, cursor)?)
                        .map_err(|_| PlanDecodeError::Invalid)?,
                }),
                2 => {
                    let count = read(words, cursor)?;
                    Ok(TypedRightPlan::Sequence(children(
                        words, cursor, count, depth,
                    )?))
                }
                3 => {
                    let tag_part = usize::try_from(read(words, cursor)?)
                        .map_err(|_| PlanDecodeError::Invalid)?;
                    let count = read(words, cursor)?;
                    if count == 0 {
                        return Err(PlanDecodeError::Invalid);
                    }
                    Ok(TypedRightPlan::Tagged {
                        tag_part,
                        arms: children(words, cursor, count, depth)?,
                    })
                }
                _ => Err(PlanDecodeError::Invalid),
            }
        }

        if words.len() < 2 || words.len() > MAX_WORDS || words[0] != VERSION {
            return Err(PlanDecodeError::Invalid);
        }
        let mut cursor = 1;
        let plan = parse(words, &mut cursor, 0)?;
        if cursor != words.len() {
            return Err(PlanDecodeError::Invalid);
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_plan_round_trips_and_rejects_malformed_words() {
        let plan = TypedRightPlan::Sequence(vec![
            TypedRightPlan::ReplyTo { part: 1 },
            TypedRightPlan::Tagged {
                tag_part: 2,
                arms: vec![TypedRightPlan::Empty, TypedRightPlan::ReplyTo { part: 3 }],
            },
        ]);
        let words = plan.encode_native_words();
        assert_eq!(TypedRightPlan::decode_native_words(&words), Ok(plan));
        assert_eq!(
            TypedRightPlan::decode_native_words(&words[..words.len() - 1]),
            Err(PlanDecodeError::Invalid)
        );
        assert_eq!(
            TypedRightPlan::decode_native_words(&[1, 0, 0]),
            Err(PlanDecodeError::Invalid)
        );
        assert_eq!(
            TypedRightPlan::decode_native_words(&[2, 0]),
            Err(PlanDecodeError::Invalid)
        );
    }
}
