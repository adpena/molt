//! Canonical unraisable-exception transaction and reporting authority.

use super::raised_state::{
    RaisedSnapshot, discard_current_raised, release_raised, resolve_raised, take_raised,
};
use super::*;
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::object::{ClassEdgeOwnership, object_init_class_edge_unpublished};
use crate::{
    alloc_property_obj, call_callable1, missing_bits, molt_get_attr_name, molt_is_callable,
    molt_sys_stderr, object_class_bits, tuple_from_iter_bits,
};

const UNRAISABLE_FIELDS: [&str; 5] = [
    "exc_type",
    "exc_value",
    "exc_traceback",
    "err_msg",
    "object",
];

fn try_join_text(parts: &[&str]) -> Option<String> {
    let capacity = parts
        .iter()
        .try_fold(0_usize, |total, part| total.checked_add(part.len()))?;
    let mut out = String::new();
    out.try_reserve_exact(capacity).ok()?;
    for part in parts {
        out.push_str(part);
    }
    Some(out)
}

struct HandledSnapshot {
    active: Vec<u64>,
    fallback: Vec<ExceptionContextFallback>,
}

thread_local! {
    /// Ownership escrow for the only restoration case that cannot complete
    /// synchronously: same-thread code holding a handled-stack `RefMut` across
    /// transaction completion. The next unraisable boundary restores this
    /// snapshot before it detaches any state of its own.
    static DEFERRED_UNRAISABLE_HANDLED: std::cell::RefCell<Option<HandledSnapshot>> = const { std::cell::RefCell::new(None) };
}

/// A transaction is required at every unraisable boundary. It preserves the
/// complete raised and handled channels while arbitrary reporting hooks run.
struct UnraisableTransaction<'a, 'py> {
    py: &'a PyToken<'py>,
    raised: Option<RaisedSnapshot>,
    handled: Option<HandledSnapshot>,
    reporting: Option<RaisedSnapshot>,
    armed: bool,
}

fn flush_deferred_handled(_py: &PyToken<'_>) -> bool {
    let Some(mut deferred) = DEFERRED_UNRAISABLE_HANDLED.with(|slot| slot.borrow_mut().take())
    else {
        return true;
    };
    let restored = ACTIVE_EXCEPTION_STACK.with(|active| {
        ACTIVE_EXCEPTION_FALLBACK.with(|fallback| {
            let (Ok(mut active), Ok(mut fallback)) =
                (active.try_borrow_mut(), fallback.try_borrow_mut())
            else {
                return None;
            };
            let old_active = std::mem::replace(&mut *active, std::mem::take(&mut deferred.active));
            let old_fallback =
                std::mem::replace(&mut *fallback, std::mem::take(&mut deferred.fallback));
            Some((old_active, old_fallback))
        })
    });
    let Some((old_active, old_fallback)) = restored else {
        DEFERRED_UNRAISABLE_HANDLED.with(|slot| {
            let old = slot.borrow_mut().replace(deferred);
            assert!(old.is_none(), "multiple deferred unraisable snapshots");
        });
        return false;
    };
    release_stack(_py, old_active);
    release_fallback_stack(_py, old_fallback);
    true
}

fn defer_handled(saved: HandledSnapshot) {
    DEFERRED_UNRAISABLE_HANDLED.with(|slot| {
        let old = slot.borrow_mut().replace(saved);
        assert!(old.is_none(), "multiple deferred unraisable snapshots");
    });
}

fn take_handled(_py: &PyToken<'_>) -> HandledSnapshot {
    assert!(
        flush_deferred_handled(_py),
        "deferred unraisable handled state remains borrowed"
    );
    // Acquire both borrows before detaching either channel. If reentrant code
    // already holds one borrow, no strong edge has moved when this panics.
    ACTIVE_EXCEPTION_STACK.with(|active| {
        ACTIVE_EXCEPTION_FALLBACK.with(|fallback| {
            let mut active = active.borrow_mut();
            let mut fallback = fallback.borrow_mut();
            HandledSnapshot {
                active: std::mem::take(&mut *active),
                fallback: std::mem::take(&mut *fallback),
            }
        })
    })
}

