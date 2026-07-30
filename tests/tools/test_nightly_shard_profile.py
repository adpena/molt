from __future__ import annotations

import copy
import json

import pytest

from tools import nightly_profile_feedback, nightly_shard_profile, nightly_sharding


PROGRAMS = tuple(nightly_sharding.SHARD_COUNTS)


def _blank_profile() -> dict:
    return {
        "schema": nightly_shard_profile.PROFILE_SCHEMA,
        "policy": {
            "algorithm": nightly_shard_profile.PROFILE_ALGORITHM,
            "bucket_count": nightly_shard_profile.BUCKET_COUNT,
            "duration_unit": "microseconds",
            "max_overrides_per_program": nightly_shard_profile.MAX_OVERRIDES,
            "max_serialized_bytes": nightly_shard_profile.MAX_SERIALIZED_BYTES,
        },
        "programs": {program: {"model": None} for program in PROGRAMS},
    }


def _entry(program: str, index: int) -> dict:
    source_bytes = 100 + index * 17
    return {
        "path": f"{program}/case_{index:03d}.py",
        "sha256": f"{index + 1:064x}",
        "source_bytes": source_bytes,
        "weight": source_bytes,
    }


def _training_inputs() -> tuple[dict, dict[str, dict]]:
    programs = {}
    aggregates = {}
    for program in PROGRAMS:
        count = max(80, nightly_sharding.SHARD_COUNTS[program])
        entries = [_entry(program, index) for index in range(count)]
        programs[program] = {
            "entries": entries,
            "selected": count,
            "shards": nightly_sharding.lpt_shards(
                entries, nightly_sharding.SHARD_COUNTS[program]
            ),
            "total_weight": sum(entry["weight"] for entry in entries),
        }
        durations = {
            entry["path"]: (100_000 + 3 * entry["source_bytes"]) / 1_000_000
            for entry in entries
        }
        durations[entries[-1]["path"]] = 3.0
        aggregates[program] = {
            "item_durations_s": durations,
        }
    plan = {
        "authority": {
            "measurement_contract_sha256": "e" * 64,
            "policy": {"training_cell": "linux-x86_64-py312-native-dev"},
        },
        "authority_sha256": "a" * 64,
        "cpython_commit": "f" * 40,
        "plan_sha256": "b" * 64,
        "programs": programs,
        "source_commit": "c" * 40,
    }
    for program in PROGRAMS:
        aggregates[program].update(
            {
                "authority_sha256": plan["authority_sha256"],
                "ok": True,
                "plan_sha256": plan["plan_sha256"],
                "program": program,
                "source_commit": plan["source_commit"],
                "status_by_path": {
                    path: "passed" for path in aggregates[program]["item_durations_s"]
                },
            }
        )
    return plan, aggregates


def test_profile_fit_is_deterministic_robust_and_compact() -> None:
    plan, aggregates = _training_inputs()
    first = nightly_shard_profile.fit_profile(plan, aggregates)
    second = nightly_shard_profile.fit_profile(plan, aggregates)

    assert first == second
    nightly_shard_profile.validate_profile(first, PROGRAMS)
    assert len(json.dumps(first, sort_keys=True)) < 24_000
    for program in PROGRAMS:
        model = first["programs"][program]["model"]
        assert model["baseline"] == {
            "intercept_us": 100_000,
            "slope_denominator": 1,
            "slope_numerator": 3,
        }
        assert len(model["overrides"]) == 1
        assert model["overrides"][0]["path"].endswith("case_079.py")


def test_robust_slope_uses_all_pairs_before_nonnegative_clamp() -> None:
    baseline = nightly_shard_profile._fit_baseline(
        [(1, 100, "a"), (2, 200, "b"), (3, 100, "c"), (4, 200, "d")]
    )
    assert baseline["slope_numerator"] == 0


def test_profile_rejects_nonfinite_measurements() -> None:
    plan, aggregates = _training_inputs()
    path = next(iter(aggregates["conformance"]["item_durations_s"]))
    aggregates["conformance"]["item_durations_s"][path] = float("nan")
    with pytest.raises(ValueError, match="positive and finite"):
        nightly_shard_profile.fit_profile(plan, aggregates)


def test_profile_applies_only_source_digest_bound_outliers() -> None:
    plan, aggregates = _training_inputs()
    profile = nightly_shard_profile.fit_profile(plan, aggregates)
    corpora = {
        program: copy.deepcopy(plan["programs"][program]["entries"])
        for program in PROGRAMS
    }
    stale_path = profile["programs"]["differential"]["model"]["overrides"][0]["path"]
    stale_entry = next(
        entry for entry in corpora["differential"] if entry["path"] == stale_path
    )
    stale_entry["sha256"] = "f" * 64

    summary = nightly_shard_profile.apply_profile(
        corpora, profile, measurement_contract_sha256="e" * 64
    )

    assert summary["programs"]["differential"]["applied_overrides"] == 0
    assert summary["programs"]["differential"]["stale_overrides"] == 1
    assert stale_entry["weight"] != 3_000_000
    assert stale_entry["weight"] == 100_000 + 3 * stale_entry["source_bytes"]


def test_measurement_contract_drift_falls_back_to_source_bytes() -> None:
    plan, aggregates = _training_inputs()
    profile = nightly_shard_profile.fit_profile(plan, aggregates)
    corpora = {
        program: copy.deepcopy(plan["programs"][program]["entries"])
        for program in PROGRAMS
    }
    summary = nightly_shard_profile.apply_profile(
        corpora, profile, measurement_contract_sha256="0" * 64
    )
    assert all(
        row["method"] == "source-bytes-contract-fallback"
        for row in summary["programs"].values()
    )
    assert all(
        entry["weight"] == entry["source_bytes"]
        for entries in corpora.values()
        for entry in entries
    )


def test_profile_feedback_reports_non_regressing_measured_makespan() -> None:
    plan, aggregates = _training_inputs()
    current = _blank_profile()
    plan["schema"] = nightly_sharding.PLAN_SCHEMA
    plan["authority"]["weight_profile"] = {
        "profile_sha256": nightly_shard_profile.profile_digest(current)
    }
    for program in PROGRAMS:
        aggregate = aggregates[program]
        aggregate.update(
            {
                "schema": nightly_sharding.AGGREGATE_SCHEMA,
                "kind": "aggregate",
                "program": program,
                "ok": True,
                "plan_sha256": plan["plan_sha256"],
                "source_commit": plan["source_commit"],
                "authority_sha256": plan["authority_sha256"],
                "expected_selected": plan["programs"][program]["selected"],
                "selected": plan["programs"][program]["selected"],
                "status_by_path": {
                    path: "passed" for path in aggregate["item_durations_s"]
                },
                "shards": [
                    {
                        "duration_s": 1.0 + index,
                        "id": index,
                        "planned_weight": plan["programs"][program]["shards"][index][
                            "weight"
                        ],
                        "returncode": 0,
                        "raw_sha256": "d" * 64,
                    }
                    for index in range(nightly_sharding.SHARD_COUNTS[program])
                ],
            }
        )

    candidate, report = nightly_profile_feedback.fit_feedback(plan, aggregates, current)

    nightly_shard_profile.validate_profile(candidate, PROGRAMS)
    assert report["candidate_profile_sha256"] == (
        nightly_shard_profile.profile_digest(candidate)
    )
    assert all(
        row["candidate_item_sum"]["max_shard_duration_us"]
        <= row["current_item_sum"]["max_shard_duration_us"]
        for row in report["programs"].values()
    )
    assert nightly_profile_feedback.promote(candidate, report) == candidate
