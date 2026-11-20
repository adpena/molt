//! The one text authority for "a special method returned the wrong type".
//!
//! CPython 3.15 rewrote this family into one shape,
//! `{Owner}.{method}() must return {a noun}, not {Actual}`, with both types
//! named by their fully qualified names (`%T`: `module.qualname`, the module
//! omitted for `builtins` and `__main__`). Targets 3.12 to 3.14 keep the
//! per-method texts below, with C's `%.Ns` widths. Molt objects
//! (molt-runtime) and native objects (this crate) raise the same errors, so
//! both format them here; each caller names the types the way the target
//! version does ([`SlotTypeNames`]). Names are bytes, so a class name with a
//! lone surrogate reaches the message as CPython's `%U` keeps it.

/// A special method whose result type CPython checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotResult {
    Index,
    Int,
    Float,
    Complex,
    Str,
    Repr,
    Bytes,
    Iter,
    Await,
    Bool,
    LengthHint,
    Format,
    Buffer,
    Annotate,
}

/// The two type names a message shows, in full: `tp_name`s before 3.15,
/// fully qualified names from 3.15. The formatter applies any width.
#[derive(Clone, Copy, Debug)]
pub struct SlotTypeNames<'a> {
    /// The type whose special method ran.
    pub owner: &'a [u8],
    /// The type of the value it returned.
    pub actual: &'a [u8],
}

/// Whether `target` (major, minor) uses the 3.15 texts.
pub const fn modern_slot_result_texts(major: i64, minor: i64) -> bool {
    major > 3 || (major == 3 && minor >= 15)
}

/// Whether the configured target uses the 3.15 texts (native callers; the
/// runtime implements this hook from its own version authority).
pub(crate) fn target_uses_modern_texts() -> bool {
    unsafe { (crate::hooks::hooks_or_stubs().target_python_at_least)(3, 15) }
}

/// A native type's full name in the target's texts: `tp_name` before 3.15,
/// `%T` from 3.15.
pub(crate) unsafe fn native_type_name(
    tp: *mut crate::abi_types::PyTypeObject,
    modern: bool,
) -> String {
    if modern {
        unsafe { crate::api::typeobj::type_fully_qualified_name(tp) }
    } else {
        unsafe { crate::api::typeobj::type_name_with_precision(tp, usize::MAX) }
    }
}

/// The `ValueError` text for a negative `__length_hint__()` result. 3.15
/// names the owner type (`%T`); earlier targets do not.
pub fn negative_length_hint(modern: bool, owner: &[u8]) -> Vec<u8> {
    if modern {
        [owner, b".__length_hint__() must return a non-negative int"].concat()
    } else {
        b"__length_hint__() should return >= 0".to_vec()
    }
}

/// Set the `ValueError` for native `owner`'s negative `__length_hint__()`.
pub(crate) unsafe fn set_native_negative_length_hint_error(owner: *mut crate::abi_types::PyObject) {
    let modern = target_uses_modern_texts();
    let name = unsafe { native_type_name(crate::bridge::semantic_type(owner), modern) };
    let message = negative_length_hint(modern, name.as_bytes());
    unsafe {
        crate::api::errors::set_python_error_bytes(
            (&raw mut crate::abi_types::PyExc_ValueError).cast(),
            &message,
        );
    }
}

/// Set the `TypeError` for `owner`'s `slot` method returning `result`, both
/// native objects (`owner` may be null when the text names no owner).
pub(crate) unsafe fn set_native_slot_result_error(
    slot: SlotResult,
    owner: *mut crate::abi_types::PyObject,
    result: *mut crate::abi_types::PyObject,
) {
    let modern = target_uses_modern_texts();
    let name = |object: *mut crate::abi_types::PyObject| -> String {
        if object.is_null() {
            String::new()
        } else {
            unsafe { native_type_name(crate::bridge::semantic_type(object), modern) }
        }
    };
    let (owner, actual) = (name(owner), name(result));
    let message = slot.type_error(
        modern,
        SlotTypeNames {
            owner: owner.as_bytes(),
            actual: actual.as_bytes(),
        },
    );
    unsafe {
        crate::api::errors::set_python_error_bytes(
            (&raw mut crate::abi_types::PyExc_TypeError).cast(),
            &message,
        );
    }
}