fn release_stack(_py: &PyToken<'_>, stack: Vec<u64>) {
    for bits in stack {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
}

fn release_fallback_stack(_py: &PyToken<'_>, stack: Vec<ExceptionContextFallback>) {
    for entry in stack {
        if entry.owned && !obj_from_bits(entry.bits).is_none() {
            dec_ref_bits(_py, entry.bits);
        }
    }
}

/// Resolve the detached handled channels. `true` means they were restored;
/// `false` means an outstanding reentrant TLS borrow required ownership to be
/// escrowed for exact restoration at the next unraisable boundary.
fn resolve_handled(_py: &PyToken<'_>, saved: &mut Option<HandledSnapshot>) -> bool {
    let Some(saved_ref) = saved.as_mut() else {
        return true;
    };
    let restored = ACTIVE_EXCEPTION_STACK.with(|active| {
        ACTIVE_EXCEPTION_FALLBACK.with(|fallback| {
            let (Ok(mut active), Ok(mut fallback)) =
                (active.try_borrow_mut(), fallback.try_borrow_mut())
            else {
                return None;
            };
            let old_active = std::mem::replace(&mut *active, std::mem::take(&mut saved_ref.active));
            let old_fallback =
                std::mem::replace(&mut *fallback, std::mem::take(&mut saved_ref.fallback));
            Some((old_active, old_fallback))
        })
    });
    match restored {
        Some((old_active, old_fallback)) => {
            // Both saved vectors are published before the transaction drops
            // its ownership marker.
            *saved = None;
            release_stack(_py, old_active);
            release_fallback_stack(_py, old_fallback);
            true
        }
        None => {
            // A borrow held across transaction completion is a caller
            // invariant breach. Preserve the exact stacks in thread-local
            // escrow before failing the caller closed; never discard state.
            let saved = saved.take().expect("handled transaction remains armed");
            defer_handled(saved);
            false
        }
    }
}

impl<'a, 'py> UnraisableTransaction<'a, 'py> {
    fn begin(_py: &'a PyToken<'py>) -> Self {
        // Arm before the first detach so unwinding any later acquisition
        // restores or releases everything already removed from runtime state.
        let mut transaction = Self {
            py: _py,
            raised: None,
            handled: None,
            reporting: None,
            armed: true,
        };
        transaction.handled = Some(take_handled(_py));
        transaction.raised = Some(take_raised(_py));
        transaction
    }

    fn resolve_original(&mut self) -> bool {
        let handled_restored = resolve_handled(self.py, &mut self.handled);
        resolve_raised(self.py, &mut self.raised);
        handled_restored
    }

    fn finish_current(
        mut self,
        context_bits: u64,
        err_msg: Option<&str>,
    ) -> std::thread::Result<()> {
        self.reporting = Some(take_raised(self.py));
        self.finish_reporting(context_bits, err_msg, report_unraisable_exception)
    }

    fn finish_raised(
        mut self,
        raised: RaisedSnapshot,
        context_bits: u64,
        err_msg: Option<&str>,
    ) -> std::thread::Result<()> {
        self.reporting = Some(raised);
        self.finish_reporting(context_bits, err_msg, report_unraisable_exception)
    }

    fn finish_reporting(
        mut self,
        context_bits: u64,
        err_msg: Option<&str>,
        reporter: impl FnOnce(&PyToken<'_>, u64, u64, Option<&str>),
    ) -> std::thread::Result<()> {
        let report = match self.reporting.as_ref() {
            Some(RaisedSnapshot::Thread(bits) | RaisedSnapshot::Task(_, bits)) => {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    reporter(self.py, context_bits, *bits, err_msg);
                }))
            }
            Some(RaisedSnapshot::Emergency(_)) => {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // The snapshot itself is allocation-free. Materialize only
                    // when reporting can recover enough memory to do so.
                    let pointer = alloc_exception(self.py, "MemoryError", "");
                    if pointer.is_null() {
                        eprintln!("Exception ignored in unraisable callback: MemoryError");
                        return;
                    }
                    let bits = MoltObject::from_ptr(pointer).bits();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        reporter(self.py, context_bits, bits, err_msg);
                    }));
                    dec_ref_bits(self.py, bits);
                    if let Err(payload) = result {
                        std::panic::resume_unwind(payload);
                    }
                }))
            }
            Some(RaisedSnapshot::None) | None => Ok(()),
        };
        discard_current_raised(self.py);
        release_raised(self.py, &mut self.reporting);
        let handled_restored = self.resolve_original();
        self.armed = false;
        assert!(
            handled_restored,
            "unraisable transaction completed while handled exception TLS was borrowed"
        );
        report
    }

    fn report_captured(
        mut self,
        context_bits: u64,
        exc_bits: u64,
        err_msg: Option<&str>,
    ) -> std::thread::Result<()> {
        discard_current_raised(self.py);
        let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !obj_from_bits(exc_bits).is_none() {
                report_unraisable_exception(self.py, context_bits, exc_bits, err_msg);
            }
        }));
        discard_current_raised(self.py);
        let handled_restored = self.resolve_original();
        self.armed = false;
        assert!(
            handled_restored,
            "unraisable transaction completed while handled exception TLS was borrowed"
        );
        report
    }
}

impl Drop for UnraisableTransaction<'_, '_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if !std::thread::panicking() {
            discard_current_raised(self.py);
            release_raised(self.py, &mut self.reporting);
            let handled_restored = resolve_handled(self.py, &mut self.handled);
            resolve_raised(self.py, &mut self.raised);
            self.armed = false;
            assert!(
                handled_restored,
                "unraisable transaction dropped while handled exception TLS was borrowed"
            );
            return;
        }

        // A cleanup defect must never replace an active panic with a double
        // panic. Each cleanup primitive consumes or publishes its Option-held
        // ownership before any decref, so containment cannot create a second
        // owner. Run the primitives independently so one defect cannot skip a
        // different ownership channel.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            discard_current_raised(self.py);
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            release_raised(self.py, &mut self.reporting);
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = resolve_handled(self.py, &mut self.handled);
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            resolve_raised(self.py, &mut self.raised);
        }));
        // If an injected cleanup panic happened before a primitive consumed
        // its slot, make one final ownership-resolution attempt. Handled state
        // is escrowed for deferred restoration; raised edges are released.
        if let Some(saved) = self.handled.take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                defer_handled(saved);
            }));
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            release_raised(self.py, &mut self.reporting);
            release_raised(self.py, &mut self.raised);
        }));
        self.armed = self.handled.is_some() || self.reporting.is_some() || self.raised.is_some();
    }
}

pub(crate) fn run_unraisable<R>(
    _py: &PyToken<'_>,
    context_bits: u64,
    err_msg: Option<&str>,
    run: impl FnOnce() -> R,
) -> R {
    let transaction = UnraisableTransaction::begin(_py);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    let report = transaction.finish_current(context_bits, err_msg);
    match result {
        Err(payload) => std::panic::resume_unwind(payload),
        Ok(result) => match report {
            Ok(()) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        },
    }
}

pub(crate) fn run_unraisable_with_policy<R>(
    _py: &PyToken<'_>,
    policy: impl FnOnce() -> (u64, Option<String>),
    run: impl FnOnce() -> R,
) -> R {
    let transaction = UnraisableTransaction::begin(_py);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    let raised = take_raised(_py);
    let has_raised = !matches!(&raised, RaisedSnapshot::None);
    let policy_result = if has_raised {
        Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            policy,
        )))
    } else {
        None
    };
    let (context_bits, err_msg) = policy_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|(context, message)| (*context, message.as_deref()))
        .unwrap_or((MoltObject::none().bits(), None));
    let report = transaction.finish_raised(raised, context_bits, err_msg);
    let output = match result {
        Ok(output) => output,
        Err(payload) => std::panic::resume_unwind(payload),
    };
    if let Some(Err(payload)) = policy_result {
        std::panic::resume_unwind(payload);
    }
    match report {
        Ok(()) => output,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

pub(crate) fn report_captured_unraisable(
    _py: &PyToken<'_>,
    context_bits: u64,
    exc_bits: u64,
    err_msg: Option<&str>,
) {
    if let Err(payload) =
        UnraisableTransaction::begin(_py).report_captured(context_bits, exc_bits, err_msg)
    {
        std::panic::resume_unwind(payload);
    }
}

fn sys_attr_bits(_py: &PyToken<'_>, name: &[u8]) -> u64 {
    let Some(sys_bits) = crate::builtins::modules::interpreter_sys_module(_py) else {
        return MoltObject::none().bits();
    };
    inc_ref_bits(_py, sys_bits);

    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        dec_ref_bits(_py, sys_bits);
        return MoltObject::none().bits();
    };
    let value_bits = molt_get_attr_name(sys_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, sys_bits);
    if exception_pending(_py) || obj_from_bits(value_bits).is_none() {
        discard_current_raised(_py);
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return MoltObject::none().bits();
    }
    value_bits
}

