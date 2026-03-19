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
]


def test_asyncio_batch_hides_raw_capability_intrinsic() -> None:
    for path in MODULE_PATHS:
        source = path.read_text()
        assert '_require_intrinsic("molt_capabilities_has", globals())' not in source
        assert '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")' in source
