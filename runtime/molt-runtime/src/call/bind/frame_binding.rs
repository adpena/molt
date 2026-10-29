//! Python frame admission, signature/default binding and adopting entry calls.
//!
//! State stays with its existing owner; calls preserve transaction and drop order.

use super::*;
use crate::BuiltinExc;
use crate::runtime_flags::{self, RuntimeFlag};

/// A Python frame's declared parameter layout (CPython `co_varnames`):
/// positional parameters, keyword-only parameters, `*args`, then `**kwargs`.
/// The compiled ABI places `*args` before the keyword-only parameters; slots
/// keep ABI order and release in declared order.
#[derive(Clone, Copy)]
struct FrameSlotLayout {
    positional: usize,
    has_vararg: bool,
    keyword_only: usize,
    has_varkw: bool,
}

impl FrameSlotLayout {
    fn vararg_slot(self) -> usize {
        self.positional
    }

    fn keyword_only_slot(self, index: usize) -> usize {
        self.positional + usize::from(self.has_vararg) + index
    }

    fn varkw_slot(self) -> usize {
        self.keyword_only_slot(self.keyword_only)
    }

    fn len(self) -> usize {
        self.varkw_slot() + usize::from(self.has_varkw)
    }

    /// ABI slot indices in declared frame order (`co_varnames`).
    fn declared_order(self) -> impl DoubleEndedIterator<Item = usize> {
        (0..self.positional)
            .chain(self.keyword_only_slot(0)..self.varkw_slot())
            .chain(self.has_vararg.then_some(self.vararg_slot()))
            .chain(self.has_varkw.then_some(self.varkw_slot()))
    }
}

/// The bound frame's argument owners, in ABI slot order.
struct BoundCallSlots<'a, 'py> {
    py: &'a PyToken<'py>,
    layout: FrameSlotLayout,
    values: Vec<Option<u64>>,
}

impl<'a, 'py> BoundCallSlots<'a, 'py> {
    fn new(py: &'a PyToken<'py>, layout: FrameSlotLayout) -> Result<Self, u64> {
        let count = layout.len();
        let mut values = Vec::new();
        if values.try_reserve_exact(count).is_err() {
            return Err(raise_exception::<_>(
                py,
                BuiltinExc::MemoryError,
                "bound arguments allocation failed",
            ));
        }
        values.resize(count, None);
        Ok(Self { py, layout, values })
    }

    fn set_borrowed(&mut self, index: usize, bits: u64) {
        inc_ref_bits(self.py, bits);
        self.set_owned(index, bits);
    }

    fn set_owned(&mut self, index: usize, bits: u64) {
        let previous = self.values[index].replace(bits);
        if let Some(previous) = previous {
            dec_ref_bits(self.py, previous);
        }
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn release_slot(&mut self, slot: usize) {
        if let Some(value) = self.values[slot].take() {
            dec_ref_bits(self.py, value);
        }
    }

    /// An adopting entry takes every bound parameter over; the slots release
    /// none of them afterwards.
    fn surrender_to_entry(&mut self) {
        for value in &mut self.values {
            value.take();
        }
    }
}

impl std::ops::Index<usize> for BoundCallSlots<'_, '_> {
    type Output = Option<u64>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.values[index]
    }
}

impl Drop for BoundCallSlots<'_, '_> {
    fn drop(&mut self) {
        // `_PyFrame_ClearLocals` walks the declared slots first to last through
        // 3.13 and last to first from 3.14; the ABI slot order is unchanged.
        // The target version matters only for two or more owners.
        let owned = self.values.iter().filter(|value| value.is_some()).count();
        let descending =
            owned > 1 && crate::object::ops_sys::runtime_target_at_least(self.py, 3, 14);
        let order = self.layout.declared_order();
        if descending {
            for slot in order.rev() {
                self.release_slot(slot);
            }
        } else {
            for slot in order {
                self.release_slot(slot);
            }
        }
    }
}

/// Binding custody for one Python frame (see `Admission`). A failed CALL
/// binding releases the unbound keyword values before the partial frame, as
/// `initialize_locals` does. Any other failed binding releases the frame's own
/// references first, leaving the call's owners last.
struct FrameBinding<'a, 'py> {
    admission: Admission,
    arguments: std::mem::ManuallyDrop<CallArguments<'a, 'py>>,
    slots: std::mem::ManuallyDrop<BoundCallSlots<'a, 'py>>,
}

impl<'a, 'py> FrameBinding<'a, 'py> {
    fn new(arguments: CallArguments<'a, 'py>, slots: BoundCallSlots<'a, 'py>) -> Self {
        Self {
            admission: arguments.admission(),
            arguments: std::mem::ManuallyDrop::new(arguments),
            slots: std::mem::ManuallyDrop::new(slots),
        }
    }

    /// Binding succeeded: the caller now owns both parts.
    fn into_parts(self) -> (CallArguments<'a, 'py>, BoundCallSlots<'a, 'py>) {
        let mut binding = std::mem::ManuallyDrop::new(self);
        // SAFETY: `binding` is never dropped, so each part is taken once.
        unsafe {
            (
                std::mem::ManuallyDrop::take(&mut binding.arguments),
                std::mem::ManuallyDrop::take(&mut binding.slots),
            )
        }
    }
}

