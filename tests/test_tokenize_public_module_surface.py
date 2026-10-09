from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import io
import sys
from tests.stdlib_intrinsic_registry import install_registry


calls = []

install_registry({{
    "molt_tokenize_runtime_ready": lambda: calls.append("ready"),
    "molt_tokenize_scan": lambda source: [
        (1, "x", (1, 0), (1, 1), source.splitlines()[0]),
        (4, "\\n", (1, 1), (1, 2), source.splitlines()[0]),
    ],
}})


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
    proc = run_surface_test_process(
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
