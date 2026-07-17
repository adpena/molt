"""Purpose: SourceFileLoader executes the compiler-admitted module body."""

import importlib.util
import os
import sys


def _admit_compiled_loader_module() -> None:
    import runpy_compiled_fixture  # noqa: F401 - static admission root


module_name = "runpy_compiled_fixture"
module_path = os.path.join(os.path.dirname(__file__), f"{module_name}.py")
spec = importlib.util.spec_from_file_location(module_name, module_path)
assert spec is not None and spec.loader is not None
missing = object()
sentinel = object()
previous = sys.modules.get(module_name, missing)


def _execute(mapping: str):
    module = importlib.util.module_from_spec(spec)
    module.capture_globals = True
    if mapping == "absent":
        sys.modules.pop(module_name, None)
    elif mapping == "target":
        sys.modules[module_name] = module
    else:
        sys.modules[module_name] = sentinel
    spec.loader.exec_module(module)
    if mapping == "absent":
        assert module.sys_modules_value_seen is None
        assert module_name not in sys.modules
    elif mapping == "target":
        assert module.sys_modules_value_seen is module
        assert sys.modules[module_name] is module
    else:
        assert module.sys_modules_value_seen is sentinel
        assert sys.modules[module_name] is sentinel
    return module


try:
    module = _execute("absent")
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
    print(_execute("target").sys_modules_seen)
    print(_execute("sentinel").sys_modules_seen)

    failed = importlib.util.module_from_spec(spec)
    failed.capture_globals = True
    failed.raise_from_runpy = True
    sys.modules.pop(module_name, None)
    try:
        spec.loader.exec_module(failed)
    except RuntimeError as exc:
        print(str(exc))
    assert module_name not in sys.modules
    print(failed.value)
    print(failed.globals_seen is failed.__dict__)
finally:
    if previous is not missing:
        sys.modules[module_name] = previous
    else:
        sys.modules.pop(module_name, None)
