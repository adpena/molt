//! Python frame admission, signature/default binding and adopting entry calls.
//!
//! State stays with its existing owner; calls preserve transaction and drop order.

use super::*;

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
                "MemoryError",
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
        .unwrap_or_else(|| MoltObject::none().bits());
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(defaults_bits).is_none() {
            return Ok(None);
        }
        let Some(defaults_ptr) = obj_from_bits(defaults_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                py,
                "TypeError",
                "call expects function object",
            ));
        };
        if object_type_id(defaults_ptr) != TYPE_ID_DICT {
            return Err(raise_exception::<_>(
                py,
                "TypeError",
                "call expects function object",
            ));
        }
        inc_ref_bits(py, defaults_bits);
        let _owner = PtrDropGuard::new(defaults_ptr);
        let value = dict_get_in_place(py, defaults_ptr, name_bits);
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        if let Some(value) = value {
            inc_ref_bits(py, value);
        }
        Ok(value)
    }
}

fn trace_function_bind_meta_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_FUNCTION_BIND_META").as_deref() == Ok("1"))
}

/// One callback-free projection of binder-relevant metadata. Ordinary calls
/// require binding for positional defaults; the fused fast path may pad those
/// defaults directly. Both consumers share all other admission facts.
pub(super) struct FunctionBindingShape {
    pub(super) full_binder: bool,
    pub(super) positional_defaults: usize,
}

pub(super) unsafe fn function_binding_meta(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
    field: FunctionBindingField,
) -> u64 {
    unsafe { crate::call::function::function_metadata_bits(py, func_ptr, field.name()) }
}

pub(super) unsafe fn function_binding_shape(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
) -> FunctionBindingShape {
    unsafe {
        let mut full_binder = builtin_args::builtin_call_binding(py, func_ptr).is_some();
        for name in [
            FunctionBindingField::BindKind,
            FunctionBindingField::Varargs,
            FunctionBindingField::VarKeywords,
        ] {
            full_binder |= !obj_from_bits(function_binding_meta(py, func_ptr, name)).is_none();
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
        if !kwdefaults.is_none() {
            full_binder |= match kwdefaults.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => dict_len(ptr) != 0,
                _ => true,
            };
        }
        let defaults = obj_from_bits(function_binding_meta(
            py,
            func_ptr,
            FunctionBindingField::Defaults,
        ));
        let positional_defaults = if defaults.is_none() {
            0
        } else {
            match defaults.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_TUPLE => {
                    crate::object::seq_access::len(ptr)
                }
                _ => {
                    full_binder = true;
                    0
                }
            }
        };
        FunctionBindingShape {
            full_binder,
            positional_defaults,
        }
    }
}

/// Positional defaults can be padded by the fused direct path; every other
/// binder requirement, including malformed defaults, comes from the same shape.
pub(super) unsafe fn function_requires_full_binding(py: &PyToken<'_>, func_ptr: *mut u8) -> bool {
    let shape = unsafe { function_binding_shape(py, func_ptr) };
    shape.full_binder || shape.positional_defaults != 0
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
        needs_binder
    }
}

pub(crate) unsafe fn function_requires_binder_flag(func_ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(func_ptr);
        ((*header).load_metadata_flags() & HEADER_FLAG_FUNC_REQUIRES_BINDER) != 0
    }
}

