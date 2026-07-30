#!/usr/bin/env python3
"""Compact measured cost models for deterministic Nightly sharding.

This module owns cost fitting and application, not corpus discovery, shard
topology, or workflow orchestration.  Each program has one robust affine
source-size model and a bounded set of source-digest-bound outliers.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from fractions import Fraction
import hashlib
import json
import math
from statistics import median_low
from typing import Any


PROFILE_SCHEMA = "molt.nightly-shard-profile.v1"
PROFILE_ALGORITHM = "robust-bucketed-affine-v1"
BUCKET_COUNT = 16
MAX_OVERRIDES = 64
MAX_SERIALIZED_BYTES = 65_536
MIN_OUTLIER_ERROR_US = 10_000
MIN_OUTLIER_RELATIVE_ERROR = Fraction(1, 4)


def _canonical_bytes(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode("utf-8")


def canonical_digest(payload: Mapping[str, Any] | Sequence[Any]) -> str:
    return hashlib.sha256(_canonical_bytes(payload)).hexdigest()


def profile_digest(profile: Mapping[str, Any]) -> str:
    return canonical_digest(profile)


def _positive_int(value: object, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise ValueError(f"{label} must be a positive integer")
    return value


def _nonnegative_int(value: object, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ValueError(f"{label} must be a nonnegative integer")
    return value


def _hex_digest(value: object, length: int, label: str) -> str:
    if (
        not isinstance(value, str)
        or len(value) != length
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise ValueError(f"{label} is invalid")
    return value


def validate_profile(profile: Mapping[str, Any], programs: Sequence[str]) -> None:
    expected_programs = set(programs)
    if profile.get("schema") != PROFILE_SCHEMA:
        raise ValueError("nightly shard profile schema is invalid")
    if profile.get("policy") != {
        "algorithm": PROFILE_ALGORITHM,
        "bucket_count": BUCKET_COUNT,
        "duration_unit": "microseconds",
        "max_overrides_per_program": MAX_OVERRIDES,
        "max_serialized_bytes": MAX_SERIALIZED_BYTES,
    }:
        raise ValueError("nightly shard profile policy is invalid")
    rows = profile.get("programs")
    if not isinstance(rows, dict) or set(rows) != expected_programs:
        raise ValueError("nightly shard profile programs are invalid")
    if len(_canonical_bytes(profile)) > MAX_SERIALIZED_BYTES:
        raise ValueError("nightly shard profile exceeds the compact size limit")
    for program in programs:
        row = rows[program]
        if not isinstance(row, dict) or set(row) != {"model"}:
            raise ValueError(f"nightly shard profile {program} row is invalid")
        model = row["model"]
        if model is None:
            continue
        if not isinstance(model, dict) or set(model) != {
            "baseline",
            "overrides",
            "training",
        }:
            raise ValueError(f"nightly shard profile {program} model is invalid")
        baseline = model["baseline"]
        if not isinstance(baseline, dict) or set(baseline) != {
            "intercept_us",
            "slope_denominator",
            "slope_numerator",
        }:
            raise ValueError(f"nightly shard profile {program} baseline is invalid")
        _nonnegative_int(baseline["intercept_us"], f"{program} intercept")
        _nonnegative_int(baseline["slope_numerator"], f"{program} slope numerator")
        _positive_int(baseline["slope_denominator"], f"{program} slope denominator")
        training = model["training"]
        if not isinstance(training, dict) or set(training) != {
            "aggregate_sha256",
            "cell",
            "corpus_sha256",
            "cpython_commit",
            "measurement_contract_sha256",
            "plan_sha256",
            "samples",
            "source_commit",
        }:
            raise ValueError(f"nightly shard profile {program} training is invalid")
        _positive_int(training["samples"], f"{program} training samples")
        _hex_digest(training["aggregate_sha256"], 64, f"{program} aggregate digest")
        if not isinstance(training["cell"], str) or not training["cell"]:
            raise ValueError(f"nightly shard profile {program} cell is invalid")
        _hex_digest(training["corpus_sha256"], 64, f"{program} corpus digest")
        _hex_digest(training["cpython_commit"], 40, f"{program} CPython commit")
        _hex_digest(
            training["measurement_contract_sha256"],
            64,
            f"{program} measurement contract digest",
        )
        _hex_digest(training["plan_sha256"], 64, f"{program} plan digest")
        _hex_digest(training["source_commit"], 40, f"{program} source commit")
        overrides = model["overrides"]
        if not isinstance(overrides, list) or len(overrides) > MAX_OVERRIDES:
            raise ValueError(f"nightly shard profile {program} overrides are invalid")
        seen: set[str] = set()
        for override in overrides:
            if not isinstance(override, dict) or set(override) != {
                "duration_us",
                "path",
                "sha256",
            }:
                raise ValueError(f"nightly shard profile {program} override is invalid")
            path = override["path"]
            if not isinstance(path, str) or not path or path in seen:
                raise ValueError(
                    f"nightly shard profile {program} override path is invalid"
                )
            seen.add(path)
            _hex_digest(
                override["sha256"], 64, f"nightly shard profile {program}:{path}"
            )
            _positive_int(override["duration_us"], f"{program}:{path} duration")


def _round_fraction(value: Fraction) -> int:
    quotient, remainder = divmod(value.numerator, value.denominator)
    return quotient + int(remainder * 2 >= value.denominator)


def _predict(source_bytes: int, baseline: Mapping[str, int]) -> int:
    denominator = int(baseline["slope_denominator"])
    numerator = (
        int(baseline["intercept_us"]) * denominator
        + int(baseline["slope_numerator"]) * source_bytes
    )
    return max(1, (numerator + denominator - 1) // denominator)


def apply_profile(
    corpora: Mapping[str, list[dict[str, Any]]],
    profile: Mapping[str, Any],
    *,
    measurement_contract_sha256: str,
) -> dict[str, Any]:
    """Apply *profile* in place and report measured versus stale coverage."""

    programs = tuple(corpora)
    validate_profile(profile, programs)
    _hex_digest(measurement_contract_sha256, 64, "measurement contract digest")
    summary: dict[str, Any] = {
        "schema": PROFILE_SCHEMA,
        "profile_sha256": profile_digest(profile),
        "programs": {},
    }
    for program in programs:
        model = profile["programs"][program]["model"]
        entries = corpora[program]
        if model is None:
            summary["programs"][program] = {
                "applied_overrides": 0,
                "method": "source-bytes-fallback",
                "stale_overrides": 0,
                "training_samples": 0,
            }
            continue
        if (
            model["training"]["measurement_contract_sha256"]
            != measurement_contract_sha256
        ):
            summary["programs"][program] = {
                "applied_overrides": 0,
                "method": "source-bytes-contract-fallback",
                "stale_overrides": len(model["overrides"]),
                "training_samples": model["training"]["samples"],
            }
            continue
        baseline = model["baseline"]
        overrides = {row["path"]: row for row in model["overrides"]}
        applied = 0
        for entry in entries:
            source_bytes = _positive_int(
                entry.get("source_bytes"), f"{program}:{entry.get('path')} source bytes"
            )
            entry["weight"] = _predict(source_bytes, baseline)
            override = overrides.get(entry["path"])
            if override is not None and override["sha256"] == entry["sha256"]:
                entry["weight"] = override["duration_us"]
                applied += 1
        summary["programs"][program] = {
            "applied_overrides": applied,
            "method": PROFILE_ALGORITHM,
            "stale_overrides": len(overrides) - applied,
            "training_samples": model["training"]["samples"],
        }
    return summary


def _duration_us(value: object, label: str) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(float(value))
        or float(value) <= 0
    ):
        raise ValueError(f"{label} duration must be positive and finite")
    return max(1, int(round(float(value) * 1_000_000)))


def validated_durations(
    plan: Mapping[str, Any], aggregates: Mapping[str, Mapping[str, Any]]
) -> dict[str, dict[str, int]]:
    programs = tuple(plan["programs"])
    if set(aggregates) != set(programs):
        raise ValueError("nightly profile aggregate program closure is invalid")
    result: dict[str, dict[str, int]] = {}
    for program in programs:
        aggregate = aggregates[program]
        expected_paths = {
            str(entry["path"]) for entry in plan["programs"][program]["entries"]
        }
        durations = aggregate.get("item_durations_s")
        statuses = aggregate.get("status_by_path")
        if (
            aggregate.get("ok") is not True
            or aggregate.get("program") != program
            or aggregate.get("plan_sha256") != plan.get("plan_sha256")
            or aggregate.get("source_commit") != plan.get("source_commit")
            or aggregate.get("authority_sha256") != plan.get("authority_sha256")
            or not isinstance(durations, dict)
            or set(durations) != expected_paths
            or not isinstance(statuses, dict)
            or set(statuses) != expected_paths
            or any(status != "passed" for status in statuses.values())
        ):
            raise ValueError(f"nightly {program} profile telemetry is invalid")
        result[program] = {
            path: _duration_us(value, f"{program}:{path}")
            for path, value in durations.items()
        }
    return result


def _bucket_points(samples: Sequence[tuple[int, int, str]]) -> list[tuple[int, int]]:
    count = min(BUCKET_COUNT, len(samples))
    ordered = sorted(samples, key=lambda row: (row[0], row[2]))
    return [
        (
            median_low(
                [
                    row[0]
                    for row in ordered[
                        index * len(ordered) // count : (index + 1)
                        * len(ordered)
                        // count
                    ]
                ]
            ),
            median_low(
                [
                    row[1]
                    for row in ordered[
                        index * len(ordered) // count : (index + 1)
                        * len(ordered)
                        // count
                    ]
                ]
            ),
        )
        for index in range(count)
    ]


def _fit_baseline(samples: Sequence[tuple[int, int, str]]) -> dict[str, int]:
    points = _bucket_points(samples)
    slopes = sorted(
        Fraction(right[1] - left[1], right[0] - left[0])
        for left_index, left in enumerate(points)
        for right in points[left_index + 1 :]
        if right[0] != left[0]
    )
    slope = max(Fraction(0), median_low(slopes)) if slopes else Fraction(0)
    intercept = max(
        Fraction(0),
        median_low(
            sorted(
                Fraction(duration) - slope * source_bytes
                for source_bytes, duration, _ in samples
            )
        ),
    )
    return {
        "intercept_us": _round_fraction(intercept),
        "slope_denominator": slope.denominator,
        "slope_numerator": slope.numerator,
    }


def fit_profile(
    plan: Mapping[str, Any], aggregates: Mapping[str, Mapping[str, Any]]
) -> dict[str, Any]:
    """Fit a compact profile in O(n log n + B^2) time and O(n) memory.

    ``B`` is the fixed 16-bucket robust-regression bound, so corpus growth is
    dominated by sorting and the checked-in result remains O(1) bounded state.
    """

    durations_by_program = validated_durations(plan, aggregates)
    programs: dict[str, Any] = {}
    for program, projected in plan["programs"].items():
        aggregate = aggregates[program]
        durations = durations_by_program[program]
        samples = [
            (
                _positive_int(entry.get("source_bytes"), f"{program} source bytes"),
                durations[entry["path"]],
                str(entry["path"]),
            )
            for entry in projected["entries"]
        ]
        baseline = _fit_baseline(samples)
        by_path = {str(entry["path"]): entry for entry in projected["entries"]}
        ranked: list[tuple[Fraction, int, str, int]] = []
        for source_bytes, duration_us, path in samples:
            predicted = _predict(source_bytes, baseline)
            error = abs(duration_us - predicted)
            relative = Fraction(error, max(1, predicted))
            if error >= MIN_OUTLIER_ERROR_US and relative >= MIN_OUTLIER_RELATIVE_ERROR:
                ranked.append((relative, error, path, duration_us))
        ranked.sort(key=lambda row: (-row[0], -row[1], row[2]))
        programs[program] = {
            "model": {
                "baseline": baseline,
                "overrides": [
                    {
                        "duration_us": duration_us,
                        "path": path,
                        "sha256": by_path[path]["sha256"],
                    }
                    for _, _, path, duration_us in ranked[:MAX_OVERRIDES]
                ],
                "training": {
                    "aggregate_sha256": canonical_digest(aggregate),
                    "cell": plan["authority"]["policy"]["training_cell"],
                    "corpus_sha256": canonical_digest(projected["entries"]),
                    "cpython_commit": plan["cpython_commit"],
                    "measurement_contract_sha256": plan["authority"][
                        "measurement_contract_sha256"
                    ],
                    "plan_sha256": plan["plan_sha256"],
                    "samples": len(samples),
                    "source_commit": plan["source_commit"],
                },
            }
        }
    profile = {
        "schema": PROFILE_SCHEMA,
        "policy": {
            "algorithm": PROFILE_ALGORITHM,
            "bucket_count": BUCKET_COUNT,
            "duration_unit": "microseconds",
            "max_overrides_per_program": MAX_OVERRIDES,
            "max_serialized_bytes": MAX_SERIALIZED_BYTES,
        },
        "programs": programs,
    }
    validate_profile(profile, tuple(plan["programs"]))
    return profile
