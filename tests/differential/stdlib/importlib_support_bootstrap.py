"""Purpose: validate importlib support-module bootstrap on the canonical runtime path."""

import importlib


util_mod = importlib.import_module("importlib.util")
machinery_mod = importlib.import_module("importlib.machinery")

print(util_mod.__name__)
print(machinery_mod.__name__)
print(hasattr(util_mod, "find_spec"))
print(hasattr(util_mod, "module_from_spec"))
print(hasattr(machinery_mod, "ModuleSpec"))
