//! Physical list admission shared by Python base descriptors and their readers.
//! Managed values retain their existing Vec owner; native values use the ABI's
//! PyListObject owner. A borrowed call receiver keeps either storage alive.
use crate::object::foreign::NativeObjectOwner;
use crate::*;
use molt_cpython_abi::abi_types::{PyList_Type, PyMappingMethods, PyObject, PySequenceMethods};
use molt_cpython_abi::api::abstract_sequence::{BinaryFunc, ObjObjProc, SsizeArgFunc};
use molt_cpython_abi::api::sequences;
use molt_cpython_abi::bridge::{GLOBAL_BRIDGE, owned_native_result_to_runtime};
use molt_cpython_abi::hooks::DecodedHandleResult;
use molt_obj_model::sequence_compare::RichCompareOp;

#[derive(Clone, Copy)]
enum Storage {
    Managed(*mut u8),
    Native(*mut PyObject),
}

#[derive(Clone, Copy)]
pub(crate) struct ListStorage<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
    storage: Storage,
}

pub(crate) fn native_list(bits: u64) -> Option<*mut PyObject> {
    let wrapper = obj_from_bits(bits).as_ptr()?;
    if unsafe { object_type_id(wrapper) } != TYPE_ID_FOREIGN {
        return None;
    }
    let object = std::ptr::with_exposed_provenance_mut(unsafe {
        crate::object::foreign::foreign_ptr_from_obj(wrapper)
    });
    (unsafe { sequences::PyList_Check(object) } != 0).then_some(object)
}

