//! Lexical task groups inside the scheduler's outer lifetime scope.

use std::task::Poll;

use super::{Context, Scheduler, TaskId, Wait};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScopeId {
    slot: usize,
    generation: u64,
}

impl ScopeId {
    pub(super) const ROOT: Self = Self {
        slot: 0,
        generation: 0,
    };
}

pub(super) struct ScopeRecord {
    pub parent: Option<ScopeId>,
    pub owner: TaskId,
    pub generation: u64,
    pub active: bool,
    pub next_free: Option<usize>,
    /// Live tasks spawned in this scope or a nested scope. Excludes its owner.
    pub live: usize,
}

impl ScopeRecord {
    pub(super) fn root() -> Self {
        Self {
            parent: None,
            owner: TaskId(0),
            generation: 0,
            active: true,
            next_free: None,
            live: 0,
        }
    }
}

impl Scheduler {
    pub(super) fn scope_record(&self, scope: ScopeId) -> Result<&ScopeRecord, String> {
        self.scopes
            .get(scope.slot)
            .filter(|record| record.active && record.generation == scope.generation)
            .ok_or_else(|| "invalid or closed task scope".into())
    }

    pub(super) fn add_task_to_scopes(&mut self, task: TaskId) {
        let mut current = Some(self.tasks[task.0].home_scope);
        while let Some(scope) = current {
            let record = &mut self.scopes[scope.slot];
            debug_assert!(record.active && record.generation == scope.generation);
            record.live += 1;
            current = record.parent;
        }
    }

    pub(super) fn release_task_from_scopes(&mut self, task: TaskId) {
        if task == TaskId(0) {
            return;
        }
        let mut current = Some(self.tasks[task.0].home_scope);
        while let Some(scope) = current {
            let record = &mut self.scopes[scope.slot];
            debug_assert!(record.active && record.generation == scope.generation);
            record.live -= 1;
            current = record.parent;
        }
    }

    pub(super) fn verify_scopes(&self) -> Result<(), String> {
        if self.scopes.len() - 1 > self.max_scopes {
            return Err("task scope slot budget exceeded".into());
        }
        let mut free = vec![false; self.scopes.len()];
        let mut cursor = self.free_scope_head;
        while let Some(slot) = cursor {
            let record = self
                .scopes
                .get(slot)
                .ok_or("invalid free task scope slot")?;
            if slot == 0 || record.active || free[slot] {
                return Err("task scope free list is invalid".into());
            }
            free[slot] = true;
            cursor = record.next_free;
        }
        let mut expected = vec![0usize; self.scopes.len()];
        for (index, task) in self.tasks.iter().enumerate() {
            if matches!(
                task.state,
                super::TaskState::Done(_) | super::TaskState::Failed(_)
            ) {
                continue;
            }
            self.scope_record(task.current_scope)?;
            if index == 0 {
                continue;
            }
            let mut current = Some(task.home_scope);
            let mut depth = 0;
            while let Some(scope) = current {
                let record = self.scope_record(scope)?;
                expected[scope.slot] += 1;
                current = record.parent;
                depth += 1;
                if depth > self.scopes.len() {
                    return Err("task scope parent cycle".into());
                }
            }
        }
        for (index, record) in self.scopes.iter().enumerate() {
            if record.active == free[index] {
                return Err(format!("task scope {index} has inconsistent free state"));
            }
            if record.live != expected[index] {
                return Err(format!("task scope {index} has incorrect live count"));
            }
            if record.active && index != 0 {
                let parent = record.parent.ok_or("nested task scope lacks a parent")?;
                self.scope_record(parent)?;
                if record.owner.0 >= self.tasks.len() {
                    return Err("task scope has invalid owner".into());
                }
            }
        }
        Ok(())
    }
}

