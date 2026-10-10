#!/usr/bin/env python3
"""Single authority boundary for what is - and is NOT - a citable perf number.

molt has exactly ONE canonical performance source of truth:
``tools/perf_scoreboard.py`` run over the native+LLVM release-fast core board
with cold+warm samples, repeat-CI classification, the quiescence guard, and a
git-ancestry/dirty-tree ``authoritative`` provenance check. Every OTHER lane
that emits wall-clock numbers - ``tools/bench.py`` (daemon batch builder) and
``bench/harness.py`` (the dev/correctness differential harness) - is
NON-CANONICAL and must SELF-IDENTIFY as such so a design agent never cites it.

This module is that shared perf boundary. It owns provenance/freshness policy
and re-exports the ratio primitives implemented in ``molt.metric_ratios`` so the
older tool imports keep one implementation authority instead of each consumer
re-implementing the math:

  1. :func:`non_canonical_provenance` - the stamp every non-canonical JSON
     carries: ``authoritative=False``, ``source=non-canonical``, the ACTUAL
     profile, and a pointer to the canonical gate. It reuses the field
     vocabulary of ``perf_scoreboard.gather_provenance`` so a reader sees the
     same keys (``authoritative`` / ``authoritative_reason``) on every board.
  2. :func:`safe_speedup`, :func:`signed_ratio`, and
     :func:`budget_utilization` - re-exported from ``molt.metric_ratios``, the
     ONE implementation authority for guarded ratio arithmetic.
  3. freshness checks (:func:`git_rev_is_ancestor_of_origin`, :func:`doc_age_days`,
     :func:`STALE_BANNER`, and :func:`stale_snapshot_metadata`) used by
     freshness consumers to flag any perf doc/store whose ``git_rev`` is not on
     origin/main or that is older than N days.

The native+LLVM release-fast core scoreboard command is the daily contract; it
is the only lane permitted to emit ``authoritative=true``. See
``tools/PERF_AUTHORITY.md`` for the consumer rule.
"""

from __future__ import annotations

import datetime as dt
import functools
import shlex
import subprocess
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

TOOLS_ROOT = Path(__file__).resolve().parent
REPO_ROOT = TOOLS_ROOT.parent
SRC_ROOT = REPO_ROOT / "src"
for import_root in (TOOLS_ROOT, SRC_ROOT):
    if str(import_root) not in sys.path:
        sys.path.insert(0, str(import_root))

from molt.metric_ratios import (  # noqa: E402
    RatioDirection as RatioDirection,
    budget_utilization as budget_utilization,
    relative_time_delta as relative_time_delta,
    safe_speedup as safe_speedup,
    signed_ratio as signed_ratio,
    signed_ratio_value as signed_ratio_value,
)
import perf_schema as perf_schema  # noqa: E402
import bench_suites as bench_suites  # noqa: E402

__all__ = [
    "CANONICAL_GATE",
    "CANONICAL_PERF_BENCHMARKS",
    "CANONICAL_PERF_BACKENDS",
    "CANONICAL_PERF_PROFILE",
    "CANONICAL_PERF_REPEAT",
    "CANONICAL_PERF_SAMPLES",
    "CANONICAL_PERF_SET",
    "CANONICAL_PERF_WARMUP",
    "CONTRACT_PROFILE",
    "DEFAULT_STALE_DAYS",
    "STALE_BANNER",
    "STALE_BANNER_MARK",
    "STALE_METADATA_KEY",
    "RatioDirection",
    "budget_utilization",
    "canonical_scoreboard_command_problems",
    "canonical_scoreboard_shape_problems",
    "current_scoreboard_problems",
    "current_origin_main_rev",
    "doc_age_days",
    "git_rev_is_ancestor_of_origin",
    "is_non_canonical_provenance",
    "is_stale_snapshot_metadata",
    "non_canonical_provenance",
    "release_scoreboard_problems",
    "release_cell_problems",
    "perf_tool_identity_problems",
    "relative_time_delta",
    "safe_speedup",
    "scoreboard_revision_fields",
    "signed_ratio",
    "signed_ratio_value",
    "stale_snapshot_metadata",
]

# The one canonical gate. Cited in every non-canonical stamp + every stale
# banner so a reader is always pointed back at the live truth.
CANONICAL_GATE = (
    "tools/perf_scoreboard.py --set core --backend native --backend llvm "
    "--profile release-fast --samples 5 --warmup 2 --repeat 5 --classify "
    "--require-quiescent --quiescence-wait-s 180 --quiescence-poll-s 15"
)

