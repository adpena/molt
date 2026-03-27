"""Purpose: ensure helper-built top-level and dotted imports stay in the import graph."""

import importlib


def _module_name(parts: tuple[str, ...]) -> str:
    return ".".join(parts)


def _load(parts: tuple[str, ...]):
    return importlib.import_module(_module_name(parts))


math_mod = _load(("math",))
util_mod = _load(("importlib", "util"))

print(math_mod.__name__)
print(util_mod.__name__)
print(hasattr(util_mod, "find_spec"))
print(hasattr(util_mod, "module_from_spec"))
