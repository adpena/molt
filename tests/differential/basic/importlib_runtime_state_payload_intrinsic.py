"""Purpose: validate importlib runtime-state intrinsic backed module cache lookup."""

import importlib
import types
import sys

name = "molt_runtime_state_payload_target"
module = types.ModuleType(name)
module.marker = "runtime-state"

sys.modules[name] = module
try:
    loaded = importlib.import_module(name)
    print(loaded is module)
    print(getattr(loaded, "marker", None))
finally:
    sys.modules.pop(name, None)
