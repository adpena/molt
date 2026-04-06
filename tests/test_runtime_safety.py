from __future__ import annotations

from pathlib import Path

import pytest

from tools import runtime_safety


def test_fuzz_workspace_for_runtime_target() -> None:
    workspace = runtime_safety._fuzz_workspace_for_target("string_ops")
    assert workspace == Path("runtime/molt-runtime/fuzz") or workspace == (
        runtime_safety.RUNTIME_FUZZ_DIR
    )


def test_fuzz_workspace_for_root_target() -> None:
    workspace = runtime_safety._fuzz_workspace_for_target("fuzz_ir_parse")
    assert workspace == Path("fuzz") or workspace == runtime_safety.ROOT_FUZZ_DIR


def test_fuzz_workspace_for_unknown_target_raises() -> None:
    with pytest.raises(SystemExit, match="unknown fuzz target"):
        runtime_safety._fuzz_workspace_for_target("definitely_missing_target")
