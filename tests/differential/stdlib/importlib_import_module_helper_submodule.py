"""Purpose: ensure helper-based submodule imports are resolved and retained."""

import importlib


BASE = "importlib"


def _submodule(name: str):
    return importlib.import_module(f"{BASE}.{name}")


machinery = _submodule("machinery")
util = _submodule("util")

print(machinery.__name__)
print(util.__name__)
print(hasattr(util, "find_spec"))
print(hasattr(machinery, "ModuleSpec"))
