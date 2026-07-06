from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import io
import sys
import types


calls = []

builtins._molt_intrinsics = {{
    "molt_import_smoke_runtime_ready": lambda: calls.append("import_ready"),
    "molt_tokenize_runtime_ready": lambda: calls.append("ready"),
    "molt_tokenize_scan": lambda source: [
        (1, "x", (1, 0), (1, 1), source.splitlines()[0]),
        (4, "\\n", (1, 1), (1, 2), source.splitlines()[0]),
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


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


tokenize = _load_module("tokenize", {str(STDLIB_ROOT / "tokenize.py")!r})
_private = _load_module("_molt_private_tokenize", {str(STDLIB_ROOT / "_tokenize.py")!r})
tokens = list(_private.tokenize(io.BytesIO(b"x\\n").readline))

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        calls == ["import_ready", "ready"]
        and _private.TokenInfo is tokenize.TokenInfo
        and _private.tokenize is tokenize.tokenize
        and tokens[0].type == _private.ENCODING
        and tokens[1].type == _private.NAME
        and tokens[1].string == "x"
    ),
    "private_handles_hidden": (
        "_MOLT_IMPORT_SMOKE_RUNTIME_READY" not in _private.__dict__
        and "_MOLT_TOKENIZE_RUNTIME_READY" not in _private.__dict__
        and "_MOLT_TOKENIZE_SCAN" not in _private.__dict__
        and "molt_import_smoke_runtime_ready" not in _private.__dict__
        and "molt_tokenize_runtime_ready" not in _private.__dict__
        and "molt_tokenize_scan" not in _private.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
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


def test__tokenize_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_import_smoke_runtime_ready" not in names
    assert "molt_tokenize_runtime_ready" not in names
    assert "molt_tokenize_scan" not in names
    assert names == [
        "COMMENT",
        "ENCODING",
        "ENDMARKER",
        "NAME",
        "NEWLINE",
        "NL",
        "NUMBER",
        "OP",
        "TokenInfo",
        "tokenize",
    ]
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
