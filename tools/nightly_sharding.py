#!/usr/bin/env python3
"""Deterministic, digest-bound planning and evidence for Nightly test shards.

This module owns the data plane only.  Workflow topology remains in the proof
plan authority, while the three harnesses remain responsible for interpreting
their own results.  A runtime plan is deliberately generated per source commit:
the complete corpus can be large, but no generated corpus manifest is checked
into the repository.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import sys
import tempfile
import time
import tomllib
from collections.abc import Mapping, Sequence
from typing import Any
import zipfile

from tools.artifact_publish import (
    atomic_write_json,
    publish_validated_outputs,
    staged_output_path,
)
from tools.command_execution import CommandExecutor
from tools import nightly_shard_profile


ROOT = Path(__file__).resolve().parents[1]
EXECUTOR = CommandExecutor.for_file(__file__)
PLAN_SCHEMA = "molt.nightly-shard-plan.v2"
EVIDENCE_SCHEMA = "molt.nightly-shard-evidence.v1"
AGGREGATE_SCHEMA = "molt.nightly-shard-aggregate.v2"
SHARD_COUNTS = {"conformance": 8, "differential": 16, "regrtest": 4}
FILE_ATTRIBUTE_REPARSE_POINT = 0x400

WEIGHT_POLICY = {
    "schema": "molt.nightly-shard-weight-policy.v2",
    "algorithm": "deterministic-lpt-v1",
    "weight": "compact-measured-profile-with-source-bytes-fallback",
    "tie_break": "weight-descending-then-path; bin-weight-then-id",
    "shards": SHARD_COUNTS,
    "training_cell": "linux-x86_64-py312-native-dev",
}
AUTHORITY_INPUTS = (
    ".github/workflows/nightly.yml",
    "tools/nightly_prepare.py",
    "tools/nightly_profile_feedback.py",
    "tools/nightly_shard_profile.py",
    "tools/nightly_sharding.py",
    "tools/nightly_runtime_bundle.py",
    "config/nightly_shard_profile.json",
    "tests/harness/run_molt_conformance.py",
    "tests/molt_diff.py",
    "tools/cpython_regrtest.py",
    "tools/cpython_regrtest_core.txt",
    "config/cpython_regrtest_sources.toml",
    "tools/proof_plan.toml",
    "tools/proof_plan.py",
)


def _canonical_bytes(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode("utf-8")


def _digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _file_digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _json_digest(value: Any) -> str:
    return _digest_bytes(_canonical_bytes(value))


def _relative(root: Path, path: Path) -> str:
    return path.resolve().relative_to(root.resolve()).as_posix()


def _git_commit(root: Path) -> str:
    value = EXECUTOR.check_output(
        ["git", "-C", str(root), "rev-parse", "HEAD"], text=True
    )
    assert isinstance(value, str)
    commit = value.strip().lower()
    if len(commit) != 40 or any(char not in "0123456789abcdef" for char in commit):
        raise ValueError(f"invalid source commit identity: {commit!r}")
    return commit


def _pinned_cpython_commit(root: Path) -> str:
    authority = tomllib.loads(
        (root / "config" / "cpython_regrtest_sources.toml").read_text(encoding="utf-8")
    )
    sources = authority.get("source")
    if authority.get("schema") != "molt.cpython-regrtest-sources.v1" or not isinstance(
        sources, list
    ):
        raise ValueError("CPython regrtest source authority is invalid")
    revisions = {
        str(row.get("revision", "")).lower()
        for row in sources
        if isinstance(row, dict) and str(row.get("python")) == "3.12"
    }
    if len(revisions) != 1:
        raise ValueError("CPython 3.12 regrtest source authority is not unique")
    revision = revisions.pop()
    if len(revision) != 40 or any(char not in "0123456789abcdef" for char in revision):
        raise ValueError("CPython 3.12 regrtest revision is not a full commit")
    return revision


def _file_entry(root: Path, path: Path) -> dict[str, Any]:
    relative = _relative(root, path)
    size = path.stat().st_size
    return {
        "path": relative,
        "sha256": _file_digest(path),
        "source_bytes": max(1, size),
        "weight": max(1, size),
    }


def _regrtest_module_sources(test_root: Path, name: str) -> list[Path]:
    file_path = test_root / f"{name}.py"
    if file_path.is_file():
        return [file_path]
    package = test_root / name
    if package.is_dir() and (package / "__main__.py").is_file():
        return sorted(path for path in package.rglob("*.py") if path.is_file())
    raise ValueError(f"CPython regrtest discovery produced no source for {name}")


def _discover_regrtest(root: Path) -> list[dict[str, Any]]:
    test_root = root / "third_party" / "cpython" / "Lib" / "test"
    if not test_root.is_dir():
        raise ValueError(f"pinned CPython Lib/test is missing: {test_root}")
    names = {
        entry.stem
        for entry in test_root.iterdir()
        if entry.is_file() and entry.suffix == ".py" and entry.name.startswith("test_")
    }
    names.update(
        entry.name
        for entry in test_root.iterdir()
        if entry.is_dir()
        and entry.name.startswith("test_")
        and (entry / "__main__.py").is_file()
    )
    if not names:
        raise ValueError("pinned CPython regrtest corpus is empty")
    entries: list[dict[str, Any]] = []
    for name in sorted(names):
        sources = _regrtest_module_sources(test_root, name)
        source_rows = [
            {
                "path": _relative(root, source),
                "sha256": _file_digest(source),
                "size": source.stat().st_size,
            }
            for source in sources
        ]
        entries.append(
            {
                "path": name,
                "sha256": _json_digest(source_rows),
                "source_bytes": max(1, sum(int(row["size"]) for row in source_rows)),
                "weight": max(1, sum(int(row["size"]) for row in source_rows)),
                "sources": source_rows,
            }
        )
    return entries


def discover_corpora(root: Path = ROOT) -> dict[str, list[dict[str, Any]]]:
    """Discover exact Nightly corpora in stable path order."""

    conformance_root = root / "tests" / "harness" / "corpus" / "monty_compat"
    differential_roots = (
        root / "tests" / "differential" / "basic",
        root / "tests" / "differential" / "stdlib",
    )
    corpora = {
        "conformance": [
            _file_entry(root, path)
            for path in sorted(conformance_root.glob("*.py"))
            if path.is_file()
        ],
        "differential": [
            _file_entry(root, path)
            for directory in differential_roots
            for path in sorted(directory.rglob("*.py"))
            if path.is_file()
        ],
        "regrtest": _discover_regrtest(root),
    }
    for name, entries in corpora.items():
        paths = [str(entry["path"]) for entry in entries]
        if not entries:
            raise ValueError(f"{name} corpus is empty")
        if paths != sorted(paths) or len(paths) != len(set(paths)):
            raise ValueError(f"{name} corpus discovery is not unique and ordered")
    return corpora


def _current_corpus_paths(root: Path, program: str) -> list[str]:
    if program == "conformance":
        return sorted(
            _relative(root, path)
            for path in (root / "tests/harness/corpus/monty_compat").glob("*.py")
            if path.is_file()
        )
    if program == "differential":
        return sorted(
            _relative(root, path)
            for directory in (
                root / "tests/differential/basic",
                root / "tests/differential/stdlib",
            )
            for path in directory.rglob("*.py")
            if path.is_file()
        )
    test_root = root / "third_party/cpython/Lib/test"
    names = {
        entry.stem
        for entry in test_root.iterdir()
        if entry.is_file() and entry.suffix == ".py" and entry.name.startswith("test_")
    }
    names.update(
        entry.name
        for entry in test_root.iterdir()
        if entry.is_dir()
        and entry.name.startswith("test_")
        and (entry / "__main__.py").is_file()
    )
    return sorted(names)


def lpt_shards(
    entries: Sequence[Mapping[str, Any]], count: int
) -> list[dict[str, Any]]:
    """Partition entries using deterministic longest-processing-time first."""

    if count <= 0:
        raise ValueError("shard count must be positive")
    if len(entries) < count:
        raise ValueError(
            f"corpus has {len(entries)} entries but requires {count} nonempty shards"
        )
    bins: list[list[Mapping[str, Any]]] = [[] for _ in range(count)]
    weights = [0] * count
    for entry in sorted(
        entries, key=lambda row: (-int(row["weight"]), str(row["path"]))
    ):
        shard_id = min(range(count), key=lambda index: (weights[index], index))
        bins[shard_id].append(entry)
        weights[shard_id] += int(entry["weight"])
    return [
        {
            "id": shard_id,
            "weight": weights[shard_id],
            "entries": sorted(str(entry["path"]) for entry in bins[shard_id]),
        }
        for shard_id in range(count)
    ]


def _authority_inputs(root: Path) -> list[dict[str, str]]:
    inputs = []
    for relative in AUTHORITY_INPUTS:
        path = root / relative
        if not path.is_file():
            raise ValueError(f"nightly shard authority input is missing: {relative}")
        inputs.append({"path": relative, "sha256": _file_digest(path)})
    return inputs


def _measurement_contract_digest(inputs: Sequence[Mapping[str, str]]) -> str:
    measured_inputs = [
        dict(row)
        for row in inputs
        if row["path"] != "config/nightly_shard_profile.json"
    ]
    return _json_digest({"policy": WEIGHT_POLICY, "inputs": measured_inputs})


def _authority_payload(
    inputs: Sequence[Mapping[str, str]],
    profile_summary: Mapping[str, Any],
    measurement_contract_sha256: str,
) -> dict[str, Any]:
    return {
        "policy": WEIGHT_POLICY,
        "inputs": list(inputs),
        "measurement_contract_sha256": measurement_contract_sha256,
        "weight_profile": profile_summary,
    }


def load_weight_profile(root: Path) -> dict[str, Any]:
    path = root / "config" / "nightly_shard_profile.json"
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError("nightly shard profile must be a JSON object")
    nightly_shard_profile.validate_profile(payload, tuple(SHARD_COUNTS))
    return payload


def build_plan(
    root: Path = ROOT,
    *,
    source_commit: str | None = None,
    cpython_commit: str | None = None,
    runtime_artifact_manifest: Path | None = None,
) -> dict[str, Any]:
    root = root.resolve()
    source_commit = (source_commit or _git_commit(root)).lower()
    if len(source_commit) != 40 or any(
        char not in "0123456789abcdef" for char in source_commit
    ):
        raise ValueError("source commit must be a full commit SHA")
    cpython_root = root / "third_party" / "cpython"
    cpython_commit = (cpython_commit or _git_commit(cpython_root)).lower()
    if cpython_commit != _pinned_cpython_commit(root):
        raise ValueError("CPython checkout does not match the pinned 3.12 revision")
    corpora = discover_corpora(root)
    authority_inputs = _authority_inputs(root)
    measurement_contract_sha256 = _measurement_contract_digest(authority_inputs)
    profile_summary = nightly_shard_profile.apply_profile(
        corpora,
        load_weight_profile(root),
        measurement_contract_sha256=measurement_contract_sha256,
    )
    authority = _authority_payload(
        authority_inputs, profile_summary, measurement_contract_sha256
    )
    runtime_manifest = None
    if runtime_artifact_manifest is not None:
        manifest_path = runtime_artifact_manifest.resolve()
        runtime_manifest = {
            "path": _relative(root, manifest_path),
            "sha256": _file_digest(manifest_path),
        }
    programs: dict[str, Any] = {}
    for name, entries in corpora.items():
        programs[name] = {
            "selected": len(entries),
            "total_weight": sum(int(entry["weight"]) for entry in entries),
            "entries": entries,
            "shards": lpt_shards(entries, SHARD_COUNTS[name]),
        }
    payload: dict[str, Any] = {
        "schema": PLAN_SCHEMA,
        "source_commit": source_commit,
        "cpython_commit": cpython_commit,
        "authority": authority,
        "authority_sha256": _json_digest(authority),
        "runtime_artifact_manifest": runtime_manifest,
        "programs": programs,
    }
    payload["plan_sha256"] = _json_digest(payload)
    return payload


def _expected_plan_digest(plan: Mapping[str, Any]) -> str:
    unsigned = dict(plan)
    unsigned.pop("plan_sha256", None)
    return _json_digest(unsigned)


def validate_plan_envelope(plan: Mapping[str, Any], root: Path = ROOT) -> None:
    """Validate a transported plan without requiring its external corpus tree."""

    if plan.get("schema") != PLAN_SCHEMA:
        raise ValueError("nightly shard plan schema is invalid")
    if plan.get("plan_sha256") != _expected_plan_digest(plan):
        raise ValueError("nightly shard plan digest mismatch")
    if plan.get("authority_sha256") != _json_digest(plan.get("authority")):
        raise ValueError("nightly shard authority digest mismatch")
    authority = plan.get("authority")
    if not isinstance(authority, dict) or authority.get("policy") != WEIGHT_POLICY:
        raise ValueError("nightly shard policy authority mismatch")
    authority_inputs = authority.get("inputs")
    current_inputs = _authority_inputs(root)
    if authority_inputs != current_inputs:
        raise ValueError("nightly shard authority inputs drift")
    contract_digest = _measurement_contract_digest(current_inputs)
    if authority.get("measurement_contract_sha256") != contract_digest:
        raise ValueError("nightly shard measurement contract mismatch")
    current_profile = load_weight_profile(root)
    weight_profile = authority.get("weight_profile")
    if not isinstance(weight_profile, dict) or weight_profile.get(
        "profile_sha256"
    ) != nightly_shard_profile.profile_digest(current_profile):
        raise ValueError("nightly shard weight profile authority mismatch")
    if plan.get("cpython_commit") != _pinned_cpython_commit(root):
        raise ValueError("nightly shard plan CPython commit is not pinned authority")
    source_commit = plan.get("source_commit")
    if (
        not isinstance(source_commit, str)
        or len(source_commit) != 40
        or any(character not in "0123456789abcdef" for character in source_commit)
    ):
        raise ValueError("nightly shard source commit is invalid")
    programs = plan.get("programs")
    if not isinstance(programs, dict) or set(programs) != set(SHARD_COUNTS):
        raise ValueError("nightly shard plan program closure mismatch")
    for name, program in programs.items():
        if not isinstance(program, dict):
            raise ValueError(f"{name} program plan is invalid")
        entries = program.get("entries")
        shards = program.get("shards")
        if not isinstance(entries, list) or not isinstance(shards, list):
            raise ValueError(f"{name} program entries or shards are invalid")
        paths = []
        for entry in entries:
            if not isinstance(entry, dict) or not isinstance(entry.get("path"), str):
                raise ValueError(f"{name} corpus entry is invalid")
            path = str(entry["path"])
            digest = entry.get("sha256")
            if (
                not isinstance(digest, str)
                or len(digest) != 64
                or any(character not in "0123456789abcdef" for character in digest)
            ):
                raise ValueError(f"{name}:{path} source digest is invalid")
            for field in ("source_bytes", "weight"):
                value = entry.get(field)
                if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
                    raise ValueError(f"{name}:{path} {field} is invalid")
            if name == "regrtest" and not isinstance(entry.get("sources"), list):
                raise ValueError(f"{name}:{path} source custody is missing")
            paths.append(path)
        if paths != sorted(paths) or len(paths) != len(set(paths)) or not paths:
            raise ValueError(f"{name} corpus paths are not exact and ordered")
        if program.get("selected") != len(entries):
            raise ValueError(f"{name} selected count mismatch")
        if program.get("total_weight") != sum(int(row["weight"]) for row in entries):
            raise ValueError(f"{name} total weight mismatch")
        if shards != lpt_shards(entries, SHARD_COUNTS[name]):
            raise ValueError(f"{name} LPT shard projection mismatch")


def validate_plan(
    plan: Mapping[str, Any],
    root: Path = ROOT,
    *,
    expected_source_commit: str | None = None,
    expected_cpython_commit: str | None = None,
) -> None:
    validate_plan_envelope(plan, root)
    authority = plan["authority"]
    current_profile = load_weight_profile(root)
    expected_corpora = discover_corpora(root)
    expected_profile_summary = nightly_shard_profile.apply_profile(
        expected_corpora,
        current_profile,
        measurement_contract_sha256=authority["measurement_contract_sha256"],
    )
    if authority.get("weight_profile") != expected_profile_summary:
        raise ValueError("nightly shard weight profile authority mismatch")
    if (
        expected_source_commit is not None
        and plan.get("source_commit") != expected_source_commit
    ):
        raise ValueError("nightly shard plan source commit mismatch")
    if (
        expected_cpython_commit is not None
        and plan.get("cpython_commit") != expected_cpython_commit
    ):
        raise ValueError("nightly shard plan CPython commit mismatch")
    runtime_manifest = plan.get("runtime_artifact_manifest")
    if runtime_manifest is not None:
        if not isinstance(runtime_manifest, dict):
            raise ValueError("runtime artifact manifest identity is invalid")
        path = root / str(runtime_manifest.get("path", ""))
        if not path.is_file() or _file_digest(path) != runtime_manifest.get("sha256"):
            raise ValueError("runtime artifact manifest digest mismatch")
    programs = plan["programs"]
    for name, program in programs.items():
        entries = program["entries"]
        paths: list[str] = []
        for entry in entries:
            path = str(entry["path"])
            if name == "regrtest":
                sources = entry.get("sources")
                if not isinstance(sources, list) or not sources:
                    raise ValueError(f"{name}:{path} source custody is missing")
                source_rows = []
                for source in sources:
                    source_path = root / str(source.get("path", ""))
                    if (
                        not source_path.is_file()
                        or _file_digest(source_path) != source.get("sha256")
                        or source_path.stat().st_size != source.get("size")
                    ):
                        raise ValueError(f"{name}:{path} source digest mismatch")
                    source_rows.append(source)
                if _json_digest(source_rows) != entry.get("sha256"):
                    raise ValueError(f"{name}:{path} module digest mismatch")
                current_sources = {
                    _relative(root, source)
                    for source in _regrtest_module_sources(
                        root / "third_party/cpython/Lib/test", path
                    )
                }
                if current_sources != {str(row["path"]) for row in source_rows}:
                    raise ValueError(f"{name}:{path} module source closure mismatch")
            else:
                source_path = root / path
                if not source_path.is_file() or _file_digest(source_path) != entry.get(
                    "sha256"
                ):
                    raise ValueError(f"{name}:{path} source digest mismatch")
            paths.append(path)
        if paths != _current_corpus_paths(root, name):
            raise ValueError(f"{name} corpus discovery closure mismatch")
        if entries != expected_corpora[name]:
            raise ValueError(f"{name} measured weight projection mismatch")


def _load_plan(path: Path, root: Path = ROOT) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError("nightly shard plan must be a JSON object")
    validate_plan(
        payload,
        root,
        expected_source_commit=_git_commit(root),
        expected_cpython_commit=_git_commit(root / "third_party" / "cpython"),
    )
    return payload


def shard_entries(plan: Mapping[str, Any], program: str, shard_id: int) -> list[str]:
    try:
        shards = plan["programs"][program]["shards"]
    except (KeyError, TypeError) as exc:
        raise ValueError(f"unknown nightly program: {program}") from exc
    for shard in shards:
        if shard.get("id") == shard_id:
            entries = shard.get("entries")
            if not isinstance(entries, list) or not entries:
                raise ValueError(f"{program} shard {shard_id} is empty")
            return [str(path) for path in entries]
    raise ValueError(f"unknown {program} shard: {shard_id}")


def build_shard_command(
    root: Path,
    program: str,
    selection_file: Path,
    summary_file: Path,
    artifact_root: Path | None = None,
) -> list[str]:
    """Return the canonical exact-file-list harness command for one shard."""

    python = sys.executable
    if program == "conformance":
        return [
            python,
            "tests/harness/run_molt_conformance.py",
            "--suite",
            "full",
            "--files-from",
            str(selection_file),
            "--json-out",
            str(summary_file),
        ]
    if program == "differential":
        return [
            python,
            "tests/molt_diff.py",
            "--build-profile",
            "dev",
            "--files-from",
            str(selection_file),
            "--json-output",
            str(summary_file),
        ]
    if program == "regrtest":
        if artifact_root is None:
            raise ValueError("regrtest shard requires an artifact root")
        return [
            python,
            "tools/cpython_regrtest.py",
            "--cpython-dir",
            "third_party/cpython",
            "--uv",
            "--uv-python",
            "3.12",
            "--timeout",
            "600",
            "--no-diff",
            "--tests-from",
            str(selection_file),
            "--output-dir",
            str(artifact_root),
        ]
    raise ValueError(f"unknown nightly program: {program}")


def _counts(
    program: str, summary: Mapping[str, Any], returncode: int, selected: int
) -> dict[str, int]:
    if program == "conformance":
        values = {
            "selected": summary.get("total", 0),
            "passed": summary.get("passed", 0),
            "failed": summary.get("failed", 0),
            "errors": int(summary.get("compile_error", 0))
            + int(summary.get("timeout", 0)),
            "skipped": summary.get("skipped", 0),
        }
    elif program == "differential":
        failed = summary.get("failed", 0)
        values = {
            "selected": summary.get("total", 0),
            "passed": summary.get("passed", 0),
            "failed": failed,
            "errors": summary.get("errors", summary.get("oom", 0)),
            "skipped": summary.get("skipped", 0),
        }
    else:
        values = {
            "selected": selected,
            "passed": selected if returncode == 0 else 0,
            "failed": 0 if returncode == 0 else selected,
            "errors": 0,
            "skipped": 0,
        }
    normalized: dict[str, int] = {}
    for name, value in values.items():
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            raise ValueError(f"child summary {name} must be a nonnegative integer")
        normalized[name] = value
    return normalized


def _item_results(
    summary: Mapping[str, Any], expected_entries: Sequence[str]
) -> list[dict[str, Any]]:
    rows = summary.get("item_results")
    if not isinstance(rows, list):
        raise ValueError("child summary item_results must be a list")
    by_path: dict[str, dict[str, Any]] = {}
    status_aliases = {
        "pass": "passed",
        "passed": "passed",
        "fail": "failed",
        "failed": "failed",
        "error": "errors",
        "errors": "errors",
        "oom": "errors",
        "timeout": "errors",
        "compile_error": "errors",
        "skip": "skipped",
        "skipped": "skipped",
    }
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("path"), str):
            raise ValueError("child summary item result row is invalid")
        path = str(row["path"]).replace("\\", "/")
        status = status_aliases.get(str(row.get("status", "")).lower())
        duration = row.get("duration_s")
        if path in by_path:
            raise ValueError(f"child summary duplicates item result: {path}")
        if status is None:
            raise ValueError(f"child summary has invalid item status: {path}")
        if (
            isinstance(duration, bool)
            or not isinstance(duration, (int, float))
            or duration <= 0
        ):
            raise ValueError(f"child summary has invalid item duration: {path}")
        by_path[path] = {
            "path": path,
            "status": status,
            "duration_s": float(duration),
        }
    if set(by_path) != set(expected_entries):
        missing = sorted(set(expected_entries).difference(by_path))[:3]
        extra = sorted(set(by_path).difference(expected_entries))[:3]
        raise ValueError(
            f"child summary item result closure mismatch: missing={missing} extra={extra}"
        )
    return [by_path[path] for path in expected_entries]


def _is_reparse(info: os.stat_result) -> bool:
    return bool(getattr(info, "st_file_attributes", 0) & FILE_ATTRIBUTE_REPARSE_POINT)


def _artifact_files(root: Path) -> list[dict[str, Any]]:
    if not root.is_dir() or root.is_symlink() or _is_reparse(root.lstat()):
        raise ValueError("regrtest artifact root is missing or unsafe")
    rows: list[dict[str, Any]] = []
    for path in sorted(root.rglob("*")):
        info = path.lstat()
        relative = path.relative_to(root).as_posix()
        if stat.S_ISLNK(info.st_mode) or _is_reparse(info):
            raise ValueError(f"artifact entry is a link or reparse point: {relative}")
        if stat.S_ISDIR(info.st_mode):
            continue
        if not stat.S_ISREG(info.st_mode):
            raise ValueError(f"artifact entry is not a regular file: {relative}")
        rows.append(
            {"path": relative, "size": info.st_size, "sha256": _file_digest(path)}
        )
    if not rows:
        raise ValueError("regrtest artifact root is empty")
    return rows


def _write_artifact_archive(root: Path, output: Path) -> list[dict[str, Any]]:
    rows = _artifact_files(root)
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as archive:
        for row in rows:
            relative = str(row["path"])
            info = zipfile.ZipInfo(relative, date_time=(1980, 1, 1, 0, 0, 0))
            info.external_attr = 0o100644 << 16
            archive.writestr(info, (root / relative).read_bytes())
    return rows


def _validate_artifact_archive(path: Path, rows: Any) -> None:
    if not isinstance(rows, list) or not rows:
        raise ValueError("artifact file manifest is empty")
    expected: dict[str, Mapping[str, Any]] = {}
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("path"), str):
            raise ValueError("artifact file manifest row is invalid")
        relative = str(row["path"])
        member = PurePosixPath(relative)
        if member.is_absolute() or ".." in member.parts or ":" in member.parts[0]:
            raise ValueError(f"unsafe artifact member path: {relative}")
        if relative in expected:
            raise ValueError(f"duplicate artifact member path: {relative}")
        expected[relative] = row
    observed: set[str] = set()
    with zipfile.ZipFile(path) as archive:
        for info in archive.infolist():
            relative = info.filename.replace("\\", "/")
            mode = info.external_attr >> 16
            if (
                info.is_dir()
                or PurePosixPath(relative).is_absolute()
                or ".." in PurePosixPath(relative).parts
                or stat.S_ISLNK(mode)
                or not stat.S_ISREG(mode)
                or relative not in expected
                or relative in observed
            ):
                raise ValueError(f"unsafe artifact archive member: {relative}")
            data = archive.read(info)
            row = expected[relative]
            if len(data) != row.get("size") or _digest_bytes(data) != row.get("sha256"):
                raise ValueError(f"artifact member digest mismatch: {relative}")
            observed.add(relative)
    if observed != set(expected):
        raise ValueError("artifact archive file closure mismatch")


def run_shard(
    plan: Mapping[str, Any],
    *,
    root: Path,
    program: str,
    shard_id: int,
    raw_out: Path,
    checkpoint_out: Path,
    artifact_root: Path | None = None,
    artifact_out: Path | None = None,
    timeout: float = 3600,
    command: Sequence[str] | None = None,
) -> None:
    validate_plan(plan, root)
    if timeout <= 0:
        raise ValueError("nightly shard timeout must be positive")
    entries = shard_entries(plan, program, shard_id)
    raw_out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f"molt-nightly-{program}-") as temporary:
        temporary_root = Path(temporary)
        selection = temporary_root / "selection.txt"
        summary_path = temporary_root / "summary.json"
        selection.write_text("\n".join(entries) + "\n", encoding="utf-8")
        argv = (
            list(command)
            if command is not None
            else build_shard_command(
                root, program, selection, summary_path, artifact_root
            )
        )
        expanded: list[str] = []
        for part in argv:
            if part == "{entries}":
                expanded.extend(entries)
            else:
                expanded.append(
                    part.replace("{selection}", str(selection)).replace(
                        "{summary}", str(summary_path)
                    )
                )
        started = time.monotonic()
        try:
            completed = EXECUTOR.run(
                expanded,
                cwd=root,
                capture_output=True,
                capture_tail_bytes=16_000,
                text=True,
                timeout=timeout,
            )
            returncode = int(completed.returncode)
            stdout_tail = str(completed.stdout or "")[-16_000:]
            stderr_tail = str(completed.stderr or "")[-16_000:]
        except subprocess.TimeoutExpired as exc:
            returncode = 124
            stdout_tail = str(exc.stdout or "")[-16_000:]
            stderr_tail = str(exc.stderr or "")[-16_000:]
        duration = max(time.monotonic() - started, 1e-9)
        summary_error = None
        if program == "regrtest" and artifact_root is not None and command is None:
            summary_path = artifact_root / "summary.json"
        try:
            summary = json.loads(summary_path.read_text(encoding="utf-8"))
            if not isinstance(summary, dict):
                raise ValueError("child summary is not an object")
        except (OSError, ValueError, json.JSONDecodeError) as exc:
            summary = {}
            summary_error = str(exc)
        try:
            counts = _counts(program, summary, returncode, len(entries))
            items = _item_results(summary, entries)
            observed_statuses = {
                status: sum(row["status"] == status for row in items)
                for status in ("passed", "failed", "errors", "skipped")
            }
            if program == "regrtest":
                counts = {"selected": len(items), **observed_statuses}
            if any(
                counts[status] != value for status, value in observed_statuses.items()
            ):
                raise ValueError(
                    "child summary counts do not match item result statuses"
                )
        except (TypeError, ValueError) as exc:
            summary_error = str(exc)
            items = []
            counts = {
                "selected": len(entries),
                "passed": 0,
                "failed": 0,
                "errors": len(entries),
                "skipped": 0,
            }
        if summary_error is not None and counts["errors"] == 0:
            counts = {
                **counts,
                "passed": 0,
                "failed": 0,
                "errors": len(entries),
                "skipped": 0,
            }
        artifact = None
        staged_artifact = None
        if program == "regrtest":
            if artifact_root is None or artifact_out is None:
                raise ValueError("regrtest shard requires artifact root and output")
            try:
                artifact_out.resolve().relative_to(artifact_root.resolve())
            except ValueError:
                pass
            else:
                raise ValueError(
                    "regrtest artifact archive must be outside its source tree"
                )
            staged_artifact = staged_output_path(artifact_out)
            files = _write_artifact_archive(artifact_root, staged_artifact)
            artifact = {
                "sha256": _file_digest(staged_artifact),
                "files": files,
            }
        elif artifact_root is not None or artifact_out is not None:
            raise ValueError("only regrtest shards may publish artifact custody")
        shard = plan["programs"][program]["shards"][shard_id]
        raw = {
            "schema": EVIDENCE_SCHEMA,
            "kind": "shard-raw",
            "program": program,
            "shard": shard_id,
            "plan_sha256": plan["plan_sha256"],
            "source_commit": plan["source_commit"],
            "authority_sha256": plan["authority_sha256"],
            "entries": entries,
            "entries_sha256": _json_digest(entries),
            "weight": shard["weight"],
            "returncode": returncode,
            "duration_s": duration,
            **counts,
            "summary_error": summary_error,
            "item_results": items,
            "child_summary": summary,
            "stdout_tail": stdout_tail,
            "stderr_tail": stderr_tail,
            "artifact": artifact,
        }
        raw_bytes = _canonical_bytes(raw) + b"\n"
        checkpoint = {
            "schema": EVIDENCE_SCHEMA,
            "kind": "shard-checkpoint",
            "program": program,
            "shard": shard_id,
            "plan_sha256": plan["plan_sha256"],
            "raw_sha256": _digest_bytes(raw_bytes),
            "entries_sha256": raw["entries_sha256"],
            "artifact_sha256": artifact["sha256"] if artifact else None,
            "returncode": returncode,
            **counts,
        }
        raw_stage = staged_output_path(raw_out)
        checkpoint_stage = staged_output_path(checkpoint_out)
        raw_stage.write_bytes(raw_bytes)
        checkpoint_stage.write_bytes(_canonical_bytes(checkpoint) + b"\n")
        publications = [(raw_stage, raw_out), (checkpoint_stage, checkpoint_out)]
        if staged_artifact is not None and artifact_out is not None:
            publications.insert(0, (staged_artifact, artifact_out))
        publish_validated_outputs(publications)


def aggregate(
    plan: Mapping[str, Any],
    *,
    root: Path,
    program: str,
    evidence_root: Path,
) -> dict[str, Any]:
    validate_plan(plan, root)
    projected = plan["programs"][program]
    expected_paths = {str(entry["path"]) for entry in projected["entries"]}
    totals = {name: 0 for name in ("selected", "passed", "failed", "errors", "skipped")}
    observed_paths: set[str] = set()
    observed_items: dict[str, dict[str, Any]] = {}
    integrity_errors: list[str] = []
    records: list[dict[str, Any]] = []
    expected_evidence_names = {
        f"shard-{int(shard['id']):02d}.{suffix}"
        for shard in projected["shards"]
        for suffix in (
            "raw.json",
            "checkpoint.json",
            *(("artifacts.zip",) if program == "regrtest" else ()),
        )
    }
    observed_evidence_names = {
        path.name
        for pattern in (
            "shard-*.raw.json",
            "shard-*.checkpoint.json",
            "shard-*.artifacts.zip",
        )
        for path in evidence_root.glob(pattern)
        if path.is_file()
    }
    unexpected_evidence = sorted(observed_evidence_names - expected_evidence_names)
    if unexpected_evidence:
        integrity_errors.append(
            f"unexpected shard evidence files: {unexpected_evidence[:3]}"
        )
    for shard in projected["shards"]:
        shard_id = int(shard["id"])
        raw_path = evidence_root / f"shard-{shard_id:02d}.raw.json"
        checkpoint_path = evidence_root / f"shard-{shard_id:02d}.checkpoint.json"
        artifact_path = evidence_root / f"shard-{shard_id:02d}.artifacts.zip"
        try:
            raw_bytes = raw_path.read_bytes()
            raw = json.loads(raw_bytes)
            checkpoint = json.loads(checkpoint_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            integrity_errors.append(f"shard {shard_id}: unreadable evidence: {exc}")
            continue
        expected_entries = [str(path) for path in shard["entries"]]
        contracts = (
            (raw, "schema", EVIDENCE_SCHEMA),
            (raw, "kind", "shard-raw"),
            (raw, "program", program),
            (raw, "shard", shard_id),
            (raw, "plan_sha256", plan["plan_sha256"]),
            (raw, "source_commit", plan["source_commit"]),
            (raw, "authority_sha256", plan["authority_sha256"]),
            (raw, "entries", expected_entries),
            (raw, "entries_sha256", _json_digest(expected_entries)),
            (raw, "weight", shard["weight"]),
            (checkpoint, "schema", EVIDENCE_SCHEMA),
            (checkpoint, "kind", "shard-checkpoint"),
            (checkpoint, "program", program),
            (checkpoint, "shard", shard_id),
            (checkpoint, "plan_sha256", plan["plan_sha256"]),
            (checkpoint, "raw_sha256", _digest_bytes(raw_bytes)),
            (checkpoint, "entries_sha256", _json_digest(expected_entries)),
        )
        for record, field, expected in contracts:
            if record.get(field) != expected:
                integrity_errors.append(f"shard {shard_id}: {field} mismatch")
        overlap = observed_paths.intersection(expected_entries)
        if overlap:
            integrity_errors.append(
                f"shard {shard_id}: duplicate corpus paths {sorted(overlap)[:3]}"
            )
        observed_paths.update(expected_entries)
        for name in totals:
            value = raw.get(name)
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                integrity_errors.append(f"shard {shard_id}: invalid {name}")
                value = 0
            if checkpoint.get(name) != value:
                integrity_errors.append(f"shard {shard_id}: checkpoint {name} mismatch")
            totals[name] += value
        if raw.get("selected") != len(expected_entries):
            integrity_errors.append(f"shard {shard_id}: selected count mismatch")
        if sum(
            int(raw.get(name, 0)) for name in ("passed", "failed", "errors", "skipped")
        ) != raw.get("selected"):
            integrity_errors.append(f"shard {shard_id}: result accounting mismatch")
        if checkpoint.get("returncode") != raw.get("returncode"):
            integrity_errors.append(f"shard {shard_id}: returncode mismatch")
        item_rows = raw.get("item_results")
        if not isinstance(item_rows, list):
            integrity_errors.append(f"shard {shard_id}: item telemetry is not a list")
            item_rows = []
        shard_items: dict[str, dict[str, Any]] = {}
        for item in item_rows:
            if not isinstance(item, dict) or not isinstance(item.get("path"), str):
                integrity_errors.append(f"shard {shard_id}: invalid item telemetry row")
                continue
            path = str(item["path"])
            status = item.get("status")
            duration = item.get("duration_s")
            if path in shard_items or path in observed_items:
                integrity_errors.append(
                    f"shard {shard_id}: duplicate item telemetry for {path}"
                )
                continue
            if status not in {"passed", "failed", "errors", "skipped"}:
                integrity_errors.append(
                    f"shard {shard_id}: invalid item status for {path}"
                )
                continue
            if (
                isinstance(duration, bool)
                or not isinstance(duration, (int, float))
                or duration <= 0
            ):
                integrity_errors.append(
                    f"shard {shard_id}: invalid item duration for {path}"
                )
                continue
            shard_items[path] = {
                "status": status,
                "duration_s": float(duration),
            }
        if set(shard_items) != set(expected_entries):
            integrity_errors.append(
                f"shard {shard_id}: item telemetry closure mismatch"
            )
        for status in ("passed", "failed", "errors", "skipped"):
            if sum(
                item["status"] == status for item in shard_items.values()
            ) != raw.get(status):
                integrity_errors.append(
                    f"shard {shard_id}: item {status} count mismatch"
                )
        observed_items.update(shard_items)
        artifact = raw.get("artifact")
        if program == "regrtest":
            if not isinstance(artifact, dict) or not artifact_path.is_file():
                integrity_errors.append(f"shard {shard_id}: regrtest artifact missing")
            else:
                digest = _file_digest(artifact_path)
                if (
                    artifact.get("sha256") != digest
                    or checkpoint.get("artifact_sha256") != digest
                ):
                    integrity_errors.append(
                        f"shard {shard_id}: artifact digest mismatch"
                    )
                try:
                    _validate_artifact_archive(artifact_path, artifact.get("files"))
                except (OSError, ValueError, zipfile.BadZipFile) as exc:
                    integrity_errors.append(
                        f"shard {shard_id}: artifact custody invalid: {exc}"
                    )
        elif artifact is not None or checkpoint.get("artifact_sha256") is not None:
            integrity_errors.append(f"shard {shard_id}: unexpected artifact custody")
        shard_duration = raw.get("duration_s")
        if (
            isinstance(shard_duration, bool)
            or not isinstance(shard_duration, (int, float))
            or not math.isfinite(float(shard_duration))
            or float(shard_duration) <= 0
        ):
            integrity_errors.append(f"shard {shard_id}: invalid wall duration")
            shard_duration = 0.0
        records.append(
            {
                "duration_s": float(shard_duration),
                "id": shard_id,
                "planned_weight": shard["weight"],
                "raw_sha256": _digest_bytes(raw_bytes),
                "returncode": raw.get("returncode"),
            }
        )
    if observed_paths != expected_paths:
        integrity_errors.append("aggregate corpus closure mismatch")
    if set(observed_items) != expected_paths:
        integrity_errors.append("aggregate item telemetry closure mismatch")
    ok = (
        not integrity_errors
        and totals["selected"] > 0
        and totals["selected"] == int(projected["selected"])
        and totals["passed"] == totals["selected"]
        and totals["failed"] == totals["errors"] == totals["skipped"] == 0
        and len(records) == SHARD_COUNTS[program]
        and all(record["returncode"] == 0 for record in records)
    )
    return {
        "schema": AGGREGATE_SCHEMA,
        "kind": "aggregate",
        "program": program,
        "ok": ok,
        "plan_sha256": plan["plan_sha256"],
        "source_commit": plan["source_commit"],
        "authority_sha256": plan["authority_sha256"],
        "expected_selected": projected["selected"],
        **totals,
        "integrity_errors": integrity_errors,
        "item_durations_s": {
            path: observed_items[path]["duration_s"] for path in sorted(observed_items)
        },
        "status_by_path": {
            path: observed_items[path]["status"] for path in sorted(observed_items)
        },
        "shards": records,
    }


def validate_aggregate(plan: Mapping[str, Any], payload: Mapping[str, Any]) -> None:
    program = payload.get("program")
    if program not in SHARD_COUNTS:
        raise ValueError("nightly aggregate program is invalid")
    expected = plan["programs"][program]
    expected_paths = {str(entry["path"]) for entry in expected["entries"]}
    durations = payload.get("item_durations_s")
    statuses = payload.get("status_by_path")
    if (
        payload.get("schema") != AGGREGATE_SCHEMA
        or payload.get("kind") != "aggregate"
        or payload.get("plan_sha256") != plan.get("plan_sha256")
        or payload.get("source_commit") != plan.get("source_commit")
        or payload.get("authority_sha256") != plan.get("authority_sha256")
        or payload.get("expected_selected") != expected.get("selected")
        or payload.get("selected") != expected.get("selected")
        or not isinstance(payload.get("shards"), list)
        or len(payload["shards"]) != SHARD_COUNTS[program]
        or not isinstance(durations, dict)
        or set(durations) != expected_paths
        or not isinstance(statuses, dict)
        or set(statuses) != expected_paths
    ):
        raise ValueError("nightly aggregate contract is invalid")
    for path in expected_paths:
        duration = durations[path]
        if (
            isinstance(duration, bool)
            or not isinstance(duration, (int, float))
            or not math.isfinite(float(duration))
            or float(duration) <= 0
            or statuses[path] not in {"passed", "failed", "errors", "skipped"}
        ):
            raise ValueError(f"nightly aggregate item telemetry is invalid: {path}")
    if payload.get("ok") is not True:
        raise RuntimeError(
            f"nightly {program} failed: failures={payload.get('failed')} "
            f"errors={payload.get('errors')} integrity={payload.get('integrity_errors')}"
        )
    for expected_id, shard in enumerate(payload["shards"]):
        if (
            not isinstance(shard, dict)
            or shard.get("id") != expected_id
            or shard.get("returncode") != 0
            or not isinstance(shard.get("raw_sha256"), str)
            or len(shard["raw_sha256"]) != 64
            or any(
                character not in "0123456789abcdef" for character in shard["raw_sha256"]
            )
            or shard.get("planned_weight") != expected["shards"][expected_id]["weight"]
            or isinstance(shard.get("duration_s"), bool)
            or not isinstance(shard.get("duration_s"), (int, float))
            or not math.isfinite(float(shard["duration_s"]))
            or float(shard["duration_s"]) <= 0
        ):
            raise ValueError("nightly aggregate shard telemetry is invalid")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)
    plan = commands.add_parser("plan")
    plan.add_argument("--out", type=Path, required=True)
    plan.add_argument("--runtime-artifact-manifest", type=Path)
    run = commands.add_parser("run-shard")
    run.add_argument("--plan", type=Path, required=True)
    run.add_argument("--program", choices=tuple(SHARD_COUNTS), required=True)
    run.add_argument("--shard", type=int, required=True)
    run.add_argument("--raw-out", type=Path, required=True)
    run.add_argument("--checkpoint-out", type=Path, required=True)
    run.add_argument("--artifact-root", type=Path)
    run.add_argument("--artifact-out", type=Path)
    run.add_argument("--timeout", type=float, default=3600)
    run.add_argument("argv", nargs=argparse.REMAINDER)
    collect = commands.add_parser("aggregate")
    collect.add_argument("--plan", type=Path, required=True)
    collect.add_argument("--program", choices=tuple(SHARD_COUNTS), required=True)
    collect.add_argument("--evidence-root", type=Path, required=True)
    collect.add_argument("--out", type=Path, required=True)
    check = commands.add_parser("validate")
    check.add_argument("--plan", type=Path, required=True)
    check.add_argument("--aggregate", type=Path, required=True)
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "plan":
            payload = build_plan(
                runtime_artifact_manifest=args.runtime_artifact_manifest,
            )
            atomic_write_json(args.out, payload, sort_keys=True)
        elif args.command == "run-shard":
            command = list(args.argv)
            if command and command[0] == "--":
                command = command[1:]
            run_shard(
                _load_plan(args.plan),
                root=ROOT,
                program=args.program,
                shard_id=args.shard,
                raw_out=args.raw_out,
                checkpoint_out=args.checkpoint_out,
                artifact_root=args.artifact_root,
                artifact_out=args.artifact_out,
                timeout=args.timeout,
                command=command or None,
            )
        elif args.command == "aggregate":
            plan_payload = _load_plan(args.plan)
            payload = aggregate(
                plan_payload,
                root=ROOT,
                program=args.program,
                evidence_root=args.evidence_root,
            )
            atomic_write_json(args.out, payload, sort_keys=True)
            validate_aggregate(plan_payload, payload)
        else:
            plan = _load_plan(args.plan)
            payload = json.loads(args.aggregate.read_text(encoding="utf-8"))
            validate_aggregate(plan, payload)
    except (
        OSError,
        ValueError,
        RuntimeError,
        json.JSONDecodeError,
        zipfile.BadZipFile,
    ) as exc:
        print(f"nightly-sharding: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
