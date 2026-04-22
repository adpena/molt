from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/asyncio/sslproto.py",
    ROOT / "src/molt/stdlib/asyncio/staggered.py",
    ROOT / "src/molt/stdlib/asyncio/streams.py",
    ROOT / "src/molt/stdlib/asyncio/subprocess.py",
    ROOT / "src/molt/stdlib/asyncio/taskgroups.py",
    ROOT / "src/molt/stdlib/asyncio/tasks.py",
    ROOT / "src/molt/stdlib/asyncio/threads.py",
    ROOT / "src/molt/stdlib/asyncio/timeouts.py",
    ROOT / "src/molt/stdlib/asyncio/tools.py",
    ROOT / "src/molt/stdlib/asyncio/transports.py",
    ROOT / "src/molt/stdlib/asyncio/trsock.py",
    ROOT / "src/molt/stdlib/asyncio/unix_events.py",
    ROOT / "src/molt/stdlib/asyncio/windows_events.py",
    ROOT / "src/molt/stdlib/asyncio/windows_utils.py",
    ROOT / "src/molt/stdlib/concurrent/futures/_base.py",
    ROOT / "src/molt/stdlib/concurrent/futures/process.py",
    ROOT / "src/molt/stdlib/concurrent/futures/thread.py",
    ROOT / "src/molt/stdlib/ctypes/_aix.py",
    ROOT / "src/molt/stdlib/importlib/metadata/diagnose.py",
    ROOT / "src/molt/stdlib/logging/config.py",
]


def test_asyncio_and_public_shim_batch_hides_raw_capability_intrinsic() -> None:
    for path in MODULE_PATHS:
        source = path.read_text()
        assert '_require_intrinsic("molt_capabilities_has", globals())' not in source
        assert (
            '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")'
            in source
        )
