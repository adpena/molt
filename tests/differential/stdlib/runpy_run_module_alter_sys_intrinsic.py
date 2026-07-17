"""Purpose: run_module alter_sys is transactional on success and exception."""

import runpy
import sys
import types


def _admit_compiled_runpy_module() -> None:
    import runpy_compiled_fixture


prior_argv0 = sys.argv[0]
sentinel = types.SimpleNamespace(name="sentinel")
sys.modules["alias.runner"] = sentinel
try:
    ns = runpy.run_module(
        "runpy_compiled_fixture",
        run_name="alias.runner",
        alter_sys=True,
    )
    print(ns["value"])
    print(ns["name_seen"])
    print(ns["sys_modules_seen"])
    print(str(ns["argv0_seen"]).endswith("runpy_compiled_fixture.py"))
    print(sys.modules.get("alias.runner") is sentinel)
    print(sys.argv[0] == prior_argv0)
    print("runpy_compiled_fixture" not in sys.modules)

    try:
        runpy.run_module(
            "runpy_compiled_fixture",
            init_globals={"raise_from_runpy": True},
            run_name="alias.runner",
            alter_sys=True,
        )
    except RuntimeError as exc:
        print(str(exc))
    print(sys.modules.get("alias.runner") is sentinel)
    print(sys.argv[0] == prior_argv0)
finally:
    sys.modules.pop("alias.runner", None)
    sys.modules.pop("runpy_compiled_fixture", None)
