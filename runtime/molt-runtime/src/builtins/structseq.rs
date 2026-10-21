//! Native struct sequences: one sealed class specification, direct tuple base,
//! and the existing native-subtype field tail for non-sequence members.
//!
//! The public namespace is a projection. Mutating n_fields, descriptors or
//! __new__ never changes the allocation, member indexing or GC reference graph.

use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::builtins::native_arguments::NativeArguments;
use crate::object::class_storage::{
    ClassDeclaration, ClassReferenceSlot, ClassSemanticPolicy, ClassSlotPolicy,
};
use crate::object::seq_access::{PinnedTuple, pin_tuple};
use crate::*;
use molt_runtime_core::OwnedRuntimeValue;

#[derive(Clone, Copy, Eq, PartialEq)]
#[repr(i64)]
enum Constructor {
    Sequence = 0,
    Forbidden = 1,
    Stat = 2,
    Time = 3,
}
impl Constructor {
    fn decode(bits: u64) -> Option<Self> {
        match obj_from_bits(bits).as_int()? {
            0 => Some(Self::Sequence),
            1 => Some(Self::Forbidden),
            2 => Some(Self::Stat),
            3 => Some(Self::Time),
            _ => None,
        }
    }
}

fn exact(py: &PyToken<'_>, bits: u64, kind: u32) -> Option<*mut u8> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        let class = object_class_bits(ptr);
        let expected = match kind {
            TYPE_ID_TUPLE => builtin_classes(py).tuple,
            TYPE_ID_STRING => builtin_classes(py).str,
            _ => return None,
        };
        (object_type_id(ptr) == kind
            && (class == 0 || class == expected)
            && !crate::object::object_has_finalizer(py, ptr))
        .then_some(ptr)
    }
}

/// Exact tuple (names, docs, visible_count, constructor, native_name, doc) with no callback-bearing
/// referents. Only untrusted factory admission traverses all metadata elements.
fn valid_spec(py: &PyToken<'_>, bits: u64) -> bool {
    let Some(ptr) = exact(py, bits, TYPE_ID_TUPLE) else {
        return false;
    };
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |spec| {
            if spec.len() != 6
                || exact(py, spec[4], TYPE_ID_STRING).is_none()
                || (!obj_from_bits(spec[5]).is_none()
                    && exact(py, spec[5], TYPE_ID_STRING).is_none())
            {
                return false;
            }
            let (Some(names), Some(docs), Some(visible), Some(role)) = (
                exact(py, spec[0], TYPE_ID_TUPLE),
                exact(py, spec[1], TYPE_ID_TUPLE),
                obj_from_bits(spec[2]).as_int(),
                Constructor::decode(spec[3]),
            ) else {
                return false;
            };
            crate::object::seq_access::with_immutable_tuple_slice(names, |names| {
                crate::object::seq_access::with_immutable_tuple_slice(docs, |docs| {
                    if visible < 0
                        || visible as usize > names.len()
                        || names.len() != docs.len()
                        || names.len() > u32::MAX as usize
                    {
                        return false;
                    }
                    if role == Constructor::Stat && (visible != 10 || names.len() < 16) {
                        return false;
                    }
                    if role == Constructor::Time && (visible != 9 || names.len() != 11) {
                        return false;
                    }
                    for (index, (&name, &doc)) in names.iter().zip(docs.iter()).enumerate() {
                        if obj_from_bits(name).is_none() {
                            if index >= visible as usize {
                                return false;
                            }
                        } else if exact(py, name, TYPE_ID_STRING).is_none() {
                            return false;
                        }
                        if !obj_from_bits(doc).is_none() && exact(py, doc, TYPE_ID_STRING).is_none()
                        {
                            return false;
                        }
                    }
                    true
                })
                .unwrap()
            })
            .unwrap()
        })
        .unwrap()
    }
}

/// The class's typed declaration admits this private immutable projection.
/// Constant-work shape checks never consult Python or reinitialize builtins;
/// terminal class retirement uses the same callback-free storage law.
pub(crate) fn assert_spec_shape(bits: u64) {
    let pointer = obj_from_bits(bits)
        .as_ptr()
        .expect("sealed struct-sequence spec");
    unsafe {
        assert_eq!(object_type_id(pointer), TYPE_ID_TUPLE);
        crate::object::seq_access::with_immutable_tuple_slice(pointer, |fields| {
            assert_eq!(fields.len(), 6);
            for index in [0, 1] {
                let tuple = obj_from_bits(fields[index])
                    .as_ptr()
                    .expect("sealed metadata tuple");
                assert_eq!(object_type_id(tuple), TYPE_ID_TUPLE);
            }
            assert!(obj_from_bits(fields[2]).as_int().is_some());
            assert!(Constructor::decode(fields[3]).is_some());
        })
        .expect("sealed spec storage");
    }
}