# The release-fast cargo profile is the daily perf contract. A board is only
# *eligible* for authoritative=true when it is the canonical gate at this
# profile; the non-canonical lanes are never authoritative regardless of
# profile, but we record the actual profile so the stamp is honest.
CONTRACT_PROFILE = "release-fast"
CANONICAL_PERF_SET = "core"
CANONICAL_PERF_BENCHMARKS = tuple(bench_suites.BENCHMARKS)
CANONICAL_PERF_BACKENDS = frozenset({"native", "llvm"})
CANONICAL_PERF_PROFILE = CONTRACT_PROFILE
CANONICAL_PERF_SAMPLES = "5"
CANONICAL_PERF_WARMUP = "2"
CANONICAL_PERF_REPEAT = "5"
CANONICAL_PERF_QUIESCENCE_WAIT = "180"
CANONICAL_PERF_QUIESCENCE_POLL = "15"
CANONICAL_PERF_BINARY_IDENTITIES = frozenset(
    f"{backend}/{CANONICAL_PERF_PROFILE}" for backend in CANONICAL_PERF_BACKENDS
)
_CANONICAL_PERF_FORBIDDEN_FLAGS = frozenset(
    {
        "--allow-nonauthoritative",
        "--no-gate",
        "--sample-hot-only",
        "--self-test",
        "--benchmark",
    }
)

# Default staleness horizon for perf docs (days). A markdown perf snapshot
# older than this - OR whose git_rev is not an ancestor of origin/main - is
# stale. 30 days is generous: the canonical board is regenerated per-release; a
# month-old hand-written table is lore, not data.
DEFAULT_STALE_DAYS = 30
_MAX_FUTURE_SKEW = dt.timedelta(minutes=5)

# The banner stamped at the top of every stale perf markdown. Self-identifies
# the doc as non-authoritative and points at the live gate.
STALE_BANNER_MARK = "<!-- PERF-AUTHORITY:stale -->"

# The top-level JSON key used by stale perf stores. Markdown and JSON share the
# same mark so the freshness checker has one acknowledgement vocabulary across
# both channels.
STALE_METADATA_KEY = "perf_authority"


def stale_snapshot_metadata(
    *, generated_at: str | None, git_rev: str | None
) -> dict[str, object]:
    """Return the structured stale acknowledgement for historical perf stores."""
    return {
        "kind": "stale-perf-snapshot",
        "mark": STALE_BANNER_MARK,
        "stale": True,
        "authoritative": False,
        "authoritative_reason": (
            "historical perf snapshot; the only citable perf source is "
            f"`{CANONICAL_GATE}`"
        ),
        "canonical_gate": CANONICAL_GATE,
        "generated_at": generated_at or "unknown",
        "git_rev": git_rev or "unknown",
    }


def is_stale_snapshot_metadata(value: object) -> bool:
    """Does ``value`` carry the canonical structured stale acknowledgement?"""
    if not isinstance(value, dict):
        return False
    return (
        value.get("kind") == "stale-perf-snapshot"
        and value.get("mark") == STALE_BANNER_MARK
        and value.get("stale") is True
        and value.get("authoritative") is False
        and value.get("canonical_gate") == CANONICAL_GATE
    )


def STALE_BANNER(*, generated_at: str, git_rev: str | None) -> str:
    """Return the freshness banner block to prepend to a stale perf markdown."""
    meta = stale_snapshot_metadata(generated_at=generated_at, git_rev=git_rev)
    rev = str(meta["git_rev"])
    return (
        f"{STALE_BANNER_MARK}\n"
        "> **STALE PERF SNAPSHOT - NOT AUTHORITATIVE.**\n"
        ">\n"
        f"> The ONLY citable perf source of truth is `{CANONICAL_GATE}`\n"
        "> (release-fast, cold+warm, quiescent, with a git-ancestry provenance\n"
        "> check). This file is a point-in-time snapshot kept for historical\n"
        "> context only; its numbers may reflect a different profile, a stale\n"
        "> tree, or an already-fixed regression. Do NOT rank or cite it.\n"
        f">\n"
        f"> - generated_at: `{meta['generated_at']}`\n"
        f"> - git_rev: `{rev}`\n"
    )


