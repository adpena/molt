"""Purpose: ensure helper-based dynamic module-name imports stay in import graph closure."""

import importlib


def _module_name(parts: tuple[str, str]) -> str:
    return "".join(parts)


def _load(parts: tuple[str, str]):
    return importlib.import_module(_module_name(parts))


math_mod = _load(("ma", "th"))
sys_mod = _load(("sy", "s"))

print(math_mod.__name__)
print(sys_mod.__name__)
print(math_mod is not None)
print(sys_mod is not None)
