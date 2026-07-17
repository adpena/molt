"""Purpose: SourceFileLoader executes the compiler-admitted module body."""

import importlib.util
import os
import sys


def _admit_compiled_loader_module() -> None:
    import runpy_compiled_fixture


module_name = "runpy_compiled_fixture"
module_path = os.path.join(os.path.dirname(__file__), f"{module_name}.py")
spec = importlib.util.spec_from_file_location(module_name, module_path)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
module.capture_globals = True
previous = sys.modules.pop(module_name, None)
try:
    spec.loader.exec_module(module)
    print(module.__doc__)
    print(module.value)
    print(module.file_seen.endswith("runpy_compiled_fixture.py"))
    print(getattr(module.spec_seen, "name", None))
    print(module.spec_seen is module.__spec__)
    print(module.loader_seen is module.__loader__)
    print(module.loader_seen is spec.loader)
    print(module.package_seen == module.__package__)
    print(module.cached_seen == module.__cached__)
    print(not module.sys_modules_seen)
    print(module.globals_seen is module.__dict__)

    failed = importlib.util.module_from_spec(spec)
    failed.capture_globals = True
    failed.raise_from_runpy = True
    try:
        spec.loader.exec_module(failed)
    except RuntimeError as exc:
        print(str(exc))
    print(failed.value)
    print(failed.globals_seen is failed.__dict__)
finally:
    if previous is not None:
        sys.modules[module_name] = previous
    else:
        sys.modules.pop(module_name, None)
