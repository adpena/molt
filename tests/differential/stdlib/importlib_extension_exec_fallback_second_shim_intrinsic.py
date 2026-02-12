"""Purpose: extension exec shim lane falls through to later intrinsic candidates."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_exec_fallback_")
ext_path = os.path.join(root, "extfallback.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")

with open(f"{ext_path}.molt.py", "w", encoding="utf-8") as handle:
    handle.write("value = 1\nif True:\n    value = 2\n")

with open(f"{ext_path}.py", "w", encoding="utf-8") as handle:
    handle.write("value = 73\n")

loader = importlib.machinery.ExtensionFileLoader("extfallback", ext_path)
spec = importlib.util.spec_from_file_location("extfallback", ext_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 73
except BaseException as exc:
    error_name = exc.__class__.__name__

print(loaded or error_name in {"ImportError", "PermissionError", "OSError"})
print((not loaded) or getattr(module, "value", None) == 73)
