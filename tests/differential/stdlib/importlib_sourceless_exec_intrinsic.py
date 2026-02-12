"""Purpose: validate intrinsic-backed sourceless loader execution path."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_sourceless_exec_")
pyc_path = os.path.join(root, "bcmod.pyc")
with open(pyc_path, "wb") as handle:
    handle.write(b"")

loader = importlib.machinery.SourcelessFileLoader("bcmod_exec", pyc_path)
spec = importlib.util.spec_from_file_location("bcmod_exec", pyc_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

exc_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
    else:
        exc_name = "missing"
except BaseException as exc:
    exc_name = exc.__class__.__name__

print(exc_name in {"ImportError", "PermissionError"})
