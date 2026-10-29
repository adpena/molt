//! Owned iteration for runtime consumers. Exhaustion, exceptions and reference
//! transfer share the unboxed iterator protocol used by compiled loops.

use crate::BuiltinExc;
use crate::*;

#[derive(Clone, Copy)]
pub(crate) enum SpecialIterationKind {
    Next,
    SequenceItem(u64),
}

/// Public next() without a default preserves the exact raised exception.
/// Loop/status consumers normalize StopIteration into an owned completion value.
#[derive(Clone, Copy)]
pub(crate) enum IterationExhaustion {
    Normalize,
    PreserveException,
}

/// Values and exhaustion payloads each transfer one owned reference.
pub(crate) enum SpecialIterationStep {
    Item(u64),
    Exhausted(u64),
    Missing,
}

/// Descriptor binding is part of the slot call: its exceptions have the same
/// iteration meaning as exceptions from the returned callable.
pub(crate) fn special_iteration_step(
    py: &PyToken<'_>,
    receiver: u64,
    kind: SpecialIterationKind,
    exhaustion: IterationExhaustion,
) -> Result<SpecialIterationStep, ()> {
    let frame = crate::builtins::exceptions::ExceptionStackScope::push(py);
    let name: &[u8] = match kind {
        SpecialIterationKind::Next => b"__next__",
        SpecialIterationKind::SequenceItem(_) => b"__getitem__",
    };
    use crate::builtins::attr::DescriptorArguments;
    let args = match kind {
        SpecialIterationKind::Next => DescriptorArguments::Zero,
        SpecialIterationKind::SequenceItem(index) => DescriptorArguments::One(index),
    };
    let (value, lookup_failed) =
        match unsafe { crate::builtins::attr::invoke_special_method(py, receiver, name, args) } {
            Ok(value) => (value, false),
            Err(()) => (None, true),
        };
    if lookup_failed || (value.is_some() && exception_pending(py)) {
        if let Some(value) = value {
            dec_ref_bits(py, value);
        }
        return finish_iteration_error(py, kind, exhaustion, frame);
    }
    drop(frame);
    Ok(value
        .map(SpecialIterationStep::Item)
        .unwrap_or(SpecialIterationStep::Missing))
}

/// The owned value a raised StopIteration carries; None when it has none.
pub(crate) fn stop_iteration_payload(py: &PyToken<'_>, exception: u64) -> u64 {
    obj_from_bits(exception)
        .as_ptr()
        .and_then(|ptr| unsafe {
            (object_type_id(ptr) == TYPE_ID_EXCEPTION)
                .then(|| {
                    crate::builtins::exceptions::exception_typed_field_get(
                        py,
                        ptr,
                        molt_obj_model::ExceptionTypedField::StopIterationValue,
                    )
                    .and_then(Result::ok)
                })
                .flatten()
        })
        .unwrap_or_else(|| MoltObject::none().bits())
}

pub(crate) unsafe fn builtin_receiver(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let class = unsafe { object_class_bits(ptr) };
    class == 0
        || (is_builtin_class_bits(py, class)
            && crate::obj_from_bits(class)
                .as_ptr()
                .is_some_and(|class| unsafe { crate::object::class_is_immutable(py, class) }))
}

pub(crate) struct OwnedIterator<'a, 'py> {
    py: &'a PyToken<'py>,
    owner: molt_runtime_core::OwnedRuntimeValue<'a>,
}

impl<'a, 'py> OwnedIterator<'a, 'py> {
    pub(crate) fn new(py: &'a PyToken<'py>, iterable: u64) -> Option<Self> {
        let bits = molt_iter(iterable);
        if exception_pending(py) {
            dec_ref_bits(py, bits);
            return None;
        }
        if obj_from_bits(bits).is_none() {
            raise_not_iterable::<()>(py, iterable);
            return None;
        }
        Some(Self {
            py,
            owner: unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), bits)
            },
        })
    }

    pub(crate) fn bits(&self) -> u64 {
        self.owner.bits()
    }

    /// An item carries one owned reference; None means clean exhaustion only.
    pub(crate) fn next(&mut self) -> Result<Option<u64>, molt_runtime_core::ErrorIndicatorSet> {
        // Both in-tree consumers and satellites use the same owned transport.
        molt_runtime_core::iter_next_owned(self.py.core_token(), &self.owner)
            .map(|item| item.map(molt_runtime_core::OwnedRuntimeValue::into_bits))
    }
}

/// Resolve the observable hint after __iter__, before the first __next__.
pub(crate) fn length_hint(py: &PyToken<'_>, iterable: u64) -> Option<usize> {
    let bits = crate::builtins::operator::molt_operator_length_hint(
        iterable,
        MoltObject::from_int(8).bits(),
    );
    if exception_pending(py) {
        dec_ref_bits(py, bits);
        return None;
    }
    let hint = crate::builtins::numbers::index_i64_integral_bits(bits)
        .and_then(|value| usize::try_from(value).ok());
    dec_ref_bits(py, bits);
    if hint.is_none() {
        raise_exception::<()>(
            py,
            BuiltinExc::OverflowError,
            "cannot fit length hint into an index-sized integer",
        );
    }
    hint
}

#[derive(Clone, Copy)]
pub(crate) enum LengthHint {
    Consult,
    Skip,
}

