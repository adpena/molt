"""Purpose: validate importlib basics."""

# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read

import importlib
import sys

mod = importlib.import_module("math")
print(mod.__name__)

fresh = importlib.reload(mod)
print(fresh is sys.modules["math"])

try:
    importlib.import_module("_molt_missing_module_hopefully")
    print("missing-noerror")
except ModuleNotFoundError as exc:
    print("missing", str(exc)[:40])
