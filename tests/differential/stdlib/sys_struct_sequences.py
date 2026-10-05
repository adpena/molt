"""sys struct sequences keep CPython's type identity, fields and repr."""

import sys

for name in ("flags", "version_info", "float_info", "int_info", "hash_info", "thread_info"):
    value = getattr(sys, name)
    cls = type(value)
    print(name, cls.__name__, cls.__module__, cls.n_fields, cls.n_sequence_fields, cls.n_unnamed_fields)
    print(" ", isinstance(value, tuple), len(value) == cls.n_fields, repr(value).startswith(f"sys.{name}("))

print("version", sys.version_info[:2] == (sys.version_info.major, sys.version_info.minor))
major, minor, micro, level, serial = sys.version_info
print("unpack", (major, minor) == sys.version_info[:2], level == sys.version_info.releaselevel)
print("compare", sys.version_info >= (3, 0), sys.version_info < (99,))
print("flags", sys.flags.optimize == sys.flags[3], sys.flags.debug == sys.flags[0])
print("hash", sys.hash_info.width in (32, 64), sys.hash_info.algorithm == sys.hash_info[5])

hooks = sys.get_asyncgen_hooks()
cls = type(hooks)
print("hooks", cls.__name__, cls.__module__, cls.n_fields, list(hooks), repr(hooks))
print("hooks-fields", hooks.firstiter is None, hooks.finalizer is None)
sys.set_asyncgen_hooks(*hooks)
print("hooks-restore", list(sys.get_asyncgen_hooks()))
print("no-leak", hasattr(sys, "asyncgen_hooks"))

try:
    sys.version_info.nonexistent
except AttributeError:
    print("missing-field AttributeError")

for name in ("flags", "version_info", "float_info", "int_info", "hash_info", "thread_info"):
    cls = type(getattr(sys, name))
    try:
        made = cls(tuple(range(cls.n_sequence_fields)))
        print("construct", name, "ok", len(made), type(made) is cls)
    except TypeError as exc:
        print("construct", name, "TypeError", exc)
try:
    type(sys.float_info)((1, 2))
except TypeError as exc:
    print("short", exc)

print("flag-types", type(sys.flags.dev_mode).__name__, type(sys.flags.safe_path).__name__, type(sys.flags.optimize).__name__)
print("flag-gil", hasattr(sys.flags, "gil") == (sys.version_info >= (3, 13)))
print("flag-n_fields", type(sys.flags).n_fields - type(sys.flags).n_sequence_fields == {12: 0, 13: 1}.get(sys.version_info[1], 3))
