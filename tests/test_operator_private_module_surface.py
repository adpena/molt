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


class itemgetter:
    pass


class attrgetter:
    pass


class methodcaller:
    pass


builtins._molt_intrinsics = {{
    "molt_operator_abs": abs,
    "molt_operator_add": lambda a, b: a + b,
    "molt_operator_sub": lambda a, b: a - b,
    "molt_operator_mul": lambda a, b: a * b,
    "molt_operator_matmul": lambda a, b: ("matmul", a, b),
    "molt_operator_truediv": lambda a, b: a / b,
    "molt_operator_floordiv": lambda a, b: a // b,
    "molt_operator_mod": lambda a, b: a % b,
    "molt_operator_pow": pow,
    "molt_operator_lshift": lambda a, b: a << b,
    "molt_operator_rshift": lambda a, b: a >> b,
    "molt_operator_and": lambda a, b: a & b,
    "molt_operator_or": lambda a, b: a | b,
    "molt_operator_xor": lambda a, b: a ^ b,
    "molt_operator_neg": lambda a: -a,
    "molt_operator_pos": lambda a: +a,
    "molt_operator_invert": lambda a: ~a,
    "molt_operator_not": lambda a: not a,
    "molt_operator_truth": lambda a: bool(a),
    "molt_operator_eq": lambda a, b: a == b,
    "molt_operator_ne": lambda a, b: a != b,
    "molt_operator_lt": lambda a, b: a < b,
    "molt_operator_le": lambda a, b: a <= b,
    "molt_operator_gt": lambda a, b: a > b,
    "molt_operator_ge": lambda a, b: a >= b,
    "molt_operator_is": lambda a, b: a is b,
    "molt_operator_is_not": lambda a, b: a is not b,
    "molt_operator_contains": lambda a, b: b in a,
    "molt_operator_getitem": lambda a, b: a[b],
    "molt_operator_setitem": lambda a, b, c: a.__setitem__(b, c),
    "molt_operator_delitem": lambda a, b: a.__delitem__(b),
    "molt_operator_countof": lambda a, b: list(a).count(b),
    "molt_operator_length_hint": lambda a: len(a),
    "molt_operator_concat": lambda a, b: a + b,
    "molt_operator_iconcat": lambda a, b: a.__iadd__(b),
    "molt_operator_iadd": lambda a, b: a + b,
    "molt_operator_isub": lambda a, b: a - b,
    "molt_operator_imul": lambda a, b: a * b,
    "molt_operator_imatmul": lambda a, b: ("imatmul", a, b),
    "molt_operator_itruediv": lambda a, b: a / b,
    "molt_operator_ifloordiv": lambda a, b: a // b,
    "molt_operator_imod": lambda a, b: a % b,
    "molt_operator_ipow": pow,
    "molt_operator_ilshift": lambda a, b: a << b,
    "molt_operator_irshift": lambda a, b: a >> b,
    "molt_operator_iand": lambda a, b: a & b,
    "molt_operator_ior": lambda a, b: a | b,
    "molt_operator_ixor": lambda a, b: a ^ b,
    "molt_operator_index": lambda a: int(a),
    "molt_operator_itemgetter_type": lambda: itemgetter,
    "molt_operator_attrgetter_type": lambda: attrgetter,
    "molt_operator_methodcaller_type": lambda: methodcaller,
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


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_private = _load_module("_molt_private_operator", {str(STDLIB_ROOT / "_operator.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        _private.add(2, 3) == 5
        and _private.sub(5, 2) == 3
        and _private.truth(1) is True
        and _private.contains([1, 2, 3], 2) is True
        and _private.getitem(["a", "b"], 1) == "b"
        and _private.index(7.0) == 7
        and _private.inv(1) == -2
        and _private.itemgetter is itemgetter
        and _private.attrgetter is attrgetter
        and _private.methodcaller is methodcaller
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows: list[tuple[str, str, str]] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "ROW":
            rows.append((rest[0], rest[1], rest[2]))
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test__operator_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_operator_add" not in names
    assert "add" in names
    assert "sub" in names
    assert "itemgetter" in names
    assert "methodcaller" in names
    assert checks == {"behavior": "True"}
