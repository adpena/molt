"""Typed members of builtin exceptions built by their constructors: compiled
code loads them at their layout slots (call_anatomy.md 4.5a, "Builtin
layouts"); absent, deleted and rebound members, errno promotion and a user
subclass that overrides a member keep CPython's answers."""


class Owner:
    pass


owner = Owner()

attribute = AttributeError("missing", name="field", obj=owner)
print("attribute", attribute.name, attribute.obj is owner)
bare = AttributeError("bare")
print("attribute-absent", bare.name, bare.obj)
attribute.name = "renamed"
print("attribute-rebound", attribute.name)
del attribute.name
print("attribute-deleted", attribute.name)

os_error = OSError(2, "missing", "input.txt")
print("oserror", type(os_error).__name__, os_error.errno, os_error.strerror)
print("oserror-files", os_error.filename, os_error.filename2)
plain_os = OSError("plain")
print("oserror-plain", plain_os.errno, plain_os.strerror, plain_os.filename)
two_files = OSError(13, "denied", "a.txt", None, "b.txt")
print("oserror-two", type(two_files).__name__, two_files.filename, two_files.filename2)
try:
    plain_os.characters_written
except AttributeError as error:
    print("characters_written", "AttributeError", error)

encode = UnicodeEncodeError("utf-8", "xyz", 1, 2, "reason")
print("encode", encode.encoding, encode.object, encode.start, encode.end, encode.reason)
decode = UnicodeDecodeError("ascii", b"\xff\xfe", 0, 1, "bad")
print("decode", decode.encoding, decode.object, decode.start, decode.end, decode.reason)
translate = UnicodeTranslateError("abc", 0, 3, "why")
print("translate", translate.object, translate.start, translate.end, translate.reason)

import_error = ImportError("nope", name="pkg", path="/tmp/pkg")
print("import", import_error.msg, import_error.name, import_error.path)
print("import-absent", ImportError().msg, ImportError().name)
syntax = SyntaxError("bad", ("f.py", 3, 4, "text", 3, 5))
print("syntax", syntax.msg, syntax.filename, syntax.lineno, syntax.offset)
print("syntax-tail", syntax.text, syntax.end_lineno, syntax.end_offset)
print("stop", StopIteration(5).value, StopIteration().value)
print("exit", SystemExit(3).code, SystemExit().code)
print("name", NameError("x", name="x").name, NameError("y").name)
group = ExceptionGroup("several", [ValueError(1), TypeError(2)])
print("group", group.message, [type(e).__name__ for e in group.exceptions])


class OverridingError(AttributeError):
    @property
    def name(self):
        return "overridden"


print("override", OverridingError("x", name="real").name)
total = 0
for index in range(3):
    error = AttributeError("loop", name="n", obj=index)
    total += error.obj
    total += error.name == "n"
print("loop", total)