impl<'a, 'py> ListStorage<'a, 'py> {
    pub(crate) fn from_bits(py: &'a PyToken<'py>, bits: u64) -> Option<Self> {
        let storage = if let Some(ptr) = super::ops_list::list_storage_ptr(bits) {
            Storage::Managed(ptr)
        } else {
            Storage::Native(native_list(bits)?)
        };
        Some(Self { py, bits, storage })
    }
    pub(crate) fn admit(py: &'a PyToken<'py>, bits: u64, method: &str) -> Option<Self> {
        Self::from_bits(py, bits).or_else(|| {
            raise_exception(
                py,
                "TypeError",
                &format!(
                    "descriptor '{method}' requires a 'list' object but received a '{}'",
                    type_name(py, obj_from_bits(bits))
                ),
            )
        })
    }
    pub(crate) fn managed(self) -> Option<*mut u8> {
        match self.storage {
            Storage::Managed(ptr) => Some(ptr),
            Storage::Native(_) => None,
        }
    }
    pub(crate) fn native(self) -> Option<*mut PyObject> {
        match self.storage {
            Storage::Native(ptr) => Some(ptr),
            Storage::Managed(_) => None,
        }
    }
    pub(crate) fn wrapper_ptr(self) -> *mut u8 {
        obj_from_bits(self.bits).as_ptr().unwrap()
    }
    pub(crate) fn promote(self) -> bool {
        match self.storage {
            Storage::Managed(ptr) => unsafe {
                super::ops_list::promote_specialized_list_to_list(self.py, ptr);
                object_type_id(ptr) == TYPE_ID_LIST
            },
            Storage::Native(_) => true,
        }
    }
    pub(crate) fn len(self) -> Option<usize> {
        match self.storage {
            Storage::Managed(ptr) => Some(unsafe { list_len(ptr) }),
            Storage::Native(ptr) => {
                let len = unsafe { sequences::PyList_Size(ptr) };
                if len < 0 {
                    self.failed();
                    None
                } else {
                    Some(len as usize)
                }
            }
        }
    }
    /// Retain the current element across callbacks. End of current storage is
    /// clean; a malformed physical element remains a real pending failure.
    pub(crate) fn item(
        self,
        index: usize,
    ) -> Option<crate::object::seq_access::PinnedSequenceItem<'a, 'py>> {
        match self.storage {
            Storage::Managed(ptr) => unsafe {
                crate::object::seq_access::pin_item(self.py, ptr, index)
            },
            Storage::Native(ptr) => {
                if index >= self.len()? {
                    return None;
                }
                let bits =
                    self.result(unsafe { sequences::PyList_GetItemRef(ptr, index as isize) })?;
                Some(unsafe {
                    crate::object::seq_access::PinnedSequenceItem::from_owned_bits(self.py, bits)
                })
            }
        }
    }
    fn failed(self) {
        crate::cpython_abi_hooks::propagate_native_failure(self.py, "list storage operation");
    }
    pub(crate) fn status(self, status: i32) -> bool {
        if status < 0 {
            self.failed();
            false
        } else {
            true
        }
    }
    fn result(self, result: *mut PyObject) -> Option<u64> {
        match unsafe { owned_native_result_to_runtime(result) }.decode() {
            DecodedHandleResult::Ok(bits) => Some(bits),
            _ => {
                self.failed();
                None
            }
        }
    }
    fn project(self, bits: u64) -> Option<NativeObjectOwner> {
        let owner = unsafe {
            NativeObjectOwner::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits))
        };
        if owner.as_ptr().is_null() {
            self.failed();
            None
        } else {
            Some(owner)
        }
    }
    pub(crate) fn append(self, value: u64) -> bool {
        let Some(value) = self.project(value) else {
            return false;
        };
        let receiver = self.native().expect("native list adapter");
        self.status(unsafe { sequences::PyList_Append(receiver, value.as_ptr()) })
    }
    pub(crate) fn insert(self, index: isize, value: u64) -> bool {
        let Some(value) = self.project(value) else {
            return false;
        };
        let receiver = self.native().expect("native list adapter");
        self.status(unsafe { sequences::PyList_Insert(receiver, index, value.as_ptr()) })
    }
    pub(crate) fn extend(self, other: u64) -> bool {
        let Some(other) = self.project(other) else {
            return false;
        };
        let receiver = self.native().expect("native list adapter");
        self.status(unsafe { sequences::PyList_Extend(receiver, other.as_ptr()) })
    }
    pub(crate) fn clear(self) -> bool {
        let receiver = self.native().expect("native list adapter");
        self.status(unsafe { sequences::PyList_Clear(receiver) })
    }
    pub(crate) fn reverse(self) -> bool {
        let receiver = self.native().expect("native list adapter");
        self.status(unsafe { sequences::PyList_Reverse(receiver) })
    }
    pub(crate) fn copy(self) -> u64 {
        let invoke = || -> Option<u64> {
            let receiver = self.native().expect("native list adapter");
            self.result(unsafe { sequences::PyList_GetSlice(receiver, 0, isize::MAX) })
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }
    pub(crate) fn remove_at(self, index: usize) -> bool {
        match self.storage {
            Storage::Managed(ptr) => {
                let len = unsafe { list_len(ptr) };
                unsafe {
                    crate::object::list_mutation::replace_range(
                        self.py,
                        ptr,
                        index.min(len),
                        index.saturating_add(1).min(len),
                        &[],
                    )
                }
            }
            Storage::Native(ptr) => self.status(unsafe {
                sequences::PyList_SetSlice(
                    ptr,
                    index as isize,
                    (index + 1) as isize,
                    std::ptr::null_mut(),
                )
            }),
        }
    }
    pub(crate) fn pop(self, index: isize) -> u64 {
        let result = match self.storage {
            Storage::Managed(ptr) => unsafe {
                crate::object::list_mutation::pop(self.py, ptr, index as usize)
            },
            Storage::Native(ptr) => self.result(unsafe { sequences::native_list_pop(ptr, index) }),
        };
        result.unwrap_or_else(|| MoltObject::none().bits())
    }
    pub(crate) fn sort(self, key: u64, reverse: bool) -> bool {
        let Some(receiver) = self.native() else {
            unreachable!("native sort adapter");
        };
        let key = if obj_from_bits(key).is_none() {
            None
        } else {
            let Some(owner) = self.project(key) else {
                return false;
            };
            Some(owner)
        };
        self.status(unsafe {
            sequences::native_list_sort(
                receiver,
                key.as_ref()
                    .map_or(std::ptr::null_mut(), NativeObjectOwner::as_ptr),
                reverse,
            )
        })
    }
    pub(crate) fn concat(self, other: u64) -> u64 {
        let invoke = || -> Option<u64> {
            let left = self.project(self.bits)?;
            let right = self.project(other)?;
            unsafe {
                let slots = PyList_Type.tp_as_sequence.cast::<PySequenceMethods>();
                let concat =
                    std::mem::transmute::<*mut std::ffi::c_void, BinaryFunc>((*slots).sq_concat);
                self.result(concat(left.as_ptr(), right.as_ptr()))
            }
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }
    pub(crate) fn repeat(self, count: isize, in_place: bool) -> u64 {
        let invoke = || -> Option<u64> {
            let receiver = self.native().expect("native list adapter");
            unsafe {
                let slots = PyList_Type.tp_as_sequence.cast::<PySequenceMethods>();
                let slot = if in_place {
                    (*slots).sq_inplace_repeat
                } else {
                    (*slots).sq_repeat
                };
                let repeat = std::mem::transmute::<*mut std::ffi::c_void, SsizeArgFunc>(slot);
                self.result(repeat(receiver, count))
            }
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }
    pub(crate) fn getitem(self, key: u64) -> u64 {
        let invoke = || -> Option<u64> {
            let receiver = self.native().expect("native list adapter");
            let key = self.project(key)?;
            unsafe {
                let slots = PyList_Type.tp_as_mapping.cast::<PyMappingMethods>();
                let get =
                    std::mem::transmute::<*mut std::ffi::c_void, BinaryFunc>((*slots).mp_subscript);
                self.result(get(receiver, key.as_ptr()))
            }
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }
    pub(crate) fn assign(self, key: u64, value: Option<u64>) -> bool {
        let invoke = || -> Option<bool> {
            let receiver = self.native().expect("native list adapter");
            let key = self.project(key)?;
            let value = match value {
                Some(bits) => Some(self.project(bits)?),
                None => None,
            };
            unsafe {
                let slots = PyList_Type.tp_as_mapping.cast::<PyMappingMethods>();
                let assign = std::mem::transmute::<
                    *mut std::ffi::c_void,
                    unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> i32,
                >((*slots).mp_ass_subscript);
                Some(
                    self.status(assign(
                        receiver,
                        key.as_ptr(),
                        value
                            .as_ref()
                            .map_or(std::ptr::null_mut(), NativeObjectOwner::as_ptr),
                    )),
                )
            }
        };
        invoke().unwrap_or(false)
    }
    pub(crate) fn contains(self, item: u64) -> u64 {
        let invoke = || -> Option<u64> {
            let receiver = self.native().expect("native list adapter");
            let item = self.project(item)?;
            unsafe {
                let slots = PyList_Type.tp_as_sequence.cast::<PySequenceMethods>();
                let contains =
                    std::mem::transmute::<*mut std::ffi::c_void, ObjObjProc>((*slots).sq_contains);
                let result = contains(receiver, item.as_ptr());
                self.status(result)
                    .then(|| MoltObject::from_bool(result != 0).bits())
            }
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }
    pub(crate) fn compare(self, other: u64, op: RichCompareOp) -> u64 {
        if Self::from_bits(self.py, other).is_none() {
            return crate::builtins::methods::not_implemented_bits(self.py);
        }
        let invoke = || -> Option<u64> {
            let left = self.project(self.bits)?;
            let right = self.project(other)?;
            self.result(unsafe {
                sequences::molt_list_richcompare(left.as_ptr(), right.as_ptr(), op as i32)
            })
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }
}
