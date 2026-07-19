//! CPython-compatible pending-call custody.
//!
//! `Py_AddPendingCall` is callable from arbitrary native threads and from
//! low-level notification paths where taking a lock or allocating is unsafe.
//! The queue is therefore a process-static, bounded sequence-stamped ring:
//! producers reserve a slot with one CAS, publish its payload with a Release
//! store, and return `-1` when every slot is still owned by the consumer side.
//! The runtime's registered main thread is the sole consumer.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::os::raw::c_int;
#[cfg(feature = "runtime-test-support")]
use std::sync::Mutex;
#[cfg(not(feature = "runtime-test-support"))]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::hooks::{AttachedRuntimeContextKind, PendingCallErrorKind};

/// CPython 3.14's `PENDINGCALLSARRAYSIZE`.  Keeping the same fixed bound makes
/// overflow deterministic and avoids making signal-path behavior allocator-
/// or configuration-dependent.
pub const PENDING_CALL_CAPACITY: usize = 300;

pub type PendingCallFn = unsafe extern "C" fn(*mut c_void) -> c_int;

#[derive(Clone, Copy)]
struct PendingCall {
    func: PendingCallFn,
    // Store the opaque C pointer as bits so the queue itself is `Send` without
    // inventing ownership or a pointee lifetime for an argument that producers
    // explicitly transfer only as an uninterpreted callback token.
    arg: usize,
}

struct PendingCallSlot {
    sequence: AtomicUsize,
    value: UnsafeCell<MaybeUninit<PendingCall>>,
}