impl Drop for FrameBinding<'_, '_> {
    fn drop(&mut self) {
        // SAFETY: each part is dropped exactly once, here.
        unsafe {
            match self.admission {
                Admission::Move => {
                    std::mem::ManuallyDrop::drop(&mut self.arguments);
                    std::mem::ManuallyDrop::drop(&mut self.slots);
                }
                Admission::Copy => {
                    std::mem::ManuallyDrop::drop(&mut self.slots);
                    std::mem::ManuallyDrop::drop(&mut self.arguments);
                }
            }
        }
    }
}

/// Read the current dictionary for this parameter, pin it across rich lookup,
/// and immediately transfer an owned value to the binding frame.
unsafe fn function_kwdefault_owned(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
    name_bits: u64,
) -> Result<Option<u64>, u64> {
    unsafe {
        let defaults_bits = function_attr_bits(
            py,
            func_ptr,
            intern_static_name(
                py,
                &runtime_state(py).interned.kwdefaults_name,
                FunctionBindingField::KeywordDefaults.name(),
            ),
        )
        .map_err(|()| MoltObject::none().bits())?
        .unwrap_or_else(|| MoltObject::none().bits());
        if obj_from_bits(defaults_bits).is_none() {
            return Ok(None);
        }
        let Some(defaults_ptr) = obj_from_bits(defaults_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                py,
                BuiltinExc::TypeError,
                "call expects function object",
            ));
        };
        inc_ref_bits(py, defaults_bits);
        let _owner = PtrDropGuard::new(defaults_ptr);
        let backing_bits = crate::object::ops::dict_backing_bits(py, defaults_bits)
            .map_err(|()| MoltObject::none().bits())?
            .ok_or_else(|| {
                raise_exception::<u64>(py, BuiltinExc::TypeError, "call expects function object")
            })?;
        let backing_ptr = obj_from_bits(backing_bits)
            .as_ptr()
            .expect("dictionary admission returned backing storage");
        // The public mapping stays visible as __kwdefaults__. Rich key lookup
        // may replace that metadata or its subclass state, so own a distinct
        // backing dictionary through the callback as well.
        let _backing_owner = if backing_ptr != defaults_ptr {
            inc_ref_bits(py, backing_bits);
            Some(PtrDropGuard::new(backing_ptr))
        } else {
            None
        };
        let value = dict_get_in_place(py, backing_ptr, name_bits);
        let value = match value {
            Ok(value) => value,
            Err(()) => {
                return Err(MoltObject::none().bits());
            }
        };
        if let Some(value) = value {
            inc_ref_bits(py, value);
        }
        Ok(value)
    }
}

fn trace_function_bind_meta_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_FUNCTION_BIND_META)
}

/// Callback-free derivation of the published binder flag. Positional-default
/// counts remain live metadata reads because direct calls can pad them without
/// selecting the full binder.
pub(super) struct FunctionBindingShape {
    pub(super) full_binder: bool,
}

pub(super) unsafe fn function_binding_meta(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
    field: FunctionBindingField,
) -> u64 {
    unsafe { crate::call::function::function_metadata_bits(py, func_ptr, field.name()) }
}

/// Read current positional defaults without retaining a second derived fact.
/// Missing/None and an empty tuple have count zero; malformed metadata has no
/// count and must be admitted by the full binder for its existing diagnostic.
unsafe fn positional_default_count(py: &PyToken<'_>, func_ptr: *mut u8) -> Option<usize> {
    unsafe {
        let defaults = obj_from_bits(function_binding_meta(
            py,
            func_ptr,
            FunctionBindingField::Defaults,
        ));
        if defaults.is_none() {
            return Some(0);
        }
        match defaults.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_TUPLE => {
                Some(crate::object::seq_access::len(ptr))
            }
            _ => None,
        }
    }
}

pub(super) unsafe fn function_binding_shape(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
) -> FunctionBindingShape {
    unsafe {
        let fixed_builtin =
            crate::builtins::functions::native_callable::fixed_builtin_convention(func_ptr)
                .is_some();
        let mut full_binder = builtin_args::builtin_call_binding(py, func_ptr).is_some();
        for name in [
            FunctionBindingField::BindKind,
            FunctionBindingField::Varargs,
            FunctionBindingField::VarKeywords,
        ] {
            if name != FunctionBindingField::BindKind || !fixed_builtin {
                let value = obj_from_bits(function_binding_meta(py, func_ptr, name));
                full_binder |= !value.is_none()
                    && (name != FunctionBindingField::BindKind
                        || value
                            .as_int()
                            .and_then(BuiltinBindingPolicy::from_abi)
                            .is_none_or(BuiltinBindingPolicy::requires_custom_binding));
            }
        }
        let kwonly = obj_from_bits(function_binding_meta(
            py,
            func_ptr,
            FunctionBindingField::KeywordOnlyNames,
        ));
        if !kwonly.is_none() {
            full_binder |= match kwonly.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_TUPLE => {
                    crate::object::seq_access::len(ptr) != 0
                }
                _ => true,
            };
        }
        let kwdefaults = obj_from_bits(function_binding_meta(
            py,
            func_ptr,
            FunctionBindingField::KeywordDefaults,
        ));
        // Any keyword-default mapping requires the binder, which also covers
        // keyword-only parameters absent from recorded metadata. Presence,
        // unlike the live dictionary's size, changes only through a metadata
        // write that refreshes the binder flag.
        full_binder |= !kwdefaults.is_none();
        full_binder |= positional_default_count(py, func_ptr).is_none();
        FunctionBindingShape { full_binder }
    }
}

