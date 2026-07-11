#!/usr/bin/env python3
"""Serialized, named-files-only, sha-guarded commit primitive (APPARATUS A11).

The multi-agent commit-collision class this session hit (M20/M24): two lanes on
one checkout race a commit; ``git add -A`` / ``git add .`` sweeps a sibling
lane's staged WIP into the wrong commit; or a sibling's hunk lands in a file
between the moment an agent READ it and the moment it commits, so the commit
absorbs a change the author never saw. pact serializes subagent commits through
an fcntl lock with named-files-only + an expected-content-sha per file
(``tools/subagent_commit_serializer.py``, APPARATUS 1.9). This is the molt port
(``fcntl`` -> ``msvcrt`` on Windows).

It is the SAFE multi-agent commit primitive, and it is OPTIONAL / ADVISORY -- it
does NOT replace ``tools/ff_land.py`` (the fast-forward push guard) or the live
``bash_guard`` hook; it COMPLEMENTS them:

  * ``bash_guard`` refuses a raw ``git add -A``/add-then-commit in a Bash tool
    call (the live PreToolUse gate);
  * this refuses ``-A`` / ``.`` / a flag / a glob at the commit API level too,
    and adds the expected-content-sha guard the hook cannot see;
  * ``ff_land`` then pushes the resulting HEAD only if it is a clean fast-forward.

Guarantees (each a pure, unit-tested surface + the locked commit that composes
them):

  * NAMED FILES ONLY -- ``validate_pathspec`` refuses an empty list, ``-A`` /
    ``--all`` / ``-a`` / ``-u``, ``.`` / ``*`` / ``:/`` / a pathspec-magic prefix,
    or any leading-``-`` flag. Only concrete paths reach ``git commit -- <files>``.
  * EXPECTED CONTENT SHA (optional) -- if the caller passes the sha256 it read for
    each file, ``check_expected_shas`` refuses the commit when a file's CURRENT
    content differs (a sibling-hunk absorption / the file moved under the author).
  * SERIALIZED -- the read-check-commit runs under an exclusive ``msvcrt`` /
    ``fcntl`` lock so two agents on one checkout cannot interleave a commit.

Return codes (``serialized_commit`` / CLI):
  0 committed (or dry-run OK)   3 pathspec refused (sweep / flag / glob)
  2 usage error                 4 expected-sha mismatch (file changed since read)
  5 git commit failed           6 could not acquire the serializer lock

``--check`` is the falsifiable self-test (ci_gate tier-1 + a check_gate_liveness
canary): ``-A`` and ``.`` and a stale sha MUST be refused, and a clean named-file
pathspec MUST be allowed. Stdlib-only; ASCII + UTF-8-explicit (M43).
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

try:  # pragma: no cover - trivial import shim
    from tools._io_utf8 import force_utf8_stdio as _force_utf8_stdio
except Exception:  # pragma: no cover - path-invocation fallback
    _sys_path_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if _sys_path_dir not in sys.path:
        sys.path.insert(0, _sys_path_dir)
    try:
        from tools._io_utf8 import force_utf8_stdio as _force_utf8_stdio
    except Exception:

        def _force_utf8_stdio(*, errors: str = "backslashreplace") -> None:
            for stream in (sys.stdout, sys.stderr):
                reconfigure = getattr(stream, "reconfigure", None)
                if reconfigure is None:
                    continue
                try:
                    reconfigure(encoding="utf-8", errors=errors)
                except (AttributeError, ValueError, OSError):
                    pass


ROOT = Path(__file__).resolve().parents[1]
LOCK_TIMEOUT_SECONDS = 30.0

RC_OK = 0
RC_USAGE = 2
RC_PATHSPEC_REFUSED = 3
RC_SHA_MISMATCH = 4
RC_GIT_FAILED = 5
RC_LOCK_TIMEOUT = 6

# Entries that are sweeps / flags / globs, not a concrete named file. ``git add
# -A``/``.`` is the exact M20 sweep; a leading ``-`` is a flag; ``*``/``?`` are
# shell/pathspec globs; ``:/`` and ``:(`` are git pathspec magic (whole-tree).
_FORBIDDEN_EXACT = frozenset(
    {"", ".", "..", "*", "-A", "--all", "-a", "-u", "--update", ":/"}
)


@dataclass
class ShaMismatch:
    path: str
    expected: str
    actual: str | None  # None == the file is missing now


@dataclass
class CommitResult:
    rc: int
    reason: str
    committed_sha: str | None = None
    mismatches: list[ShaMismatch] = field(default_factory=list)

    def as_dict(self) -> dict[str, object]:
        return {
            "rc": self.rc,
            "reason": self.reason,
            "committed_sha": self.committed_sha,
            "mismatches": [
                {"path": m.path, "expected": m.expected, "actual": m.actual}
                for m in self.mismatches
            ],
        }


# ------------------------------ pure surfaces --------------------------------


def validate_pathspec(files: list[str]) -> str | None:
    """Refuse a sweep / flag / glob; return a refusal reason, or None if OK.

    NAMED FILES ONLY: this is the commit-API twin of the ``bash_guard`` refusal of
    ``git add -A`` (M20). Every entry must be a concrete path.
    """
    if not files:
        return "no files given: a serialized commit requires an explicit pathspec (M20)"
    for f in files:
        s = str(f or "").strip()
        if s in _FORBIDDEN_EXACT:
            return (
                f"refused sweep/glob token {f!r}: pass concrete named files, never "
                "-A / . / * / :/ (that sweeps a sibling lane's WIP, M20)"
            )
        if s.startswith("-"):
            return f"refused flag-like entry {f!r}: a pathspec is not a git flag"
        if s.startswith(":(") or s.startswith(":/"):
            return (
                f"refused pathspec-magic entry {f!r}: pass a concrete path, not magic"
            )
        if "*" in s or "?" in s:
            return (
                f"refused glob entry {f!r}: expand globs to concrete named files first"
            )
    return None


def sha256_file(path: Path) -> str | None:
    """The sha256 hexdigest of ``path``'s bytes, or None if unreadable/missing."""
    try:
        h = hashlib.sha256()
        with open(path, "rb") as fh:
            for chunk in iter(lambda: fh.read(65536), b""):
                h.update(chunk)
        return h.hexdigest()
    except OSError:
        return None


