#!/usr/bin/env python3
"""Landing receipts: the one record that admits a tree to main (HF-701).

`molt_dev.py land --fast` and `molt_dev.py integrate` write a receipt after
every selected gate passed on a tree, for the main it was tested on. The
landing commit's verification and waiver text is projected from it
(`commit_message`), and the managed pre-push hook refuses a main update
without one (`check-push`). A receipt is an unsigned local file: it guards
against mistakes, not against an adversary. See docs/design/landing_queue.md,
section 9.

The module is stdlib-only: the pre-push hook runs it before any project
environment is selected.

Run:
    python tools/landing_receipt.py check-push <remote-name> <remote-url> < refs
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import textwrap
import time
from collections.abc import Iterable, Mapping, Sequence
from typing import Any
from datetime import UTC, datetime
from pathlib import Path

SCHEMA = "molt.landing-receipt.v1"
STORE = Path("molt") / "landing-receipts"
# The landing queue keeps its results this long (molt_dev_queue.RESULT_DAYS).
RETENTION_DAYS = 30
MAIN_REF = "refs/heads/main"
GIT_TIMEOUT_SECONDS = 60
MESSAGE_WIDTH = 72
_OBJECT_ID = re.compile(r"^(?:[0-9a-f]{40}|[0-9a-f]{64})$")
_LIBTEST = re.compile(
    r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; "
)
_PYTEST = re.compile(
    r"^=*\s*((?:\d+ (?:passed|failed|skipped|xfailed|xpassed|errors?|warnings?"
    r"|deselected|rerun)(?:, )?)+) in \d"
)
_PYTEST_PART = re.compile(r"(\d+) (\w+)")


def store_dir(common_dir: Path) -> Path:
    return common_dir / STORE


def receipt_path(common_dir: Path, *, tree: str, base: str) -> Path:
    for label, value in (("tree", tree), ("base", base)):
        if not _OBJECT_ID.fullmatch(value):
            raise ValueError(f"receipt {label} must be a full object id: {value!r}")
    return store_dir(common_dir) / f"{tree}.{base}.json"


def counts_from_output(text: str) -> dict[str, int] | None:
    """Test counts a gate printed: libtest summaries summed, or pytest's
    terminal summary; None when it printed neither (a static gate)."""
    totals = {"passed": 0, "failed": 0, "skipped": 0}
    found = False
    for match in _LIBTEST.finditer(text):
        found = True
        totals["passed"] += int(match[1])
        totals["failed"] += int(match[2])
        totals["skipped"] += int(match[3])
    summary = None
    for line in text.splitlines():
        if matched := _PYTEST.match(line.strip()):
            summary = matched[1]
    if summary is not None:
        found = True
        for number, word in _PYTEST_PART.findall(summary):
            if word == "passed":
                totals["passed"] += int(number)
            elif word in {"failed", "error", "errors"}:
                totals["failed"] += int(number)
            elif word in {"skipped", "xfailed", "deselected"}:
                totals["skipped"] += int(number)
    return totals if found else None


def gate_record(
    *, phase: str, command: str, seconds: float | None, log: Path | None
) -> dict[str, object]:
    """One passed gate: what ran, how long, and the counts its log shows."""
    text = ""
    if log is not None and log.is_file():
        text = log.read_text(encoding="utf-8", errors="replace")
    return {
        "phase": phase,
        "command": command,
        "seconds": None if seconds is None else round(seconds, 1),
        "counts": counts_from_output(text) if text else None,
        "log": None if log is None else str(log),
    }


def build_receipt(
    *,
    mode: str,
    base: str,
    tree: str,
    tested_commit: str,
    controller: str | None,
    started_at: str,
    gates: Sequence[Mapping[str, object]],
    waived: Sequence[str] = (),
    unverified: Sequence[str] = (),
    narrowed: Sequence[str] = (),
    postsubmit_only: Sequence[str] = (),
) -> dict[str, object]:
    """``postsubmit_only`` names the data readers presubmit did not run; they
    passed nothing here, and postsubmit runs them."""
    if mode not in {"fast", "queue"}:
        raise ValueError(f"unknown landing mode {mode!r}")
    selection = json.dumps(
        {
            "gates": [gate["command"] for gate in gates],
            "waived": list(waived),
            "postsubmit_only": list(postsubmit_only),
        },
        sort_keys=True,
    )
    return {
        "schema": SCHEMA,
        "status": "passed",
        "mode": mode,
        "base": base,
        "tree": tree,
        "tested_commit": tested_commit,
        "commits": [tested_commit],
        "controller": controller,
        "started_at": started_at,
        "finished_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "selection_sha256": hashlib.sha256(selection.encode()).hexdigest(),
        "gates": [dict(gate) for gate in gates],
        "waived": list(waived),
        "unverified": list(unverified),
        "narrowed": list(narrowed),
        "postsubmit_only": list(postsubmit_only),
    }


def _atomic_write(path: Path, payload: Mapping[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode()
    handle, temporary = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    try:
        with os.fdopen(handle, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def retire_old(common_dir: Path, *, now: float | None = None) -> None:
    """Receipts older than RETENTION_DAYS leave; a pruned receipt only makes
    the hook refuse, and the landing re-tests."""
    directory = store_dir(common_dir)
    if not directory.is_dir():
        return
    cutoff = (time.time() if now is None else now) - RETENTION_DAYS * 86400
    for path in directory.glob("*.json"):
        try:
            if path.stat().st_mtime < cutoff:
                path.unlink()
        except FileNotFoundError:
            continue


def write_receipt(common_dir: Path, receipt: Mapping[str, object]) -> Path:
    if receipt.get("schema") != SCHEMA or receipt.get("status") != "passed":
        raise ValueError("only a passing landing receipt is written")
    path = receipt_path(
        common_dir, tree=str(receipt["tree"]), base=str(receipt["base"])
    )
    retire_old(common_dir)
    _atomic_write(path, receipt)
    return path


def read_receipt(common_dir: Path, *, tree: str, base: str) -> dict | None:
    path = receipt_path(common_dir, tree=tree, base=base)
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return None
    if not isinstance(payload, dict) or payload.get("schema") != SCHEMA:
        return None
    return payload


def add_commit(common_dir: Path, *, tree: str, base: str, commit: str) -> Path:
    """List a commit whose tree is the tested tree (the landing commit)."""
    receipt = read_receipt(common_dir, tree=tree, base=base)
    if receipt is None:
        raise ValueError(f"no landing receipt for tree {tree} on base {base}")
    if commit not in receipt["commits"]:
        receipt["commits"] = [*receipt["commits"], commit]
    path = receipt_path(common_dir, tree=tree, base=base)
    _atomic_write(path, receipt)
    return path


def _wrap(text: str) -> list[str]:
    return textwrap.wrap(
        text,
        width=MESSAGE_WIDTH,
        break_long_words=False,
        break_on_hyphens=False,
        subsequent_indent="  ",
    )


def _counts_text(counts: Mapping[str, int]) -> str:
    text = f"{counts['passed']} passed"
    if counts.get("failed"):
        text += f", {counts['failed']} failed"
    if counts.get("skipped"):
        text += f", {counts['skipped']} skipped"
    return text


def commit_message(subject: str, receipt: Mapping[str, Any]) -> str:
    """The landing commit's message: the subject, then text projected from the
    receipt alone. The commit-msg hook appends Change-Id after it."""
    subject = subject.strip()
    if not subject or "\n" in subject:
        raise ValueError("a landing subject is one non-empty line")
    gates = list(receipt["gates"])
    static = [gate for gate in gates if gate["phase"] == "static"]
    tried = [gate for gate in gates if gate["phase"] != "static"]
    lines = [
        f"Landed by molt_dev.py land --fast: tree {str(receipt['tree'])[:12]} "
        f"passed on base {str(receipt['base'])[:12]} (tested commit "
        f"{str(receipt['tested_commit'])[:12]}).",
        f"Static gates: {len(static)} passed.",
    ]
    for gate in tried:
        counts = gate.get("counts")
        carried = gate.get("carried_from_base")
        lines.append(
            (f"Carried from base {str(carried)[:12]} " if carried else "Ran ")
            + str(gate["command"])
            + (f": {_counts_text(counts)}." if counts else ": exit 0.")
        )
    waived = list(receipt.get("waived", ()))
    lines.append(
        "Waived to the queue batch: " + ("; ".join(waived) if waived else "none") + "."
    )
    for label, key in (("Narrowed", "narrowed"), ("Unverified here", "unverified")):
        items = list(receipt.get(key, ()))
        if items:
            lines.append(f"{label}: " + "; ".join(items) + ".")
    left = len(list(receipt.get("postsubmit_only", ())))
    if left:
        lines.append(
            f"Left to postsubmit: {left} data-reader test file(s); "
            "presubmit ran the importers."
        )
    body = [wrapped for line in lines for wrapped in _wrap(line)]
    # A path is one token; it stays whole on its own line.
    body.append(f"Receipt: landing-receipts/{receipt['tree']}.{receipt['base']}.json")
    return subject + "\n\n" + "\n".join(body) + "\n"


# --------------------------------------------------------------------------
# The pre-push check
# --------------------------------------------------------------------------


def _git(arguments: Sequence[str], *, cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *arguments],
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=GIT_TIMEOUT_SECONDS,
        check=False,
    )


def _common_dir(cwd: Path) -> Path:
    result = _git(["rev-parse", "--git-common-dir"], cwd=cwd)
    if result.returncode:
        raise ValueError(
            f"cannot resolve the Git common directory: {result.stderr.strip()}"
        )
    common = Path(result.stdout.strip())
    return common if common.is_absolute() else (cwd / common).resolve()


def push_refusals(lines: Iterable[str], *, cwd: Path) -> list[str]:
    """Why each main update in a pre-push stdin is refused; empty admits."""
    refusals: list[str] = []
    common_dir: Path | None = None
    for line in lines:
        fields = line.split()
        if not fields:
            continue
        if len(fields) != 4:
            refusals.append(f"malformed pre-push line: {line.strip()!r}")
            continue
        _local_ref, local, remote_ref, remote = fields
        if remote_ref != MAIN_REF:
            continue
        if set(local) == {"0"}:
            refusals.append("refused deleting main")
            continue
        if set(remote) == {"0"}:
            refusals.append(
                f"refused creating main at {local}: main must already exist"
            )
            continue
        tree = _git(["rev-parse", "--verify", "--quiet", f"{local}^{{tree}}"], cwd=cwd)
        if tree.returncode:
            refusals.append(f"refused main update to {local}: unknown commit")
            continue
        tree_id = tree.stdout.strip()
        if common_dir is None:
            common_dir = _common_dir(cwd)
        receipt = read_receipt(common_dir, tree=tree_id, base=remote)
        if receipt is None or receipt.get("status") != "passed":
            refusals.append(
                f"refused main update to {local}: no landing receipt for tree "
                f"{tree_id} on base {remote}"
            )
        elif local not in receipt.get("commits", ()):
            refusals.append(
                f"refused main update to {local}: the receipt for tree {tree_id} "
                f"on base {remote} does not list this commit"
            )
    return refusals


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser(
        "check-push", help="pre-push: refuse a main update without a landing receipt"
    )
    check.add_argument("remote_name", nargs="?")
    check.add_argument("remote_url", nargs="?")
    args = parser.parse_args(argv)
    if args.command == "check-push":
        try:
            refusals = push_refusals(sys.stdin, cwd=Path.cwd())
        except (OSError, ValueError, subprocess.SubprocessError) as exc:
            refusals = [f"refused: the landing receipt check failed: {exc}"]
        for refusal in refusals:
            print(f"pre-push: {refusal}", file=sys.stderr)
        if refusals:
            print(
                "pre-push: land through `python -B tools/molt_dev.py land --fast "
                "--repo <lane checkout> --subject <text>` (docs/design/"
                "landing_queue.md, section 9)",
                file=sys.stderr,
            )
            return 1
        return 0
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