impl PendingCallSlot {
    const fn new(sequence: usize) -> Self {
        Self {
            sequence: AtomicUsize::new(sequence),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

// A producer has exclusive access only after reserving the slot's sequence;
// the consumer reads only after the producer's Release publication and returns
// the slot only after the read.  No two threads access `value` concurrently.
unsafe impl Sync for PendingCallSlot {}

/// Maximum cacheline/CAS observations made by one producer call.
///
/// Two attempts per counter bit gives 64 probes on 32-bit and 128 on 64-bit:
/// enough to absorb ordinary same-core and cross-core ownership handoff, while
/// keeping signal-path failure independent of scheduler progress. Full queues
/// still fail on the first probe. This is part of the ABI latency contract, not
/// an implementation retry-until-success knob.
pub const PENDING_CALL_PRODUCER_ATTEMPT_BUDGET: usize = 2 * usize::BITS as usize;

struct PendingCallQueue<const N: usize, const COUNTER_BITS: u32 = { usize::BITS }> {
    enqueue_pos: AtomicUsize,
    dequeue_pos: AtomicUsize,
    slots: [PendingCallSlot; N],
}

impl<const N: usize, const COUNTER_BITS: u32> PendingCallQueue<N, COUNTER_BITS> {
    /// Counter arithmetic deliberately uses a cycle whose length is a multiple
    /// of the queue capacity.  Natural integer wrap is incorrect for a 300-slot
    /// ring because `2^32 % 300 != 0`: after wrap, ticket-to-slot identity would
    /// shift.  Reserving the high, incomplete portion of the integer domain
    /// preserves `ticket % N` on every supported word size.
    const COUNTER_CYCLE: usize = {
        assert!(COUNTER_BITS > 0 && COUNTER_BITS <= usize::BITS);
        let domain_max = if COUNTER_BITS == usize::BITS {
            usize::MAX
        } else {
            (1usize << COUNTER_BITS) - 1
        };
        (domain_max / N) * N
    };

    const fn new() -> Self {
        assert!(N > 0, "pending-call queue capacity must be non-zero");
        assert!(
            Self::COUNTER_CYCLE >= N * 2,
            "pending-call counter domain must distinguish full from empty"
        );
        let mut slots = [const { PendingCallSlot::new(0) }; N];
        let mut index = 0;
        while index < N {
            slots[index] = PendingCallSlot::new(index);
            index += 1;
        }
        Self {
            enqueue_pos: AtomicUsize::new(0),
            dequeue_pos: AtomicUsize::new(0),
            slots,
        }
    }

    #[inline]
    const fn advance(pos: usize, delta: usize) -> usize {
        debug_assert!(pos < Self::COUNTER_CYCLE);
        debug_assert!(delta < Self::COUNTER_CYCLE);
        let remaining = Self::COUNTER_CYCLE - pos;
        if delta >= remaining {
            delta - remaining
        } else {
            pos + delta
        }
    }

    #[inline]
    const fn forward_distance(from: usize, to: usize) -> usize {
        if to >= from {
            to - from
        } else {
            (Self::COUNTER_CYCLE - from) + to
        }
    }

    /// Reserve and publish one call without locks or allocation.
    fn push(&self, call: PendingCall) -> Result<(), ()> {
        let mut pos = self.enqueue_pos.load(Ordering::Relaxed);
        for _ in 0..PENDING_CALL_PRODUCER_ATTEMPT_BUDGET {
            let dequeue_pos = self.dequeue_pos.load(Ordering::Acquire);
            if Self::forward_distance(dequeue_pos, pos) >= N {
                return Err(());
            }
            let slot = &self.slots[pos % N];
            let sequence = slot.sequence.load(Ordering::Acquire);
            if sequence == pos {
                match self.enqueue_pos.compare_exchange_weak(
                    pos,
                    Self::advance(pos, 1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: this producer exclusively owns `slot` until
                        // the Release publication below advances its sequence.
                        unsafe { (*slot.value.get()).write(call) };
                        slot.sequence
                            .store(Self::advance(pos, 1), Ordering::Release);
                        return Ok(());
                    }
                    Err(observed) => pos = observed,
                }
            } else {
                pos = self.enqueue_pos.load(Ordering::Relaxed);
                std::hint::spin_loop();
            }
        }
        Err(())
    }

    /// Pop one fully-published call.  The `HANDLING_PENDING_CALLS` gate makes
    /// this a single-consumer operation even when a callback re-enters the ABI.
    fn pop(&self) -> Option<PendingCall> {
        let pos = self.dequeue_pos.load(Ordering::Relaxed);
        let slot = &self.slots[pos % N];
        let sequence = slot.sequence.load(Ordering::Acquire);
        if sequence != Self::advance(pos, 1) {
            return None;
        }
        // SAFETY: the Acquire load observed the producer's Release publication;
        // no producer may reclaim this slot until the Release store below.
        let call = unsafe { (*slot.value.get()).assume_init_read() };
        self.dequeue_pos
            .store(Self::advance(pos, 1), Ordering::Relaxed);
        slot.sequence
            .store(Self::advance(pos, N), Ordering::Release);
        Some(call)
    }

    #[inline]
    fn has_ready_call(&self) -> bool {
        let pos = self.dequeue_pos.load(Ordering::Relaxed);
        self.slots[pos % N].sequence.load(Ordering::Acquire) == Self::advance(pos, 1)
    }
}

static PENDING_CALLS: PendingCallQueue<PENDING_CALL_CAPACITY> = PendingCallQueue::new();
static HANDLING_PENDING_CALLS: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "runtime-test-support")]
static MAIN_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);
#[cfg(not(feature = "runtime-test-support"))]
static MAIN_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();
static PENDING_CALL_ADMISSION: PendingCallAdmission = PendingCallAdmission::new();

/// Epoch-stamped producer admission for the process-static ring.
///
/// Odd epochs accept publishers; even epochs are closed. A producer holds an
/// active lease across its queue publication and must observe the same epoch
/// both before and after acquiring that lease. Teardown can therefore close
/// admission, wait for every old-epoch publisher, and dispose the ring without
/// racing a partially-published slot. Reopening advances the epoch again, so a
/// producer that started during finalization cannot cross into a later runtime
/// lifecycle even if that lifecycle begins immediately.
struct PendingCallAdmission {
    epoch: AtomicUsize,
    active_publishers: AtomicUsize,
}

