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


builtins._molt_intrinsics = {{
    "molt_future_features": lambda: [
        ("nested_scopes", (2, 1, 0, "beta", 1), (2, 2, 0, "alpha", 0), 16),
        ("generators", (2, 2, 0, "alpha", 1), (2, 3, 0, "final", 0), 0),
        ("division", (2, 2, 0, "alpha", 2), (3, 0, 0, "alpha", 0), 131072),
        ("absolute_import", (2, 5, 0, "alpha", 1), (3, 0, 0, "alpha", 0), 262144),
        ("with_statement", (2, 5, 0, "alpha", 1), (2, 6, 0, "alpha", 0), 524288),
        ("print_function", (2, 6, 0, "alpha", 2), (3, 0, 0, "alpha", 0), 1048576),
        ("unicode_literals", (2, 6, 0, "alpha", 2), (3, 0, 0, "alpha", 0), 2097152),
        ("barry_as_FLUFL", (3, 1, 0, "alpha", 2), (4, 0, 0, "alpha", 0), 4194304),
        ("generator_stop", (3, 5, 0, "beta", 1), (3, 7, 0, "alpha", 0), 8388608),
        ("annotations", (3, 7, 0, "beta", 1), None, 16777216),
    ],
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

spec = importlib.util.spec_from_file_location(
    "molt_test__future__", {str(STDLIB_ROOT / "__future__.py")!r}
)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
sys.modules["molt_test__future__"] = module
spec.loader.exec_module(module)

checks = {{
    "intrinsic_hidden": "molt_future_features" not in module.__dict__,
    "exports": (
        "nested_scopes" in module.__dict__
        and "annotations" in module.__dict__
        and "generators" in module.__dict__
        and module.all_feature_names[0] == "nested_scopes"
        and module.all_feature_names[-1] == "annotations"
    ),
    "flags": (
        module.CO_NESTED == 16
        and module.CO_FUTURE_ANNOTATIONS == 16777216
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> dict[str, str]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, key, value = line.split("|", 2)
        assert prefix == "CHECK"
        checks[key] = value
    return checks


def test_future_private_intrinsic_surface() -> None:
    assert _run_probe() == {
        "exports": "True",
        "flags": "True",
        "intrinsic_hidden": "True",
    }
