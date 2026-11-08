//! Native entry points for a statically balanced lexical task scope.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::Poll;

use super::{Context, NativeCallContext};

fn fault(bridge: *mut NativeCallContext) -> i64 {
    if !bridge.is_null() {
        // SAFETY: the caller's live bridge contains a writable fault flag.
        unsafe { *(*bridge).fault = true };
    }
    255
}

/// Status 0 enters a fresh scope. Status 255 faults the task before a scope
/// is published. Generated code must pair each successful enter with a join.
///
/// # Safety
///
/// `bridge` must borrow a live task context uniquely for this call.
#[unsafe(export_name = "phoebe_rt_v1_scope_enter")]
pub unsafe extern "C" fn phoebe_scope_enter(bridge: *mut NativeCallContext) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if bridge.is_null() {
            return Err(());
        }
        let context = unsafe { &mut *((*bridge).context as *mut Context<'_>) };
        context.scope_enter().map_err(|_| ())?;
        Ok(0)
    }));
    match outcome {
        Ok(Ok(code)) => code,
        _ => fault(bridge),
    }
}

/// Status 0 closes the innermost scope, 1 parks until its children finish,
/// and 255 faults on an invalid or unbalanced scope. A pending call is retried
/// after the scheduler wakes the task.
///
/// # Safety
///
/// `bridge` must satisfy `phoebe_scope_enter`'s bridge contract.
#[unsafe(export_name = "phoebe_rt_v1_scope_join")]
pub unsafe extern "C" fn phoebe_scope_join(bridge: *mut NativeCallContext) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if bridge.is_null() {
            return Err(());
        }
        let context = unsafe { &mut *((*bridge).context as *mut Context<'_>) };
        match context.scope_join_current().map_err(|_| ())? {
            Poll::Ready(()) => Ok(0),
            Poll::Pending => Ok(1),
        }
    }));
    match outcome {
        Ok(Ok(code)) => code,
        _ => fault(bridge),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NativePollEntry, NativePollTask, Scheduler, phoebe_spawn_poll};

    extern "C" fn child(_bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        unsafe { *frame.add(1) = 7 };
        0
    }

    extern "C" fn parent(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 2) };
        if words[0] == 0 {
            if unsafe { phoebe_scope_enter(bridge) } != 0 {
                return 255;
            }
            if unsafe {
                phoebe_spawn_poll(
                    bridge,
                    child as NativePollEntry,
                    2,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                )
            } != 0
            {
                return 255;
            }
            words[0] = 1;
        }
        match unsafe { phoebe_scope_join(bridge) } {
            1 => 1,
            0 => {
                words[1] = 42;
                0
            }
            _ => 255,
        }
    }

    #[test]
    fn native_scope_join_parks_and_resumes_after_spawned_child() {
        let root = unsafe { NativePollTask::new(parent, 2) }.unwrap();
        let mut scheduler = Scheduler::new(root, 2, 0, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert_eq!(scheduler.stats().parks, 1);
        assert_eq!(scheduler.stats().wakes, 1);
        scheduler.verify().unwrap();
    }

    extern "C" fn unmatched_join(bridge: *mut NativeCallContext, _frame: *mut i64) -> i64 {
        unsafe { phoebe_scope_join(bridge) }
    }

    #[test]
    fn native_join_without_scope_faults() {
        let root = unsafe { NativePollTask::new(unmatched_join, 2) }.unwrap();
        let mut scheduler = Scheduler::new(root, 1, 0, 0).unwrap();
        assert!(scheduler.run().is_err());
        scheduler.verify().unwrap();
    }
}
