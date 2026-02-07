"""Purpose: validate intrinsic-backed importlib.util.find_spec filesystem discovery."""

import importlib.util
import os
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    pkg_dir = os.path.join(tmp, "pkgdemo")
    os.mkdir(pkg_dir)
    pkg_init = os.path.join(pkg_dir, "__init__.py")
    with open(pkg_init, "w", encoding="utf-8") as handle:
        handle.write("value = 7\n")

    mod_path = os.path.join(tmp, "moddemo.py")
    with open(mod_path, "w", encoding="utf-8") as handle:
        handle.write("value = 9\n")

    sys.path.insert(0, tmp)
    try:
        pkg_spec = importlib.util.find_spec("pkgdemo")
        mod_spec = importlib.util.find_spec("moddemo")
        builtin_spec = importlib.util.find_spec("math")
        miss_spec = importlib.util.find_spec("_molt_missing_mod_for_find_spec")
    finally:
        sys.path.pop(0)
        sys.modules.pop("pkgdemo", None)
        sys.modules.pop("moddemo", None)

    print(pkg_spec is not None)
    print(pkg_spec.origin.endswith("__init__.py") if pkg_spec else False)
    print(
        tuple(
            os.path.basename(entry)
            for entry in (pkg_spec.submodule_search_locations or ())
        )
        if pkg_spec
        else ()
    )
    print(pkg_spec.cached.endswith(".pyc") if pkg_spec else False)

    print(mod_spec is not None)
    print(mod_spec.origin.endswith("moddemo.py") if mod_spec else False)
    print(mod_spec.submodule_search_locations if mod_spec else None)
    print(mod_spec.cached.endswith(".pyc") if mod_spec else False)

    print(builtin_spec is not None)
    print(builtin_spec.origin == "built-in" if builtin_spec else False)
    print(builtin_spec.has_location is False if builtin_spec else False)
    print(builtin_spec.cached is None if builtin_spec else False)

    print(miss_spec is None)
