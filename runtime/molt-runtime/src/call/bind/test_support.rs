//! Shared call-binding test fixtures.

pub(super) static RELEASED: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

pub(super) extern "C" fn record_release(self_bits: u64) -> u64 {
    RELEASED.lock().unwrap().push(self_bits);
    MoltObject::none().bits()
}

/// Instances whose finalizer records them, so a test observes the order in
/// which a call ends its last references (CPython's `__del__` oracle).
pub(super) struct ReleaseProbes {
    class_bits: u64,
    finalizer_bits: u64,
}

impl ReleaseProbes {
    pub(super) unsafe fn new(py: &crate::PyToken<'_>) -> Self {
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
    pub(super) unsafe fn instances(&self, py: &crate::PyToken<'_>, count: usize) -> Vec<u64> {
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
    pub(super) fn released(probes: &[u64]) -> Vec<usize> {
        RELEASED
            .lock()
            .unwrap()
            .iter()
            .filter_map(|bits| probes.iter().position(|probe| probe == bits))
            .collect()
    }

    pub(super) fn release(self, py: &crate::PyToken<'_>) {
        dec_ref_bits(py, self.class_bits);
        dec_ref_bits(py, self.finalizer_bits);
    }
}

/// Run `body` with the runtime targeting CPython 3.`minor` through the
/// canonical target-version authority, then restore the previous target.
pub(super) fn with_target_minor<R>(
    py: &crate::PyToken<'_>,
    minor: i64,
    body: impl FnOnce() -> R,
) -> R {
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
pub(super) unsafe fn metadata_function(
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
pub(super) unsafe fn last_owner_call(
    py: &crate::PyToken<'_>,
    form: super::CallForm,
    positional: &[u64],
    keywords: &[(u64, u64)],
) -> u64 {
    let (positional_count, keyword_count) = (positional.len() as u64, keywords.len() as u64);
    let builder = match form {
        super::CallForm::Stack => super::molt_callargs_new(positional_count, keyword_count),
        super::CallForm::Expanded => {
            super::arguments::molt_callargs_new_expanded(positional_count, keyword_count)
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

pub(super) extern "C" fn none_of_four(_a: u64, _b: u64, _c: u64, _d: u64) -> i64 {
    MoltObject::none().bits() as i64
}

pub(super) extern "C" fn none_of_two(_a: u64, _b: u64) -> i64 {
    MoltObject::none().bits() as i64
}

pub(super) extern "C" fn none_of_one(_a: u64) -> i64 {
    MoltObject::none().bits() as i64
}

pub(super) extern "C" fn none_of_none() -> i64 {
    MoltObject::none().bits() as i64
}

use crate::{dec_ref_bits, obj_from_bits, runtime_state};
use molt_obj_model::MoltObject;

pub(super) extern "C" fn compiled_init_borrows_self_for_type_call_ic(self_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        assert!(!obj_from_bits(self_bits).is_none());
        MoltObject::none().bits()
    }) as i64
}

pub(super) extern "C" fn compiled_identity_returns_owned_arg(arg_bits: u64) -> i64 {
    crate::molt_inc_ref_obj(arg_bits);
    arg_bits as i64
}

pub(super) extern "C" fn compiled_second_arg_returns_owned_arg(
    _first_bits: u64,
    second_bits: u64,
) -> i64 {
    crate::molt_inc_ref_obj(second_bits);
    second_bits as i64
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
pub(super) fn direct_ok_gate(
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
pub(super) unsafe fn make_test_function(
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
pub(super) fn intern_metadata_name(_py: &crate::PyToken<'_>, name: &'static [u8]) -> u64 {
    crate::attr_name_bits_from_bytes(_py, name).expect("metadata name")
}
