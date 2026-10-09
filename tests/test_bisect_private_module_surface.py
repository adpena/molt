from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import inspect
import bisect as _host_bisect
import sys
from tests.stdlib_intrinsic_registry import install_registry


# The intrinsics take all five arguments positionally; CPython's bisect is the
# reference implementation of that ABI.
install_registry({{
    "molt_bisect_left": lambda a, x, lo, hi, key: _host_bisect.bisect_left(
        a, x, lo, hi, key=key
    ),
    "molt_bisect_right": lambda a, x, lo, hi, key: _host_bisect.bisect_right(
        a, x, lo, hi, key=key
    ),
    "molt_bisect_insort_left": lambda a, x, lo, hi, key: _host_bisect.insort_left(
        a, x, lo, hi, key=key
    ),
    "molt_bisect_insort_right": lambda a, x, lo, hi, key: _host_bisect.insort_right(
        a, x, lo, hi, key=key
    ),
}})


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_private = _load_module("_bisect", {str(STDLIB_ROOT / "_bisect.py")!r})

for name in sorted(n for n in dir(_private) if not n.startswith("_")):
    print(
        f"ROW|{{name}}|{{inspect.signature(getattr(_private, name))}}"
        f"|{{inspect.signature(getattr(_host_bisect, name))}}"
    )

cases = [
    ([1, 3, 5], 3, {{}}),
    ([1, 3, 3, 3, 5], 3, {{"lo": 2}}),
    ([1, 3, 3, 3, 5], 3, {{"lo": 1, "hi": 3}}),
    ([5, 3, 1], 3, {{"key": lambda v: -v}}),
    ([(1, "a"), (3, "b")], 2, {{"key": lambda item: item[0]}}),
]
same = True
for data, x, kwargs in cases:
    for name in ("bisect_left", "bisect_right"):
        same &= getattr(_private, name)(data, x, **kwargs) == getattr(
            _host_bisect, name
        )(data, x, **kwargs)
    probe_x = (x, "z") if isinstance(data[0], tuple) else x
    for name in ("insort_left", "insort_right"):
        ours, theirs = list(data), list(data)
        getattr(_private, name)(ours, probe_x, **kwargs)
        getattr(_host_bisect, name)(theirs, probe_x, **kwargs)
        same &= ours == theirs
try:
    _private.bisect_left([1], 1, 0, 1, None)
except TypeError:
    keyword_only = True
else:
    keyword_only = False
print(f"CHECK|matches_cpython|{{same}}")
print(f"CHECK|key_is_keyword_only|{{keyword_only}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = run_surface_test_process(
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


def test__bisect_public_surface_matches_cpython() -> None:
    rows, checks = _run_probe()
    # Same public names and the same signatures as CPython's _bisect.
    assert [name for name, _, _ in rows] == [
        "bisect_left",
        "bisect_right",
        "insort_left",
        "insort_right",
    ]
    assert all(ours == theirs for _, ours, theirs in rows), rows
    assert checks == {"key_is_keyword_only": "True", "matches_cpython": "True"}
