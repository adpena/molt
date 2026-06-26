use super::*;

struct AttrValue {
    bits: u64,
    owned: bool,
}

fn dynamic_class_attr_get(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name: &str,
    default_bits: u64,
) -> Option<AttrValue> {
    let missing = missing_bits(_py);
    let name_bits = attr_name_bits_from_bytes(_py, name.as_bytes())?;
    let val_bits = crate::molt_get_attr_name_default(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return None;
    }
    if val_bits == missing {
        return Some(AttrValue {
            bits: default_bits,
            owned: false,
        });
    }
    Some(AttrValue {
        bits: val_bits,
        owned: true,
    })
}

fn dynamic_class_attr_set(_py: &PyToken<'_>, obj_bits: u64, name: &str, value_bits: u64) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name.as_bytes()) else {
        return false;
    };
    let _ = molt_object_setattr(obj_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_init(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(positional) = call_vararg_args(_py, "DynamicClassAttribute.__init__", args_bits)
        else {
            return MoltObject::none().bits();
        };
        let Some((_, keywords)) =
            call_vararg_kwargs(_py, "DynamicClassAttribute.__init__", kwargs_bits)
        else {
            return MoltObject::none().bits();
        };
        if positional.len() > 4 {
            let msg = format!(
                "DynamicClassAttribute.__init__() takes at most 4 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }

        let none = MoltObject::none().bits();
        let mut fget_bits = positional.first().copied().unwrap_or(none);
        let mut fset_bits = positional.get(1).copied().unwrap_or(none);
        let mut fdel_bits = positional.get(2).copied().unwrap_or(none);
        let mut doc_bits = positional.get(3).copied().unwrap_or(none);
        let mut has_fget = !positional.is_empty();
        let mut has_fset = positional.len() >= 2;
        let mut has_fdel = positional.len() >= 3;
        let mut has_doc = positional.len() >= 4;
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "fget" => {
                    if has_fget {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__init__() got multiple values for argument 'fget'",
                        );
                    }
                    fget_bits = *val_bits;
                    has_fget = true;
                }
                "fset" => {
                    if has_fset {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__init__() got multiple values for argument 'fset'",
                        );
                    }
                    fset_bits = *val_bits;
                    has_fset = true;
                }
                "fdel" => {
                    if has_fdel {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__init__() got multiple values for argument 'fdel'",
                        );
                    }
                    fdel_bits = *val_bits;
                    has_fdel = true;
                }
                "doc" => {
                    if has_doc {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__init__() got multiple values for argument 'doc'",
                        );
                    }
                    doc_bits = *val_bits;
                    has_doc = true;
                }
                _ => {
                    let msg = format!(
                        "DynamicClassAttribute.__init__() got an unexpected keyword argument '{}'",
                        key
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }

        let mut effective_doc = doc_bits;
        let overwrite_doc = obj_from_bits(doc_bits).is_none();
        if overwrite_doc {
            let Some(fget_doc) = dynamic_class_attr_get(_py, fget_bits, "__doc__", none) else {
                return MoltObject::none().bits();
            };
            effective_doc = fget_doc.bits;
            if fget_doc.owned {
                dec_ref_bits(_py, fget_doc.bits);
            }
        }
        let Some(is_abstract) = dynamic_class_attr_get(
            _py,
            fget_bits,
            "__isabstractmethod__",
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let is_abstract_flag = is_truthy(_py, obj_from_bits(is_abstract.bits));
        if is_abstract.owned {
            dec_ref_bits(_py, is_abstract.bits);
        }

        if !dynamic_class_attr_set(_py, self_bits, "fget", fget_bits)
            || !dynamic_class_attr_set(_py, self_bits, "fset", fset_bits)
            || !dynamic_class_attr_set(_py, self_bits, "fdel", fdel_bits)
            || !dynamic_class_attr_set(_py, self_bits, "__doc__", effective_doc)
            || !dynamic_class_attr_set(
                _py,
                self_bits,
                "overwrite_doc",
                MoltObject::from_bool(overwrite_doc).bits(),
            )
            || !dynamic_class_attr_set(
                _py,
                self_bits,
                "__isabstractmethod__",
                MoltObject::from_bool(is_abstract_flag).bits(),
            )
        {
            return MoltObject::none().bits();
        }

        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_get(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let kwargs_is_dict = obj_from_bits(kwargs_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DICT });
        let (positional, keywords) = if kwargs_is_dict {
            let Some(positional) =
                call_vararg_args(_py, "DynamicClassAttribute.__get__", args_bits)
            else {
                return MoltObject::none().bits();
            };
            let Some((_, keywords)) =
                call_vararg_kwargs(_py, "DynamicClassAttribute.__get__", kwargs_bits)
            else {
                return MoltObject::none().bits();
            };
            (positional, keywords)
        } else {
            // Descriptor protocol dispatch may call __get__ directly with
            // `(instance, ownerclass)` instead of vararg tuple/dict packing.
            (vec![args_bits, kwargs_bits], Vec::new())
        };
        if positional.len() > 2 {
            let msg = format!(
                "DynamicClassAttribute.__get__() takes at most 2 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let none = MoltObject::none().bits();
        let mut instance_bits = positional.first().copied().unwrap_or(0);
        let mut has_instance = !positional.is_empty();
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "instance" => {
                    if has_instance {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__get__() got multiple values for argument 'instance'",
                        );
                    }
                    instance_bits = *val_bits;
                    has_instance = true;
                }
                "ownerclass" => {}
                _ => {
                    let msg = format!(
                        "DynamicClassAttribute.__get__() got an unexpected keyword argument '{}'",
                        key
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if !has_instance {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "DynamicClassAttribute.__get__() missing 1 required positional argument: 'instance'",
            );
        }

        if obj_from_bits(instance_bits).is_none() {
            let Some(is_abstract) = dynamic_class_attr_get(
                _py,
                self_bits,
                "__isabstractmethod__",
                MoltObject::from_bool(false).bits(),
            ) else {
                return MoltObject::none().bits();
            };
            let is_abstract_flag = is_truthy(_py, obj_from_bits(is_abstract.bits));
            if is_abstract.owned {
                dec_ref_bits(_py, is_abstract.bits);
            }
            if is_abstract_flag {
                inc_ref_bits(_py, self_bits);
                return self_bits;
            }
            return raise_exception::<_>(_py, "AttributeError", "");
        }

        let Some(fget) = dynamic_class_attr_get(_py, self_bits, "fget", none) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(fget.bits).is_none() {
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            return raise_exception::<_>(_py, "AttributeError", "unreadable attribute");
        }
        let out = unsafe { call_callable1(_py, fget.bits, instance_bits) };
        if fget.owned {
            dec_ref_bits(_py, fget.bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_set(
    self_bits: u64,
    instance_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        let Some(fset) = dynamic_class_attr_get(_py, self_bits, "fset", none) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(fset.bits).is_none() {
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            return raise_exception::<_>(_py, "AttributeError", "can't set attribute");
        }
        let _ = unsafe { call_callable2(_py, fset.bits, instance_bits, value_bits) };
        if fset.owned {
            dec_ref_bits(_py, fset.bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_delete(self_bits: u64, instance_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        let Some(fdel) = dynamic_class_attr_get(_py, self_bits, "fdel", none) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(fdel.bits).is_none() {
            if fdel.owned {
                dec_ref_bits(_py, fdel.bits);
            }
            return raise_exception::<_>(_py, "AttributeError", "can't delete attribute");
        }
        let _ = unsafe { call_callable1(_py, fdel.bits, instance_bits) };
        if fdel.owned {
            dec_ref_bits(_py, fdel.bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

fn dynamic_class_attr_clone(
    _py: &PyToken<'_>,
    self_bits: u64,
    fget_bits: u64,
    fset_bits: u64,
    fdel_bits: u64,
    doc_bits: u64,
) -> u64 {
    let class_bits = type_of_bits(_py, self_bits);
    call_with_kwargs(
        _py,
        class_bits,
        &[fget_bits, fset_bits, fdel_bits, doc_bits],
        0,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_getter(self_bits: u64, fget_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        let Some(overwrite_doc) = dynamic_class_attr_get(
            _py,
            self_bits,
            "overwrite_doc",
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let overwrite_doc_flag = is_truthy(_py, obj_from_bits(overwrite_doc.bits));
        let Some(fset) = dynamic_class_attr_get(_py, self_bits, "fset", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(fdel) = dynamic_class_attr_get(_py, self_bits, "fdel", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            return MoltObject::none().bits();
        };
        let mut doc_bits = none;
        let mut doc_owned = false;
        if overwrite_doc_flag {
            if let Some(fdoc) = dynamic_class_attr_get(_py, fget_bits, "__doc__", none) {
                doc_bits = fdoc.bits;
                doc_owned = fdoc.owned;
            } else {
                if overwrite_doc.owned {
                    dec_ref_bits(_py, overwrite_doc.bits);
                }
                if fset.owned {
                    dec_ref_bits(_py, fset.bits);
                }
                if fdel.owned {
                    dec_ref_bits(_py, fdel.bits);
                }
                return MoltObject::none().bits();
            }
        }
        if obj_from_bits(doc_bits).is_none() {
            if let Some(self_doc) = dynamic_class_attr_get(_py, self_bits, "__doc__", none) {
                if doc_owned {
                    dec_ref_bits(_py, doc_bits);
                }
                doc_bits = self_doc.bits;
                doc_owned = self_doc.owned;
            } else {
                if overwrite_doc.owned {
                    dec_ref_bits(_py, overwrite_doc.bits);
                }
                if fset.owned {
                    dec_ref_bits(_py, fset.bits);
                }
                if fdel.owned {
                    dec_ref_bits(_py, fdel.bits);
                }
                return MoltObject::none().bits();
            }
        }

        let out =
            dynamic_class_attr_clone(_py, self_bits, fget_bits, fset.bits, fdel.bits, doc_bits);
        if !obj_from_bits(out).is_none()
            && !dynamic_class_attr_set(
                _py,
                out,
                "overwrite_doc",
                MoltObject::from_bool(overwrite_doc_flag).bits(),
            )
        {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            if fdel.owned {
                dec_ref_bits(_py, fdel.bits);
            }
            if doc_owned {
                dec_ref_bits(_py, doc_bits);
            }
            return MoltObject::none().bits();
        }

        if overwrite_doc.owned {
            dec_ref_bits(_py, overwrite_doc.bits);
        }
        if fset.owned {
            dec_ref_bits(_py, fset.bits);
        }
        if fdel.owned {
            dec_ref_bits(_py, fdel.bits);
        }
        if doc_owned {
            dec_ref_bits(_py, doc_bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_setter(self_bits: u64, fset_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        let Some(overwrite_doc) = dynamic_class_attr_get(
            _py,
            self_bits,
            "overwrite_doc",
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let overwrite_doc_flag = is_truthy(_py, obj_from_bits(overwrite_doc.bits));
        let Some(fget) = dynamic_class_attr_get(_py, self_bits, "fget", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(fdel) = dynamic_class_attr_get(_py, self_bits, "fdel", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(doc) = dynamic_class_attr_get(_py, self_bits, "__doc__", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            if fdel.owned {
                dec_ref_bits(_py, fdel.bits);
            }
            return MoltObject::none().bits();
        };
        let out =
            dynamic_class_attr_clone(_py, self_bits, fget.bits, fset_bits, fdel.bits, doc.bits);
        if !obj_from_bits(out).is_none()
            && !dynamic_class_attr_set(
                _py,
                out,
                "overwrite_doc",
                MoltObject::from_bool(overwrite_doc_flag).bits(),
            )
        {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            if fdel.owned {
                dec_ref_bits(_py, fdel.bits);
            }
            if doc.owned {
                dec_ref_bits(_py, doc.bits);
            }
            return MoltObject::none().bits();
        }
        if overwrite_doc.owned {
            dec_ref_bits(_py, overwrite_doc.bits);
        }
        if fget.owned {
            dec_ref_bits(_py, fget.bits);
        }
        if fdel.owned {
            dec_ref_bits(_py, fdel.bits);
        }
        if doc.owned {
            dec_ref_bits(_py, doc.bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_deleter(self_bits: u64, fdel_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        let Some(overwrite_doc) = dynamic_class_attr_get(
            _py,
            self_bits,
            "overwrite_doc",
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let overwrite_doc_flag = is_truthy(_py, obj_from_bits(overwrite_doc.bits));
        let Some(fget) = dynamic_class_attr_get(_py, self_bits, "fget", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(fset) = dynamic_class_attr_get(_py, self_bits, "fset", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(doc) = dynamic_class_attr_get(_py, self_bits, "__doc__", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            return MoltObject::none().bits();
        };
        let out =
            dynamic_class_attr_clone(_py, self_bits, fget.bits, fset.bits, fdel_bits, doc.bits);
        if !obj_from_bits(out).is_none()
            && !dynamic_class_attr_set(
                _py,
                out,
                "overwrite_doc",
                MoltObject::from_bool(overwrite_doc_flag).bits(),
            )
        {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            if doc.owned {
                dec_ref_bits(_py, doc.bits);
            }
            return MoltObject::none().bits();
        }
        if overwrite_doc.owned {
            dec_ref_bits(_py, overwrite_doc.bits);
        }
        if fget.owned {
            dec_ref_bits(_py, fget.bits);
        }
        if fset.owned {
            dec_ref_bits(_py, fset.bits);
        }
        if doc.owned {
            dec_ref_bits(_py, doc.bits);
        }
        out
    })
}

/// # Safety
/// `obj_ptr_bits` must encode a valid Molt object header that can be mutated,
/// and `class_bits` must be either zero or a valid Molt type object.

pub(crate) fn dynamic_class_attribute_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(
        _py,
        &types_state(_py).dynamic_class_attribute_class,
        "DynamicClassAttribute",
        8,
    );
    if class_bits == 0 || obj_from_bits(class_bits).is_none() {
        return class_bits;
    }
    let init_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_init_fn,
        molt_types_dynamic_class_attr_init as *const () as usize as u64,
        3,
    );
    let get_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_get_fn,
        molt_types_dynamic_class_attr_get as *const () as usize as u64,
        3,
    );
    let set_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_set_fn,
        molt_types_dynamic_class_attr_set as *const () as usize as u64,
        3,
    );
    let delete_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_delete_fn,
        molt_types_dynamic_class_attr_delete as *const () as usize as u64,
        2,
    );
    let getter_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_getter_fn,
        molt_types_dynamic_class_attr_getter as *const () as usize as u64,
        2,
    );
    let setter_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_setter_fn,
        molt_types_dynamic_class_attr_setter as *const () as usize as u64,
        2,
    );
    let deleter_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_deleter_fn,
        molt_types_dynamic_class_attr_deleter as *const () as usize as u64,
        2,
    );
    set_class_method(_py, class_bits, "__init__", init_bits);
    set_class_method(_py, class_bits, "__get__", get_bits);
    set_class_method(_py, class_bits, "__set__", set_bits);
    set_class_method(_py, class_bits, "__delete__", delete_bits);
    set_class_method(_py, class_bits, "getter", getter_bits);
    set_class_method(_py, class_bits, "setter", setter_bits);
    set_class_method(_py, class_bits, "deleter", deleter_bits);
    mark_vararg_method(_py, init_bits, true);
    mark_vararg_method(_py, get_bits, true);
    class_bits
}