pub(crate) unsafe fn function_raw_positional_call_needs_binding(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    supplied: usize,
) -> bool {
    unsafe {
        if function_requires_binder_flag(func_ptr) {
            return true;
        }
        let shape = function_binding_shape(_py, func_ptr);
        if shape.full_binder {
            return true;
        }
        let Some(arity) = function_arity_usize(func_ptr) else {
            let _ = raise_exception::<u64>(
                _py,
                "OverflowError",
                "function arity exceeds the active address space",
            );
            return true;
        };
        shape.positional_defaults != 0 && supplied != arity
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
        let bind_kind_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_bind_kind,
                FunctionBindingField::BindKind.name(),
            ),
        );
        if let Some(kind_bits) = bind_kind_bits
            && obj_from_bits(kind_bits).as_int() == Some(BIND_KIND_CAPI_METHOD)
        {
            return call_capi_method_with_bound_args(_py, func_bits, &args);
        }
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
                if let Some(bound_args) =
                    builtin_args::bind_positional_builtin_call(_py, func_bits, func_ptr, &view)
                {
                    return call_function_obj_bound_vec(_py, func_bits, bound_args.as_slice());
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
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
        );
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let Some(bits) = bits else {
            return Ok(None);
        };
        let Some(pointer) = obj_from_bits(bits).as_ptr() else {
            return Err(raise_exception::<u64>(
                py,
                "TypeError",
                "call expects function object",
            ));
        };
        let Some(names) = crate::object::seq_access::pin_tuple(py, pointer) else {
            return Err(raise_exception::<u64>(
                py,
                "TypeError",
                "call expects function object",
            ));
        };
        Ok(Some(names))
    }
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
        .unwrap_or_else(|| MoltObject::none().bits());
        let kwonly_names_pin = if obj_from_bits(kwonly_bits).is_none() {
            None
        } else {
            let Some(kw_ptr) = obj_from_bits(kwonly_bits).as_ptr() else {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "call expects function object",
                ));
            };
            if object_type_id(kw_ptr) != TYPE_ID_TUPLE {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
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
                    let name =
                        string_obj_to_owned(obj_from_bits(name)).expect("validated keyword string");
                    return Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!("got multiple values for argument '{name}'"),
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
                crate::dict_set_in_place(_py, dictionary, name, value);
                if exception_pending(_py) {
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
                        "TypeError",
                        &format!(
                            "{function}() got some positional-only arguments passed as keyword arguments: '{}'",
                            conflicts.join(", "),
                        ),
                    ));
                }
                let name =
                    string_obj_to_owned(obj_from_bits(name)).expect("validated keyword string");
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    &format!("got an unexpected keyword '{name}'"),
                ));
            }
        }

        // Keyword callbacks and their errors precede positional arity and
        // default resolution, as in CPython initialize_locals.
        if supplied > total_pos && !has_vararg {
            let func_name_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(_py, &runtime_state(_py).interned.name_name, b"__name__"),
            );
            let fname = func_name_bits
                .and_then(|b| string_obj_to_owned(obj_from_bits(b)))
                .unwrap_or_else(|| "?".to_string());
            let arg_names_strs: Vec<String> = arg_names
                .iter()
                .map(|&b| {
                    string_obj_to_owned(obj_from_bits(b))
                        .unwrap_or_else(|| format!("<raw:{:x}>", b))
                })
                .collect();
            let msg = format!(
                "too many positional arguments for {}(): got {} positional, expected {} (arg_names={:?}, kwonly={}, vararg={}, varkw={})",
                fname,
                supplied,
                total_pos,
                arg_names_strs,
                kwonly_names.len(),
                has_vararg,
                has_varkw,
            );
            return Err(raise_exception::<_>(_py, "TypeError", &msg));
        }

        let defaults_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.defaults_name,
                FunctionBindingField::Defaults.name(),
            ),
        )
        .unwrap_or_else(|| MoltObject::none().bits());
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let defaults_pin = if obj_from_bits(defaults_bits).is_none() {
            None
        } else {
            let Some(def_ptr) = obj_from_bits(defaults_bits).as_ptr() else {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "call expects function object",
                ));
            };
            if object_type_id(def_ptr) != TYPE_ID_TUPLE {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
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
            if matches!(
                std::env::var("MOLT_TRACE_CALL_BIND_MISSING")
                    .ok()
                    .as_deref(),
                Some("1")
            ) {
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
            let msg = format!("missing required argument '{name}'");
            return Err(raise_exception::<_>(_py, "TypeError", &msg));
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
        if let Some(name_bits) = first_missing_kwonly {
            let name =
                string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "?".to_string());
            let msg = format!("missing required keyword-only argument '{name}'");
            return Err(raise_exception::<_>(_py, "TypeError", &msg));
        }

        let mut final_args: Vec<u64> = Vec::with_capacity(binding.slots.len());
        for slot in &binding.slots.values {
            let Some(val) = *slot else {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
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
            return raise_exception::<u64>(py, "TypeError", "GPU kernel must be a Python function");
        };
        if object_type_id(function) != TYPE_ID_FUNCTION {
            return raise_exception::<u64>(py, "TypeError", "GPU kernel must be a Python function");
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
                    "TypeError",
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

/// Call `func_bits`, which the caller keeps alive, with a call instruction's
/// adopted `receiver` and `args`. A plain adopting function of exact
/// positional arity takes them over directly; anything else binds them as the
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
        const DIRECT_ARGV_MAX: usize = 16;
        if supplied <= DIRECT_ARGV_MAX && takes_positional_arguments_over(py, func_bits, supplied) {
            let mut argv = [0u64; DIRECT_ARGV_MAX];
            let skip = usize::from(receiver.is_some());
            if let Some(receiver) = receiver {
                argv[0] = receiver;
            }
            argv[skip..supplied].copy_from_slice(args);
            return call_function_obj_moved(py, func_bits, &argv[..supplied]);
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
