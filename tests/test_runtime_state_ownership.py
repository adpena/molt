from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_exception_object_slots_are_runtime_owned() -> None:
    text = (ROOT / "runtime/molt-runtime/src/builtins/exceptions.rs").read_text(
        encoding="utf-8"
    )
    statics = re.findall(r"^\s*static\s+([A-Z0-9_]+)\s*:\s*AtomicU64", text, re.M)

    assert statics == []
    assert "struct ExceptionsRuntimeState" in text
    assert "exceptions_clear_runtime_state" in text
    assert "clear_exceptions_runtime_state" in (
        ROOT / "runtime/molt-runtime/src/state/lifecycle.rs"
    ).read_text(encoding="utf-8")
