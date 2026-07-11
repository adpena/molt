"""Firewalled local-model advisory classification (APPARATUS A5)."""

from __future__ import annotations
import datetime as dt
import json
import os
import shlex
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

DEFAULT_EVENT_LOG = Path(".molt/state/advisory_events.jsonl")


@dataclass(frozen=True)
class AdvisoryDecision:
    verdict: str | None
    reason: str


def decide_output(output: str, *, text: str, schema: Sequence[str]) -> AdvisoryDecision:
    allowed = tuple(dict.fromkeys(x.strip() for x in schema if x.strip()))
    candidate = output.strip()
    if not allowed or not candidate:
        return AdvisoryDecision(None, "empty-schema" if not allowed else "empty-output")
    normalized = " ".join(text.split()).casefold()
    if normalized and normalized in " ".join(candidate.split()).casefold():
        return AdvisoryDecision(None, "prompt-echo")
    try:
        decoded = json.loads(candidate)
    except json.JSONDecodeError:
        decoded = candidate
    if isinstance(decoded, Mapping):
        decoded = decoded.get("verdict")
    if not isinstance(decoded, str) or decoded.strip() not in allowed:
        return AdvisoryDecision(None, "outside-closed-enum")
    return AdvisoryDecision(decoded.strip(), "accepted")


def _append_event(path: Path, payload: Mapping[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8", newline="\n") as handle:
        handle.write(
            json.dumps(
                {"utc": dt.datetime.now(dt.UTC).isoformat(), **payload}, sort_keys=True
            )
            + "\n"
        )


def classify(
    text: str,
    schema: Sequence[str],
    *,
    purpose: str = "unspecified",
    command: str | None = None,
    timeout_seconds: float = 8.0,
    event_log: Path = DEFAULT_EVENT_LOG,
) -> str | None:
    resolved = command if command is not None else os.environ.get("MOLT_FM_CMD", "")
    if not resolved.strip():
        return None
    prompt = json.dumps(
        {
            "instruction": "Return exactly one allowed verdict and no explanation.",
            "purpose": purpose,
            "allowed_verdicts": list(schema),
            "text": text,
        },
        sort_keys=True,
    )
    event = {"purpose": purpose, "schema": list(schema)}
    try:
        result = subprocess.run(
            shlex.split(resolved, posix=True),
            input=prompt,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout_seconds,
            check=False,
        )
        decision = decide_output(result.stdout, text=text, schema=schema)
        event.update(
            returncode=result.returncode,
            verdict=decision.verdict,
            reason=decision.reason,
        )
        if result.returncode != 0:
            event.update(verdict=None, reason="nonzero-exit")
        _append_event(event_log, event)
        return decision.verdict if result.returncode == 0 else None
    except Exception as exc:
        event.update(verdict=None, reason="classifier-fault", error=type(exc).__name__)
        try:
            _append_event(event_log, event)
        except Exception:
            pass
        return None


def poison_triage(text: str) -> str | None:
    return classify(
        text,
        ("poison", "misplaced_valuable", "benign"),
        purpose="fail_closed_gate_ambiguity_triage",
    )


def magnitude_confirmation(text: str) -> str | None:
    return classify(
        text,
        ("dismissal", "relative_significance", "unclear"),
        purpose="magnitude_dismissal_confirmation",
    )


def finding_tag_suggestion(text: str) -> str | None:
    return classify(
        text,
        ("dag", "dsl", "equations", "cross_leg", "not_a_finding"),
        purpose="findings_auto_tag_suggestion",
    )
