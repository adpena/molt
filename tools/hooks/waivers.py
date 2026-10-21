#!/usr/bin/env python3
"""Uniform waiver grammar for the molt apparatus (APPARATUS Wave 1, A9).

pact's lesson: every gate needs an ESCAPE VALVE that is *never binary* -- an
opt-out that forces a real rationale and is logged, so agents stop inventing
ad-hoc overrides. molt adopts one grammar everywhere:

* **inline**   ``# <GATE>_OK:<rationale>`` on the same line as the flagged
  construct. ``<GATE>`` is the upper-snake gate id (``BASH_GUARD``,
  ``LANDING_GATE``, ``MAGNITUDE_DISMISSAL``). The rationale must be REAL: at
  least 4 non-placeholder characters. ``# LANDING_GATE_OK:todo`` is refused.
* **commit token** ``[skip-<gate>] <rationale>`` in a commit subject opts the
  whole commit window out of ``<gate>`` (lower-kebab id). The rationale must
  FOLLOW the token and be real; a bare token is refused.

Every honored waiver is appended to ``.molt/state/waivers.jsonl`` for audit.

This module is pure + stdlib-only so both the hooks and the gate-lifecycle
audit (``tools/check_gate_flips.py``) share ONE classifier (pact's "one
predicate, many surfaces" discipline).
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Iterable

try:  # allow use both as ``tools.hooks.waivers`` and standalone
    from tools.hooks._common import append_jsonl, state_dir
except Exception:  # pragma: no cover - path-invocation fallback
    import os as _os
    import sys as _sys

    _sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.dirname(__file__))))
    from tools.hooks._common import append_jsonl, state_dir


# Rationales that look like a reason but carry no information -> refused.
PLACEHOLDER_RATIONALES = frozenset(
    {
        "todo",
        "fixme",
        "fix",
        "xxx",
        "xxxx",
        "placeholder",
        "reason",
        "rationale",
        "because",
        "n/a",
        "na",
        "none",
        "tbd",
        "temp",
        "temporary",
        "later",
        "wip",
        "test",
        "testing",
        "asdf",
        "foo",
        "bar",
    }
)

MIN_RATIONALE_CHARS = 4

_INLINE_RE = re.compile(
    r"#\s*([A-Za-z][A-Za-z0-9]*(?:_[A-Za-z0-9]+)*)_OK\s*:\s*(.+?)\s*$"
)
_SKIP_RE = re.compile(r"\[skip-([a-z0-9][a-z0-9-]*)\]")


def _norm_gate(gate: str) -> str:
    return gate.strip().upper().replace("-", "_")


def is_valid_rationale(rationale: str | None) -> bool:
    """True iff ``rationale`` is real: >=4 non-placeholder alnum-bearing chars."""
    if not rationale:
        return False
    text = rationale.strip()
    if len(text) < MIN_RATIONALE_CHARS:
        return False
    # Must contain at least MIN_RATIONALE_CHARS alphanumeric characters overall
    # (rejects "----", "....", "??!?").
    alnum = [c for c in text if c.isalnum()]
    if len(alnum) < MIN_RATIONALE_CHARS:
        return False
    lowered = text.lower()
    if lowered in PLACEHOLDER_RATIONALES:
        return False
    # A single word that is a placeholder (possibly with trailing punctuation).
    word = re.sub(r"[^a-z0-9/]", "", lowered)
    if word in PLACEHOLDER_RATIONALES:
        return False
    # All-identical characters ("aaaa", "xxxx") carry no information.
    if len(set(c for c in text if not c.isspace())) <= 1:
        return False
    return True


def parse_inline_waiver(text: str, gate: str) -> str | None:
    """Return the VALID rationale for ``gate`` found in ``text``, else None.

    Scans every line; matches ``# <GATE>_OK:<rationale>`` for the requested gate
    (case-insensitive on the gate id). A placeholder rationale does NOT count as
    a waiver -- the construct stays flagged.
    """
    if not text:
        return None
    target = _norm_gate(gate)
    for line in text.splitlines():
        m = _INLINE_RE.search(line)
        if not m:
            continue
        if _norm_gate(m.group(1)) != target:
            continue
        rationale = m.group(2).strip()
        if is_valid_rationale(rationale):
            return rationale
    return None


def find_all_waivers(text: str) -> list[tuple[str, str, bool]]:
    """Return every ``# <GATE>_OK:<rationale>`` as (gate, rationale, is_valid)."""
    out: list[tuple[str, str, bool]] = []
    if not text:
        return out
    for line in text.splitlines():
        m = _INLINE_RE.search(line)
        if not m:
            continue
        gate = _norm_gate(m.group(1))
        rationale = m.group(2).strip()
        out.append((gate, rationale, is_valid_rationale(rationale)))
    return out


def commit_opt_out(subjects: Iterable[str], gate: str) -> str | None:
    """The rationale of a window-wide ``[skip-<gate>] <rationale>`` opt-out in
    any commit subject, or None: the one commit-token classifier.

    The rationale is the text that FOLLOWS the token, so surrounding subject
    prose never counts and a bare token is refused.
    """
    kebab = gate.strip().lower().replace("_", "-")
    for subject in subjects:
        text = str(subject or "")
        for match in _SKIP_RE.finditer(text):
            if match.group(1) != kebab:
                continue
            rationale = text[match.end() :].strip(" -:.\t")
            if is_valid_rationale(rationale):
                return rationale
    return None


def grammar_summary() -> str:
    """The waiver grammar in one line, for the session digest."""
    return (
        "inline `# <GATE>_OK: <rationale>` on the flagged line, or "
        "`[skip-<gate>] <rationale>` in a commit subject; a rationale needs at "
        f"least {MIN_RATIONALE_CHARS} real characters"
    )


def record_waiver(
    gate: str,
    rationale: str,
    *,
    source: str,
    context: str = "",
    root: Path | None = None,
) -> None:
    """Append an honored waiver to ``.molt/state/waivers.jsonl`` (never raises)."""
    append_jsonl(
        state_dir(root) / "waivers.jsonl",
        {
            "gate": _norm_gate(gate),
            "rationale": rationale,
            "source": source,  # e.g. "inline", "commit-token", "env"
            "context": context,
        },
    )


def iter_gate_ids(gates: Iterable[str]) -> list[str]:
    return [_norm_gate(g) for g in gates]
