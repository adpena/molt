#!/usr/bin/env python3
"""Reduce successful Nightly aggregates into a reviewed next-run cost profile."""

from __future__ import annotations

import argparse
import copy
import json
import os
from pathlib import Path
import sys
from collections.abc import Mapping, Sequence
from typing import Any

from tools.artifact_publish import atomic_write_json
from tools import nightly_shard_profile, nightly_sharding


ROOT = Path(__file__).resolve().parents[1]
REPORT_SCHEMA = "molt.nightly-shard-profile-report.v1"
MIN_MAKESPAN_IMPROVEMENT_PPM = 10_000


def _read_json(path: Path, label: str) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"{label} must be a JSON object")
    return payload


def _measured_loads(
    shards: Sequence[Mapping[str, Any]], durations_us: Mapping[str, int]
) -> list[int]:
    return [
        sum(durations_us[str(path)] for path in shard["entries"]) for shard in shards
    ]


def _balance(loads: Sequence[int]) -> dict[str, int]:
    total = sum(loads)
    if not loads or total <= 0:
        raise ValueError("nightly shard duration loads are empty")
    maximum = max(loads)
    return {
        "imbalance_ppm": maximum * len(loads) * 1_000_000 // total - 1_000_000,
        "max_shard_duration_us": maximum,
        "min_shard_duration_us": min(loads),
        "total_duration_us": total,
    }


def _project(
    entries: list[dict[str, Any]], count: int, durations_us: Mapping[str, int]
) -> dict[str, int]:
    return _balance(
        _measured_loads(nightly_sharding.lpt_shards(entries, count), durations_us)
    )


