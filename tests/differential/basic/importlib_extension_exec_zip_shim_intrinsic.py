"""Purpose: validate extension loader zip shim execution parity via intrinsic runtime."""

import importlib.machinery
import importlib.util
import os
import tempfile
import zipfile


root = tempfile.mkdtemp(prefix="molt_ext_exec_zip_shim_")
archive = os.path.join(root, "mods.zip")
with zipfile.ZipFile(archive, "w") as zf:
    zf.writestr("zpkg/extdemo.so", b"")
    zf.writestr("zpkg/extdemo.so.molt.py", "value = 313\n")

ext_path = f"{archive}/zpkg/extdemo.so"
loader = importlib.machinery.ExtensionFileLoader("zpkg_extdemo", ext_path)
spec = importlib.util.spec_from_file_location("zpkg_extdemo", ext_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 313
except BaseException as exc:
    error_name = exc.__class__.__name__

print(
    loaded
    or error_name
    in {"ImportError", "PermissionError", "OSError", "FileNotFoundError", "RuntimeError"}
)
print((not loaded) or getattr(module, "value", None) == 313)
