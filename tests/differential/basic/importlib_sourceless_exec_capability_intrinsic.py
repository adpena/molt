"""Purpose: validate sourceless execution capability-gated intrinsic behavior."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_sourceless_exec_cap_")
pyc_path = os.path.join(root, "bccap.pyc")
with open(pyc_path, "wb") as handle:
    handle.write(b"")

source_path = os.path.join(root, "bccap.py")
with open(source_path, "w", encoding="utf-8") as handle:
    handle.write("value = 7\n")

loader = importlib.machinery.SourcelessFileLoader("bccap_exec", pyc_path)
spec = importlib.util.spec_from_file_location("bccap_exec", pyc_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 7
except BaseException as exc:
    error_name = exc.__class__.__name__

print(
    loaded
    or error_name
    in {"ImportError", "PermissionError", "OSError", "EOFError", "RuntimeError"}
)
print((not loaded) or getattr(module, "value", None) == 7)