def _git_output(args: list[str]) -> str | None:
    """Run a bounded read-only git probe; None on any failure (no raise)."""
    try:
        res = subprocess.run(
            ["git", *args],
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
            encoding="utf-8",
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if res.returncode != 0:
        return None
    out = res.stdout.strip()
    return out or None


@functools.lru_cache(maxsize=1)
def _origin_main_rev() -> str | None:
    return _git_output(["rev-parse", "origin/main"])


def current_origin_main_rev() -> str | None:
    """Return the current ``origin/main`` SHA known to this checkout, if any."""
    return _origin_main_rev()


def non_canonical_provenance(
    *,
    profile: str,
    source: str,
    git_rev: str | None = None,
) -> dict[str, object]:
    """Provenance stamp marking a perf JSON as NON-CANONICAL (never citable).

    Every lane that is not ``perf_scoreboard.py --profile release-fast`` emits
    this block so the numbers self-identify. ``authoritative`` is always False;
    ``authoritative_reason`` names why; ``source`` and ``profile`` let a reader
    see exactly which non-canonical lane and profile produced the file.

    Reuses the ``authoritative`` / ``authoritative_reason`` field names from
    ``perf_scoreboard.gather_provenance`` so boards share one vocabulary.
    """
    return {
        "authoritative": False,
        "authoritative_reason": (
            f"non-canonical lane ({source}); the only citable perf source is "
            f"`{CANONICAL_GATE}`"
        ),
        "source": "non-canonical",
        "lane": source,
        "profile": profile,
        "canonical_gate": CANONICAL_GATE,
        "git_rev": git_rev
        if git_rev is not None
        else (_git_output(["rev-parse", "HEAD"]) or "unknown"),
    }


def is_non_canonical_provenance(value: object) -> bool:
    """Does ``value`` carry the canonical non-citable lane provenance?"""
    if not isinstance(value, dict):
        return False
    return (
        value.get("authoritative") is False
        and value.get("source") == "non-canonical"
        and isinstance(value.get("authoritative_reason"), str)
        and CANONICAL_GATE in value["authoritative_reason"]
        and isinstance(value.get("lane"), str)
        and bool(value["lane"])
        and isinstance(value.get("profile"), str)
        and bool(value["profile"])
        and isinstance(value.get("git_rev"), str)
        and bool(value["git_rev"])
        and value.get("canonical_gate") == CANONICAL_GATE
    )


def _command_tokens(command: str) -> tuple[str, ...]:
    try:
        raw = shlex.split(command, posix=False)
    except ValueError:
        raw = command.split()
    return tuple(part.strip("'\"") for part in raw if part.strip("'\""))


def _flag_values(tokens: Sequence[str], flag: str) -> tuple[str, ...]:
    values: list[str] = []
    for index, token in enumerate(tokens):
        if token == flag:
            if index + 1 < len(tokens):
                values.append(tokens[index + 1])
        elif token.startswith(f"{flag}="):
            values.append(token.split("=", 1)[1])
    return tuple(values)


def _has_flag(tokens: Sequence[str], flag: str) -> bool:
    return any(token == flag or token.startswith(f"{flag}=") for token in tokens)


def _command_names_perf_scoreboard(tokens: Sequence[str]) -> bool:
    for token in tokens:
        normalized = token.replace("\\", "/").lower()
        if normalized == "perf_scoreboard.py" or normalized.endswith(
            "/perf_scoreboard.py"
        ):
            return True
    return False


def canonical_scoreboard_command_problems(
    command: str,
    *,
    label: str = "perf receipt",
) -> list[str]:
    """Return problems proving ``command`` is not the canonical perf gate.

    This keeps release gates, docs gates, and future queue receipt consumers on
    the same definition of citable release performance evidence.
    """
    tokens = _command_tokens(command)
    problems: list[str] = []

    if not _command_names_perf_scoreboard(tokens):
        problems.append(f"{label} must run tools/perf_scoreboard.py")

    sets = set(_flag_values(tokens, "--set"))
    if CANONICAL_PERF_SET not in sets:
        problems.append(f"{label} must include --set {CANONICAL_PERF_SET}")

    backends = set(_flag_values(tokens, "--backend"))
    missing_backends = sorted(CANONICAL_PERF_BACKENDS - backends)
    if missing_backends:
        problems.append(
            f"{label} missing canonical backends: {', '.join(missing_backends)}"
        )

    profiles = set(_flag_values(tokens, "--profile"))
    if profiles != {CANONICAL_PERF_PROFILE}:
        got = ", ".join(sorted(profiles)) if profiles else "<none>"
        problems.append(
            f"{label} must use only --profile {CANONICAL_PERF_PROFILE}; got {got}"
        )

    required_singletons = {
        "--samples": CANONICAL_PERF_SAMPLES,
        "--warmup": CANONICAL_PERF_WARMUP,
        "--repeat": CANONICAL_PERF_REPEAT,
        "--quiescence-wait-s": CANONICAL_PERF_QUIESCENCE_WAIT,
        "--quiescence-poll-s": CANONICAL_PERF_QUIESCENCE_POLL,
    }
    for flag, required in required_singletons.items():
        values = set(_flag_values(tokens, flag))
        if values != {required}:
            got = ", ".join(sorted(values)) if values else "<none>"
            problems.append(f"{label} must include {flag} {required}; got {got}")

    for flag in ("--classify", "--require-quiescent"):
        if not _has_flag(tokens, flag):
            problems.append(f"{label} missing {flag}")

    forbidden = sorted(
        flag for flag in _CANONICAL_PERF_FORBIDDEN_FLAGS if _has_flag(tokens, flag)
    )
    if forbidden:
        problems.append(f"{label} uses non-release perf flags: {', '.join(forbidden)}")

    return problems


def _cell_label(cell: Mapping[str, Any]) -> str:
    return (
        f"{cell.get('benchmark')}"
        f"[{cell.get('target')}/{cell.get('backend')}/{cell.get('profile')}]"
    )


def _sample_items(items: Sequence[str], *, limit: int = 5) -> str:
    sample = ", ".join(items[:limit])
    if len(items) > limit:
        sample += f", ... {len(items) - limit} more"
    return sample


def canonical_scoreboard_shape_problems(
    doc: Mapping[str, Any],
    *,
    label: str = "perf scoreboard",
) -> list[str]:
    """Return problems proving a scoreboard is not the canonical release shape."""
    problems: list[str] = []
    cells = perf_schema.flatten_cells(doc)

    summary = doc.get("summary")
    if not isinstance(summary, Mapping) or summary.get("classify_active") is not True:
        problems.append(f"{label} must be generated with --classify")

    expected_benchmarks = set(CANONICAL_PERF_BENCHMARKS)
    benchmarks_run = doc.get("benchmarks_run")
    if not isinstance(benchmarks_run, list) or not all(
        isinstance(item, str) for item in benchmarks_run
    ):
        problems.append(f"{label} benchmarks_run must list the canonical core suite")
        run_benchmarks: set[str] = set()
    else:
        run_benchmarks = set(benchmarks_run)
        if benchmarks_run != list(CANONICAL_PERF_BENCHMARKS):
            problems.append(
                f"{label} benchmarks_run must equal the canonical core suite in order"
            )
        missing = sorted(expected_benchmarks - run_benchmarks)
        if missing:
            problems.append(
                f"{label} missing canonical core benchmarks: {_sample_items(missing)}"
            )
        extra = sorted(run_benchmarks - expected_benchmarks)
        if extra:
            problems.append(
                f"{label} has non-core benchmarks despite --set core: "
                f"{_sample_items(extra)}"
            )

    provenance = doc.get("provenance")
    binary_identities = (
        provenance.get("backend_binary_identity")
        if isinstance(provenance, Mapping)
        else None
    )
    if not isinstance(binary_identities, Mapping):
        problems.append(f"{label} lacks backend binary identities")
    else:
        missing = sorted(CANONICAL_PERF_BINARY_IDENTITIES - set(binary_identities))
        if missing:
            problems.append(
                f"{label} missing backend binary identities: {', '.join(missing)}"
            )
        unexpected_identities = sorted(
            set(binary_identities) - CANONICAL_PERF_BINARY_IDENTITIES
        )
        if unexpected_identities:
            problems.append(
                f"{label} has noncanonical backend binary identities: "
                f"{', '.join(unexpected_identities)}"
            )
        non_strings = sorted(
            key
            for key in CANONICAL_PERF_BINARY_IDENTITIES & set(binary_identities)
            if not isinstance(binary_identities.get(key), str)
            or not str(binary_identities.get(key)).strip()
        )
        if non_strings:
            problems.append(
                f"{label} backend binary identities must be non-empty strings for: "
                f"{', '.join(non_strings)}"
            )

    unexpected = [
        _cell_label(cell)
        for cell in cells
        if cell.get("backend") not in CANONICAL_PERF_BACKENDS
        or cell.get("profile") != CANONICAL_PERF_PROFILE
    ]
    if unexpected:
        sample = ", ".join(unexpected[:5])
        if len(unexpected) > 5:
            sample += f", ... {len(unexpected) - 5} more"
        problems.append(
            f"{label} may only contain native+llvm "
            f"{CANONICAL_PERF_PROFILE} cells; unexpected: {sample}"
        )

    # Legacy labels cannot prove which producer profile built the selected bytes.
    # Keep historical artifacts readable, but never count unbound cells as E2.
    from perf_scoreboard_build_profiles import profile_binding_problems

    for cell in cells:
        profile = cell.get("profile")
        if profile in {"release-fast", "release-output", "dev-fast"}:
            for problem in profile_binding_problems(
                cell.get("build_observation"),
                backend=str(cell.get("backend")),
                profile=str(profile),
            ):
                problems.append(f"{label} {_cell_label(cell)}: {problem}")

    by_benchmark: dict[str, set[str]] = {}
    cell_benchmarks: set[str] = set()
    for cell in cells:
        benchmark = cell.get("benchmark")
        backend = cell.get("backend")
        profile = cell.get("profile")
        if isinstance(benchmark, str):
            cell_benchmarks.add(benchmark)
        if (
            isinstance(benchmark, str)
            and isinstance(backend, str)
            and backend in CANONICAL_PERF_BACKENDS
            and profile == CANONICAL_PERF_PROFILE
        ):
            by_benchmark.setdefault(benchmark, set()).add(backend)

    extra_cell_benchmarks = sorted(cell_benchmarks - expected_benchmarks)
    if extra_cell_benchmarks:
        problems.append(
            f"{label} contains non-core benchmark cells despite --set core: "
            f"{_sample_items(extra_cell_benchmarks)}"
        )

    if not by_benchmark:
        problems.append(
            f"{label} contains no native+llvm {CANONICAL_PERF_PROFILE} benchmark cells"
        )
    else:
        missing_rows = [
            (
                f"{benchmark} missing "
                f"{', '.join(sorted(CANONICAL_PERF_BACKENDS - by_benchmark.get(benchmark, set())))}"
            )
            for benchmark in sorted(expected_benchmarks)
            if CANONICAL_PERF_BACKENDS - by_benchmark.get(benchmark, set())
        ]
        if missing_rows:
            problems.append(
                f"{label} must include both native and llvm "
                f"{CANONICAL_PERF_PROFILE} cells for every benchmark; "
                f"{_sample_items(missing_rows)}"
            )

    return problems


def scoreboard_revision_fields(doc: Mapping[str, Any]) -> tuple[tuple[str, str], ...]:
    """Return scoreboard revision facts that must agree with current origin/main."""
    fields: list[tuple[str, str]] = []
    git_rev = doc.get("git_rev")
    if isinstance(git_rev, str) and git_rev and git_rev != "unknown":
        fields.append(("git_rev", git_rev))
    provenance = doc.get("provenance")
    if isinstance(provenance, Mapping):
        local_head = provenance.get("local_head_sha")
        if isinstance(local_head, str) and local_head and local_head != "unknown":
            fields.append(("provenance.local_head_sha", local_head))
    return tuple(fields)


def _short_rev(rev: str | None) -> str:
    return rev[:12] if rev else "<unknown>"


def _sample_schema_problems(problems: Sequence[str], *, limit: int = 3) -> str:
    sample = "; ".join(problems[:limit])
    if len(problems) > limit:
        sample += f"; ... {len(problems) - limit} more"
    return sample


def current_scoreboard_problems(
    doc: Mapping[str, Any],
    *,
    label: str = "scoreboard",
    shape_label: str | None = None,
    now: dt.datetime | None = None,
    max_age_days: float = DEFAULT_STALE_DAYS,
    require_canonical_shape: bool = False,
) -> list[str]:
    """Return why a scoreboard is not current, citable release evidence.

    This is the single authority behind release-exit E2 and perf-freshness:
    current scoreboard evidence must be schema-valid, generated from the exact
    current ``origin/main`` tip, authoritative, fresh, and green. Callers that
    need the full release matrix (native+LLVM, release-fast, classified, core)
    set ``require_canonical_shape=True``.
    """
    problems: list[str] = []

    schema_problems = perf_schema.validate_board(doc)
    if schema_problems:
        problems.append(
            f"{label} schema invalid: {_sample_schema_problems(schema_problems)}"
        )

    if require_canonical_shape:
        problems.extend(
            canonical_scoreboard_shape_problems(doc, label=shape_label or label)
        )

    provenance = doc.get("provenance")
    authoritative = (
        provenance.get("authoritative") if isinstance(provenance, Mapping) else None
    )
    if authoritative is not True:
        reason = (
            provenance.get("authoritative_reason")
            if isinstance(provenance, Mapping)
            and isinstance(provenance.get("authoritative_reason"), str)
            else "missing/false provenance.authoritative"
        )
        problems.append(f"{label} is not authoritative: {reason}")

    problems.extend(
        f"{label} {problem}"
        for problem in perf_tool_identity_problems(
            provenance if isinstance(provenance, Mapping) else {}
        )
    )

    summary = doc.get("summary")
    gate_fails = summary.get("gate_fails") if isinstance(summary, Mapping) else None
    if gate_fails is not False:
        problems.append(f"{label} gate_fails is not false: {gate_fails!r}")

    cells = perf_schema.flatten_cells(doc)
    if not cells:
        problems.append(f"{label} has no measured required cells")
    if doc.get("benchmarks_deferred"):
        problems.append(f"{label} contains deferred required benchmarks")
    for cell in cells:
        problems.extend(
            f"{label} {_cell_label(cell)} {problem}"
            for problem in release_cell_problems(cell)
        )

    generated_at = doc.get("generated_at")
    age = doc_age_days(generated_at if isinstance(generated_at, str) else None, now=now)
    if age is None:
        problems.append(f"{label} generated_at is missing or unparseable")
    elif age > max_age_days:
        problems.append(f"{label} generated_at is {age:.0f}d old (>{max_age_days:g}d)")

    origin_rev = current_origin_main_rev()
    rev_fields = scoreboard_revision_fields(doc)
    if origin_rev is None:
        problems.append(f"{label} cannot resolve origin/main for HEAD check")
    elif not rev_fields:
        problems.append(f"{label} has no git_rev or provenance.local_head_sha")
    else:
        for field, rev in rev_fields:
            if rev != origin_rev:
                problems.append(
                    f"{label} {field} {_short_rev(rev)} != "
                    f"origin/main {_short_rev(origin_rev)}"
                )

    return problems


def perf_tool_identity_problems(provenance: Mapping[str, Any]) -> list[str]:
    """Historical entrypoint-only hashes do not attest the measurement family."""
    if provenance.get("benchmark_tool_identity_schema") != "molt-perf-tool-family-v1":
        return [
            "benchmark tool identity must attest molt-perf-tool-family-v1; remeasure legacy evidence"
        ]
    digest = provenance.get("benchmark_tool_sha")
    if (
        not isinstance(digest, str)
        or len(digest) != 64
        or any(ch not in "0123456789abcdef" for ch in digest)
    ):
        return ["benchmark tool family identity must be a lowercase SHA256 digest"]
    return []


def release_cell_problems(cell: Mapping[str, Any]) -> list[str]:
    """One fail-closed acceptance rule for each required CPython comparison.

    Classification remains diagnostic; only a measured, quiescent, repeated
    confidence interval entirely above CPython establishes a release win.
    """
    import math

    problems: list[str] = []
    for field in ("build_ok", "molt_ok", "cpython_ok", "stable", "measured_quiescent"):
        if cell.get(field) is not True:
            problems.append(f"{field} must be true")
    for field, expected in (
        ("verdict", "GREEN"),
        ("classification", "GREEN_STABLE"),
        ("repeat_stability", "STABLE_ABOVE"),
        ("repeat_passes", int(CANONICAL_PERF_REPEAT)),
    ):
        if cell.get(field) != expected:
            problems.append(f"{field} must be {expected!r}")
    if not perf_schema.output_parity_passes(cell.get("output_parity")):
        problems.append("output parity must prove the same observable program")
    lo, hi = cell.get("repeat_ci_lo"), cell.get("repeat_ci_hi")
    if not all(
        isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v)
        for v in (lo, hi)
    ):
        problems.append("repeat confidence interval must be finite")
    elif not 1.0 < lo <= hi:
        problems.append("repeat confidence interval must lie entirely above CPython")
    return problems


