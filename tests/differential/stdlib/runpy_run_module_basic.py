"""Purpose: runpy.run_module executes compiler-admitted modules and package mains."""

import runpy
import sys


def _admit_compiled_runpy_modules() -> None:
    import runpy_compiled_fixture
    import runpy_compiled_package.__main__
    import runpy_compiled_package_no_main
    import runpy_compiled_namespace.__main__


ns = runpy.run_module("runpy_compiled_fixture")
print(ns["value"])
print(ns["name_seen"])
print(ns["package_seen"])
print(getattr(ns["spec_seen"], "name", None))
print(ns["loader_seen"] is not None)
print(ns["loader_seen"] is getattr(ns["spec_seen"], "loader", None))
print(ns["cached_seen"] == getattr(ns["spec_seen"], "cached", None))

alias = runpy.run_module(
    "runpy_compiled_fixture",
    run_name="alias.runner",
    init_globals={"seed": 9},
)
print(alias["seed_seen"])
print(alias["name_seen"])
print(alias["package_seen"])
print(getattr(alias["spec_seen"], "name", None))

package = runpy.run_module("runpy_compiled_package")
print(package["entry"])
print(package["name_seen"])
print(package["package_seen"])
print(getattr(package["spec_seen"], "name", None))

namespace = runpy.run_module("runpy_compiled_namespace")
print(namespace["entry"])
print(namespace["name_seen"])
print(namespace["package_seen"])
print(getattr(namespace["spec_seen"], "name", None))
print("runpy_compiled_fixture" in sys.modules)

try:
    runpy.run_module("runpy_compiled_package_no_main")
except ImportError as exc:
    print("cannot be directly executed" in str(exc))
