"""Purpose: Loader.exec_module sets no module metadata, as in CPython 3.12.

CPython's importlib._bootstrap.module_from_spec is the only site that runs
_init_module_attrs; a loader's exec_module only executes the body. A module
handed straight to exec_module keeps the metadata it had: absent attributes
stay absent, and a module with __path__ but no __spec__ gets no spec.
"""

import importlib.util
import os
import types

import importlib_loader_exec_metadata_pkg as package

NAMES = ("__spec__", "__loader__", "__file__", "__cached__", "__package__", "__path__")
PLAIN = "importlib_loader_exec_metadata_pkg.plain"


def metadata(module):
    namespace = module.__dict__
    return [
        (name, type(namespace[name]).__name__ if name in namespace else "absent")
        for name in NAMES
    ]


package_dir = os.path.dirname(package.__file__)
# A literal lookup also admits the child's compiled body into a Molt binary.
child_spec = importlib.util.find_spec("importlib_loader_exec_metadata_pkg.plain")
print("child found", child_spec is not None)

# A bare module: ModuleType binds __spec__, __loader__ and __package__ to None.
plain_spec = importlib.util.spec_from_file_location(
    PLAIN, os.path.join(package_dir, "plain.py")
)
bare = types.ModuleType(PLAIN)
plain_spec.loader.exec_module(bare)
print("file loader, bare module", metadata(bare), bare.value)

# The import system's loader for the package, with __path__ but no __spec__.
package_spec = importlib.util.find_spec("importlib_loader_exec_metadata_pkg")
locations = [package_dir]
loose = types.ModuleType("importlib_loader_exec_metadata_pkg")
loose.__path__ = locations
del loose.__spec__
package_spec.loader.exec_module(loose)
print("finder loader, __path__ without __spec__", metadata(loose), loose.value)
print("path kept", loose.__path__ is locations)

# module_from_spec is the metadata authority; exec_module keeps its values.
initialized = importlib.util.module_from_spec(package_spec)
before = metadata(initialized)
package_spec.loader.exec_module(initialized)
print("module_from_spec then exec_module", before == metadata(initialized))
print("spec kept", initialized.__spec__ is package_spec)
