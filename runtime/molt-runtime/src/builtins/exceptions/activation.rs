//! The exception-handler state a stateful activation owns between resumes.
//!
//! CPython gives each generator and coroutine its own `_PyErr_StackItem`
//! (`gi_exc_state`): resuming links it onto the thread's `exc_info` chain,
//! suspending unlinks it, and `sys.exception()` walks the chain to the first
//! item that holds an exception (`_PyErr_GetTopmostException`). Molt keeps the
//! thread's handler levels in `EXCEPTION_STACK` and the exception each level
//! handles in `ACTIVE_EXCEPTION_STACK`. A resumed activation moves its own
//! levels in and moves them back out when it stops; the innermost exception
//! its caller handles stays reachable below them on
//! `ACTIVE_EXCEPTION_FALLBACK`, which `exception_context_active_bits` walks
//! after the active levels.
//!
//! Each activation owns what it keeps in one word of its sidecar
//! (`aux_header::object_frame_exceptions`), and [`ActivationExceptionScope`]
//! is the one resume protocol of every site that runs an activation body.

use super::{
    ACTIVE_EXCEPTION_STACK, EXCEPTION_STACK, exception_context_align_depth,
    exception_context_fallback_pop, exception_context_fallback_push, exception_stack_baseline_get,
    exception_stack_baseline_set, exception_stack_depth, exception_stack_set_depth,
};
use crate::object::heap_lifecycle::DetachedEdgeSink;
use crate::{MoltObject, PyToken, dec_ref_bits, obj_from_bits};

/// How one kind of activation runs on the thread's exception state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivationKind {
    /// A generator body runs on its caller's handler levels, resized to its
    /// own depth, and keeps its own `with` context-manager stack.
    Generator,
    /// A task owns its handler levels. With `own_baseline` it also owns the
    /// handler baseline its body pops down to.
    Task { own_baseline: bool },
}

impl ActivationKind {
    /// The handler depth a fresh activation of this kind starts at.
    const fn initial_depth(self) -> usize {
        match self {
            Self::Generator => 1,
            Self::Task { .. } => 0,
        }
    }

    const fn owns_handlers(self) -> bool {
        matches!(self, Self::Task { .. })
    }

    const fn owns_context(self) -> bool {
        matches!(self, Self::Generator)
    }

    const fn owns_baseline(self) -> bool {
        matches!(self, Self::Task { own_baseline: true })
    }
}

/// What a suspended activation keeps. `active` and `context` own their
/// references; GC traversal and detachment reach them through the sidecar.
#[derive(Default)]
struct SavedExceptionState {
    handlers: Vec<usize>,
    active: Vec<u64>,
    context: Vec<u64>,
    depth: usize,
    baseline: Option<usize>,
}

impl SavedExceptionState {
    fn holds_buffers(&self) -> bool {
        self.handlers.capacity() != 0 || self.active.capacity() != 0 || self.context.capacity() != 0
    }
}

/// One sidecar word: zero for a fresh activation, a boxed
/// `SavedExceptionState` (an aligned address), or the scalars of a state with
/// no buffers, tagged by the low bit so a suspension that holds no handler
/// level allocates nothing.
mod word {
    use super::SavedExceptionState;

    const INLINE: u64 = 1;
    const FIELD_BITS: u32 = 31;
    const FIELD_MASK: u64 = (1 << FIELD_BITS) - 1;
    const HAS_BASELINE: u64 = 1 << 32;

    pub(super) enum Decoded {
        Fresh,
        Inline {
            depth: usize,
            baseline: Option<usize>,
        },
        Boxed(Box<SavedExceptionState>),
    }

    pub(super) fn decode(word: u64) -> Decoded {
        if word == 0 {
            Decoded::Fresh
        } else if word & INLINE != 0 {
            Decoded::Inline {
                depth: ((word >> 1) & FIELD_MASK) as usize,
                baseline: (word & HAS_BASELINE != 0).then_some((word >> 33) as usize),
            }
        } else {
            let address = usize::try_from(word).expect("exception state address fits");
            // SAFETY: a non-inline word is the address `encode` published from
            // `Box::into_raw`, taken exactly once.
            Decoded::Boxed(unsafe {
                Box::from_raw(std::ptr::with_exposed_provenance_mut::<SavedExceptionState>(address))
            })
        }
    }

