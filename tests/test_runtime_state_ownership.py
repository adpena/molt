from __future__ import annotations

import re
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


RUNTIME = ROOT / "runtime/molt-runtime/src"
OBJECT_STATIC = re.compile(r"^\s*static\s+([A-Z0-9_]+)\s*:\s*AtomicU64", re.M)


@pytest.mark.parametrize(
    ("module", "counters", "teardown"),
    [
        ("builtins/exceptions.rs", set(), "exceptions_clear_runtime_state"),
        ("builtins/modules.rs", {"TRACE_LAST_OP"}, "modules_clear_runtime_state"),
        (
            "builtins/platform.rs",
            {"EXTENSION_METADATA_CACHE_HITS", "EXTENSION_METADATA_CACHE_MISSES"},
            "platform_clear_runtime_state",
        ),
    ],
)
def test_subsystem_object_slots_live_in_runtime_state(
    module: str, counters: set[str], teardown: str
) -> None:
    """Object bits cached in a process static would outlive runtime teardown.

    Each subsystem keeps them in runtime state, and teardown clears that state;
    the only process statics left are plain counters.
    """
    text = (RUNTIME / module).read_text(encoding="utf-8")
    lifecycle = (RUNTIME / "state/lifecycle.rs").read_text(encoding="utf-8")

    assert set(OBJECT_STATIC.findall(text)) == counters
    assert re.search(rf"\b{teardown}\s*\(", text), "subsystem owns its clear"
    assert re.search(rf"\b{teardown}\s*\(", lifecycle), "teardown clears it"


def test_importlib_platform_static_names_are_runtime_owned() -> None:
    files = {
        "runtime/molt-runtime/src/builtins/platform.rs": {
            "EXTENSION_METADATA_CACHE_HITS",
            "EXTENSION_METADATA_CACHE_MISSES",
        },
        "runtime/molt-runtime/src/builtins/platform_importlib_ffi.rs": set(),
        "runtime/molt-runtime/src/async_rt/channels.rs": set(),
    }

    for rel_path, allowed in files.items():
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        statics = set(
            re.findall(r"^\s*static\s+([A-Z0-9_]+)\s*:\s*AtomicU64", text, re.M)
        )
        assert statics == allowed
        assert not re.search(r"intern_static_name\(_py,\s*&[A-Z0-9_]+", text)

    cache_text = (ROOT / "runtime/molt-runtime/src/state/cache.rs").read_text(
        encoding="utf-8"
    )
    lifecycle_text = (ROOT / "runtime/molt-runtime/src/state/lifecycle.rs").read_text(
        encoding="utf-8"
    )
    assert "struct RuntimeStaticNames" in cache_text
    assert "intern_runtime_static_name" in cache_text
    assert "clear_runtime_static_names" in lifecycle_text
