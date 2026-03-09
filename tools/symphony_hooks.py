#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import re
import shutil
import socket
import subprocess
import sys
from pathlib import Path


def _to_bool(value: str | None, *, default: bool = False) -> bool:
    if value is None:
        return default
    normalized = value.strip().lower()
    return normalized in {"1", "true", "yes", "on"}


def _tokens(raw: str | None) -> list[str]:
    if not raw:
        return []
    return [part.strip().lower() for part in raw.split(",") if part.strip()]


def _token_match(value: str, tokens: list[str]) -> bool:
    haystack = value.strip().lower()
    if not haystack:
        return False
    for token in tokens:
        normalized = token.strip().lower()
        if not normalized:
            continue
        if haystack == normalized:
            return True
        pattern = re.compile(
            rf"(?<![a-z0-9]){re.escape(normalized)}(?![a-z0-9])"
        )
        if pattern.search(haystack):
            return True
    return False


def _run(
    cmd: list[str],
    *,
    check: bool = False,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        check=check,
        capture_output=True,
        text=True,
        env=env,
    )


def _git(
    *args: str, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    return _run(["git", *args], env=env)


def _git_out(*args: str) -> str:
    proc = _git(*args)
    if proc.returncode != 0:
        return ""
    return (proc.stdout or "").strip()


def _is_git_repo() -> bool:
    return _git("rev-parse", "--is-inside-work-tree").returncode == 0


def _is_dirty() -> bool:
    return bool(_git_out("status", "--porcelain", "--untracked-files=all"))


def _hostname() -> str:
    return (
        os.environ.get("COMPUTERNAME", "").strip()
        or os.environ.get("HOSTNAME", "").strip()
        or socket.gethostname().strip()
    )


def _log(prefix: str, message: str) -> None:
    print(f"{prefix} {message}", flush=True)


def run_before_run() -> int:
    prefix = "[symphony_git_sync]"
    remote = os.environ.get("MOLT_SYMPHONY_SYNC_REMOTE", "origin").strip() or "origin"
    branch = os.environ.get("MOLT_SYMPHONY_SYNC_BRANCH", "main").strip() or "main"
    allowlist = _tokens(
        os.environ.get("MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS", "adpena,symphony")
    )

    if not _is_git_repo():
        _log(prefix, "skip reason=not_git_repo")
        return 0

    if _git("remote", "get-url", remote).returncode != 0:
        _log(prefix, f"skip reason=missing_remote remote={remote}")
        return 0

    if _git("fetch", "--prune", remote, branch).returncode != 0:
        _log(prefix, f"skip reason=fetch_failed remote={remote} branch={branch}")
        return 0

    target_ref = f"refs/remotes/{remote}/{branch}"
    if _git("show-ref", "--verify", "--quiet", target_ref).returncode != 0:
        _log(prefix, f"skip reason=missing_target_ref target_ref={target_ref}")
        return 0

    current_branch = _git_out("symbolic-ref", "--quiet", "--short", "HEAD")
    if not current_branch:
        _log(prefix, "skip reason=detached_head")
        return 0

    if current_branch != branch:
        if _is_dirty():
            _log(
                prefix,
                "skip reason=dirty_workspace "
                f"current_branch={current_branch} target_branch={branch}",
            )
            return 0
        if _git("checkout", branch).returncode != 0:
            _log(
                prefix,
                "skip reason=checkout_failed "
                f"current_branch={current_branch} target_branch={branch}",
            )
            return 0

    if _is_dirty():
        _log(prefix, f"skip reason=dirty_workspace branch={branch}")
        return 0

    if _git("merge-base", "--is-ancestor", "HEAD", target_ref).returncode != 0:
        _log(
            prefix,
            f"skip reason=not_fast_forward branch={branch} target_ref={target_ref}",
        )
        return 0

    if _git_out("rev-parse", "HEAD") == _git_out("rev-parse", target_ref):
        _log(prefix, f"ok status=up_to_date branch={branch}")
        return 0

    incoming = _git_out("log", "--format=%an <%ae>", f"HEAD..{target_ref}")
    blocked: list[str] = []
    for author in (line.strip() for line in incoming.splitlines()):
        if not author:
            continue
        if _token_match(author, allowlist):
            continue
        blocked.append(author)

    if blocked:
        _log(
            prefix,
            "skip reason=author_gate_blocked "
            f"allowed={','.join(allowlist) or '(none)'}",
        )
        for author in blocked:
            _log(prefix, f"blocked_author={author}")
        return 0

    if _git("merge", "--ff-only", target_ref).returncode == 0:
        _log(
            prefix,
            f"ok status=fast_forward_applied branch={branch} target_ref={target_ref}",
        )
        return 0

    _log(prefix, f"skip reason=ff_merge_failed branch={branch} target_ref={target_ref}")
    return 0


def run_after_run() -> int:
    prefix = "[symphony_autoland]"
    enabled = _to_bool(os.environ.get("MOLT_SYMPHONY_AUTOLAND_ENABLED"), default=True)
    if not enabled:
        _log(prefix, "skip reason=disabled")
        return 0

    mode = os.environ.get("MOLT_SYMPHONY_AUTOLAND_MODE", "direct-main").strip()
    remote = os.environ.get("MOLT_SYMPHONY_SYNC_REMOTE", "origin").strip() or "origin"
    target_branch = (
        os.environ.get("MOLT_SYMPHONY_SYNC_BRANCH", "main").strip() or "main"
    )
    commit_message = (
        os.environ.get(
            "MOLT_SYMPHONY_AUTOLAND_COMMIT_MESSAGE", "chore: sync all changes"
        ).strip()
        or "chore: sync all changes"
    )
    pr_base = os.environ.get("MOLT_SYMPHONY_AUTOLAND_PR_BASE", "main").strip() or "main"
    pr_automerge = _to_bool(
        os.environ.get("MOLT_SYMPHONY_AUTOLAND_PR_AUTOMERGE"), default=True
    )

    allowed_authors = _tokens(
        os.environ.get("MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS", "adpena,symphony")
    )
    trusted_users = _tokens(
        os.environ.get("MOLT_SYMPHONY_TRUSTED_USERS", "adpena,symphony")
    )
    trusted_machines = _tokens(os.environ.get("MOLT_SYMPHONY_TRUSTED_MACHINES", ""))

    if not _is_git_repo():
        _log(prefix, "skip reason=not_git_repo")
        return 0

    current_branch = _git_out("symbolic-ref", "--quiet", "--short", "HEAD")
    if not current_branch:
        _log(prefix, "skip reason=detached_head")
        return 0

    if not _is_dirty():
        _log(prefix, f"skip reason=no_changes branch={current_branch}")
        return 0

    author_name = _git_out("config", "--get", "user.name")
    author_email = _git_out("config", "--get", "user.email")
    author_identity = f"{author_name} <{author_email}>".strip()
    machine_identity = _hostname()

    author_allowed = _token_match(author_identity, allowed_authors) or _token_match(
        author_identity, trusted_users
    )
    machine_allowed = _token_match(machine_identity, trusted_machines)
    if not author_allowed and not machine_allowed:
        _log(
            prefix,
            "skip reason=untrusted_identity "
            f"author={author_identity} machine={machine_identity}",
        )
        return 0

    if mode not in {"direct-main", "pr-automerge"}:
        _log(prefix, f"skip reason=unsupported_mode mode={mode}")
        return 0

    if mode == "direct-main" and current_branch != target_branch:
        _log(
            prefix,
            "skip reason=branch_mismatch mode=direct-main "
            f"current={current_branch} target={target_branch}",
        )
        return 0

    _git("add", "-A")
    if not _git_out("diff", "--cached", "--name-only"):
        _log(prefix, "skip reason=nothing_staged")
        return 0

    guard = Path("tools/secret_guard.py")
    if guard.exists():
        guard_env = dict(os.environ)
        if not str(guard_env.get("PYTHONPATH", "")).strip():
            guard_env["PYTHONPATH"] = "src"
        if (
            _run([sys.executable, str(guard), "--staged"], env=guard_env).returncode
            != 0
        ):
            _log(prefix, "skip reason=secret_guard_blocked")
            return 0

    if _git("commit", "-m", commit_message).returncode != 0:
        _log(prefix, "skip reason=commit_failed")
        return 0

    pushed = _git("push", remote, current_branch).returncode == 0
    if not pushed:
        rebased = _git("pull", "--rebase", remote, current_branch).returncode == 0
        if not rebased:
            _log(prefix, f"skip reason=rebase_failed branch={current_branch}")
            return 0
        pushed = _git("push", remote, current_branch).returncode == 0
        if not pushed:
            _log(
                prefix, f"skip reason=push_failed_after_rebase branch={current_branch}"
            )
            return 0

    if mode != "pr-automerge":
        _log(prefix, f"ok status=pushed mode=direct-main branch={current_branch}")
        return 0

    if not shutil.which("gh"):
        _log(
            prefix,
            "ok status=pushed mode=pr-automerge "
            f"skip_reason=gh_missing branch={current_branch}",
        )
        return 0

    if current_branch == pr_base:
        _log(
            prefix,
            "ok status=pushed mode=pr-automerge "
            f"skip_reason=already_on_base branch={current_branch}",
        )
        return 0

    pr_title = (
        os.environ.get("MOLT_SYMPHONY_AUTOLAND_PR_TITLE", "").strip() or commit_message
    )
    pr_body = (
        os.environ.get("MOLT_SYMPHONY_AUTOLAND_PR_BODY", "").strip()
        or "Automated Symphony autoland from trusted identity."
    )
    created = (
        _run(
            [
                "gh",
                "pr",
                "create",
                "--base",
                pr_base,
                "--head",
                current_branch,
                "--title",
                pr_title,
                "--body",
                pr_body,
            ]
        ).returncode
        == 0
    )
    if not created:
        _log(
            prefix,
            "ok status=pushed mode=pr-automerge "
            f"skip_reason=pr_create_failed branch={current_branch}",
        )
        return 0

    if not pr_automerge:
        _log(
            prefix,
            f"ok status=pr_created mode=pr-automerge branch={current_branch} base={pr_base}",
        )
        return 0

    merged = (
        _run(
            [
                "gh",
                "pr",
                "merge",
                current_branch,
                "--auto",
                "--squash",
                "--delete-branch",
            ]
        ).returncode
        == 0
    )
    if merged:
        _log(
            prefix,
            f"ok status=pr_automerge_queued branch={current_branch} base={pr_base}",
        )
        return 0

    _log(
        prefix,
        "ok status=pr_created mode=pr-automerge "
        f"skip_reason=merge_queue_failed branch={current_branch} base={pr_base}",
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Cross-platform Symphony hook entrypoint."
    )
    parser.add_argument(
        "hook",
        choices=("before_run", "after_run"),
        help="Hook name to execute.",
    )
    args = parser.parse_args(argv)
    if args.hook == "before_run":
        return run_before_run()
    if args.hook == "after_run":
        return run_after_run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
