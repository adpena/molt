//! The runtime-owned MemoryError reserve. Free objects carry ordinary owners;
//! acquiring one transfers that owner, so a live exception is never pinned by
//! the reserve. Only the canonical terminal transition may refill the reserve.

use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8};

// CPython 3.12--3.14 Objects/exceptions.c, MEMERRORS_SAVE.
const CAPACITY: usize = 16;
const COLD: u8 = 0;
const READY: u8 = 1;
const DRAINING: u8 = 2;
const CLOSED: u8 = 3;

struct FreeMemoryErrors {
    values: [u64; CAPACITY],
    len: usize,
    returning: usize,
}

pub(super) struct MemoryErrorReserve {
    phase: AtomicU8,
    c_views_ready: AtomicBool,
    class: AtomicU64,
    empty_args: AtomicU64,
    last_resort: AtomicU64,
    free: Mutex<FreeMemoryErrors>,
}

impl MemoryErrorReserve {
    pub(super) fn new() -> Self {
        Self {
            phase: AtomicU8::new(COLD),
            c_views_ready: AtomicBool::new(false),
            class: AtomicU64::new(0),
            empty_args: AtomicU64::new(0),
            last_resort: AtomicU64::new(0),
            free: Mutex::new(FreeMemoryErrors {
                values: [0; CAPACITY],
                len: 0,
                returning: 0,
            }),
        }
    }
}

fn reserve(py: &PyToken<'_>) -> &'static MemoryErrorReserve {
    &exceptions_state(py).memory_error
}

/// Called once for each runtime, before primary Ready or isolate application
/// bootstrap. Allocation failure before this authority exists is an admission
/// failure, just as CPython rejects PyErr_NoMemory before exception bootstrap.
pub(crate) fn initialize(py: &PyToken<'_>) {
    let reserve = reserve(py);
    assert_eq!(reserve.phase.load(AtomicOrdering::Acquire), COLD);
    let class = exception_type_bits_from_name(py, "MemoryError");
    assert_ne!(class, 0, "MemoryError class bootstrap failed");
    let args = alloc_tuple(py, &[]);
    assert!(
        !args.is_null(),
        "MemoryError empty arguments bootstrap failed"
    );
    let args = MoltObject::from_ptr(args).bits();
    reserve.class.store(class, AtomicOrdering::Release);
    reserve.empty_args.store(args, AtomicOrdering::Release);
    let none = MoltObject::none().bits();
    let fallback = alloc_exception_obj(py, class, exception_field_missing_bits(), args, none);
    assert!(!fallback.is_null(), "MemoryError fallback bootstrap failed");
    reserve.last_resort.store(
        MoltObject::from_ptr(fallback).bits(),
        AtomicOrdering::Release,
    );
    for index in 0..CAPACITY {
        let pointer = alloc_exception_obj(py, class, exception_field_missing_bits(), args, none);
        assert!(!pointer.is_null(), "MemoryError reserve bootstrap failed");
        let mut free = reserve.free.lock().unwrap();
        free.values[index] = MoltObject::from_ptr(pointer).bits();
        free.len = index + 1;
    }
    reserve.phase.store(READY, AtomicOrdering::Release);
}

/// Prepare the already owned values only after the primary runtime installs
/// its real hook/class authority. Isolates never enter process C projection.
pub(crate) fn prepare_c_views(py: &PyToken<'_>) -> bool {
    let reserve = reserve(py);
    let values = reserve.free.lock().unwrap().values;
    let fallback = reserve.last_resort.load(AtomicOrdering::Acquire);
    for bits in std::iter::once(fallback).chain(values) {
        if unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) }
            .is_null()
        {
            return false;
        }
    }
    reserve.c_views_ready.store(true, AtomicOrdering::Release);
    true
}

fn take_free(reserve: &MemoryErrorReserve) -> Option<u64> {
    let mut free = reserve.free.lock().unwrap();
    if free.len == 0 {
        return None;
    }
    free.len -= 1;
    let index = free.len;
    Some(std::mem::replace(&mut free.values[index], 0))
}

/// Exact constructors share the same freelist. Subclasses continue through
/// their existing allocation authority and never enter this reserve.
pub(super) fn take_for_constructor(py: &PyToken<'_>, class: u64) -> Option<*mut u8> {
    let reserve = reserve(py);
    if reserve.phase.load(AtomicOrdering::Acquire) != READY
        || reserve.class.load(AtomicOrdering::Acquire) != class
    {
        return None;
    }
    take_free(reserve).and_then(|bits| obj_from_bits(bits).as_ptr())
}

