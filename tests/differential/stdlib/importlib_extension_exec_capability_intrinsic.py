"""Purpose: validate extension execution capability-gated intrinsic behavior."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_exec_cap_")
ext_path = os.path.join(root, "extcap.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")

shim_path = f"{ext_path}.molt.py"
with open(shim_path, "w", encoding="utf-8") as handle:
    handle.write("value = 41\n")

loader = importlib.machinery.ExtensionFileLoader("extcap_exec", ext_path)
spec = importlib.util.spec_from_file_location("extcap_exec", ext_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 41
except BaseException as exc:
    error_name = exc.__class__.__name__

print(loaded or error_name in {"ImportError", "PermissionError", "OSError"})
print((not loaded) or getattr(module, "value", None) == 41)
