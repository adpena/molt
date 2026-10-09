use super::*;
use crate::builtins::methods::{float_method_bits, int_method_bits};

#[derive(Clone, Copy)]
#[repr(u32)]
enum NumericMember {
    IntegerReal = 1,
    IntegerImag,
    IntegerDenominator,
    FloatReal,
    FloatImag,
    ComplexReal,
    ComplexImag,
}

/// Publish the numeric getsets/members through the ordinary descriptor owner.
/// CPython 3.12.13/3.13.11/3.14.3 long_getset, float_getset and complex_members
/// agree on these fields. Stage all three namespaces before publishing any.
pub(crate) fn numeric_publish_members(py: &PyToken<'_>) -> bool {
    use crate::builtins::types::{NativeDescriptorFlavor, NativeDescriptorSpec};
    use crate::object::builders::PtrDropGuard;
    unsafe {
        let getter = crate::builtins::methods::alloc_builtin_function(
            py,
            numeric_member_get as *const () as usize as u64,
            2,
        );
        let Some(getter_ptr) = obj_from_bits(getter).as_ptr() else {
            return false;
        };
        let _getter = PtrDropGuard::new(getter_ptr);
        let builtins = builtin_classes(py);
        let definitions: [(u64, NativeDescriptorFlavor, &[(&str, NumericMember, &str)]); 3] = [
            (
                builtins.int,
                NativeDescriptorFlavor::GetSet,
                &[
                    (
                        "real",
                        NumericMember::IntegerReal,
                        "the real part of a complex number",
                    ),
                    (
                        "imag",
                        NumericMember::IntegerImag,
                        "the imaginary part of a complex number",
                    ),
                    (
                        "numerator",
                        NumericMember::IntegerReal,
                        "the numerator of a rational number in lowest terms",
                    ),
                    (
                        "denominator",
                        NumericMember::IntegerDenominator,
                        "the denominator of a rational number in lowest terms",
                    ),
                ],
            ),
            (
                builtins.float,
                NativeDescriptorFlavor::GetSet,
                &[
                    (
                        "real",
                        NumericMember::FloatReal,
                        "the real part of a complex number",
                    ),
                    (
                        "imag",
                        NumericMember::FloatImag,
                        "the imaginary part of a complex number",
                    ),
                ],
            ),
            (
                builtins.complex,
                NativeDescriptorFlavor::Member,
                &[
                    (
                        "real",
                        NumericMember::ComplexReal,
                        "the real part of a complex number",
                    ),
                    (
                        "imag",
                        NumericMember::ComplexImag,
                        "the imaginary part of a complex number",
                    ),
                ],
            ),
        ];
        let mut owners: [Option<PtrDropGuard>; 3] = std::array::from_fn(|_| None);
        let mut publications = [(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ); 3];
        for (index, (owner, flavor, members)) in definitions.into_iter().enumerate() {
            let class = obj_from_bits(owner).as_ptr().unwrap();
            let dictionary = class_dict_bits(class);
            let live = obj_from_bits(dictionary).as_ptr().unwrap();
            let staged = crate::object::ops_dict::molt_dict_copy(dictionary);
            let Some(staged) = obj_from_bits(staged).as_ptr() else {
                return false;
            };
            owners[index] = Some(PtrDropGuard::new(staged));
            if exception_pending(py) {
                return false;
            }
            for &(name, operation, doc) in members {
                let Some(name) = attr_name_bits_from_bytes(py, name.as_bytes()) else {
                    return false;
                };
                let _name = PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
                let doc = alloc_string(py, doc.as_bytes());
                if doc.is_null() {
                    return false;
                }
                let _doc = PtrDropGuard::new(doc);
                let descriptor = crate::builtins::types::alloc_native_descriptor(
                    py,
                    NativeDescriptorSpec {
                        flavor,
                        operation: operation as u32,
                        owner,
                        name,
                        doc: MoltObject::from_ptr(doc).bits(),
                        getter,
                        setter: MoltObject::none().bits(),
                        deleter: MoltObject::none().bits(),
                    },
                );
                let Some(descriptor_ptr) = obj_from_bits(descriptor).as_ptr() else {
                    return false;
                };
                let _descriptor = PtrDropGuard::new(descriptor_ptr);
                if exception_pending(py) {
                    return false;
                }
                dict_set_in_place(py, staged, name, descriptor);
                if exception_pending(py) {
                    return false;
                }
            }
            publications[index] = (class, live, staged);
        }
        for &(_, live, staged) in &publications {
            crate::object::ops::dict_publish_staged(py, live, staged);
        }
        for &(class, _, _) in &publications {
            class_bump_layout_version(class);
        }
        true
    }
}