impl PendingCallAdmission {
    const ACCEPTING_BIT: usize = 1;

    const fn new() -> Self {
        Self {
            // CPython 3.12 requires initialized pending-call state. The one
            // runtime-init winner opens the first epoch through the same path
            // used by every explicit later lifecycle.
            epoch: AtomicUsize::new(0),
            active_publishers: AtomicUsize::new(0),
        }
    }

    fn enter(&self) -> Option<PendingCallPublisher<'_>> {
        let epoch = self.epoch.load(Ordering::Acquire);
        if epoch & Self::ACCEPTING_BIT == 0 {
            return None;
        }
        self.active_publishers.fetch_add(1, Ordering::AcqRel);
        if self.epoch.load(Ordering::Acquire) != epoch {
            self.active_publishers.fetch_sub(1, Ordering::Release);
            return None;
        }
        Some(PendingCallPublisher { admission: self })
    }

    fn close_and_quiesce(&self) {
        let mut epoch = self.epoch.load(Ordering::Acquire);
        while epoch & Self::ACCEPTING_BIT != 0 {
            match self.epoch.compare_exchange_weak(
                epoch,
                epoch.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => epoch = observed,
            }
        }
        let mut spins = 0usize;
        while self.active_publishers.load(Ordering::Acquire) != 0 {
            if spins < PENDING_CALL_PRODUCER_ATTEMPT_BUDGET {
                std::hint::spin_loop();
                spins += 1;
            } else {
                std::thread::yield_now();
            }
        }
    }

    fn reopen(&self) -> bool {
        let epoch = self.epoch.load(Ordering::Acquire);
        if epoch & Self::ACCEPTING_BIT != 0 {
            return true;
        }
        if self.active_publishers.load(Ordering::Acquire) != 0 {
            return false;
        }
        self.epoch
            .compare_exchange(
                epoch,
                epoch.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn is_accepting(&self) -> bool {
        self.epoch.load(Ordering::Acquire) & Self::ACCEPTING_BIT != 0
    }
}

struct PendingCallPublisher<'a> {
    admission: &'a PendingCallAdmission,
}

impl Drop for PendingCallPublisher<'_> {
    fn drop(&mut self) {
        self.admission
            .active_publishers
            .fetch_sub(1, Ordering::Release);
    }
}

/// Register the process main thread exactly once during runtime initialization.
/// A later initializer cannot silently transfer pending-call execution custody.
pub fn register_main_thread(owner: std::thread::ThreadId) -> bool {
    #[cfg(feature = "runtime-test-support")]
    {
        let mut registered = MAIN_THREAD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *registered {
            Some(current) if current != owner => return false,
            Some(_) => {}
            None => *registered = Some(owner),
        }
    }
    #[cfg(not(feature = "runtime-test-support"))]
    {
        let _ = MAIN_THREAD.set(owner);
        if !MAIN_THREAD
            .get()
            .is_some_and(|registered| *registered == owner)
        {
            return false;
        }
    }
    if PENDING_CALL_ADMISSION.is_accepting() {
        return true;
    }
    // A closed epoch may reopen only after teardown disposed every callback
    // from the prior runtime lifetime.
    !has_pending_calls() && PENDING_CALL_ADMISSION.reopen()
}

#[inline]
fn current_thread_is_main() -> bool {
    #[cfg(feature = "runtime-test-support")]
    {
        MAIN_THREAD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|registered| registered == std::thread::current().id())
    }
    #[cfg(not(feature = "runtime-test-support"))]
    {
        MAIN_THREAD
            .get()
            .is_some_and(|registered| *registered == std::thread::current().id())
    }
}

struct HandlingGuard<'a>(&'a AtomicBool);

