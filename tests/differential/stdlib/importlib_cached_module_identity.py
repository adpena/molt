"""An installed module's namespace never determines whether it is imported."""

import importlib
import importlib.machinery
import sys
import types


class OpaqueModule(types.ModuleType):
    def __getattribute__(self, name):
        if name in ("__name__", "__file__"):
            raise AssertionError("a cached import must not inspect module metadata")
        return super().__getattribute__(name)


for name, module in (
    ("cached_empty", types.ModuleType("cached_empty")),
    ("cached_private", types.ModuleType("cached_private")),
    ("multiprocessing_cached", types.ModuleType("multiprocessing_cached")),
    ("cached_opaque", OpaqueModule("cached_opaque")),
    ("cached_object", object()),
    ("cached_int", 42),
    ("cached_dict", {}),
):
    if name == "cached_private":
        module._molt_runtime = object()
    sys.modules[name] = module
    try:
        print(name, importlib.import_module(name) is module, __import__(name) is module)
    finally:
        del sys.modules[name]
    try:
        importlib.import_module(name)
    except ModuleNotFoundError as error:
        print("deleted", name, error.name == name)
    else:
        raise AssertionError("deleted public cache entry was revived")

sys.modules["cached_none"] = None
try:
    importlib.import_module("cached_none")
except ModuleNotFoundError:
    print("none blocks import")
finally:
    del sys.modules["cached_none"]


class SpecFailure(types.ModuleType):
    def __getattribute__(self, name):
        if name == "__spec__":
            raise ValueError("spec probe")
        return super().__getattribute__(name)


module = SpecFailure("cached_probe")
sys.modules["cached_probe"] = module
try:
    try:
        importlib.import_module("cached_probe")
    except ValueError as error:
        print(type(error).__name__, str(error))
    try:
        print("builtin probe", __import__("cached_probe") is module)
    except ValueError as error:
        print("builtin probe", type(error).__name__, str(error))
finally:
    del sys.modules["cached_probe"]


# Removed stdlib providers remain replaceable by an admitted Python loader.
class BackportLoader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        module.marker = "backport:" + module.__name__


class BackportFinder:
    def find_spec(self, fullname, path=None, target=None):
        if fullname in ("msilib", "msilib.schema"):
            return importlib.machinery.ModuleSpec(
                fullname, BackportLoader(), is_package=fullname == "msilib"
            )
        return None


saved = {name: sys.modules.pop(name) for name in ("msilib.schema", "msilib") if name in sys.modules}
finder = BackportFinder()
sys.meta_path.insert(0, finder)
try:
    backport = importlib.import_module("msilib")
    child = importlib.import_module("msilib.schema")
    print("backport-child", child.marker, backport.schema is child)
    del sys.modules["msilib.schema"]
    second = importlib.import_module("msilib.schema")
    print("backport-reimport", second is not child, backport.schema is second)
finally:
    sys.meta_path.remove(finder)
    sys.modules.pop("msilib.schema", None)
    sys.modules.pop("msilib", None)
    sys.modules.update(saved)
