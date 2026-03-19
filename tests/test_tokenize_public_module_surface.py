from __future__ import annotations

import subprocess
import sys
from pathlib import Path


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
tokens = list(tokenize.tokenize(io.BytesIO(b"x\\n").readline))

checks = {{
    "behavior": (
        calls == ["ready"]
        and tokens[0].type == tokenize.ENCODING
        and tokens[1].type == tokenize.NAME
        and tokens[1].string == "x"
    ),
    "private_handles_hidden": (
        "_MOLT_TOKENIZE_RUNTIME_READY" not in tokenize.__dict__
        and "_MOLT_TOKENIZE_SCAN" not in tokenize.__dict__
        and "molt_tokenize_runtime_ready" not in tokenize.__dict__
        and "molt_tokenize_scan" not in tokenize.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_tokenize_public_module_hides_bootstrap_handles() -> None:
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
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