/// tuple() stopped consulting length hints in CPython 3.14.
pub(crate) fn tuple_length_hint_policy(py: &PyToken<'_>) -> LengthHint {
    if crate::object::ops_sys::runtime_target_at_least(py, 3, 14) {
        LengthHint::Skip
    } else {
        LengthHint::Consult
    }
}

pub(crate) fn collect(py: &PyToken<'_>, iterable: u64, policy: LengthHint) -> Option<Vec<u64>> {
    let iter = OwnedIterator::new(py, iterable)?;
    collect_from_owned_iterator(iter, iterable, policy)
}

/// Collect an acquired iterator without conflating acquisition errors with
/// exceptions raised by its hint or next callbacks.
pub(crate) fn collect_from_owned_iterator(
    mut iter: OwnedIterator<'_, '_>,
    hint_source: u64,
    policy: LengthHint,
) -> Option<Vec<u64>> {
    let py = iter.py;
    let hint = match policy {
        LengthHint::Consult => length_hint(py, hint_source)?,
        LengthHint::Skip => 0,
    };
    let mut values = Vec::new();
    if values.try_reserve(hint).is_err() {
        return raise_exception::<_>(py, BuiltinExc::MemoryError, "iterable allocation failed");
    }
    loop {
        match iter.next() {
            Ok(Some(item)) => {
                if values.try_reserve(1).is_err() {
                    raise_exception::<()>(
                        py,
                        BuiltinExc::MemoryError,
                        "iterable allocation failed",
                    );
                    dec_ref_bits(py, item);
                    drop(iter);
                    for value in values.into_iter().rev() {
                        dec_ref_bits(py, value);
                    }
                    return None;
                }
                values.push(item);
            }
            Ok(None) => return Some(values),
            Err(molt_runtime_core::ErrorIndicatorSet) => {
                // The iterator retires before the partial materialization;
                // collection elements then unwind in reverse insertion order.
                drop(iter);
                for value in values.into_iter().rev() {
                    dec_ref_bits(py, value);
                }
                return None;
            }
        }
    }
}

/// PySequence_Fast-style custody for join consumers. Exact sequences retain
/// only the sequence object; generic iterables finish all callbacks and publish
/// an owned list before a caller validates elements or borrows payloads.
pub(crate) fn materialize_sequence<'a, 'py>(
    py: &'a PyToken<'py>,
    iterable: u64,
    acquisition_error: &str,
) -> Option<molt_runtime_core::OwnedRuntimeValue<'a>> {
    let class = type_of_bits(py, iterable);
    if class == builtin_classes(py).list || class == builtin_classes(py).tuple {
        inc_ref_bits(py, iterable);
        return Some(unsafe {
            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), iterable)
        });
    }
    let Some(first) = OwnedIterator::new(py, iterable) else {
        let error = molt_exception_last_pending();
        let type_error = crate::builtins::exceptions::exception_matches_kind(
            py,
            error,
            crate::BuiltinExc::TypeError,
        );
        dec_ref_bits(py, error);
        if type_error {
            clear_exception(py);
            raise_exception::<()>(py, BuiltinExc::TypeError, acquisition_error);
        }
        return None;
    };
    // PySequence_Fast obtains an iterator, then PySequence_List reacquires it
    // and consults that first iterator's hint. Later errors are not translated.
    let values = collect(py, first.bits(), LengthHint::Consult)?;
    let list = super::builders::alloc_list_with_capacity_owned(py, &values, values.len());
    if list.is_null() {
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            for value in values.into_iter().rev() {
                dec_ref_bits(py, value);
            }
        });
        return None;
    }
    // Publication transfers the collected references; the canonical list owner
    // retires its edges in reverse order, including callback-visible cleanup.
    Some(unsafe {
        molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
            py.core_token(),
            MoltObject::from_ptr(list).bits(),
        )
    })
}

/// Complete the active iteration exception transaction. Both native adapters
/// and Python slots use this owner; only explicitly normalized consumers clear
/// StopIteration/sequence IndexError and project an owned completion payload.
pub(crate) fn finish_iteration_error(
    py: &PyToken<'_>,
    kind: SpecialIterationKind,
    exhaustion: IterationExhaustion,
    frame: crate::builtins::exceptions::ExceptionStackScope<'_, '_>,
) -> Result<SpecialIterationStep, ()> {
    let exception = molt_exception_last_pending();
    let stop = crate::builtins::exceptions::exception_matches_kind(
        py,
        exception,
        crate::BuiltinExc::StopIteration,
    );
    let exhausted = stop
        || (matches!(kind, SpecialIterationKind::SequenceItem(_))
            && crate::builtins::exceptions::exception_matches_kind(
                py,
                exception,
                crate::BuiltinExc::IndexError,
            ));
    if exhausted && matches!(exhaustion, IterationExhaustion::Normalize) {
        let payload = if stop && matches!(kind, SpecialIterationKind::Next) {
            stop_iteration_payload(py, exception)
        } else {
            MoltObject::none().bits()
        };
        molt_exception_clear();
        drop(frame);
        dec_ref_bits(py, exception);
        return Ok(SpecialIterationStep::Exhausted(payload));
    }
    drop(frame);
    let _ = molt_exception_set_last(exception);
    dec_ref_bits(py, exception);
    Err(())
}
