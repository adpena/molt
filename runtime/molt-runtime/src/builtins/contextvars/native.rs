//! Lazy native class and callable projections of the Context owner.
use super::*;
use crate::builtins::functions::native_callable::{NativeCallableKind as Kind, NativeCallableSpec};
use crate::builtins::types::native_descriptors::{
    NativeDescriptorFlavor, NativeDescriptorSpec, alloc_native_descriptor,
};
use crate::builtins::types::{
    ClassSemanticPolicy, RuntimeClassLayout, RuntimeClassMethodSpec as Method,
    RuntimeMethodSignature, SELF_RUNTIME_ARGUMENT_NAMES,
};
use crate::object::class_storage::ClassSlotPolicy;
use crate::object::seq_access::with_immutable_tuple_slice;
use crate::state::runtime_state::runtime_state;
use std::sync::atomic::AtomicU64;

fn text(py: &PyToken<'_>, s: &str) -> u64 {
    let p = crate::alloc_string(py, s.as_bytes());
    if p.is_null() {
        none()
    } else {
        MoltObject::from_ptr(p).bits()
    }
}
fn put(py: &PyToken<'_>, dict: *mut u8, name: &str, value: u64) -> bool {
    if exception_pending(py) {
        return false;
    }
    let key = text(py, name);
    if exception_pending(py) {
        return false;
    }
    unsafe {
        crate::dict_set_in_place(py, dict, key, value);
    }
    dec_ref_bits(py, key);
    !exception_pending(py)
}
fn descriptor(py: &PyToken<'_>, class: u64, dict: *mut u8, name: &str, operation: u32) -> bool {
    let key = text(py, name);
    if exception_pending(py) {
        return false;
    }
    let getter = crate::builtins::methods::alloc_builtin_function(
        py,
        fn_addr!(molt_contextvars_property),
        2,
    );
    if exception_pending(py) {
        dec_ref_bits(py, key);
        return false;
    }
    let desc = alloc_native_descriptor(
        py,
        NativeDescriptorSpec {
            flavor: if operation == 1 {
                NativeDescriptorFlavor::Member
            } else {
                NativeDescriptorFlavor::GetSet
            },
            operation,
            owner: class,
            name: key,
            doc: none(),
            getter,
            setter: none(),
            deleter: none(),
        },
    );
    dec_ref_bits(py, key);
    dec_ref_bits(py, getter);
    if exception_pending(py) {
        return false;
    }
    let ok = put(py, dict, name, desc);
    dec_ref_bits(py, desc);
    ok
}
fn class(
    py: &PyToken<'_>,
    slot: &AtomicU64,
    name: &str,
    size: usize,
    shape: ObjectShapeId,
    weakref: bool,
    methods: &[Method<'_>],
    properties: &[(&str, u32)],
    extra: impl FnOnce(u64, *mut u8) -> bool,
) -> u64 {
    if exception_pending(py) {
        return 0;
    }
    let cached = slot.load(std::sync::atomic::Ordering::Acquire);
    if cached != 0 {
        return cached;
    }
    let bits = crate::builtins::types::init_cached_runtime_class_configured(
        py,
        slot,
        name,
        RuntimeClassLayout {
            semantics: ClassSemanticPolicy::static_type(false),
            layout_size: size as i64,
            instance_shape: Some(shape),
            native_slots: Some(ClassSlotPolicy {
                allows_dict: false,
                allows_weakref: weakref,
                variable_sized: false,
            }),
        },
        |bits, dict| {
            if matches!(
                shape,
                ObjectShapeId::Context | ObjectShapeId::ContextVar | ObjectShapeId::ContextToken
            ) {
                let module = text(py, "_contextvars");
                let ok = put(py, dict, "__module__", module);
                dec_ref_bits(py, module);
                if !ok {
                    return false;
                }
            }
            if !crate::builtins::types::configure_runtime_class_methods(py, bits, dict, methods) {
                return false;
            }
            for &(name, op) in properties {
                if !descriptor(py, bits, dict, name, op) {
                    return false;
                }
            }
            extra(bits, dict)
        },
    );
    if bits != 0
        && matches!(
            shape,
            ObjectShapeId::Context | ObjectShapeId::ContextVar | ObjectShapeId::ContextToken
        )
        && !crate::cpython_abi_hooks::bind_context_class(py, bits, shape)
    {
        // A failed C projection must not leave a cached class that later calls
        // silently reuse without its canonical static view.
        let cached = slot.swap(0, std::sync::atomic::Ordering::AcqRel);
        debug_assert_eq!(cached, bits);
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, cached));
        return 0;
    }
    bits
}
fn variadic(name: &'static str, kind: Kind, addr: u64) -> Method<'static> {
    Method::with_signature(
        name,
        kind,
        addr,
        3,
        RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, true),
    )
}
pub(crate) fn context_class(py: &PyToken<'_>) -> u64 {
    let methods = [
        variadic("__new__", Kind::Constructor, fn_addr!(molt_contextvars_new)),
        variadic(
            "run",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_run),
        ),
        variadic(
            "get",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_get),
        ),
        Method::fixed(
            "copy",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_copy),
            1,
        ),
        Method::fixed(
            "__len__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_len),
            1,
        ),
        Method::fixed(
            "__getitem__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_getitem),
            2,
        ),
        Method::fixed(
            "__contains__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_contains),
            2,
        ),
        Method::fixed(
            "__iter__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_keys),
            1,
        ),
        Method::fixed(
            "keys",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_keys),
            1,
        ),
        Method::fixed(
            "values",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_values),
            1,
        ),
        Method::fixed(
            "items",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_items),
            1,
        ),
        Method::fixed(
            "__eq__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_eq),
            2,
        ),
        Method::fixed(
            "__ne__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_ne),
            2,
        ),
    ];
    class(
        py,
        &runtime_state(py).types.context_class,
        "Context",
        std::mem::size_of::<Context>(),
        ObjectShapeId::Context,
        true,
        &methods,
        &[],
        |_, dict| put(py, dict, "__hash__", none()),
    )
}
pub(crate) fn variable_class(py: &PyToken<'_>) -> u64 {
    let methods = [
        variadic(
            "__new__",
            Kind::Constructor,
            fn_addr!(molt_contextvars_var_new),
        ),
        variadic(
            "get",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_var_get),
        ),
        Method::fixed(
            "set",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_var_set),
            2,
        ),
        Method::fixed(
            "reset",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_var_reset),
            2,
        ),
        Method::fixed(
            "__hash__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_var_hash),
            1,
        ),
        Method::fixed(
            "__repr__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_var_repr),
            1,
        ),
        Method::fixed(
            "__class_getitem__",
            Kind::ClassMethodDescriptor,
            fn_addr!(crate::molt_generic_alias_new),
            2,
        ),
    ];
    class(
        py,
        &runtime_state(py).types.context_var_class,
        "ContextVar",
        std::mem::size_of::<Variable>(),
        ObjectShapeId::ContextVar,
        false,
        &methods,
        &[("name", 1)],
        |_, _| true,
    )
}
pub(crate) fn token_class(py: &PyToken<'_>) -> u64 {
    let methods = [
        variadic(
            "__new__",
            Kind::Constructor,
            fn_addr!(molt_contextvars_token_new),
        ),
        Method::fixed(
            "__repr__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_token_repr),
            1,
        ),
        Method::fixed(
            "__class_getitem__",
            Kind::ClassMethodDescriptor,
            fn_addr!(crate::molt_generic_alias_new),
            2,
        ),
        Method::fixed(
            "__enter__",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_token_enter),
            1,
        ),
        Method::fixed(
            "__exit__",
            Kind::MethodDescriptor,
            fn_addr!(molt_contextvars_token_exit),
            4,
        ),
    ];
    let count = if crate::object::ops_sys::runtime_target_minor(py) >= 14 {
        5
    } else {
        3
    };
    class(
        py,
        &runtime_state(py).types.context_token_class,
        "Token",
        std::mem::size_of::<Token>(),
        ObjectShapeId::ContextToken,
        false,
        &methods[..count],
        &[("var", 2), ("old_value", 3)],
        |_, dict| {
            let missing = missing(py);
            if exception_pending(py) {
                return false;
            }
            put(py, dict, "MISSING", missing) && put(py, dict, "__hash__", none())
        },
    )
}
fn missing(py: &PyToken<'_>) -> u64 {
    let state = &runtime_state(py).types;
    crate::object::init_atomic_bits(py, &state.context_missing, || {
        let methods = [
            variadic(
                "__new__",
                Kind::Constructor,
                fn_addr!(molt_contextvars_missing_new),
            ),
            Method::fixed(
                "__repr__",
                Kind::WrapperDescriptor,
                fn_addr!(molt_contextvars_missing_repr),
                1,
            ),
        ];
        let cls = class(
            py,
            &state.context_missing_class,
            "Token.MISSING",
            0,
            ObjectShapeId::Plain,
            false,
            &methods,
            &[],
            |_, _| true,
        );
        alloc(py, cls, ()).unwrap_or(0)
    })
}
fn iterator_class(py: &PyToken<'_>, mode: u64) -> u64 {
    let state = &runtime_state(py).types;
    let (slot, name) = match mode {
        0 => (&state.context_keys_iterator_class, "keys"),
        1 => (&state.context_values_iterator_class, "values"),
        _ => (&state.context_items_iterator_class, "items"),
    };
    let methods = [
        variadic(
            "__new__",
            Kind::Constructor,
            match mode {
                0 => fn_addr!(molt_contextvars_keys_new),
                1 => fn_addr!(molt_contextvars_values_new),
                _ => fn_addr!(molt_contextvars_items_new),
            },
        ),
        Method::fixed(
            "__iter__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_iter_self),
            1,
        ),
        Method::fixed(
            "__next__",
            Kind::WrapperDescriptor,
            fn_addr!(molt_contextvars_iter_next),
            1,
        ),
    ];
    class(
        py,
        slot,
        &format!("{name}_iterator"),
        std::mem::size_of::<ContextIterator>(),
        ObjectShapeId::ContextIterator,
        false,
        &methods,
        &[],
        |_, _| true,
    )
}
fn positional(
    py: &PyToken<'_>,
    args: u64,
    kwargs: u64,
    min: usize,
    max: usize,
    label: &str,
) -> Option<([u64; 2], usize)> {
    let p = obj_from_bits(args).as_ptr()?;
    if unsafe { object_type_id(p) } != crate::TYPE_ID_TUPLE {
        raise_exception::<()>(py, "TypeError", "expected argument tuple");
        return None;
    }
    let out = unsafe {
        with_immutable_tuple_slice(p, |a| {
            let mut out = [none(); 2];
            for (s, v) in out.iter_mut().zip(a) {
                *s = *v;
            }
            (out, a.len())
        })
    }?;
    if out.1 < min || out.1 > max {
        raise_exception::<()>(
            py,
            "TypeError",
            &format!("{label}() takes {min} to {max} positional arguments"),
        );
        return None;
    }
    if !obj_from_bits(kwargs).is_none()
        && obj_from_bits(kwargs).as_ptr().is_none_or(|p| unsafe {
            object_type_id(p) != crate::TYPE_ID_DICT || crate::dict_len(p) != 0
        })
    {
        raise_exception::<()>(
            py,
            "TypeError",
            &format!("{label}() takes no keyword arguments"),
        );
        return None;
    }
    Some(out)
}
fn lookup_error(py: &PyToken<'_>, var: u64) -> u64 {
    let args = crate::alloc_tuple(py, &[var]);
    if args.is_null() {
        return none();
    }
    let args = MoltObject::from_ptr(args).bits();
    let class = crate::exception_type_bits_from_name(py, "LookupError");
    let p = crate::alloc_exception_from_class_bits(py, class, args);
    dec_ref_bits(py, args);
    if !p.is_null() {
        crate::builtins::exceptions::record_exception_owned(py, p);
    }
    none()
}
fn copy_callable(py: &PyToken<'_>, module: u64) -> u64 {
    let bits = crate::builtins::methods::alloc_builtin_function(
        py,
        fn_addr!(molt_contextvars_copy_current),
        0,
    );
    let Some(pointer) = obj_from_bits(bits).as_ptr() else {
        return 0;
    };
    let mut pin = PtrDropGuard::preserving(pointer);
    let module_name = text(py, "_contextvars");
    let spec = NativeCallableSpec {
        name: Some("copy_context"),
        self_bits: Some(module),
        text_signature: Some("($module, /)"),
        ..NativeCallableSpec::uncached_function()
    };
    let ok = !exception_pending(py)
        && unsafe {
            crate::builtins::functions::native_callable::configure_native_callable(
                py, pointer, spec,
            ) && crate::call::class_init::function_set_attr_name(
                py,
                pointer,
                b"__module__",
                module_name,
            )
        };
    dec_ref_bits(py, module_name);
    if !ok {
        return 0;
    }
    pin.release();
    bits
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_types(module: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if obj_from_bits(module)
            .as_ptr()
            .is_none_or(|p| unsafe { object_type_id(p) != crate::TYPE_ID_MODULE })
        {
            return raise_exception::<u64>(
                py,
                "TypeError",
                "Context namespace requires its module owner",
            );
        }
        let classes = [context_class(py), variable_class(py), token_class(py)];
        if exception_pending(py) {
            return none();
        }
        let copy = copy_callable(py, module);
        if copy == 0 {
            return none();
        }
        let _pin = PtrDropGuard::preserving(obj_from_bits(copy).as_ptr().unwrap());
        let p = crate::alloc_tuple(py, &[classes[0], classes[1], classes[2], copy]);
        if p.is_null() {
            none()
        } else {
            MoltObject::from_ptr(p).bits()
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_copy_current() -> u64 {
    crate::with_gil_entry_nopanic!(py, { copy_current(py).unwrap_or_else(none) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_new(class: u64, args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let declaring = context_class(py);
        if declaring == 0
            || crate::builtins::type_ops::native_constructor_receiver(
                py,
                declaring,
                Some(class),
                "Context",
            )
            .is_none()
        {
            return none();
        }
        if positional(py, args, kwargs, 0, 0, "Context").is_none() {
            return none();
        }
        new_context(py).unwrap_or_else(none)
    })
}
fn unconstructible(py: &PyToken<'_>, declaring: u64, receiver: u64, label: &str) -> u64 {
    if declaring == 0
        || crate::builtins::type_ops::native_constructor_receiver(
            py,
            declaring,
            Some(receiver),
            label,
        )
        .is_none()
    {
        return none();
    }
    raise_exception::<u64>(
        py,
        "RuntimeError",
        "Instances of this type cannot be created",
    )
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_token_new(class: u64, _args: u64, _kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { unconstructible(py, token_class(py), class, "Token") })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_missing_new(class: u64, _args: u64, _kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _ = missing(py);
        let declaring = runtime_state(py)
            .types
            .context_missing_class
            .load(std::sync::atomic::Ordering::Acquire);
        unconstructible(py, declaring, class, "Token.MISSING")
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_keys_new(class: u64, _args: u64, _kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unconstructible(py, iterator_class(py, 0), class, "keys")
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_values_new(class: u64, _args: u64, _kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unconstructible(py, iterator_class(py, 1), class, "values")
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_items_new(class: u64, _args: u64, _kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unconstructible(py, iterator_class(py, 2), class, "items")
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_var_new(class: u64, args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let declaring = variable_class(py);
        if declaring == 0
            || crate::builtins::type_ops::native_constructor_receiver(
                py,
                declaring,
                Some(class),
                "ContextVar",
            )
            .is_none()
        {
            return none();
        }
        let Some(args) = crate::builtins::native_arguments::NativeArguments::read(
            py,
            "ContextVar",
            args,
            kwargs,
        ) else {
            return none();
        };
        if args.positional.len() > 1 {
            return raise_exception::<u64>(
                py,
                "TypeError",
                "ContextVar() takes at most 1 positional argument",
            );
        }
        let Some(bound) = crate::builtins::native_arguments::bind_named(
            py,
            "ContextVar",
            &args.positional,
            args.keyword_view(),
            ["", "default"],
            1,
        ) else {
            return none();
        };
        new_variable(py, bound[0].unwrap(), bound[1]).unwrap_or_else(none)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_var_get(var: u64, args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some((a, n)) = positional(py, args, kwargs, 0, 1, "get") else {
            return none();
        };
        match get_variable(py, var, (n != 0).then_some(a[0])) {
            Some(Some(value)) => value,
            Some(None) => lookup_error(py, var),
            None => none(),
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_var_set(var: u64, value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_variable(py, var, value).unwrap_or_else(none) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_var_reset(var: u64, token: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        reset_variable(py, var, token);
        none()
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_var_hash(var: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if checked(py, var, ObjectShapeId::ContextVar, "ContextVar").is_none() {
            return none();
        }
        crate::builtins::numbers::int_bits_from_i64(py, unsafe { variable(var).hash })
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_copy(ctx: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { copy_context(py, ctx).unwrap_or_else(none) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_len(ctx: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if checked(py, ctx, ObjectShapeId::Context, "Context").is_none() {
            return none();
        }
        crate::builtins::numbers::int_bits_from_i64(py, unsafe { context(ctx).len } as i64)
    })
}
fn mapping_value(py: &PyToken<'_>, ctx: u64, var: u64) -> Option<Option<u64>> {
    checked(py, ctx, ObjectShapeId::Context, "Context")?;
    checked(py, var, ObjectShapeId::ContextVar, "ContextVar")?;
    Some(trie::lookup(
        unsafe { context(ctx).root },
        var,
        var_hash(var),
    ))
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_getitem(ctx: u64, var: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        match mapping_value(py, ctx, var) {
            Some(Some(v)) => owned(py, v),
            Some(None) => crate::builtins::exceptions::raise_key_error_with_key(py, var),
            None => none(),
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_contains(ctx: u64, var: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        mapping_value(py, ctx, var).map_or_else(none, |v| MoltObject::from_bool(v.is_some()).bits())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_get(ctx: u64, args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some((a, _)) = positional(py, args, kwargs, 1, 2, "get") else {
            return none();
        };
        mapping_value(py, ctx, a[0]).map_or_else(none, |v| owned(py, v.unwrap_or(a[1])))
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_run(ctx: u64, args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(args_ptr) = obj_from_bits(args).as_ptr() else {
            return none();
        };
        let Some(args) = (unsafe { crate::object::seq_access::pin_tuple(py, args_ptr) }) else {
            return none();
        };
        let Some((&function, rest)) = args.split_first() else {
            return raise_exception::<u64>(
                py,
                "TypeError",
                "run() missing required positional argument",
            );
        };
        let Some(_lease) = EnteredContext::enter(py, ctx) else {
            return none();
        };
        let builder = crate::molt_callargs_new(0, 0);
        let Some(p) = obj_from_bits(builder).as_ptr() else {
            return none();
        };
        let mut pin = PtrDropGuard::preserving(p);
        for &value in rest {
            let result = unsafe { crate::molt_callargs_push_pos(builder, value) };
            dec_ref_bits(py, result);
            if exception_pending(py) {
                return none();
            }
        }
        if !obj_from_bits(kwargs).is_none() {
            let result = unsafe { crate::molt_callargs_expand_kwstar(builder, kwargs) };
            dec_ref_bits(py, result);
            if exception_pending(py) {
                return none();
            }
        }
        // call_bind consumes the builder, preserving the ordinary call argument
        // custody authority and native/WASM dispatch.
        pin.release();
        crate::molt_call_bind(function, builder)
    })
}
fn iter(py: &PyToken<'_>, ctx: u64, mode: u64) -> u64 {
    if checked(py, ctx, ObjectShapeId::Context, "Context").is_none() {
        return none();
    }
    let root = owned(py, unsafe { context(ctx).root });
    let _pin = (root != 0).then(|| PtrDropGuard::preserving(obj_from_bits(root).as_ptr().unwrap()));
    let Some(bits) = alloc(
        py,
        iterator_class(py, mode),
        ContextIterator {
            root: 0,
            mode,
            cursor: trie::Cursor::new(root),
        },
    ) else {
        return none();
    };
    unsafe {
        (*obj_from_bits(bits)
            .as_ptr()
            .unwrap()
            .cast::<ContextIterator>())
        .root = owned(py, root);
    }
    bits
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_keys(ctx: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { iter(py, ctx, 0) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_values(ctx: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { iter(py, ctx, 1) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_items(ctx: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { iter(py, ctx, 2) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_iter_self(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if checked(py, bits, ObjectShapeId::ContextIterator, "Context iterator").is_none() {
            return none();
        }
        owned(py, bits)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_iter_next(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(p) = checked(py, bits, ObjectShapeId::ContextIterator, "Context iterator") else {
            return none();
        };
        let it = unsafe { &mut *p.cast::<ContextIterator>() };
        let Some((key, value)) = it.cursor.next() else {
            return raise_exception::<u64>(py, "StopIteration", "");
        };
        match it.mode {
            0 => owned(py, key),
            1 => owned(py, value),
            _ => {
                let p = crate::alloc_tuple(py, &[key, value]);
                if p.is_null() {
                    none()
                } else {
                    MoltObject::from_ptr(p).bits()
                }
            }
        }
    })
}
fn equal(py: &PyToken<'_>, left: u64, right: u64, invert: bool) -> u64 {
    if !is_context(left) || !is_context(right) {
        return crate::builtins::methods::not_implemented_bits(py);
    }
    let a = owned(py, unsafe { context(left).root });
    let b = owned(py, unsafe { context(right).root });
    let _ap = (a != 0).then(|| PtrDropGuard::preserving(obj_from_bits(a).as_ptr().unwrap()));
    let _bp = (b != 0).then(|| PtrDropGuard::preserving(obj_from_bits(b).as_ptr().unwrap()));
    let mut result = unsafe { context(left).len == context(right).len };
    if result && a != b {
        let mut cursor = trie::Cursor::new(a);
        while let Some((key, value)) = cursor.next() {
            let Some(other) = trie::lookup(b, key, var_hash(key)) else {
                result = false;
                break;
            };
            match crate::object::ops_compare::compare_object_eq_bool(
                py,
                obj_from_bits(value),
                obj_from_bits(other),
            ) {
                crate::object::ops_compare::CompareBoolOutcome::True => {}
                crate::object::ops_compare::CompareBoolOutcome::Error => return none(),
                _ => {
                    result = false;
                    break;
                }
            }
        }
    }
    MoltObject::from_bool(result ^ invert).bits()
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_eq(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { equal(py, a, b, false) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_ne(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { equal(py, a, b, true) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_property(descriptor: u64, bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let p = obj_from_bits(descriptor).as_ptr().unwrap();
        let op = unsafe { crate::object::layout::native_descriptor_operation(p) }.unwrap();
        if op == 1 {
            if checked(py, bits, ObjectShapeId::ContextVar, "ContextVar").is_none() {
                return none();
            }
            return owned(py, unsafe { variable(bits).name });
        }
        let Some(p) = checked(py, bits, ObjectShapeId::ContextToken, "Token") else {
            return none();
        };
        let t = unsafe { &*p.cast::<Token>() };
        if op == 2 {
            owned(py, t.variable)
        } else if t.flags & HAD_OLD != 0 {
            owned(py, t.old)
        } else {
            owned(py, missing(py))
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_token_enter(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(p) = checked(py, bits, ObjectShapeId::ContextToken, "Token") else {
            return none();
        };
        let _ = p;
        owned(py, bits)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_token_exit(bits: u64, _ty: u64, _value: u64, _tb: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(p) = checked(py, bits, ObjectShapeId::ContextToken, "Token") else {
            return none();
        };
        reset_variable(py, unsafe { (*p.cast::<Token>()).variable }, bits);
        none()
    })
}
fn repr(py: &PyToken<'_>, value: u64) -> Option<String> {
    let bits = crate::molt_repr_from_obj(value);
    if exception_pending(py) {
        return None;
    }
    let out = crate::object::ops_format::string_obj_to_owned(obj_from_bits(bits));
    dec_ref_bits(py, bits);
    out
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_var_repr(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if checked(py, bits, ObjectShapeId::ContextVar, "ContextVar").is_none() {
            return none();
        }
        let v = unsafe { variable(bits) };
        let Some(name) = repr(py, v.name) else {
            return none();
        };
        let default = if v.has_default != 0 {
            let Some(s) = repr(py, v.default) else {
                return none();
            };
            format!(" default={s}")
        } else {
            String::new()
        };
        text(
            py,
            &format!(
                "<ContextVar name={name}{default} at 0x{:x}>",
                obj_from_bits(bits).as_ptr().unwrap() as usize
            ),
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_token_repr(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(p) = checked(py, bits, ObjectShapeId::ContextToken, "Token") else {
            return none();
        };
        let t = unsafe { &*p.cast::<Token>() };
        let used = if t.flags & USED != 0 { "used " } else { "" };
        let Some(var) = repr(py, t.variable) else {
            return none();
        };
        text(
            py,
            &format!("<Token {used}var={var} at 0x{:x}>", p as usize),
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_missing_repr(_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { text(py, "<Token.MISSING>") })
}
