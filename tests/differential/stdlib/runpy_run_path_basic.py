"""Purpose: runpy.run_path executes a compiler-admitted source path transactionally."""

import os
import runpy
import sys


def _admit_compiled_runpy_module() -> None:
    import runpy_compiled_fixture
    import runpy_compiled_package.__main__


path = os.path.join(os.path.dirname(__file__), "runpy_compiled_fixture.py")
prior_argv0 = sys.argv[0]
ns = runpy.run_path(path)
print(ns["value"])
print(ns["name_seen"])
print(ns["file_seen"].endswith("runpy_compiled_fixture.py"))
print(ns["package_seen"])
print(ns["spec_seen"] is None)
print(ns["cached_seen"] is None)
print(ns["loader_seen"] is None)
print(ns["sys_modules_seen"])
print(str(ns["argv0_seen"]).endswith("runpy_compiled_fixture.py"))
print(sys.argv[0] == prior_argv0)
print("<run_path>" not in sys.modules)

custom = runpy.run_path(path, init_globals={"seed": 99}, run_name="pkg.tool")
print(custom["seed_seen"])
print(custom["name_seen"])
print(custom["package_seen"])
print(custom["spec_seen"] is None)
print(custom["sys_modules_seen"])
print(sys.argv[0] == prior_argv0)

package_path = os.path.join(os.path.dirname(__file__), "runpy_compiled_package")
container = runpy.run_path(package_path)
print(container["entry"])
print(container["name_seen"])
print(container["package_seen"])
print(getattr(container["spec_seen"], "name", None))
print(container["sys_modules_seen"])
print(container["argv0_seen"] == package_path)
print(container["path0_seen"] == package_path)
print(sys.argv[0] == prior_argv0)
