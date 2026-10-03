use crate::builtins::frames::FrameInvocationGuard;
use crate::call::function::{
    ArgumentTransfer, FunctionBindingField, call_function_obj_moved, function_bits_adopt_arguments,
};
use crate::call::type_policy::{
    callable_matches_runtime_symbol, resolved_new_is_default_object_new,
};
use crate::call::{
    CallAttrLookup, StaticmethodCallTarget, lookup_call_attr, require_call_attr,
    resolve_staticmethod_call_target,
};
use crate::state::recursion::RecursionGuard;
use crate::state::tls::FRAME_STACK;
#[cfg(test)]
use crate::alloc_string;
use crate::{
    ALLOC_BYTES_CALLARGS, BIND_KIND_CAPI_METHOD, BIND_KIND_CLINIC_NAMED, BIND_KIND_TYPE_NEW_INIT,
    CALL_BIND_IC_HIT_COUNT, CALL_BIND_IC_MISS_COUNT, HEADER_FLAG_FUNC_REQUIRES_BINDER,
    INVOKE_FFI_BRIDGE_CAPABILITY_DENIED_COUNT, MoltHeader, MoltObject, PtrDropGuard, PyToken,
    TYPE_ID_BOUND_METHOD, TYPE_ID_CALLARGS, TYPE_ID_DICT, TYPE_ID_FOREIGN, TYPE_ID_FROZENSET,
    TYPE_ID_FUNCTION, TYPE_ID_GENERIC_ALIAS, TYPE_ID_SET, TYPE_ID_STRING, TYPE_ID_TUPLE,
    TYPE_ID_TYPE, alloc_dict_with_pairs, alloc_instance_for_default_object_new, alloc_object,
    alloc_tuple,
    audit::{AuditArgs, audit_capability_decision},
    bits_from_ptr, bound_method_func_bits, bound_method_self_bits, builtin_classes,
    call_class_init_with_args, call_function_obj_bound_vec, class_attr_lookup_raw_mro,
    class_layout_version_bits, class_name_bits, class_name_for_error, code_filename_bits,
    code_name_bits, dec_ref_bits, dict_fromkeys_method, dict_get_in_place, dict_get_method,
    dict_order, dict_setdefault_method, dict_update_method, dict_update_set_via_store,
    exception_pending, function_arity, function_arity_usize, function_attr_bits,
    function_execution_closure_bits, function_fn_ptr, function_name_bits, function_trampoline_ptr,
    generic_alias_origin_bits, has_capability, header_from_obj_ptr, inc_ref_bits,
    intern_static_name, is_builtin_class_bits, is_trusted, is_truthy, issubclass_bits,
    maybe_ptr_from_bits, missing_bits, molt_bytearray_count_slice, molt_bytearray_decode,
    molt_bytearray_endswith_slice, molt_bytearray_find_slice, molt_bytearray_hex,
    molt_bytearray_index_slice, molt_bytearray_pop, molt_bytearray_rfind_slice,
    molt_bytearray_rindex_slice, molt_bytearray_rsplit_max, molt_bytearray_split_max,
    molt_bytearray_splitlines, molt_bytearray_startswith_slice, molt_bytes_count_slice,
    molt_bytes_decode, molt_bytes_endswith_slice, molt_bytes_find_slice, molt_bytes_hex,
    molt_bytes_index_slice, molt_bytes_maketrans, molt_bytes_rfind_slice, molt_bytes_rindex_slice,
    molt_bytes_rsplit_max, molt_bytes_split_max, molt_bytes_splitlines,
    molt_bytes_startswith_slice, molt_dict_pop_method, molt_file_reconfigure,
    molt_frozenset_copy_method, molt_frozenset_difference_multi, molt_frozenset_intersection_multi,
    molt_frozenset_isdisjoint, molt_frozenset_issubset, molt_frozenset_issuperset,
    molt_frozenset_symmetric_difference, molt_frozenset_union_multi, molt_int_from_bytes,
    molt_int_to_bytes, molt_list_append, molt_list_index_range, molt_list_pop, molt_list_sort,
    molt_memoryview_cast, molt_memoryview_hex, molt_object_init, molt_object_init_subclass,
    molt_object_new_bound, molt_set_clear, molt_set_copy_method,
    molt_set_difference_multi, molt_set_difference_update_multi, molt_set_intersection_multi,
    molt_set_intersection_update_multi, molt_set_isdisjoint, molt_set_issubset,
    molt_set_issuperset, molt_set_symmetric_difference, molt_set_symmetric_difference_update,
    molt_set_union_multi, molt_set_update_multi, molt_string_count_slice, molt_string_encode,
    molt_string_endswith_slice, molt_string_find_slice, molt_string_format_method,
    molt_string_index_slice, molt_string_rfind_slice, molt_string_rindex_slice,
    molt_string_rsplit_max, molt_string_split_max, molt_string_splitlines,
    molt_string_startswith_slice, molt_tuple_index_range, molt_type_call, molt_type_init,
    molt_type_new, obj_from_bits, object_class_bits, object_type_id, profile_hit_unchecked,
    ptr_from_bits, raise_exception, raise_not_callable, runtime_state, runtime_state_for_gil,
    string_obj_to_owned, type_name, type_of_bits,
};
use std::collections::{HashMap, HashSet};
use std::sync::{MutexGuard, OnceLock};

mod builtin_args;
#[cfg(test)]
#[path = "bind/class_constructor_tests.rs"]
mod class_constructor_tests;
mod inline_cache;
use inline_cache::{call_bind_ic_entry_for_call, try_call_bind_ic_fast};
pub(crate) use inline_cache::{
    clear_call_bind_ic_cache, clear_method_ic_cache, clear_super_ic_cache,
    detach_callable_ic_caches,
};

#[cfg(test)]
pub(crate) fn call_bind_ic_site_cached_for_test(site_id: u64) -> bool {
    inline_cache::ic_tls_lookup(site_id).is_some()
}
#[allow(unused_imports)]
pub use inline_cache::{
    molt_call_bind_ic, molt_call_bind_ic_owned, molt_call_indirect_ic, molt_call_method_ic_owned,
    molt_call_method_ic0, molt_call_method_ic1, molt_call_method_ic2, molt_call_method_ic3,
    molt_call_method_ic4, molt_call_super_method_ic_owned, molt_call_super_method_ic0,
    molt_call_super_method_ic1, molt_call_super_method_ic2, molt_call_super_method_ic3,
    molt_call_super_method_ic4, molt_invoke_ffi_ic,
};
/// CPython's two call instructions, fixed by the compiler at each call site.
/// A `Stack` call (CALL) passes value-stack entries that an inlined Python
/// frame takes over. An `Expanded` call (CALL_FUNCTION_EX) passes a positional
/// tuple and a keyword mapping, which own the arguments until frame admission
/// ends and lend their contents to the frame. Runtime values cannot recover
/// the form; the frontend's call-argument schedule records it on
/// `callargs_new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CallForm {
    Stack,
    Expanded,
}

pub(crate) struct CallArgs {
    pos: Vec<u64>,
    keywords: u64,
    form: CallForm,
}

impl CallArgs {
    /// Read the live dictionary, never a projection cached during expansion.
    unsafe fn keyword_count(&self) -> usize {
        obj_from_bits(self.keywords)
            .as_ptr()
            .map_or(0, |dict| unsafe { dict_order(dict).len() / 2 })
    }
}

/// True when the caller's reference is the only one: no other owner, borrowed
/// C view or frozen-layout authority can observe a mutation of `ptr`. Moving
/// edges out of an object needs this proof; without it a call retains copies.
unsafe fn exclusively_owned(ptr: *mut u8) -> bool {
    const SHARED_AUTHORITY: u32 = crate::object::HEADER_FLAG_HAS_ABI_VIEW
        | crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP
        | crate::object::HEADER_FLAG_IMMORTAL;
    let header = unsafe { &*header_from_obj_ptr(ptr) };
    header.is_uniquely_owned() && header.load_synchronized_flags() & SHARED_AUTHORITY == 0
}

/// Who owns a call's arguments while its callee runs. CPython decides this at
/// the call instruction from the callee: a plain Python function, and under
/// CALL a bound method of one, gets an inlined frame that takes the arguments
/// over. Any other callee borrows them, and the instruction releases them
/// after it returns, even when that callee binds a Python frame of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgumentCustody {
    Frame,
    Instruction,
}

/// The CPython path that releases the arguments a call still owns. The
/// `*ByTarget` paths changed in CPython 3.14 and consult the runtime target
/// version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseOrder {
    /// `initialize_locals` and `_PyEvalFramePushAndInit` failures: positional
    /// values, then keyword values, each first to last.
    Forward,
    /// CALL's cleanup after a callee without an inlined frame (DECREF_INPUTS),
    /// and an inlined frame's parameters (`_PyFrame_ClearLocals`): first to
    /// last through 3.13, last to first from 3.14.
    StackByTarget,
    /// `_PyEvalFramePushAndInit_Ex`: the positional tuple (last to first), then
    /// the keyword mapping (insertion order), in every version.
    TupleThenMapping,
    /// CALL_FUNCTION_EX's cleanup after a callee without an inlined frame: the
    /// tuple first through 3.13, the mapping first from 3.14.
    ContainersByTarget,
}

impl ReleaseOrder {
    /// The call instruction's own cleanup, before any frame takes custody.
    fn instruction(form: CallForm) -> Self {
        match form {
            CallForm::Stack => Self::StackByTarget,
            CallForm::Expanded => Self::ContainersByTarget,
        }
    }
}

/// How frame binding admits a call's arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Admission {
    /// A CALL's inlined frame: values move into their slots, surplus
    /// positional values end at once, and a failure releases the unbound
    /// keyword values before the partial frame (`initialize_locals`).
    Move,
    /// Every other binding: slots take new references and the call keeps its
    /// own until admission ends (CALL_FUNCTION_EX into an inlined frame) or
    /// until the callee returns (a callee without an inlined frame).
    Copy,
}

/// The consuming call's arguments: CPython's value-stack operands of a CALL,
/// or the positional tuple and keyword mapping of a CALL_FUNCTION_EX. The
/// consuming entry moves the heap builder's edges here (T1). A CALL's inlined
/// frame takes them over (T2); every other callee borrows them. Whatever
/// remains is released by the CPython path that owns it at that point.
struct CallArguments<'a, 'py> {
    py: &'a PyToken<'py>,
    form: CallForm,
    /// Decided once, from the call instruction's callee; redispatch keeps it.
    custody: Option<ArgumentCustody>,
    release: ReleaseOrder,
    positional: Vec<u64>,
    /// Positional values before this index have moved into a frame.
    positional_start: usize,
    keywords: CallKeywords,
}

/// Keyword custody. The builder's dictionary stays whole until a consumer
/// needs ordered entries, so extension callees still receive it directly.
enum CallKeywords {
    /// One owned reference to the keyword dictionary, or `None`.
    Mapping(u64),
    Unpacked(KeywordArguments),
}

/// Owned keyword entries in insertion order.
struct KeywordArguments {
    names: Vec<u64>,
    values: Vec<u64>,
    /// Values before this index have moved into a frame or its `**kwargs`.
    start: usize,
}

/// How a call's keyword arguments end.
#[derive(Clone, Copy)]
enum KeywordRelease {
    /// Value-stack entries, first to last.
    InOrder,
    /// Value-stack entries, last to first.
    Reversed,
    /// A mapping's teardown: each entry's name, then its value, in insertion
    /// order.
    Mapping,
}

impl CallKeywords {
    /// Keyword values this call still owns.
    fn owned_count(&self) -> usize {
        match self {
            CallKeywords::Mapping(bits) => obj_from_bits(*bits)
                .as_ptr()
                .map_or(0, |dict| unsafe { dict_order(dict).len() / 2 }),
            CallKeywords::Unpacked(keywords) => keywords.values.len() - keywords.start,
        }
    }

    fn release(self, py: &PyToken<'_>, order: KeywordRelease) {
        match self {
            CallKeywords::Mapping(bits) => {
                // Stack entries end one by one. Only a mapping this call alone
                // owns can be taken apart; a shared one ends with its owners.
                let reversed = matches!(order, KeywordRelease::Reversed)
                    .then(|| {
                        let dict = obj_from_bits(bits).as_ptr()?;
                        unsafe { exclusively_owned(dict) }.then(|| unsafe {
                            crate::object::ops::dict_clear_deferred(py, dict)
                                .expect("an exclusively owned call dictionary is mutable")
                                .into_owned_bits()
                        })
                    })
                    .flatten();
                dec_ref_bits(py, bits);
                if let Some(entries) = reversed {
                    for pair in entries.chunks_exact(2).rev() {
                        dec_ref_bits(py, pair[1]);
                        dec_ref_bits(py, pair[0]);
                    }
                }
            }
            CallKeywords::Unpacked(KeywordArguments {
                names,
                values,
                start,
            }) => match order {
                KeywordRelease::InOrder => {
                    release_in_order(py, &values[start..]);
                    release_in_order(py, &names);
                }
                KeywordRelease::Reversed => {
                    release_reversed(py, &values[start..]);
                    release_in_order(py, &names);
                }
                KeywordRelease::Mapping => {
                    for (index, &name) in names.iter().enumerate() {
                        dec_ref_bits(py, name);
                        if index >= start {
                            dec_ref_bits(py, values[index]);
                        }
                    }
                }
            },
        }
    }
}

fn release_in_order(py: &PyToken<'_>, values: &[u64]) {
    for &bits in values {
        dec_ref_bits(py, bits);
    }
}

fn release_reversed(py: &PyToken<'_>, values: &[u64]) {
    for &bits in values.iter().rev() {
        dec_ref_bits(py, bits);
    }
}

/// A borrowed projection of unpacked `CallArguments` for builtin, extension
/// and constructor binders. It owns nothing.
#[derive(Clone, Copy)]
struct CallArgumentView<'a> {
    pos: &'a [u64],
    kw_names: &'a [u64],
    kw_values: &'a [u64],
}

impl<'a, 'py> CallArguments<'a, 'py> {
    fn empty(py: &'a PyToken<'py>, form: CallForm) -> Self {
        Self {
            py,
            form,
            custody: None,
            release: ReleaseOrder::instruction(form),
            positional: Vec::new(),
            positional_start: 0,
            keywords: CallKeywords::Mapping(MoltObject::none().bits()),
        }
    }

