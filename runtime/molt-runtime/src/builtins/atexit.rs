use crate::object::ops_sys::runtime_target_minor;
use crate::state::runtime_state::{
    AtexitCallbackEntry, AtexitCallbackKind, ExitRegistry, WeakFinalizerPrepared,
    WeakrefRunnerState,
};
use crate::{
    MoltObject, PyToken, TYPE_ID_BOUND_METHOD, TYPE_ID_FUNCTION, bound_method_func_bits,
    bound_method_self_bits, clear_exception, dec_ref_bits, exception_pending,
    function_closure_bits, function_fn_ptr, inc_ref_bits, int_bits_from_i64, is_truthy,
    molt_call_bind, molt_callargs_expand_kwstar, molt_callargs_expand_star, molt_callargs_new,
    molt_is_callable, obj_from_bits, object_type_id, raise_exception, runtime_state,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExitRegistryError {
    OutOfMemory,
    RegistrationIdExhausted,
    CapacityExhausted,
    FinalizerGenerationExhausted,
    InvalidPreparedCapacity,
}

const MIN_EXIT_REGISTRY_CAPACITY: usize = 8;

fn geometric_capacity(current: usize, required: usize) -> Result<usize, ExitRegistryError> {
    if required <= current {
        return Ok(current);
    }
    let mut capacity = current.max(MIN_EXIT_REGISTRY_CAPACITY);
    while capacity < required {
        capacity = capacity
            .checked_mul(2)
            .ok_or(ExitRegistryError::CapacityExhausted)?;
    }
    Ok(capacity)
}

fn growth_capacity_for_insert(
    len: usize,
    capacity: usize,
) -> Result<Option<usize>, ExitRegistryError> {
    if len < capacity {
        return Ok(None);
    }
    let required = len
        .checked_add(1)
        .ok_or(ExitRegistryError::CapacityExhausted)?;
    geometric_capacity(capacity, required).map(Some)
}

fn try_prepare_vec<T>(required_capacity: usize) -> Result<Vec<T>, ExitRegistryError> {
    let mut prepared = Vec::new();
    prepared
        .try_reserve_exact(required_capacity)
        .map_err(|_| ExitRegistryError::OutOfMemory)?;
    Ok(prepared)
}

/// Install an externally allocated backing store while the registry lock is
/// held. The returned old allocation is empty and must be dropped after the
/// lock is released.
fn install_prepared_vec<T>(
    target: &mut Vec<T>,
    prepared: &mut Option<Vec<T>>,
) -> Result<Vec<T>, ExitRegistryError> {
    let Some(mut replacement) = prepared.take() else {
        return Err(ExitRegistryError::InvalidPreparedCapacity);
    };
    if replacement.capacity() <= target.len() {
        *prepared = Some(replacement);
        return Err(ExitRegistryError::InvalidPreparedCapacity);
    }
    replacement.append(target);
    std::mem::swap(target, &mut replacement);
    Ok(replacement)
}

fn push_callback(
    registry: &mut ExitRegistry,
    callback: &mut Option<AtexitCallbackEntry>,
) -> Result<(), ExitRegistryError> {
    let Some(registration_id) = registry.allocate_callback_id() else {
        return Err(ExitRegistryError::RegistrationIdExhausted);
    };
    let Some(mut callback) = callback.take() else {
        return Err(ExitRegistryError::InvalidPreparedCapacity);
    };
    callback.registration_id = registration_id;
    registry.callbacks.push(callback);
    Ok(())
}

fn raise_exit_registry_error(
    _py: &PyToken<'_>,
    error: ExitRegistryError,
    memory_message: &str,
) -> u64 {
    match error {
        ExitRegistryError::OutOfMemory => {
            raise_exception::<u64>(_py, "MemoryError", memory_message)
        }
        ExitRegistryError::RegistrationIdExhausted => raise_exception::<u64>(
            _py,
            "OverflowError",
            "atexit registration id space exhausted",
        ),
        ExitRegistryError::CapacityExhausted => {
            raise_exception::<u64>(_py, "OverflowError", "atexit registry capacity exhausted")
        }
        ExitRegistryError::FinalizerGenerationExhausted => raise_exception::<u64>(
            _py,
            "OverflowError",
            "weakref finalizer generation space exhausted",
        ),
        ExitRegistryError::InvalidPreparedCapacity => raise_exception::<u64>(
            _py,
            "RuntimeError",
            "invalid prepared atexit registry capacity",
        ),
    }
}

fn atexit_callback_release_refs(_py: &PyToken<'_>, callback: AtexitCallbackEntry) {
    if !obj_from_bits(callback.func_bits).is_none() {
        dec_ref_bits(_py, callback.func_bits);
    }
    if !obj_from_bits(callback.args_bits).is_none() {
        dec_ref_bits(_py, callback.args_bits);
    }
    if !obj_from_bits(callback.kwargs_bits).is_none() {
        dec_ref_bits(_py, callback.kwargs_bits);
    }
}

fn py_eq_checked(_py: &PyToken<'_>, lhs_bits: u64, rhs_bits: u64) -> Result<bool, u64> {
    match crate::object::ops_compare::compare_object_eq_bool(
        _py,
        obj_from_bits(lhs_bits),
        obj_from_bits(rhs_bits),
    ) {
        crate::object::ops_compare::CompareBoolOutcome::True => Ok(true),
        crate::object::ops_compare::CompareBoolOutcome::False => Ok(false),
        _ => Err(MoltObject::none().bits()),
    }
}

fn callable_identity_eq(lhs_bits: u64, rhs_bits: u64) -> bool {
    if lhs_bits == rhs_bits {
        return true;
    }
    let Some(lhs_ptr) = obj_from_bits(lhs_bits).as_ptr() else {
        return false;
    };
    let Some(rhs_ptr) = obj_from_bits(rhs_bits).as_ptr() else {
        return false;
    };
    let lhs_type = unsafe { object_type_id(lhs_ptr) };
    let rhs_type = unsafe { object_type_id(rhs_ptr) };
    if lhs_type != rhs_type {
        return false;
    }
    match lhs_type {
        TYPE_ID_FUNCTION => unsafe {
            function_fn_ptr(lhs_ptr) == function_fn_ptr(rhs_ptr)
                && function_closure_bits(lhs_ptr) == function_closure_bits(rhs_ptr)
        },
        TYPE_ID_BOUND_METHOD => unsafe {
            let lhs_func = bound_method_func_bits(lhs_ptr);
            let rhs_func = bound_method_func_bits(rhs_ptr);
            let lhs_self = bound_method_self_bits(lhs_ptr);
            let rhs_self = bound_method_self_bits(rhs_ptr);
            lhs_self == rhs_self && callable_identity_eq(lhs_func, rhs_func)
        },
        _ => false,
    }
}

fn unregister_compacts_for_target(target_minor: i64) -> bool {
    target_minor >= 14
}

fn atexit_unraisable_policy(_py: &PyToken<'_>, callback_bits: u64) -> (u64, Option<String>) {
    let callback_text = crate::builtins::exceptions::unraisable_context_repr(_py, callback_bits);
    if runtime_target_minor(_py) >= 13 {
        let prefix = "Exception ignored in atexit callback ";
        let message = prefix
            .len()
            .checked_add(callback_text.len())
            .and_then(|capacity| {
                let mut message = String::new();
                message.try_reserve_exact(capacity).ok()?;
                message.push_str(prefix);
                message.push_str(&callback_text);
                Some(message)
            });
        (MoltObject::none().bits(), message)
    } else {
        (callback_bits, None)
    }
}

fn atexit_call_callback(_py: &PyToken<'_>, callback: &AtexitCallbackEntry) -> u64 {
    let builder_bits = molt_callargs_new(0, 0);
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    if !obj_from_bits(callback.args_bits).is_none() {
        let _ = unsafe { molt_callargs_expand_star(builder_bits, callback.args_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return MoltObject::none().bits();
        }
    }
    if !obj_from_bits(callback.kwargs_bits).is_none() {
        let _ = unsafe { molt_callargs_expand_kwstar(builder_bits, callback.kwargs_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return MoltObject::none().bits();
        }
    }
    molt_call_bind(callback.func_bits, builder_bits)
}

fn atexit_register_impl(
    _py: &PyToken<'_>,
    func_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let callable_bits = molt_is_callable(func_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    if !is_truthy(_py, obj_from_bits(callable_bits)) {
        return raise_exception::<u64>(_py, "TypeError", "the first argument must be callable");
    }

    if !obj_from_bits(func_bits).is_none() {
        inc_ref_bits(_py, func_bits);
    }
    if !obj_from_bits(args_bits).is_none() {
        inc_ref_bits(_py, args_bits);
    }
    if !obj_from_bits(kwargs_bits).is_none() {
        inc_ref_bits(_py, kwargs_bits);
    }

    let mut entry = Some(AtexitCallbackEntry {
        registration_id: 0,
        kind: AtexitCallbackKind::Python,
        func_bits,
        args_bits,
        kwargs_bits,
    });
    let mut prepared = None;
    loop {
        let mut displaced = None;
        let mut publish_error = None;
        let required_capacity = {
            let mut registry = runtime_state(_py).exit_registry.lock().unwrap();
            let growth =
                growth_capacity_for_insert(registry.callbacks.len(), registry.callbacks.capacity());
            if let Err(error) = growth {
                publish_error = Some(error);
                None
            } else if let Some(required) = growth.ok().flatten() {
                if prepared
                    .as_ref()
                    .is_none_or(|buffer: &Vec<AtexitCallbackEntry>| buffer.capacity() < required)
                {
                    Some(required)
                } else {
                    match install_prepared_vec(&mut registry.callbacks, &mut prepared) {
                        Ok(buffer) => displaced = Some(buffer),
                        Err(error) => publish_error = Some(error),
                    }
                    if publish_error.is_none() {
                        publish_error = push_callback(&mut registry, &mut entry).err();
                    }
                    None
                }
            } else {
                publish_error = push_callback(&mut registry, &mut entry).err();
                None
            }
        };
        drop(displaced);
        if let Some(error) = publish_error {
            if let Some(entry) = entry.take() {
                atexit_callback_release_refs(_py, entry);
            }
            return raise_exit_registry_error(_py, error, "atexit callback registration failed");
        }
        if entry.is_none() {
            return func_bits;
        }
        let Some(required_capacity) = required_capacity else {
            if let Some(entry) = entry.take() {
                atexit_callback_release_refs(_py, entry);
            }
            return raise_exit_registry_error(
                _py,
                ExitRegistryError::InvalidPreparedCapacity,
                "atexit callback registration failed",
            );
        };
        prepared = match try_prepare_vec(required_capacity) {
            Ok(prepared) => Some(prepared),
            Err(error) => {
                if let Some(entry) = entry.take() {
                    atexit_callback_release_refs(_py, entry);
                }
                return raise_exit_registry_error(
                    _py,
                    error,
                    "atexit callback registration failed",
                );
            }
        };
    }
}

fn weakref_runner_entry() -> AtexitCallbackEntry {
    AtexitCallbackEntry {
        registration_id: 0,
        kind: AtexitCallbackKind::WeakrefFinalizerRunner,
        func_bits: MoltObject::none().bits(),
        args_bits: MoltObject::none().bits(),
        kwargs_bits: MoltObject::none().bits(),
    }
}

/// Atomically retain a finalizer and publish its one-time atexit runner. Both
/// backing stores are prepared outside the registry lock, so publication uses
/// only infallible pushes and no rollback can expose half a registration.
pub(crate) fn weakref_finalizer_track(
    _py: &PyToken<'_>,
    finalizer_bits: u64,
) -> Result<bool, ExitRegistryError> {
    inc_ref_bits(_py, finalizer_bits);
    let mut prepared_callbacks = None;
    let mut prepared_finalizers = None;
    loop {
        let mut displaced_callbacks = None;
        let mut displaced_finalizers = None;
        let mut publish_error = None;
        let (duplicate, committed, callback_capacity, finalizer_capacity) = {
            let mut registry = runtime_state(_py).exit_registry.lock().unwrap();
            if registry.weakref_finalizers.contains(finalizer_bits) {
                (true, false, None, None)
            } else if !registry.weakref_finalizers.generation_available() {
                publish_error = Some(ExitRegistryError::FinalizerGenerationExhausted);
                (false, false, None, None)
            } else {
                let publish_runner = registry.weakref_runner_state == WeakrefRunnerState::Available;
                let callback_capacity = if publish_runner {
                    match growth_capacity_for_insert(
                        registry.callbacks.len(),
                        registry.callbacks.capacity(),
                    ) {
                        Ok(capacity) => capacity,
                        Err(error) => {
                            publish_error = Some(error);
                            None
                        }
                    }
                } else {
                    None
                };
                let finalizer_capacity = if registry.weakref_finalizers.can_insert() {
                    None
                } else {
                    let required = registry.weakref_finalizers.len().checked_add(1);
                    match required.and_then(|required| {
                        geometric_capacity(registry.weakref_finalizers.capacity(), required).ok()
                    }) {
                        Some(capacity) => Some(capacity),
                        None => {
                            publish_error = Some(ExitRegistryError::CapacityExhausted);
                            None
                        }
                    }
                };
                let callback_ready =
                    callback_capacity.is_none_or(|required| {
                        prepared_callbacks.as_ref().is_some_and(
                            |buffer: &Vec<AtexitCallbackEntry>| buffer.capacity() >= required,
                        )
                    });
                let finalizer_ready = finalizer_capacity.is_none_or(|required| {
                    prepared_finalizers
                        .as_ref()
                        .is_some_and(|buffer: &WeakFinalizerPrepared| {
                            // Prepared capacity is validated again while installing.
                            let _ = required;
                            buffer.capacity() >= required
                        })
                });
                if publish_error.is_some() {
                    (false, false, None, None)
                } else if !callback_ready || !finalizer_ready {
                    (false, false, callback_capacity, finalizer_capacity)
                } else {
                    if callback_capacity.is_some() {
                        match install_prepared_vec(&mut registry.callbacks, &mut prepared_callbacks)
                        {
                            Ok(buffer) => displaced_callbacks = Some(buffer),
                            Err(error) => publish_error = Some(error),
                        }
                    }
                    if publish_error.is_none() && finalizer_capacity.is_some() {
                        if let Some(prepared) = prepared_finalizers.take() {
                            match registry.weakref_finalizers.install_prepared(prepared) {
                                Ok(buffer) => displaced_finalizers = Some(buffer),
                                Err(buffer) => {
                                    prepared_finalizers = Some(buffer);
                                    publish_error =
                                        Some(ExitRegistryError::InvalidPreparedCapacity);
                                }
                            }
                        } else {
                            publish_error = Some(ExitRegistryError::InvalidPreparedCapacity);
                        }
                    }
                    if publish_error.is_none() && publish_runner {
                        let mut runner = Some(weakref_runner_entry());
                        publish_error = push_callback(&mut registry, &mut runner).err();
                        if publish_error.is_none() {
                            registry.weakref_runner_state = WeakrefRunnerState::Registered;
                        }
                    }
                    if publish_error.is_none() {
                        match registry.weakref_finalizers.insert_prepared(finalizer_bits) {
                            Ok(true) => (false, true, None, None),
                            Ok(false) => (true, false, None, None),
                            Err(()) => {
                                if publish_runner {
                                    let runner = registry.callbacks.pop();
                                    if runner.as_ref().is_some_and(|callback| {
                                        callback.kind == AtexitCallbackKind::WeakrefFinalizerRunner
                                    }) {
                                        registry.weakref_runner_state =
                                            WeakrefRunnerState::Available;
                                    } else if let Some(runner) = runner {
                                        registry.callbacks.push(runner);
                                    }
                                }
                                publish_error = Some(ExitRegistryError::InvalidPreparedCapacity);
                                (false, false, None, None)
                            }
                        }
                    } else {
                        (false, false, None, None)
                    }
                }
            }
        };
        drop(displaced_callbacks);
        drop(displaced_finalizers);
        if let Some(error) = publish_error {
            dec_ref_bits(_py, finalizer_bits);
            return Err(error);
        }
        if duplicate {
            dec_ref_bits(_py, finalizer_bits);
            return Ok(false);
        }
        if committed {
            return Ok(true);
        }
        if let Some(required) = callback_capacity
            && prepared_callbacks
                .as_ref()
                .is_none_or(|buffer| buffer.capacity() < required)
        {
            prepared_callbacks = match try_prepare_vec(required) {
                Ok(buffer) => Some(buffer),
                Err(error) => {
                    dec_ref_bits(_py, finalizer_bits);
                    return Err(error);
                }
            };
        }
        if let Some(required) = finalizer_capacity
            && prepared_finalizers
                .as_ref()
                .is_none_or(|buffer| buffer.capacity() < required)
        {
            prepared_finalizers = match WeakFinalizerPrepared::try_with_capacity(required) {
                Ok(buffer) => Some(buffer),
                Err(()) => {
                    dec_ref_bits(_py, finalizer_bits);
                    return Err(ExitRegistryError::OutOfMemory);
                }
            };
        }
    }
}

pub(crate) fn weakref_finalizer_untrack(_py: &PyToken<'_>, finalizer_bits: u64) {
    let removed = {
        let mut registry = runtime_state(_py).exit_registry.lock().unwrap();
        registry.weakref_finalizers.remove(finalizer_bits)
    };
    if let Some(bits) = removed {
        dec_ref_bits(_py, bits);
    }
}

pub(crate) fn pop_weakref_finalizer(_py: &PyToken<'_>) -> Option<u64> {
    let mut registry = runtime_state(_py).exit_registry.lock().unwrap();
    registry.weakref_finalizers.pop_lifo()
}

fn atexit_unregister_impl(_py: &PyToken<'_>, func_bits: u64) -> u64 {
    let compact_matches = unregister_compacts_for_target(runtime_target_minor(_py));
    let mut candidates: Vec<(u64, u64, bool)> = loop {
        let capacity = runtime_state(_py)
            .exit_registry
            .lock()
            .unwrap()
            .callbacks
            .len();
        let mut candidates = Vec::new();
        if candidates.try_reserve_exact(capacity).is_err() {
            return raise_exception::<u64>(
                _py,
                "MemoryError",
                "atexit unregister snapshot allocation failed",
            );
        }
        let complete = {
            let registry = runtime_state(_py).exit_registry.lock().unwrap();
            if registry.callbacks.len() > capacity {
                false
            } else {
                for callback in &registry.callbacks {
                    if callback.kind == AtexitCallbackKind::Python
                        && !obj_from_bits(callback.func_bits).is_none()
                    {
                        inc_ref_bits(_py, callback.func_bits);
                        candidates.push((callback.registration_id, callback.func_bits, false));
                    }
                }
                true
            }
        };
        if complete {
            break candidates;
        }
    };

    let mut comparison_failed = false;
    for candidate in &mut candidates {
        match py_eq_checked(_py, func_bits, candidate.1) {
            Ok(equal) => {
                candidate.2 = equal || callable_identity_eq(func_bits, candidate.1);
            }
            Err(_) => {
                comparison_failed = true;
                break;
            }
        }
    }
    if comparison_failed {
        for (_, callback_bits, _) in candidates {
            dec_ref_bits(_py, callback_bits);
        }
        return MoltObject::none().bits();
    }

    let matched = candidates.iter().filter(|candidate| candidate.2).count();
    let mut removed = Vec::new();
    if removed.try_reserve_exact(matched).is_err() {
        for (_, callback_bits, _) in candidates {
            dec_ref_bits(_py, callback_bits);
        }
        return raise_exception::<u64>(
            _py,
            "MemoryError",
            "atexit unregister removal allocation failed",
        );
    }

    {
        let mut registry = runtime_state(_py).exit_registry.lock().unwrap();
        let mut candidate_index = 0;
        if compact_matches {
            registry.callbacks.retain(|callback| {
                while candidate_index < candidates.len()
                    && candidates[candidate_index].0 < callback.registration_id
                {
                    candidate_index += 1;
                }
                let matched = candidates.get(candidate_index).is_some_and(|candidate| {
                    candidate.0 == callback.registration_id && candidate.2
                });
                if matched {
                    removed.push(callback.clone());
                }
                !matched
            });
        } else {
            for callback in &mut registry.callbacks {
                while candidate_index < candidates.len()
                    && candidates[candidate_index].0 < callback.registration_id
                {
                    candidate_index += 1;
                }
                if candidate_index == candidates.len() {
                    break;
                }
                let candidate = candidates[candidate_index];
                if candidate.0 == callback.registration_id && candidate.2 {
                    removed.push(callback.clone());
                    callback.func_bits = MoltObject::none().bits();
                    callback.args_bits = MoltObject::none().bits();
                    callback.kwargs_bits = MoltObject::none().bits();
                }
            }
        }
    }
    for (_, callback_bits, _) in candidates {
        dec_ref_bits(_py, callback_bits);
    }
    for callback in removed {
        atexit_callback_release_refs(_py, callback);
    }
    MoltObject::none().bits()
}

fn detach_exit_callbacks(registry: &mut ExitRegistry) -> Vec<AtexitCallbackEntry> {
    let callbacks = std::mem::take(&mut registry.callbacks);
    // Clearing an already-published finalize runner is a persistent opt-out.
    // A clear before the first finalizer does not prevent its lazy publish.
    if registry.weakref_runner_state == WeakrefRunnerState::Registered {
        registry.weakref_runner_state = WeakrefRunnerState::Cleared;
    }
    callbacks
}

fn atexit_clear_impl(_py: &PyToken<'_>) -> u64 {
    let callbacks = {
        let mut registry = runtime_state(_py).exit_registry.lock().unwrap();
        detach_exit_callbacks(&mut registry)
    };
    for callback in callbacks {
        atexit_callback_release_refs(_py, callback);
    }
    MoltObject::none().bits()
}

/// Release late registrations without starting another exit-callback phase.
/// Keep monotonic identities and runner policy while detaching both owner
/// cohorts before any decref can register replacement callbacks/finalizers.
pub(crate) fn atexit_clear_runtime_roots(_py: &PyToken<'_>) -> bool {
    let (callbacks, finalizers) = {
        let mut registry = runtime_state(_py).exit_registry.lock().unwrap();
        let mut finalizers = Vec::with_capacity(registry.weakref_finalizers.len());
        let callbacks = detach_exit_callbacks(&mut registry);
        while let Some(bits) = registry.weakref_finalizers.pop_lifo() {
            finalizers.push(bits);
        }
        (callbacks, finalizers)
    };
    let changed = !callbacks.is_empty() || !finalizers.is_empty();
    for callback in callbacks {
        atexit_callback_release_refs(_py, callback);
    }
    for bits in finalizers {
        dec_ref_bits(_py, bits);
    }
    changed
}

fn atexit_run_exitfuncs_impl(_py: &PyToken<'_>) -> u64 {
    #[cfg(test)]
    crate::state::runtime_state::run_finalizing_atexit_test_hook();
    loop {
        let callback = {
            let mut registry = runtime_state(_py).exit_registry.lock().unwrap();
            let callback = registry.callbacks.pop();
            if let Some(callback) = callback.as_ref()
                && callback.kind == AtexitCallbackKind::WeakrefFinalizerRunner
            {
                registry.weakref_runner_state = WeakrefRunnerState::Cleared;
            }
            callback
        };
        let Some(callback) = callback else {
            break;
        };
        if callback.kind == AtexitCallbackKind::WeakrefFinalizerRunner {
            crate::builtins::exceptions::run_unraisable(
                _py,
                MoltObject::none().bits(),
                Some("Exception ignored while running weakref finalizers at exit"),
                || crate::object::weakref::weakref_run_atexit_finalizers(_py),
            );
            continue;
        }
        if obj_from_bits(callback.func_bits).is_none() {
            atexit_callback_release_refs(_py, callback);
            continue;
        }
        let (context_bits, message) = atexit_unraisable_policy(_py, callback.func_bits);
        let out_bits = crate::builtins::exceptions::run_unraisable(
            _py,
            context_bits,
            message.as_deref(),
            || atexit_call_callback(_py, &callback),
        );
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
        atexit_callback_release_refs(_py, callback);
    }
    MoltObject::none().bits()
}

fn atexit_ncallbacks_impl(_py: &PyToken<'_>) -> u64 {
    let count = runtime_state(_py)
        .exit_registry
        .lock()
        .unwrap()
        .callbacks
        .len();
    let count = i64::try_from(count).unwrap_or(i64::MAX);
    int_bits_from_i64(_py, count)
}

pub(crate) fn atexit_run_exitfuncs_teardown(_py: &PyToken<'_>) {
    let _ = atexit_run_exitfuncs_impl(_py);
    // `_clear()` intentionally disables exit execution, but the registry's
    // retained references still belong to this runtime and must be released.
    while let Some(finalizer_bits) = pop_weakref_finalizer(_py) {
        dec_ref_bits(_py, finalizer_bits);
    }
    if exception_pending(_py) {
        clear_exception(_py);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_register(func_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        atexit_register_impl(_py, func_bits, args_bits, kwargs_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_unregister(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { atexit_unregister_impl(_py, func_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_clear() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { atexit_clear_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_run_exitfuncs() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { atexit_run_exitfuncs_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_ncallbacks() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { atexit_ncallbacks_impl(_py) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    static LATE_CALLBACK: AtomicU64 = AtomicU64::new(0);
    static LATE_FINALIZER: AtomicU64 = AtomicU64::new(0);
    static EXIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static RELEASE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static RELEASE_SAW_DETACHED_ROOTS: AtomicBool = AtomicBool::new(false);

    extern "C" fn count_exit_callback(_arg: u64) -> u64 {
        EXIT_CALLS.fetch_add(1, Ordering::AcqRel);
        MoltObject::none().bits()
    }

    extern "C" fn count_exit_finalizer() -> u64 {
        EXIT_CALLS.fetch_add(1, Ordering::AcqRel);
        MoltObject::none().bits()
    }

    extern "C" fn observe_exit_owner_release(_weak: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            RELEASE_CALLS.fetch_add(1, Ordering::AcqRel);
            let detached = runtime_state(py)
                .exit_registry
                .try_lock()
                .is_ok_and(|registry| {
                    registry.callbacks.is_empty() && registry.weakref_finalizers.len() == 0
                });
            RELEASE_SAW_DETACHED_ROOTS.store(detached, Ordering::Release);
            if detached {
                // Real reentrant publication must survive the current detached
                // snapshot and belong to the next root-drain pass.
                let args = crate::alloc_tuple(py, &[MoltObject::none().bits()]);
                if args.is_null() {
                    return MoltObject::none().bits();
                }
                let args = MoltObject::from_ptr(args).bits();
                molt_atexit_register(
                    LATE_CALLBACK.load(Ordering::Acquire),
                    args,
                    MoltObject::none().bits(),
                );
                dec_ref_bits(py, args);
                crate::molt_weakref_finalize_track(LATE_FINALIZER.load(Ordering::Acquire));
            }
            MoltObject::none().bits()
        })
    }

    fn test_callable(py: &PyToken<'_>, address: *const (), arity: u64) -> u64 {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            arity,
        );
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn owner_count(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
        unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn late_exit_roots_detach_before_release_without_a_second_execution_phase() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            while atexit_clear_runtime_roots(py) {}
            EXIT_CALLS.store(0, Ordering::Release);
            RELEASE_CALLS.store(0, Ordering::Release);
            RELEASE_SAW_DETACHED_ROOTS.store(false, Ordering::Release);
            let callback = test_callable(py, count_exit_callback as *const (), 1);
            let finalizer = test_callable(py, count_exit_finalizer as *const (), 0);
            let observer = test_callable(py, observe_exit_owner_release as *const (), 1);
            let callback_base = owner_count(callback);
            let finalizer_base = owner_count(finalizer);
            LATE_CALLBACK.store(callback, Ordering::Release);
            LATE_FINALIZER.store(finalizer, Ordering::Release);

            // Run the actual one-time phase before introducing late owners.
            molt_atexit_register(
                finalizer,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            );
            atexit_run_exitfuncs_teardown(py);
            assert_eq!(EXIT_CALLS.load(Ordering::Acquire), 1);
            assert_eq!(owner_count(finalizer), finalizer_base);

            let target_ptr = crate::object::builders::alloc_set_with_entries(py, &[]);
            assert!(!target_ptr.is_null());
            let target = MoltObject::from_ptr(target_ptr).bits();
            let weak = crate::molt_weakref_new(
                crate::builtin_classes(py).reference_type,
                target,
                observer,
            );
            assert!(obj_from_bits(weak).as_ptr().is_some());
            let args_ptr = crate::alloc_tuple(py, &[target]);
            assert!(!args_ptr.is_null());
            let args = MoltObject::from_ptr(args_ptr).bits();
            molt_atexit_register(callback, args, MoltObject::none().bits());
            crate::molt_weakref_finalize_track(finalizer);
            assert!(!exception_pending(py));
            assert_eq!(owner_count(callback), callback_base + 1);
            assert_eq!(owner_count(finalizer), finalizer_base + 1);
            dec_ref_bits(py, args);
            dec_ref_bits(py, target);
            assert_eq!(RELEASE_CALLS.load(Ordering::Acquire), 0);

            assert!(atexit_clear_runtime_roots(py));
            assert_eq!(RELEASE_CALLS.load(Ordering::Acquire), 1);
            assert!(RELEASE_SAW_DETACHED_ROOTS.load(Ordering::Acquire));
            assert!(obj_from_bits(crate::molt_weakref_get(weak)).is_none());
            assert_eq!(
                owner_count(callback),
                callback_base + 1,
                "only reentrant registration remains"
            );
            assert_eq!(
                owner_count(finalizer),
                finalizer_base + 1,
                "only reentrant finalizer remains"
            );
            assert_eq!(
                EXIT_CALLS.load(Ordering::Acquire),
                1,
                "root release never executes exit functions"
            );

            assert!(atexit_clear_runtime_roots(py));
            assert_eq!(owner_count(callback), callback_base);
            assert_eq!(owner_count(finalizer), finalizer_base);
            assert!(!atexit_clear_runtime_roots(py));
            assert_eq!(EXIT_CALLS.load(Ordering::Acquire), 1);
            assert!(!exception_pending(py));
            LATE_CALLBACK.store(0, Ordering::Release);
            LATE_FINALIZER.store(0, Ordering::Release);
            for bits in [weak, observer, callback, finalizer] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn callback_capacity_growth_is_geometric_and_checked() {
        let mut capacity = 0;
        let mut growths = 0;
        for len in 0..10_000 {
            if let Some(next) = growth_capacity_for_insert(len, capacity).expect("growth") {
                assert!(next > len);
                assert!(next >= MIN_EXIT_REGISTRY_CAPACITY);
                capacity = next;
                growths += 1;
            }
        }
        assert!(growths <= 12);
        assert!(capacity >= 10_000);
        assert_eq!(
            geometric_capacity(usize::MAX / 2 + 1, usize::MAX),
            Err(ExitRegistryError::CapacityExhausted)
        );
        assert_eq!(
            growth_capacity_for_insert(usize::MAX, usize::MAX),
            Err(ExitRegistryError::CapacityExhausted)
        );
    }

    #[test]
    fn unregister_count_semantics_are_target_versioned() {
        assert!(!unregister_compacts_for_target(12));
        assert!(!unregister_compacts_for_target(13));
        assert!(unregister_compacts_for_target(14));
    }
}