impl Drop for HandlingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn drain_queue<const N: usize, const COUNTER_BITS: u32>(
    queue: &PendingCallQueue<N, COUNTER_BITS>,
    handling: &AtomicBool,
) -> Result<(), PendingCallErrorKind> {
    if handling
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        // CPython deliberately suppresses recursive pending-call drains.  The
        // outer pass remains responsible for calls queued by its callbacks.
        return Ok(());
    }
    let _handling = HandlingGuard(handling);
    for _ in 0..N {
        let Some(call) = queue.pop() else {
            return Ok(());
        };
        // SAFETY: the caller supplied a C callback with the Py_AddPendingCall
        // contract; execution occurs only under main-thread/GIL custody.
        if unsafe { (call.func)(call.arg as *mut c_void) } != 0 {
            return Err(PendingCallErrorKind::CallbackFailedWithoutException);
        }
    }
    Ok(())
}

/// Fast readiness predicate used by explicit eval-breaker and teardown polls.
#[inline]
pub fn has_pending_calls() -> bool {
    PENDING_CALLS.has_ready_call()
}

/// Proof that the current thread is both the lifecycle-selected process main
/// thread and attached under the target's execution model.  Construction is
/// private so GIL, future free-threaded, and wasm projections cannot collapse
/// into ad-hoc boolean checks at consumers.
pub struct AttachedMainRuntimeContext {
    _kind: AttachedRuntimeContextKind,
    _private: (),
}

impl AttachedMainRuntimeContext {
    #[inline]
    fn current() -> Option<Self> {
        if !current_thread_is_main() {
            return None;
        }
        let raw = unsafe { (crate::hooks::hooks_or_stubs().attached_runtime_context)() };
        let kind = AttachedRuntimeContextKind::from_abi(raw)?;
        if kind == AttachedRuntimeContextKind::Detached {
            return None;
        }
        Some(Self {
            _kind: kind,
            _private: (),
        })
    }
}

fn pending_call_result() -> Result<(), PendingCallErrorKind> {
    // Empty is the overwhelmingly common hot path: keep it to one Acquire load
    // and avoid even resolving the current ThreadId until work is published.
    if !has_pending_calls() || !current_thread_is_main() {
        return Ok(());
    }
    let Some(context) = AttachedMainRuntimeContext::current() else {
        return Err(PendingCallErrorKind::RuntimeContextDetached);
    };
    make_pending_calls(&context)
}

fn pending_call_teardown_result() -> Result<(), PendingCallErrorKind> {
    if !has_pending_calls() {
        return Ok(());
    }
    let Some(context) = AttachedMainRuntimeContext::current() else {
        return Err(PendingCallErrorKind::RuntimeContextDetached);
    };
    make_pending_calls(&context)
}

fn discard_queue<const N: usize, const COUNTER_BITS: u32>(
    queue: &PendingCallQueue<N, COUNTER_BITS>,
) -> usize {
    let mut discarded = 0usize;
    while queue.pop().is_some() {
        discarded += 1;
    }
    discarded
}

fn discard_pending_calls() -> usize {
    discard_queue(&PENDING_CALLS)
}

/// Exact process-global pending-call state borrowed by molt-runtime's unit-test
/// transaction. Production artifacts never enable this feature.
#[cfg(feature = "runtime-test-support")]
pub struct PendingCallRuntimeTestSnapshot {
    main_thread: Option<std::thread::ThreadId>,
    admission_was_open: bool,
}

