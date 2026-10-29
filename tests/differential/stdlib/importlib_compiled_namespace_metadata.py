"""A namespace and its child are admitted to the binary at compile time."""

import importlib.util
import sys

import importlib_path_identity_ns.child as child

namespace = sys.modules["importlib_path_identity_ns"]
print(type(namespace.__path__).__name__)
print(namespace.__spec__.origin)
print(type(namespace.__spec__.loader).__name__)
print(namespace.__path__ is namespace.__spec__.submodule_search_locations)
print("child", child.value)

# Finding a namespace again describes it without executing a synthetic body.
del sys.modules["importlib_path_identity_ns"]
spec = importlib.util.find_spec("importlib_path_identity_ns")
print("finder", spec.loader, spec.origin, type(spec.submodule_search_locations).__name__)
recreated = importlib.util.module_from_spec(spec)
print("recreated", type(spec.loader).__name__, recreated.__path__ is spec.submodule_search_locations)
