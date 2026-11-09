//! Exact-builtin traversal for CPython 3.12 Lib/copy.py's _deepcopy_list,
//! _deepcopy_tuple, _deepcopy_dict and _keep_alive. Python owns protocol dispatch.
use crate::*;
use molt_runtime_core::ProtectedRuntimeValue as Value;

fn owned<'a>(py: &'a PyToken<'_>, bits: u64) -> Result<Value<'a>, u64> {
    let value = unsafe { Value::from_owned_bits(py.core_token(), bits) };
    if exception_pending(py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(value)
    }
}

struct DeepCopy<'a, 'py> {
    py: &'a PyToken<'py>,
    memo: *mut u8,
    memo_bits: u64,
    copier: u64,
    sentinel: u64,
}

impl<'a> DeepCopy<'a, '_> {
    fn memo_get(&self, key: u64) -> Result<Option<Value<'a>>, u64> {
        unsafe { dict_get_in_place(self.py, self.memo, key) }
            .map(|value| value.map(|bits| Value::retain(self.py.core_token(), bits)))
            .map_err(|()| MoltObject::none().bits())
    }

    fn memo_put(&self, key: u64, value: u64) -> Result<(), u64> {
        if unsafe { dict_set_in_place(self.py, self.memo, key, value) } {
            Ok(())
        } else {
            Err(MoltObject::none().bits())
        }
    }

    fn keep_alive(&self, obj: u64) -> Result<(), u64> {
        let key = owned(self.py, molt_id_attached_body(self.py, self.memo_bits))?;
        if let Some(alive) = self.memo_get(key.bits())? {
            if let Some(ptr) = obj_from_bits(alive.bits()).as_ptr()
                && matches!(
                    unsafe { object_type_id(ptr) },
                    TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL
                )
                && unsafe { crate::object::iterable::builtin_receiver(self.py, ptr) }
            {
                if !unsafe { crate::object::list_mutation::extend_from_slice(self.py, ptr, &[obj]) }
                {
                    return Err(MoltObject::none().bits());
                }
            } else {
                // A user may seed memo[id(memo)] with an arbitrary appendable.
                let name = crate::NameId::Append
                    .owned_bits(self.py)
                    .ok_or_else(|| MoltObject::none().bits())?;
                let name = owned(self.py, name)?;
                let append = owned(
                    self.py,
                    molt_getattr_builtin_attached_body(
                        self.py,
                        alive.bits(),
                        name.bits(),
                        missing_bits(self.py),
                    ),
                )?;
                let _result = owned(self.py, unsafe {
                    call_callable1(self.py, append.bits(), obj)
                })?;
            }
        } else {
            let ptr = alloc_list(self.py, &[obj]);
            if ptr.is_null() {
                return Err(MoltObject::none().bits());
            }
            let alive = owned(self.py, MoltObject::from_ptr(ptr).bits())?;
            self.memo_put(key.bits(), alive.bits())?;
        }
        Ok(())
    }

    fn copy(&self, obj: u64) -> Result<Value<'a>, u64> {
        let value = obj_from_bits(obj);
        if value.as_ptr().is_none() {
            // molt_id uses the signed immediate word here. Its often-large
            // Python integer need only be materialized on a hash candidate;
            // preseeded/user-equal keys still use the shared rich lookup.
            let cached = unsafe {
                crate::object::ops::dict_get_i64_in_place(self.py, self.memo, obj as i64)
            }
            .map_err(|()| MoltObject::none().bits())?;
            return Ok(Value::retain(
                self.py.core_token(),
                cached.filter(|&bits| bits != self.sentinel).unwrap_or(obj),
            ));
        }
        let ptr = value.as_ptr().unwrap();
        let kind = unsafe { object_type_id(ptr) };
        // Do not perform a second memo lookup or builtin-class test for a
        // child that Python's protocol must handle anyway. In particular,
        // ordinary instances never enter native reconstruction here.
        let native_kind = matches!(
            kind,
            TYPE_ID_TYPE
                | TYPE_ID_LIST
                | TYPE_ID_LIST_INT
                | TYPE_ID_LIST_BOOL
                | TYPE_ID_DICT
                | TYPE_ID_TUPLE
                | TYPE_ID_STRING
                | TYPE_ID_BYTES
                | TYPE_ID_RANGE
                | TYPE_ID_NOT_IMPLEMENTED
                | TYPE_ID_ELLIPSIS
                | TYPE_ID_COMPLEX
                | TYPE_ID_BIGINT
                | TYPE_ID_FLOAT
        );
        if !native_kind {
            return owned(self.py, unsafe {
                call_callable2(self.py, self.copier, obj, self.memo_bits)
            });
        }
        let exact = kind == TYPE_ID_TYPE
            || unsafe { crate::object::iterable::builtin_receiver(self.py, ptr) };
        if !exact {
            return owned(self.py, unsafe {
                call_callable2(self.py, self.copier, obj, self.memo_bits)
            });
        }
        // Even atomic objects can have a user-supplied memo entry.
        let key = owned(self.py, molt_id_attached_body(self.py, obj))?;
        if let Some(result) = self
            .memo_get(key.bits())?
            .filter(|result| result.bits() != self.sentinel)
        {
            return Ok(result);
        }
        if kind == TYPE_ID_TYPE
            || (exact
                && (value.as_float().is_some()
                    || to_i64(value).is_some()
                    || matches!(
                        kind,
                        TYPE_ID_STRING
                            | TYPE_ID_BYTES
                            | TYPE_ID_RANGE
                            | TYPE_ID_NOT_IMPLEMENTED
                            | TYPE_ID_ELLIPSIS
                            | TYPE_ID_COMPLEX
                            | TYPE_ID_BIGINT
                    )))
        {
            return Ok(Value::retain(self.py.core_token(), obj));
        }
        // CPython charges each nested _deepcopy_list/_dict/_tuple frame to the
        // recursion limit. Charge each native descent, so deep nesting raises
        // RecursionError instead of overflowing the native stack.
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(self.py) else {
            return Err(MoltObject::none().bits());
        };
        let result = match kind {
            TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL if exact => {
                self.list(obj, ptr, key.bits())?
            }
            TYPE_ID_DICT if exact => self.dict(obj, key.bits())?,
            TYPE_ID_TUPLE if exact => self.tuple(obj, ptr)?,
            // Subclasses and every other type return to the ONE Python copy
            // protocol, including copyreg and hooks. Never return self here.
            _ => {
                return owned(self.py, unsafe {
                    call_callable2(self.py, self.copier, obj, self.memo_bits)
                });
            }
        };
        if result.bits() != obj {
            self.memo_put(key.bits(), result.bits())?;
            self.keep_alive(obj)?;
        }
        Ok(result)
    }

    fn list(&self, _obj: u64, ptr: *mut u8, key: u64) -> Result<Value<'a>, u64> {
        let result = alloc_list(self.py, &[]);
        if result.is_null() {
            return Err(MoltObject::none().bits());
        }
        let result_owner = owned(self.py, MoltObject::from_ptr(result).bits())?;
        self.memo_put(key, result_owner.bits())?;
        let mut index = 0;
        while index < unsafe { crate::object::seq_access::locked_len(ptr) } {
            let item = unsafe { crate::object::seq_access::pin_item(self.py, ptr, index) }
                .ok_or_else(|| MoltObject::none().bits())?;
            let copied = self.copy(item.bits())?;
            if !unsafe {
                crate::object::list_mutation::extend_from_slice(self.py, result, &[copied.bits()])
            } {
                return Err(MoltObject::none().bits());
            }
            index += 1;
        }
        Ok(result_owner)
    }

    fn dict(&self, obj: u64, key: u64) -> Result<Value<'a>, u64> {
        let result = alloc_dict_with_pairs(self.py, &[]);
        if result.is_null() {
            return Err(MoltObject::none().bits());
        }
        let result_owner = owned(self.py, MoltObject::from_ptr(result).bits())?;
        self.memo_put(key, result_owner.bits())?;
        let items = owned(self.py, molt_dict_items_attached_body(self.py, obj))?;
        let iterator = owned(self.py, molt_iter_attached_body(self.py, items.bits()))?;
        loop {
            let mut key = MoltObject::none().bits();
            let mut value = MoltObject::none().bits();
            let done = unsafe {
                crate::object::ops_iter::molt_iter_next_dict_items_attached_body(
                    self.py,
                    iterator.bits(),
                    &mut key,
                    &mut value,
                )
            };
            let key = owned(self.py, key)?;
            let value = owned(self.py, value)?;
            if is_truthy(self.py, obj_from_bits(done)) {
                break;
            }
            // Python assignment evaluates the RHS before its target subscript.
            let copied_value = self.copy(value.bits())?;
            let copied_key = self.copy(key.bits())?;
            if !unsafe {
                dict_set_in_place(self.py, result, copied_key.bits(), copied_value.bits())
            } {
                return Err(MoltObject::none().bits());
            }
        }
        Ok(result_owner)
    }

    fn tuple(&self, obj: u64, ptr: *mut u8) -> Result<Value<'a>, u64> {
        let items = unsafe {
            crate::object::seq_access::snapshot(self.py, ptr, "copy tuple allocation failed")
        }
        .ok_or_else(|| MoltObject::none().bits())?;
        let mut copied = Vec::with_capacity(items.len());
        let mut same = true;
        for &item in items.iter() {
            let result = self.copy(item)?;
            same &= result.bits() == item;
            copied.push(result);
        }
        // A tuple may contain a mutable child that has already reconstructed
        // this tuple during recursion. CPython checks memo AFTER the children.
        let key = owned(self.py, molt_id_attached_body(self.py, obj))?;
        if let Some(result) = self.memo_get(key.bits())? {
            return Ok(result);
        }
        if same {
            return Ok(Value::retain(self.py.core_token(), obj));
        }
        let bits: Vec<_> = copied.iter().map(Value::bits).collect();
        let result = alloc_tuple(self.py, &bits);
        if result.is_null() {
            return Err(MoltObject::none().bits());
        }
        owned(self.py, MoltObject::from_ptr(result).bits())
    }
}