/// One existing owned exception, obtained without allocation or callbacks.
/// Exhausting the reserve uses the same persistent last-resort object, as in
/// CPython; retained reserve objects are never reset or shared prematurely.
pub(super) fn acquire(py: &PyToken<'_>) -> u64 {
    let reserve = reserve(py);
    if reserve.phase.load(AtomicOrdering::Acquire) == READY
        && let Some(bits) = take_free(reserve)
    {
        return bits;
    }
    let fallback = reserve.last_resort.load(AtomicOrdering::Acquire);
    if fallback == 0 {
        eprintln!(
            "molt fatal: out of memory before MemoryError initialization or after retirement"
        );
        std::process::abort();
    }
    inc_ref_bits(py, fallback);
    fallback
}

/// Every exact object eligible to replenish a primary-runtime reserve needs a
/// prepared view while allocation is allowed. A failed projection follows the
/// existing constructor failure path; it does not recursively refill the pool.
pub(super) fn prepare_constructed(py: &PyToken<'_>, pointer: *mut u8, class: u64) -> bool {
    let reserve = reserve(py);
    if class != reserve.class.load(AtomicOrdering::Acquire)
        || !reserve.c_views_ready.load(AtomicOrdering::Acquire)
    {
        return true;
    }
    let bits = MoltObject::from_ptr(pointer).bits();
    let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    let already_projected =
        unsafe { (*header_from_obj_ptr(pointer)).has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW) };
    let projected = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
    !projected.is_null() && (!already_projected || bridge.refresh_exception_view(bits))
}

fn detach_and_reset(
    py: &PyToken<'_>,
    pointer: *mut u8,
) -> (
    DetachedExceptionEdges,
    Option<molt_cpython_abi::bridge::RetiredOwnedCFields>,
) {
    let detached = unsafe { exception_detach_owned_edges(pointer) };
    let projection =
        if unsafe { (*header_from_obj_ptr(pointer)).has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW) } {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .clear_owned_c_fields(MoltObject::from_ptr(pointer).bits())
        } else {
            None
        };
    let args = reserve(py).empty_args.load(AtomicOrdering::Acquire);
    inc_ref_bits(py, args);
    unsafe {
        *pointer.cast::<u64>().add(EXCEPTION_ARGS_SLOT) = args;
        *pointer.cast::<u64>().add(EXCEPTION_SUPPRESS_SLOT) = MoltObject::from_bool(false).bits();
    }
    (detached, projection)
}

fn release_detached(
    py: &PyToken<'_>,
    detached: DetachedExceptionEdges,
    projection: Option<molt_cpython_abi::bridge::RetiredOwnedCFields>,
) {
    if let Some(projection) = projection {
        crate::object::foreign::retire_native_reference(|| drop(projection));
    }
    for bits in detached.bits[..detached.len].iter().copied() {
        dec_ref_bits(py, bits);
    }
}

/// The caller is the existing rc-zero authority, after its finalizer/weakref
/// revival checks and before it retires the stable view or physical storage.
/// No new identity is created, no lock spans Python retirement, and the pool
/// receives a normal owner only after all previous mutable state is gone.
pub(crate) unsafe fn try_recycle(py: &PyToken<'_>, pointer: *mut u8) -> bool {
    let reserve = reserve(py);
    let header = unsafe { &*header_from_obj_ptr(pointer) };
    if reserve.phase.load(AtomicOrdering::Acquire) != READY
        || unsafe { object_class_bits(pointer) } != reserve.class.load(AtomicOrdering::Acquire)
        || header.has_flag(crate::object::HEADER_FLAG_DEALLOCATING)
    {
        return false;
    }
    let has_view = header.has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW);
    if reserve.c_views_ready.load(AtomicOrdering::Acquire) && !has_view {
        return false;
    }
    {
        let mut free = reserve.free.lock().unwrap();
        if free.len + free.returning == CAPACITY {
            return false;
        }
        free.returning += 1;
    }
    let bits = MoltObject::from_ptr(pointer).bits();
    let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    if has_view {
        bridge.begin_finalization(bits);
    }
    let window = header.open_revival_window(has_view);
    let baseline = window.baseline();
    let (detached, projection) = detach_and_reset(py, pointer);
    release_detached(py, detached, projection);
    let resurrected = header.ref_count_snapshot() > baseline;
    let native_resurrected = has_view && bridge.has_direct_c_refs(bits);
    if resurrected || native_resurrected {
        header.close_revival_window(window);
        if has_view {
            bridge.finish_finalization(bits, resurrected);
        }
        reserve.free.lock().unwrap().returning -= 1;
        return true;
    }
    // This new ordinary reference belongs to the free pool, not to a live
    // exception's hidden lifetime pin. Closing the internal window leaves it.
    header.retain_owned(1, "MemoryError reserve return");
    header.close_revival_window(window);
    if has_view {
        bridge.finish_finalization(bits, true);
        assert!(
            bridge.refresh_exception_view(bits),
            "cleared MemoryError view must use prepared immutable fields"
        );
    }
    let returned = {
        let mut free = reserve.free.lock().unwrap();
        free.returning -= 1;
        if reserve.phase.load(AtomicOrdering::Acquire) == READY {
            let index = free.len;
            free.values[index] = bits;
            free.len += 1;
            true
        } else {
            false
        }
    };
    if !returned {
        dec_ref_bits(py, bits);
    }
    true
}