    /// T1. The consuming call holds the builder's reference and inherits its
    /// call form. When that reference is the only one, the builder's edges move
    /// here without reference traffic and the builder is left empty. A builder
    /// that another owner can still reach keeps its edges, and the call
    /// retains its own.
    unsafe fn from_builder(py: &'a PyToken<'py>, builder_ptr: *mut u8) -> Result<Self, u64> {
        if builder_ptr.is_null() {
            return Ok(Self::empty(py, CallForm::Stack));
        }
        let builder = unsafe { &mut *require_callargs_ptr(py, builder_ptr)? };
        let mut arguments = Self::empty(py, builder.form);
        if unsafe { exclusively_owned(builder_ptr) } {
            arguments.positional = std::mem::take(&mut builder.pos);
            arguments.keywords = CallKeywords::Mapping(std::mem::replace(
                &mut builder.keywords,
                MoltObject::none().bits(),
            ));
            return Ok(arguments);
        }
        if arguments
            .positional
            .try_reserve_exact(builder.pos.len())
            .is_err()
        {
            return Err(raise_exception::<_>(
                py,
                "MemoryError",
                "call arguments allocation failed",
            ));
        }
        ALLOC_BYTES_CALLARGS.fetch_add(
            (arguments.positional.capacity() * std::mem::size_of::<u64>()) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        for &bits in &builder.pos {
            inc_ref_bits(py, bits);
            arguments.positional.push(bits);
        }
        inc_ref_bits(py, builder.keywords);
        arguments.keywords = CallKeywords::Mapping(builder.keywords);
        Ok(arguments)
    }

    /// An argument vector retaining borrowed values, for runtime callers and
    /// for the constructor phases that `type.__call__` lends its arguments
    /// to. `receiver` becomes the first positional value. The lender keeps
    /// its own references past this call.
    fn retained(
        py: &'a PyToken<'py>,
        receiver: Option<u64>,
        positional: &[u64],
        names: &[u64],
        values: &[u64],
    ) -> Result<Self, u64> {
        debug_assert_eq!(names.len(), values.len());
        let mut arguments = Self::empty(py, CallForm::Stack);
        let mut keywords = KeywordArguments {
            names: Vec::new(),
            values: Vec::new(),
            start: 0,
        };
        let receivers = usize::from(receiver.is_some());
        if arguments
            .positional
            .try_reserve_exact(receivers + positional.len())
            .is_err()
            || keywords.names.try_reserve_exact(names.len()).is_err()
            || keywords.values.try_reserve_exact(values.len()).is_err()
        {
            return Err(raise_exception::<_>(
                py,
                "MemoryError",
                "call arguments allocation failed",
            ));
        }
        ALLOC_BYTES_CALLARGS.fetch_add(
            ((arguments.positional.capacity()
                + keywords.names.capacity()
                + keywords.values.capacity())
                * std::mem::size_of::<u64>()) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        for &bits in receiver.iter().chain(positional) {
            inc_ref_bits(py, bits);
            arguments.positional.push(bits);
        }
        for (&name, &value) in names.iter().zip(values) {
            inc_ref_bits(py, name);
            inc_ref_bits(py, value);
            keywords.names.push(name);
            keywords.values.push(value);
        }
        arguments.keywords = CallKeywords::Unpacked(keywords);
        Ok(arguments)
    }

    /// A call instruction's adopted positional operands, `receiver` first when
    /// the instruction adopted one. The instruction's references move here
    /// without reference traffic; if the vector cannot be allocated they end
    /// as the instruction's inputs (`release_stack_arguments`).
    fn moved(py: &'a PyToken<'py>, receiver: Option<u64>, positional: &[u64]) -> Result<Self, u64> {
        let mut arguments = Self::empty(py, CallForm::Stack);
        let receivers = usize::from(receiver.is_some());
        if arguments
            .positional
            .try_reserve_exact(receivers + positional.len())
            .is_err()
        {
            release_stack_arguments(py, receiver, positional);
            return Err(raise_exception::<_>(
                py,
                "MemoryError",
                "call arguments allocation failed",
            ));
        }
        ALLOC_BYTES_CALLARGS.fetch_add(
            (arguments.positional.capacity() * std::mem::size_of::<u64>()) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        arguments.positional.extend(receiver);
        arguments.positional.extend_from_slice(positional);
        Ok(arguments)
    }

    /// Record the call instruction's custody decision. Redispatch through a
    /// descriptor, `__call__`, a class or a generic alias never revisits it.
    fn admit_custody(&mut self, custody: ArgumentCustody) {
        if self.custody.is_some() {
            return;
        }
        self.custody = Some(custody);
        self.release = match (custody, self.form) {
            (ArgumentCustody::Frame, CallForm::Stack) => ReleaseOrder::Forward,
            (ArgumentCustody::Frame, CallForm::Expanded) => ReleaseOrder::TupleThenMapping,
            (ArgumentCustody::Instruction, form) => ReleaseOrder::instruction(form),
        };
    }

    fn custody(&self) -> ArgumentCustody {
        self.custody.unwrap_or(ArgumentCustody::Instruction)
    }

    fn admission(&self) -> Admission {
        if self.custody() == ArgumentCustody::Frame && self.form == CallForm::Stack {
            Admission::Move
        } else {
            Admission::Copy
        }
    }

    /// An inlined frame borrows the positional vector as its exact parameters
    /// (trampoline and cached direct calls). Those references are the frame's
    /// and end in frame order after it returns.
    fn enter_inlined_frame(&mut self) {
        if self.custody() == ArgumentCustody::Frame {
            self.release = ReleaseOrder::StackByTarget;
        }
    }

    fn positional(&self) -> &[u64] {
        &self.positional[self.positional_start..]
    }

    /// An adopting entry takes every remaining positional value over as its
    /// parameters; this call releases none of them afterwards.
    fn surrender_positional(&mut self) -> &[u64] {
        let start = std::mem::replace(&mut self.positional_start, self.positional.len());
        &self.positional[start..]
    }

    /// Keyword entries this call still owns.
    fn keyword_count(&self) -> usize {
        self.keywords.owned_count()
    }

    /// Bound-method dispatch owns its receiver as the first positional value.
    fn prepend_positional(&mut self, bits: u64) -> Result<(), u64> {
        debug_assert_eq!(
            self.positional_start, 0,
            "receivers bind before any transfer"
        );
        if self.positional.try_reserve(1).is_err() {
            return Err(raise_exception::<_>(
                self.py,
                "MemoryError",
                "call arguments allocation failed",
            ));
        }
        inc_ref_bits(self.py, bits);
        self.positional.insert(0, bits);
        Ok(())
    }

    /// Keyword names are strings (subclasses included) at every call boundary.
    fn validate_keywords(&self) -> bool {
        match &self.keywords {
            CallKeywords::Mapping(bits) => {
                obj_from_bits(*bits).as_ptr().is_none_or(|dict| unsafe {
                    crate::object::mapping_merge::validate_keywords(self.py, dict)
                })
            }
            CallKeywords::Unpacked(keywords) => {
                crate::object::mapping_merge::validate_keyword_names(
                    self.py,
                    keywords.names.iter().copied(),
                )
            }
        }
    }

    /// An owned keyword mapping for extension callees: the builder's own
    /// dictionary while it is whole, otherwise a fresh dictionary of the
    /// entries this call still owns. `None` when there are no keywords.
    fn keyword_mapping(&self) -> Result<u64, u64> {
        let keywords = match &self.keywords {
            CallKeywords::Mapping(bits) => {
                inc_ref_bits(self.py, *bits);
                return Ok(*bits);
            }
            CallKeywords::Unpacked(keywords) => keywords,
        };
        if keywords.start == keywords.values.len() {
            return Ok(MoltObject::none().bits());
        }
        let mut pairs = Vec::new();
        if pairs
            .try_reserve_exact(2 * (keywords.values.len() - keywords.start))
            .is_err()
        {
            return Err(raise_exception::<_>(
                self.py,
                "MemoryError",
                "call keywords allocation failed",
            ));
        }
        for index in keywords.start..keywords.values.len() {
            pairs.push(keywords.names[index]);
            pairs.push(keywords.values[index]);
        }
        let dict = alloc_dict_with_pairs(self.py, &pairs);
        if dict.is_null() {
            // The dictionary constructor raised: MemoryError, or a keyword
            // name's hash or equality callback. That exception is the result.
            return Err(MoltObject::none().bits());
        }
        Ok(MoltObject::from_ptr(dict).bits())
    }

    /// Keyword admission, then a view of the arguments. Entries leave the
    /// builder's dictionary without reference traffic when this call holds
    /// its only reference. A dictionary another owner can reach stays intact
    /// and the call retains its entries, as `_PyStack_UnpackDict` does. Either
    /// way no later keyword callback can change what binding reads.
    unsafe fn unpacked_view(&mut self) -> Result<CallArgumentView<'_>, u64> {
        if let CallKeywords::Mapping(bits) = self.keywords {
            let mut keywords = KeywordArguments {
                names: Vec::new(),
                values: Vec::new(),
                start: 0,
            };
            if let Some(dict) = obj_from_bits(bits).as_ptr() {
                let count = unsafe { dict_order(dict).len() } / 2;
                if keywords.names.try_reserve_exact(count).is_err()
                    || keywords.values.try_reserve_exact(count).is_err()
                {
                    return Err(raise_exception::<_>(
                        self.py,
                        "MemoryError",
                        "call keywords allocation failed",
                    ));
                }
                ALLOC_BYTES_CALLARGS.fetch_add(
                    ((keywords.names.capacity() + keywords.values.capacity())
                        * std::mem::size_of::<u64>()) as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                if unsafe { exclusively_owned(dict) } {
                    // Publishing the dictionary empty is unobservable: no
                    // other owner can reach it.
                    let detached =
                        unsafe { crate::object::ops::dict_clear_deferred(self.py, dict) };
                    let entries = detached
                        .expect("an exclusively owned call dictionary is mutable")
                        .into_owned_bits();
                    for pair in entries.chunks_exact(2) {
                        keywords.names.push(pair[0]);
                        keywords.values.push(pair[1]);
                    }
                } else {
                    // No Python callback or dictionary mutation occurs here.
                    for pair in unsafe { dict_order(dict) }.chunks_exact(2) {
                        inc_ref_bits(self.py, pair[0]);
                        inc_ref_bits(self.py, pair[1]);
                        keywords.names.push(pair[0]);
                        keywords.values.push(pair[1]);
                    }
                }
            }
            // Publish the entries before the dictionary edge can be released.
            self.keywords = CallKeywords::Unpacked(keywords);
            dec_ref_bits(self.py, bits);
        }
        let CallKeywords::Unpacked(keywords) = &self.keywords else {
            unreachable!("keyword entries were just unpacked");
        };
        Ok(CallArgumentView {
            pos: &self.positional[self.positional_start..],
            kw_names: &keywords.names[keywords.start..],
            kw_values: &keywords.values[keywords.start..],
        })
    }

    /// T2 under `Admission::Move`: the next positional value moves into its
    /// frame slot.
    fn take_positional(&mut self) -> u64 {
        let bits = self.positional[self.positional_start];
        self.positional_start += 1;
        bits
    }

    /// T2 under `Admission::Move`: the remaining positional values become the
    /// frame's `*args` tuple. Ownership transfers only when the tuple exists.
    fn take_positional_tuple(&mut self) -> Option<u64> {
        let tuple = crate::object::builders::alloc_tuple_owned(self.py, self.positional());
        if tuple.is_null() {
            return None;
        }
        self.positional_start = self.positional.len();
        Some(MoltObject::from_ptr(tuple).bits())
    }

    /// `*args` under `Admission::Copy`: a tuple of new references to the
    /// positional values from `from` on. This call keeps its own.
    fn copy_positional_tuple(&self, from: usize) -> Option<u64> {
        let tuple = alloc_tuple(self.py, &self.positional()[from..]);
        (!tuple.is_null()).then(|| MoltObject::from_ptr(tuple).bits())
    }

    /// Under `Admission::Move`, `initialize_locals` releases surplus positional
    /// values as soon as the binder finds them surplus. The arity error follows
    /// keyword binding, whose callbacks observe the release.
    fn release_surplus_positional(&mut self) {
        while self.positional_start < self.positional.len() {
            let bits = self.positional[self.positional_start];
            self.positional_start += 1;
            dec_ref_bits(self.py, bits);
        }
    }

    /// Number of unpacked keyword entries, bound or not.
    fn keyword_len(&self) -> usize {
        match &self.keywords {
            CallKeywords::Unpacked(keywords) => keywords.values.len(),
            CallKeywords::Mapping(bits) => {
                assert!(
                    obj_from_bits(*bits).is_none(),
                    "binding reads unpacked keywords"
                );
                0
            }
        }
    }

    /// Keyword entry `index` of the unpacked call, borrowed.
    fn keyword_entry(&self, index: usize) -> (u64, u64) {
        let CallKeywords::Unpacked(keywords) = &self.keywords else {
            unreachable!("binding reads unpacked keywords");
        };
        (keywords.names[index], keywords.values[index])
    }

    /// T2 under `Admission::Move`: keyword `index` moves out of this call.
    /// Entries move in order, so the unbound remainder stays a suffix.
    fn take_keyword(&mut self, index: usize) -> u64 {
        let CallKeywords::Unpacked(keywords) = &mut self.keywords else {
            unreachable!("binding reads unpacked keywords");
        };
        debug_assert_eq!(index, keywords.start, "keywords move in order");
        let bits = keywords.values[keywords.start];
        keywords.start += 1;
        bits
    }

    /// Every keyword name of the call, bound or not.
    fn keyword_names(&self) -> &[u64] {
        match &self.keywords {
            CallKeywords::Unpacked(keywords) => &keywords.names,
            CallKeywords::Mapping(_) => &[],
        }
    }
}

impl Drop for CallArguments<'_, '_> {
    fn drop(&mut self) {
        let py = self.py;
        let positional = std::mem::take(&mut self.positional);
        let positional = &positional[self.positional_start..];
        let keywords = std::mem::replace(
            &mut self.keywords,
            CallKeywords::Mapping(MoltObject::none().bits()),
        );
        // Only two or more owners have an observable order; the target version
        // is consulted only then.
        let owned = positional.len() + keywords.owned_count();
        let from_3_14 = || owned > 1 && crate::object::ops_sys::runtime_target_at_least(py, 3, 14);
        match self.release {
            ReleaseOrder::StackByTarget if from_3_14() => {
                keywords.release(py, KeywordRelease::Reversed);
                release_reversed(py, positional);
            }
            ReleaseOrder::Forward | ReleaseOrder::StackByTarget => {
                release_in_order(py, positional);
                keywords.release(py, KeywordRelease::InOrder);
            }
            ReleaseOrder::ContainersByTarget if from_3_14() => {
                keywords.release(py, KeywordRelease::Mapping);
                release_reversed(py, positional);
            }
            ReleaseOrder::TupleThenMapping | ReleaseOrder::ContainersByTarget => {
                release_reversed(py, positional);
                keywords.release(py, KeywordRelease::Mapping);
            }
        }
    }
}

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

pub(crate) unsafe fn dispatch_init_subclass_hooks(
    _py: &PyToken<'_>,
    class_bits: u64,
    kw_names: &[u64],
    kw_values: &[u64],
) -> bool {
    unsafe {
        // Own keyword values before descriptor resolution can run user code.
        // The class keywords arrive as one validated mapping's entries.
        let arguments = match CallArguments::retained(_py, None, &[], kw_names, kw_values) {
            Ok(arguments) => arguments,
            Err(_) => return false,
        };
        let init_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.init_subclass_name,
            b"__init_subclass__",
        );
        if exception_pending(_py) {
            return false;
        }
        // The constructor has proved both arguments are the same live type.
        // Use the existing super/descriptor authority, not a second MRO walk:
        // one inherited hook owns cooperative dispatch to the remaining bases.
        let super_ptr =
            crate::object::builders::alloc_super_obj(_py, class_bits, class_bits, class_bits);
        if super_ptr.is_null() {
            return false;
        }
        let _super_owner = PtrDropGuard::new(super_ptr);
        let init_bits =
            crate::molt_get_attr_name(MoltObject::from_ptr(super_ptr).bits(), init_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, init_bits);
            return false;
        }
        // Descriptor binding already supplied the new class receiver.
        let result = call_bind_with_arguments(_py, init_bits, arguments);
        crate::call::discard_owned_call_result(_py, result);
        dec_ref_bits(_py, init_bits);
        !exception_pending(_py)
    }
}

fn trace_callargs_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_CALLARGS").as_deref() == Ok("1"))
}

fn trace_function_bind_meta_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_FUNCTION_BIND_META").as_deref() == Ok("1"))
}

fn trace_call_type_builder_enabled_raw(raw: Option<&str>) -> bool {
    raw == Some("1")
}

fn trace_call_type_builder_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        trace_call_type_builder_enabled_raw(
            std::env::var("MOLT_TRACE_CALL_TYPE_BUILDER")
                .ok()
                .as_deref(),
        )
    })
}

/// Cached trace mode for `molt_call_bind`.  The env var is read once;
/// subsequent calls use the cached result — eliminates a
/// `std::env::var` syscall on every function call.
#[derive(Copy, Clone)]
enum TraceCallBindMode {
    Off,
    Basic,
    Verbose,
}

fn trace_call_bind_mode() -> TraceCallBindMode {
    static MODE: OnceLock<TraceCallBindMode> = OnceLock::new();
    *MODE.get_or_init(
        || match std::env::var("MOLT_TRACE_CALL_BIND").ok().as_deref() {
            Some("all" | "verbose") => TraceCallBindMode::Verbose,
            Some("1") => TraceCallBindMode::Basic,
            _ => TraceCallBindMode::Off,
        },
    )
}

#[derive(Copy, Clone)]
struct CallArgsPtr(*mut CallArgs);

// CallArgs allocations are owned by the runtime object they are attached to
// and protected by the GIL-like runtime lock. The registry only preserves
// pointer provenance for lookups from object payload addresses.
unsafe impl Send for CallArgsPtr {}
unsafe impl Sync for CallArgsPtr {}

pub(crate) struct CallBindRuntimeState {
    callargs_builder_map: HashMap<usize, CallArgsPtr>,
    callargs_storage_registry: HashSet<usize>,
}

impl CallBindRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            callargs_builder_map: HashMap::new(),
            callargs_storage_registry: HashSet::new(),
        }
    }
}

fn call_bind_runtime_state(_py: &PyToken<'_>) -> MutexGuard<'static, CallBindRuntimeState> {
    runtime_state(_py)
        .call_bind
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn call_bind_runtime_state_if_available() -> Option<MutexGuard<'static, CallBindRuntimeState>> {
    runtime_state_for_gil().map(|state| {
        state
            .call_bind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    })
}

pub(crate) fn note_callargs_alloc(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
    args_ptr: *mut CallArgs,
) {
    let mut state = call_bind_runtime_state(_py);
    if !builder_ptr.is_null() {
        state
            .callargs_builder_map
            .insert(builder_ptr as usize, CallArgsPtr(args_ptr));
    }
    if args_ptr.is_null() {
        return;
    }
    state.callargs_storage_registry.insert(args_ptr as usize);
}

pub(crate) fn note_callargs_free(_py: &PyToken<'_>, builder_ptr: *mut u8, args_ptr: *mut CallArgs) {
    if trace_callargs_enabled() && !builder_ptr.is_null() {
        eprintln!(
            "[molt callargs] free builder_ptr=0x{:x} args_ptr=0x{:x}",
            builder_ptr as usize, args_ptr as usize,
        );
    }
    let mut state = call_bind_runtime_state(_py);
    if !builder_ptr.is_null() {
        state.callargs_builder_map.remove(&(builder_ptr as usize));
    }
    if args_ptr.is_null() {
        return;
    }
    state.callargs_storage_registry.remove(&(args_ptr as usize));
}

#[cfg(any(test, feature = "molt_gpu_primitives"))]
pub(crate) unsafe fn clone_callargs_builder_bits(
    _py: &PyToken<'_>,
    builder_bits: u64,
) -> Result<u64, u64> {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "invalid callargs builder",
        ));
    }
    if unsafe { object_type_id(builder_ptr) } != TYPE_ID_CALLARGS {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "invalid callargs builder",
        ));
    }
    let args_ptr = unsafe { require_callargs_ptr(_py, builder_ptr) }?;
    let args = unsafe { &*args_ptr };
    let clone_bits = callargs_new_with_form(
        _py,
        MoltObject::from_int(args.pos.len() as i64).bits(),
        MoltObject::from_int(0).bits(),
        args.form,
    );
    if clone_bits == 0 || obj_from_bits(clone_bits).is_none() {
        return Err(clone_bits);
    }
    for &value_bits in &args.pos {
        let pushed = unsafe { molt_callargs_push_pos(clone_bits, value_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, clone_bits);
            return Err(pushed);
        }
    }
    if !obj_from_bits(args.keywords).is_none() {
        let keywords = crate::molt_dict_copy(args.keywords);
        if exception_pending(_py) || obj_from_bits(keywords).is_none() {
            dec_ref_bits(_py, clone_bits);
            return Err(if exception_pending(_py) {
                keywords
            } else {
                raise_exception::<_>(_py, "MemoryError", "call keywords allocation failed")
            });
        }
        unsafe { (*callargs_ptr(ptr_from_bits(clone_bits))).keywords = keywords };
    }
    Ok(clone_bits)
}

#[allow(dead_code)]
pub(crate) unsafe fn callargs_positional_snapshot(
    _py: &PyToken<'_>,
    builder_bits: u64,
) -> Result<Vec<u64>, u64> {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "invalid callargs builder",
        ));
    }
    let args_ptr = unsafe { require_callargs_ptr(_py, builder_ptr) }?;
    let args = unsafe { &*args_ptr };
    if unsafe { args.keyword_count() } != 0 {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "gpu kernel launch does not support keyword arguments",
        ));
    }
    Ok(args.pos.clone())
}

fn callargs_builder_is_live(_py: &PyToken<'_>, builder_ptr: *mut u8) -> bool {
    if builder_ptr.is_null() {
        return false;
    }
    call_bind_runtime_state(_py)
        .callargs_builder_map
        .contains_key(&(builder_ptr as usize))
}

fn callargs_storage_is_live(_py: &PyToken<'_>, args_ptr: *mut CallArgs) -> bool {
    if args_ptr.is_null() {
        return false;
    }
    call_bind_runtime_state(_py)
        .callargs_storage_registry
        .contains(&(args_ptr as usize))
}

unsafe fn is_default_type_call(_py: &PyToken<'_>, call_bits: u64) -> bool {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return false;
        };
        match object_type_id(call_ptr) {
            TYPE_ID_BOUND_METHOD => {
                let func_bits = bound_method_func_bits(call_ptr);
                is_default_type_call(_py, func_bits)
            }
            TYPE_ID_FUNCTION => crate::builtins::functions::runtime_callable_represents_symbol(
                function_fn_ptr(call_ptr),
                function_trampoline_ptr(call_ptr),
                fn_addr!(molt_type_call),
            ),
            _ => false,
        }
    }
}

