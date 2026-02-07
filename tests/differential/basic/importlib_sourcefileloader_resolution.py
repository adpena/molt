"""Purpose: validate importlib source-loader package/module resolution metadata."""

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

    pkg_spec = importlib.util.spec_from_file_location("pkgdemo", pkg_init)
    assert pkg_spec is not None and pkg_spec.loader is not None
    pkg_mod = importlib.util.module_from_spec(pkg_spec)
    sys.modules.pop("pkgdemo", None)
    pkg_spec.loader.exec_module(pkg_mod)
    print(pkg_mod.__package__)
    pkg_paths = tuple(pkg_mod.__path__)
    spec_paths = tuple(pkg_spec.submodule_search_locations or ())
    print(tuple(os.path.basename(entry) for entry in pkg_paths))
    print(tuple(os.path.basename(entry) for entry in spec_paths))
    print(pkg_paths == spec_paths)
    print(pkg_mod.value)

    mod_path = os.path.join(tmp, "moddemo.py")
    with open(mod_path, "w", encoding="utf-8") as handle:
        handle.write("value = 9\n")

    mod_spec = importlib.util.spec_from_file_location("moddemo", mod_path)
    assert mod_spec is not None and mod_spec.loader is not None
    mod_mod = importlib.util.module_from_spec(mod_spec)
    sys.modules.pop("moddemo", None)
    mod_spec.loader.exec_module(mod_mod)
    print(mod_mod.__package__)
    print(getattr(mod_mod, "__path__", None))
    print(mod_mod.value)