struct Spec<'a, 'py> {
    names: PinnedTuple<'a, 'py>,
    docs: PinnedTuple<'a, 'py>,
    visible: usize,
    constructor: Constructor,
}
impl<'a, 'py> Spec<'a, 'py> {
    fn read(py: &'a PyToken<'py>, class: u64) -> Option<Self> {
        let Some(class) = obj_from_bits(class).as_ptr().filter(|&p| unsafe {
            object_type_id(p) == TYPE_ID_TYPE
                && crate::object::class_storage::class_declares(p, ClassDeclaration::StructSequence)
        }) else {
            return raise_exception(py, "TypeError", "expected a struct-sequence type");
        };
        let bits = unsafe { ClassReferenceSlot::StructSequence.load(class) };
        debug_assert!({
            assert_spec_shape(bits);
            true
        });
        unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(
                obj_from_bits(bits).as_ptr().unwrap(),
                |items| {
                    Some(Self {
                        names: pin_tuple(py, obj_from_bits(items[0]).as_ptr().unwrap())?,
                        docs: pin_tuple(py, obj_from_bits(items[1]).as_ptr().unwrap())?,
                        visible: obj_from_bits(items[2]).as_int().unwrap() as usize,
                        constructor: Constructor::decode(items[3]).unwrap(),
                    })
                },
            )
            .unwrap()
        }
    }
}

pub(crate) fn native_name(_py: &PyToken<'_>, class: u64) -> String {
    let ptr = obj_from_bits(class).as_ptr().unwrap();
    string_obj_to_owned(obj_from_bits(unsafe {
        crate::object::layout::class_native_name_bits(ptr)
    }))
    .unwrap()
}

