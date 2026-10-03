//! Native IO member ownership, publication and access share this table.
//! Ordinary Python attributes live in the handle's instance dictionary; native
//! getsets remain data descriptors even when that dictionary contains their name.
use super::*;
use crate::builtins::types::{
    NativeDescriptorFlavor, NativeDescriptorSpec, alloc_native_descriptor,
};
use crate::object::layout::{
    native_descriptor_getter_bits, native_descriptor_name_bits, native_descriptor_operation,
    native_descriptor_owner_bits,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum IoClass {
    Base,
    TextBase,
    File,
    FileIO,
    BufferedReader,
    BufferedWriter,
    BufferedRandom,
    TextWrapper,
    BytesIO,
    StringIO,
}

impl IoClass {
    fn bits(self, py: &PyToken<'_>) -> u64 {
        let classes = builtin_classes(py);
        match self {
            Self::Base => classes.io_base,
            Self::TextBase => classes.text_io_base,
            Self::File => classes.file,
            Self::FileIO => classes.file_io,
            Self::BufferedReader => classes.buffered_reader,
            Self::BufferedWriter => classes.buffered_writer,
            Self::BufferedRandom => classes.buffered_random,
            Self::TextWrapper => classes.text_io_wrapper,
            Self::BytesIO => classes.bytes_io,
            Self::StringIO => classes.string_io,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum IoOperation {
    Dictionary = 1,
    Closed,
    Name,
    Mode,
    Encoding,
    Errors,
    Newlines,
    LineBuffering,
    WriteThrough,
    Buffer,
    Closefd,
}

impl IoOperation {
    fn from_tag(tag: u32) -> Option<Self> {
        Some(match tag {
            1 => Self::Dictionary,
            2 => Self::Closed,
            3 => Self::Name,
            4 => Self::Mode,
            5 => Self::Encoding,
            6 => Self::Errors,
            7 => Self::Newlines,
            8 => Self::LineBuffering,
            9 => Self::WriteThrough,
            10 => Self::Buffer,
            11 => Self::Closefd,
            _ => return None,
        })
    }
}

struct IoMember {
    name: &'static str,
    owners: &'static [IoClass],
    operation: IoOperation,
    // FileIO.name and TextIOWrapper.mode are ordinary initialized attributes in
    // CPython. The native backing also retains operational resource metadata.
    dictionary: bool,
}

const IO_CLASSES: &[IoClass] = &[
    IoClass::Base,
    IoClass::TextBase,
    IoClass::File,
    IoClass::FileIO,
    IoClass::BufferedReader,
    IoClass::BufferedWriter,
    IoClass::BufferedRandom,
    IoClass::TextWrapper,
    IoClass::BytesIO,
    IoClass::StringIO,
];
const IO_MEMBERS: &[IoMember] = &[
    IoMember {
        name: "__dict__",
        owners: &[IoClass::Base],
        operation: IoOperation::Dictionary,
        dictionary: false,
    },
    IoMember {
        name: "closed",
        owners: &[
            IoClass::Base,
            IoClass::File,
            IoClass::FileIO,
            IoClass::BufferedReader,
            IoClass::BufferedWriter,
            IoClass::BufferedRandom,
            IoClass::TextWrapper,
            IoClass::BytesIO,
            IoClass::StringIO,
        ],
        operation: IoOperation::Closed,
        dictionary: false,
    },
    IoMember {
        name: "name",
        owners: &[
            IoClass::File,
            IoClass::BufferedReader,
            IoClass::BufferedWriter,
            IoClass::BufferedRandom,
            IoClass::TextWrapper,
        ],
        operation: IoOperation::Name,
        dictionary: false,
    },
    IoMember {
        name: "name",
        owners: &[IoClass::FileIO],
        operation: IoOperation::Name,
        dictionary: true,
    },
    IoMember {
        name: "mode",
        owners: &[
            IoClass::File,
            IoClass::FileIO,
            IoClass::BufferedReader,
            IoClass::BufferedWriter,
            IoClass::BufferedRandom,
        ],
        operation: IoOperation::Mode,
        dictionary: false,
    },
    IoMember {
        name: "mode",
        owners: &[IoClass::TextWrapper],
        operation: IoOperation::Mode,
        dictionary: true,
    },
    IoMember {
        name: "encoding",
        owners: &[IoClass::File, IoClass::TextBase, IoClass::TextWrapper],
        operation: IoOperation::Encoding,
        dictionary: false,
    },
    IoMember {
        name: "errors",
        owners: &[IoClass::File, IoClass::TextBase, IoClass::TextWrapper],
        operation: IoOperation::Errors,
        dictionary: false,
    },
    IoMember {
        name: "newlines",
        owners: &[
            IoClass::File,
            IoClass::TextBase,
            IoClass::TextWrapper,
            IoClass::StringIO,
        ],
        operation: IoOperation::Newlines,
        dictionary: false,
    },
    IoMember {
        name: "line_buffering",
        owners: &[IoClass::File, IoClass::TextWrapper, IoClass::StringIO],
        operation: IoOperation::LineBuffering,
        dictionary: false,
    },
    IoMember {
        name: "write_through",
        owners: &[IoClass::File, IoClass::TextWrapper],
        operation: IoOperation::WriteThrough,
        dictionary: false,
    },
    IoMember {
        name: "buffer",
        owners: &[IoClass::File, IoClass::TextWrapper],
        operation: IoOperation::Buffer,
        dictionary: false,
    },
    IoMember {
        name: "closefd",
        owners: &[IoClass::FileIO],
        operation: IoOperation::Closefd,
        dictionary: false,
    },
];

/// Called once by builtin-family publication, after every anchor is sealed.
/// Stage all namespaces before publishing any descriptor into a live class.
pub(crate) fn io_publish_members(py: &PyToken<'_>) -> bool {
    unsafe {
        let getter =
            crate::builtins::methods::alloc_builtin_function(py, fn_addr!(molt_io_member_get), 2);
        let Some(getter_ptr) = obj_from_bits(getter).as_ptr() else {
            return false;
        };
        let _getter = PtrDropGuard::new(getter_ptr);
        let mut owners = Vec::new();
        let mut publications = Vec::new();
        for &kind in IO_CLASSES {
            let owner = kind.bits(py);
            let class = obj_from_bits(owner).as_ptr().unwrap();
            let dictionary = class_dict_bits(class);
            let live = obj_from_bits(dictionary).as_ptr().unwrap();
            let staged_bits = crate::object::ops_dict::molt_dict_copy(dictionary);
            let Some(staged) = obj_from_bits(staged_bits).as_ptr() else {
                return false;
            };
            owners.push(PtrDropGuard::new(staged));
            if exception_pending(py) {
                return false;
            }
            for member in IO_MEMBERS
                .iter()
                .filter(|member| !member.dictionary && member.owners.contains(&kind))
            {
                let Some(name) = attr_name_bits_from_bytes(py, member.name.as_bytes()) else {
                    return false;
                };
                let _name = PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
                let descriptor = alloc_native_descriptor(
                    py,
                    NativeDescriptorSpec {
                        flavor: NativeDescriptorFlavor::GetSet,
                        operation: member.operation as u32,
                        owner,
                        name,
                        doc: MoltObject::none().bits(),
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
            publications.push((class, live, staged));
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

unsafe fn io_member_value(
    py: &PyToken<'_>,
    object: *mut u8,
    operation: IoOperation,
) -> Option<u64> {
    unsafe {
        if operation == IoOperation::Dictionary {
            let dictionary = crate::object::field_storage::materialize(py, object)?;
            inc_ref_bits(py, dictionary);
            return Some(dictionary);
        }
        // The abstract IO base classes use managed instance storage. Their
        // default text properties are None and their closed flag is an ordinary
        // internal attribute, as in CPython's IOBase implementation.
        if object_type_id(object) != TYPE_ID_FILE_HANDLE {
            if operation == IoOperation::Closed {
                let key = attr_name_bits_from_bytes(py, b"__IOBase_closed")?;
                let missing = missing_bits(py);
                let value = molt_getattr_builtin(MoltObject::from_ptr(object).bits(), key, missing);
                dec_ref_bits(py, key);
                let closed = !is_missing_bits(py, value);
                dec_ref_bits(py, value);
                return if exception_pending(py) {
                    None
                } else {
                    Some(MoltObject::from_bool(closed).bits())
                };
            }
            return matches!(
                operation,
                IoOperation::Encoding | IoOperation::Errors | IoOperation::Newlines
            )
            .then_some(MoltObject::none().bits());
        }
        let handle_ptr = file_handle_ptr(object);
        if handle_ptr.is_null() {
            return None;
        }
        let handle = &*handle_ptr;
        match operation {
            IoOperation::Dictionary => unreachable!(),
            IoOperation::Closed => {
                if handle.detached {
                    return raise_exception::<_>(
                        py,
                        "ValueError",
                        file_handle_detached_message(handle),
                    );
                }
                Some(MoltObject::from_bool(file_handle_is_closed(handle)).bits())
            }
            IoOperation::Name => {
                if handle.detached {
                    return raise_exception::<_>(
                        py,
                        "ValueError",
                        file_handle_detached_message(handle),
                    );
                }
                if handle.name_bits == 0 {
                    return None;
                }
                inc_ref_bits(py, handle.name_bits);
                Some(handle.name_bits)
            }
            IoOperation::Mode => {
                if handle.detached && !handle.text {
                    return raise_exception::<_>(
                        py,
                        "ValueError",
                        file_handle_detached_message(handle),
                    );
                }
                let value = alloc_string(py, handle.mode.as_bytes());
                (!value.is_null()).then(|| MoltObject::from_ptr(value).bits())
            }
            IoOperation::Encoding | IoOperation::Errors => {
                if !handle.text {
                    return None;
                }
                let value = if operation == IoOperation::Encoding {
                    &handle.encoding
                } else {
                    &handle.errors
                };
                let Some(value) = value else {
                    return Some(MoltObject::none().bits());
                };
                let value = alloc_string(py, value.as_bytes());
                (!value.is_null()).then(|| MoltObject::from_ptr(value).bits())
            }
            IoOperation::Newlines => {
                if !handle.text {
                    return None;
                }
                if handle.newlines_len == 0 {
                    return Some(MoltObject::none().bits());
                }
                let mut values = Vec::new();
                for index in 0..handle.newlines_len {
                    let value = match handle.newlines_seen[index as usize] {
                        NEWLINE_KIND_LF => b"\n".as_slice(),
                        NEWLINE_KIND_CR => b"\r".as_slice(),
                        NEWLINE_KIND_CRLF => b"\r\n".as_slice(),
                        _ => unreachable!("invalid native IO newline kind"),
                    };
                    let value = alloc_string(py, value);
                    if value.is_null() {
                        for value in values {
                            dec_ref_bits(py, value);
                        }
                        return None;
                    }
                    values.push(MoltObject::from_ptr(value).bits());
                }
                if values.len() == 1 {
                    return Some(values[0]);
                }
                let result = alloc_tuple(py, &values);
                for value in values {
                    dec_ref_bits(py, value);
                }
                (!result.is_null()).then(|| MoltObject::from_ptr(result).bits())
            }
            IoOperation::LineBuffering => Some(MoltObject::from_bool(handle.line_buffering).bits()),
            IoOperation::WriteThrough => handle
                .text
                .then(|| MoltObject::from_bool(handle.write_through).bits()),
            IoOperation::Buffer => {
                if !handle.text {
                    return None;
                }
                let value = handle.buffer_bits;
                if handle.detached || value == 0 {
                    return Some(MoltObject::none().bits());
                }
                inc_ref_bits(py, value);
                Some(value)
            }
            IoOperation::Closefd => {
                if handle.detached {
                    return raise_exception::<_>(
                        py,
                        "ValueError",
                        file_handle_detached_message(handle),
                    );
                }
                Some(MoltObject::from_bool(handle.closefd).bits())
            }
        }
    }
}

/// Initialize only the table's ordinary dictionary members before publication.
/// Later Python writes never alter operational file metadata in native backing.
pub(super) unsafe fn io_initialize_dictionary_members(py: &PyToken<'_>, object: *mut u8) -> bool {
    unsafe {
        let class = object_class_bits(object);
        for member in IO_MEMBERS.iter().filter(|member| member.dictionary) {
            if !member.owners.iter().any(|owner| {
                crate::object::class_layout::is_real_subtype(py, class, owner.bits(py))
            }) {
                continue;
            }
            let Some(value) = io_member_value(py, object, member.operation) else {
                if exception_pending(py) {
                    return false;
                }
                continue;
            };
            let _value = obj_from_bits(value).as_ptr().map(PtrDropGuard::new);
            let Some(name) = attr_name_bits_from_bytes(py, member.name.as_bytes()) else {
                return false;
            };
            let _name = PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
            let Some(dictionary) = crate::object::field_storage::materialize(py, object) else {
                return false;
            };
            dict_set_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), name, value);
            if exception_pending(py) {
                return false;
            }
        }
        true
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_io_member_get(descriptor: u64, instance: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let Some(descriptor) = obj_from_bits(descriptor)
                .as_ptr()
                .filter(|&ptr| object_type_id(ptr) == crate::TYPE_ID_NATIVE_DESCRIPTOR)
            else {
                return raise_exception::<_>(py, "TypeError", "invalid IO member descriptor");
            };
            let Some(object) = obj_from_bits(instance).as_ptr() else {
                return raise_exception::<_>(py, "TypeError", "invalid IO member receiver");
            };
            let Some(operation) =
                native_descriptor_operation(descriptor).and_then(IoOperation::from_tag)
            else {
                return raise_exception::<_>(py, "TypeError", "invalid IO member descriptor");
            };
            let owner = native_descriptor_owner_bits(descriptor);
            let Some(getter) = obj_from_bits(native_descriptor_getter_bits(descriptor))
                .as_ptr()
                .filter(|&ptr| object_type_id(ptr) == TYPE_ID_FUNCTION)
            else {
                return raise_exception::<_>(py, "TypeError", "invalid IO member descriptor");
            };
            if function_fn_ptr(getter)
                != crate::builtins::functions::runtime_fn_key(
                    "molt_io_member_get",
                    molt_io_member_get as *const (),
                )
            {
                return raise_exception::<_>(py, "TypeError", "invalid IO member descriptor");
            }
            let actual = type_of_bits(py, instance);
            if !crate::object::class_layout::is_real_subtype(py, actual, owner) {
                return raise_exception::<_>(py, "TypeError", "invalid IO member receiver");
            }
            if let Some(value) = io_member_value(py, object, operation) {
                return value;
            }
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let name = string_obj_to_owned(obj_from_bits(native_descriptor_name_bits(descriptor)))
                .unwrap_or_default();
            crate::builtins::attr::attr_error_with_obj(
                py,
                type_name(py, obj_from_bits(instance)),
                &name,
                instance,
            )
        }
    })
}
