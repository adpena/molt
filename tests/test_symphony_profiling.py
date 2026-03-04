from __future__ import annotations

from molt.symphony.models import LatencyStats, ProfilingStats


def test_latency_stats_snapshot_tracks_p95() -> None:
    stats = LatencyStats()
    for value in (10.0, 20.0, 30.0, 40.0, 100.0):
        stats.observe(value)
    snap = stats.snapshot()
    assert snap["count"] == 5
    assert snap["avg_ms"] == 40.0
    assert snap["p95_ms"] == 100.0
    assert snap["max_ms"] == 100.0


def test_profiling_hotspots_sorts_by_latency() -> None:
    prof = ProfilingStats()
    for _ in range(5):
        prof.observe_latency("tick", 3.0)
    for _ in range(3):
        prof.observe_latency("turn", 25.0)
    for _ in range(2):
        prof.observe_latency("retry_backoff", 8.0)
    snap = prof.snapshot()
    hotspots = snap["hotspots"]
    assert hotspots
    assert hotspots[0]["label"] == "turn"