/// Stop refill before shutdown callbacks. The last-resort owner remains usable
/// until the existing callback fixed point has drained both raised channels.
pub(crate) fn begin_shutdown(py: &PyToken<'_>) {
    let reserve = reserve(py);
    reserve.phase.store(DRAINING, AtomicOrdering::Release);
    let values = {
        let mut free = reserve.free.lock().unwrap();
        free.len = 0;
        std::mem::replace(&mut free.values, [0; CAPACITY])
    };
    for bits in values {
        if bits != 0 {
            dec_ref_bits(py, bits);
        }
    }
}

/// Mutable last-resort fields belong to the same shutdown root fixed point as
/// ordinary exceptions. Keep the empty arguments/class owners until callbacks
/// are impossible, then release the reserve in the callback-free tail.
pub(crate) fn clear_shutdown_fields(py: &PyToken<'_>) -> bool {
    let reserve = reserve(py);
    if reserve.phase.load(AtomicOrdering::Acquire) != DRAINING {
        return false;
    }
    let bits = reserve.last_resort.load(AtomicOrdering::Acquire);
    let Some(pointer) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    let args = reserve.empty_args.load(AtomicOrdering::Acquire);
    let mut changed = unsafe {
        EXCEPTION_OWNED_EDGE_SLOTS.iter().any(|&slot| {
            let value = *pointer.cast::<u64>().add(slot);
            match slot {
                EXCEPTION_ARGS_SLOT => value != args,
                EXCEPTION_MSG_SLOT | EXCEPTION_NOTES_SLOT => !exception_field_is_missing(value),
                _ => !obj_from_bits(value).is_none(),
            }
        })
    };
    if reserve.c_views_ready.load(AtomicOrdering::Acquire) {
        let view =
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
        assert!(
            !view.is_null(),
            "last-resort view remains prepared through shutdown"
        );
        let view = unsafe { &*view.cast::<molt_cpython_abi::abi_types::PyBaseExceptionObject>() };
        let expected_args =
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(args) };
        assert!(
            !expected_args.is_null(),
            "empty arguments retain their prepared view"
        );
        changed |= view.args != expected_args
            || view.suppress_context != 0
            || !view.dict.is_null()
            || !view.notes.is_null()
            || !view.traceback.is_null()
            || !view.context.is_null()
            || !view.cause.is_null();
    }
    let (detached, projection) = detach_and_reset(py, pointer);
    release_detached(py, detached, projection);
    if reserve.c_views_ready.load(AtomicOrdering::Acquire) {
        assert!(molt_cpython_abi::bridge::GLOBAL_BRIDGE.refresh_exception_view(bits));
    }
    changed
}

