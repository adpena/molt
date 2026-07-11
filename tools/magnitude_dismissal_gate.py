#!/usr/bin/env python3
"""Magnitude-dismissal + verdict-scope gate (APPARATUS Wave 2, A6).

Two pure classifiers over the commit window ``last_head..HEAD``, wired as a second
``stop_gates`` leg (docs/agent/APPARATUS_FROM_COMMA_LAB.md, A6). Ports pact's
``magnitude_dismissal_detector.py`` + the verdict-scope leg of
``triality_drift_detector.py`` -- their predicates were already pure and reusable.

1. MAGNITUDE-DISMISSAL. Dismissing a lane/finding as "small / weak / noise / not
   worth it / don't re-chase" -- in a commit message or a ``docs/agent`` ledger --
   requires one of:
     (a) RELATIVE-significance math: the delta as a FRACTION of the REMAINING gap
         to the standing goal (100-yr witness green; the build-time floor M09; the
         parity subset M02) -- an absolute that is a meaningful % of the remaining
         gap is NOT weak;
     (b) a cited MEASUREMENT of un-recoverability (noise floor / exit criterion /
         structurally superseded); or
     (c) a same-line ``# MAGNITUDE_DISMISSAL_OK:<rationale>`` waiver.
   This mechanizes molt's recurring "DON'T re-chase" lines (M46/M47) -- those are
   only legitimate WITH the relative-significance number, else they silently
   orphan a lever that is small in absolute terms but large versus the remaining
   gap. There is NO local model on the molt fleet, so this is the deterministic
   high-precision classifier -- honestly labeled "semantic confirmation owed",
   never a faked FM call (the A5 advisory layer is a later wave).

2. VERDICT-SCOPE. Any negative verdict (``KILLED / FALSIFIED / REFUTED / WONTFIX /
   NO-GO / DEAD / INERT``) in a ``docs/agent`` ledger must declare
   ``verdict_scope: instance|formulation|family|paradigm``. A FAMILY kill needs a
   citation/theorem OR >=2 structurally-distinct falsified formulations; a KILL at
   scope >= formulation names its reformulation queue. This mechanizes "fix/kill
   the CLASS not the instance -- but PROVE which one you killed" (M16), and its
   dual "don't kill the family on one formulation's evidence" (M11). molt's
   "DON'T re-chase" keystones (M46/M47) are ``verdict_scope: instance`` records.

INVARIANTS (identical to the Wave-1 legs + triality_gate): PURE unit-tested
classifiers; fail-open wrapper (a raising core -> allow); event-triggered (silent
when no new commits); loop-safe / block-once (per-leg ``EventWindow`` marker);
Windows-safe + ASCII. The two sub-checks compose independently -- either can fire.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

try:
    from tools.hooks import _common
    from tools.hooks.waivers import is_valid_rationale, record_waiver
except Exception:  # pragma: no cover - path-invocation fallback
    import os as _os

    sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.abspath(__file__))))
    from tools.hooks import _common
    from tools.hooks.waivers import is_valid_rationale, record_waiver


MARKER_NAME = "magnitude_dismissal_marker.json"


# ==================== magnitude-dismissal classifier vocabulary ====================
# The DECISION: a defer / downgrade / orphan / kill / drop / shelve of a lever or
# finding. Only ever fires in CO-OCCURRENCE with a magnitude word, which is what
# makes the pair specific. Ported from pact + molt's own "don't re-chase" idiom.
_DISMISSAL = re.compile(
    r"\b(?:defer(?:red|ring|s)?|downgrad(?:e|ed|es|ing)|orphan(?:ed|ing|s)?|"
    r"kill(?:ed|ing|s)?|shelv(?:e|ed|es|ing)|de-?prioriti[sz]e[ds]?|"
    r"park(?:ed|ing)?|abandon(?:ed|ing|s)?|set\s+aside|table\s+(?:it|this)|"
    r"not\s+worth(?:\s+(?:it|the|pursuing|chasing))?|"
    r"do(?:n'?t|\s+not)\s+re-?chase|won'?t\s+re-?chase|no\s+need\s+to\s+re-?chase|"
    r"leave\s+(?:it|this)\s+(?:for\s+)?later|deprecat(?:e|ed|es|ing)|"
    r"skip(?:ped|ping)?|(?:de-?)?scope\s+out|move\s+on\s+from)\b",
    re.IGNORECASE,
)
# The JUSTIFICATION: an ABSOLUTE-magnitude smallness claim.
_MAGNITUDE = re.compile(
    r"\b(?:weak(?:ly)?|negligibl[ey]|insignificant(?:ly)?|tiny|minimal|"
    r"trivial(?:ly)?|marginal(?:ly)?|too\s+small|so\s+small|"
    r"small\s+(?:delta.?s?|gain|effect|impact|improvement|margin|win)|"
    r"little\s+to\s+gain|not\s+much(?:\s+to\s+gain)?|barely|diminishing|"
    r"nois[ey](?![-\s]?floor)|not\s+significant|hardly\s+moves|"
    r"already\s+(?:fast|green|good)\s+enough)\b",
    re.IGNORECASE,
)
# EXEMPTION (a): a RELATIVE-significance computation is present.
_RELATIVE_SIG = re.compile(
    r"(?:relative\s+significance|remaining\s+(?:gap|distance|descent|budget)|"
    r"gap.?to.?(?:target|goal|green|floor)|fraction\s+of\s+the\s+remaining|"
    r"%\s+of\s+the\s+(?:remaining|gap|target|budget)|"
    r"of\s+the\s+(?:remaining|target)\s+(?:gap|budget|distance)|"
    r"delta.?s?\s*/|s_current|s_target|/\s*\(?\s*s_?current|"
    r"per-?cent\s+of\s+the\s+remaining|[0-9]+\s*[-]?\s*[0-9]*\s*%\s+of\s+the)",
    re.IGNORECASE,
)
# EXEMPTION (b): a MEASURED un-recoverability citation, or structurally superseded.
_MEASURED_UNRECOVERABLE = re.compile(
    r"(?:un-?recoverab\w+|irreducibl\w+|noise\s+floor|information[-\s]?floor|"
    r"exit\s+criterion|measured\s+(?:no-?go|un-?recoverab\w+|ceiling|floor|"
    r"unachievab\w+)|MEASURED\s+NO-?GO|cannot\s+be\s+(?:predicted|recovered)|"
    r"structurally\s+supersed\w+|supersed(?:e|ed|es|ing)\s+by|at\s+the\s+"
    r"noise\s+floor|hardware\s+floor|inherent\s+floor)",
    re.IGNORECASE,
)
# FALSE-POSITIVE magnitude usages that are not dismissals of a lever at all.
_LEGIT_NONDISMISSAL = re.compile(
    r"(?:weak\s+supervis|weakly[-\s]?supervis|weakly[-\s]?driven|"
    r"noise\s+inject|sigma\s+noise|gaussian\s+noise|signal[-\s]?to[-\s]?noise|"
    r"random[-\s]?walk|quantization\s+noise|weak\s+lensing|weak\s+form|"
    r"weak\s+reference|weak\s+symbol|weak\s+ptr|weak_ptr|weakref)",
    re.IGNORECASE,
)
# Per-line deliberate waiver; validity via the shared waivers classifier.
_MAG_WAIVER = re.compile(r"#\s*MAGNITUDE_DISMISSAL_OK\s*:\s*(\S.*)")
# Window-wide opt-out token.
_MAG_SKIP = re.compile(
    r"\[(magnitude-ok|skip-magnitude|skip-magnitude-dismissal)\]", re.IGNORECASE
)
# Rule-discussion / quoting cues: a line ABOUT the class, not committing it.
_MAG_DISCUSSION = re.compile(
    r"magnitude[-\s]?dismissal|relative[-\s]not[-\s]absolute|"
    r"\bnot\s+a\s+(?:defer|dismiss|kill)|never\s+dismiss|do\s+not\s+dismiss|"
    r"reopen|re-?open|re-?rank|re-?audit",
    re.IGNORECASE,
)


def _has_valid_mag_waiver(line: str) -> bool:
    m = _MAG_WAIVER.search(str(line or ""))
    return bool(m) and is_valid_rationale(m.group(1).strip())


def mag_is_opted_out(subjects: list[str]) -> bool:
    """Window-wide opt-out via [magnitude-ok]/[skip-magnitude(-dismissal)]."""
    return any(_MAG_SKIP.search(str(s or "")) for s in (subjects or []))


def _mag_line_exempt(passage: str, wide: str) -> bool:
    if _MAG_WAIVER.search(passage):
        return _has_valid_mag_waiver(passage)
    if str(passage or "").lstrip().startswith(">"):
        return True  # markdown quote -- quoting a prior verdict, not issuing one
    return bool(
        _RELATIVE_SIG.search(wide)
        or _MEASURED_UNRECOVERABLE.search(wide)
        or _LEGIT_NONDISMISSAL.search(wide)
        or _MAG_DISCUSSION.search(wide)
    )


_TABLE_LINE = re.compile(r"^\s*\|")  # a markdown table row / separator


def _blank_table_lines(lines: list[str]) -> list[str]:
    """Blank markdown table rows (``| ... |``) to "" (preserving indices).

    molt's docs/agent ledgers include DENSE tables (the C-API coverage matrix,
    the CLAIMS ledger) whose cells scatter words like "trivial"/"skip"/"minimal"
    across columns; a 3-line window straddling table rows spuriously co-locates a
    dismissal + magnitude word (measured on the live window -- the ONLY residual
    false-positive class). A genuine magnitude-dismissal is PROSE (it must carry
    relative-significance math, which cannot live in a table cell), so structured
    table rows are removed from the magnitude corpus -- fixing the class, not the
    instance."""
    return [
        "" if _TABLE_LINE.match(str(ln or "")) else str(ln or "")
        for ln in (lines or [])
    ]


def magnitude_dismissal_candidates(lines: list[str]) -> list[dict]:
    """Deterministic PRE-FILTER. A 3-line window is a CANDIDATE iff it co-locates a
    dismissal verb AND a magnitude word AND is not exempt (relative-sig /
    measured-un-recoverability / non-dismissal / waiver / discussion) within a
    wider 5-line window. Adjacent candidates collapse so one dismissal reports
    once. Returns ``[{line_no, passage}]``. Never raises on odd input."""
    lines = _blank_table_lines(lines)
    out: list[dict] = []
    last_center = -10
    for i in range(len(lines)):
        passage = "\n".join(lines[max(0, i - 1) : i + 2])
        if not (_DISMISSAL.search(passage) and _MAGNITUDE.search(passage)):
            continue
        wide = "\n".join(lines[max(0, i - 2) : i + 3])
        if _mag_line_exempt(passage, wide):
            continue
        if i - last_center <= 2:
            continue
        last_center = i
        snippet = " ".join(p.strip() for p in passage.splitlines() if p.strip())
        out.append({"line_no": i + 1, "passage": snippet[:200]})
    return out


def magnitude_flags(lines: list[str], source: str = "") -> list[str]:
    """Violation messages for one source's ``lines`` (the SoT a static sister would
    reuse). Each names ``source:line`` + the quoted passage + the exact fix."""
    src = str(source or "?")
    return [
        f"{src}:{c['line_no']}: magnitude-based dismissal without relative "
        f'significance or a measured-un-recoverability citation -- "{c["passage"]}"'
        for c in magnitude_dismissal_candidates(lines)
    ]


# ========================= verdict-scope classifier vocabulary =========================
# Which docs/agent ledgers carry verdicts (decision surfaces), and which are pure
# rule/reference/index docs that QUOTE verdicts (scanning them self-trips).
VERDICT_DOC_PAT = re.compile(
    r"docs/agent/.*(?:LEDGER|FINDINGS|REVIEW|FRONTIER|DISCOVERY|PROOF_QUEUE|"
    r"POISON|PANIC|HOTSPOT|verdict|decision|crucible|council)",
    re.IGNORECASE,
)
VERDICT_DOC_EXEMPT = re.compile(
    r"(?:APPARATUS_FROM_COMMA_LAB|MEMORY|POINTERS|ORCHESTRATION|CLAUDE\.full|"
    r"AGENTS\.full|verdict[-_]scope|COVERAGE_MATRIX|CONTRACT_MATRIX|_REFERENCE)",
    re.IGNORECASE,
)
# Load-bearing negative-verdict tokens. Single words CASE-SENSITIVE UPPERCASE (a
# real verdict is written emphatically: "Lever-D FALSIFIED", "INERT"); lowercase
# prose ("killed the process", "dead code") stays silent.
_NEG_VERDICT_CS = re.compile(
    r"\b(?:KILL(?:ED|S)?|NO[-_]GO|FALSIFIED|REFUTED|DEAD|INERT|WONTFIX|WON'?T\s?FIX)\b"
)
_NEG_VERDICT_CI = re.compile(
    r"\b(?:family(?:\s+is)?\s+dead|at\s+chance|does\s+not\s+work|"
    r"is\s+a\s+dead\s+end)\b",
    re.IGNORECASE,
)
_KILL_CLASS = re.compile(r"\b(?:KILL(?:ED|S)?|NO[-_]GO|WONTFIX|WON'?T\s?FIX)\b")

_SCOPE_DECL = re.compile(
    r"verdict[_-]scope\s*[:=]\s*(instance|formulation|family|paradigm)", re.IGNORECASE
)
_FAMILY_EVIDENCE = re.compile(
    r"(?:arxiv|\bdoi\b|theorem|impossibility\s+bound|"
    r"(?:>=|>)\s*2\s+(?:\w+\s+)?formulations|two\s+(?:\w+\s+)?(?:distinct\s+)?"
    r"formulations|2\+\s+(?:\w+\s+)?formulations)",
    re.IGNORECASE,
)
_REFORMULATION = re.compile(
    r"(?:reformulation|untested\s+formulations|alternatives\s*:)", re.IGNORECASE
)
_SCOPE_DECL_SPAN = re.compile(
    r"verdict[_-]scope\s*[:=]\s*(?:instance|formulation|family|paradigm)",
    re.IGNORECASE,
)
_SCOPE_WAIVER = re.compile(r"#\s*VERDICT_SCOPE_OK\s*:\s*(\S.*)")
_NEG_DISCUSSION = re.compile(
    r"\b(?:not\s+a|never|no\s+longer|reopened?|over-?scop\w*|premature\w*|"
    r"prior\s+verdict|previous(?:ly)?|was\s+read\s+as|had\s+been|instead\s+of|"
    r"rather\s+than|would\s+(?:be|have)|avoid\w*)\b",
    re.IGNORECASE,
)
_QUOTED_SPANS = re.compile(r"`[^`]*`|\"[^\"]*\"")


def _has_valid_scope_waiver(line: str) -> bool:
    m = _SCOPE_WAIVER.search(str(line or ""))
    return bool(m) and is_valid_rationale(m.group(1).strip())


def _verdict_line_exempt(line: str) -> bool:
    s = str(line or "").strip()
    if s.startswith(">"):
        return True
    if _SCOPE_WAIVER.search(s):
        return _has_valid_scope_waiver(s)
    low = s.lower()
    if "verdict_scope" in low or "verdict-scope" in low:
        return True
    return bool(_NEG_DISCUSSION.search(s))


def negative_verdict_tokens(added_lines: list[str]) -> tuple[list[str], bool]:
    """(tokens, kill_class_present) over non-exempt added lines, with quoted/backtick
    spans stripped so a quoted prior verdict stays silent."""
    tokens: list[str] = []
    kill = False
    for raw in added_lines or []:
        # Strip an inline ``verdict_scope: <level>`` declaration FIRST so a
        # same-line "X FALSIFIED verdict_scope: family" still exposes its verdict
        # token (the scope itself is recovered separately from the full text).
        line = _SCOPE_DECL_SPAN.sub(" ", str(raw or ""))
        if _verdict_line_exempt(line):
            continue
        searchable = _QUOTED_SPANS.sub("", line)
        hits = [m.group(0) for m in _NEG_VERDICT_CS.finditer(searchable)]
        hits += [m.group(0) for m in _NEG_VERDICT_CI.finditer(searchable)]
        if hits:
            tokens.extend(hits)
            if _KILL_CLASS.search(searchable):
                kill = True
    return tokens, kill


def verdict_scope_violations(path: str, added_lines: list[str]) -> list[str]:
    """Deterministic (BLOCKING) verdict-scope checks for ONE doc's added lines.
    Returns violation messages (empty == compliant); each shows the one-line fix."""
    tokens, kill_present = negative_verdict_tokens(added_lines)
    if not tokens:
        return []
    text = "\n".join(str(ln or "") for ln in added_lines)
    scopes = [m.group(1).lower() for m in _SCOPE_DECL.finditer(text)]
    if not scopes:
        uniq = sorted(set(tokens))[:4]
        return [
            f"{path}: negative-verdict token(s) {uniq} without a verdict_scope "
            "declaration -- add 'verdict_scope: formulation -- <which formulation>' "
            "(or instance/family/paradigm; NARROWEST level the evidence supports)"
        ]
    viol: list[str] = []
    if any(s == "family" for s in scopes) and not _FAMILY_EVIDENCE.search(text):
        viol.append(
            f"{path}: 'verdict_scope: family' requires a citation (arXiv/DOI/theorem) "
            "OR an explicit '>=2 structurally distinct formulations' evidence line "
            "(a family kill needs a theorem or kills across >=2 formulations; M11)"
        )
    if (
        kill_present
        and any(s in ("formulation", "family", "paradigm") for s in scopes)
        and not _REFORMULATION.search(text)
    ):
        viol.append(
            f"{path}: KILL/NO-GO at scope >= formulation requires a reformulation "
            "queue -- add 'untested formulations / alternatives: <enumerate them>'"
        )
    return viol


def verdict_doc_in_scope(path: str) -> bool:
    """True iff this changed file is a decision-class docs/agent ledger we scan."""
    f = str(path or "")
    return (
        f.endswith(".md")
        and bool(VERDICT_DOC_PAT.search(f))
        and not VERDICT_DOC_EXEMPT.search(f)
    )


# ---------------------------- window classification (pure) ----------------------------


def classify_window(subjects: list[str], doc_added: dict[str, list[str]]) -> list[str]:
    """The PURE combined classifier. ``subjects`` = commit messages;
    ``doc_added`` = {ledger_path: [added lines]}. Returns violation messages
    (empty == clean). No I/O; unit-tested without a crafted git HEAD."""
    msgs: list[str] = []
    # magnitude-dismissal: commit messages + every changed ledger's added lines
    msgs.extend(magnitude_flags(subjects, source="<commit-message>"))
    for path, lines in (doc_added or {}).items():
        msgs.extend(magnitude_flags(lines, source=path))
    # verdict-scope: only the decision-class ledgers' added lines
    for path, lines in (doc_added or {}).items():
        if verdict_doc_in_scope(path):
            msgs.extend(verdict_scope_violations(path, lines))
    return msgs


def build_reason(msgs: list[str]) -> str:
    mag = [m for m in msgs if "magnitude-based dismissal" in m]
    ver = [m for m in msgs if "verdict_scope" in m or "reformulation queue" in m]
    parts: list[str] = []
    if mag:
        preview = " | ".join(m for m in mag[:3])
        parts.append(
            "Magnitude-dismissal: this turn dismissed a lane/finding by ABSOLUTE "
            "smallness (weak/negligible/noise/small-delta/not-worth-it/don't-"
            "re-chase) WITHOUT (a) a RELATIVE-significance number (the delta as a "
            "fraction of the REMAINING gap to the goal -- 100-yr witness green / "
            "build-time floor M09 / parity subset M02), (b) a cited MEASUREMENT of "
            f"un-recoverability, or (c) a waiver. Passage(s): {preview}. Compute "
            "delta / remaining-gap and state BOTH numbers before the dismissal "
            "stands (M46/M47 'don't re-chase' lines are verdict_scope:instance "
            "records -- they need the number). Deliberate exception: same-line "
            "'# MAGNITUDE_DISMISSAL_OK:<reason>' or '[magnitude-ok]' in a commit. "
            "(Deterministic classifier; on-device semantic confirmation OWED -- the "
            "molt fleet has no local model, so this is the high-precision regex "
            "verdict, not a faked call.)"
        )
    if ver:
        preview = " | ".join(m for m in ver[:3])
        parts.append(
            "Verdict-scope: a negative verdict (KILLED/FALSIFIED/REFUTED/WONTFIX/"
            "NO-GO/DEAD/INERT) in a docs/agent ledger did not declare its scope on "
            "the instance|formulation|family|paradigm ladder (a family kill needs a "
            "theorem/citation or >=2 falsified formulations; a kill at >=formulation "
            f"names a reformulation queue). {preview}. Fix/kill the CLASS not the "
            "instance -- but PROVE which one you killed, and don't kill the family "
            "on one formulation's evidence (M16, M11)."
        )
    return "  ".join(parts)


# --- fact gathering + evaluate (called by stop_gates) ----------------------

_LEDGER_PATH = re.compile(r"(?:^|/)docs/agent/[^/]*\.md$", re.IGNORECASE)


def evaluate(data: dict, root: Path) -> str | None:
    """Compute window facts, apply block-once, return a block reason or None.

    Owns its OWN ``EventWindow`` marker. Fail-open by construction: every gather
    step is best-effort (uncertainty -> empty -> no flags -> allow)."""
    head = _common.git_head(root)
    win = _common.EventWindow(root, MARKER_NAME)
    base = win.base(head, bool(data.get("stop_hook_active")))
    if base is None:
        return None

    subjects = _common.git_window_subjects(root, base)

    if mag_is_opted_out(subjects):
        record_waiver(
            "magnitude_dismissal",
            "window opt-out token",
            source="commit-token",
            context="[magnitude-ok] window opt-out",
            root=root,
        )
        win.allow(head)
        return None

    files = _common.git_window_files(root, base)
    doc_added: dict[str, list[str]] = {}
    for f in files:
        if _LEDGER_PATH.search(f):
            diff = _common.git_window_diff(root, base, f)
            doc_added[f] = _common.added_lines_from_diff(diff)

    msgs = classify_window(subjects, doc_added)
    if not msgs:
        win.allow(head)
        return None

    win.block(head)
    return build_reason(msgs)


# --------------------------------- CI self-test --------------------------------
# ``--check`` -- the falsifiable CI mode: feed the PURE classifiers known-BAD and
# known-GOOD fixtures and fail (exit 1) if a check no longer FIRES on a violation
# or no longer PASSES a compliant window (the "a gate that cannot fail certifies
# nothing" meta-bug, M34/M42, pointed at this gate).

_MAG_BAD = [
    "the residual erasure lane gives a weak delta; not worth chasing, deferring it",
]
_MAG_GOOD_MATH = [
    "the erasure lane delta is 0.012 -- 18% of the remaining gap to green, so we "
    "keep it (relative significance, not absolute)",
]
_MAG_GOOD_WAIVER = [
    "deferring the tiny scalar-carrier win  # MAGNITUDE_DISMISSAL_OK: covered by "
    "the loop-unbox lane which subsumes it",
]
_MAG_GOOD_MEASURED = [
    "the label-noise residual is at the measured noise floor (exit criterion hit), "
    "so this lane is negligible and we stop",
]
_VER_BAD = {"docs/agent/POISON_ORPHAN_LEDGER.md": ["Lever-D is FALSIFIED, dropping it"]}
_VER_GOOD = {
    "docs/agent/POISON_ORPHAN_LEDGER.md": [
        "Lever-D FALSIFIED  verdict_scope: instance -- only the toy formulation"
    ]
}
_VER_FAMILY_BAD = {
    "docs/agent/REVIEW_FINDINGS_20260708.md": [
        "the whole approach is DEAD  verdict_scope: family"
    ]
}
_VER_KILL_NO_REFORM = {
    "docs/agent/PANIC_REACHABILITY_LEDGER.md": [
        "this path is NO-GO  verdict_scope: formulation"
    ]
}


def _run_selftest() -> tuple[int, list[str]]:
    failures: list[str] = []

    def expect(name: str, cond: bool) -> None:
        if not cond:
            failures.append(name)

    expect("mag-bad-fires", bool(classify_window(_MAG_BAD, {})))
    expect("mag-math-passes", not classify_window(_MAG_GOOD_MATH, {}))
    expect("mag-waiver-passes", not classify_window(_MAG_GOOD_WAIVER, {}))
    expect("mag-measured-passes", not classify_window(_MAG_GOOD_MEASURED, {}))
    expect("verdict-missing-scope-fires", bool(classify_window([], _VER_BAD)))
    expect("verdict-with-scope-passes", not classify_window([], _VER_GOOD))
    expect(
        "verdict-family-no-evidence-fires", bool(classify_window([], _VER_FAMILY_BAD))
    )
    expect(
        "verdict-kill-no-reformulation-fires",
        bool(classify_window([], _VER_KILL_NO_REFORM)),
    )
    # an exempt reference doc must NOT be scanned for verdict-scope
    expect(
        "exempt-doc-not-scanned",
        not classify_window(
            [], {"docs/agent/APPARATUS_FROM_COMMA_LAB.md": ["X is FALSIFIED"]}
        ),
    )
    return (1 if failures else 0), failures


# ------------------------------ live-window report -----------------------------


def _window_report(root: Path, n: int) -> int:
    import subprocess

    def _sh(args: list[str]) -> str:  # UTF-8 safe (M43) -- em-dash subjects/diffs
        return subprocess.run(
            args,
            cwd=str(root),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        ).stdout

    base = _sh(["git", "rev-parse", f"HEAD~{n}"]).strip()
    shas = _common.git_window_shas(root, base)
    mag = ver = 0
    fired: list[str] = []
    for sha in shas:
        subj = [
            x
            for x in _sh(["git", "log", "-1", "--format=%s", sha]).splitlines()
            if x.strip()
        ]
        files = _sh(["git", "show", "--name-only", "--format=", sha]).splitlines()
        if mag_is_opted_out(subj):
            continue
        doc_added: dict[str, list[str]] = {}
        for f in (x.strip() for x in files if x.strip()):
            if _LEDGER_PATH.search(f):
                d = _sh(["git", "show", sha, "--", f])
                doc_added[f] = _common.added_lines_from_diff(d)
        msgs = classify_window(subj, doc_added)
        if not msgs:
            continue
        m = sum(1 for x in msgs if "magnitude-based dismissal" in x)
        v = len(msgs) - m
        mag += 1 if m else 0
        ver += 1 if v else 0
        tag = ("mag," if m else "") + ("verdict" if v else "")
        fired.append(f"  {sha[:9]} [{tag.strip(',')}] {(subj[0] if subj else '')[:70]}")
    print(f"magnitude_dismissal_gate live-window report over last {len(shas)} commits:")
    print(f"  magnitude fires: {mag}   verdict-scope fires: {ver}")
    for line in fired:
        print(line)
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="magnitude_dismissal_gate", description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="CI self-test: fail (exit 1) if a check no longer fires on a "
        "violation or no longer passes a compliant window",
    )
    ap.add_argument(
        "--window", type=int, metavar="N", help="report fires over the last N commits"
    )
    args = ap.parse_args(argv)

    if args.window:
        return _window_report(_common.repo_root(), args.window)

    code, failures = _run_selftest()
    if failures:
        for f in failures:
            print(f"  [DEAD] magnitude_dismissal_gate self-test: {f}")
        print(
            f"\n{len(failures)} magnitude_dismissal_gate self-test(s) FAILED -- the "
            "gate has silently rotted (M34/M42). Fix it before trusting it."
        )
    else:
        print("All magnitude_dismissal_gate self-tests pass.")
    return code if args.check else 0


if __name__ == "__main__":
    sys.exit(main())
