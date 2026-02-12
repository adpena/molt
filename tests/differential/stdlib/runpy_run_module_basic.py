# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: runpy.run_module supports module and package __main__ execution parity."""

import os
import runpy
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    mod_path = os.path.join(tmp, "modrun.py")
    with open(mod_path, "w", encoding="utf-8") as handle:
        handle.write("value = 11\n")

    pkg_root = os.path.join(tmp, "pkgdemo")
    os.mkdir(pkg_root)
    with open(os.path.join(pkg_root, "__init__.py"), "w", encoding="utf-8") as handle:
        handle.write("marker = 3\n")
    with open(os.path.join(pkg_root, "__main__.py"), "w", encoding="utf-8") as handle:
        handle.write("entry = 42\n")

    original = list(sys.path)
    try:
        sys.path.insert(0, tmp)
        ns = runpy.run_module("modrun")
        print(ns.get("value"))
        print(ns.get("__name__"))
        print(ns.get("__package__"))
        print(getattr(ns.get("__spec__"), "name", None))

        ns2 = runpy.run_module(
            "modrun", run_name="alias.runner", init_globals={"seed": 9}
        )
        print(ns2.get("seed"))
        print(ns2.get("__name__"))
        print(ns2.get("__package__"))

        pkg_ns = runpy.run_module("pkgdemo")
        print(pkg_ns.get("entry"))
        print(pkg_ns.get("__name__"))
        print(pkg_ns.get("__package__"))
        print(getattr(pkg_ns.get("__spec__"), "name", None))
    finally:
        sys.path[:] = original
