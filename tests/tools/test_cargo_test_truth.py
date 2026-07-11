from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tools" / "check_cargo_test_truth.py"
SPEC = importlib.util.spec_from_file_location("check_cargo_test_truth", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_cargo_test_topology_cannot_mask_or_skip_binaries() -> None:
    assert MODULE.violations() == []