fn owned<'a>(py: &'a PyToken<'_>, bits: u64) -> OwnedRuntimeValue<'a> {
    unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), bits) }
}
fn tuple(py: &PyToken<'_>, fields: &[u64]) -> u64 {
    let ptr = alloc_tuple(py, fields);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

unsafe fn hidden_offset(py: &PyToken<'_>, class: *mut u8, ordinal: usize) -> usize {
    unsafe {
        // Public descriptor names may repeat. Storage is indexed by the sealed
        // native slot declaration, independently of public namespace spelling.
        let declaration = crate::object::layout::class_slot_declaration_bits(class);
        let names = crate::object::seq_access::with_immutable_tuple_slice(
            obj_from_bits(declaration).as_ptr().unwrap(),
            |items| items[0],
        )
        .unwrap();
        let name = crate::object::seq_access::with_immutable_tuple_slice(
            obj_from_bits(names).as_ptr().unwrap(),
            |items| items[ordinal],
        )
        .unwrap();
        let map = obj_from_bits(crate::object::layout::class_field_offsets_bits(class))
            .as_ptr()
            .unwrap();
        let offset = dict_get_in_place(py, map, name)
            .expect("exact frozen layout key")
            .expect("declared hidden member");
        usize::try_from(obj_from_bits(offset).as_int().unwrap()).unwrap()
    }
}

/// Native providers supply their complete owned result values once. The visible
/// tuple and hidden fields retain independent references through existing owners.
pub(crate) fn from_values(py: &PyToken<'_>, class: u64, fields: &[u64]) -> u64 {
    let Some(spec) = Spec::read(py, class) else {
        return MoltObject::none().bits();
    };
    if fields.len() != spec.names.len() {
        return raise_exception(
            py,
            "SystemError",
            "native struct-sequence result has the wrong field count",
        );
    }
    let ptr = crate::object::builders::alloc_tuple_unpublished(
        py,
        &fields[..spec.visible],
        false,
        Some(class),
    );
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let ptr = unsafe { crate::object::native_instance::prepare_publication(py, ptr, class) };
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let class = obj_from_bits(class).as_ptr().unwrap();
    unsafe {
        for index in spec.visible..fields.len() {
            let offset = hidden_offset(py, class, index - spec.visible);
            let slot = crate::object::native_instance::field_base(ptr)
                .add(offset)
                .cast::<u64>();
            let value = fields[index];
            inc_ref_bits(py, value);
            let old = slot.replace(value);
            // Fresh fields hold the canonical immortal missing value.
            dec_ref_bits(py, old);
        }
        if fields[spec.visible..]
            .iter()
            .any(|&bits| obj_from_bits(bits).as_ptr().is_some())
        {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
    }
    unsafe {
        crate::object::gc::gc_publish_initialized(py, ptr);
    }
    MoltObject::from_ptr(ptr).bits()
}

/// C API allocation leaves every physical item uninitialized, including hidden
/// fields. Missing is represented as NULL by the one tuple projection owner.
pub(crate) fn new_uninitialized(py: &PyToken<'_>, class: u64) -> u64 {
    let Some(spec) = Spec::read(py, class) else {
        return MoltObject::none().bits();
    };
    let mut fields = Vec::new();
    if fields.try_reserve_exact(spec.names.len()).is_err() {
        return raise_exception(py, "MemoryError", "");
    }
    fields.resize(spec.names.len(), missing_bits(py));
    from_values(py, class, &fields)
}

/// Exact-class constructor policy; never inherited or inferred from namespace.
pub(crate) unsafe fn disallows_instantiation(_py: &PyToken<'_>, class: *mut u8) -> bool {
    unsafe {
        if !crate::object::class_storage::class_declares(class, ClassDeclaration::StructSequence) {
            return false;
        }
        let bits = ClassReferenceSlot::StructSequence.load(class);
        debug_assert!({
            assert_spec_shape(bits);
            true
        });
        crate::object::seq_access::with_immutable_tuple_slice(
            obj_from_bits(bits).as_ptr().unwrap(),
            |items| Constructor::decode(items[3]) == Some(Constructor::Forbidden),
        )
        .unwrap()
    }
}

unsafe fn read_field(py: &PyToken<'_>, object: *mut u8, spec: &Spec<'_, '_>, index: usize) -> u64 {
    unsafe {
        if index < spec.visible {
            crate::object::seq_access::with_immutable_tuple_slice(object, |values| values[index])
                .unwrap()
        } else {
            let class = obj_from_bits(object_class_bits(object)).as_ptr().unwrap();
            let offset = hidden_offset(py, class, index - spec.visible);
            *crate::object::native_instance::field_base(object)
                .add(offset)
                .cast::<u64>()
        }
    }
}

/// Complete physical extent for a tuple's exact native declaration. Ordinary
/// tuples have no hidden tail; class names and mutable n_fields are irrelevant.
pub(crate) unsafe fn hidden_len(py: &PyToken<'_>, object: *mut u8) -> usize {
    unsafe {
        let Some(class) = obj_from_bits(object_class_bits(object)).as_ptr() else {
            return 0;
        };
        if !crate::object::class_storage::class_declares(class, ClassDeclaration::StructSequence) {
            return 0;
        }
        let spec = Spec::read(py, MoltObject::from_ptr(class).bits())
            .expect("sealed struct-sequence declaration");
        spec.names.len() - spec.visible
    }
}

pub(crate) unsafe fn hidden_item(py: &PyToken<'_>, object: *mut u8, index: usize) -> Option<u64> {
    unsafe {
        let visible = tuple_len(object);
        if index < visible || index - visible >= hidden_len(py, object) {
            return None;
        }
        let class = obj_from_bits(object_class_bits(object)).as_ptr().unwrap();
        let offset = hidden_offset(py, class, index - visible);
        Some(
            *crate::object::native_instance::field_base(object)
                .add(offset)
                .cast::<u64>(),
        )
    }
}

/// The canonical C tuple transaction already owns the incoming edge. Transfer
/// the replaced runtime edge to that transaction; release never occurs here.
pub(crate) unsafe fn replace_hidden_item(
    py: &PyToken<'_>,
    object: *mut u8,
    index: usize,
    value: u64,
) -> Option<u64> {
    unsafe {
        let visible = tuple_len(object);
        if index < visible || index - visible >= hidden_len(py, object) {
            return None;
        }
        let class = obj_from_bits(object_class_bits(object)).as_ptr().unwrap();
        let offset = hidden_offset(py, class, index - visible);
        inc_ref_bits(py, value);
        let old = crate::object::native_instance::field_base(object)
            .add(offset)
            .cast::<u64>()
            .replace(value);
        if obj_from_bits(value).as_ptr().is_some() {
            (*header_from_obj_ptr(object)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
        Some(old)
    }
}

/// Only the exact sealed Time declaration admits timemodule's hidden fields.
/// Generic C-created struct sequences never gain this role from their names or
/// field geometry. The caller pins the tuple throughout conversion callbacks.
pub(crate) unsafe fn time_fields(py: &PyToken<'_>, object: *mut u8) -> Option<(u64, u64)> {
    unsafe {
        let class = obj_from_bits(object_class_bits(object)).as_ptr()?;
        if !crate::object::class_storage::class_declares(class, ClassDeclaration::StructSequence) {
            return None;
        }
        let spec = Spec::read(py, MoltObject::from_ptr(class).bits())?;
        if spec.constructor != Constructor::Time {
            return None;
        }
        Some((
            read_field(py, object, &spec, 9),
            read_field(py, object, &spec, 10),
        ))
    }
}

extern "C" fn member_get(descriptor: u64, instance: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let descriptor = obj_from_bits(descriptor).as_ptr().unwrap();
        let class = unsafe { crate::object::layout::native_descriptor_owner_bits(descriptor) };
        let Some(spec) = Spec::read(py, class) else {
            return MoltObject::none().bits();
        };
        let index = unsafe { crate::object::layout::native_descriptor_operation(descriptor) }
            .unwrap() as usize;
        let value =
            unsafe { read_field(py, obj_from_bits(instance).as_ptr().unwrap(), &spec, index) };
        inc_ref_bits(py, value);
        value
    })
}

fn set_attr(py: &PyToken<'_>, class: *mut u8, name: &[u8], value: u64) -> bool {
    let Some(key) = attr_name_bits_from_bytes(py, name) else {
        return false;
    };
    let _key = owned(py, key);
    unsafe {
        dict_set_in_place(
            py,
            obj_from_bits(class_dict_bits(class)).as_ptr().unwrap(),
            key,
            value,
        )
    }
}

extern "C" fn construct(declaring: u64, args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(args) = NativeArguments::read(py, "structseq", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(class) = args.new_receiver(py, declaring) else {
            return MoltObject::none().bits();
        };
        let Some(spec) = Spec::read(py, class) else {
            return MoltObject::none().bits();
        };
        let label = native_name(py, class);
        if spec.constructor == Constructor::Forbidden {
            return raise_exception(
                py,
                "TypeError",
                &format!("cannot create '{label}' instances"),
            );
        }
        let Some(bound) = args.named(py, &label, ["sequence", "dict"], 1) else {
            return MoltObject::none().bits();
        };
        let Some(sequence) = crate::object::iterable::materialize_sequence(
            py,
            bound[0].unwrap(),
            "constructor requires a sequence",
        ) else {
            return MoltObject::none().bits();
        };
        let dictionary = match bound[1] {
            None => None,
            Some(bits) => match crate::object::ops::dict_backing_bits(py, bits) {
                Ok(Some(backing)) => obj_from_bits(backing).as_ptr(),
                Ok(None) => {
                    return raise_exception(
                        py,
                        "TypeError",
                        &format!("{label}() takes a dict as second arg, if any"),
                    );
                }
                Err(()) => return MoltObject::none().bits(),
            },
        };
        let Some(values) = (unsafe {
            crate::object::seq_access::pin_sequence(
                py,
                obj_from_bits(sequence.bits()).as_ptr().unwrap(),
                "struct-sequence input allocation failed",
            )
        }) else {
            return MoltObject::none().bits();
        };
        let count = values.len();
        let max = spec.names.len();
        if count < spec.visible || count > max {
            let constraint = if max == spec.visible {
                format!("a {}", spec.visible)
            } else if count < spec.visible {
                format!("an at least {}", spec.visible)
            } else {
                format!("an at most {max}")
            };
            return raise_exception(
                py,
                "TypeError",
                &format!("{label}() takes {constraint}-sequence ({count}-sequence given)"),
            );
        }
        let mut fields = Vec::new();
        if fields.try_reserve_exact(max).is_err() {
            return raise_exception(py, "MemoryError", "");
        }
        fields.extend_from_slice(&values);
        fields.resize(max, MoltObject::none().bits());
        // Allocate and retain the result before dictionary key comparison can
        // call Python, matching PyStructSequence_New's constructor phase.
        let result = owned(py, from_values(py, class, &fields));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        drop(values);
        drop(sequence);
        let object = obj_from_bits(result.bits()).as_ptr().unwrap();
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        let mut found = 0usize;
        for index in count..max {
            let value = if let Some(dictionary) = dictionary {
                match unsafe { dict_get_in_place(py, dictionary, spec.names[index]) } {
                    Ok(Some(value)) => {
                        found += 1;
                        value
                    }
                    Ok(None) => MoltObject::none().bits(),
                    Err(()) => return MoltObject::none().bits(),
                }
            } else {
                MoltObject::none().bits()
            };
            unsafe {
                let offset = hidden_offset(py, class_ptr, index - spec.visible);
                let slot = crate::object::native_instance::field_base(object)
                    .add(offset)
                    .cast::<u64>();
                inc_ref_bits(py, value);
                let old = slot.replace(value);
                if obj_from_bits(value).as_ptr().is_some() {
                    (*header_from_obj_ptr(object))
                        .fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
                }
                dec_ref_bits(py, old);
            }
        }
        if crate::object::ops_sys::runtime_target_at_least(py, 3, 13)
            && dictionary.is_some_and(|p| unsafe { dict_len(p) } > found)
        {
            return raise_exception(
                py,
                "TypeError",
                &format!("{label}() got duplicate or unexpected field name(s)"),
            );
        }
        if spec.constructor == Constructor::Stat {
            for index in 0..3 {
                unsafe {
                    let offset = hidden_offset(py, class_ptr, index);
                    let slot = crate::object::native_instance::field_base(object)
                        .add(offset)
                        .cast::<u64>();
                    if obj_from_bits(*slot).is_none() {
                        let value = read_field(py, object, &spec, 7 + index);
                        inc_ref_bits(py, value);
                        let old = slot.replace(value);
                        if obj_from_bits(value).as_ptr().is_some() {
                            (*header_from_obj_ptr(object))
                                .fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
                        }
                        dec_ref_bits(py, old);
                    }
                }
            }
        }
        result.into_bits()
    })
}

extern "C" fn repr(instance: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let class = type_of_bits(py, instance);
        let Some(spec) = Spec::read(py, class) else {
            return MoltObject::none().bits();
        };
        let object = obj_from_bits(instance).as_ptr().unwrap();
        let mut output = native_name(py, class);
        output.push('(');
        // CPython's repr walks the descriptor names, omitting unnamed sequence
        // positions, while values remain the first visible_count tuple items.
        let mut names = spec
            .names
            .iter()
            .copied()
            .filter(|&n| !obj_from_bits(n).is_none());
        for index in 0..spec.visible {
            let Some(name) = names.next() else {
                return raise_exception(
                    py,
                    "SystemError",
                    &format!(
                        "In structseq_repr(), member {index} name is NULL for type {}",
                        native_name(py, class)
                    ),
                );
            };
            if index != 0 {
                output.push_str(", ");
            }
            output.push_str(&string_obj_to_owned(obj_from_bits(name)).unwrap());
            output.push('=');
            let value = unsafe { read_field(py, object, &spec, index) };
            let text = owned(py, molt_repr_from_obj(value));
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            output.push_str(&string_obj_to_owned(obj_from_bits(text.bits())).unwrap());
        }
        output.push(')');
        let ptr = alloc_string(py, output.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

extern "C" fn reduce(instance: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let class = type_of_bits(py, instance);
        let Some(spec) = Spec::read(py, class) else {
            return MoltObject::none().bits();
        };
        let object = obj_from_bits(instance).as_ptr().unwrap();
        let visible = owned(py, unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(object, |values| {
                tuple(py, values)
            })
            .unwrap()
        });
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let dictionary = alloc_dict_with_pairs(py, &[]);
        if dictionary.is_null() {
            return MoltObject::none().bits();
        }
        let dictionary_owner = owned(py, MoltObject::from_ptr(dictionary).bits());
        for index in spec.visible..spec.names.len() {
            let value = unsafe { read_field(py, object, &spec, index) };
            if !unsafe { dict_set_in_place(py, dictionary, spec.names[index], value) } {
                return MoltObject::none().bits();
            }
        }
        let arguments = owned(py, tuple(py, &[visible.bits(), dictionary_owner.bits()]));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        tuple(py, &[class, arguments.bits()])
    })
}

