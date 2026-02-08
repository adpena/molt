"""Purpose: validate intrinsic-backed extension loader execution path."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_exec_")
ext_path = os.path.join(root, "extdemo.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")

loader = importlib.machinery.ExtensionFileLoader("extdemo_exec", ext_path)
spec = importlib.util.spec_from_file_location("extdemo_exec", ext_path, loader=loader)
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