pub(crate) fn context_repr(_py: &PyToken<'_>, context_bits: u64) -> String {
    if obj_from_bits(context_bits).is_none() {
        return "<callback>".to_string();
    }
    let repr_bits = crate::molt_repr_from_obj(context_bits);
    if exception_pending(_py) {
        discard_current_raised(_py);
        return "<callback>".to_string();
    }
    let rendered = crate::object::ops::string_obj_to_owned(obj_from_bits(repr_bits))
        .unwrap_or_else(|| "<callback>".to_string());
    if !obj_from_bits(repr_bits).is_none() {
        dec_ref_bits(_py, repr_bits);
    }
    rendered
}

fn set_class_attr(_py: &PyToken<'_>, class_ptr: *mut u8, name: &str, value_bits: u64) -> bool {
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return false;
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return false;
    }
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return false;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    unsafe { dict_set_in_place(_py, dict_ptr, name_bits, value_bits) };
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

fn init_class_edge(_py: &PyToken<'_>, ptr: *mut u8, class_bits: u64) -> bool {
    unsafe { object_init_class_edge_unpublished(_py, ptr, class_bits, ClassEdgeOwnership::Owned) }
}

fn install_readonly_field(_py: &PyToken<'_>, class_ptr: *mut u8, name: &str, getter: u64) -> bool {
    // Property dispatch passes self explicitly, so its callback must not bind.
    let getter_bits = crate::builtins::methods::alloc_builtin_function(_py, getter, 1);
    if getter_bits == 0 {
        return false;
    }
    let none = MoltObject::none().bits();
    let property_ptr = alloc_property_obj(_py, getter_bits, none, none);
    dec_ref_bits(_py, getter_bits);
    if property_ptr.is_null() {
        return false;
    }
    let property_bits = MoltObject::from_ptr(property_ptr).bits();
    let installed = set_class_attr(_py, class_ptr, name, property_bits);
    dec_ref_bits(_py, property_bits);
    installed
}

fn discard_unraisable_args_class(_py: &PyToken<'_>, class_bits: u64) -> u64 {
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        if !obj_from_bits(class_bits).is_none() {
            // Retire constructor-owned contents before detaching identity and
            // the new descriptor/constructor back-edges to the declaring class.
            if let Some(ptr) = obj_from_bits(class_bits).as_ptr() {
                unsafe { crate::object::class_storage::clear_class_runtime_contents(_py, ptr) };
            }
            class_break_cycles(_py, class_bits);
            dec_ref_bits(_py, class_bits);
        }
    });
    0
}

fn build_unraisable_args_class(_py: &PyToken<'_>) -> u64 {
    let name_ptr = alloc_string(_py, b"UnraisableHookArgs");
    if name_ptr.is_null() {
        return 0;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let class_ptr = alloc_class_obj(_py, name_bits);
    dec_ref_bits(_py, name_bits);
    if class_ptr.is_null() {
        return 0;
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    unsafe {
        crate::object::class_storage::class_declare_native_slots(
            class_ptr,
            crate::object::class_storage::ClassSlotPolicy::default(),
        );
        if !crate::object::class_storage::ClassSemanticPolicy::static_type(false)
            .apply(_py, class_ptr)
        {
            return discard_unraisable_args_class(_py, class_bits);
        }
    }
    let builtins = builtin_classes(_py);
    if !init_class_edge(_py, class_ptr, builtins.type_obj) {
        return discard_unraisable_args_class(_py, class_bits);
    }
    let _ = molt_class_set_base(class_bits, builtins.tuple);
    if exception_pending(_py) {
        return discard_unraisable_args_class(_py, class_bits);
    }

    let module_ptr = alloc_string(_py, b"builtins");
    if module_ptr.is_null() {
        return discard_unraisable_args_class(_py, class_bits);
    }
    let module_bits = MoltObject::from_ptr(module_ptr).bits();
    let module_set = set_class_attr(_py, class_ptr, "__module__", module_bits);
    dec_ref_bits(_py, module_bits);
    if !module_set
        || !set_class_attr(
            _py,
            class_ptr,
            "n_fields",
            MoltObject::from_int(UNRAISABLE_FIELDS.len() as i64).bits(),
        )
        || !set_class_attr(
            _py,
            class_ptr,
            "n_sequence_fields",
            MoltObject::from_int(UNRAISABLE_FIELDS.len() as i64).bits(),
        )
        || !set_class_attr(
            _py,
            class_ptr,
            "n_unnamed_fields",
            MoltObject::from_int(0).bits(),
        )
    {
        return discard_unraisable_args_class(_py, class_bits);
    }

    let none = MoltObject::none().bits();
    let mut match_args = [none; UNRAISABLE_FIELDS.len()];
    for (initialized, field) in UNRAISABLE_FIELDS.iter().enumerate() {
        let ptr = alloc_string(_py, field.as_bytes());
        if ptr.is_null() {
            for bits in &match_args[..initialized] {
                dec_ref_bits(_py, *bits);
            }
            return discard_unraisable_args_class(_py, class_bits);
        }
        match_args[initialized] = MoltObject::from_ptr(ptr).bits();
    }
    let match_args_ptr = alloc_tuple(_py, &match_args);
    for bits in match_args {
        dec_ref_bits(_py, bits);
    }
    if match_args_ptr.is_null() {
        return discard_unraisable_args_class(_py, class_bits);
    }
    let match_args_bits = MoltObject::from_ptr(match_args_ptr).bits();
    let match_args_set = set_class_attr(_py, class_ptr, "__match_args__", match_args_bits);
    dec_ref_bits(_py, match_args_bits);
    if !match_args_set
        || crate::builtins::methods::builtin_func_bits_with_defaults_tuple(
            _py,
            NativeCallableSpec::constructor(class_bits),
            molt_unraisable_hook_args_new as *const () as usize as u64,
            3,
            &[missing_bits(_py)],
        ) == 0
        || crate::builtins::methods::builtin_func_bits(
            _py,
            NativeCallableSpec::declared(
                NativeCallableKind::MethodDescriptor,
                class_bits,
                "__repr__",
            ),
            molt_unraisable_hook_args_repr as *const () as usize as u64,
            1,
        ) == 0
        || !install_readonly_field(
            _py,
            class_ptr,
            "exc_type",
            molt_unraisable_hook_args_exc_type as *const () as usize as u64,
        )
        || !install_readonly_field(
            _py,
            class_ptr,
            "exc_value",
            molt_unraisable_hook_args_exc_value as *const () as usize as u64,
        )
        || !install_readonly_field(
            _py,
            class_ptr,
            "exc_traceback",
            molt_unraisable_hook_args_exc_traceback as *const () as usize as u64,
        )
        || !install_readonly_field(
            _py,
            class_ptr,
            "err_msg",
            molt_unraisable_hook_args_err_msg as *const () as usize as u64,
        )
        || !install_readonly_field(
            _py,
            class_ptr,
            "object",
            molt_unraisable_hook_args_object as *const () as usize as u64,
        )
    {
        return discard_unraisable_args_class(_py, class_bits);
    }
    if exception_pending(_py)
        || unsafe { crate::object::class_finish_definition(_py, class_ptr) }.is_err()
    {
        return discard_unraisable_args_class(_py, class_bits);
    }
    class_bits
}

fn unraisable_args_class(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &exceptions_state(_py).unraisable_hook_args_class,
        || build_unraisable_args_class(_py),
    )
}

fn unraisable_args_is_exact(_py: &PyToken<'_>, bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    // Validation must not initialize a hidden type merely because user code
    // passed a forged object to sys.__unraisablehook__.
    let class_bits = exceptions_state(_py)
        .unraisable_hook_args_class
        .load(AtomicOrdering::Acquire);
    class_bits != 0
        && unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE
        && unsafe { object_class_bits(ptr) } == class_bits
}

/// Exact, unforgeable validator used by the Python-level default hook.
///
/// The hidden struct-sequence type is runtime-owned and intentionally absent
/// from `sys`; name/module/shape checks would let user classes spoof it.
#[unsafe(no_mangle)]
pub extern "C" fn molt_unraisable_hook_args_is_exact(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(unraisable_args_is_exact(_py, bits)).bits()
    })
}

