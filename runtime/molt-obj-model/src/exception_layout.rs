//! Canonical typed builtin-exception payload layouts.
//!
//! CPython's builtin exceptions share the `BaseException` prefix, but nine
//! layout families append typed state.  Runtime payload allocation, attribute
//! semantics, and the CPython ABI projection all consume this table so the
//! field order and class-to-layout mapping cannot drift independently.

pub const MAX_EXCEPTION_TYPED_FIELDS: usize = max_exception_typed_fields();
pub const MAX_EXCEPTION_TYPED_TAIL_WORDS: usize = max_exception_typed_tail_words();

/// Physical payload family after the common `BaseException` fields.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ExceptionLayoutKind {
    #[default]
    Base = 0,
    Group = 1,
    Syntax = 2,
    Import = 3,
    Unicode = 4,
    SystemExit = 5,
    OSError = 6,
    StopIteration = 7,
    NameError = 8,
    AttributeError = 9,
}

/// Canonical physical root identity. Kinds describe byte shape; roots also
/// distinguish same-shaped but layout-incompatible CPython bases (notably the
/// three concrete Unicode error families) for multiple-inheritance checks.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ExceptionLayoutRoot {
    #[default]
    Base = 0,
    BaseExceptionGroup = 1,
    SyntaxError = 2,
    ImportError = 3,
    UnicodeDecodeError = 4,
    UnicodeEncodeError = 5,
    UnicodeTranslateError = 6,
    SystemExit = 7,
    OSError = 8,
    StopIteration = 9,
    NameError = 10,
    AttributeError = 11,
}

impl ExceptionLayoutRoot {
    pub const ALL: [Self; 12] = [
        Self::Base,
        Self::BaseExceptionGroup,
        Self::SyntaxError,
        Self::ImportError,
        Self::UnicodeDecodeError,
        Self::UnicodeEncodeError,
        Self::UnicodeTranslateError,
        Self::SystemExit,
        Self::OSError,
        Self::StopIteration,
        Self::NameError,
        Self::AttributeError,
    ];

    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Base),
            1 => Some(Self::BaseExceptionGroup),
            2 => Some(Self::SyntaxError),
            3 => Some(Self::ImportError),
            4 => Some(Self::UnicodeDecodeError),
            5 => Some(Self::UnicodeEncodeError),
            6 => Some(Self::UnicodeTranslateError),
            7 => Some(Self::SystemExit),
            8 => Some(Self::OSError),
            9 => Some(Self::StopIteration),
            10 => Some(Self::NameError),
            11 => Some(Self::AttributeError),
            _ => None,
        }
    }

    pub const fn kind(self) -> ExceptionLayoutKind {
        match self {
            Self::Base => ExceptionLayoutKind::Base,
            Self::BaseExceptionGroup => ExceptionLayoutKind::Group,
            Self::SyntaxError => ExceptionLayoutKind::Syntax,
            Self::ImportError => ExceptionLayoutKind::Import,
            Self::UnicodeDecodeError | Self::UnicodeEncodeError | Self::UnicodeTranslateError => {
                ExceptionLayoutKind::Unicode
            }
            Self::SystemExit => ExceptionLayoutKind::SystemExit,
            Self::OSError => ExceptionLayoutKind::OSError,
            Self::StopIteration => ExceptionLayoutKind::StopIteration,
            Self::NameError => ExceptionLayoutKind::NameError,
            Self::AttributeError => ExceptionLayoutKind::AttributeError,
        }
    }

    pub const fn owner_name(self) -> &'static str {
        match self {
            Self::Base => "BaseException",
            Self::BaseExceptionGroup => "BaseExceptionGroup",
            Self::SyntaxError => "SyntaxError",
            Self::ImportError => "ImportError",
            Self::UnicodeDecodeError => "UnicodeDecodeError",
            Self::UnicodeEncodeError => "UnicodeEncodeError",
            Self::UnicodeTranslateError => "UnicodeTranslateError",
            Self::SystemExit => "SystemExit",
            Self::OSError => "OSError",
            Self::StopIteration => "StopIteration",
            Self::NameError => "NameError",
            Self::AttributeError => "AttributeError",
        }
    }
}