    /// Encode `state`, reusing `spare` (an emptied box of this activation)
    /// when the state holds buffers.
    pub(super) fn encode(
        state: SavedExceptionState,
        spare: Option<Box<SavedExceptionState>>,
    ) -> u64 {
        let fits = (state.depth as u64) <= FIELD_MASK
            && state
                .baseline
                .is_none_or(|baseline| (baseline as u64) <= FIELD_MASK);
        if !state.holds_buffers() && fits {
            let mut word = INLINE | ((state.depth as u64) << 1);
            if let Some(baseline) = state.baseline {
                word |= HAS_BASELINE | ((baseline as u64) << 33);
            }
            return word;
        }
        let boxed = match spare {
            Some(mut spare) => {
                *spare = state;
                spare
            }
            None => Box::new(state),
        };
        Box::into_raw(boxed).expose_provenance() as u64
    }
}

fn load_state(
    owner: *mut u8,
    kind: ActivationKind,
) -> (SavedExceptionState, Option<Box<SavedExceptionState>>) {
    let word = unsafe { crate::object::aux_header::object_take_frame_exceptions(owner) };
    match word::decode(word) {
        word::Decoded::Fresh => (
            SavedExceptionState {
                depth: kind.initial_depth(),
                ..SavedExceptionState::default()
            },
            None,
        ),
        word::Decoded::Inline { depth, baseline } => (
            SavedExceptionState {
                depth,
                baseline,
                ..SavedExceptionState::default()
            },
            None,
        ),
        word::Decoded::Boxed(mut boxed) => {
            let state = std::mem::take(&mut *boxed);
            (state, Some(boxed))
        }
    }
}

/// The innermost exception a handler level of `levels` handles, or None: the
/// first item `_PyErr_GetTopmostException` would find in this stretch of the
/// chain.
fn innermost_handled(levels: &[u64]) -> u64 {
    levels
        .iter()
        .rev()
        .copied()
        .find(|&bits| !obj_from_bits(bits).is_none())
        .unwrap_or_else(|| MoltObject::none().bits())
}

/// The caller's exception state while an activation body runs. Enter before
/// the body, exit after it on every path; the scope holds no GIL token, so a
/// caller that releases the GIL while the body is suspended (`block_on`)
/// carries it across.
#[must_use = "an activation's exception state must be restored by `exit`"]
pub(crate) struct ActivationExceptionScope {
    owner: *mut u8,
    kind: ActivationKind,
    spare: Option<Box<SavedExceptionState>>,
    caller_depth: usize,
    caller_baseline: usize,
    caller_handlers: Vec<usize>,
    caller_active: Vec<u64>,
    caller_context: Vec<u64>,
    exited: bool,
}

impl ActivationExceptionScope {
    /// Move `owner`'s saved state onto the thread, keeping the caller's.
    pub(crate) fn enter(py: &PyToken<'_>, owner: *mut u8, kind: ActivationKind) -> Self {
        crate::gil_assert();
        let (state, spare) = load_state(owner, kind);
        let caller_depth = exception_stack_depth();
        let caller_baseline = exception_stack_baseline_get();
        let caller_active = ACTIVE_EXCEPTION_STACK.with(|stack| stack.replace(state.active));
        exception_context_fallback_push(innermost_handled(&caller_active));
        let caller_handlers = if kind.owns_handlers() {
            EXCEPTION_STACK.with(|stack| stack.replace(state.handlers))
        } else {
            Vec::new()
        };
        let caller_context = if kind.owns_context() {
            crate::builtins::context::context_stack_replace(state.context)
        } else {
            Vec::new()
        };
        if kind.owns_baseline()
            && let Some(baseline) = state.baseline
        {
            exception_stack_baseline_set(baseline);
        }
        exception_stack_set_depth(py, state.depth);
        Self {
            owner,
            kind,
            spare,
            caller_depth,
            caller_baseline,
            caller_handlers,
            caller_active,
            caller_context,
            exited: false,
        }
    }

    /// Move the body's state back into its owner and restore the caller's.
    pub(crate) fn exit(mut self, py: &PyToken<'_>) {
        crate::gil_assert();
        self.exited = true;
        let kind = self.kind;
        let depth = exception_stack_depth();
        exception_context_align_depth(py, depth);
        let baseline = if kind.owns_baseline() {
            let baseline = exception_stack_baseline_get();
            exception_stack_baseline_set(self.caller_baseline);
            Some(baseline)
        } else {
            None
        };
        let handlers = if kind.owns_handlers() {
            let caller = std::mem::take(&mut self.caller_handlers);
            EXCEPTION_STACK.with(|stack| stack.replace(caller))
        } else {
            Vec::new()
        };
        let caller_active = std::mem::take(&mut self.caller_active);
        let active = ACTIVE_EXCEPTION_STACK.with(|stack| stack.replace(caller_active));
        let context = if kind.owns_context() {
            crate::builtins::context::context_stack_replace(std::mem::take(
                &mut self.caller_context,
            ))
        } else {
            Vec::new()
        };
        exception_stack_set_depth(py, self.caller_depth);
        exception_context_fallback_pop(py);
        let state = SavedExceptionState {
            handlers,
            active,
            context,
            depth,
            baseline,
        };
        let word = word::encode(state, self.spare.take());
        unsafe { crate::object::aux_header::object_set_frame_exceptions(self.owner, word) };
    }
}

