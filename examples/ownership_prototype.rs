//! Isolated ownership-semantics prototype. Run with:
//! rustc --edition=2024 --test examples/ownership_prototype.rs -o /tmp/phoebe-ownership-proto
//! /tmp/phoebe-ownership-proto
#![allow(dead_code)] // Cargo also builds this test-only model as an example binary.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;

// Cargo builds examples as binaries during workspace checks; the executable
// assertions live in the test module below.
fn main() {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Closed,
    Capability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Use {
    Read,
    Own,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Borrow,
    Move,
    Retain,
}

/// `future_uses` counts uses after this call, including uses in later functions.
/// A closed immutable value can retain a second owner when an owning call needs
/// the value and the caller still needs it. A capability cannot be retained.
fn infer(kind: Kind, use_mode: Use, future_uses: usize) -> Result<Action, &'static str> {
    match (kind, use_mode, future_uses) {
        (_, Use::Read, _) => Ok(Action::Borrow),
        (_, Use::Own, 0) => Ok(Action::Move),
        (Kind::Closed, Use::Own, _) => Ok(Action::Retain),
        (Kind::Capability, Use::Own, _) => Err("capability has another live use"),
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Costs {
    retains: usize,
    releases: usize,
    byte_copies: usize,
    result_allocations: usize,
}

fn execute(action: Action, costs: &mut Costs) {
    if action == Action::Retain {
        costs.retains += 1;
        costs.releases += 1;
    }
}

/// A caller-owned destination is never readable until every field is filled.
/// A failed or canceled construction drops only initialized fields.
#[derive(Debug)]
struct Destination<T> {
    fields: Vec<Option<T>>,
}

impl<T> Destination<T> {
    fn new(field_count: usize) -> Self {
        Self {
            fields: std::iter::repeat_with(|| None).take(field_count).collect(),
        }
    }

    fn fill(&mut self, index: usize, value: T) -> Result<(), T> {
        match self.fields.get_mut(index) {
            Some(slot @ None) => {
                *slot = Some(value);
                Ok(())
            }
            _ => Err(value),
        }
    }

    fn finish(mut self) -> Result<Vec<T>, Self> {
        if self.fields.iter().any(Option::is_none) {
            return Err(self);
        }
        Ok(self
            .fields
            .iter_mut()
            .map(|field| field.take().unwrap())
            .collect())
    }
}

/// A small derivative-based regular-language model of borrowed field paths.
/// The path alphabet is the finite set of fields in a Phoebe nominal layout.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Path {
    Empty,
    Epsilon,
    Field(&'static str),
    Alt(Box<Path>, Box<Path>),
    Then(Box<Path>, Box<Path>),
    Star(Box<Path>),
}

impl Path {
    fn nullable(&self) -> bool {
        match self {
            Self::Empty | Self::Field(_) => false,
            Self::Epsilon | Self::Star(_) => true,
            Self::Alt(a, b) => a.nullable() || b.nullable(),
            Self::Then(a, b) => a.nullable() && b.nullable(),
        }
    }

    fn alt(a: Self, b: Self) -> Self {
        if a == Self::Empty {
            return b;
        }
        if b == Self::Empty || a == b {
            return a;
        }
        Self::Alt(Box::new(a), Box::new(b))
    }

    fn then(a: Self, b: Self) -> Self {
        if a == Self::Empty || b == Self::Empty {
            return Self::Empty;
        }
        if a == Self::Epsilon {
            return b;
        }
        if b == Self::Epsilon {
            return a;
        }
        Self::Then(Box::new(a), Box::new(b))
    }

    fn derivative(&self, field: &'static str, alphabet: &[&'static str]) -> Self {
        match self {
            Self::Empty | Self::Epsilon => Self::Empty,
            Self::Field(name) if *name == field || (*name == "*" && alphabet.contains(&field)) => {
                Self::Epsilon
            }
            Self::Field(_) => Self::Empty,
            Self::Alt(a, b) => {
                Self::alt(a.derivative(field, alphabet), b.derivative(field, alphabet))
            }
            Self::Then(a, b) => {
                let first = Self::then(a.derivative(field, alphabet), (**b).clone());
                if a.nullable() {
                    Self::alt(first, b.derivative(field, alphabet))
                } else {
                    first
                }
            }
            Self::Star(a) => Self::then(a.derivative(field, alphabet), self.clone()),
        }
    }
}

/// Bounded to paths of depth at most `max_depth`, matching finite nominal
/// layouts. Recursive types need a terminating automaton implementation.
fn overlap(a: &Path, b: &Path, alphabet: &[&'static str], max_depth: usize) -> bool {
    let mut queue = VecDeque::from([(a.clone(), b.clone(), 0)]);
    let mut seen = BTreeSet::new();
    while let Some((left, right, depth)) = queue.pop_front() {
        if left.nullable() && right.nullable() {
            return true;
        }
        if depth == max_depth || !seen.insert((left.clone(), right.clone(), depth)) {
            continue;
        }
        for &field in alphabet {
            queue.push_back((
                left.derivative(field, alphabet),
                right.derivative(field, alphabet),
                depth + 1,
            ));
        }
    }
    false
}

/// A leaf's path includes its descendants: a loan of `meta` conflicts with
/// an update of `meta.label`, but not with an update of `payload`.
fn subtree_path(fields: &[&'static str]) -> Path {
    fields
        .iter()
        .copied()
        .fold(Path::Epsilon, |path, field| {
            Path::then(path, Path::Field(field))
        })
        .then_any_descendant()
}

impl Path {
    fn then_any_descendant(self) -> Self {
        Self::then(self, Self::Star(Box::new(Self::Field("*"))))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoanMode {
    Shared,
    Exclusive,
}

#[derive(Debug)]
struct Loan {
    path: Vec<&'static str>,
    mode: LoanMode,
}

/// This is an ownership model, not Phoebe's layout or runtime representation.
#[derive(Debug)]
enum Field {
    Closed(Rc<Vec<u8>>),
    Right(Right),
    Product(BTreeMap<&'static str, Field>),
    Empty,
}

#[derive(Debug)]
struct Right {
    id: u64,
    closes: Rc<Cell<usize>>,
}

impl Drop for Right {
    fn drop(&mut self) {
        self.closes.set(self.closes.get() + 1);
    }
}

#[derive(Debug)]
struct FieldStore {
    root: Field,
    loans: Vec<Option<Loan>>,
    alphabet: Vec<&'static str>,
    depth: usize,
    costs: Costs,
}

impl FieldStore {
    fn new(root: Field, alphabet: &[&'static str], depth: usize) -> Self {
        Self {
            root,
            loans: Vec::new(),
            alphabet: alphabet.to_vec(),
            depth,
            costs: Costs::default(),
        }
    }

    fn conflicts(&self, path: &[&'static str], requested: LoanMode) -> bool {
        let target = subtree_path(path);
        self.loans.iter().flatten().any(|loan| {
            (requested == LoanMode::Exclusive || loan.mode == LoanMode::Exclusive)
                && overlap(
                    &target,
                    &subtree_path(&loan.path),
                    &self.alphabet,
                    self.depth,
                )
        })
    }

    fn begin(&mut self, path: &[&'static str], mode: LoanMode) -> Result<usize, &'static str> {
        if self.conflicts(path, mode) {
            return Err("loan overlaps active loan");
        }
        if matches!(self.get(path), Some(Field::Empty) | None) {
            return Err("absent field");
        }
        let id = self.loans.len();
        self.loans.push(Some(Loan {
            path: path.to_vec(),
            mode,
        }));
        Ok(id)
    }

    fn end(&mut self, id: usize) -> Result<(), &'static str> {
        self.loans
            .get_mut(id)
            .and_then(Option::take)
            .map(|_| ())
            .ok_or("loan already ended")
    }

    fn get(&self, path: &[&'static str]) -> Option<&Field> {
        let mut at = &self.root;
        for name in path {
            at = match at {
                Field::Product(fields) => fields.get(name)?,
                _ => return None,
            };
        }
        Some(at)
    }

    fn get_mut(&mut self, path: &[&'static str]) -> Option<&mut Field> {
        let mut at = &mut self.root;
        for name in path {
            at = match at {
                Field::Product(fields) => fields.get_mut(name)?,
                _ => return None,
            };
        }
        Some(at)
    }

    fn share_closed(&mut self, path: &[&'static str]) -> Result<Rc<Vec<u8>>, &'static str> {
        if self.conflicts(path, LoanMode::Shared) {
            return Err("exclusive loan blocks read");
        }
        let Field::Closed(value) = self.get(path).ok_or("absent field")? else {
            return Err("field is not closed data");
        };
        let copy = Rc::clone(value);
        self.costs.retains += 1;
        Ok(copy)
    }

    fn update_closed(&mut self, path: &[&'static str], byte: u8) -> Result<(), &'static str> {
        if self.conflicts(path, LoanMode::Exclusive) {
            return Err("loan blocks field update");
        }
        let Field::Closed(value) = self.get_mut(path).ok_or("absent field")? else {
            return Err("field is not closed data");
        };
        let copied = Rc::strong_count(value) > 1;
        let len = value.len();
        Rc::make_mut(value)[0] = byte;
        if copied {
            self.costs.byte_copies += len;
        }
        Ok(())
    }

    fn take_right(&mut self, path: &[&'static str]) -> Result<Right, &'static str> {
        if self.conflicts(path, LoanMode::Exclusive) {
            return Err("loan blocks right transfer");
        }
        let slot = self.get_mut(path).ok_or("absent field")?;
        match std::mem::replace(slot, Field::Empty) {
            Field::Right(right) => Ok(right),
            other => {
                *slot = other;
                Err("field is not a right")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn multi_call_inference_keeps_one_source_style_and_reports_fallback() {
        let mut costs = Costs::default();
        // read(current), revise(current), inspect(current): only the owning
        // middle call needs an additional owner.
        let actions = [
            infer(Kind::Closed, Use::Read, 2).unwrap(),
            infer(Kind::Closed, Use::Own, 1).unwrap(),
            infer(Kind::Closed, Use::Read, 0).unwrap(),
        ];
        assert_eq!(actions, [Action::Borrow, Action::Retain, Action::Borrow]);
        for action in actions {
            execute(action, &mut costs);
        }
        assert_eq!(
            costs,
            Costs {
                retains: 1,
                releases: 1,
                byte_copies: 0,
                result_allocations: 0
            }
        );
        assert_eq!(infer(Kind::Closed, Use::Own, 0), Ok(Action::Move));
        assert!(infer(Kind::Capability, Use::Own, 1).is_err());
    }

    #[test]
    fn retain_fallback_has_real_shared_storage_and_update_copy_cost() {
        let current = Rc::new(vec![7_u8; 4096]);
        let next = Rc::clone(&current); // inferred Retain at an owning call
        assert_eq!(Rc::strong_count(&current), 2);
        let mut next = next;
        Rc::make_mut(&mut next)[0] = 8; // independent value semantics
        assert_eq!(current[0], 7);
        assert_eq!(next[0], 8);
        assert_ne!(Rc::as_ptr(&current), Rc::as_ptr(&next));
        // The retain was O(1), but the later write copied all 4096 bytes.
    }

    #[test]
    fn destination_is_write_once_and_unfinished_result_is_inaccessible() {
        let mut dest = Destination::new(2);
        dest.fill(0, 40).unwrap();
        assert!(dest.fill(0, 99).is_err());
        let mut dest = dest
            .finish()
            .err()
            .expect("unfinished result must not publish");
        dest.fill(1, 2).unwrap();
        assert_eq!(dest.finish().unwrap(), vec![40, 2]);
    }

    #[test]
    fn failure_and_cancellation_drop_only_initialized_fields() {
        #[derive(Debug)]
        struct Owned(Rc<Cell<usize>>);
        impl Drop for Owned {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let mut dest = Box::new(Destination::new(2)); // stable across a simulated suspension
        dest.fill(0, Owned(drops.clone())).unwrap();
        drop(dest); // failure or cancellation before field 1 is initialized
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn result_construction_keeps_destination_stable_across_suspend_and_error() {
        fn build(fail: bool) -> Result<Vec<i64>, &'static str> {
            let mut dest = Box::new(Destination::new(2));
            let address_before = (&*dest as *const Destination<i64>) as usize;
            dest.fill(0, 40).unwrap();
            // A real Phoebe task would park here; the root must stay in its
            // retained frame until this call completes or unwinds.
            let address_after = (&*dest as *const Destination<i64>) as usize;
            assert_eq!(address_before, address_after);
            if fail {
                return Err("construction failed");
            }
            dest.fill(1, 2).unwrap();
            dest.finish().map_err(|_| "incomplete result")
        }
        assert_eq!(build(false), Ok(vec![40, 2]));
        assert_eq!(build(true), Err("construction failed"));
    }

    #[test]
    fn capability_bearing_aggregate_cannot_use_closed_data_fallback() {
        assert_eq!(infer(Kind::Capability, Use::Own, 0), Ok(Action::Move));
        assert_eq!(
            infer(Kind::Capability, Use::Own, 1),
            Err("capability has another live use")
        );
    }

    #[test]
    fn regex_paths_separate_fields_and_conservatively_join_call_results() {
        let fields = ["left", "right"];
        let left = Path::Field("left");
        let right = Path::Field("right");
        assert!(!overlap(&left, &right, &fields, 2));
        assert!(overlap(&left, &left, &fields, 2));
        let any_subfield = Path::Star(Box::new(Path::Field("*")));
        assert!(overlap(&any_subfield, &right, &fields, 2));
        let left_child = Path::then(left, Path::Field("left"));
        assert!(!overlap(&left_child, &right, &fields, 2));
    }

    fn record_with_right() -> (FieldStore, Rc<Cell<usize>>) {
        let closes = Rc::new(Cell::new(0));
        (
            FieldStore::new(
                Field::Product(BTreeMap::from([
                    ("payload", Field::Closed(Rc::new(vec![1; 32]))),
                    (
                        "sender",
                        Field::Right(Right {
                            id: 7,
                            closes: closes.clone(),
                        }),
                    ),
                    (
                        "meta",
                        Field::Product(BTreeMap::from([
                            ("label", Field::Closed(Rc::new(vec![2; 8]))),
                            ("version", Field::Closed(Rc::new(vec![3; 8]))),
                        ])),
                    ),
                ])),
                &["payload", "sender", "meta", "label", "version"],
                4,
            ),
            closes,
        )
    }

    #[test]
    fn closed_sibling_can_be_shared_while_right_moves_out() {
        let (mut store, closes) = record_with_right();
        let payload = store.share_closed(&["payload"]).unwrap();
        let right = store.take_right(&["sender"]).unwrap();
        assert_eq!(right.id, 7);
        assert!(store.take_right(&["sender"]).is_err());
        assert_eq!(store.get(&["sender"]).is_some(), true); // known but empty
        assert_eq!(payload[0], 1);
        store.update_closed(&["payload"], 9).unwrap();
        assert_eq!(payload[0], 1);
        assert_eq!(store.costs.retains, 1);
        assert_eq!(store.costs.byte_copies, 32);
        assert_eq!(closes.get(), 0);
        drop(right);
        assert_eq!(closes.get(), 1);
    }

    #[test]
    fn nested_field_loans_allow_siblings_and_block_ancestors() {
        let (mut store, _) = record_with_right();
        let label = store.begin(&["meta", "label"], LoanMode::Shared).unwrap();
        let version = store
            .begin(&["meta", "version"], LoanMode::Exclusive)
            .unwrap();
        store.update_closed(&["payload"], 8).unwrap();
        assert_eq!(
            store.update_closed(&["meta", "label"], 8),
            Err("loan blocks field update")
        );
        assert_eq!(
            store.begin(&["meta"], LoanMode::Exclusive),
            Err("loan overlaps active loan")
        );
        assert_eq!(
            store.begin(&["meta", "version"], LoanMode::Shared),
            Err("loan overlaps active loan")
        );
        store.end(version).unwrap();
        store.update_closed(&["meta", "version"], 8).unwrap();
        store.end(label).unwrap();
        store.update_closed(&["meta", "label"], 8).unwrap();
        assert_eq!(store.end(label), Err("loan already ended"));
    }

    #[test]
    fn a_parent_loan_blocks_moving_a_nested_right() {
        let (mut store, _) = record_with_right();
        let parent = store.begin(&[], LoanMode::Shared).unwrap();
        assert!(matches!(
            store.take_right(&["sender"]),
            Err("loan blocks right transfer")
        ));
        store.end(parent).unwrap();
        assert_eq!(store.take_right(&["sender"]).unwrap().id, 7);
    }

    #[test]
    fn fieldwise_destination_owns_moved_right_and_cleans_on_failure() {
        let (mut store, closes) = record_with_right();
        let mut result = Destination::new(3);
        result
            .fill(0, Field::Closed(store.share_closed(&["payload"]).unwrap()))
            .unwrap();
        result
            .fill(1, Field::Right(store.take_right(&["sender"]).unwrap()))
            .unwrap();
        assert!(result.finish().is_err()); // a third field was never initialized
        assert_eq!(closes.get(), 1); // result failure closed exactly its right
        drop(store);
        assert_eq!(closes.get(), 1); // source no longer owns that right
    }
}
