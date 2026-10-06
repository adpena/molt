//! Fixed typed callable metadata, independent of the public instance dictionary.
//! Published code owns immutable signature facts; this tail owns mutable public
//! fields and the setup facts of native or not-yet-published runtime callables.
use crate::*;

// Existing executable/layout words stay at offsets 0 through 12. One 15-word
// reference tail replaces metadata dictionary allocation. No side registry,
// secondary metadata mapping, raw resource pointer, or lazy ownership split.
const METADATA_START: usize = 13;
pub(crate) const FUNCTION_PAYLOAD_WORDS: usize = METADATA_START + FunctionMetadataField::ALL.len();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(crate) enum FunctionMetadataField {
    // Native descriptor operation tags 0 through 6 stay unchanged.
    Name = 0,
    QualName = 1,
    Doc = 2,
    TextSignature = 3,
    Module = 4,
    SelfValue = 5,
    Owner = 6,
    Defaults,
    KeywordDefaults,
    ArgumentNames,
    PositionalOnly,
    KeywordOnlyNames,
    Varargs,
    VarKeywords,
    BindKind,
}

impl FunctionMetadataField {
    pub(crate) const ALL: [Self; 15] = [
        Self::Name,
        Self::QualName,
        Self::Doc,
        Self::TextSignature,
        Self::Module,
        Self::SelfValue,
        Self::Owner,
        Self::Defaults,
        Self::KeywordDefaults,
        Self::ArgumentNames,
        Self::PositionalOnly,
        Self::KeywordOnlyNames,
        Self::Varargs,
        Self::VarKeywords,
        Self::BindKind,
    ];
    pub(crate) const SIGNATURE: [Self; 5] = [
        Self::ArgumentNames,
        Self::PositionalOnly,
        Self::KeywordOnlyNames,
        Self::Varargs,
        Self::VarKeywords,
    ];
    pub(crate) const PUBLIC_FUNCTION: [Self; 6] = [
        Self::Name,
        Self::QualName,
        Self::Doc,
        Self::Module,
        Self::Defaults,
        Self::KeywordDefaults,
    ];

    pub(crate) fn from_operation(value: u32) -> Option<Self> {
        Self::ALL.get(value as usize).copied()
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Name => "__name__",
            Self::QualName => "__qualname__",
            Self::Doc => "__doc__",
            Self::TextSignature => "__text_signature__",
            Self::Module => "__module__",
            Self::SelfValue => "__self__",
            Self::Owner => "__objclass__",
            Self::Defaults => "__defaults__",
            Self::KeywordDefaults => "__kwdefaults__",
            Self::ArgumentNames => "__molt_arg_names__",
            Self::PositionalOnly => "__molt_posonly__",
            Self::KeywordOnlyNames => "__molt_kwonly_names__",
            Self::Varargs => "__molt_vararg__",
            Self::VarKeywords => "__molt_varkw__",
            Self::BindKind => "__molt_bind_kind__",
        }
    }

    pub(crate) fn from_name(name: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|field| field.name().as_bytes() == name)
    }

    unsafe fn slot(self, function: *mut u8) -> *mut u64 {
        unsafe {
            debug_assert_eq!(object_type_id(function), TYPE_ID_FUNCTION);
            function.cast::<u64>().add(METADATA_START + self as usize)
        }
    }

    /// Borrow the physical typed owner. Zero means not initialized; Python
    /// None is an initialized value, independent of public dictionary contents.
    pub(crate) unsafe fn load(self, function: *mut u8) -> Option<u64> {
        let bits = unsafe { *self.slot(function) };
        (bits != 0).then_some(bits)
    }

    /// Move one owner into the GC retirement sink, before any callback runs.
    pub(crate) unsafe fn take(self, function: *mut u8) -> u64 {
        unsafe { self.slot(function).replace(0) }
    }

    /// Retain and publish before any displaced owner can re-enter Python.
    /// The caller commits binder/cache state before releasing this receipt.
    pub(crate) unsafe fn replace_deferred<'a, 'py>(
        self,
        py: &'a PyToken<'py>,
        function: *mut u8,
        value: u64,
    ) -> MetadataRetirement<'a, 'py> {
        unsafe {
            crate::gil_assert();
            // Published code has already taken ownership of these facts.
            // Internal initializer replay cannot retain a stale second copy.
            if super::layout::function_code_signature_metadata_bits(
                function,
                self.name().as_bytes(),
            )
            .is_some()
            {
                return MetadataRetirement {
                    py,
                    bits: self.take(function),
                };
            }
            if value != 0 {
                inc_ref_bits(py, value);
                object_mark_has_ptrs(py, function);
            }
            MetadataRetirement {
                py,
                bits: self.slot(function).replace(value),
            }
        }
    }
}

#[must_use]
pub(crate) struct MetadataRetirement<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
}

impl Drop for MetadataRetirement<'_, '_> {
    fn drop(&mut self) {
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            if self.bits != 0 {
                dec_ref_bits(self.py, self.bits);
            }
        });
    }
}

pub(crate) unsafe fn initialize(function: *mut u8) {
    unsafe {
        for field in FunctionMetadataField::ALL {
            *field.slot(function) = 0;
        }
    }
}

/// One internal projection. Published code facts supersede setup fields;
/// absent facts never fall back to user-writable dictionary entries.
pub(crate) unsafe fn metadata_bits(function: *mut u8, name: &[u8]) -> Option<u64> {
    unsafe {
        let field = FunctionMetadataField::from_name(name)?;
        super::layout::function_code_signature_metadata_bits(function, name)
            .or_else(|| field.load(function))
    }
}

/// Descriptor mutation of CPython's mutable function metadata. Construction
/// uses replace_deferred directly because its metadata tuple is already typed.
pub(crate) unsafe fn write_public(
    py: &PyToken<'_>,
    function: *mut u8,
    field: FunctionMetadataField,
    value: Option<u64>,
) -> u64 {
    use FunctionMetadataField as Field;
    unsafe {
        let bits = value.unwrap_or(MoltObject::none().bits());
        match field {
            Field::Name | Field::QualName => {
                if obj_from_bits(bits)
                    .as_ptr()
                    .is_none_or(|p| object_type_id(p) != TYPE_ID_STRING)
                {
                    return raise_exception::<_>(
                        py,
                        "TypeError",
                        &format!("{} must be set to a string object", field.name()),
                    );
                }
            }
            Field::Defaults | Field::KeywordDefaults => {
                let expected = if field == Field::Defaults {
                    TYPE_ID_TUPLE
                } else {
                    TYPE_ID_DICT
                };
                if !obj_from_bits(bits).is_none()
                    && obj_from_bits(bits)
                        .as_ptr()
                        .is_none_or(|p| object_type_id(p) != expected)
                {
                    let label = if field == Field::Defaults {
                        "tuple"
                    } else {
                        "dict"
                    };
                    return raise_exception::<_>(
                        py,
                        "TypeError",
                        &format!("{} must be set to a {} object", field.name(), label),
                    );
                }
            }
            Field::Doc | Field::Module => {}
            _ => return raise_exception::<_>(py, "AttributeError", "readonly attribute"),
        }
        let retirement = field.replace_deferred(py, function, bits);
        crate::call::function::commit_function_metadata_change(
            py,
            function,
            field.name().as_bytes(),
            true,
        );
        drop(retirement);
        MoltObject::none().bits()
    }
}
