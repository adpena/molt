use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};

use super::backing::{tracked_vec_box_from_raw, tracked_vec_box_with_capacity};
use crate::{
    Buffer2D, MoltHeader, MoltObject, ObjectAuxPreselection, PyToken, TYPE_ID_BUFFER2D,
    buffer2d_ptr, dec_ref_bits, exception_pending, inc_ref_bits, obj_from_bits, object_type_id,
    raise_exception,
};

pub(crate) enum Buffer2DStorage {
    I64(*mut Vec<i64>),
    Boxed(*mut Vec<u64>),
}

pub(crate) unsafe fn visit_owned_values(buffer: *mut Buffer2D, mut visit: impl FnMut(u64)) {
    if buffer.is_null() {
        return;
    }
    if let Buffer2DStorage::Boxed(values) = unsafe { &(*buffer).data }
        && !values.is_null()
    {
        for &bits in unsafe { &**values } {
            visit(bits);
        }
    }
}

pub(crate) unsafe fn detach_owned_values(buffer: *mut Buffer2D, mut detach: impl FnMut(u64)) {
    if buffer.is_null() {
        return;
    }
    if let Buffer2DStorage::Boxed(values) = unsafe { &mut (*buffer).data }
        && !values.is_null()
    {
        for bits in unsafe { &mut **values } {
            detach(std::mem::replace(bits, MoltObject::none().bits()));
        }
    }
}

enum ExactInt {
    I64(i64),
    Big(BigInt),
}

fn memory_error<T: crate::ExceptionSentinel>(_py: &PyToken<'_>) -> T {
    raise_exception::<T>(_py, "MemoryError", "buffer2d allocation failed")
}

fn dimension_from_bits(_py: &PyToken<'_>, bits: u64, name: &str) -> Option<u64> {
    if let Some(value) = crate::builtins::numbers::index_i64_integral_bits(bits) {
        if value < 0 {
            let message = format!("{name} must be non-negative");
            return raise_exception::<Option<u64>>(_py, "ValueError", &message);
        }
        if value > isize::MAX as i64 {
            let message = format!("{name} is too large");
            return raise_exception::<Option<u64>>(_py, "OverflowError", &message);
        }
        return Some(value as u64);
    }
    let err = format!("{name} must be an integer");
    let value = crate::builtins::numbers::index_bigint_from_obj(_py, bits, &err)?;
    if value.is_negative() {
        let message = format!("{name} must be non-negative");
        return raise_exception::<Option<u64>>(_py, "ValueError", &message);
    }
    value.to_isize().map(|value| value as u64).or_else(|| {
        let message = format!("{name} is too large");
        raise_exception::<Option<u64>>(_py, "OverflowError", &message)
    })
}

fn index_from_i64(_py: &PyToken<'_>, value: i64, len: u64) -> Option<usize> {
    let len_i64 = i64::try_from(len).expect("Buffer2D dimensions are bounded by i64::MAX");
    let normalized = if value < 0 { value + len_i64 } else { value };
    if normalized < 0 || normalized >= len_i64 {
        return raise_exception::<Option<usize>>(_py, "IndexError", "buffer2d index out of range");
    }
    usize::try_from(normalized).ok().or_else(|| {
        raise_exception::<Option<usize>>(_py, "IndexError", "buffer2d index out of range")
    })
}

fn index_from_bits(_py: &PyToken<'_>, bits: u64, len: u64) -> Option<usize> {
    if let Some(value) = crate::builtins::numbers::index_i64_integral_bits(bits) {
        return index_from_i64(_py, value, len);
    }
    let error = format!(
        "'{}' object cannot be interpreted as an integer",
        crate::type_name(_py, obj_from_bits(bits))
    );
    let value = crate::builtins::numbers::index_bigint_from_obj(_py, bits, &error)?;
    let len_big = BigInt::from(len);
    let normalized = if value.is_negative() {
        value + &len_big
    } else {
        value
    };
    if normalized.is_negative() || normalized >= len_big {
        return raise_exception::<Option<usize>>(_py, "IndexError", "buffer2d index out of range");
    }
    normalized.to_usize().or_else(|| {
        raise_exception::<Option<usize>>(_py, "IndexError", "buffer2d index out of range")
    })
}

