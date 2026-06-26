use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_super_new(type_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_obj = obj_from_bits(type_bits);
        let Some(type_ptr) = type_obj.as_ptr() else {
            let got = type_name(_py, type_obj);
            let msg = format!("super() argument 1 must be a type, not {got}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(type_ptr) != TYPE_ID_TYPE {
                let got = type_name(_py, type_obj);
                let msg = format!("super() argument 1 must be a type, not {got}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let obj = obj_from_bits(obj_bits);
        // CPython allows `super(type)` and `super(type, None)` as the "unbound" form.
        if obj.is_none() || obj_bits == 0 {
            let ptr = alloc_super_obj(_py, type_bits, MoltObject::none().bits());
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        }
        let obj_is_type = if let Some(obj_ptr) = obj.as_ptr() {
            unsafe { object_type_id(obj_ptr) == TYPE_ID_TYPE }
        } else {
            false
        };
        let is_instance = isinstance_runtime(_py, obj_bits, type_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let is_subtype = obj_is_type && issubclass_bits(obj_bits, type_bits);
        if !(is_instance || is_subtype) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "super(type, obj): obj must be an instance or subtype of type",
            );
        }
        let ptr = alloc_super_obj(_py, type_bits, obj_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_classmethod_new(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_classmethod_obj(_py, func_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bootstrap_descriptor_types() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                builtins.classmethod,
                builtins.staticmethod,
                builtins.property,
            ],
        );
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generic_alias_new(origin_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args_obj = obj_from_bits(args_bits);
        // Always create a fresh heap-allocated args tuple.  This is
        // necessary because the incoming tuple may be stack-allocated
        // (from the Cranelift stack-tuple optimisation) and would become
        // a dangling pointer once the caller's stack frame is unwound.
        // Copying the elements into a new heap tuple is cheap and safe.
        let args_tuple_bits = if let Some(args_ptr) = args_obj.as_ptr() {
            unsafe {
                if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(args_ptr);
                    let new_ptr = alloc_tuple(_py, elems);
                    if new_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(new_ptr).bits()
                } else {
                    let tuple_ptr = alloc_tuple(_py, &[args_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            }
        } else {
            let tuple_ptr = alloc_tuple(_py, &[args_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        };
        let ptr = alloc_generic_alias(_py, origin_bits, args_tuple_bits);
        // The new tuple was created above; dec_ref since alloc_generic_alias
        // inc_refs it.
        dec_ref_bits(_py, args_tuple_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generic_alias_mro_entries(alias_bits: u64, _bases_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(alias_ptr) = obj_from_bits(alias_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "GenericAlias.__mro_entries__ expected GenericAlias",
            );
        };
        unsafe {
            if object_type_id(alias_ptr) != TYPE_ID_GENERIC_ALIAS {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "GenericAlias.__mro_entries__ expected GenericAlias",
                );
            }
            let origin_bits = generic_alias_origin_bits(alias_ptr);
            let tuple_ptr = alloc_tuple(_py, &[origin_bits]);
            if tuple_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generic_alias_type_new(
    cls_bits: u64,
    origin_bits: u64,
    args_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "GenericAlias.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "GenericAlias.__new__ expects type");
            }
        }
        let builtins = builtin_classes(_py);
        let is_generic_alias_subtype =
            cls_bits == builtins.generic_alias || issubclass_bits(cls_bits, builtins.generic_alias);
        if !is_generic_alias_subtype {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "GenericAlias.__new__ expected GenericAlias subtype",
            );
        }

        let out_bits = molt_generic_alias_new(origin_bits, args_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(out_ptr) = obj_from_bits(out_bits).as_ptr() else {
            return out_bits;
        };
        unsafe {
            let old_class_bits = object_class_bits(out_ptr);
            if old_class_bits != cls_bits {
                if old_class_bits != 0 {
                    dec_ref_bits(_py, old_class_bits);
                }
                object_set_class_bits(_py, out_ptr, cls_bits);
                inc_ref_bits(_py, cls_bits);
            }
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_typing_type_param(typevar_ctor_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "type parameter name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "type parameter name must be str");
            }
        }
        let builder_bits = molt_callargs_new(1, 0);
        if builder_bits == 0 {
            return MoltObject::none().bits();
        }
        unsafe {
            let _ = molt_callargs_push_pos(builder_bits, name_bits);
        }
        let typevar_bits = molt_call_bind(typevar_ctor_bits, builder_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(flag_name_bits) = attr_name_bits_from_bytes(_py, b"_pep695") else {
            return MoltObject::none().bits();
        };
        let _ = molt_object_setattr(
            typevar_bits,
            flag_name_bits,
            MoltObject::from_bool(true).bits(),
        );
        dec_ref_bits(_py, flag_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        typevar_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_new(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_staticmethod_obj(_py, func_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_property_new(get_bits: u64, set_bits: u64, del_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_property_obj(_py, get_bits, set_bits, del_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_property_getter(prop_bits: u64, get_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let prop_obj = obj_from_bits(prop_bits);
        let Some(prop_ptr) = prop_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "property.getter expects property");
        };
        unsafe {
            if object_type_id(prop_ptr) != TYPE_ID_PROPERTY {
                return raise_exception::<_>(_py, "TypeError", "property.getter expects property");
            }
            let set_bits = property_set_bits(prop_ptr);
            let del_bits = property_del_bits(prop_ptr);
            let ptr = alloc_property_obj(_py, get_bits, set_bits, del_bits);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_property_setter(prop_bits: u64, set_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let prop_obj = obj_from_bits(prop_bits);
        let Some(prop_ptr) = prop_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "property.setter expects property");
        };
        unsafe {
            if object_type_id(prop_ptr) != TYPE_ID_PROPERTY {
                return raise_exception::<_>(_py, "TypeError", "property.setter expects property");
            }
            let get_bits = property_get_bits(prop_ptr);
            let del_bits = property_del_bits(prop_ptr);
            let ptr = alloc_property_obj(_py, get_bits, set_bits, del_bits);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_property_deleter(prop_bits: u64, del_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let prop_obj = obj_from_bits(prop_bits);
        let Some(prop_ptr) = prop_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "property.deleter expects property");
        };
        unsafe {
            if object_type_id(prop_ptr) != TYPE_ID_PROPERTY {
                return raise_exception::<_>(_py, "TypeError", "property.deleter expects property");
            }
            let get_bits = property_get_bits(prop_ptr);
            let set_bits = property_set_bits(prop_ptr);
            let ptr = alloc_property_obj(_py, get_bits, set_bits, del_bits);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}