/// Isolate one runtime test from the process-static pending-call authority.
///
/// The queue must be empty at a test boundary. Admission is first closed and
/// every producer is quiesced, making the owner swap and empty-queue proof one
/// lifecycle transition instead of a test-order assumption.
#[cfg(feature = "runtime-test-support")]
pub fn begin_runtime_test_transaction(
    owner: std::thread::ThreadId,
) -> PendingCallRuntimeTestSnapshot {
    let admission_was_open = PENDING_CALL_ADMISSION.is_accepting();
    PENDING_CALL_ADMISSION.close_and_quiesce();
    assert_eq!(
        discard_pending_calls(),
        0,
        "pending-call queue leaked across a runtime test boundary"
    );
    assert!(
        !HANDLING_PENDING_CALLS.swap(false, Ordering::AcqRel),
        "pending-call handler remained active at a runtime test boundary"
    );
    let main_thread = MAIN_THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .replace(owner);
    assert!(
        PENDING_CALL_ADMISSION.reopen(),
        "pending-call admission failed to open for runtime test"
    );
    PendingCallRuntimeTestSnapshot {
        main_thread,
        admission_was_open,
    }
}

/// Restore the owner and admission state captured at test entry.
#[cfg(feature = "runtime-test-support")]
pub fn restore_runtime_test_transaction(snapshot: PendingCallRuntimeTestSnapshot) {
    PENDING_CALL_ADMISSION.close_and_quiesce();
    let leaked = discard_pending_calls();
    HANDLING_PENDING_CALLS.store(false, Ordering::Release);
    *MAIN_THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot.main_thread;
    if snapshot.admission_was_open {
        assert!(
            PENDING_CALL_ADMISSION.reopen(),
            "pending-call admission failed to restore after runtime test"
        );
    }
    if !std::thread::panicking() {
        assert_eq!(
            leaked, 0,
            "pending-call queue leaked out of a runtime test transaction"
        );
    }
}

#[inline]
fn make_pending_calls(_context: &AttachedMainRuntimeContext) -> Result<(), PendingCallErrorKind> {
    drain_queue(&PENDING_CALLS, &HANDLING_PENDING_CALLS)
}

/// End a direct C API boundary with exactly one C-visible indicator. Existing
/// C errors win; otherwise an exact runtime error is moved into C; only a
/// genuinely missing error is synthesized.
fn finish_c_boundary(result: Result<(), PendingCallErrorKind>) -> c_int {
    let Err(kind) = result else {
        return 0;
    };
    let hooks = crate::hooks::hooks_or_stubs();
    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        if !crate::api::errors::transfer_runtime_pending_to_current() {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
                    kind.message().as_ptr(),
                )
            };
            // A failed runtime projection may have installed a new runtime
            // error. The direct C boundary owns the final indicator.
            unsafe { (hooks.clear_pending_exception)() };
        }
    } else {
        unsafe { (hooks.clear_pending_exception)() };
    }
    debug_assert!(!unsafe { crate::api::errors::PyErr_Occurred() }.is_null());
    -1
}

/// End a generated-runtime/teardown boundary with exactly one runtime
/// indicator. The hook preserves an existing runtime error, otherwise consumes
/// the exact C error into runtime, otherwise synthesizes the typed SystemError.
fn finish_runtime_boundary(result: Result<(), PendingCallErrorKind>) -> c_int {
    let Err(kind) = result else {
        return 0;
    };
    unsafe { (crate::hooks::hooks_or_stubs().pending_call_error)(kind as u32) };
    -1
}

/// Drain at an already-GIL-held generated-runtime safepoint. Non-main threads
/// preserve the queue for the registered main thread.
pub fn make_pending_calls_at_runtime_safepoint() -> c_int {
    finish_runtime_boundary(pending_call_result())
}

/// Execute one final CPython-compatible bounded pass while callback-local
/// scheduling is still legal, then close producer admission, quiesce every
/// in-flight publisher, and dispose any remainder before runtime-owned callback
/// arguments can become stale. The first callback failure remains the exact
/// runtime-boundary error; later queued entries are deliberately not executed
/// after that failure, matching CPython 3.12's bounded finish followed by
/// destruction of per-interpreter pending state.
pub fn finish_pending_calls_before_teardown() -> c_int {
    let result = pending_call_teardown_result();
    PENDING_CALL_ADMISSION.close_and_quiesce();
    let _discarded = discard_pending_calls();
    finish_runtime_boundary(result)
}