extern "C" fn numeric_member_get(descriptor: u64, receiver: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(descriptor) = obj_from_bits(descriptor).as_ptr() else {
            return raise_exception(py, "SystemError", "numeric descriptor has no payload");
        };
        // NativeDescriptor validates the real owner/subtype before dispatch.
        // Read sealed payloads directly: user __int__/__float__ are not part of
        // CPython's long_long/float_float getset implementation.
        let operation = unsafe { crate::object::layout::native_descriptor_operation(descriptor) };
        match operation {
            Some(tag) if tag == NumericMember::IntegerReal as u32 => {
                if type_of_bits(py, receiver) == builtin_classes(py).int {
                    inc_ref_bits(py, receiver);
                    return receiver;
                }
                match crate::builtins::numbers::index_bigint_integral_bits(receiver) {
                    Some(value) => crate::builtins::numbers::int_bits_from_bigint(py, value),
                    None => {
                        raise_exception(py, "SystemError", "int descriptor lacks integer storage")
                    }
                }
            }
            Some(tag) if tag == NumericMember::IntegerImag as u32 => MoltObject::from_int(0).bits(),
            Some(tag) if tag == NumericMember::IntegerDenominator as u32 => {
                MoltObject::from_int(1).bits()
            }
            Some(tag) if tag == NumericMember::FloatReal as u32 => {
                if type_of_bits(py, receiver) == builtin_classes(py).float {
                    inc_ref_bits(py, receiver);
                    return receiver;
                }
                match crate::object::ops::as_float_extended(obj_from_bits(receiver)) {
                    Some(value) => crate::object::ops::float_result_bits(py, value),
                    None => {
                        raise_exception(py, "SystemError", "float descriptor lacks float storage")
                    }
                }
            }
            Some(tag) if tag == NumericMember::FloatImag as u32 => {
                MoltObject::from_float(0.0).bits()
            }
            Some(tag)
                if tag == NumericMember::ComplexReal as u32
                    || tag == NumericMember::ComplexImag as u32 =>
            {
                let Some(pointer) = crate::builtins::numbers::complex_ptr_from_bits(receiver)
                else {
                    return raise_exception(
                        py,
                        "SystemError",
                        "complex descriptor lacks complex storage",
                    );
                };
                let value = unsafe { *crate::builtins::numbers::complex_ref(pointer) };
                crate::object::ops::float_result_bits(
                    py,
                    if tag == NumericMember::ComplexReal as u32 {
                        value.re
                    } else {
                        value.im
                    },
                )
            }
            _ => raise_exception(py, "SystemError", "invalid numeric descriptor operation"),
        }
    })
}

/// Which builtin numeric scalar class a receiver belongs to, for
/// [`resolve_scalar_method`].
///
/// `Bool` is distinct from `Int` only for class selection (`bool`'s own class vs
/// `int`'s); its methods come from the int method table because, per CPython,
/// `bool` inherits `int`.
#[derive(Clone, Copy)]
enum ScalarKind {
    Int,
    Bool,
    Float,
}

fn numeric_scalar_kind_from_bits(py: &PyToken<'_>, obj_bits: u64) -> Option<ScalarKind> {
    let obj = obj_from_bits(obj_bits);
    if obj.is_float() {
        return Some(ScalarKind::Float);
    }
    if obj.is_bool() {
        return Some(ScalarKind::Bool);
    }
    if obj.is_int() {
        return Some(ScalarKind::Int);
    }
    // Heap numeric storage also carries subclasses. Only canonical class
    // identity admits this builtin-only resolver; payload kind is not proof.
    let actual = type_of_bits(py, obj_bits);
    let builtins = builtin_classes(py);
    if actual == builtins.int {
        return Some(ScalarKind::Int);
    }
    if actual == builtins.float {
        return Some(ScalarKind::Float);
    }
    None
}

pub(crate) fn is_numeric_scalar_attr_receiver(py: &PyToken<'_>, obj_bits: u64) -> bool {
    numeric_scalar_kind_from_bits(py, obj_bits).is_some()
}

fn scalar_class_bits(_py: &PyToken<'_>, kind: ScalarKind) -> u64 {
    let builtins = builtin_classes(_py);
    match kind {
        ScalarKind::Int => builtins.int,
        ScalarKind::Bool => builtins.bool,
        ScalarKind::Float => builtins.float,
    }
}

