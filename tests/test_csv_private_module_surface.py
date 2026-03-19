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


class Dialect:
    pass


class Error(Exception):
    pass


_fake_csv = types.ModuleType("csv")
_fake_csv.Dialect = Dialect
_fake_csv.Error = Error
_fake_csv.QUOTE_ALL = 1
_fake_csv.QUOTE_MINIMAL = 0
_fake_csv.QUOTE_NONE = 3
_fake_csv.QUOTE_NONNUMERIC = 2
_fake_csv.QUOTE_NOTNULL = 5
_fake_csv.QUOTE_STRINGS = 4
_fake_csv.field_size_limit = lambda limit=None: 131072 if limit is None else int(limit)
_fake_csv.get_dialect = lambda name: ("dialect", name)
_fake_csv.list_dialects = lambda: ["excel", "unix"]
_fake_csv.reader = lambda rows, *args, **kwargs: ("reader", tuple(rows))
_fake_csv.register_dialect = lambda name, dialect=None, **fmtparams: (name, dialect, fmtparams)
_fake_csv.unregister_dialect = lambda name: name
_fake_csv.writer = lambda target, *args, **kwargs: ("writer", target)
sys.modules["csv"] = _fake_csv

builtins._molt_intrinsics = {{
    "molt_csv_runtime_ready": lambda: None,
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


_private = _load_module("_csv", {str(STDLIB_ROOT / "_csv.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        _private.QUOTE_ALL == 1
        and _private.field_size_limit() == 131072
        and _private.get_dialect("excel") == ("dialect", "excel")
        and _private.list_dialects() == ["excel", "unix"]
        and _private.reader(["a,b"]) == ("reader", ("a,b",))
        and _private.writer("target") == ("writer", "target")
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


def test__csv_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("Dialect", "type", "True"),
        ("Error", "type", "True"),
        ("QUOTE_ALL", "int", "False"),
        ("QUOTE_MINIMAL", "int", "False"),
        ("QUOTE_NONE", "int", "False"),
        ("QUOTE_NONNUMERIC", "int", "False"),
        ("QUOTE_NOTNULL", "int", "False"),
        ("QUOTE_STRINGS", "int", "False"),
        ("field_size_limit", "function", "True"),
        ("get_dialect", "function", "True"),
        ("list_dialects", "function", "True"),
        ("reader", "function", "True"),
        ("register_dialect", "function", "True"),
        ("unregister_dialect", "function", "True"),
        ("writer", "function", "True"),
    ]
    assert checks == {"behavior": "True"}
