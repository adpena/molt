# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
"""Purpose: differential coverage for importlib spec from file."""

import importlib.util
import os


path = os.path.join(os.path.dirname(__file__), "res_pkg", "__init__.py")
spec = importlib.util.spec_from_file_location("res_pkg_copy", path)
print(spec is not None)
if spec and spec.loader:
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    print(getattr(module, "VALUE", None))
