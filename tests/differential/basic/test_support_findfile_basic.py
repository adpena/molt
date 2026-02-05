"""Purpose: differential coverage for importlib spec discovery."""

# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
import importlib.util
import os

abs_path = os.path.abspath(__file__)
print("abs_exists", os.path.exists(abs_path))

print("spec_os", importlib.util.find_spec("os") is not None)
print("spec_missing", importlib.util.find_spec("_molt_missing_module_hopefully") is None)