/// Queue a callback for execution by the registered main thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_AddPendingCall(func: Option<PendingCallFn>, arg: *mut c_void) -> c_int {
    let Some(func) = func else {
        return -1;
    };
    let Some(_publisher) = PENDING_CALL_ADMISSION.enter() else {
        return -1;
    };
    PENDING_CALLS
        .push(PendingCall {
            func,
            arg: arg as usize,
        })
        .map_or(-1, |()| 0)
}

/// Run a bounded pending-call pass when invoked with normal CPython custody.
#[unsafe(no_mangle)]
pub extern "C" fn Py_MakePendingCalls() -> c_int {
    finish_c_boundary(pending_call_result())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    unsafe extern "C" fn noop(_arg: *mut c_void) -> c_int {
        0
    }

    #[test]
    fn lifecycle_epoch_closes_quiesces_and_reopens_without_crossing_publishers() {
        let admission = Arc::new(PendingCallAdmission::new());
        let queue = Arc::new(PendingCallQueue::<4>::new());
        assert!(admission.enter().is_none());
        assert!(admission.reopen());
        let publisher = admission.enter().expect("first lifecycle accepts work");
        let closing = Arc::clone(&admission);
        let closer = std::thread::spawn(move || closing.close_and_quiesce());
        while admission.is_accepting() {
            std::hint::spin_loop();
        }
        assert!(admission.enter().is_none());
        assert!(
            !admission.reopen(),
            "a new epoch cannot open while an old publisher still owns a lease"
        );
        queue
            .push(PendingCall { func: noop, arg: 7 })
            .expect("the old-epoch lease may finish its in-flight publication");
        drop(publisher);
        closer.join().unwrap();
        assert!(admission.enter().is_none());
        assert_eq!(discard_queue(queue.as_ref()), 1);
        assert!(admission.reopen());
        assert!(admission.enter().is_some());
    }

    struct FinalizationEnqueueContext {
        queue: PendingCallQueue<4>,
        admission: PendingCallAdmission,
        callbacks: AtomicUsize,
    }

    unsafe extern "C" fn finalization_enqueue_callback(arg: *mut c_void) -> c_int {
        let context = unsafe { &*(arg.cast::<FinalizationEnqueueContext>()) };
        if context.callbacks.fetch_add(1, Ordering::Relaxed) == 0 {
            let _publisher = context
                .admission
                .enter()
                .expect("the bounded final pass keeps its epoch open");
            context
                .queue
                .push(PendingCall {
                    func: finalization_enqueue_callback,
                    arg: arg as usize,
                })
                .unwrap();
        }
        0
    }

    #[test]
    fn bounded_final_pass_accepts_and_executes_callback_local_scheduling() {
        let context = FinalizationEnqueueContext {
            queue: PendingCallQueue::new(),
            admission: PendingCallAdmission::new(),
            callbacks: AtomicUsize::new(0),
        };
        assert!(context.admission.reopen());
        let arg = (&raw const context).cast_mut().cast::<c_void>();
        {
            let _publisher = context.admission.enter().unwrap();
            context
                .queue
                .push(PendingCall {
                    func: finalization_enqueue_callback,
                    arg: arg as usize,
                })
                .unwrap();
        }
        assert_eq!(drain_queue(&context.queue, &AtomicBool::new(false)), Ok(()));
        context.admission.close_and_quiesce();
        assert_eq!(discard_queue(&context.queue), 0);
        assert_eq!(context.callbacks.load(Ordering::Relaxed), 2);
        assert!(context.admission.enter().is_none());
    }

    #[test]
    fn bounded_queue_reports_full_and_reuses_consumed_slots_fifo() {
        let queue = PendingCallQueue::<3>::new();
        for arg in 1..=3usize {
            assert!(queue.push(PendingCall { func: noop, arg }).is_ok());
        }
        assert!(queue.push(PendingCall { func: noop, arg: 4 }).is_err());
        assert_eq!(queue.pop().map(|call| call.arg), Some(1));
        assert!(queue.push(PendingCall { func: noop, arg: 4 }).is_ok());
        let remaining: Vec<_> = std::iter::from_fn(|| queue.pop())
            .map(|call| call.arg)
            .collect();
        assert_eq!(remaining, [2, 3, 4]);
    }

    #[test]
    fn non_power_of_two_ring_preserves_ticket_identity_across_integer_wrap() {
        // Three slots in an eight-bit model cross the integer boundary more
        // than four times.  This is the exact reduced-width proof for 32-bit
        // targets without requiring billions of queue operations.
        let queue = PendingCallQueue::<3, 8>::new();
        for arg in 1..=1_024usize {
            queue.push(PendingCall { func: noop, arg }).unwrap();
            assert_eq!(queue.pop().map(|call| call.arg), Some(arg));
        }
        assert_eq!(queue.enqueue_pos.load(Ordering::Relaxed) % 3, 1);
        assert_eq!(queue.dequeue_pos.load(Ordering::Relaxed) % 3, 1);
    }

    #[test]
    fn full_queue_fails_within_the_fixed_producer_attempt_budget() {
        let queue = PendingCallQueue::<3, 8>::new();
        for arg in 1..=3usize {
            queue.push(PendingCall { func: noop, arg }).unwrap();
        }
        let before = queue.enqueue_pos.load(Ordering::Relaxed);
        assert!(queue.push(PendingCall { func: noop, arg: 4 }).is_err());
        assert_eq!(queue.enqueue_pos.load(Ordering::Relaxed), before);
    }

    #[test]
    fn fixed_queue_memory_footprint_has_no_hidden_storage() {
        let expected = 2 * std::mem::size_of::<AtomicUsize>()
            + PENDING_CALL_CAPACITY * std::mem::size_of::<PendingCallSlot>();
        eprintln!("pending-call queue footprint={expected} bytes");
        assert_eq!(
            std::mem::size_of::<PendingCallQueue<PENDING_CALL_CAPACITY>>(),
            expected
        );
        assert!(
            expected <= 8 * 1024,
            "pending-call static queue exceeded 8 KiB"
        );
    }

    #[test]
    fn mpsc_publication_survives_contention_full_retries_and_ring_wrap() {
        const PRODUCERS: usize = 8;
        const PER_PRODUCER: usize = 200;
        let queue = Arc::new(PendingCallQueue::<32>::new());
        let mut workers = Vec::new();
        for producer in 0..PRODUCERS {
            let queue = Arc::clone(&queue);
            workers.push(std::thread::spawn(move || {
                for item in 0..PER_PRODUCER {
                    let id = producer * PER_PRODUCER + item + 1;
                    let call = PendingCall {
                        func: noop,
                        arg: id,
                    };
                    while queue.push(call).is_err() {
                        std::thread::yield_now();
                    }
                }
            }));
        }
        let mut observed = HashSet::new();
        while observed.len() < PRODUCERS * PER_PRODUCER {
            if let Some(call) = queue.pop() {
                assert!(observed.insert(call.arg));
            } else {
                std::thread::yield_now();
            }
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(observed.len(), PRODUCERS * PER_PRODUCER);
        assert!((1..=PRODUCERS * PER_PRODUCER).all(|id| observed.contains(&id)));
    }

    unsafe extern "C" fn increment_counter(arg: *mut c_void) -> c_int {
        let counter = unsafe { &*(arg.cast::<AtomicUsize>()) };
        counter.fetch_add(1, Ordering::Relaxed);
        0
    }

    #[test]
    fn exported_add_queues_without_executing_the_callback() {
        assert!(register_main_thread(std::thread::current().id()));
        let counter = AtomicUsize::new(0);
        let arg = (&raw const counter).cast_mut().cast::<c_void>();
        assert_eq!(
            unsafe { Py_AddPendingCall(Some(increment_counter), arg) },
            0
        );
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        let queued = PENDING_CALLS
            .pop()
            .expect("export must publish one queue item");
        assert_eq!(queued.arg, arg as usize);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    struct ReentrantContext {
        queue: PendingCallQueue<4>,
        handling: AtomicBool,
        calls: AtomicUsize,
        nested_result: AtomicUsize,
    }

    unsafe extern "C" fn reentrant_callback(arg: *mut c_void) -> c_int {
        let context = unsafe { &*(arg.cast::<ReentrantContext>()) };
        let prior = context.calls.fetch_add(1, Ordering::Relaxed);
        if prior == 0 {
            let nested = drain_queue(&context.queue, &context.handling);
            context
                .nested_result
                .store(usize::from(nested.is_err()), Ordering::Relaxed);
            assert!(
                context
                    .queue
                    .push(PendingCall {
                        func: reentrant_callback,
                        arg: arg as usize,
                    })
                    .is_ok()
            );
        }
        0
    }

    #[test]
    fn reentrant_drain_is_suppressed_while_outer_pass_accepts_new_work() {
        let context = ReentrantContext {
            queue: PendingCallQueue::new(),
            handling: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            nested_result: AtomicUsize::new(usize::MAX),
        };
        let arg = (&raw const context).cast_mut().cast::<c_void>();
        context
            .queue
            .push(PendingCall {
                func: reentrant_callback,
                arg: arg as usize,
            })
            .unwrap();
        assert_eq!(drain_queue(&context.queue, &context.handling), Ok(()));
        assert_eq!(context.nested_result.load(Ordering::Relaxed), 0);
        assert_eq!(context.calls.load(Ordering::Relaxed), 2);
    }

    unsafe extern "C" fn fail(_arg: *mut c_void) -> c_int {
        7
    }

    unsafe extern "C" fn fail_with_c_type_error(_arg: *mut c_void) -> c_int {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"exact callback TypeError".as_ptr(),
            )
        };
        9
    }

    #[test]
    fn callback_failure_is_normalized_and_later_work_remains_queued() {
        let queue = PendingCallQueue::<3>::new();
        let handling = AtomicBool::new(false);
        queue.push(PendingCall { func: fail, arg: 0 }).unwrap();
        queue.push(PendingCall { func: noop, arg: 2 }).unwrap();
        assert_eq!(finish_c_boundary(drain_queue(&queue, &handling)), -1);
        assert_eq!(
            unsafe { crate::api::errors::PyErr_Occurred() },
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>()
        );
        assert_eq!(queue.pop().map(|call| call.arg), Some(2));
        unsafe { crate::api::errors::PyErr_Clear() };
    }

    #[test]
    fn teardown_disposition_discards_remainder_after_first_callback_failure() {
        let queue = PendingCallQueue::<3>::new();
        let handling = AtomicBool::new(false);
        queue.push(PendingCall { func: fail, arg: 0 }).unwrap();
        queue.push(PendingCall { func: noop, arg: 2 }).unwrap();
        queue.push(PendingCall { func: noop, arg: 3 }).unwrap();
        assert_eq!(
            drain_queue(&queue, &handling),
            Err(PendingCallErrorKind::CallbackFailedWithoutException)
        );
        assert_eq!(discard_queue(&queue), 2);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn callback_failure_preserves_exact_existing_c_exception() {
        let queue = PendingCallQueue::<1>::new();
        let handling = AtomicBool::new(false);
        queue
            .push(PendingCall {
                func: fail_with_c_type_error,
                arg: 0,
            })
            .unwrap();
        assert_eq!(finish_c_boundary(drain_queue(&queue, &handling)), -1);
        assert_eq!(
            unsafe { crate::api::errors::PyErr_Occurred() },
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>()
        );
        unsafe { crate::api::errors::PyErr_Clear() };
    }
}
