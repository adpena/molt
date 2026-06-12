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


def test_module_object_slots_are_runtime_owned() -> None:
    text = (ROOT / "runtime/molt-runtime/src/builtins/modules.rs").read_text(
        encoding="utf-8"
    )
    statics = re.findall(r"^\s*static\s+([A-Z0-9_]+)\s*:\s*AtomicU64", text, re.M)

    assert statics == ["TRACE_LAST_OP"]
    assert "struct ModulesRuntimeState" in text
    assert "modules_clear_runtime_state" in text
    assert "clear_modules_runtime_state" in (
        ROOT / "runtime/molt-runtime/src/state/lifecycle.rs"
    ).read_text(encoding="utf-8")


def test_platform_object_slots_are_runtime_owned() -> None:
    text = (ROOT / "runtime/molt-runtime/src/builtins/platform.rs").read_text(
        encoding="utf-8"
    )

    for name in [
        "ERRNO_CONSTANTS_CACHE",
        "SOCKET_CONSTANTS_CACHE",
        "OS_NAME_CACHE",
        "SYS_PLATFORM_CACHE",
    ]:
        assert f"static {name}: AtomicU64" not in text
    assert "struct PlatformRuntimeState" in text
    assert "platform_clear_runtime_state" in text
    assert "clear_platform_runtime_state" in (
        ROOT / "runtime/molt-runtime/src/state/lifecycle.rs"
    ).read_text(encoding="utf-8")


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
