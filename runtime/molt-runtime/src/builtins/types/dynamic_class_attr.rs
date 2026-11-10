use super::*;
use crate::BuiltinExc;
use crate::builtins::functions::native_callable::NativeCallableKind;

struct AttrValue {
    bits: u64,
    owned: bool,
}

fn dynamic_class_attr_get(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name: crate::NameId,
    default_bits: u64,
) -> Option<AttrValue> {
    let name_bits = name.owned_bits(_py)?;
    let result = crate::lookup_attribute(_py, obj_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    match result {
        crate::AttributeLookup::Found(bits) => Some(AttrValue { bits, owned: true }),
        crate::AttributeLookup::Missing => Some(AttrValue {
            bits: default_bits,
            owned: false,
        }),
        crate::AttributeLookup::Error => None,
    }
}

fn dynamic_class_attr_set(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name: crate::NameId,
    value_bits: u64,
) -> bool {
    let Some(name_bits) = name.owned_bits(_py) else {
        return false;
    };
    let _ = molt_set_attr_name_attached_body(_py, obj_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

pub(crate) fn molt_types_dynamic_class_attr_init_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(positional) = call_vararg_args(_py, "DynamicClassAttribute.__init__", args_bits)
        else {
            return MoltObject::none().bits();
        };
        let Some(keywords) = call_vararg_kwargs(_py, "DynamicClassAttribute.__init__", kwargs_bits)
        else {
            return MoltObject::none().bits();
        };
        if positional.len() > 4 {
            let msg = format!(
                "DynamicClassAttribute.__init__() takes at most 4 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
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
                            BuiltinExc::TypeError,
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
                            BuiltinExc::TypeError,
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
                            BuiltinExc::TypeError,
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
                            BuiltinExc::TypeError,
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
                    return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
                }
            }
        }

        let mut effective_doc = doc_bits;
        let overwrite_doc = obj_from_bits(doc_bits).is_none();
        if overwrite_doc {
            let Some(fget_doc) =
                dynamic_class_attr_get(_py, fget_bits, crate::NameId::DunderDoc, none)
            else {
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
            crate::NameId::DunderIsabstractmethod,
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let is_abstract_flag = is_truthy(_py, obj_from_bits(is_abstract.bits));
        if is_abstract.owned {
            dec_ref_bits(_py, is_abstract.bits);
        }

        if !dynamic_class_attr_set(_py, self_bits, crate::NameId::Fget, fget_bits)
            || !dynamic_class_attr_set(_py, self_bits, crate::NameId::Fset, fset_bits)
            || !dynamic_class_attr_set(_py, self_bits, crate::NameId::Fdel, fdel_bits)
            || !dynamic_class_attr_set(_py, self_bits, crate::NameId::DunderDoc, effective_doc)
            || !dynamic_class_attr_set(
                _py,
                self_bits,
                crate::NameId::OverwriteDoc,
                MoltObject::from_bool(overwrite_doc).bits(),
            )
            || !dynamic_class_attr_set(
                _py,
                self_bits,
                crate::NameId::DunderIsabstractmethod,
                MoltObject::from_bool(is_abstract_flag).bits(),
            )
        {
            return MoltObject::none().bits();
        }

        MoltObject::none().bits()
    })
}

pub(crate) fn molt_types_dynamic_class_attr_get_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let kwargs_is_dict = obj_from_bits(kwargs_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DICT });
        let (positional, keywords) = if kwargs_is_dict {
            let Some(positional) =
                call_vararg_args(_py, "DynamicClassAttribute.__get__", args_bits)
            else {
                return MoltObject::none().bits();
            };
            let Some(keywords) =
                call_vararg_kwargs(_py, "DynamicClassAttribute.__get__", kwargs_bits)
            else {
                return MoltObject::none().bits();
            };
            (positional, Some(keywords))
        } else {
            // Descriptor protocol dispatch may call __get__ directly with
            // `(instance, ownerclass)` instead of vararg tuple/dict packing.
            (vec![args_bits, kwargs_bits], None)
        };
        if positional.len() > 2 {
            let msg = format!(
                "DynamicClassAttribute.__get__() takes at most 2 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
        }
        let none = MoltObject::none().bits();
        let mut instance_bits = positional.first().copied().unwrap_or(0);
        let mut has_instance = !positional.is_empty();
        // Packed ingress keeps the canonical keyword snapshot alive through
        // fget callbacks; direct descriptor ingress has no keyword carrier.
        for (key, val_bits) in keywords.iter().flat_map(|owned| owned.iter()) {
            match key.as_str() {
                "instance" => {
                    if has_instance {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
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
                    return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
                }
            }
        }
        if !has_instance {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "DynamicClassAttribute.__get__() missing 1 required positional argument: 'instance'",
            );
        }

        if obj_from_bits(instance_bits).is_none() {
            let Some(is_abstract) = dynamic_class_attr_get(
                _py,
                self_bits,
                crate::NameId::DunderIsabstractmethod,
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
            return raise_exception::<_>(_py, BuiltinExc::AttributeError, "");
        }

        let Some(fget) = dynamic_class_attr_get(_py, self_bits, crate::NameId::Fget, none) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(fget.bits).is_none() {
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            return raise_exception::<_>(_py, BuiltinExc::AttributeError, "unreadable attribute");
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

pub(crate) fn molt_types_dynamic_class_attr_set_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    instance_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let none = MoltObject::none().bits();
        let Some(fset) = dynamic_class_attr_get(_py, self_bits, crate::NameId::Fset, none) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(fset.bits).is_none() {
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            return raise_exception::<_>(_py, BuiltinExc::AttributeError, "can't set attribute");
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

pub(crate) fn molt_types_dynamic_class_attr_delete_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    instance_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let none = MoltObject::none().bits();
        let Some(fdel) = dynamic_class_attr_get(_py, self_bits, crate::NameId::Fdel, none) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(fdel.bits).is_none() {
            if fdel.owned {
                dec_ref_bits(_py, fdel.bits);
            }
            return raise_exception::<_>(_py, BuiltinExc::AttributeError, "can't delete attribute");
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

pub(crate) fn molt_types_dynamic_class_attr_getter_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    fget_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let none = MoltObject::none().bits();
        let Some(overwrite_doc) = dynamic_class_attr_get(
            _py,
            self_bits,
            crate::NameId::OverwriteDoc,
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let overwrite_doc_flag = is_truthy(_py, obj_from_bits(overwrite_doc.bits));
        let Some(fset) = dynamic_class_attr_get(_py, self_bits, crate::NameId::Fset, none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(fdel) = dynamic_class_attr_get(_py, self_bits, crate::NameId::Fdel, none) else {
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
            if let Some(fdoc) =
                dynamic_class_attr_get(_py, fget_bits, crate::NameId::DunderDoc, none)
            {
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
            if let Some(self_doc) =
                dynamic_class_attr_get(_py, self_bits, crate::NameId::DunderDoc, none)
            {
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
                crate::NameId::OverwriteDoc,
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

pub(crate) fn molt_types_dynamic_class_attr_setter_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    fset_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let none = MoltObject::none().bits();
        let Some(overwrite_doc) = dynamic_class_attr_get(
            _py,
            self_bits,
            crate::NameId::OverwriteDoc,
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let overwrite_doc_flag = is_truthy(_py, obj_from_bits(overwrite_doc.bits));
        let Some(fget) = dynamic_class_attr_get(_py, self_bits, crate::NameId::Fget, none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(fdel) = dynamic_class_attr_get(_py, self_bits, crate::NameId::Fdel, none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(doc) = dynamic_class_attr_get(_py, self_bits, crate::NameId::DunderDoc, none)
        else {
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
                crate::NameId::OverwriteDoc,
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

pub(crate) fn molt_types_dynamic_class_attr_deleter_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    fdel_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let none = MoltObject::none().bits();
        let Some(overwrite_doc) = dynamic_class_attr_get(
            _py,
            self_bits,
            crate::NameId::OverwriteDoc,
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let overwrite_doc_flag = is_truthy(_py, obj_from_bits(overwrite_doc.bits));
        let Some(fget) = dynamic_class_attr_get(_py, self_bits, crate::NameId::Fget, none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(fset) = dynamic_class_attr_get(_py, self_bits, crate::NameId::Fset, none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(doc) = dynamic_class_attr_get(_py, self_bits, crate::NameId::DunderDoc, none)
        else {
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
                crate::NameId::OverwriteDoc,
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

pub(crate) fn dynamic_class_attribute_class(_py: &PyToken<'_>) -> u64 {
    let state = types_state(_py);
    let signature = RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, true);
    let methods = [
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::DunderInit,
            NativeCallableKind::WrapperDescriptor,
            molt_types_dynamic_class_attr_init as *const () as usize as u64,
            3,
            signature,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::DunderGet,
            NativeCallableKind::WrapperDescriptor,
            molt_types_dynamic_class_attr_get as *const () as usize as u64,
            3,
            signature,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderSet,
            NativeCallableKind::WrapperDescriptor,
            molt_types_dynamic_class_attr_set as *const () as usize as u64,
            3,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderDelete,
            NativeCallableKind::WrapperDescriptor,
            molt_types_dynamic_class_attr_delete as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Getter,
            NativeCallableKind::MethodDescriptor,
            molt_types_dynamic_class_attr_getter as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Setter,
            NativeCallableKind::MethodDescriptor,
            molt_types_dynamic_class_attr_setter as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Deleter,
            NativeCallableKind::MethodDescriptor,
            molt_types_dynamic_class_attr_deleter as *const () as usize as u64,
            2,
        ),
    ];
    init_cached_runtime_class(
        _py,
        &state.dynamic_class_attribute_class,
        "DynamicClassAttribute",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(false, true),
            native_module: None,
            layout_size: 8,
            instance_shape: None,
            native_slots: None,
        },
        &methods,
    )
}

include!("../../attached_exports_generated/builtins/types/dynamic_class_attr.rs");
