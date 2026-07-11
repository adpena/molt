import json
from pathlib import Path
from tools import powerplay_acceptance as pa


def _demo(**overrides):
    values = dict(
        real_authority=True,
        release_profile=True,
        serial_differential=True,
        held_benches_pass=True,
        memory_ceiling_pass=True,
        sample_count=5,
    )
    values.update(overrides)
    return pa.CorrectnessDemonstration(**values)


def test_variant_ii_accepts_complete_real_evidence():
    assert pa.variant_ii_accept(before=10.0, after=8.0, demonstration=_demo()).accepted


def test_variant_ii_refuses_proxy_noise_regression_and_oom():
    result = pa.variant_ii_accept(
        before=8.0,
        after=9.0,
        demonstration=_demo(
            release_profile=False, sample_count=1, memory_ceiling_pass=False
        ),
    )
    assert not result.accepted
    assert any("dev-profile" in item for item in result.hard_errors)
    assert any("single-run" in item for item in result.hard_errors)
    assert any("memory-ceiling" in item for item in result.hard_errors)


def test_existing_attestations_are_parsed_without_shape_crashes():
    for path in Path("tools").glob("perf*attestation.json"):
        result = pa.validate_attestation(json.loads(path.read_text(encoding="utf-8")))
        assert isinstance(result.accepted, bool)


def test_backlog_ranked_by_gain_per_validation_cost():
    text = "| Item | Gain | Cost |\n| fast | 10 | 2 |\n| slow | 12 | 6 |"
    assert pa.rank_backlog(text)[0][0] == "fast"