/// Resolve `name` as a bound method on a numeric/bool scalar receiver.
///
/// This is the method half of the single numeric scalar attribute authority. The
/// receiver classifier in [`resolve_scalar_attr`] sends inline int/bool/float
/// and exact heap bigint/NaN-float through this same binder, so `getattr`,
/// `getattr(_, default)`, `hasattr`, and direct `object.__getattribute__` can
/// never disagree about which numeric methods a scalar exposes.
fn resolve_scalar_method(
    _py: &PyToken<'_>,
    self_bits: u64,
    kind: ScalarKind,
    name: &str,
) -> Option<u64> {
    let builtins = builtin_classes(_py);
    let class_bits = scalar_class_bits(_py, kind);
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    let direct = match kind {
        ScalarKind::Int | ScalarKind::Bool => int_method_bits(_py, name),
        ScalarKind::Float => float_method_bits(_py, name),
    };
    if let Some(func_bits) = direct {
        return unsafe {
            descriptor_bind(
                _py,
                func_bits,
                Some(MoltObject::from_ptr(class_ptr).bits()),
                Some(self_bits),
            )
        };
    }
    if let Some(func_bits) = builtin_class_method_bits(_py, class_bits, name) {
        return unsafe {
            descriptor_bind(
                _py,
                func_bits,
                Some(MoltObject::from_ptr(class_ptr).bits()),
                Some(self_bits),
            )
        };
    }
    if let Some(func_bits) = builtin_class_method_bits(_py, builtins.object, name) {
        return unsafe {
            descriptor_bind(
                _py,
                func_bits,
                Some(MoltObject::from_ptr(class_ptr).bits()),
                Some(self_bits),
            )
        };
    }
    None
}

/// Resolve `attr_name` on a scalar receiver.
///
/// Numeric scalars include inline int/bool/float plus exact heap bigint and
/// NaN-float. Heap subclasses use logical-class lookup. This is shared by every
/// numeric-scalar attribute path:
/// `molt_get_attr_name`, `molt_get_attr_name_default`, `molt_has_attr_name`,
/// `molt_get_attr_object`, `attr_lookup_ptr`, and `molt_object_getattribute`.
pub(crate) fn resolve_scalar_attr(
    _py: &PyToken<'_>,
    obj_bits: u64,
    attr_name: &str,
) -> Option<u64> {
    let class_bits = type_of_bits(_py, obj_bits);
    let name = attr_name_bits_from_bytes(_py, attr_name.as_bytes())?;
    let value = unsafe {
        obj_from_bits(class_bits)
            .as_ptr()
            .and_then(|class| class_attr_lookup_raw_mro(_py, class, name))
            .and_then(|descriptor| {
                descriptor_bind(_py, descriptor, Some(class_bits), Some(obj_bits))
            })
    };
    dec_ref_bits(_py, name);
    if value.is_some() || exception_pending(_py) {
        return value;
    }
    if let Some(kind) = numeric_scalar_kind_from_bits(_py, obj_bits) {
        return resolve_scalar_method(_py, obj_bits, kind, attr_name);
    }
    None
}

#[cfg(test)]
mod numeric_member_tests {
    use super::*;

