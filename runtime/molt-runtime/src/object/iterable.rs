//! Owned iteration for runtime consumers. Exhaustion, exceptions and reference
//! transfer share the unboxed iterator protocol used by compiled loops.

use crate::*;

#[derive(Clone, Copy)]
pub(crate) enum SpecialIterationKind {
    Next,
    SequenceItem(u64),
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
) -> Result<SpecialIterationStep, ()> {
    exception_stack_push();
    let name: &[u8] = match kind {
        SpecialIterationKind::Next => b"__next__",
        SpecialIterationKind::SequenceItem(_) => b"__getitem__",
    };
    let callable = unsafe { crate::builtins::attr::lookup_special_method(py, receiver, name) };
    let value = callable.map(|callable| {
        let value = unsafe {
            match kind {
                SpecialIterationKind::Next => call_callable0(py, callable),
                SpecialIterationKind::SequenceItem(index) => call_callable1(py, callable, index),
            }
        };
        dec_ref_bits(py, callable);
        value
    });
    if exception_pending(py) {
        if let Some(value) = value {
            dec_ref_bits(py, value);
        }
        let exception = molt_exception_last();
        let stop = crate::builtins::exceptions::exception_matches_builtin_name(
            py,
            exception,
            "StopIteration",
        );
        let exhausted = stop
            || (matches!(kind, SpecialIterationKind::SequenceItem(_))
                && crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    exception,
                    "IndexError",
                ));
        if exhausted {
            let payload = if stop && matches!(kind, SpecialIterationKind::Next) {
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
            } else {
                MoltObject::none().bits()
            };
            molt_exception_clear();
            exception_stack_pop(py);
            dec_ref_bits(py, exception);
            return Ok(SpecialIterationStep::Exhausted(payload));
        }
        exception_stack_pop_restore_last(py, exception);
        dec_ref_bits(py, exception);
        return Err(());
    }
    exception_stack_pop(py);
    Ok(value
        .map(SpecialIterationStep::Item)
        .unwrap_or(SpecialIterationStep::Missing))
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
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, bits));
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
            "OverflowError",
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
        return raise_exception::<_>(py, "MemoryError", "iterable allocation failed");
    }
    loop {
        match iter.next() {
            Ok(Some(item)) => {
                if values.try_reserve(1).is_err() {
                    dec_ref_bits(py, item);
                    for value in values {
                        dec_ref_bits(py, value);
                    }
                    return raise_exception::<_>(py, "MemoryError", "iterable allocation failed");
                }
                values.push(item);
            }
            Ok(None) => return Some(values),
            Err(molt_runtime_core::ErrorIndicatorSet) => {
                for value in values {
                    dec_ref_bits(py, value);
                }
                return None;
            }
        }
    }
}