fn unraisable_args_field(_py: &PyToken<'_>, self_bits: u64, index: usize) -> u64 {
    let Some(ptr) = obj_from_bits(self_bits).as_ptr() else {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "UnraisableHookArgs descriptor requires an instance",
        );
    };
    if !unraisable_args_is_exact(_py, self_bits) {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "UnraisableHookArgs descriptor requires an instance",
        );
    }
    let Some(bits) = (unsafe { crate::object::seq_access::item(ptr, index) }) else {
        return raise_exception::<_>(_py, "RuntimeError", "invalid UnraisableHookArgs payload");
    };
    inc_ref_bits(_py, bits);
    bits
}

macro_rules! unraisable_field_getter {
    ($name:ident, $index:expr) => {
        extern "C" fn $name(self_bits: u64) -> u64 {
            crate::with_gil_entry_nopanic!(_py, { unraisable_args_field(_py, self_bits, $index) })
        }
    };
}

unraisable_field_getter!(molt_unraisable_hook_args_exc_type, 0);
unraisable_field_getter!(molt_unraisable_hook_args_exc_value, 1);
unraisable_field_getter!(molt_unraisable_hook_args_exc_traceback, 2);
unraisable_field_getter!(molt_unraisable_hook_args_err_msg, 3);
unraisable_field_getter!(molt_unraisable_hook_args_object, 4);

extern "C" fn molt_unraisable_hook_args_new(
    cls_bits: u64,
    sequence_bits: u64,
    fields_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if cls_bits != unraisable_args_class(_py) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "UnraisableHookArgs cannot be subclassed",
            );
        }
        if fields_bits != missing_bits(_py) {
            let Some(fields_ptr) = obj_from_bits(fields_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "UnraisableHookArgs() takes a dict as second arg, if any",
                );
            };
            if unsafe { object_type_id(fields_ptr) } != TYPE_ID_DICT {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "UnraisableHookArgs() takes a dict as second arg, if any",
                );
            }
            if !unsafe { dict_order(fields_ptr) }.is_empty() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "UnraisableHookArgs() got duplicate or unexpected field name(s)",
                );
            }
        }
        let Some(source_bits) = (unsafe { tuple_from_iter_bits(_py, sequence_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(source_ptr) = obj_from_bits(source_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let fields = unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(source_ptr, |fields| {
                if fields.len() == UNRAISABLE_FIELDS.len() {
                    Ok([fields[0], fields[1], fields[2], fields[3], fields[4]])
                } else {
                    Err(fields.len())
                }
            })
        }
        .unwrap_or(Err(0));
        let fields = match fields {
            Ok(fields) => fields,
            Err(actual) => {
                dec_ref_bits(_py, source_bits);
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    &format!(
                        "UnraisableHookArgs() takes a {}-sequence ({}-sequence given)",
                        UNRAISABLE_FIELDS.len(),
                        actual
                    ),
                );
            }
        };
        // The source tuple may be the sole owner of one or more heap fields.
        // Retain them into the new tuple before releasing the source edge.
        let ptr = alloc_tuple(_py, &fields);
        dec_ref_bits(_py, source_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        if !init_class_edge(_py, ptr, cls_bits) {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

extern "C" fn molt_unraisable_hook_args_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(self_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        if !unraisable_args_is_exact(_py, self_bits) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "UnraisableHookArgs.__repr__ requires an instance",
            );
        }
        let fields = unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(ptr, |fields| {
                (fields.len() == UNRAISABLE_FIELDS.len())
                    .then(|| [fields[0], fields[1], fields[2], fields[3], fields[4]])
            })
        }
        .flatten();
        let Some(fields) = fields else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid UnraisableHookArgs payload");
        };
        let mut rendered = String::new();
        if rendered.try_reserve_exact(96).is_err() {
            return raise_exception::<_>(
                _py,
                "MemoryError",
                "UnraisableHookArgs repr allocation failed",
            );
        }
        rendered.push_str("UnraisableHookArgs(");
        for (index, (name, bits)) in UNRAISABLE_FIELDS.iter().zip(fields).enumerate() {
            let punctuation = if index == 0 { 1 } else { 3 };
            let reserve = name.len().checked_add(punctuation);
            if reserve.is_none_or(|reserve| rendered.try_reserve(reserve).is_err()) {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "UnraisableHookArgs repr allocation failed",
                );
            }
            if index != 0 {
                rendered.push_str(", ");
            }
            rendered.push_str(name);
            rendered.push('=');
            let repr_bits = crate::molt_repr_from_obj(bits);
            if exception_pending(_py) || obj_from_bits(repr_bits).is_none() {
                if !obj_from_bits(repr_bits).is_none() {
                    dec_ref_bits(_py, repr_bits);
                }
                return MoltObject::none().bits();
            }
            let Some(item) = string_obj_to_owned(obj_from_bits(repr_bits)) else {
                dec_ref_bits(_py, repr_bits);
                return raise_exception::<_>(_py, "TypeError", "__repr__ returned non-string");
            };
            dec_ref_bits(_py, repr_bits);
            if rendered.try_reserve(item.len()).is_err() {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "UnraisableHookArgs repr allocation failed",
                );
            }
            rendered.push_str(&item);
        }
        rendered.push(')');
        let out = alloc_string(_py, rendered.as_bytes());
        if out.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out).bits()
        }
    })
}

