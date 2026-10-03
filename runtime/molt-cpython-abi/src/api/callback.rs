//! Completion of native callbacks that borrow managed operand projections.
//!
//! Capture the callback's direct operands before reentry. Completion publishes
//! mutable views on success and failure through the bridge observation authority;
//! it never walks object fields or keeps a second dirty-object registry.

use crate::abi_types::PyObject;
use crate::api::{errors, mapping, refcount, sequences};
use crate::bridge::GLOBAL_BRIDGE;
use std::os::raw::c_int;
use std::ptr;

pub(crate) struct CallbackOperands<const N: usize> {
    roots: [*mut PyObject; N],
    inline: [*mut PyObject; 8],
    inline_len: usize,
    spill: Vec<*mut PyObject>,
}

impl<const N: usize> CallbackOperands<N> {
    /// Fixed protocol operands stay on the stack; descriptor completion adds no
    /// allocation. Argument snapshots retain values even if a callback clears
    /// its kwargs dictionary or replaces a container entry during reentry.
    pub(crate) unsafe fn new(mut roots: [*mut PyObject; N]) -> Self {
        for index in 0..N {
            if roots[..index].contains(&roots[index]) {
                roots[index] = ptr::null_mut();
            } else {
                unsafe { refcount::Py_XINCREF(roots[index]) };
            }
        }
        Self {
            roots,
            inline: [ptr::null_mut(); 8],
            inline_len: 0,
            spill: Vec::new(),
        }
    }

    fn reserve_arguments(&mut self, count: usize) -> bool {
        if self
            .spill
            .try_reserve_exact(count.saturating_sub(self.inline.len()))
            .is_err()
        {
            unsafe { errors::PyErr_NoMemory() };
            return false;
        }
        true
    }

    unsafe fn push_argument(&mut self, value: *mut PyObject) -> bool {
        if value.is_null() {
            if !errors::raised_error_pending() {
                unsafe { errors::PyErr_BadInternalCall() };
            }
            return false;
        }
        // Root and inline deduplication is bounded independently of arity.
        // The spill remains linear even for very large repeated argument lists.
        if self.roots.contains(&value) || self.inline[..self.inline_len].contains(&value) {
            return true;
        }
        if self.inline_len == self.inline.len() && self.spill.try_reserve(1).is_err() {
            unsafe { errors::PyErr_NoMemory() };
            return false;
        }
        unsafe { refcount::Py_INCREF(value) };
        if self.inline_len < self.inline.len() {
            self.inline[self.inline_len] = value;
            self.inline_len += 1;
        } else {
            self.spill.push(value);
        }
        true
    }

    pub(crate) unsafe fn from_vector(
        roots: [*mut PyObject; N],
        values: &[*mut PyObject],
    ) -> Option<Self> {
        let mut operands = unsafe { Self::new(roots) };
        if !operands.reserve_arguments(values.len()) {
            return None;
        }
        for &value in values {
            if !unsafe { operands.push_argument(value) } {
                return None;
            }
        }
        Some(operands)
    }

    pub(crate) unsafe fn from_tuple_dict(
        roots: [*mut PyObject; N],
        args: *mut PyObject,
        kwargs: *mut PyObject,
    ) -> Option<Self> {
        let mut operands = unsafe { Self::new(roots) };
        let positional = if args.is_null() {
            0
        } else {
            unsafe { sequences::PyTuple_Size(args) }
        };
        if positional < 0 {
            return None;
        }
        let keywords = if kwargs.is_null() {
            0
        } else {
            unsafe { mapping::PyDict_Size(kwargs) }
        };
        if keywords < 0 {
            return None;
        }
        let Some(count) = (keywords as usize)
            .checked_mul(2)
            .and_then(|count| count.checked_add(positional as usize))
        else {
            unsafe { errors::PyErr_NoMemory() };
            return None;
        };
        if !operands.reserve_arguments(count) {
            return None;
        }
        if !args.is_null() {
            for index in 0..positional {
                let value = unsafe { sequences::PyTuple_GetItem(args, index) };
                if !unsafe { operands.push_argument(value) } {
                    return None;
                }
            }
        }
        if !kwargs.is_null() {
            let (mut pos, mut key, mut value) = (0, ptr::null_mut(), ptr::null_mut());
            while unsafe {
                mapping::PyDict_Next(kwargs, &raw mut pos, &raw mut key, &raw mut value)
            } != 0
            {
                if !unsafe { operands.push_argument(key) }
                    || !unsafe { operands.push_argument(value) }
                {
                    return None;
                }
            }
        }
        Some(operands)
    }

    fn synchronize(&self) -> bool {
        let mut synchronized = true;
        // Keep the first error in its original channel. Error existence is not
        // projection success: an emergency runtime error has no C instance.
        // Later observations run with both exact channels preserved on their
        // own stacks, so one failure never skips publication of other operands.
        for &operand in self
            .roots
            .iter()
            .chain(&self.inline[..self.inline_len])
            .chain(&self.spill)
        {
            if operand.is_null() || GLOBAL_BRIDGE.molt_handle_for_pyobj(operand).is_none() {
                continue;
            }
            let observe = || {
                let observed = GLOBAL_BRIDGE.observed_handle_for_pyobj(operand).is_some();
                if !observed {
                    unsafe {
                        crate::bridge::ensure_result_error(
                            c"native callback operand synchronization failed",
                        )
                    };
                }
                observed && !errors::raised_error_pending()
            };
            let observed = if errors::raised_error_pending() {
                errors::with_preserved_error(observe)
            } else {
                observe()
            };
            synchronized &= observed;
        }
        synchronized
    }

    pub(crate) unsafe fn complete_result(
        &self,
        result: *mut PyObject,
        operation: &str,
    ) -> *mut PyObject {
        let result = unsafe { errors::check_native_result(result, operation) };
        if self.synchronize() {
            result
        } else {
            unsafe { errors::release_preserving_error(&[result]) };
            ptr::null_mut()
        }
    }

    pub(crate) unsafe fn complete_status(&self, status: c_int, operation: &str) -> c_int {
        let status = unsafe { errors::check_native_status(status, operation) };
        if self.synchronize() { status } else { -1 }
    }
}

impl<const N: usize> Drop for CallbackOperands<N> {
    fn drop(&mut self) {
        unsafe {
            errors::release_preserving_error(&self.spill);
            errors::release_preserving_error(&self.inline[..self.inline_len]);
            errors::release_preserving_error(&self.roots);
        }
    }
}
