//! Declaring builtin rich-comparison storage contracts.
//!
//! Ordinary descriptors and exact-type/C-ABI fast paths enter this same typed
//! authority. Only child elements dispatch Python equality; the outer receiver
//! never re-enters the generic operator from its builtin descriptor.
use super::*;
use crate::builtins::numbers::{
    compare_bigint_float, int_subclass_value_bits_raw,
};
use crate::object::ops::as_float_extended;
use num_bigint::BigInt;

mod containers;
mod immutable;
mod memoryview;

// One declaring-owner map drives both directions, without a parallel ordinal
// registry. The C ABI carries the actual declaring class handle and operation.
macro_rules! comparison_families {
    ($( $family:ident => $owner:ident, $name:literal; )*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum BuiltinComparison { $( $family, )* }
        impl BuiltinComparison {
            pub(crate) fn owner(self, py: &PyToken<'_>) -> u64 {
                let classes = builtin_classes(py);
                match self { $( Self::$family => classes.$owner, )* }
            }
            pub(crate) fn from_owner(py: &PyToken<'_>, owner: u64) -> Option<Self> {
                let classes = builtin_classes(py);
                $( if owner == classes.$owner { return Some(Self::$family); } )*
                None
            }
            fn name(self) -> &'static str {
                match self { $( Self::$family => $name, )* }
            }
        }
    };
}

comparison_families! {
    Int => int, "int";
    Float => float, "float";
    Complex => complex, "complex";
    Dict => dict, "dict";
    Set => set, "set";
    Frozenset => frozenset, "frozenset";
    Range => range, "range";
    MemoryView => memoryview, "memoryview";
    Slice => slice, "slice";
    GenericAlias => generic_alias, "types.GenericAlias";
    Union => union_type, "types.UnionType";
    DictKeys => dict_keys, "dict_keys";
    DictItems => dict_items, "dict_items";
}

fn physical_type(value: MoltObject) -> Option<u32> {
    value.as_ptr().map(|ptr| unsafe { object_type_id(ptr) })
}

fn integer_payload(value: MoltObject) -> Option<MoltObject> {
    let value = int_subclass_value_bits_raw(value.bits()).map(obj_from_bits).unwrap_or(value);
    (value.is_int() || value.is_bool() || bigint_ptr_from_bits(value.bits()).is_some())
        .then_some(value)
}

fn integer_value(value: MoltObject) -> Option<BigInt> {
    let value = integer_payload(value)?;
    if value.is_int() { return Some(BigInt::from(value.as_int_unchecked())); }
    if let Some(value) = value.as_bool() { return Some(BigInt::from(u8::from(value))); }
    bigint_ptr_from_bits(value.bits()).map(|ptr| unsafe { bigint_ref(ptr).clone() })
}

fn float_value(value: MoltObject) -> Option<f64> {
    as_float_extended(value)
}

impl BuiltinComparison {
    fn accepts(self, value: MoltObject) -> bool {
        match self {
            Self::Int => integer_payload(value).is_some(),
            Self::Float => float_value(value).is_some(),
            Self::Complex => complex_ptr_from_bits(value.bits()).is_some(),
            Self::Dict => physical_type(value) == Some(TYPE_ID_DICT),
            Self::Set => physical_type(value) == Some(TYPE_ID_SET),
            Self::Frozenset => physical_type(value) == Some(TYPE_ID_FROZENSET),
            Self::Range => physical_type(value) == Some(TYPE_ID_RANGE),
            Self::MemoryView => physical_type(value) == Some(TYPE_ID_MEMORYVIEW),
            Self::Slice => physical_type(value) == Some(TYPE_ID_SLICE),
            Self::GenericAlias => physical_type(value) == Some(TYPE_ID_GENERIC_ALIAS),
            Self::Union => physical_type(value) == Some(TYPE_ID_UNION),
            Self::DictKeys => physical_type(value) == Some(TYPE_ID_DICT_KEYS_VIEW),
            Self::DictItems => physical_type(value) == Some(TYPE_ID_DICT_ITEMS_VIEW),
        }
    }

    pub(crate) fn invoke(self, py: &PyToken<'_>, left: u64, right: u64, op: RichCompareOp) -> u64 {
        let left = obj_from_bits(left);
        if !self.accepts(left) {
            return raise_exception(py, "TypeError", &format!(
                "descriptor '{}' requires a '{}' object but received a '{}'",
                op.method_name(), self.name(), type_name(py, left),
            ));
        }
        match self.compare(py, left, obj_from_bits(right), op) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::NotComparable => not_implemented_bits(py),
            CompareValueOutcome::Error => MoltObject::none().bits(),
        }
    }

    pub(crate) fn invoke_contains(self, py: &PyToken<'_>, view: u64, item: u64) -> u64 {
        if !matches!(self, Self::DictKeys | Self::DictItems) || !self.accepts(obj_from_bits(view)) {
            return raise_exception(py, "TypeError", &format!(
                "descriptor '__contains__' requires a '{}' object but received a '{}'",
                self.name(), type_name(py, obj_from_bits(view)),
            ));
        }
        match containers::view_contains(py, self, view, item) {
            Ok(found) if !exception_pending(py) => MoltObject::from_bool(found).bits(),
            Ok(_) => MoltObject::none().bits(),
            Err(()) => MoltObject::none().bits(),
        }
    }

    pub(crate) fn compare(
        self, py: &PyToken<'_>, left: MoltObject, right: MoltObject, op: RichCompareOp,
    ) -> CompareValueOutcome {
        if !self.accepts(left) { return CompareValueOutcome::NotComparable; }
        let _left = Pin::borrow(py, left.bits());
        let _right = Pin::borrow(py, right.bits());
        match self {
            Self::Int | Self::Float | Self::Complex => numeric(self, left, right, op),
            Self::Dict => containers::dict(py, left, right, op),
            Self::Set | Self::Frozenset | Self::DictKeys | Self::DictItems => {
                containers::set_like(py, self, left, right, op)
            }
            Self::Range => immutable::range(left, right, op),
            Self::Slice => immutable::slice(py, left, right, op),
            Self::GenericAlias => immutable::generic_alias(py, left, right, op),
            Self::Union => immutable::union(py, left, right, op),
            Self::MemoryView => memoryview::compare(py, left, right, op),
        }
    }
}

