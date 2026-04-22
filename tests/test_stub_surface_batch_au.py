from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/multiprocessing/__init__.py",
    ROOT / "src/molt/stdlib/multiprocessing/_api_surface.py",
    ROOT / "src/molt/stdlib/multiprocessing/connection.py",
    ROOT / "src/molt/stdlib/multiprocessing/context.py",
    ROOT / "src/molt/stdlib/multiprocessing/dummy/__init__.py",
    ROOT / "src/molt/stdlib/multiprocessing/dummy/connection.py",
    ROOT / "src/molt/stdlib/multiprocessing/forkserver.py",
    ROOT / "src/molt/stdlib/multiprocessing/heap.py",
    ROOT / "src/molt/stdlib/multiprocessing/pool.py",
    ROOT / "src/molt/stdlib/multiprocessing/popen_fork.py",
    ROOT / "src/molt/stdlib/multiprocessing/popen_forkserver.py",
    ROOT / "src/molt/stdlib/multiprocessing/popen_spawn_posix.py",
    ROOT / "src/molt/stdlib/multiprocessing/popen_spawn_win32.py",
    ROOT / "src/molt/stdlib/multiprocessing/process.py",
    ROOT / "src/molt/stdlib/multiprocessing/queues.py",
    ROOT / "src/molt/stdlib/multiprocessing/reduction.py",
    ROOT / "src/molt/stdlib/multiprocessing/resource_tracker.py",
    ROOT / "src/molt/stdlib/multiprocessing/shared_memory.py",
    ROOT / "src/molt/stdlib/multiprocessing/sharedctypes.py",
    ROOT / "src/molt/stdlib/multiprocessing/synchronize.py",
]


def test_multiprocessing_batch_hides_raw_capability_intrinsic() -> None:
    for path in MODULE_PATHS:
        source = path.read_text()
        assert '_require_intrinsic("molt_capabilities_has", globals())' not in source
        assert (
            '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")'
            in source
        )
