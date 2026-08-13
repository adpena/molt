"""Purpose: one end-to-end authority for all nine typed exception layouts."""


def fields(label, exc, names):
    values = []
    for name in names:
        try:
            values.append((name, getattr(exc, name)))
        except Exception as error:
            values.append((name, type(error).__name__, str(error)))
    print(label, type(exc).__name__, exc.args, values, sorted(exc.__dict__))


def set_field(label, exc, name, value):
    try:
        setattr(exc, name, value)
        print(label, "set", name, getattr(exc, name), sorted(exc.__dict__))
    except Exception as error:
        print(label, "set", name, type(error).__name__, str(error), sorted(exc.__dict__))


def delete_field(label, exc, name):
    try:
        delattr(exc, name)
        try:
            value = getattr(exc, name)
        except Exception as error:
            value = (type(error).__name__, str(error))
        print(label, "del", name, value, sorted(exc.__dict__))
    except Exception as error:
        print(label, "del", name, type(error).__name__, str(error), sorted(exc.__dict__))


def constructor_error(label, constructor):
    try:
        constructor()
        print(label, "unexpected-success")
    except Exception as error:
        print(label, type(error).__name__, str(error))


# AttributeError: two typed keyword fields, inherited layout, atomic updates,
# deletion-to-None, and no accidental __dict__ publication.
attribute = AttributeError("attribute", name="missing", obj="owner")
fields("attribute", attribute, ("name", "obj"))
set_field("attribute", attribute, "name", "changed")
set_field("attribute", attribute, "obj", "changed-owner")
delete_field("attribute", attribute, "name")
delete_field("attribute", attribute, "obj")


class DerivedAttributeError(AttributeError):
    pass


derived_attribute = DerivedAttributeError("derived", name="child", obj="derived-owner")
fields("attribute-derived", derived_attribute, ("name", "obj"))
AttributeError.__init__(derived_attribute, "reset")
fields("attribute-reset", derived_attribute, ("name", "obj"))
constructor_error("attribute-invalid", lambda: AttributeError("x", bad=1))
constructor_error(
    "attribute-too-many",
    lambda: AttributeError("x", name="n", obj="o", bad=1),
)
attribute_direct = AttributeError("old")
AttributeError.__init__(attribute_direct, "direct", name="direct-name", obj="direct-owner")
fields("attribute-direct-init", attribute_direct, ("name", "obj"))
attribute_failed = AttributeError("old", name="old-name", obj="old-owner")
constructor_error(
    "attribute-direct-invalid",
    lambda: AttributeError.__init__(attribute_failed, "changed", bad=1),
)
fields("attribute-direct-invalid-state", attribute_failed, ("name", "obj"))


# NameError: one keyword field and descendants share the physical layout.
name_error = NameError("name", name="symbol")
fields("name", name_error, ("name",))
set_field("name", name_error, "name", "changed")
delete_field("name", name_error, "name")


class DerivedNameError(NameError):
    pass


derived_name = DerivedNameError("derived-name", name="child")
fields("name-derived", derived_name, ("name",))
NameError.__init__(derived_name, "reset")
fields("name-reset", derived_name, ("name",))
constructor_error("name-invalid", lambda: NameError("x", bad=1))
constructor_error("name-too-many", lambda: NameError("x", name="n", bad=1))
name_direct = DerivedNameError("old")
NameError.__init__(name_direct, "direct", name="direct-name")
fields("name-direct-init", name_direct, ("name",))
constructor_error(
    "name-direct-wrong-owner",
    lambda: NameError.__init__(AttributeError("wrong"), "changed"),
)
base_direct = AttributeError("base-old")
constructor_error(
    "base-direct-keyword",
    lambda: BaseException.__init__(base_direct, "base-new", name="forbidden"),
)
fields("base-direct-keyword-state", base_direct, ("name", "obj"))


# ImportError: name_from is a real typed descriptor (though absent from
# __dict__), alongside name/path, and ModuleNotFoundError inherits the layout.
import_error = ImportError("import", name="package", path="where", name_from="source")
fields("import", import_error, ("msg", "name", "path", "name_from"))
for field_name in ("msg", "name", "path", "name_from"):
    set_field("import", import_error, field_name, "changed-" + field_name)
for field_name in ("msg", "name", "path", "name_from"):
    delete_field("import", import_error, field_name)
module_error = ModuleNotFoundError("module", name="missing", path="root", name_from="member")
fields("import-derived", module_error, ("msg", "name", "path", "name_from"))
ImportError.__init__(module_error, "reset")
fields("import-reset", module_error, ("msg", "name", "path", "name_from"))
constructor_error("import-invalid", lambda: ImportError("x", bad=1))
constructor_error(
    "import-too-many",
    lambda: ImportError("x", name="n", path="p", name_from="f", bad=1),
)
import_direct = ModuleNotFoundError("old")
ImportError.__init__(
    import_direct,
    "direct",
    name="direct-name",
    path="direct-path",
    name_from="direct-from",
)
fields("import-direct-init", import_direct, ("msg", "name", "path", "name_from"))


