#!/usr/bin/env python3
"""Fail-closed PERF-FRESHNESS gate - no stale perf number masquerades as current.

molt has exactly one citable perf source of truth: the canonical scoreboard
command exported by ``tools/perf_authority.py``. Every other perf artifact -
dated result JSONs, hand-written markdown tables, triage snapshots - is a
point-in-time record that goes stale. A stale record is dangerous precisely
because it still reads as a confident table of ratios; a design agent handed a
3-month-old ``0.01x`` row can rank it #1 and chase a regression that no longer
exists (the exact forensic mislead this gate exists to kill).

This checker flags any perf doc/store that is BOTH (a) presents perf numbers and
(b) is stale - older than the staleness horizon OR measured on a tree whose
``git_rev`` is not an ancestor of ``origin/main`` - UNLESS it has been explicitly
stamped with the stale banner (``perf_authority.STALE_BANNER_MARK``) for text or
the matching structured ``perf_authority`` stale record for JSON. A stamped
artifact is an acknowledged historical record and passes; an UNSTAMPED stale
artifact is a hazard and fails the gate.

Root CPython-floor scoreboards under ``bench/scoreboard/*.json`` are stricter:
an unstamped board is claiming current release evidence, so it must be schema
valid, generated at the current ``origin/main`` tip, authoritative, fresh, and
green (``summary.gate_fails == false``). A stale/red/non-authoritative scoreboard
must either be refreshed by ``tools/perf_scoreboard.py`` or stamped as historical
so it cannot masquerade as E2 proof.

Usage::

    python3 tools/check_perf_freshness.py            # gate (exit 1 on hazard)
    python3 tools/check_perf_freshness.py --json      # machine-readable report
    python3 tools/check_perf_freshness.py --max-age-days 14
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_authority as pa  # noqa: E402

# Top-level bench result snapshots plus recursive bench/results stores are perf
# artifacts. Benchmark corpus docs, runner source, and baseline schema docs are
# intentionally outside this freshness boundary.
TOP_LEVEL_ARTIFACT_DIRS = {
    REPO_ROOT / "bench",
}
RECURSIVE_ARTIFACT_DIRS = {
    REPO_ROOT / "bench" / "results",
}
SCOREBOARD_DIR = REPO_ROOT / "bench" / "scoreboard"

# Root scoreboard JSON gets a stricter current-vs-historical pass below; keep it
# out of the generic stale snapshot scanner so support files such as cold-start
# budget docs do not inherit the CPython-floor release policy accidentally.
EXEMPT_DIRS = {
    SCOREBOARD_DIR,
}

# A doc "presents CITABLE perf numbers" if it contains a measured ratio token:
# a numeric speedup like ``1.12x`` / ``0.01x`` (the shape a design agent ranks),
# or one of the scoreboard ratio field names emitted into a results table. A
# bare prose mention of the WORD "speedup" (e.g. "expected 20-40% speedup" in a
# root-cause note) is deliberately NOT matched - that is a design projection,
# not a stale benchmark result. Only genuine citable numbers trip the gate.
_RATIO_TOKEN_RE = re.compile(
    r"\d+\.\d+\s*x\b"  # measured ratio, e.g. 1.12x / 0.01x
    r"|molt_cpython_ratio|molt_speedup|warm_speedup|cold_speedup"  # result fields
    r"|\bspeedup\b\s*[:|=]",  # a "Speedup:" / "Speedup |" column header/value
    re.IGNORECASE,
)

_JSON_PERF_KEY_RE = re.compile(
    r"(?:^|_)(?:ratio|speedup)(?:_|$)"
    r"|(?:time|compile|elapsed|median|mean|stdev|samples)_s$"
    r"|^samples_sec$"
    r"|(?:^|_)(?:size_bytes|size_kb)$",
    re.IGNORECASE,
)

# Date embedded in a filename, e.g. *_20260320.* or *-20260320T*.
_FILENAME_DATE_RE = re.compile(r"(20\d{2})(\d{2})(\d{2})")

# git_rev embedded in a markdown line, e.g. "Git rev: <40-hex>".
_DOC_GIT_REV_RE = re.compile(r"git[\s_]*rev[:\s`]*([0-9a-f]{7,40})", re.IGNORECASE)
# generated_at / Date embedded in a markdown line.
_DOC_DATE_RE = re.compile(
    r"(?:generated_at|date)[:\s`*]*([0-9]{4}-[0-9]{2}-[0-9]{2}[T0-9:+\-Z.]*)",
    re.IGNORECASE,
)


def _read_text(path: Path) -> str:
    # Perf docs are UTF-8; tolerate the recurring Windows cp1252 hazard by
    # decoding with errors="replace" rather than crashing the gate.
    try:
        return path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return path.read_bytes().decode("utf-8", errors="replace")


def _is_under(path: Path, base: Path) -> bool:
    try:
        path.resolve().relative_to(base.resolve())
        return True
    except ValueError:
        return False


def _is_exempt(path: Path) -> bool:
    return any(_is_under(path, base) for base in EXEMPT_DIRS)


def _rel_path(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(REPO_ROOT)).replace("\\", "/")
    except ValueError:
        # Path outside the repo (e.g. a unit-test tmp dir): fall back to the
        # file name. The classification logic is path-independent.
        return path.name


def _tracked_perf_artifacts() -> list[Path] | None:
    try:
        res = subprocess.run(
            ["git", "ls-files", "--", "bench"],
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if res.returncode != 0:
        return None
    return [REPO_ROOT / line for line in res.stdout.splitlines() if line.strip()]


def _iter_perf_artifacts() -> list[Path]:
    out: list[Path] = []
    seen: set[Path] = set()
    tracked = _tracked_perf_artifacts()
    if tracked is None:
        candidates: list[Path] = []
        for artifact_dir in TOP_LEVEL_ARTIFACT_DIRS:
            if artifact_dir.exists():
                candidates.extend(artifact_dir.glob("*"))
        for artifact_dir in RECURSIVE_ARTIFACT_DIRS:
            if artifact_dir.exists():
                candidates.extend(artifact_dir.rglob("*"))
    else:
        candidates = tracked

    for path in sorted(candidates):
        if path.is_dir():
            continue
        if path.suffix.lower() not in (".md", ".txt", ".json"):
            continue
        in_top_level_dir = any(
            path.parent.resolve() == artifact_dir.resolve()
            for artifact_dir in TOP_LEVEL_ARTIFACT_DIRS
        )
        in_recursive_dir = any(
            _is_under(path, artifact_dir) for artifact_dir in RECURSIVE_ARTIFACT_DIRS
        )
        if not (in_top_level_dir or in_recursive_dir):
            continue
        if _is_exempt(path):
            continue
        rp = path.resolve()
        if rp in seen:
            continue
        seen.add(rp)
        out.append(path)
    return out


def _iter_perf_docs() -> list[Path]:
    return [p for p in _iter_perf_artifacts() if p.suffix.lower() in (".md", ".txt")]


def _iter_scoreboard_artifacts() -> list[Path]:
    tracked = _tracked_perf_artifacts()
    if tracked is None:
        candidates = (
            list(SCOREBOARD_DIR.glob("*.json")) if SCOREBOARD_DIR.exists() else []
        )
    else:
        candidates = [
            path
            for path in tracked
            if path.parent.resolve() == SCOREBOARD_DIR.resolve()
            and path.suffix.lower() == ".json"
        ]
    return sorted(path for path in candidates if path.is_file())


def _doc_git_rev(text: str) -> str | None:
    m = _DOC_GIT_REV_RE.search(text)
    if not m:
        return None
    rev = m.group(1)
    return rev if rev not in ("unknown",) else None


def _doc_generated_at(text: str, path: Path) -> str | None:
    m = _DOC_DATE_RE.search(text)
    if m:
        return m.group(1)
    fn = _FILENAME_DATE_RE.search(path.name)
    if fn:
        return f"{fn.group(1)}-{fn.group(2)}-{fn.group(3)}"
    return None


def _json_find_first_str(value: object, keys: set[str]) -> str | None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key.lower() in keys and isinstance(item, str):
                return item
        for item in value.values():
            found = _json_find_first_str(item, keys)
            if found:
                return found
    elif isinstance(value, list):
        for item in value:
            found = _json_find_first_str(item, keys)
            if found:
                return found
    return None


def _json_has_perf_numbers(value: object) -> bool:
    if isinstance(value, dict):
        for key, item in value.items():
            if _JSON_PERF_KEY_RE.search(str(key)):
                return True
            if _json_has_perf_numbers(item):
                return True
    elif isinstance(value, list):
        return any(_json_has_perf_numbers(item) for item in value)
    return False


def _classify_artifact(
    *,
    path: Path,
    artifact_kind: str,
    has_numbers: bool,
    stamped: bool,
    git_rev: str | None,
    generated_at: str | None,
    max_age_days: float,
    now: dt.datetime,
    non_canonical: bool = False,
) -> dict:
    age = pa.doc_age_days(generated_at, now=now)
    ancestor = pa.git_rev_is_ancestor_of_origin(git_rev)

    reasons: list[str] = []
    if age is not None and age > max_age_days:
        reasons.append(
            f"generated_at {generated_at} is {age:.0f}d old (> {max_age_days:.0f}d)"
        )
    if ancestor is False:
        reasons.append(f"git_rev {git_rev} is NOT an ancestor of origin/main")
    # FAIL CLOSED: a doc that presents perf numbers but proves freshness through
    # NEITHER a parseable generated_at NOR an ancestor-verified git_rev is
    # UNDATEABLE - it cannot demonstrate it reflects the current tree, so it is
    # treated as stale. (A stale doc with all-zero / unknown rev and no date is
    # the classic abandoned snapshot.) Stamping it clears the hazard.
    fresh_proof = (age is not None and age <= max_age_days) or (ancestor is True)
    if has_numbers and not fresh_proof and not reasons:
        reasons.append(
            "cannot prove freshness: no parseable generated_at and no "
            "ancestor-verified git_rev (undateable perf snapshot)"
        )

    is_stale = bool(reasons)
    # A doc is a HAZARD iff it presents perf numbers, is stale/undateable, and
    # is neither stamped as historical nor born with canonical non-citable
    # lane provenance. The latter must be recognized at creation time so CI
    # does not spontaneously fail when an explicitly non-authoritative receipt
    # crosses the age horizon.
    hazard = has_numbers and is_stale and not (stamped or non_canonical)

    if not has_numbers:
        verdict = "no-perf-numbers"
    elif non_canonical:
        verdict = "non-canonical"
    elif not is_stale:
        verdict = "fresh"
    elif stamped:
        verdict = "stale-stamped"
    else:
        verdict = "stale-hazard"

    return {
        "path": _rel_path(path),
        "artifact_kind": artifact_kind,
        "verdict": verdict,
        "hazard": hazard,
        "has_perf_numbers": has_numbers,
        "stamped": stamped,
        "non_canonical": non_canonical,
        "git_rev": git_rev,
        "generated_at": generated_at,
        "age_days": round(age, 1) if age is not None else None,
        "git_rev_ancestor_of_origin": ancestor,
        "reasons": reasons,
    }


def evaluate_doc(path: Path, *, max_age_days: float, now: dt.datetime) -> dict:
    """Classify one perf markdown/text doc. Returns a verdict record."""
    text = _read_text(path)
    return _classify_artifact(
        path=path,
        artifact_kind="text",
        has_numbers=bool(_RATIO_TOKEN_RE.search(text)),
        stamped=pa.STALE_BANNER_MARK in text,
        git_rev=_doc_git_rev(text),
        generated_at=_doc_generated_at(text, path),
        max_age_days=max_age_days,
        now=now,
    )


def evaluate_json(path: Path, *, max_age_days: float, now: dt.datetime) -> dict:
    """Classify one perf JSON store. Returns a verdict record."""
    try:
        payload = json.loads(_read_text(path))
    except json.JSONDecodeError as exc:
        return _classify_artifact(
            path=path,
            artifact_kind="json",
            has_numbers=True,
            stamped=False,
            git_rev=None,
            generated_at=None,
            max_age_days=max_age_days,
            now=now,
        ) | {"reasons": [f"invalid JSON: {exc.msg}"]}

    meta = payload.get(pa.STALE_METADATA_KEY) if isinstance(payload, dict) else None
    provenance = payload.get("provenance") if isinstance(payload, dict) else None
    generated_at = _json_find_first_str(
        payload, {"generated_at", "created_at", "timestamp", "run_started_at"}
    )
    git_rev = _json_find_first_str(payload, {"git_rev"})
    return _classify_artifact(
        path=path,
        artifact_kind="json",
        has_numbers=_json_has_perf_numbers(payload),
        stamped=pa.is_stale_snapshot_metadata(meta),
        non_canonical=pa.is_non_canonical_provenance(provenance),
        git_rev=git_rev,
        generated_at=generated_at,
        max_age_days=max_age_days,
        now=now,
    )


def evaluate_scoreboard(path: Path, *, max_age_days: float, now: dt.datetime) -> dict:
    """Classify one root ``bench/scoreboard`` JSON artifact."""
    try:
        payload = json.loads(_read_text(path))
    except json.JSONDecodeError as exc:
        return {
            "path": _rel_path(path),
            "artifact_kind": "scoreboard",
            "verdict": "scoreboard-hazard",
            "hazard": True,
            "has_perf_numbers": True,
            "stamped": False,
            "git_rev": None,
            "generated_at": None,
            "age_days": None,
            "git_rev_ancestor_of_origin": None,
            "origin_main_rev": pa.current_origin_main_rev(),
            "scoreboard_kind": None,
            "reasons": [f"invalid JSON: {exc.msg}"],
        }

    if (
        not isinstance(payload, dict)
        or payload.get("kind") != "cpython_floor_scoreboard"
    ):
        return _classify_artifact(
            path=path,
            artifact_kind="scoreboard-support",
            has_numbers=False,
            stamped=False,
            git_rev=None,
            generated_at=None,
            max_age_days=max_age_days,
            now=now,
        )

    meta = payload.get(pa.STALE_METADATA_KEY)
    stamped = pa.is_stale_snapshot_metadata(meta)
    raw_generated_at = payload.get("generated_at")
    generated_at = raw_generated_at if isinstance(raw_generated_at, str) else None
    age = pa.doc_age_days(generated_at, now=now)
    rev_fields = pa.scoreboard_revision_fields(payload)
    primary_rev = rev_fields[0][1] if rev_fields else None
    ancestor = pa.git_rev_is_ancestor_of_origin(primary_rev)
    origin_rev = pa.current_origin_main_rev()

    if stamped:
        return {
            "path": _rel_path(path),
            "artifact_kind": "scoreboard",
            "verdict": "scoreboard-historical-stamped",
            "hazard": False,
            "has_perf_numbers": True,
            "stamped": True,
            "git_rev": primary_rev,
            "generated_at": generated_at,
            "age_days": round(age, 1) if age is not None else None,
            "git_rev_ancestor_of_origin": ancestor,
            "origin_main_rev": origin_rev,
            "scoreboard_kind": payload.get("kind"),
            "reasons": [],
        }

    reasons = pa.current_scoreboard_problems(
        payload,
        label="scoreboard",
        now=now,
        max_age_days=max_age_days,
        require_canonical_shape=False,
    )

    return {
        "path": _rel_path(path),
        "artifact_kind": "scoreboard",
        "verdict": "scoreboard-current-green" if not reasons else "scoreboard-hazard",
        "hazard": bool(reasons),
        "has_perf_numbers": True,
        "stamped": False,
        "git_rev": primary_rev,
        "generated_at": generated_at,
        "age_days": round(age, 1) if age is not None else None,
        "git_rev_ancestor_of_origin": ancestor,
        "origin_main_rev": origin_rev,
        "scoreboard_kind": payload.get("kind"),
        "reasons": reasons,
    }


def evaluate_artifact(path: Path, *, max_age_days: float, now: dt.datetime) -> dict:
    if path.suffix.lower() == ".json":
        return evaluate_json(path, max_age_days=max_age_days, now=now)
    return evaluate_doc(path, max_age_days=max_age_days, now=now)


def run(max_age_days: float) -> dict:
    now = dt.datetime.now(dt.timezone.utc)
    records = [
        evaluate_artifact(p, max_age_days=max_age_days, now=now)
        for p in _iter_perf_artifacts()
    ]
    records.extend(
        evaluate_scoreboard(p, max_age_days=max_age_days, now=now)
        for p in _iter_scoreboard_artifacts()
    )
    hazards = [r for r in records if r["hazard"]]
    docs_scanned = sum(1 for r in records if r["artifact_kind"] == "text")
    json_scanned = sum(1 for r in records if r["artifact_kind"] == "json")
    scoreboards_scanned = sum(1 for r in records if r["artifact_kind"] == "scoreboard")
    scoreboard_support_scanned = sum(
        1 for r in records if r["artifact_kind"] == "scoreboard-support"
    )
    return {
        "kind": "perf_freshness",
        "max_age_days": max_age_days,
        "canonical_gate": pa.CANONICAL_GATE,
        "artifacts_scanned": len(records),
        "docs_scanned": docs_scanned,
        "json_scanned": json_scanned,
        "scoreboards_scanned": scoreboards_scanned,
        "scoreboard_support_scanned": scoreboard_support_scanned,
        "hazards": len(hazards),
        "records": records,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--json", action="store_true", help="machine-readable report")
    parser.add_argument(
        "--max-age-days",
        type=float,
        default=pa.DEFAULT_STALE_DAYS,
        help=f"staleness horizon in days (default: {pa.DEFAULT_STALE_DAYS})",
    )
    ns = parser.parse_args(argv)
    report = run(ns.max_age_days)

    if ns.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(
            f"perf-freshness: scanned {report['artifacts_scanned']} perf "
            f"artifact(s) ({report['docs_scanned']} text, "
            f"{report['json_scanned']} JSON, "
            f"{report['scoreboards_scanned']} scoreboard, "
            f"{report['scoreboard_support_scanned']} scoreboard-support); "
            f"{report['hazards']} perf authority hazard(s)."
        )
        for rec in report["records"]:
            if rec["hazard"]:
                why = "; ".join(rec["reasons"])
                print(f"  HAZARD  {rec['path']}: {why}")
                print(
                    "          -> stamp it with the stale perf authority "
                    "acknowledgement (perf_authority.STALE_BANNER or "
                    f"{pa.STALE_METADATA_KEY}) or delete it. "
                    f"Cite only `{report['canonical_gate']}`."
                )
        if report["hazards"] == 0:
            print(
                "  OK: every perf artifact presenting numbers is fresh or "
                "stamped stale."
            )

    return 1 if report["hazards"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