impl ExceptionLayoutKind {
    pub const ALL: [Self; 10] = [
        Self::Base,
        Self::Group,
        Self::Syntax,
        Self::Import,
        Self::Unicode,
        Self::SystemExit,
        Self::OSError,
        Self::StopIteration,
        Self::NameError,
        Self::AttributeError,
    ];

    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Base),
            1 => Some(Self::Group),
            2 => Some(Self::Syntax),
            3 => Some(Self::Import),
            4 => Some(Self::Unicode),
            5 => Some(Self::SystemExit),
            6 => Some(Self::OSError),
            7 => Some(Self::StopIteration),
            8 => Some(Self::NameError),
            9 => Some(Self::AttributeError),
            _ => None,
        }
    }

    pub const fn tail_word_count(self) -> usize {
        let policies = self.field_policies();
        let mut count = 0;
        let mut index = 0;
        while index < policies.len() {
            if policies[index].storage as u8 != ExceptionFieldStorage::RuntimeMessage as u8 {
                count += 1;
            }
            index += 1;
        }
        count
    }

    pub const fn field_policies(self) -> &'static [ExceptionFieldPolicy] {
        match self {
            Self::Base => BASE_POLICIES,
            Self::Group => GROUP_POLICIES,
            Self::Syntax => SYNTAX_POLICIES,
            Self::Import => IMPORT_POLICIES,
            Self::Unicode => UNICODE_POLICIES,
            Self::SystemExit => SYSTEM_EXIT_POLICIES,
            Self::OSError => OS_ERROR_POLICIES,
            Self::StopIteration => STOP_ITERATION_POLICIES,
            Self::NameError => NAME_ERROR_POLICIES,
            Self::AttributeError => ATTRIBUTE_ERROR_POLICIES,
        }
    }

    pub const fn field_policy(
        self,
        field: ExceptionTypedField,
    ) -> Option<&'static ExceptionFieldPolicy> {
        let policies = self.field_policies();
        let mut index = 0;
        while index < policies.len() {
            if policies[index].field as u8 == field as u8 {
                return Some(&policies[index]);
            }
            index += 1;
        }
        None
    }

    /// Ordered physical runtime-tail policies. Message aliases are the only
    /// fields stored in the common prefix and are declared first in their
    /// layout rows, so this is a zero-allocation view of the canonical table.
    pub fn tail_policies(self) -> &'static [ExceptionFieldPolicy] {
        let policies = self.field_policies();
        let mut first = 0;
        while first < policies.len()
            && policies[first].storage == ExceptionFieldStorage::RuntimeMessage
        {
            first += 1;
        }
        &policies[first..]
    }

    pub fn constructor_keyword_policies(
        self,
    ) -> impl Clone + Iterator<Item = &'static ExceptionFieldPolicy> {
        self.field_policies()
            .iter()
            .filter(|policy| policy.constructor_keyword)
    }

    pub fn constructor_keyword_policy(
        self,
        python_name: &str,
    ) -> Option<&'static ExceptionFieldPolicy> {
        self.constructor_keyword_policies()
            .find(|policy| policy.python_name == python_name)
    }
}