extern "C" fn replace(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        // METH_VARARGS|METH_KEYWORDS consumes the original keyword dictionary.
        // A binding snapshot would retain popped values beyond CPython's phase.
        let Some(positional) = obj_from_bits(args)
            .as_ptr()
            .and_then(|ptr| unsafe { pin_tuple(py, ptr) })
        else {
            return raise_exception(py, "TypeError", "__replace__() requires an arguments tuple");
        };
        let Some(&instance) = positional.first() else {
            return raise_exception(py, "TypeError", "__replace__() requires a receiver");
        };
        if positional.len() != 1 {
            return raise_exception(
                py,
                "TypeError",
                "__replace__() takes no positional arguments",
            );
        }
        let Some(dictionary) = obj_from_bits(kwargs)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) } == TYPE_ID_DICT)
        else {
            return raise_exception(
                py,
                "TypeError",
                "__replace__() requires a keyword dictionary",
            );
        };
        inc_ref_bits(py, kwargs);
        let _dictionary = owned(py, kwargs);
        let class = type_of_bits(py, instance);
        let Some(spec) = Spec::read(py, class) else {
            return MoltObject::none().bits();
        };
        if spec.names.iter().any(|&name| obj_from_bits(name).is_none()) {
            return raise_exception(
                py,
                "TypeError",
                &format!(
                    "__replace__() is not supported for {} because it has unnamed field(s)",
                    native_name(py, class)
                ),
            );
        }
        // The result owns each admitted edge before the next key comparison.
        // Its allocation precedes all pop callbacks, exactly as PyStructSequence_New.
        let result = owned(py, new_uninitialized(py, class));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let destination = obj_from_bits(result.bits()).as_ptr().unwrap();
        let object = obj_from_bits(instance).as_ptr().unwrap();
        for (index, &name) in spec.names.iter().enumerate() {
            let value = owned(
                py,
                molt_dict_pop(
                    kwargs,
                    name,
                    missing_bits(py),
                    MoltObject::from_int(1).bits(),
                ),
            );
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let bits = if value.bits() == missing_bits(py) {
                unsafe { read_field(py, object, &spec, index) }
            } else {
                value.bits()
            };
            let old = unsafe {
                if index < spec.visible {
                    crate::object::seq_access::replace_capi_item(py, destination, index, bits)
                } else {
                    replace_hidden_item(py, destination, index, bits)
                }
            }
            .expect("sealed newly allocated struct-sequence field");
            dec_ref_bits(py, old);
        }
        if unsafe { dict_len(dictionary) } != 0 {
            let names = unsafe {
                crate::object::ops_dict::dict_snapshot(
                    py,
                    dictionary,
                    crate::object::ops_dict::DictSnapshotKind::Keys,
                )
            };
            let Some(names) = names else {
                return MoltObject::none().bits();
            };
            let names = alloc_list(py, &names);
            if names.is_null() {
                return MoltObject::none().bits();
            }
            let names = owned(py, MoltObject::from_ptr(names).bits());
            let text = owned(py, molt_repr_from_obj(names.bits()));
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            return raise_exception(
                py,
                "TypeError",
                &format!(
                    "Got unexpected field name(s): {}",
                    string_obj_to_owned(obj_from_bits(text.bits())).unwrap()
                ),
            );
        }
        result.into_bits()
    })
}