impl Drop for ActivationExceptionScope {
    fn drop(&mut self) {
        // Every resume site exits explicitly; a scope dropped without `exit`
        // would leave the caller's exception state on the activation.
        debug_assert!(self.exited || std::thread::panicking());
    }
}

/// Visit every reference a suspended activation keeps for its handlers.
pub(crate) fn activation_exception_state_visit(owner: *mut u8, mut visit: impl FnMut(u64)) {
    let word = crate::object::aux_header::object_frame_exceptions(owner);
    if word == 0 || word & 1 != 0 {
        return;
    }
    let address = usize::try_from(word).expect("exception state address fits");
    // SAFETY: a boxed word stays owned by the activation; the caller holds
    // the GIL, so no resume moves it while the traversal reads it.
    let state = unsafe { &*std::ptr::with_exposed_provenance::<SavedExceptionState>(address) };
    for &bits in state.active.iter().chain(&state.context) {
        visit(bits);
    }
}

/// Detach the state into `sink` (cycle clearing and terminal death).
pub(crate) fn activation_exception_state_detach(owner: *mut u8, sink: &mut DetachedEdgeSink) {
    let word = unsafe { crate::object::aux_header::object_take_frame_exceptions(owner) };
    if let word::Decoded::Boxed(state) = word::decode(word) {
        for bits in state.active.into_iter().chain(state.context) {
            sink.detach_if_heap(bits);
        }
    }
}