# Syntax family: all object fields are writable/deletable and aliases inherit
# the same layout.
syntax = SyntaxError("syntax", ("example.py", 2, 3, "bad\n", 4, 6))
syntax_names = (
    "msg",
    "filename",
    "lineno",
    "offset",
    "end_lineno",
    "end_offset",
    "text",
    "print_file_and_line",
)
fields("syntax", syntax, syntax_names)
for field_name in syntax_names:
    set_field("syntax", syntax, field_name, 9)
for field_name in syntax_names:
    delete_field("syntax", syntax, field_name)
tab_error = TabError("tab", ("tab.py", 1, 2, "\tbad\n", 1, 4))
fields("syntax-derived", tab_error, syntax_names)
syntax_reinit = SyntaxError("old", ("old.py", 1, 2, "old\n", 3, 4))
SyntaxError.__init__(syntax_reinit, "new")
fields("syntax-reinit-message", syntax_reinit, syntax_names)
SyntaxError.__init__(syntax_reinit, "newer", ("new.py", 5, 6, "new\n", 7, 8))
fields("syntax-reinit-location", syntax_reinit, syntax_names)
SyntaxError.__init__(syntax_reinit, "short", ("short.py", 9, 10, "short\n"))
fields("syntax-reinit-clears-optional", syntax_reinit, syntax_names)
constructor_error("syntax-keyword", lambda: SyntaxError("x", bad=1))


# Unicode family: object and Py_ssize fields share conversion authority;
# numeric fields reject deletion instead of silently becoming dictionary keys.
unicode_error = UnicodeEncodeError("utf-8", "x", 0, 1, "reason")
unicode_names = ("encoding", "object", "start", "end", "reason")
fields("unicode", unicode_error, unicode_names)
for field_name, value in (
    ("encoding", "latin-1"),
    ("object", "xy"),
    ("start", 1),
    ("end", 2),
    ("reason", "changed"),
):
    set_field("unicode", unicode_error, field_name, value)
set_field("unicode-invalid-scalar", unicode_error, "start", "bad")
delete_field("unicode", unicode_error, "encoding")
delete_field("unicode", unicode_error, "start")
constructor_error(
    "unicode-keyword",
    lambda: UnicodeEncodeError("utf-8", "x", 0, 1, "reason", bad=1),
)
constructor_error("unicode-wrong-arity", lambda: UnicodeEncodeError("utf-8", "x", 0, 1))
unicode_reinit = UnicodeEncodeError("utf-8", "x", 0, 1, "old")
UnicodeEncodeError.__init__(unicode_reinit, "ascii", "yz", 1, 2, "new")
fields("unicode-reinit", unicode_reinit, unicode_names)
try:
    UnicodeEncodeError.__init__(unicode_reinit, "bad")
except Exception as error:
    print("unicode-reinit-error", type(error).__name__, str(error))
fields("unicode-reinit-error-state", unicode_reinit, unicode_names)

# Decode and translate are distinct constructor roots despite sharing the same
# physical tail: decode accepts the buffer protocol and owns bytes, while
# translate has no encoding field and takes four arguments.
unicode_decode = UnicodeDecodeError("utf-8", bytearray(b"xy"), 0, 1, "decode")
fields("unicode-decode", unicode_decode, unicode_names)
print("unicode-decode-object-type", type(unicode_decode.object).__name__)
unicode_translate = UnicodeTranslateError("xy", 0, 1, "translate")
fields("unicode-translate", unicode_translate, unicode_names)
constructor_error(
    "unicode-translate-wrong-arity",
    lambda: UnicodeTranslateError("xy", 0, 1, "translate", "extra"),
)


# SystemExit and StopIteration each own one value field with positional reset.
system_exit = SystemExit(7)
fields("system-exit", system_exit, ("code",))
set_field("system-exit", system_exit, "code", 8)
delete_field("system-exit", system_exit, "code")
SystemExit.__init__(system_exit, "reset")
fields("system-exit-reset", system_exit, ("code",))
constructor_error("system-exit-keyword", lambda: SystemExit(1, bad=1))

stop = StopIteration(11)
fields("stop", stop, ("value",))
set_field("stop", stop, "value", 12)
delete_field("stop", stop, "value")
StopIteration.__init__(stop, "reset")
fields("stop-reset", stop, ("value",))
fields("stop-multi", StopIteration(1, 2), ("value",))
constructor_error("stop-keyword", lambda: StopIteration(1, bad=1))