fn integer_from_bits(_py: &PyToken<'_>, bits: u64, name: &str) -> Option<ExactInt> {
    if let Some(value) = crate::builtins::numbers::index_i64_integral_bits(bits) {
        return Some(ExactInt::I64(value));
    }
    let message = format!("{name} must be an integer");
    let value = crate::builtins::numbers::index_bigint_from_obj(_py, bits, &message)?;
    if let Some(value) = value.to_i64() {
        Some(ExactInt::I64(value))
    } else {
        Some(ExactInt::Big(value))
    }
}

unsafe fn drop_i64_storage(ptr: *mut Vec<i64>) {
    if !ptr.is_null() {
        drop(unsafe { tracked_vec_box_from_raw(ptr) });
    }
}

unsafe fn drop_boxed_storage(_py: &PyToken<'_>, ptr: *mut Vec<u64>) {
    if ptr.is_null() {
        return;
    }
    let owner = unsafe { tracked_vec_box_from_raw(ptr) };
    for &bits in owner.iter() {
        dec_ref_bits(_py, bits);
    }
    drop(owner);
}

unsafe fn drop_staged(_py: &PyToken<'_>, storage: Buffer2DStorage) {
    match storage {
        Buffer2DStorage::I64(ptr) => unsafe { drop_i64_storage(ptr) },
        Buffer2DStorage::Boxed(ptr) => unsafe { drop_boxed_storage(_py, ptr) },
    }
}

fn allocate_i64_storage(_py: &PyToken<'_>, len: usize, init: i64) -> Option<*mut Vec<i64>> {
    let Some(ptr) = tracked_vec_box_with_capacity::<i64>(len) else {
        return memory_error(_py);
    };
    unsafe {
        (*ptr).resize(len, init);
    }
    Some(ptr)
}

fn owned_int_bits(_py: &PyToken<'_>, value: BigInt) -> Option<u64> {
    let bits = crate::builtins::numbers::int_bits_from_bigint(_py, value);
    if exception_pending(_py) {
        dec_ref_bits(_py, bits);
        None
    } else {
        debug_assert!(super::builders::acyclic_slot_edge(
            super::heap_kinds_generated::HeapAcyclicSlot::Buffer2dCell,
            bits,
        ));
        Some(bits)
    }
}

fn owned_i64_bits(_py: &PyToken<'_>, value: i64) -> Option<u64> {
    let bits = crate::builtins::numbers::int_bits_from_i64(_py, value);
    if exception_pending(_py) {
        dec_ref_bits(_py, bits);
        None
    } else {
        debug_assert!(super::builders::acyclic_slot_edge(
            super::heap_kinds_generated::HeapAcyclicSlot::Buffer2dCell,
            bits,
        ));
        Some(bits)
    }
}

fn owned_exact_bits(_py: &PyToken<'_>, value: ExactInt) -> Option<u64> {
    match value {
        ExactInt::I64(value) => owned_i64_bits(_py, value),
        ExactInt::Big(value) => owned_int_bits(_py, value),
    }
}

fn allocate_boxed_storage(_py: &PyToken<'_>, len: usize, init: BigInt) -> Option<*mut Vec<u64>> {
    let Some(ptr) = tracked_vec_box_with_capacity::<u64>(len) else {
        return memory_error(_py);
    };
    let Some(bits) = owned_int_bits(_py, init) else {
        unsafe { drop_boxed_storage(_py, ptr) };
        return None;
    };
    unsafe {
        for _ in 0..len {
            inc_ref_bits(_py, bits);
            (*ptr).push(bits);
        }
    }
    dec_ref_bits(_py, bits);
    Some(ptr)
}

fn allocate_storage(_py: &PyToken<'_>, len: usize, init: ExactInt) -> Option<Buffer2DStorage> {
    if len == 0 {
        return allocate_i64_storage(_py, 0, 0).map(Buffer2DStorage::I64);
    }
    match init {
        ExactInt::I64(value) => allocate_i64_storage(_py, len, value).map(Buffer2DStorage::I64),
        ExactInt::Big(value) => allocate_boxed_storage(_py, len, value).map(Buffer2DStorage::Boxed),
    }
}

