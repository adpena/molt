from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"


def _probe_source(module_name: str, module_path: Path) -> str:
    return f"""
import importlib.util
import sys
from tests.stdlib_intrinsic_registry import install_registry
from tests.crypto_intrinsic_registry import crypto_intrinsics

install_registry(crypto_intrinsics())


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_load_module("hashlib", {str(STDLIB_ROOT / "hashlib.py")!r})
_load_module("hmac", {str(STDLIB_ROOT / "hmac.py")!r})
_load_module("_hashlib", {str(STDLIB_ROOT / "_hashlib.py")!r})
_load_module("_hmac", {str(STDLIB_ROOT / "_hmac.py")!r})
module = _load_module({module_name!r}, {str(module_path)!r})

public_names = [
    name
    for name in sorted(dir(module))
    if not name.startswith("_") and name != "annotations"
]
for name in public_names:
    print(f"ROW|{{name}}")

print("CHECK|" + str("molt_capabilities_has" not in module.__dict__))
"""


def _run_probe(module_name: str, module_path: Path) -> tuple[list[str], bool]:
    proc = run_surface_test_process(
        [sys.executable, "-c", _probe_source(module_name, module_path)],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    public_names: list[str] = []
    behavior_ok = False
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|", 1)
        if prefix == "ROW":
            public_names.append(rest[0])
        elif prefix == "CHECK":
            behavior_ok = rest[0] == "True"
    return public_names, behavior_ok


@pytest.mark.parametrize(
    "module_name,module_path",
    [
        ("molt_test__blake2", STDLIB_ROOT / "_blake2.py"),
        ("molt_test__hashlib", STDLIB_ROOT / "_hashlib.py"),
        ("molt_test__hmac", STDLIB_ROOT / "_hmac.py"),
        ("molt_test__md5", STDLIB_ROOT / "_md5.py"),
        ("molt_test__sha1", STDLIB_ROOT / "_sha1.py"),
        ("molt_test__sha2", STDLIB_ROOT / "_sha2.py"),
        ("molt_test__sha3", STDLIB_ROOT / "_sha3.py"),
    ],
)
def test_crypto_private_modules_do_not_leak_capability_anchor(
    module_name: str, module_path: Path
) -> None:
    _, behavior_ok = _run_probe(module_name, module_path)
    assert behavior_ok is True
