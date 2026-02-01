"""Purpose: importing the entry module yields a distinct module object."""

import importlib
import os
import sys


def _module_name_from_path(path: str) -> str | None:
    try:
        abs_path = os.path.abspath(path)
    except Exception:
        return None
    roots = list(sys.path)
    if "" in roots:
        try:
            roots[roots.index("")] = os.getcwd()
        except Exception:
            pass
    for root in roots:
        if not root:
            continue
        try:
            root_abs = os.path.abspath(root)
        except Exception:
            continue
        if abs_path == root_abs:
            continue
        if not abs_path.startswith(root_abs.rstrip(os.sep) + os.sep):
            continue
        try:
            rel = os.path.relpath(abs_path, root_abs)
        except Exception:
            continue
        if rel.startswith(".."):
            continue
        if rel.endswith(".py"):
            rel = rel[:-3]
        parts = [part for part in rel.split(os.sep) if part]
        if not parts:
            continue
        if parts[-1] == "__init__":
            parts = parts[:-1]
        if not parts:
            continue
        return ".".join(parts)
    return None


name = _module_name_from_path(__file__)
print(name)
if name is None:
    print("module-name-none")
else:
    mod = importlib.import_module(name)
    print("same" if mod is sys.modules.get("__main__") else "different")
    print(mod.__name__)
    print(sys.modules.get("__main__").__name__)