/// Positional defaults can be padded by the direct path of
/// `call_owned_function`; every other binder requirement, including malformed
/// defaults, is maintained by publication. The live count also observes public
/// defaults replacement without caching or rederiving the full binding shape.
pub(super) unsafe fn function_requires_full_binding(py: &PyToken<'_>, func_ptr: *mut u8) -> bool {
    unsafe {
        function_requires_binder_flag(func_ptr) || positional_default_count(py, func_ptr) != Some(0)
    }
}

pub(crate) unsafe fn function_needs_full_binder(py: &PyToken<'_>, func_ptr: *mut u8) -> bool {
    unsafe { function_binding_shape(py, func_ptr).full_binder }
}

pub(crate) unsafe fn refresh_function_requires_binder_flag(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
) -> bool {
    unsafe {
        let needs_binder = function_needs_full_binder(_py, func_ptr);
        let header = header_from_obj_ptr(func_ptr);
        if needs_binder {
            (*header).fetch_or_flags(HEADER_FLAG_FUNC_REQUIRES_BINDER);
        } else {
            (*header).fetch_and_flags(!HEADER_FLAG_FUNC_REQUIRES_BINDER);
        }
        // Publication derives the policy while metadata is callback-free and
        // checks the stored projection before displaced owners can finalize.
        // Calls consume that projection; they must not repeat this analysis.
        debug_assert_eq!(function_requires_binder_flag(func_ptr), needs_binder);
        needs_binder
    }
}

pub(crate) unsafe fn function_requires_binder_flag(func_ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(func_ptr);
        ((*header).load_metadata_flags() & HEADER_FLAG_FUNC_REQUIRES_BINDER) != 0
    }
}

/// Every construction and metadata write refreshes the binder flag, so the
/// flag is exact in both states and an exact-arity call reads only the header.
pub(crate) unsafe fn function_raw_positional_call_needs_binding(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    supplied: usize,
) -> bool {
    unsafe {
        if function_requires_binder_flag(func_ptr) {
            return true;
        }
        let Some(arity) = function_arity_usize(func_ptr) else {
            let _ = raise_exception::<u64>(
                _py,
                BuiltinExc::OverflowError,
                "function arity exceeds the active address space",
            );
            return true;
        };
        if supplied == arity {
            return false;
        }
        // Wrong Python arity belongs to frame binding even without defaults.
        // Native callables without a Python signature keep their own admission.
        crate::builtins::functions::native_callable::fixed_builtin_convention(func_ptr).is_some()
            || positional_default_count(_py, func_ptr) != Some(0)
            || !obj_from_bits(function_binding_meta(
                _py,
                func_ptr,
                FunctionBindingField::ArgumentNames,
            ))
            .is_none()
    }
}

/// A function object either borrows the argument vector (exact-arity
/// trampolines and the builtin, extension and positional-builtin binders) or
/// binds a Python frame (T2) as the call's custody allows (`Admission`).
pub(super) unsafe fn call_function_with_arguments(
    _py: &PyToken<'_>,
    func_bits: u64,
    func_ptr: *mut u8,
    mut args: CallArguments<'_, '_>,
) -> u64 {
    unsafe {
        if !crate::builtins::functions::native_callable::admit_native_call(
            _py,
            func_ptr,
            args.positional().first().copied(),
        ) {
            return MoltObject::none().bits();
        }

        if let Some(result) = crate::cpython_abi_hooks::try_call_cext(
            _py,
            func_ptr,
            crate::cpython_abi_hooks::CExtCallArguments::Owned(&mut args),
        ) {
            return result;
        }
        if function_trampoline_ptr(func_ptr) != 0
            && args.keyword_count() == 0
            && !function_raw_positional_call_needs_binding(_py, func_ptr, args.positional().len())
        {
            // Exact positional arity: an inlined frame takes these values over
            // as its parameters. An adopting entry owns them from here; a
            // borrowing entry borrows the vector, which releases them in frame
            // order after it returns.
            if args.custody() == ArgumentCustody::Frame && function_bits_adopt_arguments(func_bits)
            {
                return call_function_obj_moved(_py, func_bits, args.surrender_positional());
            }
            args.enter_inlined_frame();
            return call_function_obj_bound_vec(_py, func_bits, args.positional());
        }
        let bind_kind_bits = match function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_bind_kind,
                FunctionBindingField::BindKind.name(),
            ),
        ) {
            Ok(value) => value,
            Err(()) => return MoltObject::none().bits(),
        };
        if let Some(kind_bits) = bind_kind_bits
            && obj_from_bits(kind_bits).as_int() == Some(BIND_KIND_CAPI_METHOD)
        {
            return call_capi_method_with_bound_args(_py, func_bits, &args);
        }
        if let Some(convention) =
            crate::builtins::functions::native_callable::fixed_builtin_convention(func_ptr)
        {
            if let Some(error) =
                convention.argument_error(args.positional().len(), args.keyword_count())
            {
                let Ok(name) =
                    crate::builtins::functions::native_callable::native_function_error_name(
                        _py, func_ptr,
                    )
                else {
                    return MoltObject::none().bits();
                };
                return crate::builtins::exceptions::raise_exception_bytes(
                    _py,
                    BuiltinExc::TypeError,
                    &error.message(&name),
                );
            }
            if args.custody() == ArgumentCustody::Frame && function_bits_adopt_arguments(func_bits)
            {
                return call_function_obj_moved(_py, func_bits, args.surrender_positional());
            }
            args.enter_inlined_frame();
            return call_function_obj_bound_vec(_py, func_bits, args.positional());
        }
        let receiver_origin = args.receiver_origin;
        let view = match args.unpacked_view() {
            Ok(view) => view,
            Err(err) => return err,
        };
        if let Some(binding) = builtin_args::builtin_call_binding(_py, func_ptr) {
            return binding.call(_py, func_bits, func_ptr, &view);
        }

        let arg_names = match python_argument_names(_py, func_ptr) {
            Ok(Some(names)) => names,
            Ok(None) => {
                // A fixed one-slot method descriptor takes only its receiver.
                // C-extension, specialized and Python named binders already had
                // their opportunity; no argument packing is needed here.
                if let Some(admitted) =
                    crate::builtins::functions::native_callable::admit_native_noargs(
                        _py,
                        func_ptr,
                        receiver_origin,
                        view.pos,
                        view.kw_names.len(),
                    )
                {
                    return if admitted {
                        call_function_obj_bound_vec(_py, func_bits, view.pos)
                    } else {
                        MoltObject::none().bits()
                    };
                }
                if let Some(bound_args) =
                    builtin_args::bind_positional_builtin_call(_py, func_bits, func_ptr, &view)
                {
                    return call_function_obj_bound_vec(_py, func_bits, bound_args.as_slice());
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "call expects function object",
                );
            }
            Err(error) => return error,
        };
        match bind_python_frame(_py, func_ptr, args, arg_names) {
            Ok(frame) => frame.invoke(func_bits),
            Err(error) => error,
        }
    }
}