    extern "C" fn forbidden_conversion(_receiver: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            raise_exception(py, "RuntimeError", "numeric member dispatched conversion")
        })
    }

    fn name(py: &PyToken<'_>, value: &str) -> u64 {
        attr_name_bits_from_bytes(py, value.as_bytes()).unwrap()
    }

    fn descriptor(py: &PyToken<'_>, class: u64, attr: &str) -> u64 {
        let attr = name(py, attr);
        let result =
            unsafe { class_attr_lookup_raw_mro(py, obj_from_bits(class).as_ptr().unwrap(), attr) }
                .unwrap();
        inc_ref_bits(py, result);
        dec_ref_bits(py, attr);
        result
    }

    fn subclass(py: &PyToken<'_>, base: u64, conversion: &str) -> u64 {
        let label = name(py, "NumericMemberChild");
        let class = crate::molt_class_new(label);
        dec_ref_bits(py, label);
        let result = crate::molt_class_set_base(class, base);
        dec_ref_bits(py, result);
        let method = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "numeric_member_forbidden_conversion",
                forbidden_conversion as *const (),
            ),
            1,
        );
        assert!(!method.is_null());
        let method = MoltObject::from_ptr(method).bits();
        let conversion = name(py, conversion);
        let result = crate::molt_set_attr_name(class, conversion, method);
        for bits in [result, conversion, method] {
            dec_ref_bits(py, bits);
        }
        unsafe {
            crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
                .unwrap();
        }
        assert!(!exception_pending(py));
        class
    }

    #[test]
    fn numeric_readonly_descriptors_share_scalar_subclass_and_direct_protocols() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let builtins = builtin_classes(py);
            let wide =
                crate::builtins::numbers::bigint_bits(py, num_bigint::BigInt::from(1u8) << 90);
            let nan =
                crate::object::ops::alloc_heap_float(py, f64::from_bits(0x7ff8_0000_0000_0042));
            for receiver in [MoltObject::from_int(3).bits(), wide, nan] {
                let result = resolve_scalar_attr(py, receiver, "real").unwrap();
                assert_eq!(result, receiver, "exact numeric real retains identity");
                dec_ref_bits(py, result);
            }
            for receiver in [
                MoltObject::from_bool(true).bits(),
                MoltObject::from_int(3).bits(),
                wide,
            ] {
                for (attr, expected) in [("imag", 0), ("denominator", 1)] {
                    let result = resolve_scalar_attr(py, receiver, attr).unwrap();
                    assert_eq!(obj_from_bits(result).as_int(), Some(expected));
                    assert_eq!(type_of_bits(py, result), builtins.int);
                    dec_ref_bits(py, result);
                }
            }
            let bool_real =
                resolve_scalar_attr(py, MoltObject::from_bool(true).bits(), "real").unwrap();
            assert_eq!(obj_from_bits(bool_real).as_int(), Some(1));
            dec_ref_bits(py, bool_real);
            for (base, conversion, value) in [
                (builtins.int, "__int__", MoltObject::from_int(37).bits()),
                (
                    builtins.float,
                    "__float__",
                    MoltObject::from_float(-0.0).bits(),
                ),
            ] {
                let class = subclass(py, base, conversion);
                let instance = unsafe { call_callable1(py, class, value) };
                assert!(!exception_pending(py));
                let member = descriptor(py, base, "real");
                let result = unsafe {
                    crate::builtins::types::native_descriptor_get(py, member, Some(instance), None)
                };
                assert!(
                    !exception_pending(py),
                    "read must bypass conversion callbacks"
                );
                assert_eq!(type_of_bits(py, result), base);
                if base == builtins.int {
                    assert_eq!(obj_from_bits(result).as_int(), Some(37));
                } else {
                    assert_eq!(
                        crate::object::ops::as_float_extended(obj_from_bits(result))
                            .unwrap()
                            .to_bits(),
                        (-0.0f64).to_bits()
                    );
                }
                dec_ref_bits(py, result);
                let attr = name(py, "real");
                let ordinary = crate::molt_get_attr_name(instance, attr);
                assert!(!exception_pending(py));
                assert_eq!(type_of_bits(py, ordinary), base);
                dec_ref_bits(py, ordinary);
                let shadow = MoltObject::from_int(91).bits();
                let changed = crate::molt_set_attr_name(class, attr, shadow);
                dec_ref_bits(py, changed);
                let ordinary = crate::molt_get_attr_name(instance, attr);
                assert_eq!(
                    ordinary, shadow,
                    "subclass declaration precedes inherited getset"
                );
                dec_ref_bits(py, ordinary);
                let direct = unsafe {
                    crate::builtins::types::native_descriptor_get(py, member, Some(instance), None)
                };
                assert_eq!(
                    type_of_bits(py, direct),
                    base,
                    "explicit base descriptor bypasses shadow"
                );
                dec_ref_bits(py, direct);
                for mutation in [Some(shadow), None] {
                    let result = unsafe {
                        crate::builtins::types::native_descriptor_mutate(
                            py, member, instance, mutation,
                        )
                    };
                    assert!(exception_pending(py));
                    crate::clear_exception(py);
                    dec_ref_bits(py, result);
                }
                let result = unsafe {
                    crate::builtins::types::native_descriptor_get(
                        py,
                        member,
                        Some(MoltObject::none().bits()),
                        None,
                    )
                };
                assert!(
                    exception_pending(py),
                    "wrong descriptor receiver must be rejected"
                );
                crate::clear_exception(py);
                dec_ref_bits(py, result);
                for bits in [attr, member, instance, class] {
                    dec_ref_bits(py, bits);
                }
            }
            let complex = crate::builtins::numbers::complex_bits(
                py,
                -0.0,
                f64::from_bits(0x7ff8_0000_0000_0042),
            );
            for (attr, expected) in [
                ("real", (-0.0f64).to_bits()),
                ("imag", 0x7ff8_0000_0000_0042),
            ] {
                let member = descriptor(py, builtins.complex, attr);
                let value = unsafe {
                    crate::builtins::types::native_descriptor_get(py, member, Some(complex), None)
                };
                assert_eq!(
                    crate::object::ops::as_float_extended(obj_from_bits(value))
                        .unwrap()
                        .to_bits(),
                    expected
                );
                assert_eq!(type_of_bits(py, value), builtins.float);
                dec_ref_bits(py, value);
                dec_ref_bits(py, member);
            }
            for bits in [complex, nan, wide] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }
}