/// Globally unambiguous typed-field identity. Runtime storage uses the order
/// returned by [`ExceptionLayoutKind::field_policies`], not these discriminants.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExceptionTypedField {
    GroupMessage = 0,
    GroupExceptions = 1,
    SyntaxMessage = 2,
    SyntaxFilename = 3,
    SyntaxLineNumber = 4,
    SyntaxOffset = 5,
    SyntaxEndLineNumber = 6,
    SyntaxEndOffset = 7,
    SyntaxText = 8,
    SyntaxPrintFileAndLine = 9,
    ImportMessage = 10,
    ImportName = 11,
    ImportPath = 12,
    ImportNameFrom = 13,
    UnicodeEncoding = 14,
    UnicodeObject = 15,
    UnicodeStart = 16,
    UnicodeEnd = 17,
    UnicodeReason = 18,
    SystemExitCode = 19,
    OSErrorErrno = 20,
    OSErrorStrError = 21,
    OSErrorFilename = 22,
    OSErrorFilename2 = 23,
    OSErrorWinError = 24,
    OSErrorCharactersWritten = 25,
    StopIterationValue = 26,
    NameErrorName = 27,
    AttributeErrorObject = 28,
    AttributeErrorName = 29,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionFieldStorage {
    /// Aliases the common runtime exception message word; no typed-tail word.
    RuntimeMessage = 0,
    Object = 1,
    PySsize = 2,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionMissingRead {
    None = 0,
    AttributeError = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExceptionFieldPolicy {
    pub field: ExceptionTypedField,
    pub python_name: &'static str,
    pub storage: ExceptionFieldStorage,
    pub writable: bool,
    pub deletable: bool,
    /// Accepted by the declaring builtin's constructor as a keyword.
    pub constructor_keyword: bool,
    pub missing_read: ExceptionMissingRead,
}

const fn object_policy(
    field: ExceptionTypedField,
    python_name: &'static str,
    writable: bool,
    deletable: bool,
) -> ExceptionFieldPolicy {
    ExceptionFieldPolicy {
        field,
        python_name,
        storage: ExceptionFieldStorage::Object,
        writable,
        deletable,
        constructor_keyword: false,
        missing_read: ExceptionMissingRead::None,
    }
}

const fn message_policy(
    field: ExceptionTypedField,
    python_name: &'static str,
    writable: bool,
    deletable: bool,
) -> ExceptionFieldPolicy {
    ExceptionFieldPolicy {
        field,
        python_name,
        storage: ExceptionFieldStorage::RuntimeMessage,
        writable,
        deletable,
        constructor_keyword: false,
        missing_read: ExceptionMissingRead::None,
    }
}

const fn ssize_policy(
    field: ExceptionTypedField,
    python_name: &'static str,
    deletable: bool,
    missing_read: ExceptionMissingRead,
) -> ExceptionFieldPolicy {
    ExceptionFieldPolicy {
        field,
        python_name,
        storage: ExceptionFieldStorage::PySsize,
        writable: true,
        deletable,
        constructor_keyword: false,
        missing_read,
    }
}

const fn keyword_object_policy(
    field: ExceptionTypedField,
    python_name: &'static str,
) -> ExceptionFieldPolicy {
    let mut policy = object_policy(field, python_name, true, true);
    policy.constructor_keyword = true;
    policy
}

const BASE_POLICIES: &[ExceptionFieldPolicy] = &[];
const GROUP_POLICIES: &[ExceptionFieldPolicy] = &[
    message_policy(ExceptionTypedField::GroupMessage, "message", false, false),
    object_policy(
        ExceptionTypedField::GroupExceptions,
        "exceptions",
        false,
        false,
    ),
];
const SYNTAX_POLICIES: &[ExceptionFieldPolicy] = &[
    message_policy(ExceptionTypedField::SyntaxMessage, "msg", true, true),
    object_policy(ExceptionTypedField::SyntaxFilename, "filename", true, true),
    object_policy(ExceptionTypedField::SyntaxLineNumber, "lineno", true, true),
    object_policy(ExceptionTypedField::SyntaxOffset, "offset", true, true),
    object_policy(
        ExceptionTypedField::SyntaxEndLineNumber,
        "end_lineno",
        true,
        true,
    ),
    object_policy(
        ExceptionTypedField::SyntaxEndOffset,
        "end_offset",
        true,
        true,
    ),
    object_policy(ExceptionTypedField::SyntaxText, "text", true, true),
    object_policy(
        ExceptionTypedField::SyntaxPrintFileAndLine,
        "print_file_and_line",
        true,
        true,
    ),
];
const IMPORT_POLICIES: &[ExceptionFieldPolicy] = &[
    message_policy(ExceptionTypedField::ImportMessage, "msg", true, true),
    keyword_object_policy(ExceptionTypedField::ImportName, "name"),
    keyword_object_policy(ExceptionTypedField::ImportPath, "path"),
    keyword_object_policy(ExceptionTypedField::ImportNameFrom, "name_from"),
];
const UNICODE_POLICIES: &[ExceptionFieldPolicy] = &[
    object_policy(ExceptionTypedField::UnicodeEncoding, "encoding", true, true),
    object_policy(ExceptionTypedField::UnicodeObject, "object", true, true),
    ssize_policy(
        ExceptionTypedField::UnicodeStart,
        "start",
        false,
        ExceptionMissingRead::None,
    ),
    ssize_policy(
        ExceptionTypedField::UnicodeEnd,
        "end",
        false,
        ExceptionMissingRead::None,
    ),
    object_policy(ExceptionTypedField::UnicodeReason, "reason", true, true),
];
const SYSTEM_EXIT_POLICIES: &[ExceptionFieldPolicy] = &[object_policy(
    ExceptionTypedField::SystemExitCode,
    "code",
    true,
    true,
)];
#[cfg(windows)]
const OS_ERROR_POLICIES: &[ExceptionFieldPolicy] = &[
    object_policy(ExceptionTypedField::OSErrorErrno, "errno", true, true),
    object_policy(ExceptionTypedField::OSErrorStrError, "strerror", true, true),
    object_policy(ExceptionTypedField::OSErrorFilename, "filename", true, true),
    object_policy(
        ExceptionTypedField::OSErrorFilename2,
        "filename2",
        true,
        true,
    ),
    object_policy(ExceptionTypedField::OSErrorWinError, "winerror", true, true),
    ssize_policy(
        ExceptionTypedField::OSErrorCharactersWritten,
        "characters_written",
        true,
        ExceptionMissingRead::AttributeError,
    ),
];
#[cfg(not(windows))]
const OS_ERROR_POLICIES: &[ExceptionFieldPolicy] = &[
    object_policy(ExceptionTypedField::OSErrorErrno, "errno", true, true),
    object_policy(ExceptionTypedField::OSErrorStrError, "strerror", true, true),
    object_policy(ExceptionTypedField::OSErrorFilename, "filename", true, true),
    object_policy(
        ExceptionTypedField::OSErrorFilename2,
        "filename2",
        true,
        true,
    ),
    ssize_policy(
        ExceptionTypedField::OSErrorCharactersWritten,
        "characters_written",
        true,
        ExceptionMissingRead::AttributeError,
    ),
];
const STOP_ITERATION_POLICIES: &[ExceptionFieldPolicy] = &[object_policy(
    ExceptionTypedField::StopIterationValue,
    "value",
    true,
    true,
)];
const NAME_ERROR_POLICIES: &[ExceptionFieldPolicy] = &[keyword_object_policy(
    ExceptionTypedField::NameErrorName,
    "name",
)];
const ATTRIBUTE_ERROR_POLICIES: &[ExceptionFieldPolicy] = &[
    keyword_object_policy(ExceptionTypedField::AttributeErrorObject, "obj"),
    keyword_object_policy(ExceptionTypedField::AttributeErrorName, "name"),
];

const fn max_exception_typed_fields() -> usize {
    let mut max = 0;
    let mut index = 0;
    while index < ExceptionLayoutKind::ALL.len() {
        let kind = ExceptionLayoutKind::ALL[index];
        let len = kind.field_policies().len();
        if len > max {
            max = len;
        }
        index += 1;
    }
    max
}

const fn max_exception_typed_tail_words() -> usize {
    let mut max = 0;
    let mut index = 0;
    while index < ExceptionLayoutKind::ALL.len() {
        let kind = ExceptionLayoutKind::ALL[index];
        let len = kind.tail_word_count();
        if len > max {
            max = len;
        }
        index += 1;
    }
    max
}

/// Canonical builtin-exception inheritance shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionBaseSpec {
    Root,
    One(&'static str),
    Two(&'static str, &'static str),
}

/// Canonical rows own inheritance and physical layout. Compatibility aliases
/// own only their public name and canonical identity, so alias rows cannot
/// silently fork the canonical class's hierarchy or storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuiltinExceptionDefinition {
    Canonical {
        bases: ExceptionBaseSpec,
        introduced_layout_root: Option<ExceptionLayoutRoot>,
    },
    Alias {
        canonical_name: &'static str,
    },
}

/// One row is the authority for builtin identity, compatibility aliases,
/// inheritance, and physical layout. Runtime class bootstrap and the CPython
/// ABI consume the resolved row instead of maintaining parallel name switches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuiltinExceptionSpec {
    name: &'static str,
    definition: BuiltinExceptionDefinition,
}

