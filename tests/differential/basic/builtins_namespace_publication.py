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
        for name in (
            "NoneType",
            "list_iterator",
            "GenericAlias",
            "_molt_asyncgen_hooks_get",
            "_molt_getargv",
            "_molt_class_new",
        )
    )
)
print(hasattr(builtins, "PythonFinalizationError") == (sys.version_info >= (3, 13)))
print(hasattr(builtins, "WindowsError") == (sys.platform == "win32"))
print(importlib.machinery.ModuleSpec("pkg.leaf", None).parent == "pkg")
print(builtins.len.__module__, builtins.open.__module__)

# Cold startup must finish builtin capture before recursive loader metadata;
# user code sees the original namespace and complete parent/module specs.
print(
    all(
        module.__spec__.name == module.__name__
        and isinstance(module.__spec__, importlib.machinery.ModuleSpec)
        for module in (builtins, sys, importlib, importlib.machinery)
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

# Public bootstrap loaders inherit the object repr/str protocol. Their
# runtime origin, subclass dispatch and class metadata are CPython-visible.
for loader_type in (
    importlib.machinery.BuiltinImporter,
    importlib.machinery.FrozenImporter,
):
    loader = loader_type()
    print(loader_type.__name__, loader_type.__module__)
    print(repr(loader) == object.__repr__(loader), str(loader) == repr(loader))
    assert repr(loader).startswith(
        "<_frozen_importlib." + loader_type.__name__ + " object at 0x"
    )
    assert repr(loader).endswith(">")


class CustomLoader(importlib.machinery.BuiltinImporter):
    def __repr__(self):
        return "custom-loader"


print(repr(CustomLoader()), str(CustomLoader()))
