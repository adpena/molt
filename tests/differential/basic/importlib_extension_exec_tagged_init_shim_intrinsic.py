"""Purpose: validate extension tagged __init__ shim package execution intrinsic lane."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_pkg_tagged_")
pkg_dir = os.path.join(root, "pkgexttagged")
os.makedirs(pkg_dir, exist_ok=True)
ext_path = os.path.join(pkg_dir, "__init__.cpython-312-darwin.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")
with open(os.path.join(pkg_dir, "__init__.molt.py"), "w", encoding="utf-8") as handle:
    handle.write("value = 177\n")

loader = importlib.machinery.ExtensionFileLoader("pkgexttagged", ext_path)
spec = importlib.util.spec_from_file_location("pkgexttagged", ext_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
is_pkg = False
package_name = ""
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 177
        is_pkg = isinstance(getattr(module, "__path__", None), list)
        package_name = str(getattr(module, "__package__", ""))
except BaseException as exc:
    error_name = exc.__class__.__name__

print(loaded or error_name in {"ImportError", "PermissionError", "OSError"})
print((not loaded) or is_pkg)
print((not loaded) or package_name == "pkgexttagged")
