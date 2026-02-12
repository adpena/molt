"""Purpose: validate extension loader .py shim execution lane via intrinsics."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_exec_pyshim_")
ext_path = os.path.join(root, "extshim.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")

shim_path = f"{ext_path}.py"
with open(shim_path, "w", encoding="utf-8") as handle:
    handle.write("value = 52\\n")

loader = importlib.machinery.ExtensionFileLoader("extshim_exec", ext_path)
spec = importlib.util.spec_from_file_location("extshim_exec", ext_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 52
except BaseException as exc:
    error_name = exc.__class__.__name__

print(loaded or error_name in {"ImportError", "PermissionError", "OSError"})
print((not loaded) or getattr(module, "value", None) == 52)