def fit_feedback(
    plan: Mapping[str, Any],
    aggregates: Mapping[str, Mapping[str, Any]],
    current_profile: Mapping[str, Any],
    *,
    run_identity: Mapping[str, str] | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Fit a candidate and retain only materially better program models.

    Replay is O(n log n) per program because it reuses the sole LPT authority;
    working memory is O(n), while the promoted profile is capped at 64 KiB.
    """

    programs = tuple(nightly_sharding.SHARD_COUNTS)
    nightly_shard_profile.validate_profile(current_profile, programs)
    if plan["authority"]["weight_profile"][
        "profile_sha256"
    ] != nightly_shard_profile.profile_digest(current_profile):
        raise ValueError("current shard profile does not match the measured plan")
    for program in programs:
        nightly_sharding.validate_aggregate(plan, aggregates[program])
    durations_by_program = nightly_shard_profile.validated_durations(plan, aggregates)
    candidate = nightly_shard_profile.fit_profile(plan, aggregates)
    contract_digest = plan["authority"]["measurement_contract_sha256"]
    decisions: dict[str, Any] = {}
    for program in programs:
        count = nightly_sharding.SHARD_COUNTS[program]
        durations_us = durations_by_program[program]
        current_entries = copy.deepcopy(plan["programs"][program]["entries"])
        current = _project(current_entries, count, durations_us)
        source_entries = copy.deepcopy(current_entries)
        for entry in source_entries:
            entry["weight"] = entry["source_bytes"]
        source_bytes = _project(source_entries, count, durations_us)
        current_model = current_profile["programs"][program]["model"]
        training_plan_sha256 = (
            current_model["training"]["plan_sha256"]
            if current_model is not None
            else None
        )
        current_vs_source_ppm = (
            (source_bytes["max_shard_duration_us"] - current["max_shard_duration_us"])
            * 1_000_000
            // source_bytes["max_shard_duration_us"]
        )
        candidate_profile = copy.deepcopy(current_profile)
        candidate_profile["programs"][program] = copy.deepcopy(
            candidate["programs"][program]
        )
        candidate_corpora = {
            name: copy.deepcopy(plan["programs"][name]["entries"]) for name in programs
        }
        nightly_shard_profile.apply_profile(
            candidate_corpora,
            candidate_profile,
            measurement_contract_sha256=contract_digest,
        )
        projected = _project(candidate_corpora[program], count, durations_us)
        saved = current["max_shard_duration_us"] - projected["max_shard_duration_us"]
        improvement_ppm = saved * 1_000_000 // current["max_shard_duration_us"]
        accepted = saved > 0 and improvement_ppm >= MIN_MAKESPAN_IMPROVEMENT_PPM
        if not accepted:
            candidate["programs"][program] = copy.deepcopy(
                current_profile["programs"][program]
            )
            projected = current
            saved = 0
            improvement_ppm = 0
        shard_wall_us = [
            max(1, int(round(float(shard["duration_s"]) * 1_000_000)))
            for shard in aggregates[program]["shards"]
        ]
        decisions[program] = {
            "accepted": accepted,
            "candidate_item_sum": projected,
            "current_item_sum": current,
            "current_profile_training_plan_sha256": training_plan_sha256,
            "current_profile_vs_source_bytes_ppm": current_vs_source_ppm,
            "current_profile_is_out_of_sample": (
                training_plan_sha256 is not None
                and training_plan_sha256 != plan["plan_sha256"]
            ),
            "improvement_ppm": improvement_ppm,
            "observed_shard_wall": _balance(shard_wall_us),
            "saved_max_shard_duration_us": saved,
            "source_bytes_counterfactual_item_sum": source_bytes,
        }
    nightly_shard_profile.validate_profile(candidate, programs)
    report = {
        "schema": REPORT_SCHEMA,
        "candidate_profile_sha256": nightly_shard_profile.profile_digest(candidate),
        "input_profile_sha256": nightly_shard_profile.profile_digest(current_profile),
        "plan_sha256": plan["plan_sha256"],
        "programs": decisions,
        "run_identity": dict(run_identity or {}),
        "source_commit": plan["source_commit"],
        "validation": "training-run-in-sample; next-nightly-run-is-out-of-sample",
    }
    return candidate, report


def promote(candidate: Mapping[str, Any], report: Mapping[str, Any]) -> dict[str, Any]:
    """Validate an emitted candidate/report pair before checked-in promotion."""

    programs = tuple(nightly_sharding.SHARD_COUNTS)
    nightly_shard_profile.validate_profile(candidate, programs)
    if (
        report.get("schema") != REPORT_SCHEMA
        or report.get("candidate_profile_sha256")
        != nightly_shard_profile.profile_digest(candidate)
        or not isinstance(report.get("programs"), dict)
        or set(report["programs"]) != set(programs)
    ):
        raise ValueError("nightly shard profile promotion report is invalid")
    return dict(candidate)


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)
    fit = commands.add_parser("fit")
    fit.add_argument("--plan", type=Path, required=True)
    for program in nightly_sharding.SHARD_COUNTS:
        fit.add_argument(f"--{program}-aggregate", type=Path, required=True)
    fit.add_argument("--out", type=Path, required=True)
    fit.add_argument("--report-out", type=Path, required=True)
    promote_parser = commands.add_parser("promote")
    promote_parser.add_argument("--candidate", type=Path, required=True)
    promote_parser.add_argument("--report", type=Path, required=True)
    promote_parser.add_argument(
        "--out", type=Path, default=ROOT / "config/nightly_shard_profile.json"
    )
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "fit":
            plan = _read_json(args.plan, "nightly shard plan")
            nightly_sharding.validate_plan_envelope(plan, ROOT)
            aggregates = {
                program: _read_json(
                    getattr(args, f"{program}_aggregate"), f"{program} aggregate"
                )
                for program in nightly_sharding.SHARD_COUNTS
            }
            identity = {
                key: os.environ[key]
                for key in (
                    "GITHUB_REPOSITORY",
                    "GITHUB_RUN_ATTEMPT",
                    "GITHUB_RUN_ID",
                    "GITHUB_SHA",
                )
                if os.environ.get(key)
            }
            candidate, report = fit_feedback(
                plan,
                aggregates,
                nightly_sharding.load_weight_profile(ROOT),
                run_identity=identity,
            )
            atomic_write_json(args.out, candidate, sort_keys=True)
            atomic_write_json(args.report_out, report, sort_keys=True)
        else:
            candidate = _read_json(args.candidate, "nightly shard profile candidate")
            report = _read_json(args.report, "nightly shard profile report")
            atomic_write_json(args.out, promote(candidate, report), sort_keys=True)
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError) as exc:
        print(f"nightly-profile-feedback: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