/// Class construction lends the call's argument vector to `__new__` and
/// `__init__`, as `type.__call__` does: each phase retains its own vector.
/// A class never inlines a frame, so the call instruction releases the
/// construction arguments once construction ends, in its own order.
unsafe fn call_type_with_arguments(
    _py: &PyToken<'_>,
    call_ptr: *mut u8,
    mut arguments: CallArguments<'_, '_>,
) -> u64 {
    unsafe {
        let class_bits = MoltObject::from_ptr(call_ptr).bits();
        let builtins = builtin_classes(_py);
        let args = match arguments.unpacked_view() {
            Ok(args) => args,
            Err(err) => return err,
        };
        let pos_args = args.pos;
        let kw_names = args.kw_names;
        let kw_values = args.kw_values;
        if class_bits == builtins.type_obj && pos_args.len() == 3 {
            return build_class_from_args(
                _py,
                class_bits,
                pos_args[0],
                pos_args[1],
                pos_args[2],
                kw_names,
                kw_values,
            );
        }
        // Custom metaclass (subclass of type) with 3 args:
        // Meta(name, bases, namespace).  CPython's `type.__call__` dispatches
        // to `Meta.__new__(Meta, name, bases, namespace, **kwds)` and then
        // `Meta.__init__(cls, name, bases, namespace, **kwds)`.  Honor user
        // overrides of either method.
        if pos_args.len() == 3 && issubclass_bits(class_bits, builtins.type_obj) {
            // Build the kwargs dict once; reused for the fast path
            // (`molt_type_new`) and to dec-ref at exit.
            let kwargs_bits = if kw_names.is_empty() {
                MoltObject::none().bits()
            } else {
                let mut pairs = Vec::with_capacity(kw_names.len() * 2);
                for (k, v) in kw_names.iter().zip(kw_values.iter()) {
                    pairs.push(*k);
                    pairs.push(*v);
                }
                let ptr = alloc_dict_with_pairs(_py, &pairs);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(ptr).bits()
            };

            // Look up `__new__` on the metaclass.  If the user did not
            // override it, the lookup resolves to the inherited
            // `type.__new__` (intrinsic `molt_type_new`); use the fast
            // path that also runs `__init_subclass__` and class slot
            // setup inline.  Otherwise dispatch to the user's override.
            let new_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
            let new_lookup = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits);
            let new_is_default = new_lookup
                .map(|bits| {
                    let obj = obj_from_bits(bits);
                    let Some(p) = obj.as_ptr() else { return true };
                    if object_type_id(p) != TYPE_ID_FUNCTION {
                        return false;
                    }
                    function_fn_ptr(p) == fn_addr!(molt_type_new)
                })
                .unwrap_or(true);

            // `class_attr_lookup_raw_mro` returns borrowed bits.  Match
            // the OLD code path's lifetime contract: never dec-ref the
            // looked-up function bits.
            let new_class_bits = if new_is_default {
                molt_type_new(
                    class_bits,
                    pos_args[0],
                    pos_args[1],
                    pos_args[2],
                    kwargs_bits,
                )
            } else {
                let new_bits = new_lookup.expect("non-default __new__ must resolve");
                // `type.__call__` lends its arguments to each constructor
                // phase; the phase retains its own argument vector.
                match CallArguments::retained(_py, Some(class_bits), pos_args, kw_names, kw_values)
                {
                    Ok(new_arguments) => call_bind_with_arguments(_py, new_bits, new_arguments),
                    Err(err) => {
                        if !kw_names.is_empty() {
                            dec_ref_bits(_py, kwargs_bits);
                        }
                        return err;
                    }
                }
            };

            if exception_pending(_py) {
                if !kw_names.is_empty() {
                    dec_ref_bits(_py, kwargs_bits);
                }
                return MoltObject::none().bits();
            }

            // CPython: only invoke `__init__` when `__new__` returned an
            // instance of `cls` (here, of the metaclass).  This matches
            // `type.__call__` semantics.
            let new_class_obj = obj_from_bits(new_class_bits);
            let returned_instance = if let Some(p) = new_class_obj.as_ptr() {
                let inst_class_bits = object_class_bits(p);
                inst_class_bits != 0 && issubclass_bits(inst_class_bits, class_bits)
            } else {
                false
            };

            if returned_instance {
                // Call Meta.__init__(new_class, name, bases, namespace, **kwds).
                // `class_attr_lookup_raw_mro` returns borrowed bits — do
                // not dec-ref.
                let init_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
                if let Some(init_bits) = class_attr_lookup_raw_mro(_py, call_ptr, init_name_bits) {
                    let init_result = match CallArguments::retained(
                        _py,
                        Some(new_class_bits),
                        pos_args,
                        kw_names,
                        kw_values,
                    ) {
                        Ok(init_arguments) => {
                            call_bind_with_arguments(_py, init_bits, init_arguments)
                        }
                        Err(err) => err,
                    };
                    // A failed allocation or `__init__` leaves the error pending;
                    // consuming the result reports both the same way.
                    if !crate::call::class_init::consume_init_result(_py, init_result) {
                        dec_ref_bits(_py, new_class_bits);
                        if !kw_names.is_empty() {
                            dec_ref_bits(_py, kwargs_bits);
                        }
                        return MoltObject::none().bits();
                    }
                }
            }

            if !kw_names.is_empty() {
                dec_ref_bits(_py, kwargs_bits);
            }
            return new_class_bits;
        }
        if class_bits == builtins.type_obj && pos_args.len() == 1 && kw_names.is_empty() {
            let bits = type_of_bits(_py, pos_args[0]);
            inc_ref_bits(_py, bits);
            return bits;
        }
        let abstract_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.abstractmethods_name,
            b"__abstractmethods__",
        );
        if let Some(abstract_bits) = class_attr_lookup_raw_mro(_py, call_ptr, abstract_name_bits)
            && !obj_from_bits(abstract_bits).is_none()
            && is_truthy(_py, obj_from_bits(abstract_bits))
        {
            let class_name = class_name_for_error(class_bits);
            let msg = format!("Can't instantiate abstract class {class_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if is_builtin_class_bits(_py, class_bits)
            && crate::object::class_is_immutable(_py, call_ptr)
            && class_bits != builtins.module
            && !crate::builtins::types::native_constructors::owns_constructor_descriptors(
                _py, class_bits,
            )
        {
            if let Some(result) = crate::builtins::types::wrappers::try_construct_exact_wrapper(
                _py, class_bits, pos_args, kw_names, kw_values,
            ) {
                return result;
            }

            if class_bits == builtins.super_type {
                return crate::builtins::types::descriptor_objects::super_call(
                    _py,
                    pos_args,
                    !kw_names.is_empty(),
                );
            }

            if class_bits == builtins.enumerate {
                if pos_args.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "enumerate() missing required argument 'iterable' (pos 1)",
                    );
                }
                if pos_args.len() > 2 {
                    let msg = format!(
                        "enumerate expected at most 2 arguments, got {}",
                        pos_args.len()
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let iterable_bits = pos_args[0];
                let mut start_opt = if pos_args.len() == 2 {
                    Some(pos_args[1])
                } else {
                    None
                };
                for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
                    let name = string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<name>".to_string());
                    if name != "start" {
                        let msg =
                            format!("enumerate() got an unexpected keyword argument '{name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if start_opt.is_some() {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "enumerate() got multiple values for argument 'start'",
                        );
                    }
                    start_opt = Some(val_bits);
                }
                return crate::object::ops::enumerate_new_impl(_py, iterable_bits, start_opt);
            }

            if class_bits == builtins.bool {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "bool() takes no keyword arguments",
                    );
                }
                if pos_args.len() > 1 {
                    let msg = format!("bool expected at most 1 argument, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if pos_args.is_empty() {
                    return MoltObject::from_bool(false).bits();
                }
                let result = is_truthy(_py, obj_from_bits(pos_args[0]));
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_bool(result).bits();
            }

            if class_bits == builtins.reversed {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "reversed() takes no keyword arguments",
                    );
                }
                if pos_args.len() != 1 {
                    let msg = format!("reversed expected 1 argument, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                return crate::object::ops::reversed_new_impl(_py, pos_args[0]);
            }

            if class_bits == builtins.map {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "map() takes no keyword arguments",
                    );
                }
                if pos_args.len() < 2 {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "map() must have at least two arguments",
                    );
                }
                return crate::object::ops::map_new_impl(_py, pos_args[0], &pos_args[1..]);
            }

            if class_bits == builtins.filter {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "filter() takes no keyword arguments",
                    );
                }
                if pos_args.len() != 2 {
                    let msg = format!("filter expected 2 arguments, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                return crate::object::ops::filter_new_impl(_py, pos_args[0], pos_args[1]);
            }

            if class_bits == builtins.zip {
                let mut strict = false;
                for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
                    let name = string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<name>".to_string());
                    if name != "strict" {
                        let msg = format!("zip() got an unexpected keyword argument '{name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    strict = is_truthy(_py, obj_from_bits(val_bits));
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                return crate::object::ops::zip_new_impl(_py, pos_args, strict);
            }

            if class_bits == builtins.text_io_wrapper && !kw_names.is_empty() {
                if let Some(bound_args) =
                    builtin_args::bind_builtin_class_text_io_wrapper(_py, &args)
                {
                    return call_class_init_with_args(_py, call_ptr, &bound_args);
                }
                return MoltObject::none().bits();
            }
            if class_bits == builtins.string_io && !kw_names.is_empty() {
                if let Some(bound_args) = builtin_args::bind_builtin_class_string_io(_py, &args) {
                    return call_class_init_with_args(_py, call_ptr, &bound_args);
                }
                return MoltObject::none().bits();
            }
            if !kw_names.is_empty() {
                let class_name = class_name_for_error(class_bits);
                let msg = format!("{class_name}() takes no keyword arguments");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            return call_class_init_with_args(_py, call_ptr, pos_args);
        }
        let is_exc_subclass = issubclass_bits(class_bits, builtins.base_exception);
        if trace_call_type_builder_enabled() {
            let class_name = class_name_for_error(class_bits);
            eprintln!(
                "[DEBUG] call_type_with_arguments: class={} bits={:#x} is_exc_subclass={}",
                class_name, class_bits, is_exc_subclass
            );
        }
        if is_exc_subclass {
            return crate::call::class_init::construct_exception_from_args(
                _py, call_ptr, pos_args, kw_names, kw_values,
            );
        }
        crate::call::class_init::construct_regular_class(
            _py, call_ptr, pos_args, kw_names, kw_values,
        )
    }
}

unsafe fn build_class_from_args(
    _py: &PyToken<'_>,
    metaclass_bits: u64,
    name_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    kw_names: &[u64],
    kw_values: &[u64],
) -> u64 {
    unsafe {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        };
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        }

        let mut bases_vec: Vec<u64> = Vec::new();
        let mut bases_tuple_bits = bases_bits;
        let mut bases_owned = false;
        if obj_from_bits(bases_bits).is_none() || bases_bits == 0 {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            bases_owned = true;
        } else if let Some(bases_ptr) = obj_from_bits(bases_bits).as_ptr() {
            match object_type_id(bases_ptr) {
                TYPE_ID_TUPLE => {
                    bases_vec =
                        crate::object::seq_access::with_borrowed(bases_ptr, |bases| bases.to_vec());
                }
                TYPE_ID_TYPE => {
                    let tuple_ptr = alloc_tuple(_py, &[bases_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                    bases_owned = true;
                    bases_vec.push(bases_bits);
                }
                _ => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "bases must be a tuple of types",
                    );
                }
            }
        }

        if bases_vec.is_empty() {
            let builtins = builtin_classes(_py);
            let tuple_ptr = alloc_tuple(_py, &[builtins.object]);
            if tuple_ptr.is_null() {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            bases_owned = true;
            bases_vec.push(builtins.object);
        }

        let mut winner_bits = metaclass_bits;
        for base_bits in bases_vec.iter().copied() {
            let base_meta_bits = type_of_bits(_py, base_bits);
            if issubclass_bits(winner_bits, base_meta_bits) {
                continue;
            }
            if issubclass_bits(base_meta_bits, winner_bits) {
                winner_bits = base_meta_bits;
                continue;
            }
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the metaclasses of all its bases",
            );
        }

        if winner_bits != metaclass_bits {
            // The winning metaclass receives its own retained argument vector;
            // this adapter keeps its borrowed operands.
            let class_bits = match CallArguments::retained(
                _py,
                None,
                &[name_bits, bases_tuple_bits, namespace_bits],
                kw_names,
                kw_values,
            ) {
                Ok(arguments) => call_bind_with_arguments(_py, winner_bits, arguments),
                Err(err) => err,
            };
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return class_bits;
        }

        // Metaclass selection is the adapter's only construction policy.
        // The canonical type constructor owns namespace copying, metadata cells,
        // unpublished-class cleanup, slots, and the ordered callback phases.
        let kwargs_bits = if kw_names.is_empty() {
            MoltObject::none().bits()
        } else {
            let pairs: Vec<u64> = kw_names
                .iter()
                .zip(kw_values.iter())
                .flat_map(|(&name, &value)| [name, value])
                .collect();
            let kwargs = alloc_dict_with_pairs(_py, &pairs);
            if kwargs.is_null() {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(kwargs).bits()
        };
        let result = molt_type_new(
            metaclass_bits,
            name_bits,
            bases_tuple_bits,
            namespace_bits,
            kwargs_bits,
        );
        if !kw_names.is_empty() {
            dec_ref_bits(_py, kwargs_bits);
        }
        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        result
    }
}

pub(crate) unsafe fn callargs_ptr(ptr: *mut u8) -> *mut CallArgs {
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    let Some(state) = call_bind_runtime_state_if_available() else {
        return std::ptr::null_mut();
    };
    state
        .callargs_builder_map
        .get(&(ptr as usize))
        .copied()
        .map_or(std::ptr::null_mut(), |raw| raw.0)
}

unsafe fn require_callargs_ptr(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
) -> Result<*mut CallArgs, u64> {
    unsafe {
        if builder_ptr.is_null() {
            return Ok(std::ptr::null_mut());
        }
        if !callargs_builder_is_live(_py, builder_ptr) {
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] invalid_builder builder_ptr=0x{:x}",
                    builder_ptr as usize,
                );
            }
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "invalid callargs builder",
            ));
        }
        let args_ptr = callargs_ptr(builder_ptr);
        if args_ptr.is_null() || !callargs_storage_is_live(_py, args_ptr) {
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] invalid_storage builder_ptr=0x{:x} args_ptr=0x{:x}",
                    builder_ptr as usize, args_ptr as usize,
                );
            }
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "invalid callargs storage",
            ));
        }
        Ok(args_ptr)
    }
}

pub(crate) unsafe fn callargs_visit_owned(args_ptr: *mut CallArgs, mut visit: impl FnMut(u64)) {
    unsafe {
        if args_ptr.is_null() {
            return;
        }
        let args = &*args_ptr;
        for &bits in &args.pos {
            visit(bits);
        }
        visit(args.keywords);
    }
}

pub(crate) unsafe fn callargs_detach_owned(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
    args_ptr: *mut CallArgs,
    mut detach: impl FnMut(u64),
) {
    unsafe {
        if args_ptr.is_null() {
            return;
        }
        // Registry visibility is retired before any edge can be released by
        // the caller's object-wide sink.
        note_callargs_free(_py, builder_ptr, args_ptr);
        let args = &mut *args_ptr;
        // Consuming calls take the builder's edges at entry, so a builder that
        // still holds arguments is the value stack of a failed preparation. It
        // unwinds as CPython's does: the keyword mapping sits above the
        // positional entries, which release last to first.
        detach(std::mem::replace(
            &mut args.keywords,
            MoltObject::none().bits(),
        ));
        for bits in std::mem::take(&mut args.pos).into_iter().rev() {
            detach(bits);
        }
        drop(Box::from_raw(args_ptr));
    }
}

/// C-API methods take `(args, kwargs)` containers and may retain either. The
/// call's argument vector keeps its own references until the call ends.
unsafe fn call_capi_method_with_bound_args(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &CallArguments<'_, '_>,
) -> u64 {
    unsafe {
        let keywords = match args.keyword_mapping() {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let tuple_ptr = alloc_tuple(_py, args.positional());
        if tuple_ptr.is_null() {
            dec_ref_bits(_py, keywords);
            return MoltObject::none().bits();
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let result = call_function_obj_bound_vec(_py, func_bits, &[tuple_bits, keywords]);
        dec_ref_bits(_py, tuple_bits);
        dec_ref_bits(_py, keywords);
        result
    }
}

/// The builder of a CALL call site: value-stack arguments.
#[unsafe(no_mangle)]
pub extern "C" fn molt_callargs_new(pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        callargs_new_with_form(_py, pos_capacity_bits, kw_capacity_bits, CallForm::Stack)
    })
}

/// The builder of a CALL_FUNCTION_EX call site: its storage is that call's
/// positional tuple and keyword mapping.
#[unsafe(no_mangle)]
pub extern "C" fn molt_callargs_new_expanded(pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        callargs_new_with_form(_py, pos_capacity_bits, kw_capacity_bits, CallForm::Expanded)
    })
}