fields("system-exit-multi", SystemExit(1, 2), ("code",))
system_exit_preserve = SystemExit(17)
SystemExit.__init__(system_exit_preserve)
fields("system-exit-zero-reinit", system_exit_preserve, ("code",))


# OSError's physical written slot and missing-aware descriptor are universal;
# only BlockingIOError's positional constructor initializes it from argument 3.
os_error = OSError(2, "missing", "input.txt")
os_names = ["errno", "strerror", "filename", "filename2"]
if hasattr(os_error, "winerror"):
    os_names.append("winerror")
fields("os", os_error, os_names)
fields("os-owner-gate", os_error, ("characters_written",))
set_field("os-owner-gate", os_error, "characters_written", 5)
print("os-owner-gate-dict", os_error.__dict__)
delete_field("os-owner-gate", os_error, "characters_written")
constructor_error("os-keyword", lambda: OSError(2, "missing", bad=1))
os_reinit = OSError(12345, "old", "old.txt")
OSError.__init__(os_reinit, 54321, "new", "new.txt")
fields("os-reinit-noop", os_reinit, ("errno", "strerror", "filename", "filename2"))

blocking = BlockingIOError(1, "blocked", 4)
fields("blocking", blocking, ("characters_written",))
set_field("blocking", blocking, "characters_written", 5)
delete_field("blocking", blocking, "characters_written")


class DerivedBlockingIOError(BlockingIOError):
    pass


derived_blocking = DerivedBlockingIOError(1, "blocked", 6)
fields("blocking-derived", derived_blocking, ("characters_written",))


# ExceptionGroup's two typed fields are inherited read-only data descriptors.
group = ExceptionGroup("group", [ValueError("value")])
fields("group", group, ("message", "exceptions"))
for field_name in ("message", "exceptions"):
    set_field("group", group, field_name, None)
    delete_field("group", group, field_name)
constructor_error(
    "group-keyword",
    lambda: ExceptionGroup("group", [ValueError("value")], bad=1),
)


class DerivedExceptionGroup(ExceptionGroup):
    pass


derived_group = DerivedExceptionGroup("derived-group", [TypeError("type")])
fields("group-derived", derived_group, ("message", "exceptions"))
BaseExceptionGroup.__init__(derived_group, "new-args", [ValueError("new")])
fields("group-reinit", derived_group, ("message", "exceptions"))

group_narrowed = BaseExceptionGroup("narrow", [ValueError("value")])
group_base = BaseExceptionGroup("base", [KeyboardInterrupt()])
print("group-selection", type(group_narrowed).__name__, type(group_base).__name__)
constructor_error(
    "group-reject-base",
    lambda: ExceptionGroup("invalid", [KeyboardInterrupt()]),
)


# Distinct non-base physical layouts cannot be combined through Python MI.
try:
    class ConflictingExceptionLayout(OSError, SyntaxError):
        pass
except Exception as error:
    print("layout-conflict", type(error).__name__, str(error))


class EncodeA(UnicodeEncodeError):
    pass


class EncodeB(UnicodeEncodeError):
    pass


class DecodeA(UnicodeDecodeError):
    pass


try:
    class ConflictingUnicodeRoots(UnicodeEncodeError, UnicodeDecodeError):
        pass
except Exception as error:
    print("unicode-root-conflict", type(error).__name__, str(error))

try:
    class ConflictingDerivedUnicodeRoots(EncodeA, DecodeA):
        pass
except Exception as error:
    print("unicode-derived-root-conflict", type(error).__name__, str(error))


class SharedUnicodeRoot(EncodeA, EncodeB):
    pass


print(
    "unicode-shared-root",
    issubclass(SharedUnicodeRoot, EncodeA),
    issubclass(SharedUnicodeRoot, EncodeB),
)

# A class name is never physical-layout authority. User exceptions that merely
# reuse builtin names inherit plain Exception storage, expose no typed builtin
# descriptors, and remain layout-compatible with one another.
class OSError(Exception):
    pass


class SyntaxError(Exception):
    pass


shadow_os = OSError("shadow-os")
shadow_syntax = SyntaxError("shadow-syntax")
print(
    "shadow-layout",
    hasattr(shadow_os, "errno"),
    hasattr(shadow_os, "characters_written"),
    hasattr(shadow_syntax, "msg"),
    hasattr(shadow_syntax, "filename"),
)


class SharedShadowLayout(OSError, SyntaxError):
    pass


print(
    "shadow-layout-mi",
    issubclass(SharedShadowLayout, OSError),
    issubclass(SharedShadowLayout, SyntaxError),
)

constructor_error("base-keyword", lambda: ValueError("value", bad=1))