/// Traversal of exact list/tuple/dict objects with Python's actual memo. The
/// callback handles non-builtin children under the same copy protocol.
pub(crate) fn molt_copy_deepcopy_attached_body(
    _py: &crate::PyToken<'_>,
    obj: u64,
    memo: u64,
    copier: u64,
    sentinel: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(ptr) = obj_from_bits(memo).as_ptr() else {
            return raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "copy memo must be an exact dict",
            );
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT
            || !unsafe { crate::object::iterable::builtin_receiver(py, ptr) }
        {
            return raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "copy memo must be an exact dict",
            );
        }
        let _memo_owner = Value::retain(py.core_token(), memo);
        let _copier_owner = Value::retain(py.core_token(), copier);
        let _sentinel_owner = Value::retain(py.core_token(), sentinel);
        (|| {
            let context = DeepCopy {
                py,
                memo: ptr,
                memo_bits: memo,
                copier,
                sentinel,
            };
            let Some(obj_ptr) = obj_from_bits(obj).as_ptr() else {
                return Err(raise_exception::<u64>(
                    py,
                    BuiltinExc::TypeError,
                    "copy traversal requires an exact container",
                ));
            };
            if !unsafe { crate::object::iterable::builtin_receiver(py, obj_ptr) } {
                return Err(raise_exception::<u64>(
                    py,
                    BuiltinExc::TypeError,
                    "copy traversal requires an exact container",
                ));
            }
            let key = owned(py, molt_id_attached_body(py, obj))?;
            match unsafe { object_type_id(obj_ptr) } {
                TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => {
                    context.list(obj, obj_ptr, key.bits())
                }
                TYPE_ID_DICT => context.dict(obj, key.bits()),
                TYPE_ID_TUPLE => context.tuple(obj, obj_ptr),
                _ => Err(raise_exception::<u64>(
                    py,
                    BuiltinExc::TypeError,
                    "copy traversal requires an exact container",
                )),
            }
        })()
        .map(Value::into_bits)
        .unwrap_or_else(|error| error)
    })
}

include!("../attached_exports_generated/builtins/copy_mod.rs");