fn callargs_new_with_form(
    _py: &PyToken<'_>,
    pos_capacity_bits: u64,
    kw_capacity_bits: u64,
    form: CallForm,
) -> u64 {
    if exception_pending(_py) {
        return 0;
    }
    let decode_capacity = |bits: u64| -> Option<usize> {
        let obj = MoltObject::from_bits(bits);
        if let Some(value) = obj.as_int() {
            return usize::try_from(value).ok();
        }
        if let Some(value) = obj.as_bool() {
            return Some(usize::from(value));
        }
        if obj.is_ptr() || obj.is_none() || obj.is_pending() {
            return None;
        }
        crate::provenance::abi::address(bits)
    };
    let (Some(pos_capacity), Some(kw_capacity)) = (
        decode_capacity(pos_capacity_bits),
        decode_capacity(kw_capacity_bits),
    ) else {
        raise_exception::<()>(_py, "TypeError", "callargs capacity expects an integer");
        return 0;
    };
    let mut args = Box::new(CallArgs {
        pos: Vec::new(),
        keywords: MoltObject::none().bits(),
        form,
    });
    if args.pos.try_reserve_exact(pos_capacity).is_err() {
        raise_exception::<()>(_py, "MemoryError", "call arguments allocation failed");
        return 0;
    }
    if kw_capacity != 0 {
        let dict =
            crate::object::builders::alloc_dict_with_capacity_and_pairs(_py, kw_capacity, &[]);
        if dict.is_null() {
            // The dictionary constructor already raised; keep its exception.
            return 0;
        }
        args.keywords = MoltObject::from_ptr(dict).bits();
    }
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut CallArgs>();
    let ptr = alloc_object(_py, total, TYPE_ID_CALLARGS);
    if ptr.is_null() {
        dec_ref_bits(_py, args.keywords);
        return 0;
    }
    let callargs_bytes =
        std::mem::size_of::<CallArgs>() + args.pos.capacity() * std::mem::size_of::<u64>();
    ALLOC_BYTES_CALLARGS.fetch_add(callargs_bytes as u64, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        let args_ptr = Box::into_raw(args);
        note_callargs_alloc(_py, ptr, args_ptr);
        *(ptr as *mut *mut CallArgs) = args_ptr;
        if trace_callargs_enabled() {
            eprintln!(
                "[molt callargs] new builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} pos_cap={} kw_cap={} form={:?}",
                bits_from_ptr(ptr),
                ptr as usize,
                args_ptr as usize,
                pos_capacity,
                kw_capacity,
                form,
            );
        }
    }
    bits_from_ptr(ptr)
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new` and
/// remain owned by the caller for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_push_pos(builder_bits: u64, val: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(_py, builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos_builder builder_bits=0x{:x} builder_ptr=0x{:x} live=true",
                    builder_bits, builder_ptr as usize,
                );
            }
            let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => ptr,
                Err(err) => return err,
            };
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos_raw builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} val_type={} val_bits=0x{:x}",
                    builder_bits,
                    builder_ptr as usize,
                    args_ptr as usize,
                    type_name(_py, obj_from_bits(val)),
                    val,
                );
            }
            let args = &mut *args_ptr;
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} len_before={} val_type={} val_bits=0x{:x}",
                    builder_bits,
                    builder_ptr as usize,
                    args_ptr as usize,
                    args.pos.len(),
                    type_name(_py, obj_from_bits(val)),
                    val,
                );
            }
            // CallArgs must keep arguments alive even if the caller drops its temporaries before
            // `molt_call_bind` executes.
            if args.pos.try_reserve(1).is_err() {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "call arguments allocation failed",
                );
            }
            inc_ref_bits(_py, val);
            args.pos.push(val);
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos_done builder_bits=0x{:x} len_after={}",
                    builder_bits,
                    args.pos.len(),
                );
            }
            MoltObject::none().bits()
        })
    }
}

/// Keywords have one owned dictionary authority throughout expansion.
unsafe fn callargs_keyword_dict(_py: &PyToken<'_>, args_ptr: *mut CallArgs) -> Option<*mut u8> {
    unsafe {
        if obj_from_bits((*args_ptr).keywords).is_none() {
            let ptr = alloc_dict_with_pairs(_py, &[]);
            if ptr.is_null() {
                return None;
            }
            (*args_ptr).keywords = MoltObject::from_ptr(ptr).bits();
        }
        obj_from_bits((*args_ptr).keywords).as_ptr()
    }
}

unsafe fn callargs_push_kw(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
            Ok(ptr) => ptr,
            Err(err) => return err,
        };
        let Some(dict) = callargs_keyword_dict(_py, args_ptr) else {
            return MoltObject::none().bits();
        };
        if !crate::object::mapping_merge::keyword_available(_py, dict, name_bits, None)
            || !crate::object::mapping_merge::insert_dict(_py, dict, name_bits, val_bits, None)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    }
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
/// `name_bits` must reference a Molt string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_push_kw(
    builder_bits: u64,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(_py, builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            callargs_push_kw(_py, builder_ptr, name_bits, val_bits)
        })
    }
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_expand_star(builder_bits: u64, iterable_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(_py, builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] expand_star_builder builder_bits=0x{:x} builder_ptr=0x{:x} live=true",
                    builder_bits, builder_ptr as usize,
                );
            }
            let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => ptr,
                Err(err) => return err,
            };
            if trace_callargs_enabled() {
                let len = if args_ptr.is_null() {
                    0
                } else {
                    (&*args_ptr).pos.len()
                };
                eprintln!(
                    "[molt callargs] expand_star builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} len_before={} iterable_type={} iterable_bits=0x{:x}",
                    builder_bits,
                    builder_ptr as usize,
                    args_ptr as usize,
                    len,
                    type_name(_py, obj_from_bits(iterable_bits)),
                    iterable_bits,
                );
            }
            let Some(mut iter) = crate::object::iterable::OwnedIterator::new(_py, iterable_bits)
            else {
                return MoltObject::none().bits();
            };
            let Some(hint) = crate::object::iterable::length_hint(_py, iterable_bits) else {
                return MoltObject::none().bits();
            };
            if (*args_ptr).pos.try_reserve(hint).is_err() {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "call arguments allocation failed",
                );
            }
            loop {
                match iter.next() {
                    Ok(Some(item)) => {
                        if (*args_ptr).pos.try_reserve(1).is_err() {
                            dec_ref_bits(_py, item);
                            return raise_exception::<_>(
                                _py,
                                "MemoryError",
                                "call arguments allocation failed",
                            );
                        }
                        (*args_ptr).pos.push(item);
                    }
                    Ok(None) => break,
                    Err(()) => return MoltObject::none().bits(),
                }
            }
            MoltObject::none().bits()
        })
    }
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_expand_kwstar(builder_bits: u64, mapping_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(_py, builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            let args = match require_callargs_ptr(_py, builder_ptr) {
                Ok(args) => args,
                Err(err) => return err,
            };
            let Some(dict) = callargs_keyword_dict(_py, args) else {
                return MoltObject::none().bits();
            };
            crate::object::mapping_merge::merge_keywords(_py, dict, mapping_bits);
            MoltObject::none().bits()
        })
    }
}

/// One callback-free projection of binder-relevant metadata. Ordinary calls
/// require binding for positional defaults; the fused fast path may pad those
/// defaults directly. Both consumers share all other admission facts.
struct FunctionBindingShape {
    full_binder: bool,
    positional_defaults: usize,
}

unsafe fn function_binding_meta(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
    field: FunctionBindingField,
) -> u64 {
    unsafe { crate::call::function::function_metadata_bits(py, func_ptr, field.name()) }
}

unsafe fn function_binding_shape(py: &PyToken<'_>, func_ptr: *mut u8) -> FunctionBindingShape {
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
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => !dict_order(ptr).is_empty(),
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
unsafe fn function_requires_full_binding(py: &PyToken<'_>, func_ptr: *mut u8) -> bool {
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

/// Call `call_bits` with operands its runtime caller keeps. The call retains
/// its own argument vector, which binding moves into the callee frame, so the
/// caller's references stay the last owners of its operands. `receiver` is
/// prepended, as `type.__call__` lends its arguments to `__new__` and
/// `__init__`. No heap builder or builder registry entry is involved.
pub(crate) unsafe fn call_bind_borrowed(
    _py: &PyToken<'_>,
    call_bits: u64,
    receiver: Option<u64>,
    positional: &[u64],
    kw_names: &[u64],
    kw_values: &[u64],
) -> u64 {
    unsafe {
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        match CallArguments::retained(_py, receiver, positional, kw_names, kw_values) {
            Ok(arguments) => call_bind_with_arguments(_py, call_bits, arguments),
            Err(err) => err,
        }
    }
}

/// Route a call on a `TYPE_ID_FOREIGN` wrapper through the wrapped C object's
/// own `tp_call`. Materializes the call's positional arguments into a Molt
/// tuple and passes its keyword mapping, then hands them to the ABI bridge
/// (which builds a C-layout args tuple the callee can read). The C callee may
/// retain either container; the call's argument vector releases its own
/// references after the call. Returns the call result as an owned Molt handle,
/// or the error sentinel with an exception set.
///
/// # Safety
/// `call_ptr` must be a live `TYPE_ID_FOREIGN` object.
unsafe fn call_foreign_with_arguments(
    _py: &PyToken<'_>,
    call_ptr: *mut u8,
    args: &CallArguments<'_, '_>,
) -> u64 {
    let c_ptr = unsafe { crate::object::foreign::foreign_ptr_from_obj(call_ptr) };
    let tuple_ptr = crate::alloc_tuple(_py, args.positional());
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let args_bits = MoltObject::from_ptr(tuple_ptr).bits();
    let kwargs_bits = match args.keyword_mapping() {
        Ok(bits) if obj_from_bits(bits).is_none() => 0,
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, args_bits);
            return err;
        }
    };
    let result =
        unsafe { molt_cpython_abi::bridge::molt_foreign_call(c_ptr, args_bits, kwargs_bits) };
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        if args_bits != 0 {
            dec_ref_bits(_py, args_bits);
        }
        if kwargs_bits != 0 {
            dec_ref_bits(_py, kwargs_bits);
        }
    });
    match result.decode() {
        molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => bits,
        molt_cpython_abi::hooks::DecodedHandleResult::Missing
        | molt_cpython_abi::hooks::DecodedHandleResult::Error => {
            crate::cpython_abi_hooks::propagate_native_failure(_py, "foreign object call");
            MoltObject::none().bits()
        }
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is a live CallArgs builder whose reference
/// this call consumes.
pub extern "C" fn molt_call_bind(call_bits: u64, builder_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let builder_ptr = ptr_from_bits(builder_bits);
            let builder_guard = PtrDropGuard::new(builder_ptr);
            // A pending error means argument preparation failed: the builder
            // is still that call's value stack and releases as one.
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let args = match CallArguments::from_builder(_py, builder_ptr) {
                Ok(args) => args,
                Err(err) => return err,
            };
            // T1 is complete; the builder owns no argument edge any longer.
            drop(builder_guard);
            call_bind_with_arguments(_py, call_bits, args)
        }
    })
}

/// The call instruction's custody decision for `call_bits`. CPython inlines a
/// frame only for a plain Python function (`PyFunction_Type`), and CALL also
/// expands a bound method of one. The native-function class family
/// (`builtin_function_or_method`, C-API and extension callables) and every
/// other callable borrow the arguments instead.
unsafe fn callee_custody(py: &PyToken<'_>, call_bits: u64, form: CallForm) -> ArgumentCustody {
    let python_function = |bits: u64| {
        obj_from_bits(bits).as_ptr().is_some_and(|ptr| unsafe {
            object_type_id(ptr) == TYPE_ID_FUNCTION
                && !builtin_classes(py).is_native_callable_class(object_class_bits(ptr))
        })
    };
    let inlined =
        obj_from_bits(call_bits)
            .as_ptr()
            .is_some_and(|ptr| match unsafe { object_type_id(ptr) } {
                TYPE_ID_FUNCTION => python_function(call_bits),
                TYPE_ID_BOUND_METHOD => {
                    form == CallForm::Stack
                        && python_function(unsafe { bound_method_func_bits(ptr) })
                }
                _ => false,
            });
    if inlined {
        ArgumentCustody::Frame
    } else {
        ArgumentCustody::Instruction
    }
}

/// Dispatch a call that owns its argument vector. Custody is decided from the
/// original callee; callee resolution and every redispatch pass the same owner
/// onward, and nothing returns to a builder.
unsafe fn call_bind_with_arguments(
    _py: &PyToken<'_>,
    call_bits: u64,
    mut args: CallArguments<'_, '_>,
) -> u64 {
    unsafe {
        // User code never starts under an unhandled error (a constructor's
        // `isinstance` or truth callback can leave one); the argument vector
        // then releases as an ended call.
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        args.admit_custody(callee_custody(_py, call_bits, args.form));
        let call_obj = obj_from_bits(call_bits);
        let cached_mode = trace_call_bind_mode();
        let trace = !matches!(cached_mode, TraceCallBindMode::Off);
        let trace_verbose = matches!(cached_mode, TraceCallBindMode::Verbose);
        if trace_verbose {
            let callee_type = type_name(_py, call_obj);
            let first_pos_type = args
                .positional()
                .first()
                .map(|&bits| type_name(_py, obj_from_bits(bits)))
                .unwrap_or_else(|| std::borrow::Cow::Borrowed("<none>"));
            eprintln!(
                "molt call_bind enter callee_bits=0x{call_bits:x} callee_type={} pos_len={} kw_len={} first_pos_type={}",
                callee_type,
                args.positional().len(),
                args.keyword_count(),
                first_pos_type
            );
        }
        let Some(call_ptr) = call_obj.as_ptr() else {
            if trace {
                if let Some(frame) = FRAME_STACK.with(|stack| stack.borrow().last().copied())
                    && let Some(code_ptr) = maybe_ptr_from_bits(frame.code_bits)
                {
                    let (name_bits, file_bits) =
                        (code_name_bits(code_ptr), code_filename_bits(code_ptr));
                    let name = string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<code>".to_string());
                    let file = string_obj_to_owned(obj_from_bits(file_bits))
                        .unwrap_or_else(|| "<file>".to_string());
                    eprintln!(
                        "molt call_bind frame name={} file={} line={}",
                        name, file, frame.line
                    );
                }
                let none_flag = call_obj.is_none();
                let bool_flag = call_obj.as_bool();
                let int_flag = call_obj.as_int();
                let float_flag = call_obj.as_float();
                eprintln!(
                    "molt call_bind callee bits=0x{call_bits:x} none={} bool={:?} int={:?} float={:?}",
                    none_flag, bool_flag, int_flag, float_flag,
                );
                let bt = std::backtrace::Backtrace::force_capture();
                eprintln!("molt call_bind: not ptr bits=0x{call_bits:x}\n{bt}",);
                let positional = args.positional();
                eprintln!(
                    "molt call_bind args pos_len={} kw_len={} first_pos={:?} second_pos={:?}",
                    positional.len(),
                    args.keyword_count(),
                    positional.first(),
                    positional.get(1),
                );
                if let Some(&bits) = positional.first() {
                    eprintln!(
                        "molt call_bind args first_pos_bits=0x{bits:x} first_pos_type={}",
                        type_name(_py, obj_from_bits(bits)),
                    );
                    if let Some(s) = string_obj_to_owned(obj_from_bits(bits)) {
                        eprintln!("molt call_bind args first_pos_str={}", s);
                    }
                }
                if let Some(&bits) = positional.get(1)
                    && let Some(s) = string_obj_to_owned(obj_from_bits(bits))
                {
                    eprintln!("molt call_bind args second_pos_str={}", s);
                }
            }
            return raise_not_callable(_py, call_obj);
        };
        match resolve_staticmethod_call_target(_py, call_bits) {
            StaticmethodCallTarget::Owned(target) => {
                return call_bind_with_arguments(_py, target.bits(), args);
            }
            StaticmethodCallTarget::Raised => return MoltObject::none().bits(),
            StaticmethodCallTarget::NotStaticmethod => {}
        }
        let mut func_bits = call_bits;
        let mut self_bits = None;
        if matches!(
            object_type_id(call_ptr),
            TYPE_ID_FUNCTION | TYPE_ID_BOUND_METHOD | TYPE_ID_TYPE | TYPE_ID_FOREIGN
        ) && !args.validate_keywords()
        {
            return MoltObject::none().bits();
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => {}
            TYPE_ID_BOUND_METHOD => {
                func_bits = bound_method_func_bits(call_ptr);
                self_bits = Some(bound_method_self_bits(call_ptr));
            }
            TYPE_ID_TYPE => {
                match lookup_call_attr(_py, call_ptr) {
                    CallAttrLookup::Found(call_attr_bits) => {
                        if !is_default_type_call(_py, call_attr_bits) {
                            let result = call_bind_with_arguments(_py, call_attr_bits, args);
                            dec_ref_bits(_py, call_attr_bits);
                            return result;
                        }
                        dec_ref_bits(_py, call_attr_bits);
                    }
                    CallAttrLookup::Raised => return MoltObject::none().bits(),
                    CallAttrLookup::Missing => {}
                }
                return call_type_with_arguments(_py, call_ptr, args);
            }
            TYPE_ID_GENERIC_ALIAS => {
                let origin_bits = generic_alias_origin_bits(call_ptr);
                return call_bind_with_arguments(_py, origin_bits, args);
            }
            TYPE_ID_FOREIGN => {
                // Foreign (C-extension) callable: route through the wrapped
                // object's own `tp_call` via the ABI bridge. The foreign call
                // borrows the argument vector, which releases after it returns.
                return call_foreign_with_arguments(_py, call_ptr, &args);
            }
            _ => {
                let call_attr_bits = match require_call_attr(_py, call_ptr, call_obj) {
                    Ok(bits) => bits,
                    Err(result) => return result,
                };
                if let Some(entry) = call_bind_ic_entry_for_call(_py, call_attr_bits)
                    && let Some(res) = try_call_bind_ic_fast(_py, entry, call_attr_bits, &mut args)
                {
                    dec_ref_bits(_py, call_attr_bits);
                    return res;
                }
                let result = call_bind_with_arguments(_py, call_attr_bits, args);
                dec_ref_bits(_py, call_attr_bits);
                return result;
            }
        }
        if let Some(bound_self_bits) = self_bits {
            let target_obj = obj_from_bits(func_bits);
            let target_ptr = target_obj.as_ptr();
            if target_ptr.is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_FUNCTION) {
                if let Err(err) = args.prepend_positional(bound_self_bits) {
                    return err;
                }
                return call_bind_with_arguments(_py, func_bits, args);
            }
        }
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        if callable_matches_runtime_symbol(Some(func_bits), fn_addr!(molt_type_call)) {
            let Some(self_bits) = self_bits else {
                return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
            };
            let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
            };
            if object_type_id(self_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
            }
            return call_type_with_arguments(_py, self_ptr, args);
        }
        if let Some(self_bits) = self_bits {
            // The argument vector owns the receiver like any other positional.
            if let Err(err) = args.prepend_positional(self_bits) {
                return err;
            }
        }
        call_function_with_arguments(_py, func_bits, func_ptr, args)
    }
}

/// A function object either borrows the argument vector (exact-arity
/// trampolines and the builtin, extension and positional-builtin binders) or
/// binds a Python frame (T2) as the call's custody allows (`Admission`).
unsafe fn call_function_with_arguments(
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
        if let Some(result) = crate::cpython_abi_hooks::try_call_cext(
            _py,
            func_ptr,
            view.pos,
            view.kw_names,
            view.kw_values,
        ) {
            return result;
        }
        if let Some(binding) = builtin_args::builtin_call_binding(_py, func_ptr) {
            return binding.call(_py, func_bits, func_ptr, &view);
        }

        let arg_names_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_arg_names,
                FunctionBindingField::ArgumentNames.name(),
            ),
        );
        let arg_names = if let Some(bits) = arg_names_bits {
            let arg_names_ptr = obj_from_bits(bits).as_ptr();
            let Some(arg_names_ptr) = arg_names_ptr else {
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            };
            if object_type_id(arg_names_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            }
            // Pin immutable metadata without allocating. The guard keeps
            // this exact tuple alive if another thread replaces the
            // function attribute while a future gilless binder is active.
            crate::object::seq_access::pin_tuple(_py, arg_names_ptr)
                .expect("type-checked argument-name tuple must be pinnable")
        } else {
            if let Some(bound_args) =
                builtin_args::bind_positional_builtin_call(_py, func_bits, func_ptr, &view)
            {
                return call_function_obj_bound_vec(_py, func_bits, bound_args.as_slice());
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };

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
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            };
            if object_type_id(kw_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
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
        let slots = match BoundCallSlots::new(_py, layout) {
            Ok(slots) => slots,
            Err(error) => return error,
        };
        // T2: an inlined CALL frame takes the call's arguments over; any other
        // binding gives the frame its own references (`Admission`).
        let mut binding = FrameBinding::new(args, slots);
        let admission = binding.admission;
        // Match initialize_locals: own **kwargs, positional slots, and
        // *args before rich keyword matching can call Python.
        let varkw_ptr = if has_varkw {
            let dictionary = alloc_dict_with_pairs(_py, &[]);
            if dictionary.is_null() {
                return MoltObject::none().bits();
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
                        return MoltObject::none().bits();
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
                        return MoltObject::none().bits();
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
                        _ => return MoltObject::none().bits(),
                    }
                }
            }
            if let Some(slot) = matched {
                if binding.slots[slot].is_some() {
                    let name =
                        string_obj_to_owned(obj_from_bits(name)).expect("validated keyword string");
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!("got multiple values for argument '{name}'"),
                    );
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
                    return MoltObject::none().bits();
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
                            _ => return MoltObject::none().bits(),
                        }
                    }
                }
                if !conflicts.is_empty() {
                    let function = function_name_bits(_py, func_ptr);
                    let function = string_obj_to_owned(obj_from_bits(function))
                        .unwrap_or_else(|| "function".to_string());
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!(
                            "{function}() got some positional-only arguments passed as keyword arguments: '{}'",
                            conflicts.join(", "),
                        ),
                    );
                }
                let name =
                    string_obj_to_owned(obj_from_bits(name)).expect("validated keyword string");
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    &format!("got an unexpected keyword '{name}'"),
                );
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
            return raise_exception::<_>(_py, "TypeError", &msg);
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
            return MoltObject::none().bits();
        }
        let defaults_pin = if obj_from_bits(defaults_bits).is_none() {
            None
        } else {
            let Some(def_ptr) = obj_from_bits(defaults_bits).as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            };
            if object_type_id(def_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
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
            return raise_exception::<_>(_py, "TypeError", &msg);
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
            let default = match function_kwdefault_owned(_py, func_ptr, name_bits) {
                Ok(value) => value,
                Err(error) => return error,
            };
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
            return raise_exception::<_>(_py, "TypeError", &msg);
        }

        let mut final_args: Vec<u64> = Vec::with_capacity(binding.slots.len());
        for slot in &binding.slots.values {
            let Some(val) = *slot else {
                return raise_exception::<_>(_py, "TypeError", "call binding failed");
            };
            final_args.push(val);
        }
        let (arguments, mut slots) = binding.into_parts();
        let inlined_frame = arguments.custody() == ArgumentCustody::Frame;
        // The inlined frame owns its parameters before the callee runs: a
        // CALL's vector keeps only keyword names, and a CALL_FUNCTION_EX's
        // tuple and mapping end here, as `_PyEvalFramePushAndInit_Ex` releases
        // them. A callee without an inlined frame borrows the call's arguments,
        // whose owners end after the frame it bound.
        let mut instruction_owners = Some(arguments);
        if inlined_frame {
            drop(instruction_owners.take());
        }
        // The bound slots are the callee frame's parameters: the moved call
        // arguments (`Admission::Move`) or the frame's own references
        // (`Admission::Copy`). An adopting entry takes them over and releases
        // them in frame order at its own exit; for a borrowing entry they end
        // in frame order when it returns.
        let result = if function_bits_adopt_arguments(func_bits) {
            slots.surrender_to_entry();
            call_function_obj_moved(_py, func_bits, final_args.as_slice())
        } else {
            call_function_obj_bound_vec(_py, func_bits, final_args.as_slice())
        };
        drop(slots);
        drop(instruction_owners);
        result
    }
}

/// Argument words a runtime call entry reads through `(args_ptr, nargs)`,
/// copied out on entry. The WASM lane spills them into one shared region that
/// every reentrant compiled call reuses, so no entry may read that region once
/// Python code can run.
pub(crate) struct EntryArguments {
    inline: [u64; ENTRY_ARGUMENTS_INLINE],
    len: usize,
    heap: Vec<u64>,
}

const ENTRY_ARGUMENTS_INLINE: usize = 16;

impl EntryArguments {
    /// Copy `args_ptr_bits[..nargs]`. `None`: the range lies outside the
    /// active target.
    ///
    /// # Safety
    /// A nonzero `nargs` requires `args_ptr_bits` to address that many
    /// initialized words, as every compiled call site's spill does.
    pub(crate) unsafe fn copy(args_ptr_bits: u64, nargs: u64) -> Option<Self> {
        let args_ptr = crate::provenance::abi::const_ptr::<u64>(args_ptr_bits)?;
        let raw = unsafe { crate::provenance::abi::slice(args_ptr, nargs) }?;
        let mut arguments = Self {
            inline: [0; ENTRY_ARGUMENTS_INLINE],
            len: raw.len(),
            heap: Vec::new(),
        };
        if raw.len() <= ENTRY_ARGUMENTS_INLINE {
            arguments.inline[..raw.len()].copy_from_slice(raw);
        } else {
            arguments.heap = raw.to_vec();
        }
        Some(arguments)
    }

    pub(crate) fn as_slice(&self) -> &[u64] {
        if self.len <= ENTRY_ARGUMENTS_INLINE {
            &self.inline[..self.len]
        } else {
            &self.heap
        }
    }
}

/// A call instruction's cleanup of the value-stack references it still owns
/// when no frame took them over: after a callee without an inlined frame
/// returns, or when no callee ran at all (`DECREF_INPUTS`). First to last
/// through 3.13, last to first from 3.14; `receiver` is the bottom value.
pub(crate) fn release_stack_arguments(py: &PyToken<'_>, receiver: Option<u64>, args: &[u64]) {
    let owned = usize::from(receiver.is_some()) + args.len();
    if owned > 1 && crate::object::ops_sys::runtime_target_at_least(py, 3, 14) {
        release_reversed(py, args);
        if let Some(receiver) = receiver {
            dec_ref_bits(py, receiver);
        }
    } else {
        if let Some(receiver) = receiver {
            dec_ref_bits(py, receiver);
        }
        release_in_order(py, args);
    }
}

/// Whether `func_bits` is a plain Python function that takes `supplied`
/// positional arguments over directly: its entry adopts, its borrowed-lane
/// trampoline exists, and those arguments need no binding.
unsafe fn takes_positional_arguments_over(
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

/// CPython's CALL on a callable reference the call instruction owns: an
/// ordinary source call (`call_func`, `call_method`, the fallback leg of
/// `call_guarded`, and `call_bind`/`call_indirect` of a stack-form builder).
/// A bound method hands its receiver to the argument vector and ends before
/// its function runs, so a temporary bound method's receiver is then owned by
/// the callee frame alone. Any other callable ends after the call, once the
/// argument vector has ended, as `DECREF_INPUTS` releases the callable last.
/// `dispatch` runs the call on a callable this call keeps alive.
unsafe fn call_with_adopted_callable<'a, 'py>(
    py: &'a PyToken<'py>,
    callable_bits: u64,
    mut args: CallArguments<'a, 'py>,
    dispatch: impl FnOnce(u64, CallArguments<'a, 'py>) -> u64,
) -> u64 {
    unsafe {
        if let Some(method_ptr) = obj_from_bits(callable_bits).as_ptr()
            && object_type_id(method_ptr) == TYPE_ID_BOUND_METHOD
        {
            let func_bits = bound_method_func_bits(method_ptr);
            // The argument vector retains the receiver and the call holds the
            // function: releasing the bound method runs no finalizer of either.
            if let Err(err) = args.prepend_positional(bound_method_self_bits(method_ptr)) {
                drop(args);
                dec_ref_bits(py, callable_bits);
                return err;
            }
            inc_ref_bits(py, func_bits);
            dec_ref_bits(py, callable_bits);
            let result = dispatch(func_bits, args);
            dec_ref_bits(py, func_bits);
            return result;
        }
        let result = dispatch(callable_bits, args);
        dec_ref_bits(py, callable_bits);
        result
    }
}

/// Call `func_bits`, which the caller keeps alive, with a call instruction's
/// adopted `receiver` and `args`. A plain adopting function of exact
/// positional arity takes them over directly; anything else binds them as the
/// instruction's argument vector, which moves them into an adopting frame or
/// ends them after a borrowing callee returns.
unsafe fn call_owned_function(
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

/// An ordinary source call instruction (CPython's CALL) whose callable and
/// positional arguments this call now owns: `call_func`, `call_method` and the
/// fallback leg of `call_guarded`. A temporary bound method ends before its
/// function runs; the arguments move into an adopting frame or end as
/// `DECREF_INPUTS` once a borrowing callee returns. None of them ever returns
/// to the caller.
pub(crate) unsafe fn call_owned_arguments(
    _py: &PyToken<'_>,
    callable_bits: u64,
    positional: &[u64],
) -> u64 {
    unsafe {
        if exception_pending(_py) {
            // A pending error means argument preparation failed: the operands
            // end as the ended instruction's inputs, the callable last.
            release_stack_arguments(_py, None, positional);
            dec_ref_bits(_py, callable_bits);
            return MoltObject::none().bits();
        }
        if takes_positional_arguments_over(_py, callable_bits, positional.len()) {
            let result = call_function_obj_moved(_py, callable_bits, positional);
            dec_ref_bits(_py, callable_bits);
            return result;
        }
        // A bound method of such a function: its receiver moves into the
        // frame as `self` and the method ends before the function runs.
        if let Some(method_ptr) = obj_from_bits(callable_bits).as_ptr()
            && object_type_id(method_ptr) == TYPE_ID_BOUND_METHOD
        {
            let func_bits = bound_method_func_bits(method_ptr);
            let self_bits = bound_method_self_bits(method_ptr);
            if positional.len() < 16
                && takes_positional_arguments_over(_py, func_bits, positional.len() + 1)
            {
                inc_ref_bits(_py, self_bits);
                inc_ref_bits(_py, func_bits);
                dec_ref_bits(_py, callable_bits);
                let result = call_owned_function(_py, func_bits, Some(self_bits), positional);
                dec_ref_bits(_py, func_bits);
                return result;
            }
        }
        match CallArguments::moved(_py, None, positional) {
            Ok(arguments) => {
                call_with_adopted_callable(_py, callable_bits, arguments, |callable, arguments| {
                    call_bind_with_arguments(_py, callable, arguments)
                })
            }
            Err(err) => {
                dec_ref_bits(_py, callable_bits);
                err
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn callargs_pending_error_stops_mutation_and_consuming_dispatch() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let item = alloc_list(_py, &[]);
                let item_bits = MoltObject::from_ptr(item).bits();
                let baseline = (*crate::header_from_obj_ptr(item)).ref_count_snapshot();
                // No callback or invalid-callee TypeError may replace the
                // original exception, including cached and indirect entries.
                for dispatch in 0..3 {
                    let builder = super::molt_callargs_new(1, 0);
                    let builder_ptr = super::ptr_from_bits(builder);
                    super::molt_callargs_push_pos(builder, item_bits);
                    assert_eq!(
                        (*crate::header_from_obj_ptr(item)).ref_count_snapshot(),
                        baseline + 1
                    );
                    crate::raise_exception::<()>(_py, "ValueError", "argument failure");
                    let original = crate::exception_last_bits_noinc(_py).unwrap();
                    assert_eq!(super::molt_callargs_new(0, 0), 0);
                    super::molt_callargs_push_pos(builder, item_bits);
                    super::molt_callargs_push_kw(builder, item_bits, item_bits);
                    // Invalid iterables/mappings would raise fresh exceptions
                    // if the failed transaction reached protocol dispatch.
                    super::molt_callargs_expand_star(builder, MoltObject::none().bits());
                    super::molt_callargs_expand_kwstar(builder, MoltObject::none().bits());
                    assert_eq!((*super::callargs_ptr(builder_ptr)).pos, [item_bits]);
                    assert!(obj_from_bits((*super::callargs_ptr(builder_ptr)).keywords).is_none());
                    let invalid_callee = MoltObject::from_int(42).bits();
                    let result = match dispatch {
                        0 => super::molt_call_bind(invalid_callee, builder),
                        1 => super::inline_cache::molt_call_bind_ic(23, invalid_callee, builder),
                        _ => {
                            super::inline_cache::molt_call_indirect_ic(29, invalid_callee, builder)
                        }
                    };
                    assert!(obj_from_bits(result).is_none());
                    assert_eq!(crate::exception_last_bits_noinc(_py), Some(original));
                    assert!(!super::callargs_builder_is_live(_py, builder_ptr));
                    assert_eq!(
                        (*crate::header_from_obj_ptr(item)).ref_count_snapshot(),
                        baseline
                    );
                    crate::molt_exception_clear();
                }
                dec_ref_bits(_py, item_bits);
            }
        });
    }

    #[test]
    fn bound_call_slots_own_borrowed_values_and_transferred_containers() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let value = alloc_list(_py, &[]);
                assert!(!value.is_null());
                let value_bits = MoltObject::from_ptr(value).bits();
                let refs = || (*crate::header_from_obj_ptr(value)).ref_count_snapshot();
                let baseline = refs();
                let layout = super::FrameSlotLayout {
                    positional: 2,
                    has_vararg: false,
                    keyword_only: 0,
                    has_varkw: false,
                };
                let mut slots = super::BoundCallSlots::new(_py, layout).unwrap();
                slots.set_borrowed(0, value_bits);
                assert_eq!(refs(), baseline + 1);
                let tuple = alloc_tuple(_py, &[value_bits]);
                assert!(!tuple.is_null());
                slots.set_owned(1, MoltObject::from_ptr(tuple).bits());
                assert_eq!(refs(), baseline + 2);
                drop(slots);
                assert_eq!(refs(), baseline);
                dec_ref_bits(_py, value_bits);
            }
        });
    }

    #[test]
    fn keyword_default_lookup_returns_an_owner_independent_of_metadata_dictionary() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let function = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::provenance::abi::expose_function_address(
                        compiled_identity_returns_owned_arg as *const (),
                    ),
                    1,
                );
                assert!(!function.is_null());
                let function_bits = MoltObject::from_ptr(function).bits();
                let name = crate::alloc_string(_py, b"value");
                let value = alloc_list(_py, &[]);
                assert!(!name.is_null() && !value.is_null());
                let name_bits = MoltObject::from_ptr(name).bits();
                let value_bits = MoltObject::from_ptr(value).bits();
                let dictionary = crate::alloc_dict_with_pairs(_py, &[name_bits, value_bits]);
                assert!(!dictionary.is_null());
                let dictionary_bits = MoltObject::from_ptr(dictionary).bits();
                let attribute = intern_metadata_name(_py, b"__kwdefaults__");
                assert!(crate::call::class_init::function_set_attr_bits(
                    _py,
                    function,
                    attribute,
                    dictionary_bits,
                ));
                dec_ref_bits(_py, dictionary_bits);
                let value_before = (*crate::header_from_obj_ptr(value)).ref_count_snapshot();
                let owned = super::function_kwdefault_owned(_py, function, name_bits)
                    .unwrap()
                    .unwrap();
                assert_eq!(owned, value_bits);
                assert_eq!(
                    (*crate::header_from_obj_ptr(value)).ref_count_snapshot(),
                    value_before + 1,
                );
                assert!(crate::call::class_init::function_set_attr_bits(
                    _py,
                    function,
                    attribute,
                    MoltObject::none().bits(),
                ));
                assert_eq!(
                    (*crate::header_from_obj_ptr(value)).ref_count_snapshot(),
                    value_before,
                    "the owned default survives metadata replacement",
                );
                dec_ref_bits(_py, owned);
                for bits in [function_bits, name_bits, value_bits] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!crate::exception_pending(_py));
            }
        });
    }

    #[test]
    fn callargs_expansion_has_one_keyword_owner_and_releases_iterator() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                // Ownership deltas require mortal payloads, not the canonical
                // immortal identifier strings returned by alloc_string.
                let key = crate::object::builders::alloc_string_nointern(_py, b"key");
                let value = crate::object::builders::alloc_string_nointern(_py, b"value");
                let key_bits = MoltObject::from_ptr(key).bits();
                let value_bits = MoltObject::from_ptr(value).bits();
                let list = crate::alloc_list(_py, &[value_bits]);
                let list_bits = MoltObject::from_ptr(list).bits();
                let refs = |ptr| (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot();
                let key_before = refs(key);
                let value_before = refs(value);
                let list_before = refs(list);
                let builder = super::molt_callargs_new(0, 0);
                super::molt_callargs_push_kw(builder, key_bits, value_bits);
                assert!(!crate::exception_pending(_py));
                assert_eq!(
                    refs(key),
                    key_before + 1,
                    "expansion must retain keywords only through their dictionary"
                );
                assert_eq!(refs(value), value_before + 1);
                super::molt_callargs_expand_star(builder, list_bits);
                assert!(!crate::exception_pending(_py));
                assert_eq!(
                    refs(list),
                    list_before,
                    "star expansion must release its iterator"
                );
                assert_eq!(refs(value), value_before + 2);
                crate::dec_ref_bits(_py, builder);
                assert_eq!(refs(key), key_before);
                assert_eq!(refs(value), value_before);
                crate::dec_ref_bits(_py, list_bits);
                crate::dec_ref_bits(_py, key_bits);
                crate::dec_ref_bits(_py, value_bits);
            }
        });
    }

    #[test]
    fn keyword_admission_reads_replacements_and_retains_a_shared_mapping() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let key = crate::object::builders::alloc_string_nointern(_py, b"key");
                let old = alloc_list(_py, &[]);
                let replacement = alloc_list(_py, &[MoltObject::from_int(9).bits()]);
                let later = alloc_list(_py, &[]);
                assert!(!key.is_null() && !old.is_null());
                assert!(!replacement.is_null() && !later.is_null());
                let key_bits = MoltObject::from_ptr(key).bits();
                let old_bits = MoltObject::from_ptr(old).bits();
                let replacement_bits = MoltObject::from_ptr(replacement).bits();
                let later_bits = MoltObject::from_ptr(later).bits();
                let refs = |ptr| (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot();
                let key_before = refs(key);
                let old_before = refs(old);
                let replacement_before = refs(replacement);
                let builder = super::molt_callargs_new(0, 1);
                assert_ne!(builder, 0);
                super::molt_callargs_push_kw(builder, key_bits, old_bits);
                let dict_bits = (*super::callargs_ptr(ptr_from_bits(builder))).keywords;
                let dict = obj_from_bits(dict_bits).as_ptr().unwrap();
                // Both a stateful __eq__ during insertion and foreign mutation
                // can replace an existing value without growing dictionary order.
                crate::dict_set_in_place(_py, dict, key_bits, replacement_bits);
                assert!(!crate::exception_pending(_py));
                assert_eq!(
                    refs(old),
                    old_before,
                    "no stale builder edge may retain the old value"
                );
                // Another owner can still reach the mapping, as a C view or a
                // GC referrer could. Admission must not clear it.
                crate::inc_ref_bits(_py, dict_bits);
                let mut arguments =
                    super::CallArguments::from_builder(_py, ptr_from_bits(builder)).unwrap();
                dec_ref_bits(_py, builder);
                let view = arguments.unpacked_view().unwrap();
                assert_eq!(view.kw_names, [key_bits]);
                assert_eq!(view.kw_values, [replacement_bits]);
                assert_eq!(
                    crate::dict_order(dict).len(),
                    2,
                    "a shared mapping stays intact"
                );
                assert_eq!(refs(key), key_before + 2);
                assert_eq!(refs(replacement), replacement_before + 2);
                // The other owner's later mutation cannot change what binding reads.
                crate::dict_set_in_place(_py, dict, key_bits, later_bits);
                crate::dict_clear_in_place(_py, dict);
                assert!(!crate::exception_pending(_py));
                let view = arguments.unpacked_view().unwrap();
                assert_eq!(view.kw_values, [replacement_bits]);
                assert_eq!(refs(key), key_before + 1);
                assert_eq!(refs(replacement), replacement_before + 1);
                drop(arguments);
                assert_eq!(refs(key), key_before);
                assert_eq!(refs(replacement), replacement_before);
                dec_ref_bits(_py, dict_bits);
                for bits in [key_bits, old_bits, replacement_bits, later_bits] {
                    dec_ref_bits(_py, bits);
                }
            }
        });
    }

    #[test]
    fn callargs_clone_and_ic_read_the_live_keyword_dictionary() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let key = crate::alloc_string(_py, b"key");
                assert!(!key.is_null());
                let key_bits = MoltObject::from_ptr(key).bits();
                let builder = super::molt_callargs_new(1, 1);
                assert_ne!(builder, 0);
                let value = MoltObject::from_int(17).bits();
                super::molt_callargs_push_pos(builder, value);
                super::molt_callargs_push_kw(builder, key_bits, MoltObject::from_int(1).bits());
                let args = super::callargs_ptr(ptr_from_bits(builder));
                let dict = obj_from_bits((*args).keywords).as_ptr().unwrap();
                crate::dict_set_in_place(_py, dict, key_bits, MoltObject::from_int(2).bits());
                let clone = super::clone_callargs_builder_bits(_py, builder).unwrap();
                let cloned_args = super::callargs_ptr(ptr_from_bits(clone));
                assert_ne!((*args).keywords, (*cloned_args).keywords);
                let mut cloned =
                    super::CallArguments::from_builder(_py, ptr_from_bits(clone)).unwrap();
                assert_eq!(
                    cloned.unpacked_view().unwrap().kw_values,
                    [MoltObject::from_int(2).bits()]
                );
                crate::dict_clear_in_place(_py, dict);
                assert_eq!((*args).keyword_count(), 0);
                assert_eq!(cloned.keyword_count(), 1);
                assert_eq!(
                    super::callargs_positional_snapshot(_py, builder).unwrap(),
                    [value]
                );
                let mut original =
                    super::CallArguments::from_builder(_py, ptr_from_bits(builder)).unwrap();
                let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::provenance::abi::expose_function_address(
                        compiled_identity_returns_owned_arg as *const (),
                    ),
                    1,
                );
                assert!(!func_ptr.is_null());
                let func_bits = MoltObject::from_ptr(func_ptr).bits();
                let entry = CallBindIcEntry {
                    fn_ptr: crate::function_fn_ptr(func_ptr),
                    target_bits: 0,
                    class_bits: 0,
                    class_version: 0,
                    type_version: crate::global_type_version(),
                    function_version: 0,
                    cached_alloc_size: 0,
                    arity: 1,
                    kind: CALL_BIND_IC_KIND_DIRECT_FUNC,
                };
                assert_eq!(
                    try_call_bind_ic_fast(_py, entry, func_bits, &mut original),
                    Some(value)
                );
                assert_eq!(
                    try_call_bind_ic_fast(_py, entry, func_bits, &mut cloned),
                    None
                );
                drop(original);
                drop(cloned);
                for bits in [builder, clone, key_bits, func_bits] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!crate::exception_pending(_py));
            }
        });
    }

    #[test]
    fn call_bind_uses_replaced_keyword_value_at_binding_boundary() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::provenance::abi::expose_function_address(
                        compiled_identity_returns_owned_arg as *const (),
                    ),
                    1,
                );
                assert!(!func_ptr.is_null());
                let func_bits = MoltObject::from_ptr(func_ptr).bits();
                let key = crate::alloc_string(_py, b"key");
                assert!(!key.is_null());
                let key_bits = MoltObject::from_ptr(key).bits();
                let names = alloc_tuple(_py, &[key_bits]);
                assert!(!names.is_null());
                let names_bits = MoltObject::from_ptr(names).bits();
                assert!(crate::call::class_init::function_set_attr_bits(
                    _py,
                    func_ptr,
                    intern_metadata_name(_py, b"__molt_arg_names__"),
                    names_bits,
                ));
                let builder = super::molt_callargs_new(0, 1);
                assert_ne!(builder, 0);
                super::molt_callargs_push_kw(builder, key_bits, MoltObject::from_int(1).bits());
                let args = super::callargs_ptr(ptr_from_bits(builder));
                let dict = obj_from_bits((*args).keywords).as_ptr().unwrap();
                let expected = MoltObject::from_int(29).bits();
                crate::dict_set_in_place(_py, dict, key_bits, expected);
                assert_eq!(super::molt_call_bind(func_bits, builder), expected);
                assert!(!crate::exception_pending(_py));
                for bits in [names_bits, key_bits, func_bits] {
                    dec_ref_bits(_py, bits);
                }
            }
        });
    }

    #[test]
    fn callargs_nonstring_keywords_are_rejected_at_call_boundary() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let builder = super::molt_callargs_new(0, 0);
                super::molt_callargs_push_kw(
                    builder,
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                );
                assert!(!crate::exception_pending(_py));
                let arguments =
                    super::CallArguments::from_builder(_py, super::ptr_from_bits(builder)).unwrap();
                crate::dec_ref_bits(_py, builder);
                assert!(!arguments.validate_keywords());
                assert!(crate::exception_pending(_py));
                crate::molt_exception_clear();
                // Names already unpacked from a mapping face the same check.
                let unpacked = super::CallArguments::retained(
                    _py,
                    None,
                    &[],
                    &[MoltObject::from_int(3).bits()],
                    &[MoltObject::from_int(4).bits()],
                )
                .unwrap();
                assert!(!unpacked.validate_keywords());
                assert!(crate::exception_pending(_py));
                crate::molt_exception_clear();
            }
        });
    }

    #[test]
    fn consuming_entry_moves_builder_edges_into_the_call_argument_vector() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let key = crate::object::builders::alloc_string_nointern(_py, b"key");
                let value = alloc_list(_py, &[]);
                let item = alloc_list(_py, &[]);
                assert!(!key.is_null() && !value.is_null() && !item.is_null());
                let key_bits = MoltObject::from_ptr(key).bits();
                let value_bits = MoltObject::from_ptr(value).bits();
                let item_bits = MoltObject::from_ptr(item).bits();
                let refs = |ptr| (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot();
                let builder = super::molt_callargs_new(1, 1);
                super::molt_callargs_push_pos(builder, item_bits);
                super::molt_callargs_push_kw(builder, key_bits, value_bits);
                assert!(!crate::exception_pending(_py));
                let counts = (refs(item), refs(key), refs(value));
                let builder_ptr = ptr_from_bits(builder);
                let mut arguments = super::CallArguments::from_builder(_py, builder_ptr).unwrap();
                // T1 moved both edges: the builder is empty and no count changed.
                assert!((*super::callargs_ptr(builder_ptr)).pos.is_empty());
                assert!(obj_from_bits((*super::callargs_ptr(builder_ptr)).keywords).is_none());
                assert_eq!(arguments.positional(), [item_bits]);
                assert_eq!((refs(item), refs(key), refs(value)), counts);
                dec_ref_bits(_py, builder);
                assert_eq!(refs(item), counts.0, "an emptied builder releases nothing");
                // This call holds the mapping's only reference: its entries move too.
                let view = arguments.unpacked_view().unwrap();
                assert_eq!(view.kw_names, [key_bits]);
                assert_eq!(view.kw_values, [value_bits]);
                assert_eq!((refs(item), refs(key), refs(value)), counts);
                drop(arguments);
                assert_eq!(
                    (refs(item), refs(key), refs(value)),
                    (counts.0 - 1, counts.1 - 1, counts.2 - 1),
                    "the argument vector released exactly the edges it received"
                );
                for bits in [item_bits, key_bits, value_bits] {
                    dec_ref_bits(_py, bits);
                }
            }
        });
    }

    #[test]
    fn a_shared_builder_keeps_its_edges_and_the_call_retains_its_own() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let item = alloc_list(_py, &[]);
                assert!(!item.is_null());
                let item_bits = MoltObject::from_ptr(item).bits();
                let refs = || (*crate::header_from_obj_ptr(item)).ref_count_snapshot();
                let builder = super::molt_callargs_new(1, 0);
                super::molt_callargs_push_pos(builder, item_bits);
                let before = refs();
                // Another owner can still reach the builder: T1 must not drain it.
                crate::inc_ref_bits(_py, builder);
                let arguments =
                    super::CallArguments::from_builder(_py, ptr_from_bits(builder)).unwrap();
                assert_eq!(
                    (*super::callargs_ptr(ptr_from_bits(builder))).pos,
                    [item_bits]
                );
                assert_eq!(refs(), before + 1);
                drop(arguments);
                assert_eq!(refs(), before);
                dec_ref_bits(_py, builder);
                dec_ref_bits(_py, builder);
                assert_eq!(refs(), before - 1);
                dec_ref_bits(_py, item_bits);
            }
        });
    }

    #[test]
    fn frame_slot_layout_projects_declared_frame_order() {
        // `def f(a, b, *rest, k, **kw)`: the ABI order is `(a, b, rest, k, kw)`
        // while CPython's frame (`co_varnames`) is `(a, b, k, rest, kw)`.
        let layout = super::FrameSlotLayout {
            positional: 2,
            has_vararg: true,
            keyword_only: 1,
            has_varkw: true,
        };
        assert_eq!(layout.len(), 5);
        assert_eq!(layout.vararg_slot(), 2);
        assert_eq!(layout.keyword_only_slot(0), 3);
        assert_eq!(layout.varkw_slot(), 4);
        assert_eq!(layout.declared_order().collect::<Vec<_>>(), [0, 1, 3, 2, 4]);
        // 3.14 clears the same declared slots last to first.
        assert_eq!(
            layout.declared_order().rev().collect::<Vec<_>>(),
            [4, 2, 3, 1, 0]
        );
        let keyword_only = super::FrameSlotLayout {
            positional: 1,
            has_vararg: false,
            keyword_only: 2,
            has_varkw: false,
        };
        assert_eq!(keyword_only.declared_order().collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn bound_frame_is_the_only_argument_owner_during_the_call() {
        use std::sync::Mutex;
        static PROBES: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        static OBSERVED: Mutex<Vec<u32>> = Mutex::new(Vec::new());
        // `def observed(a, *rest, k, **kw)`, called in ABI order `(a, rest, k, kw)`.
        extern "C" fn observe_frame_owners(_a: u64, _rest: u64, _k: u64, _kw: u64) -> i64 {
            let probes = PROBES.lock().unwrap().clone();
            let counts = probes
                .into_iter()
                .map(|bits| unsafe {
                    let ptr = obj_from_bits(bits).as_ptr().expect("probed argument");
                    (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot()
                })
                .collect();
            *OBSERVED.lock().unwrap() = counts;
            MoltObject::none().bits() as i64
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let function = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::provenance::abi::expose_function_address(
                        observe_frame_owners as *const (),
                    ),
                    4,
                );
                assert!(!function.is_null());
                let function_bits = MoltObject::from_ptr(function).bits();
                let string =
                    |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
                let names = [
                    string(b"a"),
                    string(b"k"),
                    string(b"rest"),
                    string(b"kw"),
                    string(b"x"),
                ];
                let positional_names = MoltObject::from_ptr(alloc_tuple(_py, &names[..1])).bits();
                let keyword_only_names =
                    MoltObject::from_ptr(alloc_tuple(_py, &names[1..2])).bits();
                for (field, value) in [
                    (b"__molt_arg_names__".as_slice(), positional_names),
                    (b"__molt_kwonly_names__".as_slice(), keyword_only_names),
                    (b"__molt_vararg__".as_slice(), names[2]),
                    (b"__molt_varkw__".as_slice(), names[3]),
                ] {
                    assert!(crate::call::class_init::function_set_attr_bits(
                        _py,
                        function,
                        intern_metadata_name(_py, field),
                        value,
                    ));
                }
                let values: Vec<u64> = (0..5)
                    .map(|_| MoltObject::from_ptr(alloc_list(_py, &[])).bits())
                    .collect();
                *PROBES.lock().unwrap() = values.clone();
                let builder = super::molt_callargs_new(3, 2);
                for &bits in &values[..3] {
                    super::molt_callargs_push_pos(builder, bits);
                }
                super::molt_callargs_push_kw(builder, names[1], values[3]);
                super::molt_callargs_push_kw(builder, names[4], values[4]);
                assert!(!crate::exception_pending(_py));
                let result = super::molt_call_bind(function_bits, builder);
                assert!(!crate::exception_pending(_py));
                assert!(obj_from_bits(result).is_none());
                // `a` and `k` sit in parameter slots, `rest` owns two values and
                // `kw` owns `x`. No builder or keyword pin outlives admission.
                assert_eq!(
                    *OBSERVED.lock().unwrap(),
                    [2, 2, 2, 2, 2],
                    "each argument has this test's owner and exactly one frame owner"
                );
                for &bits in &values {
                    let ptr = obj_from_bits(bits).as_ptr().unwrap();
                    assert_eq!(
                        (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot(),
                        1,
                        "the frame released its owners when the activation ended"
                    );
                }
                for bits in values
                    .into_iter()
                    .chain([function_bits, positional_names, keyword_only_names])
                    .chain(names)
                {
                    dec_ref_bits(_py, bits);
                }
            }
        });
    }

    #[test]
    fn failed_binding_releases_every_argument_exactly_once() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static ENTERED: AtomicBool = AtomicBool::new(false);
        // `def two(a, b)`: a failed binding never enters the callee.
        extern "C" fn two(_a: u64, _b: u64) -> i64 {
            ENTERED.store(true, Ordering::SeqCst);
            MoltObject::none().bits() as i64
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let function = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::provenance::abi::expose_function_address(two as *const ()),
                    2,
                );
                assert!(!function.is_null());
                let function_bits = MoltObject::from_ptr(function).bits();
                let string =
                    |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
                let names = [string(b"a"), string(b"b"), string(b"c"), string(b"d")];
                let parameters = MoltObject::from_ptr(alloc_tuple(_py, &names[..2])).bits();
                assert!(crate::call::class_init::function_set_attr_bits(
                    _py,
                    function,
                    intern_metadata_name(_py, b"__molt_arg_names__"),
                    parameters,
                ));
                // Surplus positional values; a parameter bound twice; an
                // unexpected keyword after a bound one; a missing parameter.
                for (positional, keywords) in [
                    (3, &[][..]),
                    (2, &names[..1]),
                    (1, &names[1..4]),
                    (1, &[][..]),
                ] {
                    let values: Vec<u64> = (0..positional + keywords.len())
                        .map(|_| MoltObject::from_ptr(alloc_list(_py, &[])).bits())
                        .collect();
                    let builder =
                        super::molt_callargs_new(positional as u64, keywords.len() as u64);
                    for &bits in &values[..positional] {
                        super::molt_callargs_push_pos(builder, bits);
                    }
                    for (&name, &bits) in keywords.iter().zip(&values[positional..]) {
                        super::molt_callargs_push_kw(builder, name, bits);
                    }
                    assert!(!crate::exception_pending(_py));
                    let result = super::molt_call_bind(function_bits, builder);
                    assert!(obj_from_bits(result).is_none());
                    assert!(crate::exception_pending(_py), "the binding error is raised");
                    assert!(!ENTERED.load(Ordering::SeqCst), "the callee never ran");
                    crate::molt_exception_clear();
                    for bits in values {
                        let ptr = obj_from_bits(bits).as_ptr().unwrap();
                        assert_eq!(
                            (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot(),
                            1,
                            "every argument is released exactly once"
                        );
                        dec_ref_bits(_py, bits);
                    }
                }
                for bits in [function_bits, parameters].into_iter().chain(names) {
                    dec_ref_bits(_py, bits);
                }
            }
        });
    }

    #[test]
    fn extension_callees_receive_the_whole_mapping_or_a_fresh_one() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let key = crate::object::builders::alloc_string_nointern(_py, b"key");
                let value = alloc_list(_py, &[]);
                assert!(!key.is_null() && !value.is_null());
                let key_bits = MoltObject::from_ptr(key).bits();
                let value_bits = MoltObject::from_ptr(value).bits();
                let builder = super::molt_callargs_new(0, 1);
                super::molt_callargs_push_kw(builder, key_bits, value_bits);
                let dict_bits = (*super::callargs_ptr(ptr_from_bits(builder))).keywords;
                let mut arguments =
                    super::CallArguments::from_builder(_py, ptr_from_bits(builder)).unwrap();
                dec_ref_bits(_py, builder);
                // While whole, the builder's own dictionary is lent without copying.
                let whole = arguments.keyword_mapping().unwrap();
                assert_eq!(whole, dict_bits);
                dec_ref_bits(_py, whole);
                // Once unpacked, a C callee still receives a mapping it may keep.
                let _ = arguments.unpacked_view().unwrap();
                let fresh = arguments.keyword_mapping().unwrap();
                let fresh_ptr = obj_from_bits(fresh).as_ptr().unwrap();
                assert_eq!(
                    crate::dict_order(fresh_ptr).as_slice(),
                    [key_bits, value_bits]
                );
                drop(arguments);
                assert_eq!(
                    (*crate::header_from_obj_ptr(value)).ref_count_snapshot(),
                    2,
                    "the fresh mapping and this test own the value"
                );
                dec_ref_bits(_py, fresh);
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
        });
    }

    static RELEASED: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

    extern "C" fn record_release(self_bits: u64) -> u64 {
        RELEASED.lock().unwrap().push(self_bits);
        MoltObject::none().bits()
    }

    /// Instances whose finalizer records them, so a test observes the order in
    /// which a call ends its last references (CPython's `__del__` oracle).
    struct ReleaseProbes {
        class_bits: u64,
        finalizer_bits: u64,
    }

    impl ReleaseProbes {
        unsafe fn new(py: &crate::PyToken<'_>) -> Self {
            let finalizer = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::provenance::abi::expose_function_address(record_release as *const ()),
                1,
            );
            assert!(!finalizer.is_null());
            let finalizer_bits = MoltObject::from_ptr(finalizer).bits();
            let name_bits = MoltObject::from_ptr(crate::alloc_string(py, b"ReleaseProbe")).bits();
            let del_bits = MoltObject::from_ptr(crate::alloc_string(py, b"__del__")).bits();
            let attrs = [del_bits, finalizer_bits];
            let bases = [crate::builtin_classes(py).object];
            let class_bits = unsafe {
                crate::object::ops::molt_guarded_class_def(
                    name_bits,
                    crate::provenance::abi::expose_address(bases.as_ptr()),
                    bases.len() as u64,
                    crate::provenance::abi::expose_address(attrs.as_ptr()),
                    1,
                    std::mem::size_of::<u64>() as i64,
                    1,
                    1,
                )
            };
            assert!(!obj_from_bits(class_bits).is_none() && !crate::exception_pending(py));
            dec_ref_bits(py, del_bits);
            dec_ref_bits(py, name_bits);
            Self {
                class_bits,
                finalizer_bits,
            }
        }

        /// Fresh instances, each owned only by the returned reference.
        unsafe fn instances(&self, py: &crate::PyToken<'_>, count: usize) -> Vec<u64> {
            let class_ptr = obj_from_bits(self.class_bits).as_ptr().unwrap();
            (0..count)
                .map(|_| {
                    let bits = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
                    assert!(!obj_from_bits(bits).is_none());
                    bits
                })
                .collect()
        }

        /// The order in which `probes` were released, as indices into it.
        fn released(probes: &[u64]) -> Vec<usize> {
            RELEASED
                .lock()
                .unwrap()
                .iter()
                .filter_map(|bits| probes.iter().position(|probe| probe == bits))
                .collect()
        }

        fn release(self, py: &crate::PyToken<'_>) {
            dec_ref_bits(py, self.class_bits);
            dec_ref_bits(py, self.finalizer_bits);
        }
    }

    /// Run `body` with the runtime targeting CPython 3.`minor` through the
    /// canonical target-version authority, then restore the previous target.
    fn with_target_minor<R>(py: &crate::PyToken<'_>, minor: i64, body: impl FnOnce() -> R) -> R {
        let state = runtime_state(py);
        let saved = state.sys_version_info.lock().unwrap().clone();
        *state.sys_version_info.lock().unwrap() =
            Some(crate::state::runtime_state::PythonVersionInfo {
                major: 3,
                minor,
                micro: 0,
                releaselevel: "final".to_string(),
                serial: 0,
            });
        let result = body();
        *state.sys_version_info.lock().unwrap() = saved;
        result
    }

    /// A function with binding metadata. `builtin` publishes it in the
    /// native-function class family, which never inlines a frame.
    unsafe fn metadata_function(
        py: &crate::PyToken<'_>,
        address: *const (),
        arity: u64,
        metadata: &[(&'static [u8], u64)],
        builtin: bool,
    ) -> u64 {
        let function = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            arity,
        );
        assert!(!function.is_null());
        for &(field, value) in metadata {
            assert!(unsafe {
                crate::call::class_init::function_set_attr_bits(
                    py,
                    function,
                    intern_metadata_name(py, field),
                    value,
                )
            });
        }
        let bits = MoltObject::from_ptr(function).bits();
        if builtin {
            let _ = crate::molt_function_set_builtin(bits);
            assert!(!crate::exception_pending(py));
        }
        bits
    }

    /// A call builder of `form` holding `positional` and `keywords`. The test
    /// then releases its own references, so the call holds the last ones.
    unsafe fn last_owner_call(
        py: &crate::PyToken<'_>,
        form: super::CallForm,
        positional: &[u64],
        keywords: &[(u64, u64)],
    ) -> u64 {
        let (positional_count, keyword_count) = (positional.len() as u64, keywords.len() as u64);
        let builder = match form {
            super::CallForm::Stack => super::molt_callargs_new(positional_count, keyword_count),
            super::CallForm::Expanded => {
                super::molt_callargs_new_expanded(positional_count, keyword_count)
            }
        };
        assert_ne!(builder, 0);
        for &bits in positional {
            unsafe { super::molt_callargs_push_pos(builder, bits) };
        }
        for &(name, value) in keywords {
            unsafe { super::molt_callargs_push_kw(builder, name, value) };
        }
        assert!(!crate::exception_pending(py));
        for &bits in positional
            .iter()
            .chain(keywords.iter().map(|(_, value)| value))
        {
            dec_ref_bits(py, bits);
        }
        RELEASED.lock().unwrap().clear();
        builder
    }

    extern "C" fn none_of_four(_a: u64, _b: u64, _c: u64, _d: u64) -> i64 {
        MoltObject::none().bits() as i64
    }

    extern "C" fn none_of_two(_a: u64, _b: u64) -> i64 {
        MoltObject::none().bits() as i64
    }

    extern "C" fn none_of_one(_a: u64) -> i64 {
        MoltObject::none().bits() as i64
    }

    extern "C" fn none_of_none() -> i64 {
        MoltObject::none().bits() as i64
    }

    #[test]
    fn inlined_frames_release_parameters_in_target_version_order() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let probes = ReleaseProbes::new(_py);
                let string =
                    |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
                let [a, k, rest, kw, x] = [&b"a"[..], b"k", b"rest", b"kw", b"x"].map(string);
                let positional_names = MoltObject::from_ptr(alloc_tuple(_py, &[a])).bits();
                let keyword_only_names = MoltObject::from_ptr(alloc_tuple(_py, &[k])).bits();
                // `def mixed(a, *rest, k, **kw)`, called in ABI order `(a, rest, k, kw)`.
                let mixed = metadata_function(
                    _py,
                    none_of_four as *const (),
                    4,
                    &[
                        (b"__molt_arg_names__", positional_names),
                        (b"__molt_kwonly_names__", keyword_only_names),
                        (b"__molt_vararg__", rest),
                        (b"__molt_varkw__", kw),
                    ],
                    false,
                );
                // mixed(a, r1, r2, k=k, x=x). CPython 3.12 and 3.13 clear the
                // frame as `a, k, rest, kw`; 3.14 as `kw, rest, k, a`. The
                // `*args` tuple releases last to first in every version.
                for (minor, expected) in [
                    (12, [0, 3, 2, 1, 4]),
                    (13, [0, 3, 2, 1, 4]),
                    (14, [4, 2, 1, 3, 0]),
                ] {
                    with_target_minor(_py, minor, || {
                        let values = probes.instances(_py, 5);
                        let builder = last_owner_call(
                            _py,
                            super::CallForm::Stack,
                            &values[..3],
                            &[(k, values[3]), (x, values[4])],
                        );
                        let result = super::molt_call_bind(mixed, builder);
                        assert!(obj_from_bits(result).is_none());
                        assert!(!crate::exception_pending(_py));
                        assert_eq!(ReleaseProbes::released(&values), expected, "3.{minor}");
                    });
                }
                for bits in [
                    mixed,
                    positional_names,
                    keyword_only_names,
                    a,
                    k,
                    rest,
                    kw,
                    x,
                ] {
                    dec_ref_bits(_py, bits);
                }
                probes.release(_py);
            }
        });
    }

    #[test]
    fn failed_frame_binding_releases_in_initialize_locals_order() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let probes = ReleaseProbes::new(_py);
                let string =
                    |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
                let [a, b, c, d] = [&b"a"[..], b"b", b"c", b"d"].map(string);
                let two_names = MoltObject::from_ptr(alloc_tuple(_py, &[a, b])).bits();
                let one_names = MoltObject::from_ptr(alloc_tuple(_py, &[a])).bits();
                let two = metadata_function(
                    _py,
                    none_of_two as *const (),
                    2,
                    &[(b"__molt_arg_names__", two_names)],
                    false,
                );
                let one = metadata_function(
                    _py,
                    none_of_one as *const (),
                    1,
                    &[(b"__molt_arg_names__", one_names)],
                    false,
                );
                for (minor, remaining_keywords, surplus_duplicate) in
                    [(12, [2, 3, 0, 1], [1, 2, 0]), (14, [2, 3, 1, 0], [1, 2, 0])]
                {
                    with_target_minor(_py, minor, || {
                        // two(p1, b=k1, c=k2, d=k3): `c` is unexpected. kw_fail
                        // releases k2 and k3, then the partial frame clears.
                        let values = probes.instances(_py, 4);
                        let builder = last_owner_call(
                            _py,
                            super::CallForm::Stack,
                            &values[..1],
                            &[(b, values[1]), (c, values[2]), (d, values[3])],
                        );
                        let result = super::molt_call_bind(two, builder);
                        assert!(obj_from_bits(result).is_none());
                        assert!(crate::exception_pending(_py), "the binding error is raised");
                        let _ = crate::molt_exception_clear();
                        assert_eq!(
                            ReleaseProbes::released(&values),
                            remaining_keywords,
                            "3.{minor}"
                        );
                        // one(p1, p2, a=k1): p2 is surplus at once, then `a` is
                        // bound twice and the frame clears.
                        let values = probes.instances(_py, 3);
                        let builder = last_owner_call(
                            _py,
                            super::CallForm::Stack,
                            &values[..2],
                            &[(a, values[2])],
                        );
                        let result = super::molt_call_bind(one, builder);
                        assert!(obj_from_bits(result).is_none());
                        assert!(crate::exception_pending(_py), "the binding error is raised");
                        let _ = crate::molt_exception_clear();
                        assert_eq!(
                            ReleaseProbes::released(&values),
                            surplus_duplicate,
                            "3.{minor}"
                        );
                    });
                }
                for bits in [two, one, two_names, one_names, a, b, c, d] {
                    dec_ref_bits(_py, bits);
                }
                probes.release(_py);
            }
        });
    }

    #[test]
    fn expanded_calls_keep_their_containers_through_frame_admission() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let probes = ReleaseProbes::new(_py);
                let string =
                    |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
                let [a, b] = [&b"a"[..], b"b"].map(string);
                let no_names = MoltObject::from_ptr(alloc_tuple(_py, &[])).bits();
                let two_names = MoltObject::from_ptr(alloc_tuple(_py, &[a, b])).bits();
                let zero = metadata_function(
                    _py,
                    none_of_none as *const (),
                    0,
                    &[(b"__molt_arg_names__", no_names)],
                    false,
                );
                let two = metadata_function(
                    _py,
                    none_of_two as *const (),
                    2,
                    &[(b"__molt_arg_names__", two_names)],
                    false,
                );
                for (minor, success) in [(12, [0, 1]), (13, [0, 1]), (14, [1, 0])] {
                    with_target_minor(_py, minor, || {
                        // zero(p1, p2, *(), a=k1, b=k2): binding fails. The
                        // frame's references end first; the tuple (last to
                        // first) and then the mapping hold the last ones, as
                        // `_PyEvalFramePushAndInit_Ex` releases them.
                        let values = probes.instances(_py, 4);
                        let builder = last_owner_call(
                            _py,
                            super::CallForm::Expanded,
                            &values[..2],
                            &[(a, values[2]), (b, values[3])],
                        );
                        let result = super::molt_call_bind(zero, builder);
                        assert!(obj_from_bits(result).is_none());
                        assert!(crate::exception_pending(_py), "the binding error is raised");
                        let _ = crate::molt_exception_clear();
                        assert_eq!(ReleaseProbes::released(&values), [1, 0, 2, 3], "3.{minor}");
                        // two(*(p1, p2)): admission succeeds, the containers end
                        // before the callee runs, and the frame clears its own
                        // parameters in frame order rather than as a tuple.
                        let values = probes.instances(_py, 2);
                        let builder = last_owner_call(_py, super::CallForm::Expanded, &values, &[]);
                        let result = super::molt_call_bind(two, builder);
                        assert!(obj_from_bits(result).is_none());
                        assert!(!crate::exception_pending(_py));
                        assert_eq!(ReleaseProbes::released(&values), success, "3.{minor}");
                    });
                }
                for bits in [zero, two, no_names, two_names, a, b] {
                    dec_ref_bits(_py, bits);
                }
                probes.release(_py);
            }
        });
    }

    #[test]
    fn borrowing_callees_leave_the_last_release_to_the_call_instruction() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let probes = ReleaseProbes::new(_py);
                let string =
                    |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
                let [a, b, args, kwargs] = [&b"a"[..], b"b", b"args", b"kwargs"].map(string);
                let no_names = MoltObject::from_ptr(alloc_tuple(_py, &[])).bits();
                // A native `(*args, **kwargs)` callable, like `"".format`: it
                // binds its own references and never inlines a frame.
                let variadic = metadata_function(
                    _py,
                    none_of_two as *const (),
                    2,
                    &[
                        (b"__molt_arg_names__", no_names),
                        (b"__molt_vararg__", args),
                        (b"__molt_varkw__", kwargs),
                    ],
                    true,
                );
                // CALL cleanup is DECREF_INPUTS over the stack; CALL_FUNCTION_EX
                // releases its tuple and mapping. 3.14 reversed both.
                for (minor, stack, expanded) in [
                    (12, [0, 1, 2, 3], [1, 0, 2, 3]),
                    (13, [0, 1, 2, 3], [1, 0, 2, 3]),
                    (14, [3, 2, 1, 0], [2, 3, 1, 0]),
                ] {
                    with_target_minor(_py, minor, || {
                        for (form, expected) in [
                            (super::CallForm::Stack, stack),
                            (super::CallForm::Expanded, expanded),
                        ] {
                            let values = probes.instances(_py, 4);
                            let builder = last_owner_call(
                                _py,
                                form,
                                &values[..2],
                                &[(a, values[2]), (b, values[3])],
                            );
                            let result = super::molt_call_bind(variadic, builder);
                            assert!(obj_from_bits(result).is_none());
                            assert!(!crate::exception_pending(_py));
                            assert_eq!(
                                ReleaseProbes::released(&values),
                                expected,
                                "3.{minor} {form:?}"
                            );
                        }
                    });
                }
                for bits in [variadic, no_names, a, b, args, kwargs] {
                    dec_ref_bits(_py, bits);
                }
                probes.release(_py);
            }
        });
    }

    #[test]
    fn keyword_mapping_keeps_the_exception_a_name_callback_raised() {
        extern "C" fn raising_hash(_self_bits: u64) -> u64 {
            crate::with_gil_entry_nopanic!(_py, {
                crate::raise_exception::<u64>(_py, "ValueError", "keyword name hash")
            })
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let builtins = crate::builtin_classes(_py);
                let hash = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::provenance::abi::expose_function_address(raising_hash as *const ()),
                    1,
                );
                assert!(!hash.is_null());
                let hash_bits = MoltObject::from_ptr(hash).bits();
                let hash_name = MoltObject::from_ptr(crate::alloc_string(_py, b"__hash__")).bits();
                let namespace = crate::alloc_dict_with_pairs(_py, &[hash_name, hash_bits]);
                assert!(!namespace.is_null());
                let namespace_bits = MoltObject::from_ptr(namespace).bits();
                let class_name = MoltObject::from_ptr(crate::alloc_string(_py, b"Name")).bits();
                // `class Name(str): __hash__ = raising_hash`
                let class_bits = crate::builtins::types::molt_type_new(
                    builtins.type_obj,
                    class_name,
                    builtins.str,
                    namespace_bits,
                    MoltObject::none().bits(),
                );
                assert!(!obj_from_bits(class_bits).is_none() && !crate::exception_pending(_py));
                // A `Name("key")` instance: string storage of the subclass.
                let text = b"key";
                let key = crate::object::builders::alloc_native_inline_bytes(
                    _py,
                    class_bits,
                    crate::object::native_instance::NativePayload::String,
                    text,
                );
                assert!(!key.is_null());
                let key_bits = MoltObject::from_ptr(key).bits();
                let arguments = super::CallArguments::retained(
                    _py,
                    None,
                    &[],
                    &[key_bits],
                    &[MoltObject::from_int(1).bits()],
                )
                .unwrap();
                assert!(
                    arguments.validate_keywords(),
                    "a str subclass names a keyword"
                );
                // Building the fresh mapping hashes the name; its callback raises.
                let error = arguments.keyword_mapping().unwrap_err();
                assert!(obj_from_bits(error).is_none());
                let pending = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(
                    crate::builtins::exceptions::exception_matches_builtin_name(
                        _py,
                        pending,
                        "ValueError"
                    ),
                    "the callback's exception reaches the caller, not a MemoryError"
                );
                let _ = crate::molt_exception_clear();
                dec_ref_bits(_py, pending);
                drop(arguments);
                for bits in [
                    key_bits,
                    class_bits,
                    class_name,
                    namespace_bits,
                    hash_name,
                    hash_bits,
                ] {
                    dec_ref_bits(_py, bits);
                }
            }
        });
    }

    use super::inline_cache::{
        CALL_BIND_IC_KIND_DIRECT_FUNC, CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC,
        CALL_BIND_IC_KIND_TYPE_CALL, CallBindIcEntry, cached_attr_matches_bytes,
        clear_call_bind_ic_cache, ic_tls_insert, ic_tls_lookup, method_ic_call_plan,
        try_call_bind_ic_fast, type_epoch_matches, type_resolution_epoch_is_stable,
    };
    use super::trace_call_type_builder_enabled_raw;
    use crate::object::builders::{alloc_list, alloc_tuple};
    use crate::{
        TYPE_ID_OBJECT, dec_ref_bits, obj_from_bits, object_type_id, ptr_from_bits, runtime_state,
    };
    use molt_obj_model::MoltObject;

    extern "C" fn compiled_init_borrows_self_for_type_call_ic(self_bits: u64) -> i64 {
        crate::with_gil_entry_nopanic!(_py, {
            assert!(!obj_from_bits(self_bits).is_none());
            MoltObject::none().bits()
        }) as i64
    }

    extern "C" fn compiled_identity_returns_owned_arg(arg_bits: u64) -> i64 {
        crate::molt_inc_ref_obj(arg_bits);
        arg_bits as i64
    }

    #[test]
    fn cached_method_name_must_match_even_at_the_same_site() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let alpha_ptr = super::alloc_string(_py, b"alpha");
            assert!(!alpha_ptr.is_null());
            let alpha_bits = MoltObject::from_ptr(alpha_ptr).bits();
            assert!(unsafe { cached_attr_matches_bytes(alpha_bits, b"alpha") });
            assert!(
                !unsafe { cached_attr_matches_bytes(alpha_bits, b"beta") },
                "a same-site lookup for another name must miss instead of reusing the target"
            );
            dec_ref_bits(_py, alpha_bits);
        });
    }

    extern "C" fn compiled_second_arg_returns_owned_arg(_first_bits: u64, second_bits: u64) -> i64 {
        crate::molt_inc_ref_obj(second_bits);
        second_bits as i64
    }

    #[test]
    fn trace_call_type_builder_gate_requires_explicit_opt_in() {
        assert!(!trace_call_type_builder_enabled_raw(None));
        assert!(!trace_call_type_builder_enabled_raw(Some("0")));
        assert!(!trace_call_type_builder_enabled_raw(Some("true")));
        assert!(trace_call_type_builder_enabled_raw(Some("1")));
    }

    #[test]
    fn call_bind_builtin_full_binding_preserves_callee_owned_alias_return() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_identity_returns_owned_arg as *const (),
                ),
                1,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let list_ptr = alloc_list(_py, &[MoltObject::from_int(13).bits()]);
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();

            let builder_bits = super::molt_callargs_new(1, 0);
            assert!(!obj_from_bits(builder_bits).is_none());
            let _ = unsafe { super::molt_callargs_push_pos(builder_bits, list_bits) };

            dec_ref_bits(_py, list_bits);
            let result_bits = super::molt_call_bind(func_bits, builder_bits);
            assert_eq!(
                result_bits, list_bits,
                "identity callable must return the argument bits unchanged"
            );
            let result_ptr = obj_from_bits(result_bits).as_ptr().expect("live result");
            assert_eq!(result_ptr, list_ptr);
            let rc =
                unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() };
            assert_eq!(
                rc, 1,
                "CallArgs teardown must preserve the callee-owned return"
            );

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn call_bind_builtin_default_padded_argv_preserves_callee_owned_alias_return() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_second_arg_returns_owned_arg as *const (),
                ),
                2,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();

            let default_ptr = alloc_list(_py, &[MoltObject::from_int(17).bits()]);
            assert!(!default_ptr.is_null());
            let default_bits = MoltObject::from_ptr(default_ptr).bits();
            let defaults_ptr = alloc_tuple(_py, &[default_bits]);
            assert!(!defaults_ptr.is_null());
            let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
            let defaults_name = intern_metadata_name(_py, b"__defaults__");
            unsafe {
                assert!(crate::call::class_init::function_set_attr_bits(
                    _py,
                    func_ptr,
                    defaults_name,
                    defaults_bits,
                ));
            }
            dec_ref_bits(_py, defaults_bits);
            dec_ref_bits(_py, default_bits);

            let before_call =
                unsafe { (*crate::object::header_from_obj_ptr(default_ptr)).ref_count_snapshot() };
            assert_eq!(
                before_call, 1,
                "function __defaults__ tuple should be the only default owner before call"
            );

            let builder_bits = super::molt_callargs_new(1, 0);
            assert!(!obj_from_bits(builder_bits).is_none());
            let _ = unsafe {
                super::molt_callargs_push_pos(builder_bits, MoltObject::from_int(5).bits())
            };

            let result_bits = super::molt_call_bind(func_bits, builder_bits);
            assert_eq!(result_bits, default_bits);
            let result_ptr = obj_from_bits(result_bits)
                .as_ptr()
                .expect("live default result");
            assert_eq!(result_ptr, default_ptr);
            let after_call =
                unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() };
            assert_eq!(
                after_call, 2,
                "default cleanup must preserve the callee-owned return"
            );

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, func_bits);
        });
    }

    // ------------------------------------------------------------------
    // task #60: the constructor `__init__`-exception resolution invariant.
    //
    // `resolve_construct_after_init` is the single authority every construct
    // path (the IC fast path AND the full-binding `call_type_with_arguments`
    // ForwardArgs arm AND `call_class_init_with_args`) routes through after
    // `__init__` runs. The invariant: it consumes the owned init result and
    // returns the instance iff no exception is pending and the result was
    // `None`; otherwise it drops the instance and returns the `none` sentinel
    // so construct-site propagation guards fire.
    // ------------------------------------------------------------------

    #[test]
    fn resolve_construct_after_init_no_pending_returns_instance_unchanged() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let list_ptr = alloc_list(_py, &[MoltObject::from_int(7).bits()]);
            assert!(!list_ptr.is_null());
            let inst_bits = MoltObject::from_ptr(list_ptr).bits();
            let before =
                unsafe { (*crate::object::header_from_obj_ptr(list_ptr)).ref_count_snapshot() };
            assert_eq!(crate::molt_exception_pending(), 0, "no exception expected");
            // No pending exception: the owning reference is handed back as-is.
            let out = unsafe {
                crate::call::class_init::resolve_construct_after_init(
                    _py,
                    inst_bits,
                    MoltObject::none().bits(),
                )
            };
            assert_eq!(out, inst_bits, "must return the constructed instance");
            let after =
                unsafe { (*crate::object::header_from_obj_ptr(list_ptr)).ref_count_snapshot() };
            assert_eq!(after, before, "success path must not perturb the refcount");
            dec_ref_bits(_py, inst_bits);
        });
    }

    #[test]
    fn resolve_construct_after_init_pending_drops_instance_and_returns_none() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // Hold an extra owning reference so the helper's drop is observable
            // without freeing the object out from under the test.
            let list_ptr = alloc_list(_py, &[MoltObject::from_int(9).bits()]);
            assert!(!list_ptr.is_null());
            let inst_bits = MoltObject::from_ptr(list_ptr).bits();
            super::inc_ref_bits(_py, inst_bits);
            let before =
                unsafe { (*crate::object::header_from_obj_ptr(list_ptr)).ref_count_snapshot() };

            // Simulate `__init__` having raised: set a pending exception, then
            // resolve. The helper must drop the instance's owning reference and
            // surface the raise via a `none` result.
            let _: u64 = crate::builtins::exceptions::raise_exception(
                _py,
                "ValueError",
                "task60 init raise",
            );
            assert_eq!(
                crate::molt_exception_pending(),
                1,
                "exception must be pending"
            );

            let out = unsafe {
                crate::call::class_init::resolve_construct_after_init(
                    _py,
                    inst_bits,
                    MoltObject::none().bits(),
                )
            };
            assert!(
                MoltObject::from_bits(out).is_none(),
                "a pending __init__ exception must yield the None sentinel, not the instance"
            );
            assert_eq!(
                crate::molt_exception_pending(),
                1,
                "the helper must not clear the pending exception — the caller propagates it"
            );
            let after =
                unsafe { (*crate::object::header_from_obj_ptr(list_ptr)).ref_count_snapshot() };
            assert_eq!(
                after,
                before - 1,
                "the exception path must drop exactly one (the instance's) owning reference"
            );

            let _ = crate::molt_exception_clear();
            assert_eq!(crate::molt_exception_pending(), 0);
            // Release the extra reference taken above.
            dec_ref_bits(_py, inst_bits);
        });
    }

    #[test]
    fn resolve_construct_after_init_rejects_and_consumes_non_none_result() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let inst_ptr = alloc_list(_py, &[MoltObject::from_int(11).bits()]);
            let result_ptr = alloc_list(_py, &[MoltObject::from_int(13).bits()]);
            assert!(!inst_ptr.is_null());
            assert!(!result_ptr.is_null());
            let inst_bits = MoltObject::from_ptr(inst_ptr).bits();
            let result_bits = MoltObject::from_ptr(result_ptr).bits();
            super::inc_ref_bits(_py, inst_bits);
            super::inc_ref_bits(_py, result_bits);
            let inst_before =
                unsafe { (*crate::object::header_from_obj_ptr(inst_ptr)).ref_count_snapshot() };
            let result_before =
                unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() };

            let out = unsafe {
                crate::call::class_init::resolve_construct_after_init(_py, inst_bits, result_bits)
            };
            assert!(MoltObject::from_bits(out).is_none());
            assert_eq!(crate::molt_exception_pending(), 1);
            assert_eq!(
                unsafe { (*crate::object::header_from_obj_ptr(inst_ptr)).ref_count_snapshot() },
                inst_before - 1,
                "invalid __init__ return must consume the constructed instance"
            );
            assert_eq!(
                unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() },
                result_before - 1,
                "invalid __init__ return must consume its owned call result"
            );

            let _ = crate::molt_exception_clear();
            dec_ref_bits(_py, inst_bits);
            dec_ref_bits(_py, result_bits);
        });
    }

    #[test]
    fn type_call_ic_returns_single_owned_constructor_result_after_borrowed_init() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            clear_call_bind_ic_cache(_py);
            let init_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_init_borrows_self_for_type_call_ic as *const (),
                ),
                1,
            );
            assert!(!init_ptr.is_null());
            let init_bits = MoltObject::from_ptr(init_ptr).bits();
            let builtins = crate::builtins::classes::builtin_classes(_py);
            let name_ptr = super::alloc_string(_py, b"IcCtor");
            let init_name_ptr = super::alloc_string(_py, b"__init__");
            assert!(!name_ptr.is_null());
            assert!(!init_name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let init_name_bits = MoltObject::from_ptr(init_name_ptr).bits();
            let attrs = [init_name_bits, init_bits];
            let bases = [builtins.object];
            let class_bits = unsafe {
                crate::object::ops::molt_guarded_class_def(
                    name_bits,
                    crate::provenance::abi::expose_address(bases.as_ptr()),
                    bases.len() as u64,
                    crate::provenance::abi::expose_address(attrs.as_ptr()),
                    1,
                    std::mem::size_of::<u64>() as i64,
                    0,
                    1, // Install the supplied bases before inherited-hook dispatch.
                )
            };
            assert!(!obj_from_bits(class_bits).is_none());
            let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
            let layout_size = unsafe {
                crate::call::class_init::class_layout_size_cached(_py, class_ptr)
                    .expect("class layout must be representable")
            };
            let churn_name = super::alloc_string(_py, b"unrelated_attr");
            let churn_bits = MoltObject::from_ptr(churn_name).bits();
            assert_eq!(
                crate::molt_set_attr_name(class_bits, churn_bits, MoltObject::from_int(1).bits()),
                MoltObject::none().bits()
            );
            assert_eq!(
                unsafe { crate::call::class_init::class_layout_size_cached(_py, class_ptr) },
                Some(layout_size),
                "ordinary class attribute churn must not invalidate immutable payload size"
            );
            dec_ref_bits(_py, churn_bits);

            let layout_name = super::alloc_string(_py, b"__molt_layout_size__");
            let layout_name_bits = MoltObject::from_ptr(layout_name).bits();
            let _ = crate::molt_set_attr_name(
                class_bits,
                layout_name_bits,
                MoltObject::from_int(1).bits(),
            );
            assert_eq!(crate::molt_exception_pending(), 1);
            let _ = crate::molt_exception_clear();
            assert_eq!(
                unsafe { crate::call::class_init::class_layout_size_cached(_py, class_ptr) },
                Some(layout_size)
            );
            dec_ref_bits(_py, layout_name_bits);
            let entry = CallBindIcEntry {
                fn_ptr: crate::provenance::abi::expose_function_address(
                    compiled_init_borrows_self_for_type_call_ic as *const (),
                ),
                target_bits: init_bits,
                class_bits,
                class_version: unsafe { crate::class_layout_version_bits(class_ptr) },
                type_version: crate::global_type_version(),
                function_version: 0,
                cached_alloc_size: layout_size
                    .checked_add(std::mem::size_of::<crate::object::MoltHeader>())
                    .expect("class allocation size must be representable"),
                arity: 0,
                kind: CALL_BIND_IC_KIND_TYPE_CALL,
            };
            let mut arguments = super::CallArguments::retained(_py, None, &[], &[], &[])
                .expect("an empty argument vector");
            let result_bits = unsafe {
                try_call_bind_ic_fast(_py, entry, class_bits, &mut arguments)
                    .expect("type-call IC entry should apply")
            };
            let result_ptr = obj_from_bits(result_bits).as_ptr().expect("live instance");
            assert_eq!(unsafe { object_type_id(result_ptr) }, TYPE_ID_OBJECT);
            let ref_count =
                unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() };
            assert_eq!(
                ref_count, 1,
                "type-call IC must return exactly the constructor result owner; borrowed __init__ self must not leave a hidden retain"
            );
            dec_ref_bits(_py, result_bits);
            drop(arguments);
            dec_ref_bits(_py, init_name_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, init_bits);
        });
    }

    #[test]
    fn callargs_registries_are_runtime_scoped() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            {
                let mut guard = state.call_bind.lock().unwrap();
                guard.callargs_builder_map.clear();
                guard.callargs_storage_registry.clear();
            }

            let builder_bits = super::molt_callargs_new(1, 0);
            assert!(!obj_from_bits(builder_bits).is_none());
            let builder_ptr = ptr_from_bits(builder_bits);
            assert!(!builder_ptr.is_null());
            let args_ptr = unsafe { super::callargs_ptr(builder_ptr) };
            assert!(!args_ptr.is_null());
            {
                let guard = state.call_bind.lock().unwrap();
                assert_eq!(guard.callargs_builder_map.len(), 1);
                assert_eq!(guard.callargs_storage_registry.len(), 1);
                assert!(
                    guard
                        .callargs_builder_map
                        .contains_key(&(builder_ptr as usize))
                );
                assert!(
                    guard
                        .callargs_storage_registry
                        .contains(&(args_ptr as usize))
                );
            }

            dec_ref_bits(_py, builder_bits);
            {
                let guard = state.call_bind.lock().unwrap();
                assert!(guard.callargs_builder_map.is_empty());
                assert!(guard.callargs_storage_registry.is_empty());
            }
        });
    }

    #[test]
    fn clear_call_bind_ic_cache_clears_thread_local_cache() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let entry = CallBindIcEntry {
                fn_ptr: 11,
                target_bits: 22,
                class_bits: 0,
                class_version: 33,
                type_version: 0,
                function_version: 0,
                cached_alloc_size: 44,
                arity: 1,
                kind: CALL_BIND_IC_KIND_DIRECT_FUNC,
            };
            ic_tls_insert(_py, 99, entry);
            assert!(ic_tls_lookup(99).is_some());
            clear_call_bind_ic_cache(_py);
            assert!(ic_tls_lookup(99).is_none());
        });
    }

    #[test]
    fn mro_resolved_call_cache_owns_and_releases_target() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            clear_call_bind_ic_cache(_py);
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_init_borrows_self_for_type_call_ic as *const (),
                ),
                1,
            );
            assert!(!func_ptr.is_null());
            let target_bits = MoltObject::from_ptr(func_ptr).bits();
            let before = unsafe { (*crate::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
            let entry = CallBindIcEntry {
                fn_ptr: crate::provenance::abi::expose_function_address(
                    compiled_init_borrows_self_for_type_call_ic as *const (),
                ),
                target_bits,
                class_bits: 0,
                class_version: 0,
                type_version: crate::global_type_version(),
                function_version: 0,
                cached_alloc_size: 0,
                arity: 0,
                kind: CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC,
            };
            ic_tls_insert(_py, 101, entry);
            let retained = unsafe { (*crate::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
            assert_eq!(retained, before + 1);
            clear_call_bind_ic_cache(_py);
            let released = unsafe { (*crate::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
            assert_eq!(released, before);
            dec_ref_bits(_py, target_bits);
        });
    }

    #[test]
    fn public_gil_release_drains_foreign_thread_call_cache_owners() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert_eq!(crate::c_api::molt_init(), 0);
        let (target_bits, target_address, before) = crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_init_borrows_self_for_type_call_ic as *const (),
                ),
                1,
            );
            assert!(!func_ptr.is_null());
            let target_bits = MoltObject::from_ptr(func_ptr).bits();
            let before = unsafe { (*crate::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
            (target_bits, func_ptr as usize, before)
        });

        let worker = std::thread::spawn(move || {
            assert_eq!(crate::c_api::molt_gil_acquire(), 0);
            let retained = crate::with_gil_entry_nopanic!(_py, {
                ic_tls_insert(
                    _py,
                    0x4d4f_4c54,
                    CallBindIcEntry {
                        fn_ptr: crate::provenance::abi::expose_function_address(
                            compiled_init_borrows_self_for_type_call_ic as *const (),
                        ),
                        target_bits,
                        class_bits: 0,
                        class_version: 0,
                        type_version: crate::global_type_version(),
                        function_version: 0,
                        cached_alloc_size: 0,
                        arity: 0,
                        kind: CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC,
                    },
                );
                unsafe {
                    (*crate::header_from_obj_ptr(target_address as *mut u8)).ref_count_snapshot()
                }
            });
            assert_eq!(crate::c_api::molt_gil_release(), 0);
            let released = unsafe {
                (*crate::header_from_obj_ptr(target_address as *mut u8)).ref_count_snapshot()
            };
            (retained, released)
        });
        let (retained, released) = worker.join().expect("foreign thread must exit cleanly");
        assert_eq!(retained, before + 1, "thread-local IC must own its target");
        assert_eq!(
            released, before,
            "outermost public GIL release must drain foreign-thread IC owners before detach"
        );
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_bits(_py, target_bits);
        });
    }

    #[cfg(feature = "l7-attestation-probe")]
    #[test]
    fn direct_call_ic_hot_path_is_allocation_free_with_slow_path_control() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_identity_returns_owned_arg as *const (),
                ),
                1,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let mut args = super::CallArguments::retained(
                _py,
                None,
                &[MoltObject::from_int(17).bits()],
                &[],
                &[],
            )
            .expect("an argument vector");
            let entry = CallBindIcEntry {
                fn_ptr: crate::provenance::abi::expose_function_address(
                    compiled_identity_returns_owned_arg as *const (),
                ),
                target_bits: 0,
                class_bits: 0,
                class_version: 0,
                type_version: crate::global_type_version(),
                function_version: 0,
                cached_alloc_size: 0,
                arity: 1,
                kind: CALL_BIND_IC_KIND_DIRECT_FUNC,
            };
            for _ in 0..64 {
                assert_eq!(
                    unsafe { try_call_bind_ic_fast(_py, entry, func_bits, &mut args) },
                    Some(MoltObject::from_int(17).bits())
                );
            }

            crate::attestation_probe::reset();
            crate::attestation_probe::set_tracking(true);
            for _ in 0..10_000 {
                assert_eq!(
                    unsafe { try_call_bind_ic_fast(_py, entry, func_bits, &mut args) },
                    Some(MoltObject::from_int(17).bits())
                );
            }
            crate::attestation_probe::set_tracking(false);
            let fast = crate::attestation_probe::snapshot();
            assert_eq!(
                fast.allocations, 0,
                "direct IC hot path allocated: {fast:?}"
            );

            // Bypass the IC and exercise the production CallArgs/binder entry as
            // the observer control. This must register allocation traffic, or a
            // zero fast-path count would not be meaningful evidence.
            crate::attestation_probe::reset();
            crate::attestation_probe::set_tracking(true);
            for _ in 0..64 {
                let builder_bits = super::molt_callargs_new(1, 0);
                assert!(!obj_from_bits(builder_bits).is_none());
                assert_eq!(
                    unsafe {
                        super::molt_callargs_push_pos(builder_bits, MoltObject::from_int(17).bits())
                    },
                    MoltObject::none().bits()
                );
                assert_eq!(
                    super::molt_call_bind(func_bits, builder_bits),
                    MoltObject::from_int(17).bits()
                );
            }
            crate::attestation_probe::set_tracking(false);
            let slow = crate::attestation_probe::snapshot();
            assert!(
                slow.allocations > 0,
                "slow-path control observed no allocations: {slow:?}"
            );
            dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn type_epoch_invalidates_every_mro_resolved_call_cache_family() {
        let recorded = crate::global_type_version();
        assert!(type_epoch_matches(recorded));
        assert!(type_resolution_epoch_is_stable(recorded));
        crate::bump_type_version();
        assert!(!type_epoch_matches(recorded));
        assert!(!type_resolution_epoch_is_stable(recorded));
    }

    // ------------------------------------------------------------------
    // Fused method-call IC: call-plan classification + direct-vs-binder gate.
    //
    // `method_ic_call_plan` returns `(fixed_arity_including_self,
    // n_pos_defaults, needs_binder)`. The fused fast path's `direct_ok` gate is
    //   !needs_binder
    //     && fixed_arity <= DIRECT_ARGV_MAX
    //     && (fixed_arity - n_pos_defaults) <= supplied+1 <= fixed_arity
    // (the `+1` is `self`). When the gate is false the call routes to the
    // cached-bind path (full binder). The load-bearing distinction: POSITIONAL
    // defaults are direct-fillable (gate may be true), while kw-only / `*args` /
    // `**kwargs` set `needs_binder` (gate always false). These tests pin the
    // classifier + gate exhaustively over the reviewer-enumerated shapes:
    // no-default exact arity (direct), positional default (direct, paddable
    // range), kw-only ±default (binder), *args (binder), **kwargs (binder), and
    // an arity mismatch (binder).
    // ------------------------------------------------------------------

    /// Mirror of the production `direct_ok` closure in `call_method_ic_dispatch`
    /// (kept in lock-step). `supplied_pos` excludes `self`; the production
    /// `DIRECT_ARGV_MAX` is 16.
    fn direct_ok_gate(
        fixed_arity: u8,
        n_pos_defaults: u8,
        needs_binder: bool,
        supplied_pos: usize,
    ) -> bool {
        if needs_binder {
            return false;
        }
        let fixed_arity = fixed_arity as usize;
        if fixed_arity > 16 {
            return false;
        }
        let supplied = supplied_pos + 1;
        let min_supplied = fixed_arity.saturating_sub(n_pos_defaults as usize);
        supplied >= min_supplied && supplied <= fixed_arity
    }

    /// Build a `TYPE_ID_FUNCTION` with the given fixed arity (INCLUDING `self`)
    /// and optional binding metadata, then return its bits. The function never
    /// runs in these tests — only its shape metadata is read.
    unsafe fn make_test_function(
        _py: &crate::PyToken<'_>,
        arity_including_self: u64,
        meta: &[(&'static [u8], u64)],
    ) -> u64 {
        use crate::object::builders::alloc_function_obj;
        // fn_ptr is irrelevant for shape classification; use a dummy non-null.
        let func_ptr = alloc_function_obj(_py, 1, arity_including_self);
        assert!(!func_ptr.is_null());
        for (name, val_bits) in meta.iter().copied() {
            let attr_bits = intern_metadata_name(_py, name);
            unsafe {
                assert!(crate::call::class_init::function_set_attr_bits(
                    _py, func_ptr, attr_bits, val_bits
                ))
            };
        }
        MoltObject::from_ptr(func_ptr).bits()
    }

    /// Use the runtime's canonical attribute-name interning authority. Keeping
    /// another metadata-name allowlist here makes new binder fields fail in the
    /// fixture before they ever reach their production consumer.
    fn intern_metadata_name(_py: &crate::PyToken<'_>, name: &'static [u8]) -> u64 {
        crate::attr_name_bits_from_bytes(_py, name).expect("metadata name")
    }

    #[test]
    fn method_ic_plan_no_default_exact_arity_is_direct() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, x): ...  called as obj.m(arg)  -> direct
            let func_bits = unsafe { make_test_function(_py, 2, &[]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 2);
            assert_eq!(plan.n_pos_defaults, 0);
            assert!(!plan.needs_binder, "no metadata => no binder");
            assert!(
                direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "1 supplied + self == arity 2 -> direct"
            );
            crate::dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn specialized_builtin_binding_owns_raw_admission_with_or_without_trampolines() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                // Cover ordinary arguments, direct dictionary mutation, and
                // closure-owned exception init: all three execution strategies.
                for (symbol, arity) in [
                    (fn_addr!(crate::molt_object_init_subclass), 1),
                    (fn_addr!(crate::molt_object_init), 1),
                    (fn_addr!(crate::molt_object_new_bound), 1),
                    (fn_addr!(crate::molt_int_new), 3),
                    (fn_addr!(crate::dict_update_method), 2),
                    (
                        fn_addr!(crate::builtins::exceptions::molt_exception_init_owned),
                        4,
                    ),
                ] {
                    let ptr =
                        crate::builtins::functions::alloc_runtime_function_obj(py, symbol, arity);
                    assert!(!ptr.is_null());
                    let bits = MoltObject::from_ptr(ptr).bits();
                    let original = crate::function_trampoline_ptr(ptr);
                    for trampoline in [0, 1] {
                        // A non-callable trampoline is deliberate: missing
                        // receivers must be rejected before dispatch reaches it.
                        crate::object::layout::function_set_trampoline_ptr(ptr, trampoline);
                        assert!(super::builtin_args::builtin_call_binding(py, ptr).is_some());
                        assert!(super::function_raw_positional_call_needs_binding(
                            py, ptr, 0
                        ));
                        assert!(super::function_raw_positional_call_needs_binding(
                            py,
                            ptr,
                            arity as usize
                        ));
                        assert!(method_ic_call_plan(py, bits).unwrap().needs_binder);
                        let result = super::molt_call_bind(bits, super::molt_callargs_new(0, 0));
                        assert!(
                            crate::exception_pending(py),
                            "missing receiver cannot be synthesized"
                        );
                        crate::molt_exception_clear();
                        dec_ref_bits(py, result);
                        let result = crate::call::function::call_function_obj_vec(py, bits, &[]);
                        assert!(
                            crate::exception_pending(py),
                            "raw vector calls must use the same binder"
                        );
                        crate::molt_exception_clear();
                        dec_ref_bits(py, result);
                    }
                    crate::object::layout::function_set_trampoline_ptr(ptr, original);
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn method_ic_plan_positional_default_is_direct_over_paddable_range() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, x, bump=1): ...  -> direct (positional default), NOT
            // binder. __defaults__ = (1,) (a non-empty tuple).
            let one = MoltObject::from_int(1).bits();
            let defaults_ptr = crate::object::builders::alloc_tuple(_py, &[one]);
            let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 3, &[(b"__defaults__", defaults_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 3);
            assert_eq!(plan.n_pos_defaults, 1, "len(__defaults__) == 1");
            assert!(!plan.needs_binder, "positional default => NOT binder");
            // obj.m(x)        -> supplied 2, pad bump  -> direct
            assert!(
                direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "x supplied, bump padded -> direct"
            );
            // obj.m(x, bump)  -> supplied 3 == arity   -> direct (no pad)
            assert!(
                direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 2),
                "x+bump supplied -> direct"
            );
            // obj.m()         -> supplied 1 < min 2    -> binder (arity error)
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 0),
                "0 supplied (self only) below min -> binder"
            );
            // obj.m(a,b,c)    -> supplied 4 > arity 3  -> binder (arity error)
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 3),
                "too many positionals -> binder"
            );
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, defaults_bits);
        });
    }

    #[test]
    fn method_ic_plan_two_positional_defaults_widen_paddable_range() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, a, b, c=1, d=2): ...  -> arity 5, 2 defaults.
            let one = MoltObject::from_int(1).bits();
            let two = MoltObject::from_int(2).bits();
            let defaults_ptr = crate::object::builders::alloc_tuple(_py, &[one, two]);
            let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 5, &[(b"__defaults__", defaults_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 5);
            assert_eq!(plan.n_pos_defaults, 2);
            assert!(!plan.needs_binder);
            // min supplied = 5 - 2 = 3 (self,a,b); max = 5 (self,a,b,c,d).
            for supplied_pos in 2..=4usize {
                // supplied incl self = 3,4,5 -> all direct.
                assert!(
                    direct_ok_gate(
                        plan.fixed_arity,
                        plan.n_pos_defaults,
                        plan.needs_binder,
                        supplied_pos
                    ),
                    "supplied_pos={} should be direct",
                    supplied_pos
                );
            }
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "only a supplied (self,a=2) below min 3 -> binder"
            );
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 5),
                "6 incl self > arity 5 -> binder"
            );
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, defaults_bits);
        });
    }

    #[test]
    fn method_ic_plan_kwonly_with_default_needs_binder() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, x, *, ctx=None): ...  -> binder (kwonly name present).
            let name_ptr = crate::object::builders::alloc_string(_py, b"ctx");
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let kwonly_ptr = crate::object::builders::alloc_tuple(_py, &[name_bits]);
            let kwonly_bits = MoltObject::from_ptr(kwonly_ptr).bits();
            // kwdefaults present too (ctx=None), but the kwonly NAME alone forces
            // the binder.
            let func_bits =
                unsafe { make_test_function(_py, 2, &[(b"__molt_kwonly_names__", kwonly_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "kw-only param => binder");
            assert!(!direct_ok_gate(
                plan.fixed_arity,
                plan.n_pos_defaults,
                plan.needs_binder,
                1
            ));
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, kwonly_bits);
            crate::dec_ref_bits(_py, name_bits);
        });
    }

    #[test]
    fn method_ic_plan_kwonly_without_default_needs_binder() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, x, *, ctx): ...  (kwonly, no default) -> binder.
            // The kw-only NAME alone forces the binder; defaults are orthogonal.
            let name_ptr = crate::object::builders::alloc_string(_py, b"ctx");
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let kwonly_ptr = crate::object::builders::alloc_tuple(_py, &[name_bits]);
            let kwonly_bits = MoltObject::from_ptr(kwonly_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 2, &[(b"__molt_kwonly_names__", kwonly_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "kw-only param (no default) => binder");
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, kwonly_bits);
            crate::dec_ref_bits(_py, name_bits);
        });
    }

    #[test]
    fn method_ic_plan_kwdefaults_only_needs_binder() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // A non-empty __kwdefaults__ dict (kw-only defaults) forces the
            // binder even if the kwonly-names tuple was not explicitly recorded.
            let none_bits = MoltObject::none().bits();
            let key_ptr = crate::object::builders::alloc_string(_py, b"ctx");
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let dict_ptr =
                crate::object::builders::alloc_dict_with_pairs(_py, &[key_bits, none_bits]);
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 2, &[(b"__kwdefaults__", dict_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "non-empty __kwdefaults__ => binder");
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, dict_bits);
            crate::dec_ref_bits(_py, key_bits);
        });
    }

    #[test]
    fn method_ic_plan_varargs_needs_binder() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, *args): ...  -> binder (*args present).
            let star_ptr = crate::object::builders::alloc_string(_py, b"args");
            let star_bits = MoltObject::from_ptr(star_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 1, &[(b"__molt_vararg__", star_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "*args => binder");
            assert!(!direct_ok_gate(
                plan.fixed_arity,
                plan.n_pos_defaults,
                plan.needs_binder,
                3
            ));
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, star_bits);
        });
    }

    #[test]
    fn method_ic_plan_bind_kind_needs_binder() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let bind_kind_bits = MoltObject::from_int(crate::BIND_KIND_PACKED_BUILTIN).bits();
            let func_bits =
                unsafe { make_test_function(_py, 2, &[(b"__molt_bind_kind__", bind_kind_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "bind kind => binder");
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "bind-kind functions cannot use the direct positional path"
            );
            crate::dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn method_ic_plan_kwargs_needs_binder() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, **kwargs): ...  -> binder (**kwargs present).
            let kw_ptr = crate::object::builders::alloc_string(_py, b"kwargs");
            let kw_bits = MoltObject::from_ptr(kw_ptr).bits();
            let func_bits = unsafe { make_test_function(_py, 1, &[(b"__molt_varkw__", kw_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "**kwargs => binder");
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, kw_bits);
        });
    }

    #[test]
    fn method_ic_plan_arity_mismatch_blocks_direct_without_binder() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, a, b): ...  (no defaults). Direct only at exact arity.
            let func_bits = unsafe { make_test_function(_py, 3, &[]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 3);
            assert_eq!(plan.n_pos_defaults, 0);
            assert!(!plan.needs_binder);
            // No defaults => min == max == arity 3 (incl self).
            assert!(
                direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 2),
                "2 supplied + self == 3 OK"
            );
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "1 supplied + self < 3 -> binder"
            );
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 3),
                "3 supplied + self > 3 -> binder"
            );
            crate::dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn method_ic_plan_wide_arity_over_argv_max_blocks_direct() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // A method whose fixed arity exceeds DIRECT_ARGV_MAX (16) must take
            // the binder even with no binder-forcing features, since the direct
            // path's stack arg buffer cannot hold the call.
            let func_bits = unsafe { make_test_function(_py, 17, &[]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 17);
            assert!(!plan.needs_binder);
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 16),
                "arity 17 > DIRECT_ARGV_MAX -> binder"
            );
            crate::dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn method_ic_plan_non_function_classifies_none() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // A non-function callable bits value must not classify (the fast path
            // is function-only).
            let list_ptr = crate::object::builders::alloc_list(_py, &[]);
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            assert!(unsafe { method_ic_call_plan(_py, list_bits) }.is_none());
            crate::dec_ref_bits(_py, list_bits);
        });
    }
}
