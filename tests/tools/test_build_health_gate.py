from __future__ import annotations

from tools import build_health_gate as gate


def _diagnostics(*, hits: int, misses: int, relowered_s: float) -> dict:
    observed = hits + misses
    return {
        "frontend_lowering_cache": {
            "hits": hits,
            "misses": misses,
            "observed": observed,
            "hit_rate": hits / observed,
            "relowered_s": relowered_s,
        }
    }


def test_warm_pair_accepts_high_frontend_lowering_cache_reuse() -> None:
    cold = _diagnostics(hits=0, misses=145, relowered_s=199.8)
    warm = _diagnostics(hits=145, misses=0, relowered_s=0.0)

    assert gate.check_warm_pair(cold, warm, {"frontend_lowering_cache_warm_hit_floor": 0.9}) == []


def test_warm_pair_fails_hard_on_configured_but_ineffective_cache() -> None:
    cold = _diagnostics(hits=0, misses=145, relowered_s=199.8)
    warm = _diagnostics(hits=0, misses=145, relowered_s=198.5)

    anomalies = gate.check_warm_pair(
        cold,
        warm,
        {"frontend_lowering_cache_warm_hit_floor": 0.9},
    )

    assert [item.invariant for item in anomalies] == [
        "frontend_lowering_cache_warm_hit_rate"
    ]
    assert anomalies[0].hard is True