impl Context<'_> {
    /// Start a nested task scope. Spawns by this task join it until `scope_join`.
    pub fn scope_enter(&mut self) -> Result<ScopeId, String> {
        if self.wait.is_some() {
            return Err("task already registered a wait in this poll".into());
        }
        let parent = self.scheduler.tasks[self.task.0].current_scope;
        self.scheduler.scope_record(parent)?;
        let scope = if let Some(slot) = self.scheduler.free_scope_head {
            let generation = self.scheduler.scopes[slot]
                .generation
                .checked_add(1)
                .ok_or("task scope generation exhausted")?;
            self.scheduler.free_scope_head = self.scheduler.scopes[slot].next_free;
            self.scheduler.scopes[slot] = ScopeRecord {
                parent: Some(parent),
                owner: self.task,
                generation,
                active: true,
                next_free: None,
                live: 0,
            };
            ScopeId { slot, generation }
        } else {
            if self.scheduler.scopes.len() - 1 >= self.scheduler.max_scopes {
                return Err("task scope capacity exhausted".into());
            }
            self.scheduler
                .scopes
                .try_reserve(1)
                .map_err(|error| error.to_string())?;
            let slot = self.scheduler.scopes.len();
            self.scheduler.scopes.push(ScopeRecord {
                parent: Some(parent),
                owner: self.task,
                generation: 0,
                active: true,
                next_free: None,
                live: 0,
            });
            ScopeId {
                slot,
                generation: 0,
            }
        };
        self.scheduler.tasks[self.task.0].current_scope = scope;
        Ok(scope)
    }

    /// Wait for tasks spawned inside this scope, including nested descendants.
    /// On Ready, close the scope and restore its owner's enclosing scope.
    pub fn scope_join(&mut self, scope: ScopeId) -> Result<Poll<()>, String> {
        if self.wait.is_some() {
            return Err("task already registered a wait in this poll".into());
        }
        let record = self.scheduler.scope_record(scope)?;
        if scope == ScopeId::ROOT
            || record.owner != self.task
            || self.scheduler.tasks[self.task.0].current_scope != scope
        {
            return Err("task may join only its innermost owned scope".into());
        }
        if record.live != 0 {
            self.wait = Some(Wait::ScopeJoin(scope));
            return Ok(Poll::Pending);
        }
        let parent = record.parent.expect("nested scope has a parent");
        self.scheduler.tasks[self.task.0].current_scope = parent;
        self.scheduler.scopes[scope.slot].active = false;
        self.scheduler.scopes[scope.slot].next_free = self.scheduler.free_scope_head;
        self.scheduler.free_scope_head = Some(scope.slot);
        Ok(Poll::Ready(()))
    }

    /// Join the innermost scope; used by code whose scope nesting is checked
    /// statically and therefore does not carry a runtime token in its frame.
    pub fn scope_join_current(&mut self) -> Result<Poll<()>, String> {
        let current = self.scheduler.tasks[self.task.0].current_scope;
        self.scope_join(current)
    }
}

#[cfg(test)]
mod tests {
    use super::super::Task;
    use super::*;

    struct RepeatedEmptyScopes;

    impl Task for RepeatedEmptyScopes {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            for _ in 0..1_000 {
                let scope = context.scope_enter()?;
                assert_eq!(context.scope_join(scope)?, Poll::Ready(()));
            }
            Ok(Poll::Ready(42))
        }
    }

    #[test]
    fn sequential_scopes_reuse_one_slot() {
        let mut scheduler = Scheduler::with_limits(
            RepeatedEmptyScopes,
            super::super::ScopeLimits {
                tasks: 1,
                scopes: 1,
                ..super::super::ScopeLimits::default()
            },
        )
        .unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert_eq!(scheduler.scopes.len(), 2);
        scheduler.verify().unwrap();
    }

    struct FailedNestedEntry;

    impl Task for FailedNestedEntry {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            let outer = context.scope_enter()?;
            assert_eq!(
                context.scope_enter().unwrap_err(),
                "task scope capacity exhausted"
            );
            assert_eq!(context.scheduler.tasks[context.task.0].current_scope, outer);
            assert_eq!(context.scope_join(outer)?, Poll::Ready(()));
            Ok(Poll::Ready(42))
        }
    }

    #[test]
    fn failed_scope_entry_does_not_publish_a_scope() {
        let mut scheduler = Scheduler::with_limits(
            FailedNestedEntry,
            super::super::ScopeLimits {
                tasks: 1,
                scopes: 1,
                ..super::super::ScopeLimits::default()
            },
        )
        .unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert_eq!(scheduler.scopes.len(), 2);
        scheduler.verify().unwrap();
    }
}
