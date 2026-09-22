"""Builtin publication, constructor identity, target gates, and deletion custody."""

import builtins
import importlib.machinery
import sys

print(
    all(
        isinstance(getattr(builtins, name), type)
        for name in (
            "property",
            "classmethod",
            "staticmethod",
            "enumerate",
            "filter",
            "map",
            "reversed",
            "zip",
        )
    )
)
print(
    all(
        not hasattr(builtins, name)
        for name in ("NoneType", "list_iterator", "GenericAlias")
    )
)
print(hasattr(builtins, "PythonFinalizationError") == (sys.version_info >= (3, 13)))
print(hasattr(builtins, "WindowsError") == (sys.platform == "win32"))
print(importlib.machinery.ModuleSpec("pkg.leaf", None).parent == "pkg")

# Cold startup must finish builtin capture before recursive loader metadata;
# user code sees the original namespace and complete parent/module specs.
print(
    all(
        module.__spec__.name == module.__name__
        and isinstance(module.__spec__, importlib.machinery.ModuleSpec)
        for module in (builtins, sys, importlib)
    )
)
frame = sys._getframe()
print(frame.f_builtins is builtins.__dict__)
print(frame.f_globals is globals(), frame.f_locals is globals())
del frame

# Runtime exception identity must not repair a deleted Python namespace entry.
name = "Value" + "Error"
saved = getattr(builtins, name)
try:
    delattr(builtins, name)
    error = saved("still constructible")
    print(type(error) is saved, not hasattr(builtins, name))
    print(str(error), not hasattr(builtins, name))
finally:
    setattr(builtins, name, saved)
print(getattr(builtins, name) is saved)