pub(crate) fn family_for_owner(py: &PyToken<'_>, owner: u64) -> Option<BuiltinComparison> {
    BuiltinComparison::from_owner(py, owner)
}

pub(crate) fn family_for_value(_py: &PyToken<'_>, value: MoltObject) -> Option<BuiltinComparison> {
    use BuiltinComparison::*;
    if integer_payload(value).is_some() { return Some(Int); }
    if float_value(value).is_some() { return Some(Float); }
    match physical_type(value)? {
        TYPE_ID_COMPLEX => Some(Complex),
        TYPE_ID_DICT => Some(Dict),
        TYPE_ID_SET => Some(Set),
        TYPE_ID_FROZENSET => Some(Frozenset),
        TYPE_ID_RANGE => Some(Range),
        TYPE_ID_MEMORYVIEW => Some(MemoryView),
        TYPE_ID_SLICE => Some(Slice),
        TYPE_ID_GENERIC_ALIAS => Some(GenericAlias),
        TYPE_ID_UNION => Some(Union),
        TYPE_ID_DICT_KEYS_VIEW => Some(DictKeys),
        TYPE_ID_DICT_ITEMS_VIEW => Some(DictItems),
        _ => None,
    }
}

fn boolean(value: bool) -> CompareValueOutcome {
    CompareValueOutcome::Value(MoltObject::from_bool(value).bits())
}

fn equality(value: Result<bool, ()>, op: RichCompareOp) -> CompareValueOutcome {
    match value {
        Ok(value) => boolean(if op == RichCompareOp::Ne { !value } else { value }),
        Err(()) => CompareValueOutcome::Error,
    }
}

fn element_equal(py: &PyToken<'_>, left: u64, right: u64) -> Result<bool, ()> {
    match compare_object_eq_bool(py, obj_from_bits(left), obj_from_bits(right)) {
        CompareBoolOutcome::True => Ok(true),
        CompareBoolOutcome::False => Ok(false),
        CompareBoolOutcome::Error | CompareBoolOutcome::NotComparable => Err(()),
    }
}

struct Pin<'a, 'py> { py: &'a PyToken<'py>, bits: u64 }
impl<'a, 'py> Pin<'a, 'py> {
    fn borrow(py: &'a PyToken<'py>, bits: u64) -> Self {
        inc_ref_bits(py, bits);
        Self { py, bits }
    }
    fn adopt(py: &'a PyToken<'py>, bits: u64) -> Self { Self { py, bits } }
}
impl Drop for Pin<'_, '_> {
    fn drop(&mut self) { dec_ref_bits(self.py, self.bits); }
}

fn numeric(
    family: BuiltinComparison, left: MoltObject, right: MoltObject, op: RichCompareOp,
) -> CompareValueOutcome {
    if family == BuiltinComparison::Complex {
        if !op.is_equality() { return CompareValueOutcome::NotComparable; }
        let lhs = unsafe { *complex_ref(complex_ptr_from_bits(left.bits()).unwrap()) };
        let equal = if let Some(ptr) = complex_ptr_from_bits(right.bits()) {
            let rhs = unsafe { *complex_ref(ptr) };
            lhs.re == rhs.re && lhs.im == rhs.im
        } else if let Some(rhs) = integer_value(right) {
            lhs.im == 0.0 && compare_bigint_float(&rhs, lhs.re) == Some(Ordering::Equal)
        } else if let Some(rhs) = float_value(right) {
            lhs.im == 0.0 && lhs.re == rhs
        } else { return CompareValueOutcome::NotComparable; };
        return equality(Ok(equal), op);
    }
    let order = if family == BuiltinComparison::Int {
        // int's declaring slot declines float/complex; reflection owns them.
        let Some(rhs) = integer_value(right) else { return CompareValueOutcome::NotComparable; };
        Some(integer_value(left).unwrap().cmp(&rhs))
    } else {
        let lhs = float_value(left).unwrap();
        if let Some(rhs) = float_value(right) { lhs.partial_cmp(&rhs) }
        else if let Some(rhs) = integer_value(right) { compare_bigint_float(&rhs, lhs).map(Ordering::reverse) }
        else { return CompareValueOutcome::NotComparable; }
    };
    boolean(order.map(|order| op.test(order)).unwrap_or(op == RichCompareOp::Ne))
}