def release_scoreboard_problems(
    doc: Mapping[str, Any],
    *,
    expected_source_sha: str,
    label: str = "release scoreboard",
    now: dt.datetime | None = None,
    max_age_days: float = DEFAULT_STALE_DAYS,
) -> list[str]:
    """Validate one canonical scoreboard for an exact release revision.

    Unlike :func:`current_scoreboard_problems`, this authority never reads an
    ambient branch tip. The requested release revision is the identity, and
    every scoreboard revision field must name it exactly. The canonical
    command is not accepted as caller-authored metadata: its recorded
    projections (core matrix, native+LLVM release-fast cells, sample/warmup
    counts, classification, and quiescence custody) must all be present in the
    scoreboard bytes.
    """

    problems: list[str] = []
    schema_problems = perf_schema.validate_board(doc)
    if schema_problems:
        problems.append(
            f"{label} schema invalid: {_sample_schema_problems(schema_problems)}"
        )
    problems.extend(canonical_scoreboard_shape_problems(doc, label=label))
    problems.extend(
        f"{label} {problem}" for problem in scoreboard_observed_toolchain_problems(doc)
    )

    if doc.get("kind") != "cpython_floor_scoreboard":
        problems.append(f"{label} kind must be 'cpython_floor_scoreboard'")

    provenance = doc.get("provenance")
    if not isinstance(provenance, Mapping):
        problems.append(f"{label} provenance must be an object")
        provenance = {}
    if provenance.get("authoritative") is not True:
        problems.append(f"{label} provenance.authoritative must be true")
    if provenance.get("dirty_tree") is not False:
        problems.append(f"{label} provenance.dirty_tree must be false")
    if provenance.get("require_quiescent") is not True:
        problems.append(f"{label} provenance.require_quiescent must be true")
    if provenance.get("quiescent") is not True:
        problems.append(f"{label} provenance.quiescent must be true")
    quiescence = provenance.get("quiescence")
    if not isinstance(quiescence, Mapping):
        problems.append(f"{label} provenance.quiescence must be an object")
    else:
        if quiescence.get("quiet") is not True:
            problems.append(f"{label} provenance.quiescence.quiet must be true")
        expected_wait = float(CANONICAL_PERF_QUIESCENCE_WAIT)
        if quiescence.get("quiescence_wait_timeout_s") != expected_wait:
            problems.append(
                f"{label} provenance.quiescence.quiescence_wait_timeout_s "
                f"must be {expected_wait:g}"
            )

    host = doc.get("host", {})
    oracle = host.get("cpython_oracle", {}) if isinstance(host, Mapping) else {}
    try:
        from molt.target_python import resolve_target_python_for_oracle

        minor = tuple(int(part) for part in oracle["version"].split(".")[:2])
        resolve_target_python_for_oracle(minor, host.get("molt_target_python"))
        if host.get("molt_target_python") is None:
            problems.append(f"{label} lacks recorded Molt target Python")
    except (ValueError, TypeError, KeyError, AttributeError) as exc:
        problems.append(f"{label} oracle/target mismatch: {exc}")

    problems.extend(
        f"{label} {problem}" for problem in perf_tool_identity_problems(provenance)
    )

    summary = doc.get("summary")
    gate_fails = summary.get("gate_fails") if isinstance(summary, Mapping) else None
    if gate_fails is not False:
        problems.append(f"{label} gate_fails is not false: {gate_fails!r}")

    methodology = doc.get("methodology")
    if not isinstance(methodology, Mapping):
        problems.append(f"{label} methodology must be an object")
    else:
        expected_methodology = {
            "samples_per_phase": int(CANONICAL_PERF_SAMPLES),
            "warmup_runs": int(CANONICAL_PERF_WARMUP),
        }
        for field, expected in expected_methodology.items():
            if methodology.get(field) != expected:
                problems.append(
                    f"{label} methodology.{field} must be {expected}; "
                    f"got {methodology.get(field)!r}"
                )

    generated_at = doc.get("generated_at")
    current = now or dt.datetime.now(dt.timezone.utc)
    if current.tzinfo is None or current.utcoffset() is None:
        raise ValueError("release scoreboard validation 'now' must be timezone-aware")
    age = doc_age_days(
        generated_at if isinstance(generated_at, str) else None,
        now=current,
    )
    if age is None:
        problems.append(f"{label} generated_at is missing or unparseable")
    elif age < -(_MAX_FUTURE_SKEW.total_seconds() / 86400.0):
        problems.append(f"{label} generated_at is unreasonably far in the future")
    elif age > max_age_days:
        problems.append(f"{label} generated_at is {age:.0f}d old (>{max_age_days:g}d)")

    from tools.git_identity import is_git_object_id

    if not is_git_object_id(expected_source_sha):
        raise ValueError("expected_source_sha must be lowercase Git object hex")
    revision_fields = dict(scoreboard_revision_fields(doc))
    required_revision_fields = {
        "git_rev": doc.get("git_rev"),
        "provenance.local_head_sha": provenance.get("local_head_sha"),
        "provenance.origin_sha": provenance.get("origin_sha"),
        "provenance.merge_base_sha": provenance.get("merge_base_sha"),
    }
    for field, value in required_revision_fields.items():
        if value != expected_source_sha:
            problems.append(
                f"{label} {field} must equal requested source "
                f"{_short_rev(expected_source_sha)}; got "
                f"{_short_rev(value if isinstance(value, str) else None)}"
            )
    if set(revision_fields) != {"git_rev", "provenance.local_head_sha"}:
        problems.append(
            f"{label} must contain exact git_rev and provenance.local_head_sha"
        )

    if doc.get("benchmarks_deferred"):
        problems.append(f"{label} contains deferred required benchmarks")
    for cell in perf_schema.flatten_cells(doc):
        problems.extend(
            f"{label} {_cell_label(cell)} {problem}"
            for problem in release_cell_problems(cell)
        )

    return problems


