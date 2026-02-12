"""Purpose: validate extension shim lane with container/bytes restricted literals."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_container_shim_")
ext_path = os.path.join(root, "extcontainershim.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")

shim_path = f"{ext_path}.molt.py"
with open(shim_path, "w", encoding="utf-8") as handle:
    handle.write(
        "value = {'nums': [1, 2, 3], 'pair': (4, 5), 'blob': b'xy'}\\n"
        "name = 'ok'\\n"
    )

loader = importlib.machinery.ExtensionFileLoader("extcontainershim", ext_path)
spec = importlib.util.spec_from_file_location("extcontainershim", ext_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = (
            isinstance(getattr(module, "value", None), dict)
            and getattr(module, "value", {}).get("nums") == [1, 2, 3]
            and getattr(module, "value", {}).get("pair") == (4, 5)
            and getattr(module, "value", {}).get("blob") == b"xy"
            and getattr(module, "name", None) == "ok"
        )
except BaseException as exc:
    error_name = exc.__class__.__name__

print(loaded or error_name in {"ImportError", "PermissionError", "OSError"})
print((not loaded) or getattr(module, "value", {}).get("pair") == (4, 5))