/// A successfully bound Python frame. The canonical slot owner preserves the
/// target-version release order; instruction owners end after those slots.
struct BoundPythonFrame<'a, 'py> {
    slots: BoundCallSlots<'a, 'py>,
    _instruction_owners: Option<CallArguments<'a, 'py>>,
    values: Vec<u64>,
}

impl BoundPythonFrame<'_, '_> {
    unsafe fn invoke(mut self, func_bits: u64) -> u64 {
        unsafe {
            if function_bits_adopt_arguments(func_bits) {
                self.slots.surrender_to_entry();
                call_function_obj_moved(self.slots.py, func_bits, &self.values)
            } else {
                call_function_obj_bound_vec(self.slots.py, func_bits, &self.values)
            }
        }
    }
}

unsafe fn python_argument_names<'a, 'py>(
    py: &'a PyToken<'py>,
    function: *mut u8,
) -> Result<Option<crate::object::seq_access::PinnedTuple<'a, 'py>>, u64> {
    unsafe {
        let bits = function_attr_bits(
            py,
            function,
            intern_static_name(
                py,
                &runtime_state(py).interned.molt_arg_names,
                FunctionBindingField::ArgumentNames.name(),
            ),
        )
        .map_err(|()| MoltObject::none().bits())?;
        let Some(bits) = bits else {
            return Ok(None);
        };
        let Some(pointer) = obj_from_bits(bits).as_ptr() else {
            return Err(raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "call expects function object",
            ));
        };
        let Some(names) = crate::object::seq_access::pin_tuple(py, pointer) else {
            return Err(raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "call expects function object",
            ));
        };
        Ok(Some(names))
    }
}

#[cold]
unsafe fn raise_missing_arguments(
    py: &PyToken<'_>,
    function: *mut u8,
    kind: &str,
    names: impl Iterator<Item = u64>,
) -> u64 {
    let function = unsafe { function_name_bits(py, function) };
    let function =
        string_obj_to_owned(obj_from_bits(function)).unwrap_or_else(|| "function".to_string());
    let mut missing = Vec::new();
    for name in names {
        missing.push(crate::format_obj(py, obj_from_bits(name)));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
    }
    let count = missing.len();
    let names = match missing.as_slice() {
        [name] => name.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => format!(
            "{}, and {}",
            missing[..count - 1].join(", "),
            missing[count - 1],
        ),
    };
    let plural = if count == 1 { "" } else { "s" };
    raise_exception::<u64>(
        py,
        BuiltinExc::TypeError,
        &format!("{function}() missing {count} required {kind} argument{plural}: {names}"),
    )
}