/// Declaration-only boundary used by sys's one shared factory. Names/docs are
/// exact immutable tuples; public providers cannot alter the sealed spec later.
#[unsafe(no_mangle)]
pub extern "C" fn molt_structseq_type(
    name: u64,
    module: u64,
    names: u64,
    docs: u64,
    visible: u64,
    constructor: u64,
    static_origin: u64,
    doc: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if exact(py, name, TYPE_ID_STRING).is_none()
            || (!obj_from_bits(module).is_none() && exact(py, module, TYPE_ID_STRING).is_none())
            || obj_from_bits(static_origin).as_bool().is_none()
        {
            return raise_exception(
                py,
                "TypeError",
                "invalid native struct-sequence declaration",
            );
        }
        if !crate::object::layout::validate_class_name(py, name) {
            return MoltObject::none().bits();
        }
        let specification = owned(
            py,
            tuple(py, &[names, docs, visible, constructor, name, doc]),
        );
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if !valid_spec(py, specification.bits()) {
            return raise_exception(py, "TypeError", "invalid native struct-sequence fields");
        }
        let namespace = alloc_dict_with_pairs(py, &[]);
        if namespace.is_null() {
            return MoltObject::none().bits();
        }
        let namespace = owned(py, MoltObject::from_ptr(namespace).bits());
        let full_name = string_obj_to_owned(obj_from_bits(name)).unwrap();
        let short_name = full_name.rsplit('.').next().unwrap();
        let short_name = alloc_string(py, short_name.as_bytes());
        if short_name.is_null() {
            return MoltObject::none().bits();
        }
        let short_name = owned(py, MoltObject::from_ptr(short_name).bits());
        let class = crate::object::builders::alloc_class_obj_with_spec(
            py,
            short_name.bits(),
            namespace.bits(),
            Some(specification.bits()),
        );
        if class.is_null() {
            return MoltObject::none().bits();
        }
        let bits = MoltObject::from_ptr(class).bits();
        let class_owner = owned(py, bits);
        crate::molt_class_set_base(bits, builtin_classes(py).tuple);
        if exception_pending(py)
            || (!obj_from_bits(module).is_none() && !set_attr(py, class, b"__module__", module))
            || !set_attr(py, class, b"__doc__", doc)
        {
            return MoltObject::none().bits();
        }
        let Some(spec) = Spec::read(py, bits) else {
            return MoltObject::none().bits();
        };
        let unnamed = spec
            .names
            .iter()
            .filter(|&&name| obj_from_bits(name).is_none())
            .count();
        for (key, value) in [
            (b"n_fields".as_slice(), spec.names.len()),
            (b"n_sequence_fields".as_slice(), spec.visible),
            (b"n_unnamed_fields".as_slice(), unnamed),
        ] {
            if !set_attr(py, class, key, MoltObject::from_int(value as i64).bits()) {
                return MoltObject::none().bits();
            }
        }
        let mut hidden_names = Vec::new();
        if hidden_names
            .try_reserve_exact(spec.names.len() - spec.visible)
            .is_err()
        {
            return raise_exception(py, "MemoryError", "");
        }
        for index in spec.visible..spec.names.len() {
            let text = format!("_molt_structseq_field_{index:08x}");
            let pointer = alloc_string(py, text.as_bytes());
            if pointer.is_null() {
                return MoltObject::none().bits();
            }
            hidden_names.push(owned(py, MoltObject::from_ptr(pointer).bits()));
        }
        let slots = owned(
            py,
            tuple(py, OwnedRuntimeValue::as_bits_slice(&hidden_names)),
        );
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let slot_policy = ClassSlotPolicy {
            variable_sized: true,
            ..ClassSlotPolicy::default()
        };
        let declaration = owned(py, tuple(py, &[slots.bits(), slot_policy.encode()]));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        unsafe {
            inc_ref_bits(py, declaration.bits());
            crate::object::class_storage::class_declare_native_slots(class, slot_policy);
            crate::object::layout::class_set_slot_declaration_owned(class, declaration.bits());
            if crate::object::class_finish_definition(py, class).is_err() {
                return MoltObject::none().bits();
            }
        }
        // Native storage keys are not Python members; the public descriptors
        // below are the only namespace projection of the field declaration.
        for name in &hidden_names {
            if unsafe {
                crate::object::ops::dict_del_in_place(
                    py,
                    obj_from_bits(namespace.bits()).as_ptr().unwrap(),
                    name.bits(),
                )
            }
            .is_err()
            {
                return MoltObject::none().bits();
            }
        }
        let mut match_names = Vec::new();
        if match_names.try_reserve_exact(spec.visible).is_err() {
            return raise_exception(py, "MemoryError", "");
        }
        match_names.extend(
            spec.names[..spec.visible]
                .iter()
                .copied()
                .filter(|&name| !obj_from_bits(name).is_none()),
        );
        let match_names = owned(py, tuple(py, &match_names));
        if exception_pending(py) || !set_attr(py, class, b"__match_args__", match_names.bits()) {
            return MoltObject::none().bits();
        }
        let getter =
            crate::builtins::functions::alloc_runtime_function_obj(py, fn_addr!(member_get), 2);
        if getter.is_null() {
            return MoltObject::none().bits();
        }
        let getter = owned(py, MoltObject::from_ptr(getter).bits());
        for (index, (&field, &doc)) in spec.names.iter().zip(spec.docs.iter()).enumerate() {
            if obj_from_bits(field).is_none() {
                continue;
            }
            let descriptor = owned(
                py,
                crate::builtins::types::alloc_native_descriptor(
                    py,
                    crate::builtins::types::NativeDescriptorSpec {
                        flavor: crate::builtins::types::NativeDescriptorFlavor::Member,
                        operation: index as u32,
                        owner: bits,
                        name: field,
                        doc,
                        getter: getter.bits(),
                        setter: MoltObject::none().bits(),
                        deleter: MoltObject::none().bits(),
                    },
                ),
            );
            if exception_pending(py)
                || !unsafe {
                    dict_set_in_place(
                        py,
                        obj_from_bits(namespace.bits()).as_ptr().unwrap(),
                        field,
                        descriptor.bits(),
                    )
                }
            {
                return MoltObject::none().bits();
            }
        }
        if spec.constructor != Constructor::Forbidden {
            let function = crate::builtins::methods::builtin_variadic_func_bits(
                py,
                NativeCallableSpec::constructor(bits)
                    .with_text_signature("($type, sequence, dict={}, /)"),
                fn_addr!(construct),
            );
            if function == 0 || exception_pending(py) {
                return MoltObject::none().bits();
            }
            unsafe {
                crate::function_set_closure_bits(
                    py,
                    obj_from_bits(function).as_ptr().unwrap(),
                    bits,
                    crate::FunctionCallAbi::OpaqueContextFirst,
                );
            }
        }
        for (name, address) in [
            ("__repr__", fn_addr!(repr)),
            ("__reduce__", fn_addr!(reduce)),
        ] {
            let function = crate::builtins::methods::builtin_func_bits(
                py,
                NativeCallableSpec::declared(
                    if name == "__repr__" {
                        NativeCallableKind::WrapperDescriptor
                    } else {
                        NativeCallableKind::MethodDescriptor
                    },
                    bits,
                    name,
                ),
                address,
                1,
            );
            if function == 0 || exception_pending(py) {
                return MoltObject::none().bits();
            }
        }
        if crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
            let function = crate::builtins::methods::builtin_variadic_func_bits(
                py,
                NativeCallableSpec::declared(
                    NativeCallableKind::MethodDescriptor,
                    bits,
                    "__replace__",
                ),
                fn_addr!(replace),
            );
            if function == 0 || exception_pending(py) {
                return MoltObject::none().bits();
            }
        }
        unsafe {
            crate::object::class_storage::class_declare(
                class,
                ClassDeclaration::NativeNamespacePublished,
            );
            let is_static = obj_from_bits(static_origin).as_bool().unwrap();
            if !is_static {
                // The existing native module owner is also the creation-name
                // phase. Heap __name__ publication retires it after success;
                // public __module__ edits remain independent of native tp_name.
                if crate::object::class_storage::class_capture_native_module(py, class, false)
                    .is_err()
                {
                    return MoltObject::none().bits();
                }
            }
            let policy = if is_static {
                ClassSemanticPolicy::static_type(false)
            } else {
                ClassSemanticPolicy::heap(false, false)
            };
            if !policy.apply(py, class) {
                return MoltObject::none().bits();
            }
        }
        class_owner.into_bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_structseq_new(class: u64, values: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(values) = exact(py, values, TYPE_ID_TUPLE) else {
            return raise_exception(
                py,
                "TypeError",
                "native struct-sequence values must be an exact tuple",
            );
        };
        let Some(values) = (unsafe { pin_tuple(py, values) }) else {
            return MoltObject::none().bits();
        };
        from_values(py, class, &values)
    })
}