/// Release the state of an activation that finished.
pub(crate) fn activation_exception_state_drop(py: &PyToken<'_>, owner: *mut u8) {
    let word = unsafe { crate::object::aux_header::object_take_frame_exceptions(owner) };
    if let word::Decoded::Boxed(state) = word::decode(word) {
        for bits in state.active.into_iter().chain(state.context) {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(py, bits);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::exceptions::{
        exception_context_active_bits, exception_context_set, exception_stack_pop,
        exception_stack_push, task_exception_detach_owned_edges,
    };
    use crate::{alloc_exception, dec_ref_bits, inc_ref_bits};

    fn activation(kind: u64, size: usize) -> (u64, *mut u8) {
        let bits = crate::molt_task_new(0, size as u64, kind);
        let ptr = obj_from_bits(bits).as_ptr().expect("activation allocation");
        (bits, ptr)
    }

    fn generator() -> (u64, *mut u8) {
        activation(crate::TASK_KIND_GENERATOR, crate::GEN_CONTROL_SIZE)
    }

    fn coroutine() -> (u64, *mut u8) {
        activation(crate::TASK_KIND_COROUTINE, 0)
    }

    fn exception(py: &PyToken<'_>, name: crate::BuiltinExc) -> u64 {
        MoltObject::from_ptr(alloc_exception(py, name, name.name())).bits()
    }

    fn refcount(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().unwrap();
        unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    /// CPython links a resumed frame's exception state onto the thread's
    /// chain, and `sys.exception()` finds the innermost item that holds an
    /// exception. A caller whose innermost handler level holds none still
    /// exposes the one below it to the body it resumes.
    #[test]
    fn every_kind_sees_the_callers_innermost_handled_exception() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handled = exception(py, crate::BuiltinExc::LookupError);
            exception_stack_push();
            exception_context_set(py, handled);
            exception_stack_push();
            assert_eq!(exception_context_active_bits(), Some(handled));
            for (bits, ptr, kind) in [
                {
                    let (bits, ptr) = generator();
                    (bits, ptr, ActivationKind::Generator)
                },
                {
                    let (bits, ptr) = coroutine();
                    (bits, ptr, ActivationKind::Task { own_baseline: true })
                },
                {
                    let (bits, ptr) = coroutine();
                    (
                        bits,
                        ptr,
                        ActivationKind::Task {
                            own_baseline: false,
                        },
                    )
                },
            ] {
                let scope = ActivationExceptionScope::enter(py, ptr, kind);
                assert_eq!(exception_context_active_bits(), Some(handled), "{kind:?}");
                scope.exit(py);
                activation_exception_state_drop(py, ptr);
                dec_ref_bits(py, bits);
            }
            assert_eq!(exception_context_active_bits(), Some(handled));
            exception_stack_pop(py);
            exception_stack_pop(py);
            assert_eq!(exception_context_active_bits(), None);
            dec_ref_bits(py, handled);
        });
    }

    /// A body's own handled exception leaves with it at suspension, comes
    /// back at the next resume, and is owned by the activation in between.
    #[test]
    fn suspended_handler_state_round_trips_through_its_one_owner() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let (bits, ptr) = generator();
            let own = exception(py, crate::BuiltinExc::KeyError);
            let baseline = refcount(own);
            let scope = ActivationExceptionScope::enter(py, ptr, ActivationKind::Generator);
            exception_stack_push();
            exception_context_set(py, own);
            scope.exit(py);
            assert_eq!(
                exception_context_active_bits(),
                None,
                "the caller sees nothing"
            );
            assert_eq!(
                refcount(own),
                baseline + 1,
                "the activation owns one reference"
            );
            let mut visited = Vec::new();
            activation_exception_state_visit(ptr, |edge| visited.push(edge));
            assert!(
                visited.contains(&own),
                "GC traversal reaches the saved exception"
            );
            let scope = ActivationExceptionScope::enter(py, ptr, ActivationKind::Generator);
            assert_eq!(exception_context_active_bits(), Some(own));
            exception_stack_pop(py);
            scope.exit(py);
            activation_exception_state_drop(py, ptr);
            assert_eq!(crate::object::aux_header::object_frame_exceptions(ptr), 0);
            assert_eq!(refcount(own), baseline);
            dec_ref_bits(py, own);
            dec_ref_bits(py, bits);
        });
    }

    /// A suspension that holds no handler level keeps its scalars in the
    /// sidecar word itself.
    #[test]
    fn handlerless_suspension_allocates_no_state() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let (bits, ptr) = coroutine();
            let scope = ActivationExceptionScope::enter(
                py,
                ptr,
                ActivationKind::Task { own_baseline: true },
            );
            scope.exit(py);
            let word = crate::object::aux_header::object_frame_exceptions(ptr);
            assert_eq!(word & 1, 1, "inline scalars, not a boxed state: {word:#x}");
            activation_exception_state_drop(py, ptr);
            dec_ref_bits(py, bits);
        });
    }

    /// An activation that dies suspended releases what it kept: terminal
    /// death detaches the saved state through the task projection, for a
    /// generator and for a coroutine.
    #[test]
    fn death_releases_the_saved_state() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (bits, ptr, kind) in [
                {
                    let (bits, ptr) = generator();
                    (bits, ptr, ActivationKind::Generator)
                },
                {
                    let (bits, ptr) = coroutine();
                    (bits, ptr, ActivationKind::Task { own_baseline: true })
                },
            ] {
                let own = exception(py, crate::BuiltinExc::KeyError);
                let baseline = refcount(own);
                let scope = ActivationExceptionScope::enter(py, ptr, kind);
                exception_stack_push();
                exception_context_set(py, own);
                scope.exit(py);
                assert_eq!(refcount(own), baseline + 1, "{kind:?} owns one reference");
                dec_ref_bits(py, bits);
                assert_eq!(refcount(own), baseline, "{kind:?} death releases it");
                dec_ref_bits(py, own);
            }
            assert!(!crate::exception_pending(py));
        });
    }

    /// Cycle clearing and terminal death move the saved references into the
    /// detachment sink and publish the owner empty first.
    #[test]
    fn detachment_moves_saved_references_into_the_sink() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let (bits, ptr) = coroutine();
            let own = exception(py, crate::BuiltinExc::ValueError);
            inc_ref_bits(py, own);
            let scope = ActivationExceptionScope::enter(
                py,
                ptr,
                ActivationKind::Task { own_baseline: true },
            );
            exception_stack_push();
            exception_context_set(py, own);
            scope.exit(py);
            let baseline = refcount(own);
            let mut sink =
                crate::object::heap_lifecycle::DetachedEdgeSink::try_with_capacities(2, 0)
                    .expect("detach sink");
            task_exception_detach_owned_edges(py, ptr, &mut sink);
            assert_eq!(crate::object::aux_header::object_frame_exceptions(ptr), 0);
            assert_eq!(
                refcount(own),
                baseline,
                "detachment moves, it does not release"
            );
            sink.release_all(py);
            assert_eq!(refcount(own), baseline - 1);
            dec_ref_bits(py, own);
            dec_ref_bits(py, own);
            dec_ref_bits(py, bits);
        });
    }
}