/// Bind using the same metadata, defaults, keyword matching and owner ordering
/// as ordinary invocation. This operation does not run the function body.
unsafe fn bind_python_frame<'a, 'py>(
    _py: &'a PyToken<'py>,
    func_ptr: *mut u8,
    args: CallArguments<'a, 'py>,
    arg_names: crate::object::seq_access::PinnedTuple<'a, 'py>,
) -> Result<BoundPythonFrame<'a, 'py>, u64> {
    unsafe {
        let posonly_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_posonly,
                FunctionBindingField::PositionalOnly.name(),
            ),
        )
        .map_err(|()| MoltObject::none().bits())?
        .unwrap_or_else(|| MoltObject::from_int(0).bits());
        let posonly = obj_from_bits(posonly_bits).as_int().unwrap_or(0).max(0) as usize;

        let kwonly_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_kwonly_names,
                FunctionBindingField::KeywordOnlyNames.name(),
            ),
        )
        .map_err(|()| MoltObject::none().bits())?
        .unwrap_or_else(|| MoltObject::none().bits());
        let kwonly_names_pin = if obj_from_bits(kwonly_bits).is_none() {
            None
        } else {
            let Some(kw_ptr) = obj_from_bits(kwonly_bits).as_ptr() else {
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "call expects function object",
                ));
            };
            if object_type_id(kw_ptr) != TYPE_ID_TUPLE {
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "call expects function object",
                ));
            }
            Some(
                crate::object::seq_access::pin_tuple(_py, kw_ptr)
                    .expect("type-checked keyword-only tuple must be pinnable"),
            )
        };
        let kwonly_names: &[u64] = kwonly_names_pin.as_deref().unwrap_or(&[]);

        let vararg_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_vararg,
                FunctionBindingField::Varargs.name(),
            ),
        )
        .map_err(|()| MoltObject::none().bits())?
        .unwrap_or_else(|| MoltObject::none().bits());
        let varkw_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_varkw,
                FunctionBindingField::VarKeywords.name(),
            ),
        )
        .map_err(|()| MoltObject::none().bits())?
        .unwrap_or_else(|| MoltObject::none().bits());
        let has_vararg = !obj_from_bits(vararg_bits).is_none();
        let has_varkw = !obj_from_bits(varkw_bits).is_none();

        if trace_function_bind_meta_enabled() {
            let func_name_bits = function_name_bits(_py, func_ptr);
            let func_name = if func_name_bits == 0 || obj_from_bits(func_name_bits).is_none() {
                "<unnamed>".to_string()
            } else {
                string_obj_to_owned(obj_from_bits(func_name_bits))
                    .unwrap_or_else(|| "<unnamed>".to_string())
            };
            eprintln!(
                "[molt bind_meta] name={} total_pos={} posonly={} kwonly={} has_vararg={} has_varkw={} defaults_phase=pending",
                func_name,
                arg_names.len(),
                posonly,
                kwonly_names.len(),
                has_vararg,
                has_varkw,
            );
        }

        let positional_range = obj_from_bits(function_binding_meta(
            _py,
            func_ptr,
            FunctionBindingField::BindKind,
        ))
        .as_int()
        .and_then(BuiltinBindingPolicy::from_abi)
            == Some(BuiltinBindingPolicy::PositionalRange);
        if positional_range {
            if posonly != arg_names.len()
                || !kwonly_names.is_empty()
                || has_vararg
                || has_varkw
                || function_arity_usize(func_ptr) != Some(arg_names.len())
            {
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::SystemError,
                    "invalid fixed positional builtin metadata",
                ));
            }
            // Expansion and argument evaluation already completed. Native
            // positional-only callables reject keywords before arity checks.
            if args.keyword_count() != 0 {
                let name = string_obj_to_owned(obj_from_bits(function_name_bits(_py, func_ptr)))
                    .unwrap_or_else(|| "function".to_string());
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    &format!("{name}() takes no keyword arguments"),
                ));
            }
        }

        let layout = FrameSlotLayout {
            positional: arg_names.len(),
            has_vararg,
            keyword_only: kwonly_names.len(),
            has_varkw,
        };
        let total_pos = layout.positional;
        let slots = BoundCallSlots::new(_py, layout)?;
        // T2: an inlined CALL frame takes the call's arguments over; any other
        // binding gives the frame its own references (`Admission`).
        let mut binding = FrameBinding::new(args, slots);
        let admission = binding.admission;
        // Match initialize_locals: own **kwargs, positional slots, and
        // *args before rich keyword matching can call Python.
        let varkw_ptr = if has_varkw {
            let dictionary = alloc_dict_with_pairs(_py, &[]);
            if dictionary.is_null() {
                return Err(MoltObject::none().bits());
            }
            binding
                .slots
                .set_owned(layout.varkw_slot(), MoltObject::from_ptr(dictionary).bits());
            Some(dictionary)
        } else {
            None
        };
        let supplied = binding.arguments.positional().len();
        let bound = supplied.min(total_pos);
        match admission {
            Admission::Move => {
                for idx in 0..bound {
                    let value = binding.arguments.take_positional();
                    binding.slots.set_owned(idx, value);
                }
                if has_vararg {
                    let Some(tuple_bits) = binding.arguments.take_positional_tuple() else {
                        return Err(MoltObject::none().bits());
                    };
                    binding.slots.set_owned(layout.vararg_slot(), tuple_bits);
                } else {
                    // initialize_locals releases surplus positional values once
                    // they are known surplus; the arity error follows keyword
                    // binding.
                    binding.arguments.release_surplus_positional();
                }
            }
            Admission::Copy => {
                for idx in 0..bound {
                    let value = binding.arguments.positional()[idx];
                    binding.slots.set_borrowed(idx, value);
                }
                if has_vararg {
                    let Some(tuple_bits) = binding.arguments.copy_positional_tuple(bound) else {
                        return Err(MoltObject::none().bits());
                    };
                    binding.slots.set_owned(layout.vararg_slot(), tuple_bits);
                }
            }
        }

        for index in 0..binding.arguments.keyword_len() {
            let (name, value) = binding.arguments.keyword_entry(index);
            // CPython checks all parameter identities before performing
            // ordered rich equality. A str subclass can override equality;
            // converting it to a Rust String would erase that callback.
            let parameters = arg_names.iter().copied().enumerate().skip(posonly).chain(
                kwonly_names
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(i, name)| (layout.keyword_only_slot(i), name)),
            );
            let mut matched = parameters
                .clone()
                .find(|(_, parameter)| *parameter == name)
                .map(|(slot, _)| slot);
            if matched.is_none() {
                for (slot, parameter) in parameters {
                    match crate::object::ops_compare::compare_object_eq_bool(
                        _py,
                        obj_from_bits(parameter),
                        obj_from_bits(name),
                    ) {
                        crate::object::ops_compare::CompareBoolOutcome::True => {
                            matched = Some(slot);
                            break;
                        }
                        crate::object::ops_compare::CompareBoolOutcome::False => {}
                        _ => return Err(MoltObject::none().bits()),
                    }
                }
            }
            if let Some(slot) = matched {
                if binding.slots[slot].is_some() {
                    let function = function_name_bits(_py, func_ptr);
                    let function = string_obj_to_owned(obj_from_bits(function))
                        .unwrap_or_else(|| "function".to_string());
                    let name = crate::format_obj_str(_py, obj_from_bits(name));
                    if exception_pending(_py) {
                        return Err(MoltObject::none().bits());
                    }
                    return Err(raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        &format!("{function}() got multiple values for argument '{name}'"),
                    ));
                }
                let owned = match admission {
                    Admission::Move => binding.arguments.take_keyword(index),
                    Admission::Copy => {
                        inc_ref_bits(_py, value);
                        value
                    }
                };
                binding.slots.set_owned(slot, owned);
            } else if let Some(dictionary) = varkw_ptr {
                // Positional-only names are ordinary entries in **kwargs.
                // Insertion callbacks belong to keyword binding, before
                // positional arity checks and live default resolution.
                let completed = crate::dict_set_in_place(_py, dictionary, name, value);
                if !completed {
                    return Err(MoltObject::none().bits());
                }
                // The dictionary retained its own entry; a moved edge ends here.
                if admission == Admission::Move {
                    dec_ref_bits(_py, binding.arguments.take_keyword(index));
                }
            } else {
                let mut conflicts = Vec::new();
                for &parameter in arg_names.iter().take(posonly) {
                    for &keyword in binding.arguments.keyword_names() {
                        match crate::object::ops_compare::compare_object_eq_bool(
                            _py,
                            obj_from_bits(parameter),
                            obj_from_bits(keyword),
                        ) {
                            crate::object::ops_compare::CompareBoolOutcome::True => {
                                conflicts.push(
                                    string_obj_to_owned(obj_from_bits(parameter))
                                        .expect("parameter name string"),
                                );
                                break;
                            }
                            crate::object::ops_compare::CompareBoolOutcome::False => {}
                            _ => return Err(MoltObject::none().bits()),
                        }
                    }
                }
                if !conflicts.is_empty() {
                    let function = function_name_bits(_py, func_ptr);
                    let function = string_obj_to_owned(obj_from_bits(function))
                        .unwrap_or_else(|| "function".to_string());
                    return Err(raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        &format!(
                            "{function}() got some positional-only arguments passed as keyword arguments: '{}'",
                            conflicts.join(", "),
                        ),
                    ));
                }
                let function = function_name_bits(_py, func_ptr);
                let function = string_obj_to_owned(obj_from_bits(function))
                    .unwrap_or_else(|| "function".to_string());
                let name = crate::format_obj_str(_py, obj_from_bits(name));
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    &format!("{function}() got an unexpected keyword argument '{name}'"),
                ));
            }
        }

        // Keyword callbacks and their errors precede positional arity and
        // default resolution, as in CPython initialize_locals.
        let defaults_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.defaults_name,
                FunctionBindingField::Defaults.name(),
            ),
        )
        .map_err(|()| MoltObject::none().bits())?
        .unwrap_or_else(|| MoltObject::none().bits());
        let defaults_pin = if obj_from_bits(defaults_bits).is_none() {
            None
        } else {
            let Some(def_ptr) = obj_from_bits(defaults_bits).as_ptr() else {
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "call expects function object",
                ));
            };
            if object_type_id(def_ptr) != TYPE_ID_TUPLE {
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "call expects function object",
                ));
            }
            Some(
                crate::object::seq_access::pin_tuple(_py, def_ptr)
                    .expect("type-checked defaults tuple must be pinnable"),
            )
        };
        let defaults: &[u64] = defaults_pin.as_deref().unwrap_or(&[]);

        let defaults_len = defaults.len();
        if positional_range {
            let Some(required) = total_pos.checked_sub(defaults_len) else {
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::SystemError,
                    "invalid fixed positional builtin defaults",
                ));
            };
            if supplied < required || supplied > total_pos {
                let name = string_obj_to_owned(obj_from_bits(function_name_bits(_py, func_ptr)))
                    .unwrap_or_else(|| "function".to_string());
                let (qualifier, expected) = if required == total_pos {
                    ("", total_pos)
                } else if supplied < required {
                    ("at least ", required)
                } else {
                    ("at most ", total_pos)
                };
                let plural = if expected == 1 { "" } else { "s" };
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    &format!(
                        "{name} expected {qualifier}{expected} argument{plural}, got {supplied}"
                    ),
                ));
            }
        }
        if supplied > total_pos && !has_vararg {
            // CPython reports the live default range and only keyword-only
            // arguments already supplied, before filling any defaults.
            let keyword_only_given = (0..kwonly_names.len())
                .filter(|&index| binding.slots[layout.keyword_only_slot(index)].is_some())
                .count();
            drop(defaults_pin);
            let function = function_name_bits(_py, func_ptr);
            let function = string_obj_to_owned(obj_from_bits(function))
                .unwrap_or_else(|| "function".to_string());
            let (expected, plural) = if defaults_len != 0 {
                (
                    format!(
                        "from {} to {total_pos}",
                        total_pos as i128 - defaults_len as i128
                    ),
                    "s",
                )
            } else {
                (total_pos.to_string(), if total_pos == 1 { "" } else { "s" })
            };
            let keyword_only = if keyword_only_given != 0 {
                format!(
                    " positional argument{} (and {keyword_only_given} keyword-only argument{})",
                    if supplied == 1 { "" } else { "s" },
                    if keyword_only_given == 1 { "" } else { "s" },
                )
            } else {
                String::new()
            };
            let verb = if supplied == 1 && keyword_only_given == 0 {
                "was"
            } else {
                "were"
            };
            return Err(raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                &format!(
                    "{function}() takes {expected} positional argument{plural} but {supplied}{keyword_only} {verb} given",
                ),
            ));
        }
        let default_start = total_pos.saturating_sub(defaults_len);
        let defaults_offset = defaults_len.saturating_sub(total_pos);
        for idx in 0..total_pos {
            if binding.slots[idx].is_some() {
                continue;
            }
            if idx >= default_start {
                binding
                    .slots
                    .set_borrowed(idx, defaults[defaults_offset + idx - default_start]);
                continue;
            }
            let name = string_obj_to_owned(obj_from_bits(arg_names[idx]))
                .unwrap_or_else(|| "?".to_string());
            if runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_CALL_BIND_MISSING) {
                let func_name_bits = function_name_bits(_py, func_ptr);
                let func_name = if func_name_bits == 0 || obj_from_bits(func_name_bits).is_none() {
                    "<function>".to_string()
                } else {
                    string_obj_to_owned(obj_from_bits(func_name_bits))
                        .unwrap_or_else(|| "<function>".to_string())
                };
                eprintln!(
                    "molt call_bind: missing required arg func={} arg={} pos={}",
                    func_name,
                    name,
                    idx + 1
                );
            }
            return Err(raise_missing_arguments(
                _py,
                func_ptr,
                "positional",
                (0..default_start)
                    .filter(|&index| binding.slots[index].is_none())
                    .map(|index| arg_names[index]),
            ));
        }

        // Each bound slot now owns its default. Do not retain unrelated
        // tuple elements across later keyword-default callbacks.
        drop(defaults_pin);

        let mut first_missing_kwonly = None;
        for (kw_idx, name_bits) in kwonly_names.iter().copied().enumerate() {
            let slot_idx = layout.keyword_only_slot(kw_idx);
            if binding.slots[slot_idx].is_some() {
                continue;
            }
            let default = function_kwdefault_owned(_py, func_ptr, name_bits)?;
            if let Some(val) = default {
                binding.slots.set_owned(slot_idx, val);
                continue;
            }
            first_missing_kwonly.get_or_insert(name_bits);
        }
        // Resolve every remaining keyword default before reporting missing
        // parameters: later rich lookups can raise or mutate metadata.
        if first_missing_kwonly.is_some() {
            return Err(raise_missing_arguments(
                _py,
                func_ptr,
                "keyword-only",
                kwonly_names
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(index, _)| binding.slots[layout.keyword_only_slot(*index)].is_none())
                    .map(|(_, name)| name),
            ));
        }

        let mut final_args: Vec<u64> = Vec::with_capacity(binding.slots.len());
        for slot in &binding.slots.values {
            let Some(val) = *slot else {
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "call binding failed",
                ));
            };
            final_args.push(val);
        }
        let (arguments, slots) = binding.into_parts();
        let mut instruction_owners = Some(arguments);
        if instruction_owners
            .as_ref()
            .is_some_and(|args| args.custody() == ArgumentCustody::Frame)
        {
            drop(instruction_owners.take());
        }
        Ok(BoundPythonFrame {
            slots,
            _instruction_owners: instruction_owners,
            values: final_args,
        })
    }
}