/// Copy the public C descriptor into the same immutable declaration consumed
/// by sys/time/os/pwd. Pointer identity alone marks unnamed fields; equal text
/// is a normal field name. C creation always selects generic sequence semantics.
pub(crate) unsafe fn from_c_description(
    py: &PyToken<'_>,
    desc: *const molt_cpython_abi::abi_types::PyStructSequence_Desc,
    static_origin: bool,
) -> u64 {
    unsafe { from_description(py, desc, static_origin, Constructor::Sequence) }
}

/// The os producer selects this role from its compile-target field declaration.
/// Public C descriptions always use generic sequence semantics.
pub(crate) unsafe fn from_stat_description(
    py: &PyToken<'_>,
    desc: *const molt_cpython_abi::abi_types::PyStructSequence_Desc,
) -> u64 {
    unsafe { from_description(py, desc, false, Constructor::Stat) }
}

unsafe fn from_description(
    py: &PyToken<'_>,
    desc: *const molt_cpython_abi::abi_types::PyStructSequence_Desc,
    static_origin: bool,
    constructor: Constructor,
) -> u64 {
    unsafe fn text(py: &PyToken<'_>, pointer: *const std::os::raw::c_char) -> u64 {
        if pointer.is_null() {
            return MoltObject::none().bits();
        }
        let bytes = unsafe { std::ffi::CStr::from_ptr(pointer) }.to_bytes();
        let value = match crate::object::ops::decode_bytes_text("utf-8", "strict", bytes) {
            Ok((value, _)) => value,
            Err(error) => return error.raise::<u64>(py, bytes),
        };
        let value = alloc_string(py, &value);
        if value.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(value).bits()
        }
    }
    unsafe {
        if desc.is_null()
            || (*desc).name.is_null()
            || (*desc).fields.is_null()
            || (*desc).n_in_sequence < 0
        {
            return raise_exception(py, "SystemError", "invalid struct-sequence descriptor");
        }
        let name = owned(py, text(py, (*desc).name));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let full_name = string_obj_to_owned(obj_from_bits(name.bits())).unwrap();
        let module = match full_name.rsplit_once('.') {
            Some((module, _)) => Some(module),
            None if static_origin => Some("builtins"),
            None => None,
        };
        let module = match module {
            None => owned(py, MoltObject::none().bits()),
            Some(module) => {
                let value = alloc_string(py, module.as_bytes());
                if value.is_null() {
                    return MoltObject::none().bits();
                }
                owned(py, MoltObject::from_ptr(value).bits())
            }
        };
        let doc = owned(py, text(py, (*desc).doc));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let mut names = Vec::new();
        let mut docs = Vec::new();
        let mut field = (*desc).fields;
        while !(*field).name.is_null() {
            if names.len() == u32::MAX as usize
                || names.try_reserve(1).is_err()
                || docs.try_reserve(1).is_err()
            {
                return raise_exception(py, "MemoryError", "");
            }
            let field_name = if (*field).name
                == molt_cpython_abi::api::sequences::PyStructSequence_UnnamedField
            {
                MoltObject::none().bits()
            } else {
                text(py, (*field).name)
            };
            names.push(owned(py, field_name));
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            docs.push(owned(py, text(py, (*field).doc)));
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            field = field.add(1);
        }
        let names = owned(py, tuple(py, OwnedRuntimeValue::as_bits_slice(&names)));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let docs = owned(py, tuple(py, OwnedRuntimeValue::as_bits_slice(&docs)));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        molt_structseq_type(
            name.bits(),
            module.bits(),
            names.bits(),
            docs.bits(),
            MoltObject::from_int((*desc).n_in_sequence as i64).bits(),
            MoltObject::from_int(constructor as i64).bits(),
            MoltObject::from_bool(static_origin).bits(),
            doc.bits(),
        )
    }
}