/// The `DeprecationWarning` text for `owner`'s `slot` method returning
/// `result`, an instance of a strict subclass, both native objects.
pub(crate) unsafe fn native_slot_subclass_warning(
    slot: SlotResult,
    owner: *mut crate::abi_types::PyObject,
    result: *mut crate::abi_types::PyObject,
) -> std::ffi::CString {
    let modern = target_uses_modern_texts();
    let name = |object| unsafe { native_type_name(crate::bridge::semantic_type(object), modern) };
    let (owner, actual) = (name(owner), name(result));
    let text = slot.subclass_warning(
        modern,
        SlotTypeNames {
            owner: owner.as_bytes(),
            actual: actual.as_bytes(),
        },
    );
    std::ffi::CString::new(text).expect("type names hold no NUL")
}

impl SlotResult {
    pub const fn method(self) -> &'static str {
        match self {
            Self::Index => "__index__",
            Self::Int => "__int__",
            Self::Float => "__float__",
            Self::Complex => "__complex__",
            Self::Str => "__str__",
            Self::Repr => "__repr__",
            Self::Bytes => "__bytes__",
            Self::Iter => "__iter__",
            Self::Await => "__await__",
            Self::Bool => "__bool__",
            Self::LengthHint => "__length_hint__",
            Self::Format => "__format__",
            Self::Buffer => "__buffer__",
            Self::Annotate => "__annotate__",
        }
    }

    /// The expected result, with its article, as 3.15 says it.
    const fn expected(self) -> &'static str {
        match self {
            Self::Index | Self::Int | Self::LengthHint => "an int",
            Self::Float => "a float",
            Self::Complex => "a complex",
            Self::Str | Self::Repr | Self::Format => "a str",
            Self::Bytes => "a bytes",
            Self::Iter | Self::Await => "an iterator",
            Self::Bool => "a bool",
            Self::Buffer => "a memoryview",
            Self::Annotate => "a dict",
        }
    }

    /// The type a strict subclass result is deprecated for, if any.
    const fn subclass_base(self) -> Option<&'static str> {
        match self {
            Self::Index | Self::Int => Some("int"),
            Self::Float => Some("float"),
            Self::Complex => Some("complex"),
            _ => None,
        }
    }

    /// CPython's `%.Ns` width for this text's type names before 3.15
    /// (`usize::MAX` for `%s`). `%T` has no width.
    const fn legacy_precision(self) -> usize {
        match self {
            Self::Float => 50,
            Self::Iter | Self::Await | Self::LengthHint => 100,
            Self::Bool | Self::Annotate | Self::Buffer => usize::MAX,
            _ => 200,
        }
    }

    /// The `TypeError` text for a result of the wrong type.
    pub fn type_error(self, modern: bool, names: SlotTypeNames<'_>) -> Vec<u8> {
        let method = self.method().as_bytes();
        if modern {
            let mut text = Vec::new();
            // A function's `__annotate__` has no owner type in the text.
            if self != Self::Annotate {
                text.extend_from_slice(names.owner);
                text.push(b'.');
            }
            let parts: [&[u8]; 5] = [
                method,
                b"() must return ",
                self.expected().as_bytes(),
                b", not ",
                names.actual,
            ];
            for part in parts {
                text.extend_from_slice(part);
            }
            return text;
        }
        let width = |name: &[u8]| -> Vec<u8> {
            let precision = self.legacy_precision();
            if name.len() <= precision {
                name.to_vec()
            } else {
                crate::api::typeobj::native_name_bytes_with_precision(name, precision).into_bytes()
            }
        };
        let (owner, actual) = (width(names.owner), width(names.actual));
        let parts: Vec<&[u8]> = match self {
            Self::Index | Self::Int => vec![method, b" returned non-int (type ", &actual, b")"],
            Self::Float => vec![
                &owner,
                b".__float__ returned non-float (type ",
                &actual,
                b")",
            ],
            Self::Complex => vec![b"__complex__ returned non-complex (type ", &actual, b")"],
            Self::Str | Self::Repr => vec![method, b" returned non-string (type ", &actual, b")"],
            Self::Bytes => vec![b"__bytes__ returned non-bytes (type ", &actual, b")"],
            Self::Iter => vec![b"iter() returned non-iterator of type '", &actual, b"'"],
            Self::Await => vec![
                b"__await__() returned non-iterator of type '",
                &actual,
                b"'",
            ],
            Self::Bool => vec![b"__bool__ should return bool, returned ", &actual],
            Self::LengthHint => vec![b"__length_hint__ must be an integer, not ", &actual],
            Self::Format => vec![b"__format__ must return a str, not ", &actual],
            Self::Buffer => vec![b"__buffer__ returned non-memoryview object"],
            Self::Annotate => vec![b"__annotate__ returned non-dict of type '", &actual, b"'"],
        };
        parts.concat()
    }

    /// The `DeprecationWarning` text for a result that is an instance of a
    /// strict subclass of the expected type.
    pub fn subclass_warning(self, modern: bool, names: SlotTypeNames<'_>) -> String {
        let base = self
            .subclass_base()
            .unwrap_or_else(|| panic!("{} has no subclass deprecation", self.method()));
        format!(
            "{}.  The ability to return an instance of a strict subclass of {base} is \
             deprecated, and may be removed in a future version of Python.",
            String::from_utf8_lossy(&self.type_error(modern, names))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names<'a>(owner: &'a str, actual: &'a str) -> SlotTypeNames<'a> {
        SlotTypeNames {
            owner: owner.as_bytes(),
            actual: actual.as_bytes(),
        }
    }

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("ASCII names")
    }

    /// Each expected text is CPython's own output: 3.14.10 for `false`,
    /// 3.15.2 for `true`, from the probe programs in this slice's commit.
    #[test]
    fn each_target_says_what_cpython_says() {
        let cases: [(SlotResult, bool, &str, &str, &str); 12] = [
            (
                SlotResult::Index,
                false,
                "B",
                "float",
                "__index__ returned non-int (type float)",
            ),
            (
                SlotResult::Float,
                false,
                "B",
                "int",
                "B.__float__ returned non-float (type int)",
            ),
            (
                SlotResult::Bool,
                false,
                "B",
                "int",
                "__bool__ should return bool, returned int",
            ),
            (
                SlotResult::Iter,
                false,
                "B",
                "int",
                "iter() returned non-iterator of type 'int'",
            ),
            (
                SlotResult::Buffer,
                false,
                "B",
                "int",
                "__buffer__ returned non-memoryview object",
            ),
            (
                SlotResult::Annotate,
                false,
                "function",
                "int",
                "__annotate__ returned non-dict of type 'int'",
            ),
            (
                SlotResult::Index,
                true,
                "pkg.mod.B",
                "str",
                "pkg.mod.B.__index__() must return an int, not str",
            ),
            (
                SlotResult::Iter,
                true,
                "B",
                "int",
                "B.__iter__() must return an iterator, not int",
            ),
            (
                SlotResult::Bool,
                true,
                "B",
                "int",
                "B.__bool__() must return a bool, not int",
            ),
            (
                SlotResult::Buffer,
                true,
                "B",
                "int",
                "B.__buffer__() must return a memoryview, not int",
            ),
            (
                SlotResult::Annotate,
                true,
                "function",
                "int",
                "__annotate__() must return a dict, not int",
            ),
            (
                SlotResult::Format,
                true,
                "C",
                "collections.OrderedDict",
                "C.__format__() must return a str, not collections.OrderedDict",
            ),
        ];
        for (slot, modern, owner, actual, expected) in cases {
            assert_eq!(
                text(slot.type_error(modern, names(owner, actual))),
                expected
            );
        }
        assert_eq!(
            SlotResult::Float.subclass_warning(true, names("B", "F")),
            "B.__float__() must return a float, not F.  The ability to return an instance of a \
             strict subclass of float is deprecated, and may be removed in a future version of \
             Python."
        );
        assert_eq!(
            SlotResult::Complex.subclass_warning(false, names("B", "C")),
            "__complex__ returned non-complex (type C).  The ability to return an instance of a \
             strict subclass of complex is deprecated, and may be removed in a future version of \
             Python."
        );
    }

    /// CPython 3.14.10 and 3.15.2 on `operator.length_hint` of a class `H`
    /// whose `__length_hint__` returns -1.
    #[test]
    fn negative_length_hint_names_the_owner_from_3_15() {
        assert_eq!(
            text(negative_length_hint(false, b"H")),
            "__length_hint__() should return >= 0"
        );
        assert_eq!(
            text(negative_length_hint(true, b"pkg.H")),
            "pkg.H.__length_hint__() must return a non-negative int"
        );
    }

    /// A name with a lone surrogate (UTF-8-encoded as CPython stores it)
    /// reaches the text unchanged; only a name over the legacy width is cut.
    #[test]
    fn names_keep_their_bytes() {
        let surrogate = b"B\xed\xb0\x80";
        let message = SlotResult::Str.type_error(
            false,
            SlotTypeNames {
                owner: b"O",
                actual: surrogate,
            },
        );
        assert_eq!(message, b"__str__ returned non-string (type B\xed\xb0\x80)");
        let long = "x".repeat(60);
        let message = SlotResult::Float.type_error(false, names(&long, "int"));
        assert!(text(message).starts_with(&format!("{}.__float__", "x".repeat(50))));
    }

    #[test]
    fn the_rewrite_starts_at_3_15() {
        assert!(!modern_slot_result_texts(3, 14));
        assert!(modern_slot_result_texts(3, 15));
    }
}