def check_expected_shas(root: Path, expected: dict[str, str]) -> list[ShaMismatch]:
    """Every file whose CURRENT sha256 differs from the caller's expected sha.

    A mismatch means the file changed since the author read it (a sibling-hunk
    absorption / a move under the author) -- the commit must be refused so the
    author re-reads instead of blindly committing someone else's change.
    """
    out: list[ShaMismatch] = []
    for rel, exp in expected.items():
        actual = sha256_file(root / rel)
        if actual != exp:
            out.append(ShaMismatch(path=rel, expected=exp, actual=actual))
    return out


# ------------------------------ serializer lock -------------------------------


def _lock_path(root: Path) -> Path:
    d = root / ".molt" / "state"
    with contextlib.suppress(OSError):
        d.mkdir(parents=True, exist_ok=True)
    return d / "commit_serializer.lock"


@contextlib.contextmanager
def serializer_lock(root: Path, timeout: float = LOCK_TIMEOUT_SECONDS):
    """Exclusive cross-platform commit lock (msvcrt on Windows, fcntl on POSIX).

    Yields on acquire; raises ``TimeoutError`` if it cannot lock within
    ``timeout``. Mirrors ``tools/findings_registry.registry_lock``.
    """
    p = _lock_path(root)
    fd = os.open(str(p), os.O_RDWR | os.O_CREAT, 0o644)
    try:
        if os.fstat(fd).st_size == 0:
            os.write(fd, b"\0")  # msvcrt.locking needs a byte to lock
        deadline = time.monotonic() + timeout
        if os.name == "nt":
            import msvcrt

            while True:
                try:
                    os.lseek(fd, 0, os.SEEK_SET)
                    msvcrt.locking(fd, msvcrt.LK_NBLCK, 1)
                    break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise TimeoutError(
                            f"could not acquire commit serializer lock within {timeout:g}s"
                        ) from None
                    time.sleep(0.05)
            try:
                yield fd
            finally:
                with contextlib.suppress(OSError):
                    os.lseek(fd, 0, os.SEEK_SET)
                    msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
        else:
            import fcntl

            while True:
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except (BlockingIOError, OSError):
                    if time.monotonic() >= deadline:
                        raise TimeoutError(
                            f"could not acquire commit serializer lock within {timeout:g}s"
                        ) from None
                    time.sleep(0.05)
            try:
                yield fd
            finally:
                with contextlib.suppress(OSError):
                    fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)


