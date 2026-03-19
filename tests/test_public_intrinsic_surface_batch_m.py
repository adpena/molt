from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import math
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
    "molt_cmath_acos": lambda r, i: (1.0, 2.0),
    "molt_cmath_acosh": lambda r, i: (1.1, 2.1),
    "molt_cmath_asin": lambda r, i: (1.2, 2.2),
    "molt_cmath_asinh": lambda r, i: (1.3, 2.3),
    "molt_cmath_atan": lambda r, i: (1.4, 2.4),
    "molt_cmath_atanh": lambda r, i: (1.5, 2.5),
    "molt_cmath_cos": lambda r, i: (1.6, 2.6),
    "molt_cmath_cosh": lambda r, i: (1.7, 2.7),
    "molt_cmath_sin": lambda r, i: (1.8, 2.8),
    "molt_cmath_sinh": lambda r, i: (1.9, 2.9),
    "molt_cmath_tan": lambda r, i: (2.0, 3.0),
    "molt_cmath_tanh": lambda r, i: (2.1, 3.1),
    "molt_cmath_exp": lambda r, i: (2.2, 3.2),
    "molt_cmath_log": lambda r, i: (2.3, 3.3),
    "molt_cmath_log10": lambda r, i: (2.4, 3.4),
    "molt_cmath_sqrt": lambda r, i: (2.5, 3.5),
    "molt_cmath_phase": lambda r, i: 0.75,
    "molt_cmath_polar": lambda r, i: (5.0, 0.25),
    "molt_cmath_rect": lambda r, phi: (r, phi),
    "molt_cmath_isfinite": lambda r, i: True,
    "molt_cmath_isinf": lambda r, i: False,
    "molt_cmath_isnan": lambda r, i: False,
    "molt_cmath_isclose": lambda ar, ai, br, bi: True,
    "molt_cmath_constants": lambda: (math.pi, math.e, math.tau, math.inf, 0.0, math.inf, math.nan, 0.0, math.nan),
    "molt_zoneinfo_runtime_ready": lambda: None,
    "molt_zoneinfo_new": lambda key: {{"key": key}},
    "molt_zoneinfo_drop": lambda handle: None,
    "molt_zoneinfo_key": lambda handle: handle["key"],
    "molt_zoneinfo_utcoffset": lambda handle, comps: 3600,
    "molt_zoneinfo_dst": lambda handle, comps: 0,
    "molt_zoneinfo_tzname": lambda handle, comps: "CST",
    "molt_zoneinfo_available_timezones": lambda: {{"UTC", "America/Chicago"}},
    "molt_capabilities_has": lambda name: True,
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


cmath_mod = _load_module("molt_test_cmath", {str(STDLIB_ROOT / "cmath.py")!r})
zoneinfo_mod = _load_module("molt_test_zoneinfo", {str(STDLIB_ROOT / "zoneinfo" / "__init__.py")!r})
aifc_mod = _load_module("molt_test_aifc", {str(STDLIB_ROOT / "aifc.py")!r})


def _raises_runtimeerror(mod, attr):
    try:
        getattr(mod, attr)
    except RuntimeError:
        return True
    return False


zi = zoneinfo_mod.ZoneInfo("America/Chicago")

checks = {{
    "cmath": (
        cmath_mod.sin(1) == complex(1.8, 2.8)
        and cmath_mod.phase(1j) == 0.75
        and "molt_cmath_sin" not in cmath_mod.__dict__
    ),
    "zoneinfo": (
        zi.key == "America/Chicago"
        and zoneinfo_mod.available_timezones() == {{"UTC", "America/Chicago"}}
        and "molt_zoneinfo_new" not in zoneinfo_mod.__dict__
    ),
    "aifc": (
        _raises_runtimeerror(aifc_mod, "open")
        and "molt_capabilities_has" not in aifc_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_m() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        if line.startswith("CHECK|"):
            _, key, value = line.split("|", 2)
            checks[key] = value
    assert checks == {
        "aifc": "True",
        "cmath": "True",
        "zoneinfo": "True",
    }
