//! Managed layout ancestry and sealed concrete field storage.
//!
//! Base admission follows one derived solid owner. A sealed class then owns one
//! immutable record: its attribute-offset projection and typed physical rows.
//! Neither consumers nor GC reconstruct offsets from ancestors or namespaces.

use super::class_storage::{ClassDeclaration, class_declares};
use super::layout::{ClassSlotDeclaration, class_slot_declaration};
use crate::object::ops_compare::string_storage_equal;
use crate::*;
use std::collections::{HashMap, HashSet};
use std::mem::size_of;

const WORD: usize = size_of::<u64>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScalarValueKind {
    Int,
    Float,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClassFieldKind {
    Inferred,
    DeclaredSlot,
    Intrinsic(ScalarValueKind),
}

impl ClassFieldKind {
    fn encode(self) -> u64 {
        MoltObject::from_int(match self {
            Self::Inferred => 0,
            Self::DeclaredSlot => 1,
            Self::Intrinsic(ScalarValueKind::Int) => 2,
            Self::Intrinsic(ScalarValueKind::Float) => 3,
        })
        .bits()
    }

    pub(crate) fn decode(bits: u64) -> Option<Self> {
        match obj_from_bits(bits).as_int()? {
            0 => Some(Self::Inferred),
            1 => Some(Self::DeclaredSlot),
            2 => Some(Self::Intrinsic(ScalarValueKind::Int)),
            3 => Some(Self::Intrinsic(ScalarValueKind::Float)),
            _ => None,
        }
    }

    pub(crate) fn is_declared_slot(self) -> bool {
        self == Self::DeclaredSlot
    }

    pub(crate) fn is_intrinsic(self) -> bool {
        matches!(self, Self::Intrinsic(_))
    }

    pub(crate) fn is_inferred(self) -> bool {
        self == Self::Inferred
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClassField {
    pub(crate) name: u64,
    pub(crate) offset: usize,
    pub(crate) kind: ClassFieldKind,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct NativeLayout {
    pub(crate) type_id: u32,
    pub(crate) shape: super::ObjectShapeId,
    pub(crate) exception: molt_obj_model::ExceptionLayoutRoot,
}

impl NativeLayout {
    const PLAIN: Self = Self {
        type_id: TYPE_ID_OBJECT,
        shape: super::ObjectShapeId::Plain,
        exception: molt_obj_model::ExceptionLayoutRoot::Base,
    };

    unsafe fn of(class: *mut u8) -> Self {
        unsafe {
            Self {
                type_id: super::class_instance_type_id(class),
                shape: super::class_instance_shape_id(class),
                exception: super::class_exception_layout_root(class),
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BestBase {
    pub(crate) direct: u64,
    pub(crate) owner: u64,
    pub(crate) native: NativeLayout,
}

/// Construction-time view of the selected direct base and secondary policy.
/// Pins outlive arbitrary __slots__ iteration; no descendant registry is kept.
pub(crate) struct SlotAdmission {
    base: Option<BestBase>,
    inherited: super::class_storage::ClassSlotPolicy,
    secondary: super::class_storage::ClassSlotPolicy,
    native: Option<super::class_storage::ClassSlotPolicy>,
    _pins: Vec<crate::PtrDropGuard>,
}

pub(crate) unsafe fn prepare_slot_admission(
    py: &PyToken<'_>,
    bases: &[u64],
    native: Option<super::class_storage::ClassSlotPolicy>,
) -> Result<SlotAdmission, ()> {
    unsafe {
        let mut pins = Vec::with_capacity(bases.len());
        for &base in bases {
            let Some(class) = obj_from_bits(base)
                .as_ptr()
                .filter(|&class| object_type_id(class) == TYPE_ID_TYPE)
            else {
                raise_exception::<()>(py, "TypeError", "base must be a type object");
                return Err(());
            };
            inc_ref_bits(py, base);
            pins.push(crate::PtrDropGuard::new(class));
        }
        let best = select_best_base(py, bases)?;
        let policy = |base| {
            super::layout::class_slot_policy(obj_from_bits(base).as_ptr().unwrap()).ok_or_else(
                || {
                    raise_exception::<()>(py, "TypeError", "base has no live slot policy");
                },
            )
        };
        let inherited = match best {
            Some(base) => policy(base.direct)?,
            None => super::class_storage::ClassSlotPolicy::default(),
        };
        let mut secondary = super::class_storage::ClassSlotPolicy::default();
        for &base in bases {
            if best.is_some_and(|best| best.direct == base) {
                continue;
            }
            let policy = policy(base)?;
            secondary.allows_dict |= policy.allows_dict;
            secondary.allows_weakref |= policy.allows_weakref;
        }
        Ok(SlotAdmission {
            base: best,
            inherited,
            secondary,
            native,
            _pins: pins,
        })
    }
}

impl SlotAdmission {
    /// CPython type_new_slots: materialization precedes this function; variable
    /// layout rejection precedes per-item validation, then special names are
    /// visited in declaration order. Ordinary duplicate names remain valid.
    pub(crate) unsafe fn validate(
        &self,
        py: &PyToken<'_>,
        names: Option<&[u64]>,
    ) -> Result<super::class_storage::ClassSlotPolicy, ()> {
        unsafe {
            let may_add_dict = !self.inherited.allows_dict;
            let may_add_weak = !self.inherited.allows_weakref && !self.inherited.variable_sized;
            let mut add_dict = false;
            let mut add_weak = false;
            if let Some(names) = names {
                if !names.is_empty() && self.inherited.variable_sized {
                    let base = self.base.expect("variable layout has a base");
                    let name = class_name_for_error(base.direct);
                    raise_exception::<()>(
                        py,
                        "TypeError",
                        &format!("nonempty __slots__ not supported for subtype of '{name}'"),
                    );
                    return Err(());
                }
                for &name in names {
                    let Some(ptr) = obj_from_bits(name)
                        .as_ptr()
                        .filter(|&ptr| object_type_id(ptr) == TYPE_ID_STRING)
                    else {
                        raise_exception::<()>(
                            py,
                            "TypeError",
                            &format!(
                                "__slots__ items must be strings, not '{}'",
                                type_name(py, obj_from_bits(name)),
                            ),
                        );
                        return Err(());
                    };
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    if !super::ops_string::is_identifier_bytes(bytes) {
                        raise_exception::<()>(py, "TypeError", "__slots__ must be identifiers");
                        return Err(());
                    }
                    if bytes == b"__dict__" {
                        if !may_add_dict || add_dict {
                            raise_exception::<()>(
                                py,
                                "TypeError",
                                "__dict__ slot disallowed: we already got one",
                            );
                            return Err(());
                        }
                        add_dict = true;
                    }
                    if bytes == b"__weakref__" {
                        if !may_add_weak || add_weak {
                            raise_exception::<()>(
                                py,
                                "TypeError",
                                "__weakref__ slot disallowed: we already got one",
                            );
                            return Err(());
                        }
                        add_weak = true;
                    }
                }
                add_dict |= may_add_dict && self.secondary.allows_dict;
                add_weak |= may_add_weak && self.secondary.allows_weakref;
            } else if self.native.is_none() {
                add_dict = may_add_dict;
                add_weak = may_add_weak;
            }
            let native = self.native.unwrap_or_default();
            Ok(super::class_storage::ClassSlotPolicy {
                allows_dict: self.inherited.allows_dict || native.allows_dict || add_dict,
                allows_weakref: self.inherited.allows_weakref || native.allows_weakref || add_weak,
                variable_sized: self.inherited.variable_sized || native.variable_sized,
            })
        }
    }
}

/// Borrow a real class's C view without invoking Python identity hooks. A
/// foreign wrapper is not a managed class layout: its native metatype owns
/// class admission, including metaclasses derived from `type`.
pub(crate) unsafe fn real_class_view(
    bits: u64,
) -> Result<Option<*mut molt_cpython_abi::abi_types::PyTypeObject>, ()> {
    unsafe {
        let Some(object) = obj_from_bits(bits).as_ptr() else {
            return Ok(None);
        };
        let value = match object_type_id(object) {
            TYPE_ID_TYPE => molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits),
            TYPE_ID_FOREIGN => {
                std::ptr::with_exposed_provenance_mut(super::foreign::foreign_ptr_from_obj(object))
            }
            _ => return Ok(None),
        };
        // A failed projection is not a negative class relation. Admission
        // callers retain the original error; pure inquiries suppress it.
        if value.is_null() {
            return Err(());
        }
        if molt_cpython_abi::api::typeobj::PyType_Check(value) == 0 {
            return Ok(None);
        }
        Ok(Some(value.cast()))
    }
}

/// Return one owned reference to the object's actual Python type. Foreign
/// values read the live native type on every call: their wrapper owns the C
/// object, not a cached managed class edge. No Python identity hooks run.
pub(crate) unsafe fn real_type_bits(py: &PyToken<'_>, instance: u64) -> Result<u64, ()> {
    unsafe {
        if let Some(class) = native_type_view(instance)? {
            return molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(class.cast())
                .ok_or(());
        }
        let class = type_of_bits(py, instance);
        if class == 0 {
            return Err(());
        }
        inc_ref_bits(py, class);
        Ok(class)
    }
}

unsafe fn foreign_value_view(bits: u64) -> Option<*mut molt_cpython_abi::abi_types::PyObject> {
    unsafe {
        let object = obj_from_bits(bits).as_ptr()?;
        (object_type_id(object) == TYPE_ID_FOREIGN).then(|| {
            std::ptr::with_exposed_provenance_mut(super::foreign::foreign_ptr_from_obj(object))
        })
    }
}

/// Borrow the live native type while the caller keeps the instance and GIL.
/// Predicates compare this view directly; only an escaping Python type result
/// acquires an owned runtime wrapper through real_type_bits.
unsafe fn native_type_view(
    bits: u64,
) -> Result<Option<*mut molt_cpython_abi::abi_types::PyTypeObject>, ()> {
    unsafe {
        let Some(value) = foreign_value_view(bits) else {
            return Ok(None);
        };
        let class = molt_cpython_abi::bridge::molt_capi_semantic_type(value);
        if class.is_null() {
            Err(())
        } else {
            Ok(Some(class))
        }
    }
}

/// Keep the existing managed MRO lane: sealed classes use their pinned tuple,
/// with no C projection or error-preservation transaction on the ordinary
/// managed exception/descriptor path.
unsafe fn managed_subtype_relation(py: &PyToken<'_>, subtype: u64, base: u64) -> Option<bool> {
    unsafe {
        let (Some(class), Some(base_ptr)) = (
            obj_from_bits(subtype).as_ptr(),
            obj_from_bits(base).as_ptr(),
        ) else {
            return Some(false);
        };
        (object_type_id(class) == TYPE_ID_TYPE && object_type_id(base_ptr) == TYPE_ID_TYPE)
            .then(|| subtype == base || class_mro_view(py, class).contains(&base))
    }
}

/// Structural inheritance shared by layout admission, descriptors and exception
/// matching. Managed pairs use their existing MRO without C projection; mixed
/// pairs use the canonical C type relation. Neither lane consults `__class__`
/// or metaclass instance/subclass hooks.
/// Failure leaves the projection error available to the admission caller.
pub(crate) unsafe fn try_is_real_subtype(
    py: &PyToken<'_>,
    subtype: u64,
    base: u64,
) -> Result<bool, ()> {
    unsafe {
        if let Some(result) = managed_subtype_relation(py, subtype, base) {
            return Ok(result);
        }
        let Some(class) = real_class_view(subtype)? else {
            return Ok(false);
        };
        let Some(base) = real_class_view(base)? else {
            return Ok(false);
        };
        Ok(molt_cpython_abi::api::typeobj::PyType_IsSubtype(class, base) != 0)
    }
}

/// A structural inquiry is error-neutral, including failed mixed projections.
/// Both independent incoming channels retain their exact state.
pub(crate) unsafe fn is_real_subtype(py: &PyToken<'_>, subtype: u64, base: u64) -> bool {
    if let Some(result) = unsafe { managed_subtype_relation(py, subtype, base) } {
        return result;
    }
    molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
        try_is_real_subtype(py, subtype, base).unwrap_or(false)
    })
}

/// Test a receiver's actual class without manufacturing a managed class edge
/// for a C-owned object or confusing its wrapper storage tag with Python type.
/// This establishes inheritance only; field access must also admit its layout.
pub(crate) unsafe fn try_is_real_instance(
    py: &PyToken<'_>,
    instance: u64,
    base: u64,
) -> Result<bool, ()> {
    unsafe {
        let Some(class) = native_type_view(instance)? else {
            return try_is_real_subtype(py, type_of_bits(py, instance), base);
        };
        let Some(base) = real_class_view(base)? else {
            return Ok(false);
        };
        Ok(molt_cpython_abi::api::typeobj::PyType_IsSubtype(class, base) != 0)
    }
}

pub(crate) unsafe fn is_real_instance(py: &PyToken<'_>, instance: u64, base: u64) -> bool {
    if unsafe { foreign_value_view(instance).is_none() } {
        return unsafe { is_real_subtype(py, type_of_bits(py, instance), base) };
    }
    molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
        try_is_real_instance(py, instance, base).unwrap_or(false)
    })
}

/// Dictionary/weakref declarations are policy, not additional physical slots.
unsafe fn is_storage_slot(name: u64) -> bool {
    unsafe {
        let ptr = obj_from_bits(name).as_ptr().expect("validated slot name");
        let text = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
        text != b"__dict__" && text != b"__weakref__"
    }
}

unsafe fn introduces_slots(py: &PyToken<'_>, class: *mut u8) -> Result<bool, ()> {
    unsafe {
        if !crate::builtins::attr::capture_class_slot_declaration_for_seal(py, class) {
            return Err(());
        }
        match class_slot_declaration(class) {
            ClassSlotDeclaration::Names(names) => {
                let names = obj_from_bits(names).as_ptr().expect("captured slot tuple");
                Ok(
                    super::seq_access::with_immutable_tuple_slice(names, |names| {
                        names.iter().copied().any(|name| is_storage_slot(name))
                    })
                    .expect("captured slot tuple"),
                )
            }
            ClassSlotDeclaration::Absent => Ok(false),
            ClassSlotDeclaration::Uninitialized => unreachable!("slot capture did not publish"),
        }
    }
}

#[derive(Default)]
struct BaseSelection {
    owners: HashMap<u64, u64>,
    active: HashSet<u64>,
}

impl BaseSelection {
    unsafe fn solid_owner(&mut self, py: &PyToken<'_>, bits: u64) -> Result<u64, ()> {
        unsafe {
            if let Some(&owner) = self.owners.get(&bits) {
                return Ok(owner);
            }
            if !self.active.insert(bits) {
                raise_exception::<()>(
                    py,
                    "TypeError",
                    "a __bases__ item causes an inheritance cycle",
                );
                return Err(());
            }
            let class = obj_from_bits(bits).as_ptr().expect("validated base");
            inc_ref_bits(py, bits);
            let _class_owner = crate::PtrDropGuard::new(class);
            let has_slots = introduces_slots(py, class)?;
            let bases = class_bases_vec(class_bases_bits(class));
            let best = self.select(py, &bases)?;
            let inherited = best.map_or(NativeLayout::PLAIN, |base| base.native);
            let introduces = class_declares(class, ClassDeclaration::IntrinsicLayout)
                || class_declares(class, ClassDeclaration::IntValue)
                || class_declares(class, ClassDeclaration::FloatValue)
                || NativeLayout::of(class) != inherited
                || has_slots;
            let owner = if introduces {
                bits
            } else {
                best.map_or(bits, |base| base.owner)
            };
            self.active.remove(&bits);
            self.owners.insert(bits, owner);
            Ok(owner)
        }
    }

    unsafe fn select(&mut self, py: &PyToken<'_>, bases: &[u64]) -> Result<Option<BestBase>, ()> {
        unsafe {
            // Slot declaration capture can iterate Python. Keep every candidate
            // alive before any capture can reenter a mutable construction graph.
            let mut pins = Vec::with_capacity(bases.len());
            for &bits in bases {
                let Some(class) = obj_from_bits(bits).as_ptr() else {
                    raise_exception::<()>(py, "TypeError", "base must be a type object");
                    return Err(());
                };
                if object_type_id(class) != TYPE_ID_TYPE {
                    raise_exception::<()>(py, "TypeError", "base must be a type object");
                    return Err(());
                }
                inc_ref_bits(py, bits);
                pins.push(crate::PtrDropGuard::new(class));
            }
            let mut best: Option<BestBase> = None;
            for &direct in bases {
                let class = obj_from_bits(direct).as_ptr().unwrap();
                let owner = self.solid_owner(py, direct)?;
                let candidate = BestBase {
                    direct,
                    owner,
                    native: NativeLayout::of(class),
                };
                best = Some(
                    match molt_obj_model::hierarchy::dominant_layout_base(
                        best.map(|winner| (winner, winner.owner)),
                        (candidate, owner),
                        |subclass, base| is_real_subtype(py, subclass, base),
                    ) {
                        Ok((selected, _)) => selected,
                        Err(_) => {
                            raise_exception::<()>(
                                py,
                                "TypeError",
                                "multiple bases have instance lay-out conflict",
                            );
                            return Err(());
                        }
                    },
                );
            }
            Ok(best)
        }
    }
}

/// Canonical physical owner for structural ABI metadata; C facade sizes do not
/// describe a managed class's slot/storage declarations.
pub(crate) unsafe fn class_solid_owner(py: &PyToken<'_>, class_bits: u64) -> Result<u64, ()> {
    unsafe { BaseSelection::default().solid_owner(py, class_bits) }
}

/// No retained owner cache: the direct-base graph and own layout declarations
/// are the only admission authority, including during builtin bootstrap.
pub(crate) unsafe fn select_best_base(
    py: &PyToken<'_>,
    bases: &[u64],
) -> Result<Option<BestBase>, ()> {
    unsafe { BaseSelection::default().select(py, bases) }
}

pub(crate) unsafe fn class_best_base(
    py: &PyToken<'_>,
    class: *mut u8,
) -> Result<Option<BestBase>, ()> {
    unsafe { select_best_base(py, &class_bases_vec(class_bases_bits(class))) }
}

/// The class owns this exact immutable tuple. Its two edges are the frozen
/// public name map (or None) and a flat tuple of (name, offset, kind) triples.
pub(crate) unsafe fn projection_parts(record: u64) -> (u64, u64) {
    unsafe {
        let record = obj_from_bits(record)
            .as_ptr()
            .expect("sealed physical record");
        super::seq_access::with_immutable_tuple_slice(record, |parts| {
            assert_eq!(parts.len(), 2, "invalid physical record");
            (parts[0], parts[1])
        })
        .expect("physical record must be an exact tuple")
    }
}

pub(crate) unsafe fn decode_row(row: &[u64]) -> ClassField {
    assert_eq!(row.len(), 3);
    ClassField {
        name: row[0],
        offset: obj_from_bits(row[1])
            .as_int()
            .and_then(|value| usize::try_from(value).ok())
            .expect("sealed field offset"),
        kind: ClassFieldKind::decode(row[2]).expect("sealed field kind"),
    }
}

/// Pure copied-result lookup in the sealed sorted rows. Callers already own a
/// live class or instance; no Python callback or reference-count release occurs.
pub(crate) unsafe fn field_at_offset(class: *mut u8, offset: usize) -> Option<ClassField> {
    unsafe {
        let (_, rows) = projection_parts(super::layout::class_field_layout_bits(class));
        let rows = obj_from_bits(rows).as_ptr().expect("sealed row tuple");
        super::seq_access::with_immutable_tuple_slice(rows, |rows| {
            assert_eq!(rows.len() % 3, 0, "invalid physical rows");
            let mut low = 0;
            let mut high = rows.len() / 3;
            while low < high {
                let mid = low + (high - low) / 2;
                let field = decode_row(&rows[mid * 3..mid * 3 + 3]);
                match field.offset.cmp(&offset) {
                    std::cmp::Ordering::Less => low = mid + 1,
                    std::cmp::Ordering::Greater => high = mid,
                    std::cmp::Ordering::Equal => return Some(field),
                }
            }
            None
        })
        .expect("sealed row tuple")
    }
}

/// Pin the complete record before handing out row/name borrows. A callback may
/// change __class__ and retire its old class while the visitor still runs.
pub(crate) unsafe fn for_each_field(
    py: &PyToken<'_>,
    class: *mut u8,
    visit: &mut dyn FnMut(ClassField),
) {
    unsafe {
        let record = super::layout::class_field_layout_bits(class);
        let record_ptr = obj_from_bits(record)
            .as_ptr()
            .expect("physical traversal requires sealed layout");
        inc_ref_bits(py, record);
        let _record_owner = crate::PtrDropGuard::new(record_ptr);
        let (_, rows) = projection_parts(record);
        let rows = obj_from_bits(rows).as_ptr().expect("sealed row tuple");
        super::seq_access::with_immutable_tuple_slice(rows, |rows| {
            assert_eq!(rows.len() % 3, 0, "invalid physical rows");
            for row in rows.as_chunks::<3>().0 {
                visit(decode_row(row));
            }
        })
        .expect("sealed row tuple");
    }
}

pub(crate) unsafe fn fields_match(py: &PyToken<'_>, left: *mut u8, right: *mut u8) -> bool {
    unsafe {
        let left = super::layout::class_field_layout_bits(left);
        let right = super::layout::class_field_layout_bits(right);
        inc_ref_bits(py, left);
        inc_ref_bits(py, right);
        let _left = crate::PtrDropGuard::new(obj_from_bits(left).as_ptr().unwrap());
        let _right = crate::PtrDropGuard::new(obj_from_bits(right).as_ptr().unwrap());
        let (_, left) = projection_parts(left);
        let (_, right) = projection_parts(right);
        super::seq_access::with_immutable_tuple_slice(
            obj_from_bits(left).as_ptr().unwrap(),
            |left| {
                super::seq_access::with_immutable_tuple_slice(
                    obj_from_bits(right).as_ptr().unwrap(),
                    |right| {
                        left.len() == right.len()
                            && left
                                .as_chunks::<3>()
                                .0
                                .iter()
                                .zip(right.as_chunks::<3>().0)
                                .all(|(left, right)| {
                                    let left = decode_row(left);
                                    let right = decode_row(right);
                                    left.offset == right.offset
                                        && left.kind == right.kind
                                        && (left.kind.is_intrinsic()
                                            || string_storage_equal(left.name, right.name))
                                })
                    },
                )
                .expect("sealed row tuple")
            },
        )
        .expect("sealed row tuple")
    }
}

pub(crate) struct PreparedLayout {
    record: u64,
    size: usize,
    record_owner: crate::PtrDropGuard,
    namespace: *mut u8,
    staged_namespace: *mut u8,
    map_publication: Option<(*mut u8, *mut u8)>,
    projection: Option<*mut u8>,
    // Commit moves displaced owners into these staged dictionaries. Release
    // only after class_finish_definition publishes the completed flag.
    _owners: Vec<crate::PtrDropGuard>,
}

impl PreparedLayout {
    pub(crate) unsafe fn publish(&mut self, py: &PyToken<'_>, class: *mut u8) {
        unsafe {
            if let Some((live, staged)) = self.map_publication {
                super::ops::dict_publish_staged(py, live, staged);
            }
            super::ops::dict_publish_staged(py, self.namespace, self.staged_namespace);
            if let Some(map) = self.projection {
                (*super::header_from_obj_ptr(map))
                    .fetch_or_flags(super::HEADER_FLAG_FROZEN_LAYOUT_MAP);
            }
            self.record_owner.release();
            super::layout::class_set_field_layout_owned(class, self.record);
            super::layout::class_set_cached_layout_size(class, self.size);
            crate::class_bump_layout_version(class);
        }
    }
}

fn overflow(py: &PyToken<'_>) {
    raise_exception::<()>(py, "OverflowError", "class instance layout is too large");
}

unsafe fn next_offset(py: &PyToken<'_>, rows: &[ClassField], prefix: usize) -> Result<usize, ()> {
    let end = rows
        .iter()
        .map(|field| field.offset)
        .max()
        .map_or(Some(prefix), |offset| {
            offset.checked_add(WORD).map(|end| end.max(prefix))
        });
    end.ok_or_else(|| overflow(py))
}

unsafe fn find_name(rows: &[ClassField], name: u64) -> Option<ClassField> {
    unsafe {
        rows.iter()
            .rev()
            .copied()
            .find(|field| string_storage_equal(field.name, name))
    }
}

/// Construct concrete storage once. Explicit compiler offsets remain fixed;
/// inherited inferred fields contribute names and receive placements in this
/// concrete class. Only the selected structural base supplies fixed slot owners.
pub(crate) unsafe fn prepare(py: &PyToken<'_>, class: *mut u8) -> Result<PreparedLayout, ()> {
    unsafe {
        let namespace = obj_from_bits(class_dict_bits(class))
            .as_ptr()
            .filter(|&ptr| object_type_id(ptr) == TYPE_ID_DICT)
            .ok_or_else(|| {
                raise_exception::<()>(
                    py,
                    "SystemError",
                    "class namespace is absent during sealing",
                );
            })?;
        let live_namespace = namespace;
        inc_ref_bits(py, MoltObject::from_ptr(live_namespace).bits());
        let mut owners = vec![crate::PtrDropGuard::new(live_namespace)];
        let staged_bits =
            super::ops_dict::molt_dict_copy(MoltObject::from_ptr(live_namespace).bits());
        let namespace = obj_from_bits(staged_bits).as_ptr().ok_or(())?;
        owners.push(crate::PtrDropGuard::new(namespace));
        if exception_pending(py) {
            return Err(());
        }
        let fields_name = intern_static_name(
            py,
            &runtime_state(py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let size_name = intern_static_name(
            py,
            &runtime_state(py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        if exception_pending(py) {
            return Err(());
        }
        let original = dict_get_in_place(py, namespace, fields_name)
            .filter(|&bits| !obj_from_bits(bits).is_none());
        if exception_pending(py) {
            return Err(());
        }
        let original_ptr = match original {
            Some(bits) => Some(
                obj_from_bits(bits)
                    .as_ptr()
                    .filter(|&ptr| object_type_id(ptr) == TYPE_ID_DICT)
                    .ok_or_else(|| {
                        raise_exception::<()>(
                            py,
                            "TypeError",
                            "__molt_field_offsets__ must be dict",
                        );
                    })?,
            ),
            None => None,
        };
        if let Some(bits) = original {
            inc_ref_bits(py, bits);
        }
        let _original_owner = original_ptr.map(crate::PtrDropGuard::new);
        let size_hint = dict_get_in_place(py, namespace, size_name)
            .and_then(|bits| obj_from_bits(bits).as_int())
            .filter(|&size| size > 0)
            .map(|size| usize::try_from(size).map_err(|_| overflow(py)))
            .transpose()?;
        if exception_pending(py) {
            return Err(());
        }
        let mut prefix = super::class_reserved_layout_prefix(class);
        let tail = super::class_reserved_layout_tail(py, class);
        let best = class_best_base(py, class)?;
        let shape = super::class_instance_shape_id(class);
        let native_extension = super::native_instance::NativePayload::from_type_id(
            super::class_instance_type_id(class),
        )
        .is_some();
        let mut rows = Vec::new();
        let mut inherited_size = prefix.checked_add(tail).ok_or_else(|| overflow(py))?;
        let mut inherited_shape_prefix = None;
        if let Some(best) = best {
            let base = obj_from_bits(best.direct).as_ptr().unwrap();
            assert!(
                super::class_definition_is_finished(base),
                "selected base must be sealed"
            );
            let base_size = super::layout::class_cached_layout_size(base).unwrap();
            inherited_size = inherited_size.max(base_size);
            let mut first_field: Option<usize> = None;
            for_each_field(py, base, &mut |field| {
                first_field =
                    Some(first_field.map_or(field.offset, |offset| offset.min(field.offset)));
                if !field.kind.is_inferred() {
                    rows.push(field);
                }
            });
            if shape != super::ObjectShapeId::Plain && shape == best.native.shape {
                // Opaque shape words precede the first concrete field. A root
                // with no fields reserves its entire payload before the tail.
                inherited_shape_prefix = Some(first_field.unwrap_or_else(|| {
                    base_size.saturating_sub(super::class_reserved_layout_tail(py, base))
                }));
            }
        }
        if shape != super::ObjectShapeId::Plain {
            prefix = prefix.max(
                inherited_shape_prefix
                    .unwrap_or_else(|| size_hint.unwrap_or(inherited_size).saturating_sub(tail)),
            );
        }
        let scalar = if class_declares(class, ClassDeclaration::IntValue) {
            Some(ScalarValueKind::Int)
        } else if class_declares(class, ClassDeclaration::FloatValue) {
            Some(ScalarValueKind::Float)
        } else {
            None
        };
        if let Some(scalar) = scalar {
            assert!(
                rows.is_empty() && prefix == 0,
                "scalar root must own the initial tagged word"
            );
            rows.push(ClassField {
                name: MoltObject::none().bits(),
                offset: 0,
                kind: ClassFieldKind::Intrinsic(scalar),
            });
        }
        let mut own_slots = match class_slot_declaration(class) {
            ClassSlotDeclaration::Names(names) => super::seq_access::with_immutable_tuple_slice(
                obj_from_bits(names).as_ptr().unwrap(),
                |names| {
                    names
                        .iter()
                        .copied()
                        .filter(|&name| is_storage_slot(name))
                        .collect::<Vec<_>>()
                },
            )
            .unwrap(),
            ClassSlotDeclaration::Absent => Vec::new(),
            ClassSlotDeclaration::Uninitialized => unreachable!("seal must capture slots first"),
        };
        // Physical slot order follows names, independent of source declaration
        // order. The captured declaration tuple still governs getstate order.
        own_slots.sort_by(|&left, &right| {
            let left = obj_from_bits(left).as_ptr().unwrap();
            let right = obj_from_bits(right).as_ptr().unwrap();
            std::slice::from_raw_parts(string_bytes(left), string_len(left)).cmp(
                std::slice::from_raw_parts(string_bytes(right), string_len(right)),
            )
        });
        let own_slot = |name| {
            own_slots
                .iter()
                .copied()
                .any(|slot| string_storage_equal(slot, name))
        };
        let mut slot_hints = Vec::new();
        if let Some(offsets) = original_ptr {
            let extent = size_hint.map_or(usize::MAX, |size| size.saturating_sub(tail));
            super::validate_class_field_offsets(py, offsets, prefix, extent)?;
            for pair in dict_order(offsets).as_chunks::<2>().0 {
                let field = ClassField {
                    name: pair[0],
                    offset: usize::try_from(obj_from_bits(pair[1]).as_int().unwrap()).unwrap(),
                    kind: if own_slot(pair[0]) {
                        ClassFieldKind::DeclaredSlot
                    } else {
                        ClassFieldKind::Inferred
                    },
                };
                if field.kind.is_declared_slot() {
                    slot_hints.push(field);
                    continue;
                }
                if native_extension {
                    // Compiler-inferred attributes on native payloads are
                    // dictionary-backed. Only declared slots occupy the tail.
                    continue;
                }
                if let Some(inherited) = find_name(&rows, field.name) {
                    if inherited.offset != field.offset {
                        raise_exception::<()>(
                            py,
                            "ValueError",
                            "explicit field offset disagrees with inherited fixed storage",
                        );
                        return Err(());
                    }
                    continue;
                }
                if rows.iter().any(|existing| existing.offset == field.offset) {
                    raise_exception::<()>(
                        py,
                        "ValueError",
                        "explicit field offset overlaps inherited fixed storage",
                    );
                    return Err(());
                }
                rows.push(field);
            }
        }
        // These are sealed concrete rows, never raw ancestor offset maps.
        // Inferred fields are ordinary attribute names, not inherited addresses.
        let mro = class_mro_view(py, class);
        for bits in mro.iter().copied().skip(1) {
            if native_extension {
                break;
            }
            let base = obj_from_bits(bits).as_ptr().expect("class MRO entry");
            let mut names = Vec::new();
            for_each_field(py, base, &mut |field| {
                if field.kind.is_inferred() {
                    names.push(field.name);
                }
            });
            for name in names {
                if own_slot(name) || find_name(&rows, name).is_some() {
                    continue;
                }
                let offset = next_offset(py, &rows, prefix)?;
                rows.push(ClassField {
                    name,
                    offset,
                    kind: ClassFieldKind::Inferred,
                });
            }
        }
        // Keep every physical declaration even when its public name is hidden
        // by an own declaration. Duplicate own declarations also remain owners.
        for name in own_slots.iter().copied() {
            let hint = slot_hints
                .iter()
                .copied()
                .find(|field| string_storage_equal(field.name, name));
            let offset = if let Some(hint) = hint {
                if rows.iter().any(|field| field.offset == hint.offset) {
                    raise_exception::<()>(
                        py,
                        "ValueError",
                        "explicit slot offset overlaps inherited storage",
                    );
                    return Err(());
                }
                hint.offset
            } else {
                next_offset(py, &rows, prefix)?
            };
            rows.push(ClassField {
                name,
                offset,
                kind: ClassFieldKind::DeclaredSlot,
            });
        }
        let required = next_offset(py, &rows, prefix)?
            .checked_add(tail)
            .ok_or_else(|| overflow(py))?;
        let size = required
            .max(inherited_size)
            .max(if native_extension {
                0
            } else {
                size_hint.unwrap_or(0)
            })
            .max(WORD);
        i64::try_from(size).map_err(|_| overflow(py))?;
        // Physical ordering is stable for initialization, GC and assignment.
        // Save visible name choices before sorting: own slots override inherited.
        let mut visible: Vec<ClassField> = Vec::new();
        for field in rows.iter().filter(|field| !field.kind.is_intrinsic()) {
            if let Some(current) = visible
                .iter_mut()
                .find(|current| string_storage_equal(current.name, field.name))
            {
                *current = *field;
            } else {
                visible.push(*field);
            }
        }
        rows.sort_unstable_by_key(|field| field.offset);
        // The public map keeps its original identity. All normalization occurs
        // in a staged exact dictionary; seal publishes it without releases.
        let map = if let Some(bits) = original {
            if (*super::header_from_obj_ptr(original_ptr.unwrap()))
                .has_flag(super::HEADER_FLAG_FROZEN_LAYOUT_MAP)
            {
                raise_exception::<()>(py, "TypeError", "class field map is already sealed");
                return Err(());
            }
            bits
        } else if visible.is_empty() {
            MoltObject::none().bits()
        } else {
            let map = alloc_dict_with_pairs(py, &[]);
            if map.is_null() {
                return Err(());
            }
            let bits = MoltObject::from_ptr(map).bits();
            owners.push(crate::PtrDropGuard::new(map));
            bits
        };
        let projection = obj_from_bits(map).as_ptr();
        let mut map_publication = None;
        if let Some(map_ptr) = projection {
            let staged_bits = super::ops_dict::molt_dict_copy(map);
            let staged = obj_from_bits(staged_bits).as_ptr().ok_or(())?;
            owners.push(crate::PtrDropGuard::new(staged));
            if exception_pending(py) {
                return Err(());
            }
            for field in visible {
                let offset = i64::try_from(field.offset).map_err(|_| overflow(py))?;
                dict_set_in_place(py, staged, field.name, MoltObject::from_int(offset).bits());
                if exception_pending(py) {
                    return Err(());
                }
            }
            super::validate_class_field_offsets(py, staged, prefix, size - tail)?;
            map_publication = Some((map_ptr, staged));
        }
        // A member descriptor owns the declaring class and resolves only that
        // class's immutable slot row. No function address or registry is added.
        let class_bits = MoltObject::from_ptr(class).bits();
        let mut published_names: Vec<u64> = Vec::new();
        for name in own_slots {
            if published_names
                .iter()
                .any(|&seen| string_storage_equal(seen, name))
            {
                continue;
            }
            let descriptor = crate::builtins::types::alloc_native_descriptor(
                py,
                crate::builtins::types::NativeDescriptorSpec {
                    flavor: super::layout::NativeDescriptorFlavor::ManagedSlot,
                    operation: 0,
                    owner: class_bits,
                    name,
                    doc: MoltObject::none().bits(),
                    getter: MoltObject::none().bits(),
                    setter: MoltObject::none().bits(),
                    deleter: MoltObject::none().bits(),
                },
            );
            let descriptor_ptr = obj_from_bits(descriptor).as_ptr().ok_or(())?;
            let _descriptor_owner = crate::PtrDropGuard::new(descriptor_ptr);
            if exception_pending(py) {
                return Err(());
            }
            dict_set_in_place(py, namespace, name, descriptor);
            if exception_pending(py) {
                return Err(());
            }
            published_names.push(name);
        }
        let policy = super::layout::class_slot_policy(class).expect("captured slot policy");
        if policy.allows_dict
            && !best.is_some_and(|base| {
                super::layout::class_slot_policy(obj_from_bits(base.direct).as_ptr().unwrap())
                    .is_some_and(|policy| policy.allows_dict)
            })
        {
            let name = intern_static_name(py, &runtime_state(py).interned.dict_name, b"__dict__");
            if exception_pending(py) {
                return Err(());
            }
            if dict_get_in_place(py, namespace, name).is_none() {
                let descriptor = crate::builtins::types::alloc_native_descriptor(
                    py,
                    crate::builtins::types::NativeDescriptorSpec {
                        flavor: super::layout::NativeDescriptorFlavor::InstanceDictionary,
                        operation: 0,
                        owner: class_bits,
                        name,
                        doc: MoltObject::none().bits(),
                        getter: MoltObject::none().bits(),
                        setter: MoltObject::none().bits(),
                        deleter: MoltObject::none().bits(),
                    },
                );
                let descriptor_ptr = obj_from_bits(descriptor).as_ptr().ok_or(())?;
                let _descriptor_owner = crate::PtrDropGuard::new(descriptor_ptr);
                if exception_pending(py) {
                    return Err(());
                }
                dict_set_in_place(py, namespace, name, descriptor);
                if exception_pending(py) {
                    return Err(());
                }
            }
        }
        let mut encoded = Vec::with_capacity(rows.len() * 3);
        for field in &rows {
            encoded.extend([
                field.name,
                MoltObject::from_int(i64::try_from(field.offset).map_err(|_| overflow(py))?).bits(),
                field.kind.encode(),
            ]);
        }
        let rows_ptr = alloc_tuple(py, &encoded);
        if rows_ptr.is_null() {
            return Err(());
        }
        let _rows_owner = crate::PtrDropGuard::new(rows_ptr);
        let record_ptr = alloc_tuple(py, &[map, MoltObject::from_ptr(rows_ptr).bits()]);
        if record_ptr.is_null() {
            return Err(());
        }
        let record_owner = crate::PtrDropGuard::new(record_ptr);
        // Construction hints are consumed into the private record/cache.
        // They never survive publication as Python class attributes.
        dict_del_in_place(py, namespace, fields_name);
        dict_del_in_place(py, namespace, size_name);
        if exception_pending(py) {
            return Err(());
        }
        Ok(PreparedLayout {
            record: MoltObject::from_ptr(record_ptr).bits(),
            size,
            record_owner,
            namespace: live_namespace,
            staged_namespace: namespace,
            map_publication,
            projection,
            _owners: owners,
        })
    }
}

/// The sealed row is the shared scalar storage authority. An unfinished class
/// has no projection, and an ordinary declared/inferred word is never scalar.
unsafe fn scalar_value_field(class: *mut u8, kind: ScalarValueKind) -> Option<ClassField> {
    unsafe {
        if object_type_id(class) != TYPE_ID_TYPE
            || super::layout::class_field_layout_bits(class) == 0
        {
            return None;
        }
        field_at_offset(class, 0).filter(|field| field.kind == ClassFieldKind::Intrinsic(kind))
    }
}

/// Borrow the tagged intrinsic value from a managed scalar instance. Physical
/// kind and extent checks protect the read; the inherited typed row establishes
/// which scalar payload the word contains. No MRO scan, callback, or ownership
/// transfer occurs. Native float carriers are projected by their native reader.
pub(crate) fn scalar_value_bits(obj: MoltObject, kind: ScalarValueKind) -> Option<u64> {
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_OBJECT {
            return None;
        }
        let class = obj_from_bits(object_class_bits(ptr)).as_ptr()?;
        let field = scalar_value_field(class, kind)?;
        if field.offset.checked_add(WORD)? > super::object_payload_size(ptr) {
            return None;
        }
        Some(ptr.add(field.offset).cast::<u64>().read())
    }
}

/// Scalar constructors initialize the same typed intrinsic word read above.
/// Its name is deliberately None, so user fields cannot redirect the payload.
pub(crate) unsafe fn scalar_value_offset(
    py: &PyToken<'_>,
    class: *mut u8,
    kind: ScalarValueKind,
) -> Option<usize> {
    unsafe {
        let field = scalar_value_field(class, kind);
        if field.is_none() {
            raise_exception::<()>(
                py,
                "SystemError",
                "scalar class has no matching intrinsic value word",
            );
        }
        field.map(|field| field.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_type_relation_admits_native_metaclasses_without_confusing_values_and_classes() {
        use molt_cpython_abi::abi_types::*;
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let mut meta: PyTypeObject = std::mem::zeroed();
                meta.ob_base.ob_base.ob_refcnt = 1;
                meta.ob_base.ob_base.ob_type = &raw mut PyType_Type;
                meta.tp_base = &raw mut PyType_Type;
                meta.tp_name = c"NativeIdentityMeta".as_ptr();
                meta.tp_is_gc = PyType_Type.tp_is_gc;
                let mut class: PyTypeObject = std::mem::zeroed();
                class.ob_base.ob_base.ob_refcnt = 1;
                class.ob_base.ob_base.ob_type = &raw mut meta;
                class.tp_base = &raw mut PyBaseObject_Type;
                class.tp_name = c"NativeIdentityObject".as_ptr();
                class.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
                let mut value = PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut class,
                };
                let class_bits = GLOBAL_BRIDGE
                    .molt_value_for_pyobj((&raw mut class).cast())
                    .unwrap();
                let value_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(&raw mut value).unwrap();
                let builtins = builtin_classes(py);
                assert!(is_real_subtype(py, class_bits, builtins.object));
                assert!(!is_real_subtype(py, class_bits, builtins.int));
                assert!(!is_real_subtype(py, builtins.object, class_bits));
                assert!(is_real_instance(py, value_bits, class_bits));
                assert!(is_real_instance(py, value_bits, builtins.object));
                assert!(is_real_instance(py, class_bits, builtins.type_obj));
                assert!(!is_real_subtype(py, value_bits, value_bits));
                let integer = MoltObject::from_int(1).bits();
                assert!(!is_real_subtype(py, integer, integer));
                assert!(is_real_instance(py, integer, builtins.int));
                assert!(!exception_pending(py));
                assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
                dec_ref_bits(py, value_bits);
                dec_ref_bits(py, class_bits);
                assert_eq!(value.ob_refcnt, 1);
                assert_eq!(class.ob_base.ob_base.ob_refcnt, 1);
                assert_eq!(meta.ob_base.ob_base.ob_refcnt, 1);
            }
        });
    }

    static REENTERED_SLOT_CLASS: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    extern "C" fn slot_capture_reenter(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let target = REENTERED_SLOT_CLASS.load(std::sync::atomic::Ordering::SeqCst);
                let class = obj_from_bits(target).as_ptr().unwrap();
                let namespace = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
                let name = attr_name_bits_from_bytes(py, b"__slots__").unwrap();
                let empty = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
                dict_set_in_place(py, namespace, name, empty);
                assert!(crate::builtins::attr::capture_class_slot_declaration_for_seal(py, class));
                let iterator = crate::molt_iter(empty);
                dec_ref_bits(py, name);
                dec_ref_bits(py, empty);
                iterator
            }
        })
    }

    unsafe fn slotted_class(py: &PyToken<'_>, label: &[u8], base: u64, slots: &[&[u8]]) -> u64 {
        unsafe {
            let name = attr_name_bits_from_bytes(py, label).unwrap();
            let class = molt_class_new(name);
            dec_ref_bits(py, name);
            let result = molt_class_set_base(class, base);
            dec_ref_bits(py, result);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let namespace = obj_from_bits(class_dict_bits(class_ptr)).as_ptr().unwrap();
            let slots_name = attr_name_bits_from_bytes(py, b"__slots__").unwrap();
            let names: Vec<u64> = slots
                .iter()
                .map(|name| attr_name_bits_from_bytes(py, name).unwrap())
                .collect();
            let declaration = alloc_tuple(py, &names);
            for name in names {
                dec_ref_bits(py, name);
            }
            dict_set_in_place(
                py,
                namespace,
                slots_name,
                MoltObject::from_ptr(declaration).bits(),
            );
            dec_ref_bits(py, slots_name);
            dec_ref_bits(py, MoltObject::from_ptr(declaration).bits());
            super::super::class_finish_definition(py, class_ptr).expect("sealed test class");
            assert!(!exception_pending(py));
            class
        }
    }

    #[test]
    fn static_and_dynamic_slot_admission_share_sealed_native_policy() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let native = builtin_classes(py);
                let cached_exception =
                    crate::builtins::exceptions::exception_type_bits_from_name(py, "ValueError");
                let cached_exception = obj_from_bits(cached_exception).as_ptr().unwrap();
                let published_policy =
                    crate::builtins::attr::class_slots_info(py, cached_exception)
                        .expect("native cache publication admits slots");
                assert!(published_policy.allows_dict && !published_policy.allows_weakref);
                let name = attr_name_bits_from_bytes(py, b"AdmissionPolicy").unwrap();
                let slots = attr_name_bits_from_bytes(py, b"__slots__").unwrap();
                let dict = attr_name_bits_from_bytes(py, b"__dict__").unwrap();
                let weak = attr_name_bits_from_bytes(py, b"__weakref__").unwrap();
                let unfinished = molt_class_new(name);
                let unfinished_ptr = obj_from_bits(unfinished).as_ptr().unwrap();
                assert!(crate::builtins::attr::class_slots_info(py, unfinished_ptr).is_none());
                for attribute in [dict, weak] {
                    assert_eq!(
                        crate::builtins::attr::class_instance_layout_attr_allowed(
                            py,
                            unfinished_ptr,
                            attribute,
                        ),
                        Some(false),
                        "bootstrap absence cannot grant instance storage"
                    );
                }
                dec_ref_bits(py, unfinished);
                let empty = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
                let admitted = molt_class_new(name);
                let admitted_ptr = obj_from_bits(admitted).as_ptr().unwrap();
                let namespace = obj_from_bits(class_dict_bits(admitted_ptr))
                    .as_ptr()
                    .unwrap();
                dict_set_in_place(py, namespace, slots, empty);
                assert!(
                    crate::builtins::attr::capture_class_slot_declaration_for_seal(
                        py,
                        admitted_ptr,
                    )
                );
                let record = super::super::layout::class_slot_declaration_bits(admitted_ptr);
                // Admission seals the hierarchy, even before physical layout
                // publication; rebinding names cannot reopen construction.
                let original_bases = class_bases_bits(admitted_ptr);
                let changed = MoltObject::from_ptr(alloc_tuple(py, &[weak])).bits();
                dict_set_in_place(py, namespace, slots, changed);
                dec_ref_bits(py, changed);
                for base in [native.object, native.set] {
                    let result = molt_class_set_base(admitted, base);
                    dec_ref_bits(py, result);
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "TypeError",
                    ));
                    assert_eq!(
                        crate::builtins::exceptions::format_exception_message(
                            py,
                            obj_from_bits(error).as_ptr().unwrap(),
                        ),
                        "class bases are immutable after slot admission"
                    );
                    clear_exception(py);
                    dec_ref_bits(py, error);
                }
                assert_eq!(class_bases_bits(admitted_ptr), original_bases);
                assert_eq!(
                    super::super::layout::class_slot_declaration_bits(admitted_ptr),
                    record
                );
                dec_ref_bits(py, admitted);
                let both = MoltObject::from_ptr(alloc_tuple(py, &[dict, weak])).bits();
                // Bit 0: dict; bit 1: weakrefs; bit 2: variable-size layout.
                for (base, declaration, expected) in [
                    (native.object, None, 3),
                    (native.object, Some(empty), 0),
                    (native.list, Some(both), 3),
                    (native.int, None, 5),
                    (native.int, Some(empty), 4),
                    (native.bytes, Some(empty), 4),
                    (native.tuple, Some(empty), 4),
                    (native.set, Some(empty), 2),
                    (native.base_exception, Some(empty), 1),
                    (native.module, Some(empty), 3),
                    (native.type_obj, Some(empty), 7),
                ] {
                    let namespace = alloc_dict_with_pairs(py, &[]);
                    if let Some(declaration) = declaration {
                        dict_set_in_place(py, namespace, slots, declaration);
                    }
                    let static_class = molt_class_new(name);
                    let static_ptr = obj_from_bits(static_class).as_ptr().unwrap();
                    let result = molt_class_set_base(static_class, base);
                    dec_ref_bits(py, result);
                    if let Some(declaration) = declaration {
                        dict_set_in_place(
                            py,
                            obj_from_bits(class_dict_bits(static_ptr)).as_ptr().unwrap(),
                            slots,
                            declaration,
                        );
                    }
                    super::super::class_finish_definition(py, static_ptr)
                        .expect("static slot admission");
                    let bases = MoltObject::from_ptr(alloc_tuple(py, &[base])).bits();
                    let dynamic_class = crate::builtins::types::molt_type_new(
                        native.type_obj,
                        name,
                        bases,
                        MoltObject::from_ptr(namespace).bits(),
                        MoltObject::none().bits(),
                    );
                    assert!(!exception_pending(py), "dynamic slot admission");
                    let dynamic_ptr = obj_from_bits(dynamic_class).as_ptr().unwrap();
                    let expected = MoltObject::from_int(expected).bits();
                    for class in [static_ptr, dynamic_ptr] {
                        let record = super::super::layout::class_slot_declaration_bits(class);
                        let policy = crate::builtins::attr::class_slots_info(py, class).unwrap();
                        assert_eq!(policy.encode(), expected);
                        let namespace = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
                        dict_set_in_place(py, namespace, slots, empty);
                        assert_eq!(
                            super::super::layout::class_slot_declaration_bits(class),
                            record
                        );
                        assert_eq!(
                            crate::builtins::attr::class_slots_info(py, class),
                            Some(policy)
                        );
                    }
                    for bits in [
                        static_class,
                        dynamic_class,
                        bases,
                        MoltObject::from_ptr(namespace).bits(),
                    ] {
                        dec_ref_bits(py, bits);
                    }
                }
                for bits in [name, slots, dict, weak, empty, both] {
                    dec_ref_bits(py, bits);
                }
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn reentrant_slot_capture_preserves_the_first_publication() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let iterable = slotted_class(
                    py,
                    b"ReentrantSlotIterable",
                    builtin_classes(py).object,
                    &[],
                );
                let iterable_ptr = obj_from_bits(iterable).as_ptr().unwrap();
                let iterator_name = attr_name_bits_from_bytes(py, b"__iter__").unwrap();
                let function = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::builtins::functions::runtime_fn_addr(
                        "slot_capture_reenter",
                        slot_capture_reenter as *const (),
                    ),
                    1,
                );
                let function = MoltObject::from_ptr(function).bits();
                crate::molt_set_attr_name(iterable, iterator_name, function);
                let declaration = alloc_instance_for_class(py, iterable_ptr);
                let name = attr_name_bits_from_bytes(py, b"ReenteredSlotOwner").unwrap();
                let target = molt_class_new(name);
                let target_ptr = obj_from_bits(target).as_ptr().unwrap();
                let slots = attr_name_bits_from_bytes(py, b"__slots__").unwrap();
                crate::molt_set_attr_name(target, slots, declaration);
                REENTERED_SLOT_CLASS.store(target, std::sync::atomic::Ordering::SeqCst);
                assert!(
                    !crate::builtins::attr::capture_class_slot_declaration_for_seal(py, target_ptr)
                );
                REENTERED_SLOT_CLASS.store(0, std::sync::atomic::Ordering::SeqCst);
                let error = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    error,
                    "RuntimeError"
                ));
                assert_eq!(
                    crate::builtins::exceptions::format_exception_message(
                        py,
                        obj_from_bits(error).as_ptr().unwrap(),
                    ),
                    "class definition changed during slot admission"
                );
                clear_exception(py);
                assert!(matches!(
                    class_slot_declaration(target_ptr),
                    ClassSlotDeclaration::Names(_)
                ));
                assert!(
                    !super::super::layout::class_slot_policy(target_ptr)
                        .unwrap()
                        .allows_dict
                );
                for bits in [
                    error,
                    slots,
                    target,
                    name,
                    declaration,
                    function,
                    iterator_name,
                    iterable,
                ] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn exception_slot_admission_preserves_incoming_raised_identity() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            raise_exception::<()>(py, "ValueError", "incoming class construction context");
            let incoming = crate::builtins::exceptions::molt_exception_last_pending();
            let class = crate::builtins::exceptions::exception_type_bits_from_name(
                py,
                "SlotAdmissionPendingException",
            );
            let class = obj_from_bits(class)
                .as_ptr()
                .expect("class admitted under pending error");
            assert!(unsafe { super::super::layout::class_slot_policy(class) }.is_some());
            let restored = crate::builtins::exceptions::molt_exception_last_pending();
            assert_eq!(incoming, restored);
            assert!(exception_pending(py));
            clear_exception(py);
            dec_ref_bits(py, incoming);
            dec_ref_bits(py, restored);
        });
    }

    #[test]
    fn cycle_clear_preserves_layout_until_last_instance_release() {
        let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let native = builtin_classes(py);
                for base in [
                    native.object,
                    native.list,
                    native.dict,
                    native.module,
                    native.classmethod,
                    native.staticmethod,
                    native.property,
                ] {
                    for class_first in [true, false] {
                        let parent = slotted_class(py, b"ClearOrderBase", base, &[b"x"]);
                        let class = slotted_class(py, b"ClearOrderChild", parent, &[b"x", b"x"]);
                        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                        let instance = alloc_instance_for_class(py, class_ptr);
                        let instance_ptr = obj_from_bits(instance).as_ptr().unwrap();
                        let layout = super::super::layout::class_field_layout_bits(class_ptr);
                        let slots =
                            super::super::class_storage::ClassReferenceSlot::SlotDeclaration
                                .load(class_ptr);
                        let marker = alloc_list(py, &[]);
                        let marker_bits = MoltObject::from_ptr(marker).bits();
                        assert!(!marker.is_null());
                        let marker_refs = || (*header_from_obj_ptr(marker)).ref_count_snapshot();
                        let mut owners = 0;
                        super::super::field_storage::for_each_instance_field(
                            py,
                            instance_ptr,
                            class_ptr,
                            &mut |_, slot| {
                                assert!(is_missing_bits(py, *slot));
                                inc_ref_bits(py, marker_bits);
                                *slot = marker_bits;
                                owners += 1;
                            },
                        );
                        assert_eq!(owners, 3, "hidden inherited and duplicate slots survive");
                        assert_eq!(marker_refs(), owners + 1);
                        let clear = |ptr| super::super::heap_lifecycle::clear_cycle_edges(py, ptr);
                        if class_first {
                            clear(class_ptr);
                            let mut visited = 0;
                            super::super::heap_lifecycle::visit_owned_values(
                                py,
                                instance_ptr,
                                &mut |bits| {
                                    visited += usize::from(bits == marker_bits);
                                },
                            );
                            assert_eq!(visited, owners as usize);
                            clear(instance_ptr);
                        } else {
                            clear(instance_ptr);
                            clear(class_ptr);
                        }
                        clear(class_ptr);
                        clear(instance_ptr);
                        assert_eq!(marker_refs(), 1);
                        assert_eq!(
                            super::super::layout::class_field_layout_bits(class_ptr),
                            layout
                        );
                        assert_eq!(
                            super::super::class_storage::ClassReferenceSlot::SlotDeclaration
                                .load(class_ptr),
                            slots
                        );
                        // The instance is now the last external class owner.
                        // Terminal instance release must still use its sealed rows.
                        dec_ref_bits(py, class);
                        dec_ref_bits(py, instance);
                        assert_eq!(marker_refs(), 1);
                        dec_ref_bits(py, marker_bits);
                        dec_ref_bits(py, parent);
                        assert!(!exception_pending(py));
                    }
                }
            }
        });
    }

    #[test]
    fn retired_class_has_no_live_slot_policy() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let class =
                    slotted_class(py, b"RetiredSlotPolicy", builtin_classes(py).object, &[]);
                let ptr = obj_from_bits(class).as_ptr().unwrap();
                assert!(super::super::layout::class_slot_policy(ptr).is_some());
                let edges = super::super::class_storage::detach_class_references(
                    ptr,
                    super::super::class_storage::ClassReferenceRelease::Terminal,
                );
                assert!(super::super::layout::class_slot_policy(ptr).is_none());
                assert!(matches!(
                    class_slot_declaration(ptr),
                    ClassSlotDeclaration::Absent
                ));
                for edge in edges {
                    dec_ref_bits(py, edge);
                }
                dec_ref_bits(py, class);
            }
        });
    }

    #[test]
    fn managed_slots_preserve_hidden_and_duplicate_physical_owners() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let base =
                    slotted_class(py, b"PhysicalListBase", builtin_classes(py).list, &[b"x"]);
                let child = slotted_class(py, b"PhysicalListChild", base, &[b"x", b"x"]);
                let base_ptr = obj_from_bits(base).as_ptr().unwrap();
                let child_ptr = obj_from_bits(child).as_ptr().unwrap();
                let mut rows = Vec::new();
                for_each_field(py, child_ptr, &mut |field| rows.push(field));
                assert_eq!(rows.len(), 3);
                assert!(rows.iter().all(|field| field.kind.is_declared_slot()));
                assert!(rows.windows(2).all(|pair| pair[0].offset < pair[1].offset));
                for field in &rows {
                    assert_eq!(
                        field_at_offset(child_ptr, field.offset).unwrap().offset,
                        field.offset
                    );
                }
                let x = attr_name_bits_from_bytes(py, b"x").unwrap();
                let base_descriptor = dict_get_in_place(
                    py,
                    obj_from_bits(class_dict_bits(base_ptr)).as_ptr().unwrap(),
                    x,
                )
                .unwrap();
                let child_descriptor = dict_get_in_place(
                    py,
                    obj_from_bits(class_dict_bits(child_ptr)).as_ptr().unwrap(),
                    x,
                )
                .unwrap();
                let object = alloc_instance_for_class(py, child_ptr);
                assert!(obj_from_bits(object).as_ptr().is_some());
                let base_value = MoltObject::from_int(11).bits();
                let child_value = MoltObject::from_int(22).bits();
                crate::builtins::types::native_descriptor_mutate(
                    py,
                    base_descriptor,
                    object,
                    Some(base_value),
                );
                let missing = crate::builtins::types::native_descriptor_get(
                    py,
                    child_descriptor,
                    Some(object),
                    None,
                );
                dec_ref_bits(py, missing);
                assert!(exception_pending(py));
                clear_exception(py);
                crate::builtins::types::native_descriptor_mutate(
                    py,
                    child_descriptor,
                    object,
                    Some(child_value),
                );
                assert_eq!(
                    crate::builtins::types::native_descriptor_get(
                        py,
                        base_descriptor,
                        Some(object),
                        None
                    ),
                    base_value
                );
                assert_eq!(
                    crate::builtins::types::native_descriptor_get(
                        py,
                        child_descriptor,
                        Some(object),
                        None
                    ),
                    child_value
                );
                crate::builtins::types::native_descriptor_mutate(py, base_descriptor, object, None);
                let missing = crate::builtins::types::native_descriptor_get(
                    py,
                    base_descriptor,
                    Some(object),
                    None,
                );
                dec_ref_bits(py, missing);
                assert!(exception_pending(py));
                clear_exception(py);
                assert_eq!(
                    crate::builtins::types::native_descriptor_get(
                        py,
                        child_descriptor,
                        Some(object),
                        None
                    ),
                    child_value
                );
                dec_ref_bits(py, object);
                dec_ref_bits(py, x);
                dec_ref_bits(py, child);
                dec_ref_bits(py, base);
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn dict_shape_tail_and_declared_rows_have_disjoint_owners() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let class = slotted_class(py, b"PhysicalDict", builtin_classes(py).dict, &[b"x"]);
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                let object = alloc_instance_for_class(py, class_ptr);
                let ptr = obj_from_bits(object).as_ptr().unwrap();
                let tail = super::super::layout::dict_subclass_storage_slot(ptr).unwrap();
                let backing = super::super::ops::dict_like_bits_from_ptr(py, ptr).unwrap();
                assert_eq!(*tail, backing);
                for_each_field(py, class_ptr, &mut |field| {
                    assert!(field.offset + WORD <= tail as usize - ptr as usize);
                });
                // Corrupt non-dictionary storage must be reported, never replaced
                // with a second owner that hides the broken physical invariant.
                *tail = MoltObject::from_int(99).bits();
                dec_ref_bits(py, backing);
                assert!(super::super::ops::dict_like_bits_from_ptr(py, ptr).is_none());
                assert!(exception_pending(py));
                assert_eq!(*tail, MoltObject::from_int(99).bits());
                clear_exception(py);
                *tail = 0;
                dec_ref_bits(py, object);
                dec_ref_bits(py, class);
            }
        });
    }
}