@functools.lru_cache(maxsize=512)
def git_rev_is_ancestor_of_origin(git_rev: str | None) -> bool | None:
    """Is ``git_rev`` an ancestor of (or equal to) ``origin/main``?

    Returns True/False, or None when it cannot be determined (unknown rev,
    origin/main ref absent, or git unavailable). A perf doc whose recorded
    ``git_rev`` is NOT an ancestor of origin/main was measured on a tree that
    is not the shipped contract - freshness consumers flag it.
    """
    if not git_rev or git_rev == "unknown":
        return None
    origin = _origin_main_rev()
    if origin is None:
        return None
    # `git merge-base --is-ancestor A B` exits 0 iff A is an ancestor of B
    # (or A == B). Run it directly so we get the exit code, not stdout.
    try:
        res = subprocess.run(
            ["git", "merge-base", "--is-ancestor", git_rev, origin],
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
            encoding="utf-8",
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if res.returncode == 0:
        return True
    if res.returncode == 1:
        return False
    # rc 128 etc. => unknown commit / bad ref: undeterminable, not "fresh".
    return None


def doc_age_days(
    generated_at: str | None, *, now: dt.datetime | None = None
) -> float | None:
    """Age in days of an ISO-8601 ``generated_at`` timestamp, or None if unparseable."""
    if not generated_at:
        return None
    text = generated_at.strip()
    # Accept a trailing 'Z' (UTC) which datetime.fromisoformat rejects before 3.11
    # in some forms; normalize to +00:00.
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        ts = dt.datetime.fromisoformat(text)
    except ValueError:
        # Date-only form (e.g. "2026-03-25") used by some hand-written docs.
        try:
            ts = dt.datetime.strptime(text, "%Y-%m-%d")
        except ValueError:
            return None
    if ts.tzinfo is None:
        ts = ts.replace(tzinfo=dt.timezone.utc)
    current = now or dt.datetime.now(dt.timezone.utc)
    if current.tzinfo is None:
        current = current.replace(tzinfo=dt.timezone.utc)
    return (current - ts).total_seconds() / 86400.0


def scoreboard_observed_toolchain_problems(doc: Mapping[str, Any]) -> list[str]:
    """Reject absent observations and avoid laundering them into E2 attestation.

    Publication observations are useful diagnostics. A daemon/compile admission
    receipt proving the exact compiler/runtime bytes is still required for E2.
    """
    import re

    problems: list[str] = []

    def check_identity(value: Any, label: str) -> None:
        if (
            not isinstance(value, Mapping)
            or not isinstance(value.get("size"), int)
            or isinstance(value.get("size"), bool)
            or value.get("size", 0) <= 0
            or not isinstance(value.get("sha256"), str)
            or re.fullmatch(r"[0-9a-f]{64}", value["sha256"]) is None
        ):
            problems.append(f"{label} lacks observed file-content identity")

    def check_runtime(value: Any, image: Any, label: str, version: Any = None) -> None:
        from molt.python_runtime_identity import (
            validate_python_runtime_identity,
            runtime_explicit_file_content,
        )

        try:
            runtime = validate_python_runtime_identity(value)
        except (ValueError, TypeError) as exc:
            problems.append(
                f"{label} canonical Python runtime closure is invalid: {exc}"
            )
            return
        if version is not None and runtime.get("version") != version:
            problems.append(f"{label} runtime version differs from observed oracle")
        base = runtime_explicit_file_content(runtime, "base-executable")
        if (
            not isinstance(image, Mapping)
            or not isinstance(base, Mapping)
            or any(image.get(k) != base.get(k) for k in ("sha256", "size"))
            or image.get("content_filename") != base.get("filename")
        ):
            problems.append(
                f"{label} base image differs from canonical loaded runtime closure"
            )

    provenance = doc.get("provenance", {})
    invocations = (
        provenance.get("producer_invocations")
        if isinstance(provenance, Mapping)
        else None
    )
    if not isinstance(invocations, list) or not invocations:
        problems.append("producer invocation records are missing")
    else:
        for record in invocations:
            if not isinstance(record, Mapping):
                problems.append("producer invocation record is invalid")
                continue
            argv = record.get("argv")
            if (
                not isinstance(argv, list)
                or not argv
                or any(not isinstance(v, str) or not v for v in argv)
            ):
                problems.append("producer invocation argv is invalid")
            check_identity(
                record.get("command_interpreter"), "producer command interpreter"
            )
            check_identity(record.get("base_interpreter"), "producer base interpreter")
            check_runtime(
                record.get("runtime_closure"),
                record.get("base_interpreter"),
                "producer",
            )
    host = doc.get("host", {})
    oracle = host.get("cpython_oracle", {}) if isinstance(host, Mapping) else {}
    for field in ("command_executable_identity", "base_executable_identity"):
        check_identity(
            oracle.get(field) if isinstance(oracle, Mapping) else None,
            f"CPython {field}",
        )
    check_runtime(
        oracle.get("runtime_closure") if isinstance(oracle, Mapping) else None,
        oracle.get("base_executable_identity") if isinstance(oracle, Mapping) else None,
        "CPython",
        oracle.get("version") if isinstance(oracle, Mapping) else None,
    )
    cells = perf_schema.flatten_cells(doc)
    if not cells:
        problems.append("measured cells are missing")
        return problems
    for cell in cells:
        observation = (
            cell.get("build_observation") if isinstance(cell, Mapping) else None
        )
        if (
            not isinstance(observation, Mapping)
            or observation.get("kind") != "molt-build-observation-v1"
        ):
            problems.append("cell build-toolchain observation is missing")
            continue
        for name in ("compiler", "runtime", "artifact"):
            fact = observation.get(name)
            check_identity(
                fact.get("identity") if isinstance(fact, Mapping) else None,
                f"cell {name}",
            )
        # No admission receipt is emitted yet; a self-authored true flag must not
        # bypass that missing structural proof.
        problems.append(
            "cell compiler/runtime used-byte admission receipt is unavailable"
        )
    return problems


def scoreboard_release_eligibility(doc: Mapping[str, Any]) -> dict[str, Any]:
    """Report canonical-core E2 eligibility, never complete release readiness."""
    from tools.git_identity import is_git_object_id

    revision = doc.get("git_rev")
    if not is_git_object_id(revision):
        problems = ["scoreboard lacks an exact source revision"]
        problems.extend(scoreboard_observed_toolchain_problems(doc))
    else:
        problems = release_scoreboard_problems(doc, expected_source_sha=revision)
    return {
        "scope": "canonical-core-E2",
        "eligible": not problems,
        "problems": problems,
    }