/// Device lowering consumes the canonical bound ABI slots, never a raw argv
/// length/zip approximation. The returned tuple owns every value independently.
#[cfg(feature = "molt_gpu_primitives")]
pub(crate) unsafe fn bind_python_frame_tuple(
    py: &PyToken<'_>,
    function_bits: u64,
    positional: &[u64],
) -> u64 {
    unsafe {
        let Some(function) = obj_from_bits(function_bits).as_ptr() else {
            return raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "GPU kernel must be a Python function",
            );
        };
        if object_type_id(function) != TYPE_ID_FUNCTION {
            return raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "GPU kernel must be a Python function",
            );
        }
        let mut arguments = match CallArguments::retained(py, None, positional, &[], &[]) {
            Ok(arguments) => arguments,
            Err(error) => return error,
        };
        arguments.admit_custody(callee_custody(py, function_bits, arguments.form));
        let names = match python_argument_names(py, function) {
            Ok(Some(names)) => names,
            Ok(None) => {
                return raise_exception::<u64>(
                    py,
                    BuiltinExc::TypeError,
                    "GPU kernel lacks a Python signature",
                );
            }
            Err(error) => return error,
        };
        match bind_python_frame(py, function, arguments, names) {
            Ok(frame) => {
                let tuple = alloc_tuple(py, &frame.values);
                if tuple.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(tuple).bits()
                }
            }
            Err(error) => error,
        }
    }
}

