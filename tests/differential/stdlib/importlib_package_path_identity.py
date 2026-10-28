"""Purpose: a package's __path__ is its spec's submodule_search_locations.

CPython 3.12 importlib._bootstrap._init_module_attrs binds module.__path__ to
the spec's list object, and Loader.exec_module sets no metadata. A compiled
package body must not replace that object. A namespace package's module and
spec share one locations object too. A module body's own __path__ binding is
ordinary code and stays in effect.
"""

import importlib
import importlib.util
import os
import sys

print(
    "importlib",
    importlib.__path__ is importlib.__spec__.submodule_search_locations,
)

import importlib_path_identity_pkg as package

print(
    "import",
    package.__path__ is package.__spec__.submodule_search_locations,
    package.body_path_is_spec_locations,
)

import importlib_path_identity_ns.child as namespace_child

namespace = sys.modules["importlib_path_identity_ns"]
print(
    "namespace",
    namespace.__path__ is namespace.__spec__.submodule_search_locations,
    [os.path.basename(path) for path in namespace.__path__],
    namespace_child.value,
)

loaded_spec = importlib.util.find_spec("importlib_path_identity_pkg.loaded")
sentinel = ["sentinel-location"]
loaded_spec.submodule_search_locations = sentinel
loaded = importlib.util.module_from_spec(loaded_spec)
print("module-from-spec", loaded.__path__ is sentinel)
loaded_spec.loader.exec_module(loaded)
print(
    "exec-module",
    loaded.__path__ is sentinel,
    loaded.__spec__ is loaded_spec,
    loaded.body_path_is_spec_locations,
)

plain_spec = importlib.util.find_spec("importlib_path_identity_pkg.plain")
plain_spec.submodule_search_locations = ["sentinel-location"]
plain = importlib.util.module_from_spec(plain_spec)
plain_spec.loader.exec_module(plain)
print("module-body-path", plain.__path__)

reloaded = importlib.reload(package)
print(
    "reload",
    reloaded is package,
    package.__path__ is package.__spec__.submodule_search_locations,
    package.body_path_is_spec_locations,
)
