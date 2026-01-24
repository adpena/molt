# MOLT_ENV: PYTHONPATH=src:tests/differential/planned
"""Purpose: differential coverage for importlib module from spec."""

import importlib.util
import os


path = os.path.join(os.path.dirname(__file__), "res_pkg", "__init__.py")
spec = importlib.util.spec_from_file_location("res_pkg_clone", path)
print(spec is not None)
if spec:
    module = importlib.util.module_from_spec(spec)
    print(module.__spec__ is spec)
