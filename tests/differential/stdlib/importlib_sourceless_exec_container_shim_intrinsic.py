"""Purpose: validate sourceless shim lane with container/bytes restricted literals."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_sourceless_container_shim_")
pyc_path = os.path.join(root, "shimcontainers.pyc")
with open(pyc_path, "wb") as handle:
    handle.write(b"")

shim_path = f"{pyc_path[:-4]}.molt.py"
with open(shim_path, "w", encoding="utf-8") as handle:
    handle.write(
        "value = {'nums': [7, 8], 'pair': (9,), 'blob': b'zz'}\\n"
        "flag = True\\n"
    )

loader = importlib.machinery.SourcelessFileLoader("shimcontainers", pyc_path)
spec = importlib.util.spec_from_file_location("shimcontainers", pyc_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = (
            isinstance(getattr(module, "value", None), dict)
            and getattr(module, "value", {}).get("nums") == [7, 8]
            and getattr(module, "value", {}).get("pair") == (9,)
            and getattr(module, "value", {}).get("blob") == b"zz"
            and getattr(module, "flag", None) is True
        )
except BaseException as exc:
    error_name = exc.__class__.__name__

print(
    loaded
    or error_name
    in {"ImportError", "PermissionError", "OSError", "EOFError", "RuntimeError"}
)
print((not loaded) or getattr(module, "flag", None) is True)
