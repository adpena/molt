from __future__ import annotations

import importlib.util
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "tools" / "canonicalization_contract.py"


def _load_contract_module():
    spec = importlib.util.spec_from_file_location("canonicalization_contract", TOOL)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_runtime_support_satellite_is_not_stdlib_duplicate_domain() -> None:
    contract = _load_contract_module()

    assert contract.layer_of("molt-stdlib-text") == "stdlib"
    assert contract._stdlib_domain("molt-stdlib-text") == "text"
    assert contract.layer_of("molt-runtime-stringprep") == "stdlib"
    assert contract._stdlib_domain("molt-runtime-stringprep") == "stringprep"
    assert contract.layer_of("molt-runtime-platform") == "core"
    assert contract._stdlib_domain("molt-runtime-platform") is None