const fn exception_spec(
    name: &'static str,
    bases: ExceptionBaseSpec,
    introduced_layout_root: Option<ExceptionLayoutRoot>,
) -> BuiltinExceptionSpec {
    BuiltinExceptionSpec {
        name,
        definition: BuiltinExceptionDefinition::Canonical {
            bases,
            introduced_layout_root,
        },
    }
}

const fn exception_alias(name: &'static str, canonical_name: &'static str) -> BuiltinExceptionSpec {
    BuiltinExceptionSpec {
        name,
        definition: BuiltinExceptionDefinition::Alias { canonical_name },
    }
}

const BUILTIN_EXCEPTION_SPECS: &[BuiltinExceptionSpec] = &[
    exception_spec("BaseException", ExceptionBaseSpec::Root, None),
    exception_spec(
        "BaseExceptionGroup",
        ExceptionBaseSpec::One("BaseException"),
        Some(ExceptionLayoutRoot::BaseExceptionGroup),
    ),
    exception_spec("Exception", ExceptionBaseSpec::One("BaseException"), None),
    exception_spec(
        "ExceptionGroup",
        ExceptionBaseSpec::Two("BaseExceptionGroup", "Exception"),
        None,
    ),
    exception_spec(
        "GeneratorExit",
        ExceptionBaseSpec::One("BaseException"),
        None,
    ),
    exception_spec(
        "KeyboardInterrupt",
        ExceptionBaseSpec::One("BaseException"),
        None,
    ),
    exception_spec(
        "SystemExit",
        ExceptionBaseSpec::One("BaseException"),
        Some(ExceptionLayoutRoot::SystemExit),
    ),
    exception_spec(
        "CancelledError",
        ExceptionBaseSpec::One("BaseException"),
        None,
    ),
    exception_spec("ArithmeticError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec("AssertionError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec(
        "AttributeError",
        ExceptionBaseSpec::One("Exception"),
        Some(ExceptionLayoutRoot::AttributeError),
    ),
    exception_spec("BufferError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec("EOFError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec(
        "ImportError",
        ExceptionBaseSpec::One("Exception"),
        Some(ExceptionLayoutRoot::ImportError),
    ),
    exception_spec("LookupError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec("MemoryError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec(
        "NameError",
        ExceptionBaseSpec::One("Exception"),
        Some(ExceptionLayoutRoot::NameError),
    ),
    exception_spec(
        "OSError",
        ExceptionBaseSpec::One("Exception"),
        Some(ExceptionLayoutRoot::OSError),
    ),
    exception_spec("ReferenceError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec("RuntimeError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec(
        "StopIteration",
        ExceptionBaseSpec::One("Exception"),
        Some(ExceptionLayoutRoot::StopIteration),
    ),
    exception_spec(
        "StopAsyncIteration",
        ExceptionBaseSpec::One("Exception"),
        None,
    ),
    exception_spec(
        "SyntaxError",
        ExceptionBaseSpec::One("Exception"),
        Some(ExceptionLayoutRoot::SyntaxError),
    ),
    exception_spec("SystemError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec("TypeError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec("ValueError", ExceptionBaseSpec::One("Exception"), None),
    exception_spec("Warning", ExceptionBaseSpec::One("Exception"), None),
    exception_spec(
        "FloatingPointError",
        ExceptionBaseSpec::One("ArithmeticError"),
        None,
    ),
    exception_spec(
        "OverflowError",
        ExceptionBaseSpec::One("ArithmeticError"),
        None,
    ),
    exception_spec(
        "ZeroDivisionError",
        ExceptionBaseSpec::One("ArithmeticError"),
        None,
    ),
    exception_spec(
        "ModuleNotFoundError",
        ExceptionBaseSpec::One("ImportError"),
        None,
    ),
    exception_spec("IndexError", ExceptionBaseSpec::One("LookupError"), None),
    exception_spec("KeyError", ExceptionBaseSpec::One("LookupError"), None),
    exception_spec(
        "UnboundLocalError",
        ExceptionBaseSpec::One("NameError"),
        None,
    ),
    exception_spec("ConnectionError", ExceptionBaseSpec::One("OSError"), None),
    exception_spec(
        "BrokenPipeError",
        ExceptionBaseSpec::One("ConnectionError"),
        None,
    ),
    exception_spec(
        "ConnectionAbortedError",
        ExceptionBaseSpec::One("ConnectionError"),
        None,
    ),
    exception_spec(
        "ConnectionRefusedError",
        ExceptionBaseSpec::One("ConnectionError"),
        None,
    ),
    exception_spec(
        "ConnectionResetError",
        ExceptionBaseSpec::One("ConnectionError"),
        None,
    ),
    exception_spec("BlockingIOError", ExceptionBaseSpec::One("OSError"), None),
    exception_spec("ChildProcessError", ExceptionBaseSpec::One("OSError"), None),
    exception_spec("FileExistsError", ExceptionBaseSpec::One("OSError"), None),
    exception_spec("FileNotFoundError", ExceptionBaseSpec::One("OSError"), None),
    exception_spec("InterruptedError", ExceptionBaseSpec::One("OSError"), None),
    exception_spec("IsADirectoryError", ExceptionBaseSpec::One("OSError"), None),
    exception_spec(
        "NotADirectoryError",
        ExceptionBaseSpec::One("OSError"),
        None,
    ),
    exception_spec("PermissionError", ExceptionBaseSpec::One("OSError"), None),
    exception_spec(
        "ProcessLookupError",
        ExceptionBaseSpec::One("OSError"),
        None,
    ),
    exception_spec("TimeoutError", ExceptionBaseSpec::One("OSError"), None),
    exception_spec(
        "UnsupportedOperation",
        ExceptionBaseSpec::Two("OSError", "ValueError"),
        None,
    ),
    exception_spec(
        "NotImplementedError",
        ExceptionBaseSpec::One("RuntimeError"),
        None,
    ),
    exception_spec(
        "PythonFinalizationError",
        ExceptionBaseSpec::One("RuntimeError"),
        None,
    ),
    exception_spec(
        "RecursionError",
        ExceptionBaseSpec::One("RuntimeError"),
        None,
    ),
    exception_spec(
        "IndentationError",
        ExceptionBaseSpec::One("SyntaxError"),
        None,
    ),
    exception_spec("TabError", ExceptionBaseSpec::One("IndentationError"), None),
    exception_spec("UnicodeError", ExceptionBaseSpec::One("ValueError"), None),
    exception_spec(
        "UnicodeDecodeError",
        ExceptionBaseSpec::One("UnicodeError"),
        Some(ExceptionLayoutRoot::UnicodeDecodeError),
    ),
    exception_spec(
        "UnicodeEncodeError",
        ExceptionBaseSpec::One("UnicodeError"),
        Some(ExceptionLayoutRoot::UnicodeEncodeError),
    ),
    exception_spec(
        "UnicodeTranslateError",
        ExceptionBaseSpec::One("UnicodeError"),
        Some(ExceptionLayoutRoot::UnicodeTranslateError),
    ),
    exception_spec(
        "DeprecationWarning",
        ExceptionBaseSpec::One("Warning"),
        None,
    ),
    exception_spec(
        "PendingDeprecationWarning",
        ExceptionBaseSpec::One("Warning"),
        None,
    ),
    exception_spec("RuntimeWarning", ExceptionBaseSpec::One("Warning"), None),
    exception_spec("SyntaxWarning", ExceptionBaseSpec::One("Warning"), None),
    exception_spec("UserWarning", ExceptionBaseSpec::One("Warning"), None),
    exception_spec("FutureWarning", ExceptionBaseSpec::One("Warning"), None),
    exception_spec("ImportWarning", ExceptionBaseSpec::One("Warning"), None),
    exception_spec("UnicodeWarning", ExceptionBaseSpec::One("Warning"), None),
    exception_spec("BytesWarning", ExceptionBaseSpec::One("Warning"), None),
    exception_spec("ResourceWarning", ExceptionBaseSpec::One("Warning"), None),
    exception_spec("EncodingWarning", ExceptionBaseSpec::One("Warning"), None),
    exception_alias("EnvironmentError", "OSError"),
    exception_alias("IOError", "OSError"),
    exception_alias("WindowsError", "OSError"),
];

pub fn builtin_exception_spec(name: &str) -> Option<&'static BuiltinExceptionSpec> {
    BUILTIN_EXCEPTION_SPECS
        .iter()
        .find(|spec| spec.name == name)
}

#[cfg(test)]
const fn builtin_exception_specs() -> &'static [BuiltinExceptionSpec] {
    BUILTIN_EXCEPTION_SPECS
}

impl BuiltinExceptionSpec {
    #[cfg(test)]
    const fn is_alias(&self) -> bool {
        matches!(self.definition, BuiltinExceptionDefinition::Alias { .. })
    }

    /// Resolve aliases to the one row that owns hierarchy and storage.
    fn canonical(&'static self) -> &'static BuiltinExceptionSpec {
        match self.definition {
            BuiltinExceptionDefinition::Canonical { .. } => self,
            BuiltinExceptionDefinition::Alias { canonical_name } => {
                builtin_exception_spec(canonical_name)
                    .expect("builtin exception alias must name a canonical row")
            }
        }
    }

    pub fn canonical_name(&'static self) -> &'static str {
        self.canonical().name
    }

    pub fn bases(&'static self) -> ExceptionBaseSpec {
        match self.canonical().definition {
            BuiltinExceptionDefinition::Canonical { bases, .. } => bases,
            BuiltinExceptionDefinition::Alias { .. } => {
                unreachable!("canonical exception row cannot be an alias")
            }
        }
    }

    pub fn layout_root(&'static self) -> ExceptionLayoutRoot {
        match self.canonical().definition {
            BuiltinExceptionDefinition::Canonical {
                introduced_layout_root: Some(root),
                ..
            } => root,
            BuiltinExceptionDefinition::Canonical {
                bases: ExceptionBaseSpec::Root,
                introduced_layout_root: None,
            } => ExceptionLayoutRoot::Base,
            BuiltinExceptionDefinition::Canonical {
                bases: ExceptionBaseSpec::One(parent) | ExceptionBaseSpec::Two(parent, _),
                introduced_layout_root: None,
            } => builtin_exception_spec(parent)
                .expect("builtin exception parent must remain registered")
                .layout_root(),
            BuiltinExceptionDefinition::Alias { .. } => {
                unreachable!("canonical exception row cannot be an alias")
            }
        }
    }

    pub fn layout(&'static self) -> ExceptionLayoutKind {
        self.layout_root().kind()
    }

    /// Only the canonical class that introduces a non-base physical layout is
    /// explicitly tagged. Descendants inherit the root through their base.
    pub fn introduced_layout_root(&'static self) -> Option<ExceptionLayoutRoot> {
        match self.definition {
            BuiltinExceptionDefinition::Canonical {
                introduced_layout_root,
                ..
            } => introduced_layout_root,
            BuiltinExceptionDefinition::Alias { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_are_dense_and_field_tables_match_policies() {
        for (raw, kind) in ExceptionLayoutKind::ALL.iter().copied().enumerate() {
            assert_eq!(kind as usize, raw);
            let policies = kind.field_policies();
            for policy in policies {
                assert!(!policy.python_name.is_empty());
            }
            assert_eq!(
                kind.tail_word_count(),
                policies
                    .iter()
                    .filter(|policy| policy.storage != ExceptionFieldStorage::RuntimeMessage)
                    .count()
            );
        }
        assert_eq!(ExceptionLayoutKind::from_u8(10), None);
        assert_eq!(
            ExceptionLayoutKind::ALL
                .iter()
                .copied()
                .map(|kind| kind.field_policies().len())
                .max(),
            Some(MAX_EXCEPTION_TYPED_FIELDS)
        );
        assert_eq!(
            ExceptionLayoutKind::ALL
                .iter()
                .copied()
                .map(ExceptionLayoutKind::tail_word_count)
                .max(),
            Some(MAX_EXCEPTION_TYPED_TAIL_WORDS)
        );
    }

    #[test]
    fn typed_descendants_and_aliases_share_one_layout_authority() {
        assert_eq!(
            builtin_exception_spec("TabError").map(|spec| spec.layout()),
            Some(ExceptionLayoutKind::Syntax)
        );
        assert_eq!(
            builtin_exception_spec("ModuleNotFoundError").map(|spec| spec.layout()),
            Some(ExceptionLayoutKind::Import)
        );
        assert_eq!(
            builtin_exception_spec("UnicodeError").map(|spec| spec.layout()),
            Some(ExceptionLayoutKind::Base)
        );
        assert_eq!(
            builtin_exception_spec("UnicodeTranslateError").map(|spec| spec.layout()),
            Some(ExceptionLayoutKind::Unicode)
        );
        for name in ["OSError", "IOError", "EnvironmentError", "WindowsError"] {
            assert_eq!(
                builtin_exception_spec(name).map(|spec| spec.layout()),
                Some(ExceptionLayoutKind::OSError)
            );
        }
        assert_eq!(
            builtin_exception_spec("UnboundLocalError").map(|spec| spec.layout()),
            Some(ExceptionLayoutKind::NameError)
        );
        assert_eq!(builtin_exception_spec("UserException"), None);
        assert_eq!(
            builtin_exception_spec("UnicodeDecodeError").map(|spec| spec.layout_root()),
            Some(ExceptionLayoutRoot::UnicodeDecodeError)
        );
        assert_eq!(
            builtin_exception_spec("UnicodeEncodeError").map(|spec| spec.layout_root()),
            Some(ExceptionLayoutRoot::UnicodeEncodeError)
        );
        assert_eq!(
            builtin_exception_spec("UnicodeTranslateError").map(|spec| spec.layout_root()),
            Some(ExceptionLayoutRoot::UnicodeTranslateError)
        );
        assert_eq!(
            ExceptionLayoutRoot::UnicodeDecodeError.kind(),
            ExceptionLayoutRoot::UnicodeEncodeError.kind()
        );
    }

    #[test]
    fn cpython_312_special_field_policies_are_explicit() {
        let group = ExceptionLayoutKind::Group.field_policies();
        assert!(
            group
                .iter()
                .all(|policy| !policy.writable && !policy.deletable)
        );
        assert_eq!(group[0].storage, ExceptionFieldStorage::RuntimeMessage);
        assert_eq!(
            ExceptionLayoutKind::Group
                .tail_policies()
                .iter()
                .map(|policy| policy.field)
                .collect::<Vec<_>>(),
            [ExceptionTypedField::GroupExceptions]
        );

        let name_from = ExceptionLayoutKind::Import
            .field_policy(ExceptionTypedField::ImportNameFrom)
            .expect("ImportError name_from field");
        assert!(name_from.writable && name_from.deletable);
        assert!(name_from.constructor_keyword);
        assert_eq!(name_from.missing_read, ExceptionMissingRead::None);

        let unicode_object = ExceptionLayoutKind::Unicode
            .field_policy(ExceptionTypedField::UnicodeObject)
            .expect("Unicode object field");
        assert!(!unicode_object.constructor_keyword);

        let written = ExceptionLayoutKind::OSError
            .field_policy(ExceptionTypedField::OSErrorCharactersWritten)
            .expect("OSError characters_written policy");
        assert_eq!(written.storage, ExceptionFieldStorage::PySsize);
        assert!(written.writable && written.deletable);
        assert!(!written.constructor_keyword);
        assert_eq!(written.missing_read, ExceptionMissingRead::AttributeError);

        assert_eq!(
            ExceptionLayoutKind::AttributeError
                .field_policies()
                .iter()
                .map(|policy| policy.field)
                .collect::<Vec<_>>(),
            [
                ExceptionTypedField::AttributeErrorObject,
                ExceptionTypedField::AttributeErrorName,
            ]
        );
    }

    #[test]
    fn hierarchy_and_aliases_share_the_schema() {
        let mut names = std::collections::HashSet::new();
        for spec in builtin_exception_specs() {
            assert!(
                names.insert(spec.name),
                "duplicate schema row: {}",
                spec.name
            );
            assert!(
                builtin_exception_spec(spec.canonical_name()).is_some(),
                "missing canonical row: {}",
                spec.canonical_name()
            );
            assert!(!spec.canonical().is_alias(), "alias chain: {}", spec.name);
            match spec.bases() {
                ExceptionBaseSpec::Root => assert_eq!(spec.name, "BaseException"),
                ExceptionBaseSpec::One(parent) | ExceptionBaseSpec::Two(parent, _) => {
                    assert!(
                        builtin_exception_spec(parent).is_some(),
                        "missing parent: {parent}"
                    );
                }
            }
            if let ExceptionBaseSpec::Two(_, secondary) = spec.bases() {
                assert!(
                    builtin_exception_spec(secondary).is_some(),
                    "missing secondary parent: {secondary}"
                );
            }
        }
        assert_eq!(
            builtin_exception_spec("BaseException").map(|spec| spec.bases()),
            Some(ExceptionBaseSpec::Root)
        );
        assert_eq!(
            builtin_exception_spec("ExceptionGroup").map(|spec| spec.bases()),
            Some(ExceptionBaseSpec::Two("BaseExceptionGroup", "Exception"))
        );
        assert_eq!(
            builtin_exception_spec("IOError").map(|spec| spec.canonical_name()),
            Some("OSError")
        );
        assert_eq!(
            builtin_exception_spec("IOError").unwrap().canonical().name,
            "OSError"
        );
        assert_eq!(builtin_exception_specs().len(), 73);
        assert_eq!(
            builtin_exception_spec("SyntaxError").and_then(|spec| spec.introduced_layout_root()),
            Some(ExceptionLayoutRoot::SyntaxError)
        );
        assert_eq!(
            builtin_exception_spec("TabError").and_then(|spec| spec.introduced_layout_root()),
            None
        );
        assert_eq!(
            builtin_exception_spec("IOError").and_then(|spec| spec.introduced_layout_root()),
            None
        );
        assert_eq!(
            builtin_exception_spec("UnicodeEncodeError")
                .and_then(|spec| spec.introduced_layout_root()),
            Some(ExceptionLayoutRoot::UnicodeEncodeError)
        );
        assert_eq!(
            builtin_exception_spec("IOError").map(|spec| spec.bases()),
            Some(ExceptionBaseSpec::One("Exception"))
        );
        assert_eq!(builtin_exception_spec("NotAnException"), None);
    }
}
