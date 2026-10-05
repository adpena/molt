use super::*;
use crate::object::buffer_exports::{BufferAccessError, ScopedBuffer};
use crate::object::memoryview::{
    MOLT_BUFFER_MAX_NDIM, MoltBufferView, memoryview_format_from_str, memoryview_read_scalar,
    memoryview_shape_product, memoryview_strided_bounds,
};

fn format(view: &MoltBufferView) -> Option<MemoryViewFormat> {
    let end = view
        .format
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(view.format.len());
    let text = std::str::from_utf8(&view.format[..end]).ok()?;
    let fmt = memoryview_format_from_str(text)?;
    (u64::try_from(fmt.itemsize).ok()? == view.itemsize).then_some(fmt)
}

fn element_count(view: &MoltBufferView) -> Option<usize> {
    let ndim = usize::try_from(view.ndim).ok()?;
    if ndim > MOLT_BUFFER_MAX_NDIM {
        return None;
    }
    let itemsize = usize::try_from(view.itemsize).ok()?;
    let shape = &view.shape[..ndim];
    let count = usize::try_from(memoryview_shape_product(shape)?).ok()?;
    if u64::try_from(count.checked_mul(itemsize)?).ok()? != view.len {
        return None;
    }
    let bounds = memoryview_strided_bounds(shape, &view.strides[..ndim], itemsize)?;
    if u64::try_from(bounds.span_len).ok()? > view.backing_capacity
        || (count != 0 && view.data.is_null())
    {
        return None;
    }
    Some(count)
}

fn scalar<'a, 'py>(
    py: &'a PyToken<'py>,
    view: &MoltBufferView,
    mut index: usize,
    fmt: MemoryViewFormat,
) -> Result<Pin<'a, 'py>, ()> {
    let mut offset = 0isize;
    for axis in (0..view.ndim as usize).rev() {
        let dim = usize::try_from(view.shape[axis]).map_err(|_| ())?;
        if dim == 0 {
            return Err(());
        }
        let coordinate = isize::try_from(index % dim).map_err(|_| ())?;
        index /= dim;
        offset = coordinate
            .checked_mul(view.strides[axis])
            .and_then(|delta| offset.checked_add(delta))
            .ok_or(())?;
    }
    // The counted lease and geometry validation prove the address. Copy the
    // scalar to the stack before decoding can allocate or invoke finalizers.
    let mut bytes = [0u8; 8];
    if fmt.itemsize > bytes.len() {
        return Err(());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            view.data.offset(offset).cast_const(),
            bytes.as_mut_ptr(),
            fmt.itemsize,
        );
        let bits = memoryview_read_scalar(py, &bytes[..fmt.itemsize], 0, fmt).ok_or(())?;
        let value = Pin::adopt(py, bits);
        if exception_pending(py) {
            Err(())
        } else {
            Ok(value)
        }
    }
}

pub(super) fn compare(
    py: &PyToken<'_>,
    left: MoltObject,
    right: MoltObject,
    op: RichCompareOp,
) -> CompareValueOutcome {
    if !op.is_equality() {
        return CompareValueOutcome::NotComparable;
    }
    let released = |value: MoltObject| {
        physical_type(value) == Some(TYPE_ID_MEMORYVIEW)
            && unsafe { memoryview_released(value.as_ptr().unwrap()) }
    };
    if released(left) || released(right) {
        return equality(Ok(left.bits() == right.bits()), op);
    }
    let left = match ScopedBuffer::new(py, left.bits()) {
        Ok(buffer) => buffer,
        Err(BufferAccessError::Pending) => return CompareValueOutcome::Error,
        Err(BufferAccessError::Invalid) => return CompareValueOutcome::NotComparable,
    };
    let right_is_view = physical_type(right) == Some(TYPE_ID_MEMORYVIEW);
    let right = match ScopedBuffer::new(py, right.bits()) {
        Ok(buffer) => buffer,
        Err(BufferAccessError::Pending) => {
            if right_is_view {
                return CompareValueOutcome::Error;
            }
            // A non-view export refusal is a declined comparison in CPython,
            // including exporters whose acquisition raises an exception.
            crate::builtins::exceptions::exception_clear_reason_set(
                "memoryview comparison exporter refusal",
            );
            molt_exception_clear();
            return CompareValueOutcome::NotComparable;
        }
        Err(BufferAccessError::Invalid) => return CompareValueOutcome::NotComparable,
    };
    let compare = || -> Result<bool, ()> {
        let (left, right) = (left.view(), right.view());
        let Some(lcount) = element_count(left) else {
            raise_exception::<()>(py, "BufferError", "invalid comparison buffer geometry");
            return Err(());
        };
        let Some(rcount) = element_count(right) else {
            raise_exception::<()>(py, "BufferError", "invalid comparison buffer geometry");
            return Err(());
        };
        if left.ndim != right.ndim || lcount != rcount {
            return Ok(false);
        }
        // Empty suffix dimensions are unobservable: [1, 0, 5] and [1, 0, 7]
        // describe the same zero-element logical shape in CPython.
        for axis in 0..left.ndim as usize {
            if left.shape[axis] != right.shape[axis] {
                return Ok(false);
            }
            if left.shape[axis] == 0 {
                break;
            }
        }
        // Unsupported formats compare unequal even for the same exporter.
        let (Some(lfmt), Some(rfmt)) = (format(left), format(right)) else {
            return Ok(false);
        };
        for index in 0..lcount {
            let lhs = scalar(py, left, index, lfmt)?;
            let rhs = scalar(py, right, index, rfmt)?;
            // Buffer elements have value semantics, including NaN != NaN.
            // The container identity-or-equality shortcut is inapplicable.
            let equal = comparison_value_to_bool(
                py,
                compare_object_eq_value(py, obj_from_bits(lhs.bits), obj_from_bits(rhs.bits)),
            );
            drop(lhs);
            drop(rhs);
            if exception_pending(py) {
                return Err(());
            }
            match equal {
                CompareBoolOutcome::True => {}
                CompareBoolOutcome::False => return Ok(false),
                _ => return Err(()),
            }
        }
        Ok(true)
    };
    equality(compare(), op)
}
