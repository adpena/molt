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


builtins._molt_intrinsics = {{
    "molt_colorsys_rgb_to_hls": lambda r, g, b: (0.1, 0.2, 0.3),
    "molt_colorsys_hls_to_rgb": lambda h, l, s: (0.4, 0.5, 0.6),
    "molt_colorsys_rgb_to_hsv": lambda r, g, b: (0.7, 0.8, 0.9),
    "molt_colorsys_hsv_to_rgb": lambda h, s, v: (1.0, 0.0, 0.0),
    "molt_colorsys_rgb_to_yiq": lambda r, g, b: (0.1, 0.0, -0.1),
    "molt_colorsys_yiq_to_rgb": lambda y, i, q: (0.2, 0.3, 0.4),
    "molt_keyword_lists": lambda: (["False", "None"], ["match"]),
    "molt_keyword_iskeyword": lambda value: value in {{"False", "None"}},
    "molt_keyword_issoftkeyword": lambda value: value == "match",
    "molt_graphlib_new": lambda: {{"ready": [], "active": False}},
    "molt_graphlib_add": lambda handle, node, preds: handle["ready"].append(node),
    "molt_graphlib_prepare": lambda handle: None,
    "molt_graphlib_get_ready": lambda handle: tuple(handle["ready"]),
    "molt_graphlib_is_active": lambda handle: False,
    "molt_graphlib_done": lambda handle, nodes: None,
    "molt_graphlib_static_order": lambda handle: (True, tuple(handle["ready"])),
    "molt_graphlib_drop": lambda handle: None,
    "molt_fnmatch_fnmatch": lambda name, pat: str(name).endswith(str(pat).lstrip("*")),
    "molt_fnmatch_fnmatchcase": lambda name, pat: str(name) == str(pat),
    "molt_fnmatch_filter": lambda names, pat, _case: [name for name in names if str(name).endswith(str(pat).lstrip("*"))],
    "molt_fnmatch_translate": lambda pat: f"re:{{pat}}",
    "molt_gettext_gettext": lambda message: f"T:{{message}}",
    "molt_gettext_ngettext": lambda singular, plural, n: singular if int(n) == 1 else plural,
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


colorsys_mod = _load_module("molt_test_colorsys", {str(STDLIB_ROOT / "colorsys.py")!r})
keyword_mod = _load_module("molt_test_keyword", {str(STDLIB_ROOT / "keyword.py")!r})
graphlib_mod = _load_module("molt_test_graphlib", {str(STDLIB_ROOT / "graphlib.py")!r})
fnmatch_mod = _load_module("molt_test_fnmatch", {str(STDLIB_ROOT / "fnmatch.py")!r})
gettext_mod = _load_module("molt_test_gettext", {str(STDLIB_ROOT / "gettext.py")!r})

ts = graphlib_mod.TopologicalSorter()
ts.add("a")
ts.add("b", "a")
ts.prepare()

checks = {{
    "colorsys": (
        colorsys_mod.rgb_to_hls(1, 2, 3) == (0.1, 0.2, 0.3)
        and colorsys_mod.hsv_to_rgb(0, 0, 0) == (1.0, 0.0, 0.0)
        and "molt_colorsys_rgb_to_hls" not in colorsys_mod.__dict__
    ),
    "keyword": (
        keyword_mod.kwlist == ["False", "None"]
        and keyword_mod.issoftkeyword("match") is True
        and "molt_keyword_lists" not in keyword_mod.__dict__
    ),
    "graphlib": (
        ts.get_ready() == ("a", "b")
        and list(ts.static_order()) == ["a", "b"]
        and "molt_graphlib_new" not in graphlib_mod.__dict__
    ),
    "fnmatch": (
        fnmatch_mod.fnmatch("a.py", "*.py") is True
        and fnmatch_mod.filter(["a.py", "b.txt"], "*.py") == ["a.py"]
        and "molt_fnmatch_fnmatch" not in fnmatch_mod.__dict__
    ),
    "gettext": (
        gettext_mod.gettext("hello") == "T:hello"
        and gettext_mod.ngettext("apple", "apples", 2) == "apples"
        and "molt_gettext_gettext" not in gettext_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_e() -> None:
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
        "colorsys": "True",
        "fnmatch": "True",
        "gettext": "True",
        "graphlib": "True",
        "keyword": "True",
    }
