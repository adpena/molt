"""Audit every tracked Cargo workspace for advisories, licenses, bans and sources.

`security.rust.deny` runs `deny` and `security.rust.audit` runs `audit`. The
workspaces come from `tools/dependency_manifests.py`, the authority that also
keeps Dependabot watching each lockfile, so a new lockfile is audited at once.

One `deny.toml` serves every workspace. An advisory ignore or allowed git
source that matches nothing in one workspace is expected; one that matches
nothing in any workspace is a stale entry, and the audit fails on it.

usage: cargo_lock_audit.py deny
       cargo_lock_audit.py audit --db <advisory-db-dir>
"""

from __future__ import annotations

import argparse
from collections.abc import Callable, Sequence
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from tools.dependency_manifests import cargo_workspace_dirs  # noqa: E402

# Lints that report a shared-config entry unused in one workspace.
UNUSED_ENTRY_CODES = ("advisory-not-detected", "unmatched-source")
# Runs one command; tests pass a recording fake instead of a process.
Runner = Callable[..., subprocess.CompletedProcess[str]]


def _run_cargo(
    command: list[str], *, capture: bool
) -> subprocess.CompletedProcess[str]:
    """One cargo-deny or cargo-audit child; the proof plan's guard owns its process tree."""
    return subprocess.run(
        command,
        cwd=ROOT,
        capture_output=capture,
        text=True,
        encoding="utf-8",
        check=False,
    )


def _deny_command(workspace: PurePosixPath) -> list[str]:
    # cargo-deny 0.20 moved --config from check to the root command.
    return [
        "cargo",
        "deny",
        "--format",
        "json",
        "--manifest-path",
        str(workspace / "Cargo.toml"),
        "--locked",
        "--config",
        "deny.toml",
    ]


def _deny_result(
    proc: subprocess.CompletedProcess[str],
) -> tuple[int, dict[str, set[str]], list[str]]:
    unused: dict[str, set[str]] = {code: set() for code in UNUSED_ENTRY_CODES}
    errors: list[str] = []
    # Keep stream boundaries: stdout need not end with a newline before stderr.
    for output in (proc.stdout, proc.stderr):
        for line in output.splitlines():
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                if proc.returncode and line.strip():
                    errors.append(line)
                continue
            if not isinstance(record, dict):
                if proc.returncode:
                    errors.append(line)
                continue
            fields = record.get("fields", {})
            if record.get("type") == "diagnostic":
                code = fields.get("code")
                if code in unused:
                    unused[code].add(fields["labels"][0]["span"])
                elif fields.get("severity") == "error":
                    errors.append(f"{code}: {fields.get('message')}")
                    # Advisory identity, source URL and safe upgrade ranges
                    # live in notes, outside the short diagnostic title.
                    errors.extend(fields.get("notes", []))
            elif record.get("type") == "log" and fields.get("level") == "ERROR":
                errors.append(str(fields.get("message")))
            elif proc.returncode:
                errors.append(line)
    return proc.returncode, unused, errors


def deny_workspace(
    workspace: PurePosixPath, *, run: Runner = _run_cargo
) -> tuple[int, dict[str, set[str]], list[str]]:
    """Check a prepared workspace against the run's shared offline snapshot."""
    return _deny_result(
        run([*_deny_command(workspace), "--offline", "check"], capture=True)
    )


def run_deny(workspaces: Sequence[PurePosixPath], run: Runner = _run_cargo) -> int:
    # In 0.20, --offline also disables crate downloads. Prepare every locked
    # workspace first; fetch the shared advisory/replacement data only once.
    # A preparation failure cannot admit checks against stale shared data.
    for index, workspace in enumerate(workspaces):
        proc = run(
            [*_deny_command(workspace), "fetch", "all" if index == 0 else "index"],
            capture=True,
        )
        returncode, _, errors = _deny_result(proc)
        status = "ok" if returncode == 0 else f"FAILED (exit {returncode})"
        print(f"cargo deny fetch {workspace}: {status}", flush=True)
        for error in errors:
            print(f"  {error}", flush=True)
        if returncode:
            return 1
    failed = False
    unused_everywhere: dict[str, set[str]] | None = None
    for workspace in workspaces:
        returncode, unused, errors = deny_workspace(workspace, run=run)
        status = "ok" if returncode == 0 else f"FAILED (exit {returncode})"
        print(f"cargo deny {workspace}: {status}", flush=True)
        for error in errors:
            print(f"  {error}", flush=True)
        failed |= returncode != 0
        if unused_everywhere is None:
            unused_everywhere = unused
        else:
            for code in UNUSED_ENTRY_CODES:
                unused_everywhere[code] &= unused[code]
    for code in UNUSED_ENTRY_CODES:
        for entry in sorted((unused_everywhere or {}).get(code, ())):
            print(
                f"deny.toml entry matches no tracked workspace ({code}): {entry}",
                flush=True,
            )
            failed = True
    return 1 if failed else 0


def run_audit(
    workspaces: Sequence[PurePosixPath], db: str, run: Runner = _run_cargo
) -> int:
    failed = False
    for index, workspace in enumerate(workspaces):
        command = [
            "cargo",
            "audit",
            "--db",
            db,
            "--file",
            str(workspace / "Cargo.lock"),
        ]
        if index:
            # One advisory database snapshot for every lockfile in this run.
            command.append("--no-fetch")
        proc = run(command, capture=False)
        print(
            f"cargo audit {workspace}: {'ok' if proc.returncode == 0 else 'FAILED'}",
            flush=True,
        )
        failed |= proc.returncode != 0
    return 1 if failed else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    sub = parser.add_subparsers(dest="mode", required=True)
    sub.add_parser("deny", help="cargo deny check for every tracked workspace")
    audit = sub.add_parser("audit", help="cargo audit for every tracked lockfile")
    audit.add_argument(
        "--db", required=True, help="cargo-audit advisory database directory"
    )
    args = parser.parse_args()
    workspaces = cargo_workspace_dirs(ROOT)
    return (
        run_deny(workspaces) if args.mode == "deny" else run_audit(workspaces, args.db)
    )


if __name__ == "__main__":
    raise SystemExit(main())
