from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/asyncio/__main__.py",
    ROOT / "src/molt/stdlib/asyncio/base_events.py",
    ROOT / "src/molt/stdlib/asyncio/base_futures.py",
    ROOT / "src/molt/stdlib/asyncio/base_subprocess.py",
    ROOT / "src/molt/stdlib/asyncio/base_tasks.py",
    ROOT / "src/molt/stdlib/asyncio/constants.py",
    ROOT / "src/molt/stdlib/asyncio/coroutines.py",
    ROOT / "src/molt/stdlib/asyncio/events.py",
    ROOT / "src/molt/stdlib/asyncio/exceptions.py",
    ROOT / "src/molt/stdlib/asyncio/format_helpers.py",
    ROOT / "src/molt/stdlib/asyncio/futures.py",
    ROOT / "src/molt/stdlib/asyncio/graph.py",
    ROOT / "src/molt/stdlib/asyncio/locks.py",
    ROOT / "src/molt/stdlib/asyncio/log.py",
    ROOT / "src/molt/stdlib/asyncio/mixins.py",
    ROOT / "src/molt/stdlib/asyncio/proactor_events.py",
    ROOT / "src/molt/stdlib/asyncio/protocols.py",
    ROOT / "src/molt/stdlib/asyncio/queues.py",
    ROOT / "src/molt/stdlib/asyncio/runners.py",
    ROOT / "src/molt/stdlib/asyncio/selector_events.py",
    ROOT / "src/molt/stdlib/asyncio/streams.py",
    ROOT / "src/molt/stdlib/asyncio/trsock.py",
    ROOT / "src/molt/stdlib/asyncio/unix_events.py",
]
SOCKET_SHIM_PATHS = [
    ROOT / "src/molt/stdlib/asyncio/base_events.py",
    ROOT / "src/molt/stdlib/asyncio/events.py",
    ROOT / "src/molt/stdlib/asyncio/proactor_events.py",
    ROOT / "src/molt/stdlib/asyncio/selector_events.py",
    ROOT / "src/molt/stdlib/asyncio/streams.py",
    ROOT / "src/molt/stdlib/asyncio/trsock.py",
]


def test_asyncio_batch_hides_raw_capability_intrinsic() -> None:
    for path in MODULE_PATHS:
        source = path.read_text()
        assert '_require_intrinsic("molt_capabilities_has", globals())' not in source
        assert (
            '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")'
            in source
        )


def test_asyncio_top_level_keeps_socket_import_lazy_and_module_shaped() -> None:
    source = (ROOT / "src/molt/stdlib/asyncio/__init__.py").read_text()

    assert "import socket as _socket" not in source
    assert "class _LazySocketModule(_types.ModuleType):" in source
    assert '"socket": _SOCKET' in source
    assert '"socket": _socket_module' not in source
    assert not any(
        line == "_SOCKET_EAI_CODES = _socket_eai_codes()"
        for line in source.splitlines()
    )


def test_asyncio_submodule_shims_reuse_lazy_socket_surface() -> None:
    for path in SOCKET_SHIM_PATHS:
        source = path.read_text()
        assert "import socket" not in source.splitlines()
        assert "from asyncio import socket as socket" in source


def test_asyncio_unix_events_hides_child_watchers_on_py314_surface() -> None:
    source = (ROOT / "src/molt/stdlib/asyncio/unix_events.py").read_text()

    assert "_UnixDefaultEventLoopPolicy as DefaultEventLoopPolicy" in source
    assert "if _VERSION_INFO < (3, 14):" in source
    py314_branch = source.split("else:", 1)[1]
    assert '"DefaultEventLoopPolicy"' not in py314_branch
    assert '"AbstractChildWatcher"' not in py314_branch
