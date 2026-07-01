from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CHECK_RUST_TOOLCHAIN = ROOT / "tools" / "check_rust_toolchain.py"


def _load_check_rust_toolchain():
    spec = importlib.util.spec_from_file_location(
        "molt_test_check_rust_toolchain",
        CHECK_RUST_TOOLCHAIN,
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_repository_rust_toolchain_contract_is_canonical() -> None:
    tool = _load_check_rust_toolchain()

    report = tool.check_repository_contract()

    assert report.errors == ()


def test_ci_gate_uses_repo_rust_toolchain_gate_without_compile_slot() -> None:
    spec = importlib.util.spec_from_file_location(
        "molt_test_ci_gate_for_rust_toolchain",
        ROOT / "tools" / "ci_gate.py",
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)

    check = {entry.name: entry for entry in module._build_checks()}["rust-toolchain"]

    assert check.cmd == [
        sys.executable,
        str(module.TOOLS / "check_rust_toolchain.py"),
    ]
    assert check.needs_rust is False
    assert check.needs_cargo is True
