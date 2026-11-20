"""Builtin exception classes called positionally construct exactly as CPython.

Every typed layout family appears: base, attribute, name, import, OSError
(with errno promotion to a subclass, and BlockingIOError's written count),
StopIteration, SystemExit, syntax, Unicode and exception groups. Classes whose
constructor validates its arguments must raise the same error, also when the
constructed value is unused. A local bound to a builtin class constructs the
same class.
"""

import errno


def show(error):
    print(type(error).__name__, error.args, repr(str(error)))


show(AttributeError("missing"))
attribute = AttributeError("missing")
print("attribute fields", attribute.name, attribute.obj)
name_error = NameError("undefined")
print("name field", name_error.name)
import_error = ImportError("no module")
print("import fields", import_error.msg, import_error.name, import_error.path)
show(KeyError("key"))
show(LookupError())
show(ZeroDivisionError("division", 0))
print("stop value", StopIteration(5).value, StopIteration().value)
print("exit code", SystemExit(3).code, SystemExit().code, SystemExit(1, 2).code)

missing_file = OSError(2, "No such file", "input.txt")
print(
    "oserror promotion",
    type(missing_file).__name__,
    missing_file.errno,
    missing_file.strerror,
    missing_file.filename,
    missing_file.args,
)
print("oserror unknown errno", type(OSError(99999, "odd")).__name__)
print("oserror one arg", type(OSError("plain")).__name__, OSError("plain").errno)
blocking = OSError(errno.EAGAIN, "again")
print("blocking", type(blocking).__name__)
print("file exists", type(FileExistsError(2, "x")).__name__)

encode = UnicodeEncodeError("utf-8", "x€", 1, 2, "no euro")
print("encode", encode.encoding, encode.object, encode.start, encode.end, str(encode))
decode = UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid start")
print("decode", decode.object, str(decode))
try:
    UnicodeEncodeError("utf-8", "x")
except TypeError as exc:
    print("encode refused", exc)
try:
    UnicodeDecodeError("utf-8", "not bytes", 0, 1, "reason")
except TypeError as exc:
    print("decode refused", exc)

syntax = SyntaxError("bad", ("file.py", 3, 4, "text\n"))
print("syntax", syntax.msg, syntax.filename, syntax.lineno, syntax.offset, syntax.text)
try:
    SyntaxError("bad", ("file.py", 3))
except TypeError as exc:
    print("syntax refused", exc)

group = ExceptionGroup("several", [ValueError(1), TypeError(2)])
print("group", type(group).__name__, group.message, len(group.exceptions))
base_group = BaseExceptionGroup("plain", [ValueError(1)])
print("base group answers", type(base_group).__name__)
base_only = BaseExceptionGroup("mixed", [KeyboardInterrupt()])
print("base group keeps", type(base_only).__name__)
try:
    ExceptionGroup("empty", [])
except ValueError as exc:
    print("group refused", exc)
try:
    ExceptionGroup("one argument")
except TypeError as exc:
    print("group arity", exc)


def make_alias_errors():
    error_class = ValueError
    os_class = OSError
    return error_class("through alias"), os_class(2, "aliased")


aliased, aliased_os = make_alias_errors()
show(aliased)
print("aliased os", type(aliased_os).__name__)

try:
    raise IndexError("raised", 7)
except IndexError as exc:
    show(exc)
