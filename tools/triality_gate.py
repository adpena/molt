#!/usr/bin/env python3
"""Triality drift gate (APPARATUS Wave 2, A3) -- M63 made mechanical.

molt's campaign, like pact's, is ONE object seen through three legs; a finding is
only "known" when all three agree, and drift between them is the campaign-level
form of forgetting (M63). This gate is the structural backstop pointed at the
commit window ``last_head..HEAD``:

  DAG / trajectory   -> ``memory/`` topic files + ``docs/agent/`` ledgers
  DSL / dispatch     -> gates + generated-authority registries (op_kinds,
                        op_family, ``*_registry.toml``)
  Equations / law    -> gates + attested measurements (bench_evidence /
                        PERF_AUTHORITY / a test that pins the behavior)

It classifies commit MESSAGES (not raw diffs -- see ``_common.git_window_messages``
for why) into three leg-requiring classes and blocks when the required leg was
NOT touched this window:

  * BUG-CLASS  (``fail-closed|poison|silent|wrong answer|truncat|miscompile|UAF|
    corruption``) on a code fix MUST touch a gate / registry / authority / test
    -- "you fixed an INSTANCE; where is the class extinction?" (M16, M32-M45).
  * PERF-CLAIM (a measured claim: ``speedup|faster|<N>x|ns/op|throughput``) MUST
    touch a bench_evidence / PERF_AUTHORITY attestation -- no UNMEASURED perf
    lands (M10). (The bare ``perf(...)`` commit *type* is deliberately NOT the
    trigger: it labels build-infra / gate / refactor commits too. The trigger is
    a stated NUMBER or comparison -- calibrated against the live window.)
  * FINDING   (``falsified|root-cause|discovered``) attached to a code change
    MUST record the lesson in a ledger / memory / findings registry -- where the
    next session reads it (M63, M12). (``landed`` is deliberately dropped: M12
    makes "LANDED" boilerplate on ~every commit -- it would storm.)

ESCAPE VALVE ("never binary"): ``[no-triality]`` / ``[skip-triality]`` /
``[skip-drift]`` in a commit subject opts the whole window out -- but ONLY with a
real trailing rationale (>=4 non-placeholder chars); a bare token is refused. The
honored waiver is logged to ``.molt/state/waivers.jsonl``.

INVARIANTS (identical to the Wave-1 legs): PURE unit-tested classifier; fail-open
wrapper (a raising core -> allow); event-triggered (silent when no new commits);
loop-safe / block-once (per-leg ``EventWindow`` marker); Windows-safe + ASCII.
Composes with the Wave-1 legs under ``stop_gates`` -- one leg's exception can
never sink the others.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

try:
    from tools.hooks import _common
    from tools.hooks.waivers import is_valid_rationale, record_waiver
except Exception:  # pragma: no cover - path-invocation fallback
    import os as _os

    sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.abspath(__file__))))
    from tools.hooks import _common
    from tools.hooks.waivers import is_valid_rationale, record_waiver


MARKER_NAME = "triality_gate_marker.json"


# ============================ classifier vocabulary ============================
# Matched against commit MESSAGES (subject+body). Calibrated against the live
# molt window (docs/agent/APPARATUS_FROM_COMMA_LAB.md, A3): a bug-CLASS word in a
# message is common (POISON is a whole lane name), so the leg only fires when a
# CODE FIX changed source WITHOUT a class net -- a docs/ledger commit that merely
# NAMES the class is recording, not fixing, and is handled by the finding leg.

# The dangerous silent-wrong / memory-safety bug classes (M32-M45). ``.`` between
# words matches a space/hyphen/underscore so "fail closed"/"fail-closed"/
# "fail_closed" all hit; "fail open" does NOT (a different, benign construct).
BUG_CLASS = re.compile(
    r"\b(?:fail.?clos\w*|poison\w*|silent\w*|wrong.?answer\w*|truncat\w*|"
    r"miscompile\w*|mis-?compil\w*|\bUAF\b|use-?after-?free|corrupt\w*|"
    r"silently[-\s]?wrong|silent[-\s]?(?:drop|miscompile|truncation))",
    re.IGNORECASE,
)

# A MEASURED perf CLAIM -- a stated number or comparison, not the ``perf`` type.
#   * speedup / faster / slower(-by) / throughput(+gain)
#   * an "<N>x" / "<N>.<M>x" / "<N>×" multiplier (a version token like "v2.0x"
#     stays silent: the number must start at a word boundary, not after "v"/digit)
#   * a "<N> ns/op" | "ns/iter" | "us/" | "ms/" rate
PERF_CLAIM = re.compile(
    r"(?:\bspeed-?up\w*|\bfaster\b|\bslower\b|\bthroughput\b[^\n]{0,24}"
    r"(?:gain|improv|up|higher|\+)|\b\d[\d.]*\s?[x×]\b|"
    r"\b\d[\d.]*\s?(?:ns|us|µs|ms)\s?/\s?(?:op|iter|call|elem|row)|"
    r"\bregress\w*\s+by\b|\b\d[\d.]*\s?%\s+(?:faster|slower|speedup))",
    re.IGNORECASE,
)

# A FINDING announcement -- a discovery / root-cause / falsification. ``landed``
# is intentionally excluded (M12 boilerplate). ``discover\w*`` is NOT used: it
# would hit the "discovery" lane/dir name; only past-tense "discovered" (the
# announcement of a fact) counts.
FINDING_CLASS = re.compile(
    r"\b(?:falsified|falsif\w*\s+(?:the|that)|root.?cause[ds]?|root-?caused|"
    r"discovered|mis-?diagnos\w*|the\s+bug\s+was|turned\s+out\s+to\s+be)\b",
    re.IGNORECASE,
)

# --- file-path classifiers (which triality leg a changed file belongs to) ---
_GATE_FILE = re.compile(r"(?:^|/)tools/[^/]*gate[^/]*\.py$", re.IGNORECASE)
_REGISTRY_FILE = re.compile(
    r"(?:_registry\.(?:toml|py)$|(?:^|/)op_kinds\.toml$)", re.IGNORECASE
)
_AUTHORITY_FILE = re.compile(
    r"(?:op_family\.rs$|op_kinds(?:_generated)?\.(?:rs|py)$|repr_facts)",
    re.IGNORECASE,
)
_TEST_FILE = re.compile(
    r"(?:(?:^|/)tests?/|(?:^|/)test_[^/]*\.(?:py|rs)$|[^/]*_test\.(?:py|rs)$|"
    r"(?:^|/)test_[^/]*\.py$)",
    re.IGNORECASE,
)
_PERF_ATTEST_FILE = re.compile(
    r"(?:bench_evidence|perf_authority|PERF_AUTHORITY|(?:^|/)bench/|"
    r"perf.*\.(?:json|toml)$)",
    re.IGNORECASE,
)
_FINDING_NET_FILE = re.compile(
    r"(?:(?:^|/)docs/agent/[^/]*\.md$|(?:^|/)memory/[^/]*\.md$|"
    r"findings_registry\.jsonl$|(?:^|/)MEMORY\.md$|(?:^|/)POINTERS\.md$)",
    re.IGNORECASE,
)
_CODE_SUFFIX = re.compile(r"\.(?:rs|py|c|h|cc|cpp|pyx|pyi|toml)$", re.IGNORECASE)
_DOC_FILE = re.compile(r"\.(?:md|rst|txt)$", re.IGNORECASE)

_SKIP_TOKEN = re.compile(r"\[(no-triality|skip-triality|skip-drift)\]", re.IGNORECASE)


# ----------------------------- pure logic (tested) -----------------------------
def _any(pat: re.Pattern[str], messages: list[str]) -> bool:
    return any(pat.search(str(m or "")) for m in (messages or []))


def touched_class_net(files: list[str]) -> bool:
    """True if the window touched a gate / registry / authority / test (the leg a
    bug-class fix must extend to extinguish the class, not just the instance)."""
    return any(
        _GATE_FILE.search(f)
        or _REGISTRY_FILE.search(f)
        or _AUTHORITY_FILE.search(f)
        or _TEST_FILE.search(f)
        for f in (str(x or "") for x in (files or []))
    )


def touched_perf_attest(files: list[str]) -> bool:
    """True if the window touched a bench_evidence / PERF_AUTHORITY attestation."""
    return any(_PERF_ATTEST_FILE.search(str(f or "")) for f in (files or []))


def touched_finding_net(files: list[str]) -> bool:
    """True if the window recorded to a ledger / memory / findings registry."""
    return any(_FINDING_NET_FILE.search(str(f or "")) for f in (files or []))


def touched_code(files: list[str]) -> bool:
    """True if the window changed a source file (a real FIX / change happened).

    A pure docs-only commit changes no code -- it is recording, not fixing, and
    so is exempt from the bug-class / finding-class code-fix requirements.
    """
    for f in (str(x or "") for x in (files or [])):
        if _CODE_SUFFIX.search(f) and not _DOC_FILE.search(f):
            return True
    return False


def opt_out_rationale(subjects: list[str]) -> str | None:
    """The trailing rationale of a ``[no-triality]``/``[skip-*]`` opt-out, or None.

    Window-wide (fail-SAFE: never over-nag). Honored ONLY with a real rationale
    (>=4 non-placeholder chars after the token is removed); a bare token is
    refused so the opt-out always carries a reason.
    """
    for s in (str(x or "") for x in (subjects or [])):
        m = _SKIP_TOKEN.search(s)
        if not m:
            continue
        # The rationale must FOLLOW the token (``[no-triality] <real reason>``);
        # the surrounding subject prose does NOT count, so a bare token is refused.
        rest = s[m.end() :].strip(" -:.\t")
        if is_valid_rationale(rest):
            return rest
    return None


def window_drift(subjects: list[str], files: list[str]) -> list[tuple[str, str]]:
    """The PURE classifier: ``[(leg, reason), ...]`` for every leg that drifted.

    Empty == the window is triality-consistent. ``subjects`` = commit messages,
    ``files`` = changed paths. No I/O; unit-tested without a crafted git HEAD.
    """
    out: list[tuple[str, str]] = []
    code = touched_code(files)

    if _any(BUG_CLASS, subjects) and code and not touched_class_net(files):
        out.append(
            (
                "bug-class",
                "a bug-class fix (fail-closed / poison / silent-wrong / truncation "
                "/ miscompile / UAF / corruption) landed WITHOUT touching a gate, "
                "registry, authority, or test. You fixed an INSTANCE; where is the "
                "class extinction? Land the regression net (a *_gate.py / "
                "*_registry.toml / op_kinds|op_family authority / a test that fails "
                "on the un-fixed code) so the class cannot silently return (M16, "
                "M32-M45).",
            )
        )

    if _any(PERF_CLAIM, subjects) and not touched_perf_attest(files):
        out.append(
            (
                "perf-claim",
                "a measured perf CLAIM (speedup / <N>x / ns-per-op / throughput) "
                "landed WITHOUT a bench_evidence or PERF_AUTHORITY attestation. No "
                "UNMEASURED perf lands: attach the before/after measurement on the "
                "real authority (tools/bench_evidence.py, tools/PERF_AUTHORITY.md) "
                "with the noise floor (M10).",
            )
        )

    if _any(FINDING_CLASS, subjects) and code and not touched_finding_net(files):
        out.append(
            (
                "finding",
                "a finding (falsified / root-cause / discovered) landed in code "
                "WITHOUT recording the lesson where the next session reads it. Add "
                "it to a docs/agent ledger, a memory/ topic file, or the findings "
                "registry -- a lesson only in a commit body silently rots (M63, "
                "M12).",
            )
        )
    return out


def build_reason(drift: list[tuple[str, str]]) -> str:
    """The one firm re-engage nudge naming every drifting leg + the exact fix."""
    legs = ", ".join(leg for leg, _ in drift)
    body = " ".join(f"[{leg}] {reason}" for leg, reason in drift)
    return (
        f"Triality drift ({legs}): a substantive commit this turn kept the "
        f"knowledge legs OUT of sync (M63 -- the campaign form of forgetting). "
        f"{body} Deliberate exception: '[no-triality] <real reason>' in a commit "
        f"subject (window-wide, logged)."
    )


# --- fact gathering + evaluate (called by stop_gates) ----------------------


def evaluate(data: dict, root: Path) -> str | None:
    """Compute window facts, apply block-once, return a block reason or None.

    Owns its OWN ``EventWindow`` marker so ``stop_gates`` only prints the JSON.
    Fail-open by construction: every gather step is best-effort (uncertainty ->
    empty -> no drift -> allow).
    """
    head = _common.git_head(root)
    win = _common.EventWindow(root, MARKER_NAME)
    base = win.base(head, bool(data.get("stop_hook_active")))
    if base is None:
        return None

    subjects = _common.git_window_subjects(root, base)
    files = _common.git_window_files(root, base)

    rationale = opt_out_rationale(subjects)
    if rationale is not None:
        record_waiver(
            "triality_gate",
            rationale,
            source="commit-token",
            context="[no-triality] window opt-out",
            root=root,
        )
        win.allow(head)  # opt-out is a deliberate window; advance past it
        return None

    drift = window_drift(subjects, files)
    if not drift:
        win.allow(head)
        return None

    win.block(head)  # hold base so the fix lands in-window; block-once via marker
    return build_reason(drift)


# --------------------------------- CI self-test --------------------------------
# ``--check`` is the falsifiable CI mode (wired into ci_gate tier-1): it feeds the
# PURE classifier known-BAD and known-GOOD fixtures and fails (exit 1) if the gate
# no longer FIRES on drift or no longer PASSES a consistent window -- the "a gate
# that cannot fail certifies nothing" meta-bug (M34/M42), pointed at this gate.

_SELFTEST_CASES: list[tuple[str, list[str], list[str], bool]] = [
    # (name, subjects, files, expect_drift)
    (
        "bug-fix-without-net-fires",
        ["fix(runtime): PyLong_AsLong silent truncation on >2**46 (POISON #7)"],
        ["runtime/molt-lang-cpython-abi/src/numbers.rs"],
        True,
    ),
    (
        "bug-fix-with-test-passes",
        ["fix(runtime): PyLong_AsLong silent truncation on >2**46"],
        [
            "runtime/molt-lang-cpython-abi/src/numbers.rs",
            "runtime/molt-lang-cpython-abi/tests/test_long_conversions.rs",
        ],
        False,
    ),
    (
        "bug-fix-with-gate-passes",
        ["fix: miscompile in loop-IV modulo"],
        ["runtime/src/lowering.rs", "tools/fail_closed_gate.py"],
        False,
    ),
    (
        "docs-only-poison-ledger-passes",
        ["docs(agent): poison-orphan ledger -- 32 audited findings"],
        ["docs/agent/POISON_ORPHAN_LEDGER.md"],
        False,
    ),
    (
        "perf-claim-without-bench-fires",
        ["perf(runtime): int-mul CheckedMul peel -- 1.65x CPython"],
        ["runtime/src/arith.rs"],
        True,
    ),
    (
        "perf-claim-with-bench-passes",
        ["perf(runtime): int-mul peel -- 1.65x CPython"],
        ["runtime/src/arith.rs", "tools/bench_evidence.py"],
        False,
    ),
    (
        "finding-in-code-without-ledger-fires",
        ["fix: root-cause of the stale module listing was an NTFS mtime cache"],
        ["src/molt/frontend/module_resolution.py"],
        True,
    ),
    (
        "finding-with-ledger-passes",
        ["fix: root-cause of stale module listing (NTFS mtime cache)"],
        [
            "src/molt/frontend/module_resolution.py",
            "docs/agent/REVIEW_FINDINGS_20260708.md",
        ],
        False,
    ),
    (
        "clean-refactor-passes",
        ["refactor(runtime): split handle_call_op into 12 helpers"],
        ["runtime/src/function_compiler.rs"],
        False,
    ),
    (
        "opt-out-with-reason-passes",
        ["fix: silent truncation [no-triality] mechanical revert, net lands next"],
        ["runtime/src/numbers.rs"],
        False,  # honored via opt_out_rationale (checked separately below)
    ),
]


def _run_selftest() -> tuple[int, list[str]]:
    failures: list[str] = []
    for name, subjects, files, expect in _SELFTEST_CASES:
        if name == "opt-out-with-reason-passes":
            ok = opt_out_rationale(subjects) is not None
            got = not ok  # "drift stands" == opt-out NOT honored
        else:
            got = bool(window_drift(subjects, files))
        if got != expect:
            failures.append(f"{name}: expected drift={expect}, got drift={got}")
    # bare opt-out token (no reason) must NOT be honored
    if opt_out_rationale(["fix: x [no-triality]"]) is not None:
        failures.append("bare-opt-out-token: honored without a rationale (must not)")
    return (1 if failures else 0), failures


# ------------------------------ live-window report -----------------------------


def _window_report(root: Path, n: int) -> int:
    """Report which of the last ``n`` commits WOULD fire, per leg (calibration)."""

    def _sh(args: list[str]) -> str:  # UTF-8 safe (M43) -- em-dash subjects/diffs
        return _COMMANDS.run(
            args,
            cwd=str(root),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        ).stdout

    base = _sh(["git", "rev-parse", f"HEAD~{n}"]).strip()
    shas = _common.git_window_shas(root, base)
    counts = {"bug-class": 0, "perf-claim": 0, "finding": 0}
    fired: list[str] = []
    for sha in shas:
        # per-commit window = that single commit; SUBJECT line only (the surface
        # the live gate classifies -- bodies over-fire).
        one = _sh(["git", "log", "-1", "--format=%s", sha]).splitlines()
        files = _sh(["git", "show", "--name-only", "--format=", sha]).splitlines()
        subj = [x for x in one if x.strip()]
        fl = [x.strip() for x in files if x.strip()]
        if opt_out_rationale(subj) is not None:
            continue
        drift = window_drift(subj, fl)
        for leg, _ in drift:
            counts[leg] += 1
        if drift:
            tags = ",".join(leg for leg, _ in drift)
            fired.append(f"  {sha[:9]} [{tags}] {subj[0][:70]}")
    total = sum(counts.values())
    print(f"triality_gate live-window report over last {len(shas)} commits:")
    print(f"  per-leg fires: {json.dumps(counts)}  (total {total})")
    for line in fired:
        print(line)
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="triality_gate", description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="CI self-test: fail (exit 1) if the classifier no longer fires on "
        "drift or no longer passes a consistent window",
    )
    ap.add_argument(
        "--window",
        type=int,
        metavar="N",
        help="calibration: report per-leg fires over the last N commits",
    )
    args = ap.parse_args(argv)

    if args.window:
        return _window_report(_common.repo_root(), args.window)

    code, failures = _run_selftest()
    if failures:
        for f in failures:
            print(f"  [DEAD] triality_gate self-test: {f}")
        print(
            f"\n{len(failures)} triality_gate self-test(s) FAILED -- the gate has "
            "silently rotted (M34/M42). Fix it before trusting it."
        )
    else:
        print(f"All {len(_SELFTEST_CASES) + 1} triality_gate self-tests pass.")
    return code if args.check else 0


if __name__ == "__main__":
    sys.exit(main())
