from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import sys
import types


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_callbacks = []


def _atexit_register(func, args, kwargs):
    _callbacks.append((func, tuple(args), dict(kwargs)))


def _atexit_unregister(func):
    _callbacks[:] = [entry for entry in _callbacks if entry[0] is not func]


def _atexit_run():
    for func, args, kwargs in list(reversed(_callbacks)):
        func(*args, **kwargs)


builtins._molt_intrinsics = {{
    "molt_atexit_register": _atexit_register,
    "molt_atexit_unregister": _atexit_unregister,
    "molt_atexit_clear": lambda: _callbacks.clear(),
    "molt_atexit_run_exitfuncs": _atexit_run,
    "molt_atexit_ncallbacks": lambda: len(_callbacks),
    "molt_difflib_ratio": lambda a, b: 0.5,
    "molt_difflib_quick_ratio": lambda a, b: 0.75,
    "molt_difflib_get_matching_blocks": lambda a, b: [(0, 0, 1), (1, 1, 0)],
    "molt_difflib_get_opcodes": lambda a, b: [("equal", 0, 1, 0, 1)],
    "molt_difflib_is_junk": lambda value: value == " ",
    "molt_difflib_ndiff": lambda a, b: ["- a\\n", "+ b\\n"],
    "molt_difflib_unified_diff": lambda a, b, fromfile, tofile, n: ["--- a", "+++ b"],
    "molt_difflib_context_diff": lambda a, b, fromfile, tofile, n: ["*** a", "--- b"],
    "molt_difflib_get_close_matches": lambda word, possibilities, n, cutoff: ["alpha"],
    "molt_pkgutil_iter_modules": lambda source, prefix: [("finder", prefix + "mod", False)],
    "molt_pkgutil_walk_packages": lambda source, prefix: [("finder", prefix + "pkg", True)],
    "molt_trace_runtime_ready": lambda: None,
    "molt_html_entities_codepoint2name": lambda: {{34: "quot"}},
    "molt_html_entities_name2codepoint": lambda: {{"quot": 34}},
    "molt_html_entities_html5": lambda: {{"quot;": '"'}},
}}

_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    intrinsics = getattr(builtins, "_molt_intrinsics", {{}})
    if name in intrinsics:
        value = intrinsics[name]
        if namespace is not None:
            namespace[name] = value
        return value
    raise RuntimeError(f"intrinsic unavailable: {{name}}")


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod


atexit_mod = _load_module("molt_test_atexit", {str(STDLIB_ROOT / "atexit.py")!r})
difflib_mod = _load_module("molt_test_difflib", {str(STDLIB_ROOT / "difflib.py")!r})
pkgutil_mod = _load_module("molt_test_pkgutil", {str(STDLIB_ROOT / "pkgutil.py")!r})
trace_mod = _load_module("molt_test_trace", {str(STDLIB_ROOT / "trace.py")!r})
html_entities_mod = _load_module(
    "molt_test_html_entities", {str(STDLIB_ROOT / "html" / "entities.py")!r}
)

events = []


def _record(value):
    events.append(value)


atexit_mod.register(_record, "alpha")
atexit_mod.register(_record, "beta")
atexit_mod.unregister(_record)
atexit_mod.register(_record, "gamma")
atexit_mod._run_exitfuncs()

matcher = difflib_mod.SequenceMatcher(None, "a", "b")
trace_runner = trace_mod.Trace(count=False, trace=False)

checks = {{
    "atexit": (
        atexit_mod._ncallbacks() == 1
        and events == ["gamma"]
        and "molt_atexit_register" not in atexit_mod.__dict__
    ),
    "difflib": (
        matcher.ratio() == 0.5
        and matcher.get_matching_blocks() == [(0, 0, 1), (1, 1, 0)]
        and difflib_mod.get_close_matches("alp", ["alpha"]) == ["alpha"]
        and "molt_difflib_ratio" not in difflib_mod.__dict__
    ),
    "pkgutil": (
        list(pkgutil_mod.iter_modules(prefix="x."))[0].name == "x.mod"
        and list(pkgutil_mod.walk_packages(prefix="y."))[0].ispkg is True
        and "molt_pkgutil_iter_modules" not in pkgutil_mod.__dict__
    ),
    "trace": (
        trace_runner.runfunc(lambda x: x + 1, 2) == 3
        and "molt_trace_runtime_ready" not in trace_mod.__dict__
    ),
    "html_entities": (
        html_entities_mod.codepoint2name[34] == "quot"
        and html_entities_mod.name2codepoint["quot"] == 34
        and html_entities_mod.html5["quot;"] == '"'
        and "molt_html_entities_html5" not in html_entities_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_g() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "CHECK":
            checks[rest[0]] = rest[1]
    assert checks == {
        "atexit": "True",
        "difflib": "True",
        "html_entities": "True",
        "pkgutil": "True",
        "trace": "True",
    }