pub(crate) fn finish_shutdown(py: &PyToken<'_>) {
    let reserve = reserve(py);
    reserve.phase.store(CLOSED, AtomicOrdering::Release);
    reserve.c_views_ready.store(false, AtomicOrdering::Release);
    reserve.class.store(0, AtomicOrdering::Release);
    let fallback = reserve.last_resort.swap(0, AtomicOrdering::AcqRel);
    let args = reserve.empty_args.swap(0, AtomicOrdering::AcqRel);
    if fallback != 0 {
        dec_ref_bits(py, fallback);
    }
    if args != 0 {
        dec_ref_bits(py, args);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_cpython_abi::abi_types::{PyExc_MemoryError, PyObject};
    use molt_cpython_abi::api::{capsule, errors, object, refcount::OwnedPyObject};
    use std::sync::atomic::AtomicUsize;

    struct AllocationDenial;
    impl AllocationDenial {
        fn enter() -> Self {
            crate::resource::set_tracker(Box::new(crate::resource::LimitedTracker::new(
                &crate::resource::ResourceLimits {
                    max_allocations: Some(0),
                    max_memory: Some(0),
                    ..Default::default()
                },
            )));
            Self
        }
    }
    impl Drop for AllocationDenial {
        fn drop(&mut self) {
            crate::resource::set_tracker(Box::new(crate::resource::UnlimitedTracker));
        }
    }

    #[test]
    fn public_no_memory_transfers_real_values_after_reserve_exhaustion_without_allocation() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // Every transferred value remains owned until all acquisitions have
            // completed: no test release can secretly replenish the reserve.
            let mut values: [Option<OwnedPyObject>; CAPACITY + 2] = std::array::from_fn(|_| None);
            let _denial = AllocationDenial::enter();
            for slot in &mut values {
                unsafe {
                    assert!(errors::PyErr_NoMemory().is_null());
                    let value = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert!(!value.as_ptr().is_null());
                    assert_eq!((*value.as_ptr()).ob_type, &raw mut PyExc_MemoryError);
                    assert!(errors::PyErr_Occurred().is_null());
                    *slot = Some(value);
                }
            }
            for (index, value) in values[..CAPACITY].iter().enumerate() {
                for later in &values[index + 1..] {
                    assert_ne!(
                        value.as_ref().unwrap().as_ptr(),
                        later.as_ref().unwrap().as_ptr(),
                        "every retained reserve value is distinct from the rest and the fallback"
                    );
                }
            }
            assert_eq!(
                values[CAPACITY].as_ref().unwrap().as_ptr(),
                values[CAPACITY + 1].as_ref().unwrap().as_ptr(),
                "exhaustion uses the interpreter's retained last-resort object"
            );
            // Return owners while denial remains active, then prove both public
            // transfer forms on a freshly selected (replenished) reserve value.
            for value in &mut values {
                drop(value.take());
            }
            record_memory_error_without_allocation(py);
            crate::test_support::assert_and_clear_emergency_memory_error(py);
        });
    }

    static PAYLOAD_DROPS: AtomicUsize = AtomicUsize::new(0);
    static PAYLOAD: u8 = 1;
    unsafe extern "C" fn payload_drop(_: *mut PyObject) {
        PAYLOAD_DROPS.fetch_add(1, AtomicOrdering::SeqCst);
    }

    #[test]
    fn reserved_error_releases_mutable_payload_before_returning_to_the_reserve() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            PAYLOAD_DROPS.store(0, AtomicOrdering::SeqCst);
            unsafe {
                errors::PyErr_NoMemory();
                let value = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!value.as_ptr().is_null());
                let payload = OwnedPyObject::from_owned(capsule::PyCapsule_New(
                    (&raw const PAYLOAD).cast_mut().cast(),
                    std::ptr::null(),
                    Some(payload_drop),
                ));
                assert!(!payload.as_ptr().is_null());
                assert_eq!(
                    object::PyObject_SetAttrString(
                        value.as_ptr(),
                        c"retained_payload".as_ptr(),
                        payload.as_ptr()
                    ),
                    0
                );
                drop(payload);
                assert_eq!(PAYLOAD_DROPS.load(AtomicOrdering::SeqCst), 0);
                let _denial = AllocationDenial::enter();
                drop(value);
                assert_eq!(
                    PAYLOAD_DROPS.load(AtomicOrdering::SeqCst),
                    1,
                    "the reserve cannot retain a departed exception's arbitrary owners"
                );
                record_memory_error_without_allocation(py);
                crate::test_support::assert_and_clear_emergency_memory_error(py);
            }
        });
    }

    #[test]
    fn allocation_free_raised_values_follow_the_actual_task_owner() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let make_task = || {
                let bits = crate::molt_task_new(0, 0, crate::TASK_KIND_FUTURE);
                let owner = ExceptionValue::adopt(py, bits);
                assert!(obj_from_bits(owner.bits()).as_ptr().is_some());
                owner
            };
            let a = make_task();
            let b = make_task();
            let a_ptr = obj_from_bits(a.bits()).as_ptr().unwrap();
            let b_ptr = obj_from_bits(b.bits()).as_ptr().unwrap();
            let _denial = AllocationDenial::enter();
            let a_bits = {
                let _a = crate::CurrentTaskScope::enter(py, a_ptr);
                record_memory_error_without_allocation(py);
                exception_last_bits_noinc(py).expect("task A owns an error")
            };
            let b_bits = {
                let _b = crate::CurrentTaskScope::enter(py, b_ptr);
                assert!(!exception_pending(py));
                record_memory_error_without_allocation(py);
                exception_last_bits_noinc(py).expect("task B owns an error")
            };
            assert_ne!(
                a_bits, b_bits,
                "suspended tasks retain separate live reserve values"
            );
            assert!(!exception_pending(py));
            record_memory_error_without_allocation(py);
            let thread_bits = exception_last_bits_noinc(py).expect("thread owns an error");
            assert_ne!(thread_bits, a_bits);
            assert_ne!(thread_bits, b_bits);
            for (task, expected) in [(a_ptr, a_bits), (b_ptr, b_bits)] {
                let _task = crate::CurrentTaskScope::enter(py, task);
                assert_eq!(exception_last_bits_noinc(py), Some(expected));
                crate::test_support::assert_and_clear_emergency_memory_error(py);
            }
            assert_eq!(exception_last_bits_noinc(py), Some(thread_bits));
            crate::test_support::assert_and_clear_emergency_memory_error(py);
            drop(b);
            drop(a);
        });
    }
}