/// Whether `func_bits` is a plain Python function that takes `supplied`
/// positional arguments over directly: its entry adopts, its borrowed-lane
/// trampoline exists, and those arguments need no binding.
pub(super) unsafe fn takes_positional_arguments_over(
    py: &PyToken<'_>,
    func_bits: u64,
    supplied: usize,
) -> bool {
    obj_from_bits(func_bits)
        .as_ptr()
        .is_some_and(|func_ptr| unsafe {
            object_type_id(func_ptr) == TYPE_ID_FUNCTION
                && function_bits_adopt_arguments(func_bits)
                && function_trampoline_ptr(func_ptr) != 0
                && function_arity_usize(func_ptr) == Some(supplied)
                && !function_raw_positional_call_needs_binding(py, func_ptr, supplied)
        })
}

/// Largest positional arity a direct call passes from a stack vector.
const DIRECT_ARGV_MAX: usize = 16;

/// Fill the trailing parameters `argv[supplied..]` of a plain adopting
/// function from its live positional defaults, each with a reference of its
/// own for the frame, and return the function's positional arity. `None`
/// when the call needs the binder: keyword-only parameters, `*args`,
/// `**kwargs`, too many arguments, or defaults that do not cover the gap.
/// Defaults are read at call time, so a reassigned `__defaults__` applies.
unsafe fn pad_positional_defaults(
    py: &PyToken<'_>,
    func_bits: u64,
    supplied: usize,
    argv: &mut [u64; DIRECT_ARGV_MAX],
) -> Option<usize> {
    unsafe {
        let func_ptr = obj_from_bits(func_bits).as_ptr()?;
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION
            || !function_bits_adopt_arguments(func_bits)
            || function_trampoline_ptr(func_ptr) == 0
            || function_requires_binder_flag(func_ptr)
        {
            return None;
        }
        let arity = function_arity_usize(func_ptr)?;
        if arity <= supplied || arity > DIRECT_ARGV_MAX {
            return None;
        }
        // The binder flag is exact, so the defaults are None or a tuple.
        let defaults = obj_from_bits(function_binding_meta(
            py,
            func_ptr,
            FunctionBindingField::Defaults,
        ))
        .as_ptr()?;
        crate::object::seq_access::with_immutable_tuple_slice(defaults, |items| {
            // Defaults align to the end of the positional parameters.
            let start = items.len().checked_sub(arity - supplied)?;
            argv[supplied..arity].copy_from_slice(&items[start..]);
            for &bits in &argv[supplied..arity] {
                inc_ref_bits(py, bits);
            }
            Some(arity)
        })?
    }
}