# ------------------------------- the commit ----------------------------------


def _git_head(root: Path) -> str | None:
    try:
        r = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=str(root),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=8.0,
        )
        return r.stdout.strip() or None if r.returncode == 0 else None
    except Exception:
        return None


def serialized_commit(
    root: Path,
    message: str,
    files: list[str],
    expected_shas: dict[str, str] | None = None,
    dry_run: bool = False,
    timeout: float = LOCK_TIMEOUT_SECONDS,
) -> CommitResult:
    """Commit ``files`` (named pathspec only) under the serializer lock.

    Refuses a sweep pathspec (rc 3), an expected-sha mismatch (rc 4), a git
    failure (rc 5), or a lock timeout (rc 6). ``dry_run`` runs every guard but
    stops before ``git commit`` (rc 0 == would-commit).
    """
    reason = validate_pathspec(files)
    if reason is not None:
        return CommitResult(RC_PATHSPEC_REFUSED, reason)

    try:
        with serializer_lock(root, timeout=timeout):
            if expected_shas:
                mism = check_expected_shas(root, expected_shas)
                if mism:
                    detail = ", ".join(
                        f"{m.path} (expected {m.expected[:12]}, got "
                        f"{'MISSING' if m.actual is None else m.actual[:12]})"
                        for m in mism
                    )
                    return CommitResult(
                        RC_SHA_MISMATCH,
                        f"expected-sha mismatch -- file(s) changed since you read them: "
                        f"{detail}. Re-read and reconcile before committing (M24).",
                        mismatches=mism,
                    )
            if dry_run:
                return CommitResult(RC_OK, "dry-run: all guards passed, commit not run")
            # Stage the NAMED files only (the M20-safe add -- never `-A`/`.`), so a
            # new/untracked target commits too; `git commit -- <pathspec>` alone
            # would only commit already-tracked changes.
            add = subprocess.run(
                ["git", "add", "--", *files],
                cwd=str(root),
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=30.0,
            )
            if add.returncode != 0:
                tail = (add.stderr or add.stdout or "").strip().splitlines()
                return CommitResult(
                    RC_GIT_FAILED,
                    "git add failed: " + (tail[-1] if tail else "unknown error"),
                )
            proc = subprocess.run(
                ["git", "commit", "-m", message, "--", *files],
                cwd=str(root),
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=30.0,
            )
            if proc.returncode != 0:
                tail = (proc.stderr or proc.stdout or "").strip().splitlines()
                return CommitResult(
                    RC_GIT_FAILED,
                    "git commit failed: " + (tail[-1] if tail else "unknown error"),
                )
            return CommitResult(RC_OK, "committed", committed_sha=_git_head(root))
    except TimeoutError as exc:
        return CommitResult(RC_LOCK_TIMEOUT, str(exc))


# ------------------------------- CI self-test --------------------------------