fn publish_buffer(_py: &PyToken<'_>, rows: u64, cols: u64, storage: Buffer2DStorage) -> u64 {
    let Some(total) =
        std::mem::size_of::<MoltHeader>().checked_add(std::mem::size_of::<Buffer2D>())
    else {
        unsafe { drop_staged(_py, storage) };
        return memory_error(_py);
    };
    let ptr = crate::object::alloc_object_zeroed_unpublished_with_aux(
        _py,
        total,
        TYPE_ID_BUFFER2D,
        ObjectAuxPreselection::Default,
    );
    if ptr.is_null() {
        unsafe { drop_staged(_py, storage) };
        return MoltObject::none().bits();
    }
    unsafe {
        std::ptr::write(
            ptr.cast::<Buffer2D>(),
            Buffer2D {
                rows,
                cols,
                data: storage,
            },
        );
        crate::object::gc::gc_publish_initialized(_py, ptr);
    }
    MoltObject::from_ptr(ptr).bits()
}

fn checked_buffer(_py: &PyToken<'_>, bits: u64) -> Option<*mut Buffer2D> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return raise_exception::<Option<*mut Buffer2D>>(
            _py,
            "TypeError",
            "expected a Buffer2D object",
        );
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_BUFFER2D {
            return raise_exception::<Option<*mut Buffer2D>>(
                _py,
                "TypeError",
                "expected a Buffer2D object",
            );
        }
        Some(buffer2d_ptr(ptr))
    }
}

unsafe fn exact_at(storage: &Buffer2DStorage, index: usize) -> ExactInt {
    match storage {
        Buffer2DStorage::I64(ptr) => ExactInt::I64(unsafe { (&**ptr)[index] }),
        Buffer2DStorage::Boxed(ptr) => {
            let bits = unsafe { (&**ptr)[index] };
            ExactInt::Big(
                crate::builtins::numbers::index_bigint_integral_bits(bits)
                    .expect("Buffer2D boxed storage contains only integers"),
            )
        }
    }
}

fn add_product(acc: ExactInt, left: ExactInt, right: ExactInt) -> ExactInt {
    match (acc, left, right) {
        (ExactInt::I64(acc), ExactInt::I64(left), ExactInt::I64(right)) => left
            .checked_mul(right)
            .and_then(|product| acc.checked_add(product))
            .map(ExactInt::I64)
            .unwrap_or_else(|| {
                ExactInt::Big(BigInt::from(acc) + BigInt::from(left) * BigInt::from(right))
            }),
        (acc, left, right) => {
            let acc = match acc {
                ExactInt::I64(value) => BigInt::from(value),
                ExactInt::Big(value) => value,
            };
            let left = match left {
                ExactInt::I64(value) => BigInt::from(value),
                ExactInt::Big(value) => value,
            };
            let right = match right {
                ExactInt::I64(value) => BigInt::from(value),
                ExactInt::Big(value) => value,
            };
            ExactInt::Big(acc + left * right)
        }
    }
}

unsafe fn compute_cell(
    a: &Buffer2D,
    b: &Buffer2D,
    row: usize,
    col: usize,
    a_cols: usize,
    b_cols: usize,
) -> ExactInt {
    let mut acc = ExactInt::I64(0);
    for k in 0..a_cols {
        let left = unsafe { exact_at(&a.data, row * a_cols + k) };
        let right = unsafe { exact_at(&b.data, k * b_cols + col) };
        acc = add_product(acc, left, right);
    }
    match acc {
        ExactInt::Big(value) if value.to_i64().is_some() => {
            ExactInt::I64(value.to_i64().expect("checked above"))
        }
        value => value,
    }
}