/// Call `func_bits`, which the caller keeps alive, with a call instruction's
/// adopted `receiver` and `args`. A plain adopting function takes them over
/// directly when they fill its positional parameters, exactly or with its
/// trailing positional defaults; anything else binds them as the
/// instruction's argument vector, which moves them into an adopting frame or
/// ends them after a borrowing callee returns.
pub(super) unsafe fn call_owned_function(
    py: &PyToken<'_>,
    func_bits: u64,
    receiver: Option<u64>,
    args: &[u64],
) -> u64 {
    unsafe {
        let supplied = usize::from(receiver.is_some()) + args.len();
        if supplied <= DIRECT_ARGV_MAX {
            let mut argv = [0u64; DIRECT_ARGV_MAX];
            let skip = usize::from(receiver.is_some());
            if let Some(receiver) = receiver {
                argv[0] = receiver;
            }
            argv[skip..supplied].copy_from_slice(args);
            if takes_positional_arguments_over(py, func_bits, supplied) {
                return call_function_obj_moved(py, func_bits, &argv[..supplied]);
            }
            if let Some(arity) = pad_positional_defaults(py, func_bits, supplied, &mut argv) {
                return call_function_obj_moved(py, func_bits, &argv[..arity]);
            }
        }
        match CallArguments::moved(py, receiver, args) {
            Ok(arguments) => call_bind_with_arguments(py, func_bits, arguments),
            Err(err) => err,
        }
    }
}

#[cfg(test)]
#[path = "frame_binding_binding_tests.rs"]
mod tests;
