"""Full release coverage projection; the daily scoreboard remains a separate gate.

This module grants no execution authority. It joins exact, independently validated
semantic receipts and canonical performance cells to a source-bound required set.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import hashlib
import json
import subprocess
from pathlib import Path

from molt.verified_subset import (
    capture_verified_subset_policy,
    verified_subset_coordinates,
)
from molt.release_lanes import SCHEMA, capture_release_lanes
from molt.release_matrix import RELEASE_TARGETS
from molt.toolchain_identity import (
    capture_stable_regular_file,
    verify_stable_regular_file_identity,
)
from tools import bench_suites

SHARD_SCHEMA = "molt.release-performance-shard.v2"
_ROOT = Path(__file__).resolve().parents[1]


def _digest(value: object) -> str:
    return hashlib.sha256(
        json.dumps(
            value, sort_keys=True, separators=(",", ":"), allow_nan=False
        ).encode()
    ).hexdigest()


@dataclass(frozen=True)
class ReleaseMatrix:
    source_sha: str
    authority_sha256: str
    semantic_ids: tuple[str, ...]
    performance_cells: tuple[dict[str, str], ...]
    blockers: tuple[str, ...]

    @property
    def identity(self) -> str:
        return _digest(self.as_record())

    def as_record(self) -> dict[str, object]:
        return {
            "schema": SCHEMA,
            "source_sha": self.source_sha,
            "authority_sha256": self.authority_sha256,
            "semantic_ids": list(self.semantic_ids),
            "performance_cells": list(self.performance_cells),
            "blockers": list(self.blockers),
        }


def required_matrix(*, source_sha: str, root: Path = _ROOT) -> ReleaseMatrix:
    """Project existing authorities without letting receipt authors select coverage."""
    if len(source_sha) != 40 or any(c not in "0123456789abcdef" for c in source_sha):
        raise ValueError("release matrix requires an exact Git source SHA")
    root = root.resolve(strict=True)
    inventory = capture_release_lanes(root)
    config = inventory.configuration
    policy = inventory.semantic_policy
    lanes = inventory.lanes
    semantic = tuple(sorted(c.id for c in verified_subset_coordinates(policy)))
    # Primary runnable suites, not the smoke alias. Typed exclusions remain
    # visible below until an appropriate measurement authority is implemented.
    benchmarks = tuple(
        sorted(
            (
                *bench_suites.BENCHMARKS,
                *bench_suites.WS_BENCHMARKS,
                *bench_suites.DYNAMIC_BUILTIN_SLICES,
            )
        )
    )
    if len(set(benchmarks)) != len(benchmarks):
        raise ValueError("canonical primary benchmark suites overlap")
    discovered = {
        path.relative_to(root).as_posix()
        for path in (root / "tests/benchmarks").rglob("bench_*.py")
        if path.is_file()
    }
    if discovered != set(benchmarks).union(bench_suites.EXCLUDED_BENCHMARKS):
        raise ValueError(
            "canonical benchmark ownership is incomplete or narrows source inventory"
        )
    cells = []
    for target in RELEASE_TARGETS:
        for minor, reference in zip(
            policy.python_versions, policy.reference_cpython, strict=True
        ):
            for lane in lanes:
                for benchmark in benchmarks:
                    coordinate = {
                        "python": minor,
                        "reference_python": reference,
                        "platform": target["platform"],
                        "arch": target["arch"],
                        "rust_target": target["rust_target"],
                        **lane.as_record(),
                        "benchmark": benchmark,
                    }
                    coordinate["id"] = "perf:" + ":".join(
                        coordinate[k]
                        for k in (
                            "python",
                            "platform",
                            "arch",
                            "backend",
                            "guest_profile",
                            "runtime_profile",
                            "compiler_profile",
                            "benchmark",
                        )
                    )
                    cells.append(coordinate)
    authorities = {
        "config/release_acceptance_matrix.toml",
        "config/verified_subset.toml",
        "config/release_targets.toml",
        "Cargo.toml",
        "tools/bench_suites.py",
        "tools/PERF_AUTHORITY.md",
        "src/molt/verified_subset.py",
        "src/molt/release_matrix.py",
        "src/molt/release_lanes.py",
        "tools/release_matrix_acceptance.py",
        "tools/perf_authority.py",
        "tools/perf_scoreboard_build_profiles.py",
        "src/molt/cli/build_results.py",
        "src/molt/cli/backend_output_pipeline.py",
        "src/molt/metric_ratios.py",
        "tools/perf_schema.py",
        *benchmarks,
        *bench_suites.EXCLUDED_BENCHMARKS,
    }
    blockers = [
        f"benchmark {p}: {e.kind}; {e.reason}"
        for p, e in sorted(bench_suites.EXCLUDED_BENCHMARKS.items())
    ]
    semantic_backends = config["required_semantic_backends"]
    if semantic_backends != ["llvm", "native", "wasm"]:
        raise ValueError(
            "release matrix cannot narrow advertised semantic backend coverage"
        )
    for backend in semantic_backends:
        if backend not in policy.backends:
            blockers.append(
                f"semantic backend {backend}: canonical E3 coordinate/receipt producer unavailable"
            )
    for exclusion in config["excluded_backend"]:
        if (
            not isinstance(exclusion, dict)
            or set(exclusion) != {"backend", "reason", "authority"}
            or any(not isinstance(v, str) or not v for v in exclusion.values())
        ):
            raise ValueError("backend applicability exclusion is not exact")
        if exclusion["backend"] in {lane.backend for lane in lanes}:
            raise ValueError("backend exclusion conflicts with required coverage")
        authorities.add(exclusion["authority"])
    for lane in config["lane"]:
        authorities.add(lane["authority"])
    for metric in config["required_metric"]:
        if (
            not isinstance(metric, dict)
            or set(metric) != {"name", "authority", "status"}
            or any(not isinstance(value, str) or not value for value in metric.values())
            or metric["status"] != "unmeasured"
        ):
            raise ValueError(
                "resource/scaling applicability must retain an explicit unmet gate"
            )
        authorities.add(metric["authority"])
        blockers.append(
            f"metric {metric['name']}: authoritative coordinate-bound budgets/evidence missing"
        )
    # Full release admission below binds the entire Git source tree, including
    # future transitive imports. Do not recursively analyze this resolver's own
    # compiler implementation just to identify an immutable source snapshot.
    hashes = {}
    identities = list(inventory.identities)
    for path in sorted(authorities):
        candidate = root / path
        if (
            not candidate.is_file()
            or candidate.is_symlink()
            or not candidate.resolve().is_relative_to(root)
        ):
            raise ValueError(f"missing or noncanonical matrix authority: {path}")
        identity, raw = capture_stable_regular_file(
            candidate, label=f"matrix authority {path}"
        )
        identities.append(identity)
        hashes[path] = hashlib.sha256(raw).hexdigest()
    for identity in identities:
        verify_stable_regular_file_identity(
            identity, label="release matrix authority generation"
        )
    return ReleaseMatrix(
        source_sha,
        _digest(hashes),
        semantic,
        tuple(sorted(cells, key=lambda c: c["id"])),
        tuple(blockers),
    )


def shard_projection(
    matrix: ReleaseMatrix, *, index: int, count: int
) -> tuple[dict[str, str], ...]:
    """Stable disjoint sharding, independent of execution order or wall clock."""
    if (
        isinstance(count, bool)
        or isinstance(index, bool)
        or not isinstance(count, int)
        or not isinstance(index, int)
        or not 0 <= index < count <= max(1, len(matrix.performance_cells))
    ):
        raise ValueError("invalid release matrix shard coordinate")
    return tuple(
        cell
        for cell in matrix.performance_cells
        if int(hashlib.sha256(cell["id"].encode()).hexdigest(), 16) % count == index
    )


def performance_matrix_problems(
    matrix: ReleaseMatrix,
    shards: Sequence[object],
    *,
    toolchain_identities: Mapping[str, str],
) -> list[str]:
    """Join exact source/toolchain-bound cells; never accept aggregate/skip claims.

    Expected toolchain digests must come from the consuming release custody
    authority, not be copied from these receipts by the caller.
    """
    from tools.perf_authority import release_cell_problems

    problems = []
    expected = {cell["id"]: cell for cell in matrix.performance_cells}
    observed = set()
    shard_coordinates = set()
    partition_count = None
    for offset, shard in enumerate(shards):
        label = f"shard[{offset}]"
        keys = {
            "schema",
            "matrix_sha256",
            "source_sha",
            "authority_sha256",
            "index",
            "count",
            "cells",
        }
        if not isinstance(shard, Mapping) or set(shard) != keys:
            problems.append(f"{label}: exact shard fields required")
            continue
        if (
            shard["schema"] != SHARD_SCHEMA
            or shard["matrix_sha256"] != matrix.identity
            or shard["source_sha"] != matrix.source_sha
            or shard["authority_sha256"] != matrix.authority_sha256
        ):
            problems.append(f"{label}: wrong schema/source/authority/matrix identity")
            continue
        try:
            projected = {
                c["id"]
                for c in shard_projection(
                    matrix, index=shard["index"], count=shard["count"]
                )
            }
        except ValueError as exc:
            problems.append(f"{label}: {exc}")
            continue
        if partition_count is None:
            partition_count = shard["count"]
        elif shard["count"] != partition_count:
            problems.append(f"{label}: mixed partition counts are not one parallel run")
        if shard["index"] in shard_coordinates:
            problems.append(f"{label}: duplicate shard coordinate")
        shard_coordinates.add(shard["index"])
        rows = shard["cells"]
        if not isinstance(rows, list):
            problems.append(f"{label}: cells must be an array")
            continue
        local = set()
        for row in rows:
            if not isinstance(row, Mapping) or set(row) != {
                "coordinate",
                "toolchain_sha256",
                "observed_profiles",
                "measurement",
            }:
                problems.append(f"{label}: exact measured-cell fields required")
                continue
            coordinate = row["coordinate"]
            identity = coordinate.get("id") if isinstance(coordinate, Mapping) else None
            if (
                not isinstance(identity, str)
                or identity not in expected
                or coordinate != expected[identity]
            ):
                problems.append(f"{label}: undeclared/mismatched coordinate")
                continue
            if identity in observed or identity in local:
                problems.append(f"{identity}: duplicate measurement")
            local.add(identity)
            observed.add(identity)
            toolchain = toolchain_identities.get(identity)
            if (
                not isinstance(toolchain, str)
                or len(toolchain) != 64
                or any(c not in "0123456789abcdef" for c in toolchain)
                or row["toolchain_sha256"] != toolchain
            ):
                problems.append(
                    f"{identity}: missing/mismatched custody toolchain identity"
                )
            profile = {
                k: expected[identity][k]
                for k in (
                    "backend",
                    "target",
                    "guest_profile",
                    "runtime_profile",
                    "compiler_profile",
                )
            }
            if row["observed_profiles"] != profile:
                problems.append(f"{identity}: observed profile facts mismatch")
            measurement = row["measurement"]
            if not isinstance(measurement, Mapping):
                problems.append(f"{identity}: measured canonical cell required")
            else:
                observation = measurement.get("build_observation")
                selected = (
                    observation.get("selected_profiles")
                    if isinstance(observation, Mapping)
                    else None
                )
                expected_selected = profile
                if selected != expected_selected:
                    problems.append(
                        f"{identity}: measured build profile observation is missing/mismatched; "
                        "caller-authored coordinate labels are not producer binding"
                    )
                problems.extend(
                    f"{identity}: {p}" for p in release_cell_problems(measurement)
                )
        if local != projected:
            problems.append(
                f"{label}: shard cells do not equal deterministic required projection"
            )
    if partition_count is not None and shard_coordinates != set(range(partition_count)):
        problems.append("missing required parallel shard coordinates")
    missing = set(expected) - observed
    if missing:
        problems.append(f"missing {len(missing)} required performance cells")
    return problems


def full_release_problems(
    matrix: ReleaseMatrix,
    shards: Sequence[object],
    *,
    toolchain_identities: Mapping[str, str],
    semantic_bundle_manifest: Path,
    root: Path = _ROOT,
) -> list[str]:
    """Fail-closed admission; caller-authored coordinate/digest claims grant no credit.

    Shard statistics remain useful diagnostics, but no authenticated full-matrix
    compiler/runtime used-byte admission producer exists yet. Never reinterpret a
    matching digest supplied alongside a measurement as that missing authority.
    """
    from tools import release_exit_gate

    problems = performance_matrix_problems(
        matrix, shards, toolchain_identities=toolchain_identities
    )
    problems.extend(source_admission_problems(root=root, source_sha=matrix.source_sha))
    report = release_exit_gate.verify_release_bundle(
        semantic_bundle_manifest, repo_root=root
    )
    if report.source_sha != matrix.source_sha or not report.passed or report.problems:
        problems.append("canonical source-bound E1–E4/E3 bundle admission failed")
        problems.extend(f"semantic bundle: {problem}" for problem in report.problems)
    canonical = {
        c.id
        for c in verified_subset_coordinates(
            capture_verified_subset_policy(root / "config/verified_subset.toml")[0]
        )
    }
    if set(matrix.semantic_ids) != canonical or len(matrix.semantic_ids) != len(
        canonical
    ):
        problems.append("matrix semantic coordinates differ from canonical E3 policy")
    problems.append(
        "authenticated full-matrix performance receipt and compiler/runtime used-byte admission producer unavailable"
    )
    problems.extend(matrix.blockers)
    return problems


def source_admission_problems(*, root: Path, source_sha: str) -> list[str]:
    """Bind every source blob to an exact Git generation, with no custom filters.

    Release builders should use an immutable Git source projection. Raw CRLF
    transformations are deliberately not treated as byte-identical evidence.
    Tool/interpreter/compiled-artifact custody is independent of this source gate.
    This function never changes the worktree or index.
    """

    def git(*args: str, input_bytes: bytes | None = None) -> bytes:
        completed = subprocess.run(
            ["git", "-C", str(root), *args],
            input=input_bytes,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=30,
            check=False,
        )
        if completed.returncode != 0:
            raise ValueError(
                "Git source admission command failed: " + " ".join(args[:2])
            )
        return completed.stdout

    try:
        if len(source_sha) != 40 or any(
            c not in "0123456789abcdef" for c in source_sha
        ):
            raise ValueError("exact source commit SHA required")
        root = root.resolve(strict=True)
        if Path(git("rev-parse", "--show-toplevel").decode().strip()).resolve() != root:
            raise ValueError("release source must be the exact Git repository root")
        if git("cat-file", "-t", source_sha).strip() != b"commit":
            raise ValueError("release source SHA must identify a commit")
        tree = git("ls-tree", "-rz", source_sha)
        paths = []
        expected = []
        for entry in tree.split(b"\0"):
            if not entry:
                continue
            metadata, raw_path = entry.split(b"\t", 1)
            mode, kind, blob = metadata.split()
            path = raw_path.decode("utf-8")
            if mode not in {b"100644", b"100755"} or kind != b"blob":
                raise ValueError(
                    "source snapshot contains an unadmitted link/submodule"
                )
            if (
                "\n" in path
                or "\r" in path
                or path.startswith("/")
                or ".." in Path(path).parts
            ):
                raise ValueError("source snapshot path is noncanonical")
            candidate = root / path
            if (
                not candidate.is_file()
                or candidate.is_symlink()
                or candidate.resolve() != candidate.absolute()
            ):
                raise ValueError("source snapshot contains missing/noncanonical bytes")
            paths.append(path)
            expected.append(blob)
        if not paths:
            raise ValueError("source snapshot is empty")
        if set(git("ls-files", "-z").split(b"\0")) - {b""} != {
            path.encode("utf-8") for path in paths
        }:
            raise ValueError("index path closure differs from declared source tree")
        # Ignored target/.venv build outputs are independent custody inputs;
        # untracked code within executable source families is never admitted.
        source_families = (
            "src",
            "tools",
            "config",
            "runtime",
            "tests",
            "bench",
            ".github",
            "docs",
            "packaging",
            "formal",
            "wasm",
        )
        root_sources = tuple(
            ":(top,glob)*" + suffix
            for suffix in (
                ".py",
                ".pyi",
                ".rs",
                ".toml",
                ".json",
                ".yml",
                ".yaml",
                ".sh",
                ".pyd",
                ".dll",
                ".so",
            )
        )
        if git("ls-files", "--others", "-z", "--", *source_families, *root_sources):
            raise ValueError(
                "untracked source-family bytes are outside declared commit"
            )
        path_input = "".join(path + "\n" for path in paths).encode("utf-8")
        for _ in range(2):
            actual = git(
                "hash-object", "--no-filters", "--stdin-paths", input_bytes=path_input
            ).splitlines()
            if actual != expected:
                raise ValueError(
                    "tracked source bytes differ from declared commit tree"
                )
            if git("ls-tree", "-rz", source_sha) != tree:
                raise ValueError("source tree changed during admission")
            if git("ls-files", "--others", "-z", "--", *source_families, *root_sources):
                raise ValueError("source bytes added during admission")
        return []
    except (OSError, UnicodeError, ValueError, subprocess.TimeoutExpired) as exc:
        return [f"source snapshot admission failed: {exc}"]
