"""Purpose: ensure restricted extension shim errors surface as ImportError lane."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_restricted_shim_")
ext_path = os.path.join(root, "extrestricted.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")

shim_path = f"{ext_path}.molt.py"
with open(shim_path, "w", encoding="utf-8") as handle:
    handle.write("value = 1\nif True:\n    value = 2\n")

loader = importlib.machinery.ExtensionFileLoader("extrestricted", ext_path)
spec = importlib.util.spec_from_file_location("extrestricted", ext_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
except BaseException as exc:
    error_name = exc.__class__.__name__

print(error_name in {"ImportError", "PermissionError", "OSError"})
print(error_name != "NotImplementedError")