fn allocate_boxed_values(
    _py: &PyToken<'_>,
    capacity: usize,
    values: impl IntoIterator<Item = ExactInt>,
) -> Option<*mut Vec<u64>> {
    let Some(boxed_ptr) = tracked_vec_box_with_capacity::<u64>(capacity) else {
        memory_error::<Option<()>>(_py);
        return None;
    };
    unsafe {
        for value in values {
            let Some(bits) = owned_exact_bits(_py, value) else {
                drop_boxed_storage(_py, boxed_ptr);
                return None;
            };
            (*boxed_ptr).push(bits);
        }
    }
    Some(boxed_ptr)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffer2d_new(rows_bits: u64, cols_bits: u64, init_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(rows) = dimension_from_bits(_py, rows_bits, "rows") else {
            return MoltObject::none().bits();
        };
        let Some(cols) = dimension_from_bits(_py, cols_bits, "cols") else {
            return MoltObject::none().bits();
        };
        let Some(init) = integer_from_bits(_py, init_bits, "init") else {
            return MoltObject::none().bits();
        };
        let Some(element_count) = rows.checked_mul(cols) else {
            return memory_error(_py);
        };
        let Ok(len) = usize::try_from(element_count) else {
            return memory_error(_py);
        };
        let Some(storage) = allocate_storage(_py, len, init) else {
            return MoltObject::none().bits();
        };
        publish_buffer(_py, rows, cols, storage)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffer2d_get(obj_bits: u64, row_bits: u64, col_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(buf_ptr) = checked_buffer(_py, obj_bits) else {
            return MoltObject::none().bits();
        };
        let (rows, cols) = unsafe { ((*buf_ptr).rows, (*buf_ptr).cols) };
        let Some(row) = index_from_bits(_py, row_bits, rows) else {
            return MoltObject::none().bits();
        };
        let Some(col) = index_from_bits(_py, col_bits, cols) else {
            return MoltObject::none().bits();
        };
        let cols = usize::try_from(cols).expect("non-empty Buffer2D columns fit usize");
        let index = row * cols + col;
        match unsafe { &(*buf_ptr).data } {
            Buffer2DStorage::I64(ptr) => owned_i64_bits(_py, unsafe { (&**ptr)[index] })
                .unwrap_or_else(|| MoltObject::none().bits()),
            Buffer2DStorage::Boxed(ptr) => {
                let bits = unsafe { (&**ptr)[index] };
                inc_ref_bits(_py, bits);
                bits
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffer2d_rows(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(buf_ptr) = checked_buffer(_py, obj_bits) else {
            return MoltObject::none().bits();
        };
        owned_i64_bits(_py, unsafe { (*buf_ptr).rows as i64 })
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffer2d_cols(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(buf_ptr) = checked_buffer(_py, obj_bits) else {
            return MoltObject::none().bits();
        };
        owned_i64_bits(_py, unsafe { (*buf_ptr).cols as i64 })
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffer2d_set(
    obj_bits: u64,
    row_bits: u64,
    col_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(buf_ptr) = checked_buffer(_py, obj_bits) else {
            return MoltObject::none().bits();
        };
        let (rows, cols) = unsafe { ((*buf_ptr).rows, (*buf_ptr).cols) };
        let Some(value) = integer_from_bits(_py, val_bits, "value") else {
            return MoltObject::none().bits();
        };
        let Some(row) = index_from_bits(_py, row_bits, rows) else {
            return MoltObject::none().bits();
        };
        let Some(col) = index_from_bits(_py, col_bits, cols) else {
            return MoltObject::none().bits();
        };
        let cols = usize::try_from(cols).expect("non-empty Buffer2D columns fit usize");
        let index = row * cols + col;
        let mut promoted = None;
        match unsafe { &mut (*buf_ptr).data } {
            Buffer2DStorage::I64(ptr) => match value {
                ExactInt::I64(value) => {
                    unsafe { (&mut **ptr)[index] = value };
                }
                ExactInt::Big(value) => {
                    let old_ptr = *ptr;
                    let old = unsafe { &*old_ptr };
                    let values = old[..index]
                        .iter()
                        .copied()
                        .map(ExactInt::I64)
                        .chain(std::iter::once(ExactInt::Big(value)))
                        .chain(old[index + 1..].iter().copied().map(ExactInt::I64));
                    let Some(boxed_ptr) = allocate_boxed_values(_py, old.len(), values) else {
                        return MoltObject::none().bits();
                    };
                    promoted = Some((old_ptr, boxed_ptr));
                }
            },
            Buffer2DStorage::Boxed(ptr) => {
                let Some(bits) = owned_exact_bits(_py, value) else {
                    return MoltObject::none().bits();
                };
                let old_bits = unsafe { std::mem::replace(&mut (&mut **ptr)[index], bits) };
                dec_ref_bits(_py, old_bits);
            }
        }
        if let Some((old_ptr, boxed_ptr)) = promoted {
            unsafe {
                (*buf_ptr).data = Buffer2DStorage::Boxed(boxed_ptr);
                drop_i64_storage(old_ptr);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffer2d_matmul(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a_ptr) = checked_buffer(_py, a_bits) else {
            return MoltObject::none().bits();
        };
        let Some(b_ptr) = checked_buffer(_py, b_bits) else {
            return MoltObject::none().bits();
        };
        let a = unsafe { &*a_ptr };
        let b = unsafe { &*b_ptr };
        if a.cols != b.rows {
            return raise_exception::<_>(_py, "ValueError", "matmul dimension mismatch");
        }
        let Some(element_count) = a.rows.checked_mul(b.cols) else {
            return memory_error(_py);
        };
        let Ok(len) = usize::try_from(element_count) else {
            return memory_error(_py);
        };
        if len == 0 {
            let Some(storage) = allocate_i64_storage(_py, 0, 0).map(Buffer2DStorage::I64) else {
                return MoltObject::none().bits();
            };
            return publish_buffer(_py, a.rows, b.cols, storage);
        }
        let (Ok(rows), Ok(a_cols), Ok(b_cols)) = (
            usize::try_from(a.rows),
            usize::try_from(a.cols),
            usize::try_from(b.cols),
        ) else {
            return memory_error(_py);
        };
        let Some(i64_ptr) = tracked_vec_box_with_capacity::<i64>(len) else {
            return memory_error(_py);
        };
        let mut staged = Buffer2DStorage::I64(i64_ptr);
        for row in 0..rows {
            for col in 0..b_cols {
                let value = unsafe { compute_cell(a, b, row, col, a_cols, b_cols) };
                match (&mut staged, value) {
                    (Buffer2DStorage::I64(ptr), ExactInt::I64(value)) => unsafe {
                        (**ptr).push(value);
                    },
                    (Buffer2DStorage::I64(ptr), ExactInt::Big(value)) => {
                        let old_ptr = *ptr;
                        let values = unsafe { &*old_ptr }
                            .iter()
                            .copied()
                            .map(ExactInt::I64)
                            .chain(std::iter::once(ExactInt::Big(value)));
                        let Some(boxed_ptr) = allocate_boxed_values(_py, len, values) else {
                            unsafe { drop_i64_storage(old_ptr) };
                            return MoltObject::none().bits();
                        };
                        unsafe {
                            drop_i64_storage(old_ptr);
                        }
                        staged = Buffer2DStorage::Boxed(boxed_ptr);
                    }
                    (Buffer2DStorage::Boxed(ptr), value) => {
                        let Some(bits) = owned_exact_bits(_py, value) else {
                            unsafe { drop_boxed_storage(_py, *ptr) };
                            return MoltObject::none().bits();
                        };
                        unsafe { (**ptr).push(bits) };
                    }
                }
            }
        }
        publish_buffer(_py, a.rows, b.cols, staged)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{
        OperationEstimate, ResourceError, ResourceTracker, UnlimitedTracker, set_tracker,
    };
    use std::{cell::Cell, rc::Rc};

    struct TrackerReset;

    impl Drop for TrackerReset {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    #[derive(Default)]
    struct TrackerUsage {
        used_allocations: Cell<usize>,
        used_bytes: Cell<usize>,
        max_allocations: Cell<Option<usize>>,
        remaining_allocations: Cell<Option<usize>>,
    }

    struct InspectableTracker {
        usage: Rc<TrackerUsage>,
    }

    impl ResourceTracker for InspectableTracker {
        fn on_allocate(&mut self, size: usize) -> Result<(), ResourceError> {
            if self.usage.remaining_allocations.get() == Some(0) {
                return Err(ResourceError::Allocation { count: 1, limit: 0 });
            }
            let allocations = self
                .usage
                .used_allocations
                .get()
                .checked_add(1)
                .expect("test tracker allocation count overflow");
            if let Some(limit) = self.usage.max_allocations.get()
                && allocations > limit
            {
                return Err(ResourceError::Allocation {
                    count: allocations,
                    limit,
                });
            }
            let bytes = self
                .usage
                .used_bytes
                .get()
                .checked_add(size)
                .expect("test tracker byte count overflow");
            self.usage.used_allocations.set(allocations);
            self.usage.used_bytes.set(bytes);
            if let Some(remaining) = self.usage.remaining_allocations.get() {
                self.usage.remaining_allocations.set(Some(remaining - 1));
            }
            Ok(())
        }

        fn on_free(&mut self, size: usize) {
            self.usage.used_allocations.set(
                self.usage
                    .used_allocations
                    .get()
                    .checked_sub(1)
                    .expect("Buffer2D rollback released an unowned allocation"),
            );
            self.usage.used_bytes.set(
                self.usage
                    .used_bytes
                    .get()
                    .checked_sub(size)
                    .expect("Buffer2D rollback released unowned bytes"),
            );
        }

        fn on_grow(&mut self, additional_bytes: usize) -> Result<(), ResourceError> {
            self.usage.used_bytes.set(
                self.usage
                    .used_bytes
                    .get()
                    .checked_add(additional_bytes)
                    .expect("test tracker byte count overflow"),
            );
            Ok(())
        }

        fn on_shrink(&mut self, released_bytes: usize) {
            self.usage.used_bytes.set(
                self.usage
                    .used_bytes
                    .get()
                    .checked_sub(released_bytes)
                    .expect("Buffer2D rollback released unowned growth bytes"),
            );
        }

        fn check_time(&mut self) -> Result<(), ResourceError> {
            Ok(())
        }

        fn check_recursion_depth(&mut self, _depth: usize) -> Result<(), ResourceError> {
            Ok(())
        }

        fn check_operation_size(&mut self, _op: &OperationEstimate) -> Result<(), ResourceError> {
            Ok(())
        }
    }

    fn install_inspectable_tracker() -> Rc<TrackerUsage> {
        let usage = Rc::new(TrackerUsage::default());
        set_tracker(Box::new(InspectableTracker {
            usage: Rc::clone(&usage),
        }));
        usage
    }

    fn inline(value: i64) -> u64 {
        MoltObject::from_int(value).bits()
    }

    #[test]
    fn small_storage_promotes_once_and_preserves_negative_indexing() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let buffer_bits = molt_buffer2d_new(inline(1), inline(2), inline(7));
            let buffer_ptr = checked_buffer(_py, buffer_bits).expect("buffer");
            assert!(matches!(
                unsafe { &(*buffer_ptr).data },
                Buffer2DStorage::I64(_)
            ));

            let large = BigInt::from(1u8) << 90usize;
            let large_bits = crate::builtins::numbers::int_bits_from_bigint(_py, large.clone());
            assert_eq!(
                molt_buffer2d_set(buffer_bits, inline(-1), inline(-1), large_bits),
                MoltObject::none().bits()
            );
            dec_ref_bits(_py, large_bits);
            assert!(matches!(
                unsafe { &(*buffer_ptr).data },
                Buffer2DStorage::Boxed(_)
            ));

            let result = molt_buffer2d_get(buffer_bits, inline(0), inline(1));
            assert_eq!(
                crate::builtins::numbers::index_bigint_integral_bits(result),
                Some(large)
            );
            dec_ref_bits(_py, result);
            dec_ref_bits(_py, buffer_bits);
        });
    }

    #[test]
    fn matmul_promotes_on_arithmetic_overflow_without_wrapping() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let value = BigInt::from(i64::MAX);
            let value_bits = crate::builtins::numbers::int_bits_from_bigint(_py, value.clone());
            let left = molt_buffer2d_new(inline(1), inline(1), value_bits);
            let right = molt_buffer2d_new(inline(1), inline(1), value_bits);
            dec_ref_bits(_py, value_bits);

            let product = molt_buffer2d_matmul(left, right);
            let result = molt_buffer2d_get(product, inline(0), inline(0));
            assert_eq!(
                crate::builtins::numbers::index_bigint_integral_bits(result),
                Some(&value * &value)
            );
            dec_ref_bits(_py, result);
            dec_ref_bits(_py, product);
            dec_ref_bits(_py, left);
            dec_ref_bits(_py, right);
        });
    }

    #[test]
    fn get_roundtrips_full_width_i64_without_matmul() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let max_bits = owned_i64_bits(_py, i64::MAX).expect("i64::MAX bits");
            let buffer_bits = molt_buffer2d_new(inline(1), inline(1), max_bits);
            dec_ref_bits(_py, max_bits);

            let buffer_ptr = checked_buffer(_py, buffer_bits).expect("buffer");
            assert!(matches!(
                unsafe { &(*buffer_ptr).data },
                Buffer2DStorage::I64(_)
            ));
            let result = molt_buffer2d_get(buffer_bits, inline(0), inline(0));
            let result_ptr = obj_from_bits(result)
                .as_ptr()
                .expect("i64::MAX must use canonical boxed integer storage");
            assert_eq!(unsafe { object_type_id(result_ptr) }, crate::TYPE_ID_BIGINT);
            assert_eq!(
                crate::builtins::numbers::index_i64_integral_bits(result),
                Some(i64::MAX)
            );
            dec_ref_bits(_py, result);
            dec_ref_bits(_py, buffer_bits);
        });
    }

    #[test]
    fn denied_object_publication_rolls_back_staged_backing() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let usage = install_inspectable_tracker();
            let _reset = TrackerReset;
            let sentinel = tracked_vec_box_with_capacity::<u8>(8).expect("sentinel");
            let baseline_allocations = usage.used_allocations.get();
            let baseline_bytes = usage.used_bytes.get();
            for init in [
                ExactInt::I64(0),
                ExactInt::Big(BigInt::from(1u8) << 90usize),
            ] {
                let storage = allocate_storage(_py, 4, init).expect("staged storage");
                // Deny publication after staging succeeds, independently of how
                // many allocations each storage representation needs.
                usage
                    .max_allocations
                    .set(Some(usage.used_allocations.get()));
                let denied = publish_buffer(_py, 2, 2, storage);
                assert_eq!(denied, MoltObject::none().bits());
                assert!(exception_pending(_py));
                crate::clear_exception(_py);
                assert_eq!(usage.used_allocations.get(), baseline_allocations);
                assert_eq!(usage.used_bytes.get(), baseline_bytes);
                usage.max_allocations.set(None);
            }

            usage.max_allocations.set(None);
            unsafe { drop(tracked_vec_box_from_raw(sentinel)) };
        });
    }

    #[test]
    fn denied_boxed_promotion_preserves_original_storage_and_cells() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let usage = install_inspectable_tracker();
            let _reset = TrackerReset;
            let sentinel = tracked_vec_box_with_capacity::<u8>(8).expect("sentinel");
            let max_bits = owned_i64_bits(_py, i64::MAX).expect("i64::MAX bits");
            let buffer_bits = molt_buffer2d_new(inline(1), inline(3), max_bits);
            dec_ref_bits(_py, max_bits);
            let buffer_ptr = checked_buffer(_py, buffer_bits).expect("buffer");
            let Buffer2DStorage::I64(original_storage) = (unsafe { &(*buffer_ptr).data }) else {
                panic!("small Buffer2D must start in packed storage");
            };
            let original_storage = *original_storage;
            let large = BigInt::from(1u8) << 90usize;
            let large_bits = crate::builtins::numbers::int_bits_from_bigint(_py, large.clone());
            let baseline_allocations = usage.used_allocations.get();
            let baseline_bytes = usage.used_bytes.get();
            // One tracked vector, then one integer per final cell. Denials
            // cover both sides of the replacement without boxing its old value.
            for allowed in 0..4 {
                usage
                    .max_allocations
                    .set(Some(baseline_allocations + allowed));
                assert_eq!(
                    molt_buffer2d_set(buffer_bits, inline(0), inline(1), large_bits),
                    MoltObject::none().bits()
                );
                assert!(exception_pending(_py), "allocation boundary {allowed}");
                crate::clear_exception(_py);
                assert_eq!(usage.used_allocations.get(), baseline_allocations);
                assert_eq!(usage.used_bytes.get(), baseline_bytes);

                let Buffer2DStorage::I64(values) = (unsafe { &(*buffer_ptr).data }) else {
                    panic!("failed promotion must leave packed storage authoritative");
                };
                assert_eq!(*values, original_storage);
                assert_eq!(unsafe { &***values }, &[i64::MAX, i64::MAX, i64::MAX]);
            }

            usage.max_allocations.set(Some(baseline_allocations + 4));
            molt_buffer2d_set(buffer_bits, inline(0), inline(1), large_bits);
            assert!(!exception_pending(_py));
            let Buffer2DStorage::Boxed(values) = (unsafe { &(*buffer_ptr).data }) else {
                panic!("successful promotion must publish boxed storage");
            };
            assert_eq!(
                crate::builtins::numbers::index_i64_integral_bits(unsafe { (&***values)[0] }),
                Some(i64::MAX)
            );
            assert_eq!(
                crate::builtins::numbers::index_bigint_integral_bits(unsafe { (&***values)[1] }),
                Some(large)
            );
            assert_eq!(
                crate::builtins::numbers::index_i64_integral_bits(unsafe { (&***values)[2] }),
                Some(i64::MAX)
            );

            usage.max_allocations.set(None);
            dec_ref_bits(_py, large_bits);
            dec_ref_bits(_py, buffer_bits);
            unsafe { drop(tracked_vec_box_from_raw(sentinel)) };
        });
    }

    #[test]
    fn denied_matmul_stages_preserve_inputs_and_release_partial_output() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let usage = install_inspectable_tracker();
            let _reset = TrackerReset;
            let sentinel = tracked_vec_box_with_capacity::<u8>(8).expect("sentinel");
            let max_bits = owned_i64_bits(_py, i64::MAX).expect("i64::MAX bits");
            let left = molt_buffer2d_new(inline(1), inline(1), max_bits);
            dec_ref_bits(_py, max_bits);
            let right = molt_buffer2d_new(inline(1), inline(3), inline(1));
            molt_buffer2d_set(right, inline(0), inline(1), inline(2));
            molt_buffer2d_set(right, inline(0), inline(2), inline(3));
            let left_ptr = checked_buffer(_py, left).expect("left");
            let right_ptr = checked_buffer(_py, right).expect("right");
            let baseline_allocations = usage.used_allocations.get();
            let baseline_bytes = usage.used_bytes.get();

            // Packed output, boxed replacement backing, three integer cells,
            // then object publication. A cumulative budget reaches the final
            // cell even after promotion releases the original packed backing.
            for allowed in 0..6 {
                usage.remaining_allocations.set(Some(allowed));
                assert_eq!(molt_buffer2d_matmul(left, right), MoltObject::none().bits());
                assert!(
                    exception_pending(_py),
                    "matmul allocation boundary {allowed}"
                );
                crate::clear_exception(_py);
                assert_eq!(usage.used_allocations.get(), baseline_allocations);
                assert_eq!(usage.used_bytes.get(), baseline_bytes);
                let Buffer2DStorage::I64(left_values) = (unsafe { &(*left_ptr).data }) else {
                    panic!("matmul must not promote its left input");
                };
                let Buffer2DStorage::I64(right_values) = (unsafe { &(*right_ptr).data }) else {
                    panic!("matmul must not promote its right input");
                };
                assert_eq!(unsafe { &***left_values }, &[i64::MAX]);
                assert_eq!(unsafe { &***right_values }, &[1, 2, 3]);
            }

            usage.remaining_allocations.set(Some(6));
            let product = molt_buffer2d_matmul(left, right);
            assert!(!exception_pending(_py));
            usage.remaining_allocations.set(None);
            for column in 0..3 {
                let bits = molt_buffer2d_get(product, inline(0), inline(column));
                assert_eq!(
                    crate::builtins::numbers::index_bigint_integral_bits(bits),
                    Some(BigInt::from(i64::MAX) * (column + 1))
                );
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, product);
            assert_eq!(usage.used_allocations.get(), baseline_allocations);
            assert_eq!(usage.used_bytes.get(), baseline_bytes);
            dec_ref_bits(_py, left);
            dec_ref_bits(_py, right);
            unsafe { drop(tracked_vec_box_from_raw(sentinel)) };
        });
    }

    #[test]
    fn empty_shapes_preserve_dimensions_without_dimension_sized_storage() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let max_bits = owned_i64_bits(_py, isize::MAX as i64).expect("dimension");
            for (rows, cols) in [(inline(0), max_bits), (max_bits, inline(0))] {
                let left = molt_buffer2d_new(rows, inline(0), inline(7));
                let right = molt_buffer2d_new(inline(0), cols, inline(9));
                let product = molt_buffer2d_matmul(left, right);
                assert!(!exception_pending(_py));
                let buffer_ptr = checked_buffer(_py, product).expect("empty product");
                assert_eq!(
                    unsafe { ((*buffer_ptr).rows, (*buffer_ptr).cols) },
                    (
                        crate::builtins::numbers::index_i64_integral_bits(rows).unwrap() as u64,
                        crate::builtins::numbers::index_i64_integral_bits(cols).unwrap() as u64,
                    )
                );
                let Buffer2DStorage::I64(values) = (unsafe { &(*buffer_ptr).data }) else {
                    panic!("empty storage must stay packed");
                };
                let cells = unsafe { &**values };
                assert!(cells.is_empty());
                assert_eq!(cells.capacity(), 0);
                dec_ref_bits(_py, product);
                dec_ref_bits(_py, left);
                dec_ref_bits(_py, right);
            }
            dec_ref_bits(_py, max_bits);
        });
    }

    #[test]
    fn wrong_object_is_a_type_error_instead_of_a_none_result() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let result = molt_buffer2d_get(MoltObject::none().bits(), inline(0), inline(0));
            assert_eq!(result, MoltObject::none().bits());
            assert!(exception_pending(_py));
            crate::clear_exception(_py);
        });
    }
}
