#!/usr/bin/env python3
"""SessionStart digest: the live facts a new session needs, read cheaply.

Claude Code injects this hook's stdout into a new session. The digest reports
only live facts that cost little to read, and it points at the authority that
owns everything else instead of restating it: ``AGENTS.md`` holds the rules,
``tools/agent_coordination.py context`` the lanes, records and claims,
``docs/agent/V1_HANDOFF_FINDINGS.md`` the work list, and the harness loads the
memory index itself. It reports:

  * where to start (those authorities);
  * the landing queue: pending requests and the newest results;
  * the proof queue's active rows;
  * the live worktree count against the drift gate's SPRAWL threshold;
  * what the hooks enforce, read from their live wiring.

It also anchors the session window (the HEAD at session start) that the Stop
legs diff against; ``landing_gate`` owns that marker. Every section is
guarded, and the whole digest runs under the fail-open wrapper.
"""

from __future__ import annotations

import io
import sys
import time
from datetime import datetime
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.hooks import _common, landing_gate  # noqa: E402


RECENT_LANDING_RESULTS = 3
# SessionStart sources that begin a conversation; "resume" and "compact"
# continue one. A payload without a source anchors, as before.
ANCHORING_SOURCES = frozenset({"startup", "clear", None})


def _section_start(out: io.StringIO) -> None:
    out.write(
        "START: AGENTS.md; `uv run --python 3.12 python tools/agent_coordination.py "
        "context` (lanes, records, claims); open v1.0 findings: "
        "docs/agent/V1_HANDOFF_FINDINGS.md.\n"
    )


def _section_landing_queue(out: io.StringIO, root: Path) -> None:
    from tools import molt_dev_queue as queue

    paths = queue.queue_paths(root)
    requests = queue.pending(paths)
    out.write(f"LANDING QUEUE: {len(requests)} pending")
    if requests:
        out.write(
            " ("
            + ", ".join(request.branch or request.head[:12] for request in requests)
            + ")"
        )
    out.write(".\n")
    for result in queue.recorded_results(paths, limit=RECENT_LANDING_RESULTS):
        request = result["request"]
        out.write(
            f"  {queue.LandingStatus(result['status'])} "
            f"{request['branch'] or request['head'][:12]} at {result['recorded_at']}\n"
        )


def _section_proof_queue(out: io.StringIO, root: Path) -> None:
    count = _common.proof_queue_active_count(root)
    if count is None:
        out.write("PROOF QUEUE: status undeterminable.\n")
    else:
        out.write(f"PROOF QUEUE: {count} active row(s).\n")


def _section_worktrees(out: io.StringIO, root: Path) -> None:
    from tools import drift_harvest

    count = drift_harvest.live_worktree_count(root)
    limit = drift_harvest.MAX_LIVE_WORKTREES
    over = (
        "; over the threshold: `uv run --python 3.12 python tools/drift_harvest.py "
        "--report`, then retire landed lanes"
        if count > limit
        else ""
    )
    out.write(f"WORKTREES: {count} live (SPRAWL threshold {limit}){over}.\n")


SHADOW_WINDOW_DAYS = 7


def _observed(root: Path, name: str, key: str) -> str:
    """Would-be blocks an observe-only gate recorded in the window, by ``key``."""
    cutoff = time.time() - SHADOW_WINDOW_DAYS * 86_400
    counts: dict[str, int] = {}
    for record in _common.shadow_records(root, name):
        try:
            stamp = datetime.fromisoformat(str(record["recorded_at"])).timestamp()
        except (KeyError, ValueError):
            continue
        if stamp >= cutoff:
            label = str(record.get(key, "?"))
            counts[label] = counts.get(label, 0) + 1
    if not counts:
        return "0"
    detail = ", ".join(f"{label} {count}" for label, count in sorted(counts.items()))
    return f"{sum(counts.values())} ({detail})"


def _section_hooks(out: io.StringIO, root: Path) -> None:
    from tools.hooks import bash_guard, stop_gates, waivers

    if bash_guard.enforcement_state() == "strict":
        guard = "refuses"
        override = "`MOLT_GUARD_OK=1 <cmd>` overrides once, audited."
    else:
        guard = "records, without blocking,"
        override = "its gate_flip in tools/proof_plan.toml is warn."
    legs = ", ".join(
        f"{leg.name} ({stop_gates.enforcement_state(leg)})" for leg in stop_gates.LEGS
    )
    out.write(
        f"HOOKS: bash_guard {guard} destructive git on the shared checkout, "
        "`git add`-then-commit sweeps, heavy builds that bypass a live proof "
        f"queue, and HTTPS pushes; {override}\n"
        f"  Stop legs: {legs}; a strict leg re-engages a turn at most once, a "
        "warn leg only records.\n"
        f"  Observed would-be blocks, last {SHADOW_WINDOW_DAYS} days: bash_guard "
        f"{_observed(root, bash_guard.SHADOW_NAME, 'rule')}; Stop legs "
        f"{_observed(root, stop_gates.SHADOW_NAME, 'leg')}.\n"
        f"  Waivers: {waivers.grammar_summary()}.\n"
    )


def run() -> int:
    data = _common.read_hook_input()
    cwd = data.get("cwd")
    root = _common.repo_root(cwd)
    session_id = str(data.get("session_id", ""))

    # Anchor the session window the Stop legs diff against, once per
    # conversation: a resumed or compacted session keeps its window.
    if data.get("source") in ANCHORING_SOURCES:
        try:
            landing_gate.write_session_baseline(root, session_id)
        except Exception as exc:
            _common.log_error("session_digest", exc, root)

    out = io.StringIO()
    out.write("== molt session start ==\n")
    for name, section in (
        ("start", lambda: _section_start(out)),
        ("landing queue", lambda: _section_landing_queue(out, root)),
        ("proof queue", lambda: _section_proof_queue(out, root)),
        ("worktrees", lambda: _section_worktrees(out, root)),
        ("hooks", lambda: _section_hooks(out, root)),
    ):
        try:
            section()
        except Exception as exc:  # a bad section must not sink the digest
            out.write(f"  ({name} unavailable: {type(exc).__name__})\n")
            _common.log_error("session_digest", exc, root)
    sys.stdout.write(out.getvalue())
    return 0


def main() -> None:
    _common.run_fail_open("session_digest", run)


if __name__ == "__main__":
    main()