fn alloc_unraisable_hook_args(_py: &PyToken<'_>, fields: &[u64; 5]) -> u64 {
    let class_bits = unraisable_args_class(_py);
    if class_bits == 0 || obj_from_bits(class_bits).is_none() {
        return MoltObject::none().bits();
    }
    let ptr = alloc_tuple(_py, fields);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    if !init_class_edge(_py, ptr, class_bits) {
        dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn build_hook_args(
    _py: &PyToken<'_>,
    context_bits: u64,
    exc_bits: u64,
    err_msg: Option<&str>,
) -> u64 {
    let msg_bits = if let Some(err_msg) = err_msg {
        let msg_ptr = alloc_string(_py, err_msg.as_bytes());
        if msg_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(msg_ptr).bits()
    } else {
        MoltObject::none().bits()
    };
    let message = ExceptionValue::adopt(_py, msg_bits);
    let exception = ExceptionValue::pin(_py, exc_bits);
    let (class, traceback) = if exception_is_instance(_py, exception.bits()) {
        let Some(class) = exception_class(_py, exception.bits()) else {
            discard_current_raised(_py);
            return MoltObject::none().bits();
        };
        let Some(traceback) = exception_traceback(_py, exception.bits()) else {
            discard_current_raised(_py);
            return MoltObject::none().bits();
        };
        (class, traceback)
    } else {
        (
            ExceptionValue::pin(_py, MoltObject::none().bits()),
            ExceptionValue::pin(_py, MoltObject::none().bits()),
        )
    };
    let out = alloc_unraisable_hook_args(
        _py,
        &[
            class.bits(),
            exception.bits(),
            traceback.bits(),
            message.bits(),
            context_bits,
        ],
    );
    if exception_pending(_py) {
        discard_current_raised(_py);
        if !obj_from_bits(out).is_none() {
            dec_ref_bits(_py, out);
        }
        return MoltObject::none().bits();
    }
    out
}

enum HookOutcome {
    Handled,
    Unavailable,
    Raised,
}

fn try_hook(
    _py: &PyToken<'_>,
    context_bits: u64,
    exc_bits: u64,
    err_msg: Option<&str>,
) -> HookOutcome {
    let hook_bits = sys_attr_bits(_py, b"unraisablehook");
    if obj_from_bits(hook_bits).is_none()
        || !is_truthy(_py, obj_from_bits(molt_is_callable(hook_bits)))
    {
        if !obj_from_bits(hook_bits).is_none() {
            dec_ref_bits(_py, hook_bits);
        }
        return HookOutcome::Unavailable;
    }
    let args_bits = build_hook_args(_py, context_bits, exc_bits, err_msg);
    if obj_from_bits(args_bits).is_none() {
        dec_ref_bits(_py, hook_bits);
        return HookOutcome::Unavailable;
    }
    let out = unsafe { call_callable1(_py, hook_bits, args_bits) };
    let hook_exc_bits = exception_last_bits_noinc(_py)
        .filter(|bits| exception_pending(_py) && !obj_from_bits(*bits).is_none());
    if let Some(bits) = hook_exc_bits {
        inc_ref_bits(_py, bits);
    }
    discard_current_raised(_py);
    if !obj_from_bits(out).is_none() {
        dec_ref_bits(_py, out);
    }
    dec_ref_bits(_py, args_bits);
    if let Some(bits) = hook_exc_bits {
        let hook_text = context_repr(_py, hook_bits);
        let formatted = obj_from_bits(bits)
            .as_ptr()
            .map(|ptr| format_exception_with_traceback(_py, ptr))
            .unwrap_or_default();
        if let Some(line) =
            try_join_text(&["Exception ignored in sys.unraisablehook: ", &hook_text])
        {
            write_stderr_line(_py, &line);
        }
        if !formatted.is_empty() {
            write_stderr_line(_py, &formatted);
        }
        dec_ref_bits(_py, bits);
        dec_ref_bits(_py, hook_bits);
        HookOutcome::Raised
    } else {
        dec_ref_bits(_py, hook_bits);
        HookOutcome::Handled
    }
}

fn write_stderr_line(_py: &PyToken<'_>, text: &str) {
    let text_ptr = alloc_string(_py, text.as_bytes());
    if text_ptr.is_null() {
        return;
    }
    let text_bits = MoltObject::from_ptr(text_ptr).bits();
    let args_ptr = alloc_tuple(_py, &[text_bits]);
    if args_ptr.is_null() {
        dec_ref_bits(_py, text_bits);
        return;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let stderr_bits = sys_attr_bits(_py, b"stderr");
    let stderr_bits = if obj_from_bits(stderr_bits).is_none() {
        molt_sys_stderr()
    } else {
        stderr_bits
    };
    if !obj_from_bits(stderr_bits).is_none() {
        let none = MoltObject::none().bits();
        let out = crate::molt_print_builtin(
            args_bits,
            none,
            none,
            stderr_bits,
            MoltObject::from_bool(true).bits(),
        );
        if !obj_from_bits(out).is_none() {
            dec_ref_bits(_py, out);
        }
        discard_current_raised(_py);
        dec_ref_bits(_py, stderr_bits);
    }
    dec_ref_bits(_py, args_bits);
    dec_ref_bits(_py, text_bits);
}

fn default_unraisable_prefix(
    context_is_none: bool,
    context_text: &str,
    err_msg: Option<&str>,
) -> Option<String> {
    match (err_msg, context_is_none) {
        (Some(message), true) => try_join_text(&[message, ":"]),
        (Some(message), false) => try_join_text(&[message, ": ", context_text]),
        (None, true) => None,
        (None, false) => try_join_text(&["Exception ignored in: ", context_text]),
    }
}

fn report_unraisable_exception(
    _py: &PyToken<'_>,
    context_bits: u64,
    exc_bits: u64,
    err_msg: Option<&str>,
) {
    match try_hook(_py, context_bits, exc_bits, err_msg) {
        HookOutcome::Handled | HookOutcome::Raised => return,
        HookOutcome::Unavailable => {}
    }
    let context_text = context_repr(_py, context_bits);
    let formatted = obj_from_bits(exc_bits)
        .as_ptr()
        .map(|ptr| format_exception_with_traceback(_py, ptr))
        .unwrap_or_default();
    if let Some(prefix) = default_unraisable_prefix(
        obj_from_bits(context_bits).is_none(),
        &context_text,
        err_msg,
    ) {
        write_stderr_line(_py, &prefix);
    }
    if !formatted.is_empty() {
        write_stderr_line(_py, &formatted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ref_count(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    fn pending_exception(_py: &PyToken<'_>, message: &str) -> u64 {
        let _: u64 = raise_exception(_py, "RuntimeError", message);
        exception_last_bits_noinc(_py).expect("pending exception")
    }

    fn class_dict_value(_py: &PyToken<'_>, class_bits: u64, name: &str) -> u64 {
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class pointer");
        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        let dict_ptr = obj_from_bits(dict_bits).as_ptr().expect("class dictionary");
        let name_ptr = alloc_string(_py, name.as_bytes());
        assert!(!name_ptr.is_null(), "attribute name allocation");
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value = unsafe { dict_get_in_place(_py, dict_ptr, name_bits) }
            .unwrap_or_else(|| panic!("missing class attribute {name}"));
        dec_ref_bits(_py, name_bits);
        value
    }

    #[test]
    fn unraisable_args_are_an_internal_immutable_structured_tuple() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let none = MoltObject::none().bits();
            let fields = [
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                none,
                MoltObject::from_int(4).bits(),
                MoltObject::from_bool(true).bits(),
            ];
            let args_bits = alloc_unraisable_hook_args(_py, &fields);
            assert!(unraisable_args_is_exact(_py, args_bits));
            assert!(!unraisable_args_is_exact(_py, none));
            assert_eq!(
                obj_from_bits(molt_unraisable_hook_args_is_exact(args_bits)).as_bool(),
                Some(true)
            );
            assert_eq!(
                obj_from_bits(molt_unraisable_hook_args_is_exact(none)).as_bool(),
                Some(false)
            );

            let args_ptr = obj_from_bits(args_bits).as_ptr().expect("args pointer");
            assert_eq!(unsafe { object_type_id(args_ptr) }, TYPE_ID_TUPLE);
            assert_eq!(
                unsafe {
                    crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |items| {
                        items == fields
                    })
                },
                Some(true)
            );
            assert!(isinstance_bits(_py, args_bits, builtin_classes(_py).tuple));
            assert_eq!(unraisable_args_field(_py, args_bits, 1), fields[1]);

            let field_name_ptr = alloc_string(_py, b"exc_value");
            assert!(!field_name_ptr.is_null());
            let field_name_bits = MoltObject::from_ptr(field_name_ptr).bits();
            let field_bits = molt_get_attr_name(args_bits, field_name_bits);
            assert_eq!(field_bits, fields[1]);
            dec_ref_bits(_py, field_bits);
            dec_ref_bits(_py, field_name_bits);

            let class_bits = unsafe { object_class_bits(args_ptr) };
            let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class pointer");
            assert!(
                unsafe { crate::object::class_is_not_base(_py, class_ptr) },
                "the hidden structured type must not be an acceptable base"
            );
            assert!(
                unsafe { crate::object::class_is_immutable(_py, class_ptr) },
                "the hidden structured type must use generic immutable-type metadata"
            );
            let class_refs_before =
                unsafe { (*header_from_obj_ptr(class_ptr)).ref_count_snapshot() };
            let second_args_bits = alloc_unraisable_hook_args(_py, &fields);
            let class_refs_with_second =
                unsafe { (*header_from_obj_ptr(class_ptr)).ref_count_snapshot() };
            assert_eq!(class_refs_with_second, class_refs_before + 1);
            dec_ref_bits(_py, second_args_bits);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(class_ptr)).ref_count_snapshot() },
                class_refs_before,
                "each structured tuple owns and releases exactly one class edge"
            );
            assert_eq!(
                string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) })),
                Some("UnraisableHookArgs".to_string())
            );
            assert_eq!(
                string_obj_to_owned(obj_from_bits(class_dict_value(
                    _py,
                    class_bits,
                    "__module__"
                ))),
                Some("builtins".to_string())
            );
            assert_eq!(
                obj_from_bits(class_dict_value(_py, class_bits, "n_fields")).as_int(),
                Some(5)
            );

            let repr_name_ptr = alloc_string(_py, b"__repr__");
            assert!(!repr_name_ptr.is_null());
            let repr_name_bits = MoltObject::from_ptr(repr_name_ptr).bits();
            let original_repr = class_dict_value(_py, class_bits, "__repr__");
            let repr_ptr = obj_from_bits(original_repr).as_ptr().unwrap();
            assert_eq!(
                unsafe { object_class_bits(repr_ptr) },
                builtin_classes(_py).method_descriptor
            );
            assert_eq!(
                unsafe {
                    crate::call::function::function_metadata_bits(_py, repr_ptr, b"__objclass__")
                },
                class_bits
            );
            let bound_repr = molt_get_attr_name(args_bits, repr_name_bits);
            assert!(!exception_pending(_py));
            let repr_bits = unsafe { crate::call_callable0(_py, bound_repr) };
            assert!(
                !exception_pending(_py),
                "instance repr must receive its bound receiver"
            );
            assert_eq!(string_obj_to_owned(obj_from_bits(repr_bits)), Some("UnraisableHookArgs(exc_type=1, exc_value=2, exc_traceback=None, err_msg=4, object=True)".to_string()));
            dec_ref_bits(_py, repr_bits);
            dec_ref_bits(_py, bound_repr);
            let class_mutation =
                crate::molt_set_attr_name(class_bits, repr_name_bits, MoltObject::none().bits());
            assert_eq!(
                class_mutation, none,
                "setattr reports failure through the exception channel"
            );
            assert!(
                exception_pending(_py),
                "the hidden runtime type itself must be immutable"
            );
            clear_exception(_py);
            assert_eq!(class_dict_value(_py, class_bits, "__repr__"), original_repr);
            dec_ref_bits(_py, repr_name_bits);

            let source_ptr = alloc_tuple(_py, &fields);
            assert!(!source_ptr.is_null());
            let source_bits = MoltObject::from_ptr(source_ptr).bits();
            let new_name = attr_name_bits_from_bytes(_py, b"__new__").unwrap();
            let constructor = molt_get_attr_name(args_bits, new_name);
            assert_eq!(constructor, class_dict_value(_py, class_bits, "__new__"));
            let builder = crate::molt_callargs_new(
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(0).bits(),
            );
            unsafe { crate::molt_callargs_push_pos(builder, class_bits) };
            unsafe { crate::molt_callargs_push_pos(builder, source_bits) };
            let constructed_bits = crate::molt_call_bind(constructor, builder);
            assert!(
                !exception_pending(_py),
                "nonbinding constructor must retain its optional argument"
            );
            dec_ref_bits(_py, constructor);
            dec_ref_bits(_py, new_name);
            assert!(unraisable_args_is_exact(_py, constructed_bits));
            assert_ne!(constructed_bits, source_bits);
            dec_ref_bits(_py, constructed_bits);
            dec_ref_bits(_py, source_bits);

            let repr_bits = molt_unraisable_hook_args_repr(args_bits);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(repr_bits)),
                Some(
                    "UnraisableHookArgs(exc_type=1, exc_value=2, exc_traceback=None, err_msg=4, object=True)"
                        .to_string()
                )
            );
            dec_ref_bits(_py, repr_bits);

            let err_name_ptr = alloc_string(_py, b"err_msg");
            assert!(!err_name_ptr.is_null());
            let err_name_bits = MoltObject::from_ptr(err_name_ptr).bits();
            let result = crate::molt_set_attr_name(
                args_bits,
                err_name_bits,
                MoltObject::from_int(99).bits(),
            );
            assert_eq!(
                result, none,
                "setattr reports failure through the exception channel"
            );
            assert!(
                exception_pending(_py),
                "structured tuple fields are readonly"
            );
            clear_exception(_py);
            assert_eq!(unraisable_args_field(_py, args_bits, 3), fields[3]);
            dec_ref_bits(_py, err_name_bits);

            dec_ref_bits(_py, args_bits);
        });
    }

    #[test]
    fn armed_transaction_restores_edges_when_the_body_unwinds() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let original = pending_exception(_py, "outer");
            inc_ref_bits(_py, original); // Test-owned observation edge.
            let baseline = ref_count(original);

            let unwind = crate::test_support::catch_expected_unwind(|| {
                let _transaction = UnraisableTransaction::begin(_py);
                assert!(!exception_pending(_py));
                panic!("body panic");
            });
            assert!(unwind.is_err());
            assert_eq!(exception_last_bits_noinc(_py), Some(original));
            assert_eq!(ref_count(original), baseline);

            clear_exception(_py);
            dec_ref_bits(_py, original);
        });
    }

    #[test]
    fn begin_and_drop_recover_a_poisoned_task_exception_map_without_losing_ownership() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let bits = pending_exception(_py, "task-owned");
            inc_ref_bits(_py, bits);
            let baseline = ref_count(bits);
            let detached = take_raised(_py);
            assert!(matches!(detached, RaisedSnapshot::Thread(value) if value == bits));
            let task_ptr = obj_from_bits(bits).as_ptr().expect("task identity pointer");
            let key = PtrSlot(task_ptr);
            let map = task_last_exceptions(_py);

            let poisoned = crate::test_support::catch_expected_unwind(|| {
                let _lock = map.lock().unwrap();
                panic!("poison task exception map");
            });
            assert!(poisoned.is_err() && map.is_poisoned());
            map.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key, PtrSlot(task_ptr));
            crate::CURRENT_TASK.with(|slot| slot.set(task_ptr));
            CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));

            let transaction = UnraisableTransaction::begin(_py);
            assert!(!CURRENT_EXCEPTION_PENDING.with(|pending| pending.get()));
            drop(transaction);
            assert_eq!(ref_count(bits), baseline);

            let restored = {
                let mut guard = map
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                assert_eq!(guard.get(&key).copied(), Some(PtrSlot(task_ptr)));
                guard.remove(&key).expect("restored task exception edge")
            };
            crate::CURRENT_TASK.with(|slot| slot.set(std::ptr::null_mut()));
            CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(false));
            map.clear_poison();
            dec_ref_bits(_py, MoltObject::from_ptr(restored.0).bits());
            dec_ref_bits(_py, bits);
        });
    }

    #[test]
    fn reporter_panic_releases_report_edge_and_restores_original_exactly_once() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let original = pending_exception(_py, "outer");
            inc_ref_bits(_py, original);
            let original_baseline = ref_count(original);
            let mut transaction = UnraisableTransaction::begin(_py);

            let reported = pending_exception(_py, "reported");
            inc_ref_bits(_py, reported);
            let reported_with_channel = ref_count(reported);
            let raised = take_raised(_py);
            transaction.reporting = Some(raised);
            let report = crate::test_support::with_expected_panic(|| {
                transaction.finish_reporting(
                    MoltObject::none().bits(),
                    None,
                    |_py, _context, bits, _message| {
                        assert_eq!(bits, reported);
                        panic!("report panic");
                    },
                )
            });

            assert!(report.is_err());
            assert_eq!(exception_last_bits_noinc(_py), Some(original));
            assert_eq!(ref_count(original), original_baseline);
            assert_eq!(ref_count(reported), reported_with_channel - 1);

            clear_exception(_py);
            dec_ref_bits(_py, original);
            dec_ref_bits(_py, reported);
            // `raised` was transferred into `finish_reporting`; this assertion
            // ensures the test exercised an owned reporting edge.
            assert!(matches!(
                raised,
                RaisedSnapshot::Thread(bits) if bits == reported
            ));
        });
    }

    #[test]
    fn policy_panic_preserves_outer_exception_and_releases_inner_exception() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let original = pending_exception(_py, "outer");
            inc_ref_bits(_py, original);
            let original_baseline = ref_count(original);
            let reported = std::cell::Cell::new(0_u64);
            let reported_baseline = std::cell::Cell::new(0_isize);

            let unwind = crate::test_support::catch_expected_unwind(|| {
                run_unraisable_with_policy(
                    _py,
                    || -> (u64, Option<String>) { panic!("policy panic") },
                    || {
                        let bits = pending_exception(_py, "inner");
                        inc_ref_bits(_py, bits);
                        reported.set(bits);
                        reported_baseline.set(
                            ref_count(bits) as isize
                                + molt_cpython_abi::bridge::GLOBAL_BRIDGE.gc_ref_adjustment(bits),
                        );
                    },
                );
            });
            assert!(unwind.is_err());
            assert_eq!(exception_last_bits_noinc(_py), Some(original));
            assert_eq!(ref_count(original), original_baseline);
            assert_eq!(
                // Formatting can lazily acquire the canonical C-view hold.
                // Count external owners through the collector's authority,
                // then independently require the view to retire below.
                ref_count(reported.get()) as isize
                    + molt_cpython_abi::bridge::GLOBAL_BRIDGE.gc_ref_adjustment(reported.get()),
                reported_baseline.get() - 1,
                "the detached reporting channel owns exactly one reference"
            );

            clear_exception(_py);
            dec_ref_bits(_py, original);
            let reported_view = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(reported.get())
            };
            assert!(!reported_view.is_null());
            dec_ref_bits(_py, reported.get());
            assert!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .managed_handle_for_pyobj(reported_view)
                    .is_none(),
                "the report's final owner must retire its C view"
            );
        });
    }

    #[test]
    fn reentrant_reporting_keeps_outer_transaction_edges_private() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let original = pending_exception(_py, "outer");
            inc_ref_bits(_py, original);
            let original_baseline = ref_count(original);
            let mut transaction = UnraisableTransaction::begin(_py);
            let reported = pending_exception(_py, "reported");
            inc_ref_bits(_py, reported);
            let reported_with_channel = ref_count(reported);
            let raised = take_raised(_py);
            transaction.reporting = Some(raised);

            let report =
                transaction.finish_reporting(MoltObject::none().bits(), None, |_py, _, _, _| {
                    run_unraisable(_py, MoltObject::none().bits(), None, || ());
                });
            assert!(report.is_ok());
            assert_eq!(exception_last_bits_noinc(_py), Some(original));
            assert_eq!(ref_count(original), original_baseline);
            assert_eq!(ref_count(reported), reported_with_channel - 1);

            clear_exception(_py);
            dec_ref_bits(_py, original);
            dec_ref_bits(_py, reported);
            assert!(matches!(
                raised,
                RaisedSnapshot::Thread(bits) if bits == reported
            ));
        });
    }

    #[test]
    fn impossible_reentrant_restore_defers_both_handled_edge_classes_and_fails_closed() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let active_ptr = alloc_exception(_py, "RuntimeError", "active");
            let fallback_ptr = alloc_exception(_py, "RuntimeError", "fallback");
            assert!(!active_ptr.is_null() && !fallback_ptr.is_null());
            let active = MoltObject::from_ptr(active_ptr).bits();
            let fallback = MoltObject::from_ptr(fallback_ptr).bits();
            inc_ref_bits(_py, active);
            inc_ref_bits(_py, fallback);
            ACTIVE_EXCEPTION_STACK.with(|stack| stack.borrow_mut().push(active));
            ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
                stack.borrow_mut().push(ExceptionContextFallback {
                    bits: fallback,
                    owned: true,
                });
            });
            let active_with_channel = ref_count(active);
            let fallback_with_channel = ref_count(fallback);
            let transaction = UnraisableTransaction::begin(_py);

            let failure = crate::test_support::catch_expected_unwind(|| {
                ACTIVE_EXCEPTION_STACK.with(|stack| {
                    let _borrow = stack.borrow_mut();
                    let _ = transaction.finish_raised(
                        RaisedSnapshot::None,
                        MoltObject::none().bits(),
                        None,
                    );
                });
            });
            assert!(failure.is_err());
            assert!(ACTIVE_EXCEPTION_STACK.with(|stack| stack.borrow().is_empty()));
            assert!(ACTIVE_EXCEPTION_FALLBACK.with(|stack| stack.borrow().is_empty()));
            assert_eq!(ref_count(active), active_with_channel);
            assert_eq!(ref_count(fallback), fallback_with_channel);
            assert!(
                flush_deferred_handled(_py),
                "state must restore once the reentrant borrow ends"
            );
            assert!(ACTIVE_EXCEPTION_STACK.with(|stack| stack.borrow().as_slice() == [active]));
            assert!(ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
                let stack = stack.borrow();
                stack.len() == 1 && stack[0].owned && stack[0].bits == fallback
            }));

            let active_saved =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            let fallback_saved =
                ACTIVE_EXCEPTION_FALLBACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            release_stack(_py, active_saved);
            release_fallback_stack(_py, fallback_saved);
            assert_eq!(ref_count(active), active_with_channel - 1);
            assert_eq!(ref_count(fallback), fallback_with_channel - 1);

            dec_ref_bits(_py, active);
            dec_ref_bits(_py, fallback);
        });
    }

    #[test]
    fn default_hook_prefix_matches_cpython_punctuation() {
        assert_eq!(default_unraisable_prefix(true, "ignored", None), None);
        assert_eq!(
            default_unraisable_prefix(false, "callback", None),
            Some("Exception ignored in: callback".to_string())
        );
        assert_eq!(
            default_unraisable_prefix(true, "ignored", Some("message")),
            Some("message:".to_string())
        );
        assert_eq!(
            default_unraisable_prefix(false, "callback", Some("message")),
            Some("message: callback".to_string())
        );
    }
}