def _run_selftest() -> tuple[int, list[str]]:
    failures: list[str] = []

    # NAMED FILES ONLY: sweeps / flags / globs must be refused.
    for bad in (["-A"], ["."], ["--all"], ["-a"], [], ["*"], [":/"], ["--", "x"]):
        if validate_pathspec(bad) is None:
            failures.append(f"validate_pathspec allowed a sweep/flag: {bad!r}")
    # A clean named pathspec must be allowed.
    if (
        validate_pathspec(["tools/commit_serializer.py", "docs/agent/CLAIMS.md"])
        is not None
    ):
        failures.append("validate_pathspec refused a clean named pathspec")

    # EXPECTED SHA: a mismatch must be detected on a synthetic tmp file.
    import tempfile

    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        f = root / "f.txt"
        f.write_text("hello", encoding="utf-8")
        good = sha256_file(f)
        if good is None:
            failures.append("sha256_file could not hash a written file")
        else:
            if check_expected_shas(root, {"f.txt": good}):
                failures.append("check_expected_shas flagged a matching file")
            if not check_expected_shas(root, {"f.txt": "0" * 64}):
                failures.append("check_expected_shas missed a stale-sha mismatch")
            if not check_expected_shas(root, {"missing.txt": good}):
                failures.append("check_expected_shas missed a missing file")

    return (1 if failures else 0), failures


# ----------------------------------- main ------------------------------------


def _parse_file_arg(entry: str) -> tuple[str, str | None]:
    """``path`` or ``path:sha256`` -> (path, expected_sha_or_None)."""
    if ":" in entry:
        # Split on the LAST colon so a Windows drive-letter path stays intact; a
        # 64-hex tail is treated as the expected sha, else the whole thing is a path.
        head, _, tail = entry.rpartition(":")
        if len(tail) == 64 and all(c in "0123456789abcdef" for c in tail.lower()):
            return head, tail.lower()
    return entry, None


def main(argv: list[str] | None = None) -> int:
    _force_utf8_stdio()
    ap = argparse.ArgumentParser(
        prog="commit_serializer", description=__doc__.splitlines()[0]
    )
    ap.add_argument("--message", "-m", help="commit message")
    ap.add_argument(
        "--file",
        action="append",
        default=[],
        dest="files",
        metavar="PATH[:SHA256]",
        help="a named file to commit, optionally with the sha256 you read",
    )
    ap.add_argument("--root", type=Path, default=ROOT, help="repo root")
    ap.add_argument(
        "--dry-run", action="store_true", help="run guards, skip git commit"
    )
    ap.add_argument("--json", action="store_true", help="emit machine-readable output")
    ap.add_argument(
        "--check",
        action="store_true",
        help="falsifiable self-test: exit 1 if the pathspec/sha guards rot",
    )
    args = ap.parse_args(argv)

    if args.check:
        code, failures = _run_selftest()
        if failures:
            for f in failures:
                print(f"  [DEAD] commit_serializer self-test: {f}")
            print(
                f"\n{len(failures)} commit_serializer self-test(s) FAILED -- the "
                "named-files-only / expected-sha guard has silently rotted (M34/M42)."
            )
        else:
            print("All commit_serializer self-tests pass.")
        return code

    if not args.message:
        print("commit_serializer: --message/-m is required to commit", file=sys.stderr)
        return RC_USAGE

    files: list[str] = []
    expected: dict[str, str] = {}
    for entry in args.files:
        path, sha = _parse_file_arg(entry)
        files.append(path)
        if sha:
            expected[path] = sha

    result = serialized_commit(
        args.root.resolve(),
        args.message,
        files,
        expected_shas=expected or None,
        dry_run=args.dry_run,
    )
    if args.json:
        print(json.dumps(result.as_dict(), indent=2, sort_keys=True))
    else:
        print(f"[rc={result.rc}] {result.reason}")
        if result.committed_sha:
            print(f"  committed: {result.committed_sha}")
    return result.rc


if __name__ == "__main__":
    sys.exit(main())
