#!/usr/bin/env python3
"""molt_dev.py — the CANONICAL integration + agent-ops driver for molt.

This is molt's `x.py` / `dist`: ONE executable driver that encodes the
operator discipline a night of hard-won incidents produced, so the workflow
stops being prompt-lore and starts being machine-enforced. The mandate:
"Manual is fragile and brittle and error prone and not canonical or world
class OSS." Every recurring hazard below is converted into a fail-loud,
tested countermeasure. No subcommand trusts a swallowed exit code, a shell
text tool, or an interpreter it did not pin.

Why each verdict avoids shell text tools (hazard 5)
---------------------------------------------------
Under the rtk proxy / sandbox, `git diff`, `ls`, `stat`, and `cmp` can print
misleading output (filtered, cached, or summarized). So every COMPARISON this
driver makes — "is this sha in that range", "does origin contain this tip",
"did this file change", "are these two binaries identical" — is computed from
git plumbing (`rev-list`, `cat-file`, `patch-id`, `rev-parse`) captured as
bytes via subprocess, or from python file ops (`pathlib` + `filecmp` / a
chunked byte compare), NEVER from a human-readable shell text tool whose output
the proxy may rewrite. The plumbing commands emit machine-stable IDs that no
proxy reformats.

The hazard inventory -> countermeasure map (the spec this file implements)
-------------------------------------------------------------------------
  1. Rebase silently drops commits (patch-id dedup vs moved upstream)
       -> `integrate` computes the patch-id of every source commit BEFORE the
          rebase and verifies each one is present AFTER (either in the new
          range, or provably already-upstream by a patch-id match against the
          upstream tip range). A missing patch-id is a LOUD FAIL listing the
          dangling shas; integration never proceeds past a dropped commit.
  2. Push exit codes lie (rtk/sandbox swallow; 144-detached pushes can SUCCEED)
       -> `verify-push` (and the push step of `integrate`) FETCHES the upstream
          and confirms origin/<branch> CONTAINS the pushed tip sha via
          `git merge-base --is-ancestor`. Cleanup is gated on THAT, never on
          the push command's exit code.
  3. Worktree cleanup loses work
       -> cleanup REFUSES when unpushed commits exist (rev-list
          upstream..HEAD non-empty) OR tracked staged/unstaged changes exist
          (excluding a configurable ignore set, default the wasm sha256 files).
          --force requires NAMING the sha being abandoned (so an abandon is
          deliberate and auditable, never accidental).
  4. Partial WIP salvage (staged vs unstaged split commits)
       -> `secure-wip` stages ALL tracked modifications (staged + unstaged,
          enumerated from `git status --porcelain`, partner-excludes honored)
          into ONE recovery-marked commit, so a split staged/unstaged state can
          never be half-lost.
  5. diff/ls/stat lie under rtk
       -> see the section above; all verdicts use plumbing + python file ops.
  6. Stale-binary misattribution
       -> `verify-toolchain` runs a configurable BEHAVIOR-MARKER probe program
          against a built binary (e.g. the SETATTR error-message marker) and
          reports the binary mtime vs the newest commit touching Rust runtime/
          backend sources, so a green result can never be credited to a stale
          artifact. `integrate` can require freshness via a gate manifest flag.
  7. .venv interpreter flips (3.12 <-> 3.14t across uv calls)
       -> `python-oracle` resolves the requested CPython version, PINS it, and
          verifies `sys.version` before returning the interpreter path;
          calibration/differential tooling routes through it.
  8. Content-marker verification
       -> the gate manifest's per-change `markers` (file-exists / file-contains)
          are checked post-rebase, pre-push, via python file ops.
  9. Liveness/recovery probes (agent transcripts, daemon pids)
       -> `probe` reports size+mtime (python `os.stat`) and pid liveness
          (`os.kill(pid, 0)`), replacing ad-hoc shell one-liners.
 10. Gate selection by change-class
       -> `integrate` reads tools/molt_dev_gates.toml (touched-path glob ->
          required gate commands) and runs exactly the gates the change-class
          demands; --extra-gate adds arc-specific gates.
 11. Backgrounded long-runs die silently (observed 2026-06-06, twice in one
     session: the harness reaps run_in_background process groups at detach
     [exit 144], and sandboxed tool calls reap even setsid daemons at
     container teardown — both deaths lose block-buffered output, leaving an
     EMPTY log, NO exit status, and no way to distinguish "killed" from
     "still running" after the fact)
       -> `detached-run` double-forks + setsid with unbuffered IO and writes
          an atomic state dir (pid / sid / cmd.json / run.log / rc). The
          companion `detached-verify` is the REQUIRED second tool call: it
          proves the daemon outlived the spawning call's teardown window
          (--min-age-s) and reports running / done(rc) / DIED-SILENT loudly.
          Spawn-and-verify in the SAME tool call is structurally untrustable
          (teardown happens after the call returns), so the protocol is
          two-step BY DESIGN. The spawning call must itself run unsandboxed;
          when it does not, verify turns the silent reap into a loud
          died-silent verdict instead of an empty log discovered an hour
          later. detached-run NEVER kills: a live same-name daemon is a
          refusal, and --replace only clears DEAD/finished state.
 12. Split-root toolchain (a worktree edit silently not compiled in)
       -> `difftest` builds a program with the frontend (PYTHONPATH) AND the
          runtime/backend (MOLT_PROJECT_ROOT) derived from ONE --root, so a
          runtime/backend source edit in a worktree is actually compiled in
          (otherwise the runtime-staticlib fingerprint is computed against the
          canonical checkout, never rebuilds, and stale behavior is credited to
          a fix that never shipped — hazard 6 on the source-tree axis). It then
          runs the artifact under the safe_run watchdog and diffs stdout+exit
          BYTE-for-BYTE against a version-pinned CPython oracle, fail-loud.

Non-goals
---------
This driver drives GIT + GATES. It does NOT replace human/code review, it does
NOT decide what is correct, and it never force-pushes or rewrites published
history. It refuses rather than guesses.

Exit codes
----------
  0  success / all checks green
  1  a check failed (LOUD; the reason is printed to stderr)
  2  usage / configuration error

Subcommands
-----------
  integrate         fetch -> rebase -> verify-commits -> markers -> gates ->
                    push -> confirm -> cleanup  (each step loud, idempotent,
                    re-run safe; --dry-run plans without mutating)
  secure-wip        commit ALL tracked modifications in one recovery commit
  verify-push       confirm origin/<branch> contains a tip sha (by fetch+ancestor)
  verify-toolchain  behavior-marker probe + binary-freshness report
  probe             file size+mtime / pid liveness (python ops)
  python-oracle     resolve + PIN + verify a CPython version before use
  detached-run      spawn a setsid daemon that survives harness detach
                    (state dir: pid/sid/cmd.json/run.log/rc)
  detached-verify   prove the daemon outlived its spawning call; report
                    running / done(rc) / DIED-SILENT (the hazard-11 class)
"""

from __future__ import annotations

import argparse
import filecmp
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

import harness_memory_guard

# --------------------------------------------------------------------------
# Constants / locations
# --------------------------------------------------------------------------

# The repo this driver acts on. Resolved from the driver's own location so a
# detached worktree copy still finds ITS OWN .git (worktrees share the object
# store but each has its own HEAD); --repo overrides for cross-tree invocation.
DEFAULT_REPO = Path(__file__).resolve().parents[1]
GATES_CONFIG_NAME = "molt_dev_gates.toml"

# Tracked paths excluded from the cleanup "dirty tree" and secure-wip checks by
# default: the wasm checksum sidecars that a build legitimately churns and that
# a partner regenerates. Override via --ignore (repeatable) or the gate config.
DEFAULT_IGNORE_GLOBS = (
    "wasm/molt_runtime.wasm.sha256",
    "wasm/molt_runtime_reloc.wasm.sha256",
)

# Recovery marker prefix for secure-wip commits, greppable so a salvaged WIP is
# never mistaken for a deliberate landing.
WIP_MARKER = "WIP-RECOVERY"

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2


# --------------------------------------------------------------------------
# Output helpers (deterministic, stderr for status, stdout for data)
# --------------------------------------------------------------------------


def _say(msg: str) -> None:
    """A status line -> stderr (so stdout stays clean for machine consumers)."""
    print(msg, file=sys.stderr, flush=True)


def _step(name: str) -> None:
    _say(f"==> {name}")


def _ok(msg: str) -> None:
    _say(f"    OK: {msg}")


def _fail(msg: str) -> None:
    _say(f"    FAIL: {msg}")


def _warn(msg: str) -> None:
    _say(f"    WARN: {msg}")


class DriverError(Exception):
    """A loud, fatal driver condition. Carries an exit code (default FAIL)."""

    def __init__(self, message: str, code: int = EXIT_FAIL):
        super().__init__(message)
        self.code = code


def _run_driver_command(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout: float | None = 60.0,
    prefix: str = "MOLT_DEV",
) -> subprocess.CompletedProcess[str]:
    """Run one bounded captured driver command through the shared guard."""
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix=prefix,
        cwd=cwd or DEFAULT_REPO,
        env=env,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def _run_driver_command_bytes(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: float,
    prefix: str,
) -> subprocess.CompletedProcess[bytes]:
    """Run one bounded captured driver command with byte-exact output custody."""
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix=prefix,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=False,
        timeout=timeout,
    )


def _run_live_gate(cmd: str, *, repo: Path, env: dict[str, str]) -> int:
    """Run a manifest gate live through memory custody while preserving shell syntax."""
    proc = harness_memory_guard.guarded_completed_process(
        ["/bin/sh", "-c", cmd],
        prefix="MOLT_TEST_SUITE",
        cwd=repo,
        env=env,
        capture_output=False,
        text=True,
        timeout=None,
    )
    return proc.returncode


# --------------------------------------------------------------------------
# Git plumbing (bytes-stable; never a shell TEXT tool whose output a proxy
# could rewrite — only plumbing that emits stable IDs / explicit exit codes)
# --------------------------------------------------------------------------


@dataclass
class Git:
    """A thin, fail-loud wrapper over `git -C <repo>` plumbing.

    Every method returns plumbing output (stable IDs) or a boolean derived from
    git's own EXIT CODE (e.g. merge-base --is-ancestor), never parsed from a
    porcelain/text rendering. This is the hazard-5 discipline in code form.
    """

    repo: Path

    def _run(
        self,
        args: list[str],
        *,
        check: bool = True,
        input_text: str | None = None,
    ) -> subprocess.CompletedProcess[str]:
        proc = _run_driver_command(
            ["git", "-C", str(self.repo), *args],
            input_text=input_text,
        )
        if check and proc.returncode != 0:
            raise DriverError(
                f"git {' '.join(args)} failed (exit {proc.returncode}):\n"
                f"{proc.stderr.strip()}"
            )
        return proc

    # -- identity / state ---------------------------------------------------

    def rev_parse(self, ref: str) -> str:
        return self._run(["rev-parse", "--verify", ref]).stdout.strip()

    def head_sha(self) -> str:
        return self.rev_parse("HEAD")

    def symbolic_head(self) -> str | None:
        """The branch name HEAD points at, or None when detached."""
        proc = self._run(["symbolic-ref", "--quiet", "--short", "HEAD"], check=False)
        name = proc.stdout.strip()
        return name or None

    def short(self, sha: str) -> str:
        return sha[:9]

    def commits_in_range(self, range_expr: str) -> list[str]:
        """Full shas in `range_expr` (e.g. 'origin/main..HEAD'), newest first."""
        out = self._run(["rev-list", range_expr]).stdout.strip()
        return [line for line in out.splitlines() if line]

    def is_ancestor(self, ancestor: str, descendant: str) -> bool:
        """True iff `ancestor` is an ancestor of `descendant` (by git exit code).

        This is the hazard-2 primitive: "does origin/<branch> CONTAIN this tip"
        == "is the tip an ancestor of origin/<branch>". Derived from git's exit
        code (0 yes / 1 no), not from any text rendering a proxy could rewrite.
        """
        proc = self._run(
            ["merge-base", "--is-ancestor", ancestor, descendant], check=False
        )
        if proc.returncode not in (0, 1):
            raise DriverError(
                f"git merge-base --is-ancestor {ancestor} {descendant} errored "
                f"(exit {proc.returncode}): {proc.stderr.strip()}"
            )
        return proc.returncode == 0

    def patch_id(self, sha: str) -> str | None:
        """The stable patch-id of one commit, or None for an empty/merge diff.

        `git patch-id` reduces a diff to a content hash that is invariant under
        rebase/cherry-pick (line numbers, parent shas, author/date all drop
        out). This is the hazard-1 primitive: two commits with the same change
        share a patch-id even after a rebase moves them. A commit whose diff is
        empty (e.g. a no-op merge) yields no patch-id and is reported as None so
        the caller treats it explicitly rather than silently.
        """
        diff = self._run(["diff-tree", "-p", "--no-color", sha]).stdout
        if not diff.strip():
            return None
        proc = self._run(["patch-id", "--stable"], input_text=diff, check=False)
        first = proc.stdout.strip().split("\n", 1)[0]
        if not first:
            return None
        # `patch-id` prints "<patch-sha> <commit-sha>"; we want the first field.
        return first.split()[0]

    def patch_ids_for_range(self, range_expr: str) -> dict[str, str]:
        """Map patch-id -> a representative commit sha for every commit in range.

        Empty-diff commits are skipped (no patch-id). When two commits share a
        patch-id (a genuine duplicate), the first wins as the representative;
        membership is all the caller needs.
        """
        out: dict[str, str] = {}
        for sha in self.commits_in_range(range_expr):
            pid = self.patch_id(sha)
            if pid is not None and pid not in out:
                out[pid] = sha
        return out

    # -- mutating ops (only the safe, non-destructive ones) -----------------

    def fetch(self, remote: str, branch: str) -> None:
        """Fetch `branch` from `remote`, FORCING the remote-tracking ref update.

        A bare `git fetch <remote> <branch>` only updates FETCH_HEAD; whether it
        also moves `refs/remotes/<remote>/<branch>` depends on the remote's
        configured refspec, so `origin/main` can stay STALE after it (observed).
        An explicit `+<branch>:refs/remotes/<remote>/<branch>` refspec
        guarantees the tracking ref is updated, which every downstream check
        (rev-parse origin/main, ancestor confirmation) depends on. This is
        load-bearing for hazard 2: confirming a push requires an up-to-date
        view of the remote tip.
        """
        refspec = f"+{branch}:refs/remotes/{remote}/{branch}"
        self._run(["fetch", "--quiet", remote, refspec])

    def status_porcelain(self) -> list[tuple[str, str]]:
        """Parse `git status --porcelain=v1 -z` into (xy, path) tuples.

        The -z form is NUL-delimited and never localized/reflowed, so this is a
        machine-stable read (not a human text rendering). XY is the two-char
        status code; for renames we keep the destination path (the live file).
        """
        out = self._run(["status", "--porcelain=v1", "-z"]).stdout
        entries: list[tuple[str, str]] = []
        tokens = out.split("\0")
        i = 0
        while i < len(tokens):
            tok = tokens[i]
            if not tok:
                i += 1
                continue
            xy = tok[:2]
            path = tok[3:]
            # A rename/copy entry ('R'/'C') is followed by the ORIGIN path in
            # the next NUL field; we keep the destination (already in `path`).
            if xy and xy[0] in ("R", "C"):
                i += 2
            else:
                i += 1
            entries.append((xy, path))
        return entries


# --------------------------------------------------------------------------
# Ignore-set helper
# --------------------------------------------------------------------------


def _glob_to_regex(glob: str) -> "re.Pattern[str]":
    """Translate a repo-relative POSIX glob to an anchored regex with REAL `**`.

    Path-glob semantics (NOT fnmatch's, which treats `**` like `*` and so would
    fail to match `src/molt/**/*.py` against `src/molt/frontend.py`):
      *   matches any run of characters EXCEPT '/'  (one path segment)
      ?   matches a single character except '/'
      **  matches any number of FULL path segments, including zero, so
          `a/**/b.py` matches `a/b.py`, `a/x/b.py`, `a/x/y/b.py`.
      **/ at a prefix collapses to "zero-or-more leading segments".
    The result is cached per glob (compiled once) for cheap repeated matching.
    """
    out: list[str] = ["^"]
    i = 0
    n = len(glob)
    while i < n:
        c = glob[i]
        if c == "*":
            if i + 1 < n and glob[i + 1] == "*":
                # `**` — consume it, and an optional trailing '/'.
                i += 2
                if i < n and glob[i] == "/":
                    i += 1
                    # `**/` -> zero or more leading segments ("(seg/)*").
                    out.append("(?:[^/]+/)*")
                else:
                    # bare `**` -> anything, including '/'.
                    out.append(".*")
                continue
            out.append("[^/]*")
            i += 1
            continue
        if c == "?":
            out.append("[^/]")
            i += 1
            continue
        out.append(re.escape(c))
        i += 1
    out.append("$")
    return re.compile("".join(out))


_GLOB_CACHE: dict[str, "re.Pattern[str]"] = {}


def _glob_match(path: str, glob: str) -> bool:
    norm = path.replace("\\", "/")
    pat = _GLOB_CACHE.get(glob)
    if pat is None:
        pat = _glob_to_regex(glob)
        _GLOB_CACHE[glob] = pat
    return pat.match(norm) is not None


def _is_ignored(path: str, ignore_globs: tuple[str, ...]) -> bool:
    """True if `path` matches any glob (repo-relative POSIX, real `**`).

    Shared by the cleanup/secure-wip ignore set AND by gate-rule selection, so
    both use identical, correct path-glob semantics (a `**` rule matches nested
    files; an exact path matches itself).
    """
    return any(_glob_match(path, glob) for glob in ignore_globs)


def _tracked_dirty(git: Git, ignore_globs: tuple[str, ...]) -> list[tuple[str, str]]:
    """Tracked, non-ignored, modified paths (staged or unstaged), sorted.

    Untracked files ('??') are NOT included: cleanup of a worktree is about not
    losing TRACKED work the user committed-against; untracked scratch is the
    user's to manage. secure-wip uses a separate enumerator that DOES capture
    intentionally-added tracked changes.
    """
    dirty: list[tuple[str, str]] = []
    for xy, path in git.status_porcelain():
        if xy == "??":
            continue
        if _is_ignored(path, ignore_globs):
            continue
        dirty.append((xy, path))
    return sorted(dirty, key=lambda t: t[1])


# --------------------------------------------------------------------------
# Gate manifest (change-class -> required gates)
# --------------------------------------------------------------------------


@dataclass
class GateRule:
    name: str
    globs: list[str]
    gates: list[str]
    description: str = ""
    require_fresh_toolchain: bool = False


@dataclass
class GateConfig:
    rules: list[GateRule] = field(default_factory=list)
    always: list[str] = field(default_factory=list)

    @staticmethod
    def load(path: Path) -> "GateConfig":
        if not path.exists():
            raise DriverError(
                f"gate manifest missing: {path}. The change-class->gates manifest "
                "is required for `integrate` (run with --no-gates to skip gating, "
                "or point --gates-config at it).",
                code=EXIT_USAGE,
            )
        try:
            data = tomllib.loads(path.read_text(encoding="utf-8"))
        except tomllib.TOMLDecodeError as exc:
            raise DriverError(f"gate manifest is not valid TOML ({path}): {exc}")
        rules: list[GateRule] = []
        for entry in data.get("rule", []):
            name = entry.get("name")
            globs = entry.get("globs")
            gates = entry.get("gates")
            if not isinstance(name, str) or not name:
                raise DriverError(f"gate rule missing string `name`: {entry!r}")
            if not isinstance(globs, list) or not all(
                isinstance(g, str) for g in globs
            ):
                raise DriverError(f"gate rule {name!r} `globs` must be a list of str")
            if not isinstance(gates, list) or not all(
                isinstance(g, str) for g in gates
            ):
                raise DriverError(f"gate rule {name!r} `gates` must be a list of str")
            rules.append(
                GateRule(
                    name=name,
                    globs=globs,
                    gates=gates,
                    description=str(entry.get("description", "")),
                    require_fresh_toolchain=bool(
                        entry.get("require_fresh_toolchain", False)
                    ),
                )
            )
        always = data.get("always", [])
        if not isinstance(always, list) or not all(isinstance(g, str) for g in always):
            raise DriverError("gate manifest `always` must be a list of str")
        return GateConfig(rules=rules, always=list(always))

    def select(self, touched: list[str]) -> tuple[list[str], list[GateRule]]:
        """Return (ordered unique gate commands, matched rules) for touched paths.

        A rule matches when ANY touched path matches ANY of its globs. The
        `always` gates run for every non-empty change. Order: `always` first,
        then rules in manifest order; duplicates are de-duplicated preserving
        first occurrence (so a determinism gate listed by two rules runs once).
        """
        selected: list[str] = []
        matched_rules: list[GateRule] = []

        def _add(cmds: list[str]) -> None:
            for cmd in cmds:
                if cmd not in selected:
                    selected.append(cmd)

        if touched:
            _add(self.always)
        for rule in self.rules:
            if any(_is_ignored(path, tuple(rule.globs)) for path in touched):
                matched_rules.append(rule)
                _add(rule.gates)
        return selected, matched_rules


# --------------------------------------------------------------------------
# python-oracle (hazard 7): resolve + PIN + verify a CPython version
# --------------------------------------------------------------------------


def _verify_interpreter_version(exe: str, want: str) -> tuple[bool, str]:
    """Run `exe -c 'print(sys.version)'` and confirm it starts with `want`.

    Returns (ok, reported_version_first_line). The check is the major.minor
    PREFIX match against `sys.version_info`, computed by the interpreter itself
    (not parsed from a path or a `uv` claim), so a .venv symlink flip cannot
    masquerade as the requested version.
    """
    proc = _run_driver_command(
        [
            exe,
            "-c",
            "import sys; print('%d.%d' % sys.version_info[:2]); "
            "print(sys.version.replace(chr(10), ' '))",
        ],
        timeout=30.0,
    )
    if proc.returncode != 0:
        return False, proc.stderr.strip() or f"exit {proc.returncode}"
    lines = proc.stdout.strip().splitlines()
    if not lines:
        return False, "no output"
    reported_mm = lines[0].strip()
    full = lines[1].strip() if len(lines) > 1 else reported_mm
    return reported_mm == want, full


def resolve_python(version: str, *, prefer_uv: bool = True) -> str:
    """Resolve a CPython interpreter PINNED to `version` (e.g. '3.12').

    Resolution order, each VERIFIED before acceptance (hazard 7 — never trust a
    name; verify `sys.version_info`):
      1. `uv python find <version>` (when uv is present and prefer_uv) — uv's
         own managed interpreter for that exact version.
      2. the bare `python<version>` / `pythonX.Y` on PATH.
      3. the current `sys.executable`, only if it already IS `version`.
    The first candidate whose interpreter self-reports `version` wins. If none
    does, raise LOUDLY listing what each candidate reported (so a flip is
    diagnosable, never silent).
    """
    if version.count(".") != 1 or not all(p.isdigit() for p in version.split(".")):
        raise DriverError(
            f"python version {version!r} must be 'MAJOR.MINOR' (e.g. '3.12')",
            code=EXIT_USAGE,
        )

    candidates: list[tuple[str, str]] = []  # (label, exe)
    if prefer_uv and shutil.which("uv"):
        proc = _run_driver_command(
            ["uv", "python", "find", version],
            timeout=30.0,
        )
        if proc.returncode == 0:
            exe = proc.stdout.strip().splitlines()[0].strip() if proc.stdout else ""
            if exe:
                candidates.append((f"uv python find {version}", exe))
    bare = shutil.which(f"python{version}")
    if bare:
        candidates.append((f"python{version} on PATH", bare))
    candidates.append(("sys.executable", sys.executable))

    reports: list[str] = []
    for label, exe in candidates:
        ok, reported = _verify_interpreter_version(exe, version)
        if ok:
            return exe
        reports.append(f"      {label}: {exe} -> reported {reported!r}")

    raise DriverError(
        f"could not resolve a verified CPython {version}. Candidates tried:\n"
        + "\n".join(reports)
        + "\n    (hazard 7: an interpreter that does not self-report the "
        "requested version is refused, never used.)"
    )


# --------------------------------------------------------------------------
# verify-toolchain (hazard 6): behavior-marker probe + binary freshness
# --------------------------------------------------------------------------

# Source globs whose newest commit defines "the toolchain". A built binary
# OLDER than the newest such commit is stale (its behavior predates the source).
RUST_SOURCE_GLOBS = (
    "runtime/**/*.rs",
    "runtime/**/Cargo.toml",
    "Cargo.toml",
    "Cargo.lock",
)


def _file_contains(path: Path, needle: str) -> bool:
    """True iff `needle` occurs in the file's bytes (python op, never `grep`).

    Reads as bytes and decodes errors-replace so a binary artifact (or a log
    with stray bytes) never raises; the membership test is exact-substring.
    """
    if not path.exists():
        return False
    try:
        data = path.read_bytes()
    except OSError:
        return False
    return needle.encode("utf-8") in data


def _binaries_identical(a: Path, b: Path) -> bool:
    """True iff two files are byte-identical (filecmp deep compare, hazard 5).

    Uses filecmp.cmp(shallow=False) — a real content comparison, never `cmp`/
    `diff` shell text whose output a proxy could rewrite. The "are these two
    binaries identical" verdict (e.g. confirming a rebuild actually CHANGED the
    artifact, so a stale binary can never be mistaken for a fresh one) is a pure
    python file op.
    """
    if not a.exists() or not b.exists():
        return False
    return filecmp.cmp(str(a), str(b), shallow=False)


def newest_rust_commit_epoch(git: Git) -> tuple[int, str] | None:
    """(committer-epoch, sha) of the newest commit touching a Rust-source glob.

    Used to judge binary freshness: a binary's mtime must be >= this epoch to
    be the product of current sources. Returns None when no such commit exists
    (e.g. a shallow clone with no history for those paths) — the caller then
    reports freshness as UNKNOWN rather than asserting a false pass.
    """
    args = [
        "log",
        "-1",
        "--format=%ct %H",
        "--",
        *RUST_SOURCE_GLOBS,
    ]
    proc = git._run(args, check=False)
    line = proc.stdout.strip()
    if proc.returncode != 0 or not line:
        return None
    epoch_str, _, sha = line.partition(" ")
    try:
        return int(epoch_str), sha.strip()
    except ValueError:
        return None


@dataclass
class ToolchainReport:
    binary: str
    binary_exists: bool
    binary_mtime: float | None
    newest_rust_epoch: int | None
    newest_rust_sha: str | None
    fresh: bool | None  # None == unknown (no reference commit)
    marker_required: str | None
    marker_found: bool | None  # None == not probed
    probe_exit: int | None
    # None == no reference given; True == binary DIFFERS from the reference
    # (a rebuild that genuinely changed the artifact); False == byte-identical
    # to the reference (suspicious: the rebuild produced the SAME binary).
    differs_from_reference: bool | None = None

    def to_dict(self) -> dict:
        return {
            "binary": self.binary,
            "binary_exists": self.binary_exists,
            "binary_mtime": self.binary_mtime,
            "newest_rust_epoch": self.newest_rust_epoch,
            "newest_rust_sha": self.newest_rust_sha,
            "fresh": self.fresh,
            "marker_required": self.marker_required,
            "marker_found": self.marker_found,
            "probe_exit": self.probe_exit,
            "differs_from_reference": self.differs_from_reference,
        }


def _resolve_safe_run(probed_repo: Path) -> Path:
    """Locate tools/safe_run.py (the mandatory binary guard).

    Prefers the driver's own repo (DEFAULT_REPO) since safe_run ships with this
    driver; falls back to the probed repo's copy. Raises if neither exists, so a
    binary is NEVER run unguarded (CLAUDE.md: no raw binary execution).
    """
    candidates = [
        DEFAULT_REPO / "tools" / "safe_run.py",
        probed_repo / "tools" / "safe_run.py",
    ]
    for c in candidates:
        if c.exists():
            return c
    raise DriverError(
        "tools/safe_run.py not found in the driver repo or the probed repo; "
        "refusing to run the binary unguarded (a runaway could OOM the host)."
    )


def verify_toolchain(
    git: Git,
    binary: Path,
    *,
    marker: str | None,
    probe_args: list[str],
    rss_mb: int,
    timeout: int,
    reference: Path | None = None,
) -> ToolchainReport:
    """Probe a built binary for a behavior marker and judge its freshness.

    Freshness (hazard 6): binary mtime >= newest Rust-source commit epoch. The
    marker probe RUNS the binary (always under tools/safe_run.py so a runaway
    can never OOM the host) and checks the marker substring in its combined
    output via a python op. A green marker on a STALE binary is still flagged
    stale, so a pass can never be misattributed to an old artifact.
    """
    exists = binary.exists()
    mtime = binary.stat().st_mtime if exists else None
    ref = newest_rust_commit_epoch(git)
    newest_epoch = ref[0] if ref else None
    newest_sha = ref[1] if ref else None
    if mtime is None or newest_epoch is None:
        fresh: bool | None = None
    else:
        fresh = mtime >= newest_epoch

    marker_found: bool | None = None
    probe_exit: int | None = None
    if marker is not None:
        if not exists:
            marker_found = False
        else:
            # Always route the binary through safe_run.py (CLAUDE.md: never run
            # a compiled binary unguarded). safe_run.py is part of THIS driver's
            # canonical toolchain, so resolve it relative to the driver's own
            # repo (DEFAULT_REPO) — NOT the arbitrary repo being probed, which
            # may be a detached worktree/clone without tools/. Fall back to the
            # probed repo's copy only if the driver's is somehow absent.
            safe_run = _resolve_safe_run(git.repo)
            out_path = Path(
                tempfile.mkstemp(prefix="molt_dev_probe_", suffix=".out")[1]
            )
            try:
                cmd = [
                    sys.executable,
                    str(safe_run),
                    "--rss-mb",
                    str(rss_mb),
                    "--timeout",
                    str(timeout),
                    "--quiet",
                    "--",
                    str(binary),
                    *probe_args,
                ]
                proc = _run_driver_command_bytes(
                    cmd,
                    cwd=git.repo,
                    env=os.environ.copy(),
                    timeout=float(timeout) + 30.0,
                    prefix="MOLT_DEV",
                )
                out_path.write_bytes((proc.stdout or b"") + (proc.stderr or b""))
                probe_exit = proc.returncode
                marker_found = _file_contains(out_path, marker)
            finally:
                out_path.unlink(missing_ok=True)

    differs: bool | None = None
    if reference is not None:
        differs = not _binaries_identical(binary, reference)

    return ToolchainReport(
        binary=str(binary),
        binary_exists=exists,
        binary_mtime=mtime,
        newest_rust_epoch=newest_epoch,
        newest_rust_sha=newest_sha,
        fresh=fresh,
        marker_required=marker,
        marker_found=marker_found,
        probe_exit=probe_exit,
        differs_from_reference=differs,
    )


# --------------------------------------------------------------------------
# probe (hazard 9): file size+mtime / pid liveness via python ops
# --------------------------------------------------------------------------


def probe_path(path: Path) -> dict:
    """Size + mtime for a file via os.stat (never `ls`/`stat` shell tools)."""
    if not path.exists():
        return {"path": str(path), "exists": False}
    st = path.stat()
    return {
        "path": str(path),
        "exists": True,
        "size": st.st_size,
        "mtime": st.st_mtime,
        "mtime_iso": time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(st.st_mtime)),
        "age_s": round(time.time() - st.st_mtime, 3),
    }


def probe_pid(pid: int) -> dict:
    """Liveness of a pid via os.kill(pid, 0) (never `ps`/`kill -0` shell text)."""
    alive: bool
    detail = ""
    try:
        os.kill(pid, 0)
        alive = True
    except ProcessLookupError:
        alive = False
    except PermissionError:
        # Exists but owned by another user — still ALIVE for liveness purposes.
        alive = True
        detail = "owned by another user"
    except OSError as exc:
        alive = False
        detail = str(exc)
    return {"pid": pid, "alive": alive, "detail": detail}


# --------------------------------------------------------------------------
# Marker checks (hazard 8): content-marker verification, python file ops
# --------------------------------------------------------------------------


@dataclass
class Marker:
    kind: str  # "file-exists" | "file-contains"
    path: str
    needle: str | None = None

    @staticmethod
    def parse(spec: str) -> "Marker":
        """Parse a CLI marker spec.

        Forms:
          exists:<repo-rel-path>
          contains:<repo-rel-path>::<needle>
        """
        if spec.startswith("exists:"):
            return Marker(kind="file-exists", path=spec[len("exists:") :])
        if spec.startswith("contains:"):
            body = spec[len("contains:") :]
            if "::" not in body:
                raise DriverError(
                    f"marker {spec!r} must be 'contains:<path>::<needle>'",
                    code=EXIT_USAGE,
                )
            path, needle = body.split("::", 1)
            return Marker(kind="file-contains", path=path, needle=needle)
        raise DriverError(
            f"unknown marker {spec!r}; use 'exists:<path>' or "
            "'contains:<path>::<needle>'",
            code=EXIT_USAGE,
        )

    def check(self, repo: Path) -> tuple[bool, str]:
        target = repo / self.path
        if self.kind == "file-exists":
            ok = target.exists()
            return ok, f"exists({self.path})"
        # file-contains
        assert self.needle is not None
        ok = _file_contains(target, self.needle)
        return ok, f"contains({self.path}, {self.needle!r})"


# --------------------------------------------------------------------------
# Gate execution
# --------------------------------------------------------------------------


def _resolve_repo_venv(repo: Path) -> Path | None:
    """Resolve the repository virtualenv for gate subprocesses, worktree-aware.

    Worktrees do not carry a .venv; the canonical venv lives in the MAIN
    checkout. Resolution order (first hit wins): the repo's own .venv, the
    main checkout's .venv (via `git rev-parse --git-common-dir`, whose parent
    is the main worktree), an already-set MOLT_VENV. Returns None if nothing
    resolves — gates that need the venv then fail loudly with venv_exec's own
    diagnostic, which names the remedy.
    """
    own = repo / ".venv"
    if (own / "bin" / "python").exists():
        return own
    proc = _run_driver_command(
        ["git", "rev-parse", "--git-common-dir"],
        cwd=repo,
        timeout=30.0,
    )
    if proc.returncode == 0:
        common = Path(proc.stdout.strip())
        if not common.is_absolute():
            common = (repo / common).resolve()
        main_repo = common.parent
        cand = main_repo / ".venv"
        if (cand / "bin" / "python").exists():
            return cand
    envv = os.environ.get("MOLT_VENV")
    if envv and (Path(envv) / "bin" / "python").exists():
        return Path(envv)
    return None


def run_gate(cmd: str, repo: Path, env: dict[str, str]) -> int:
    """Run one gate command string in the repo with the given env.

    The command is a shell string from the manifest/--extra-gate (so it can use
    pipes/&&); it runs with shell=True and cwd=repo. Output streams live to the
    caller's stdout/stderr (no capture) so a long gate is observable.

    MOLT_VENV is injected (worktree-aware, see _resolve_repo_venv) so manifest
    gates routed through tools/venv_exec.py resolve the canonical pinned
    toolchain from ANY worktree — gate verdicts must never depend on host
    PATH state.
    """
    _say(f"    gate: {cmd}")
    if "MOLT_VENV" not in env:
        venv = _resolve_repo_venv(repo)
        if venv is not None:
            env = {**env, "MOLT_VENV": str(venv)}
    return _run_live_gate(cmd, repo=repo, env=env)


# --------------------------------------------------------------------------
# Subcommand: integrate
# --------------------------------------------------------------------------


def _verify_no_dropped_commits(
    git: Git,
    source_shas: list[str],
    pre_patch_ids: dict[str, str],
    new_range: str,
    upstream_ref: str,
) -> None:
    """Hazard 1: every source commit's patch-id must survive the rebase.

    For each pre-rebase source sha we computed its patch-id BEFORE the rebase.
    After the rebase, a source commit is accounted for iff its patch-id appears
    EITHER in the new branch range (it was replayed) OR already in the upstream
    (it landed upstream independently — the legitimate "moved upstream" dedup).
    Any source patch-id present in neither is a DROPPED commit -> LOUD FAIL.
    """
    new_pids = set(git.patch_ids_for_range(new_range))
    # Upstream range that could legitimately already contain a source change:
    # the merge-base..upstream span (what the rebase replayed onto).
    upstream_pids = set(
        git.patch_ids_for_range(f"{new_range.split('..')[0]}..{upstream_ref}")
    )
    dangling: list[str] = []
    empty: list[str] = []
    for sha in source_shas:
        pid = pre_patch_ids.get(sha)
        if pid is None:
            # An empty-diff source commit (e.g. an empty merge): nothing to
            # account for by patch-id. Record it for transparency, not failure.
            empty.append(sha)
            continue
        if pid in new_pids or pid in upstream_pids:
            continue
        dangling.append(sha)
    if empty:
        _warn(
            "source commits with empty diffs (no patch-id, not patch-tracked): "
            + ", ".join(git.short(s) for s in empty)
        )
    if dangling:
        raise DriverError(
            "REBASE DROPPED COMMITS (hazard 1): the following source commits have "
            "no matching patch-id in the rebased range and are not already "
            "upstream — they were silently dropped:\n"
            + "\n".join(f"      {git.short(s)}  {_subject(git, s)}" for s in dangling)
            + "\n    Refusing to proceed. Investigate the rebase (a conflicting "
            "resolution may have discarded them)."
        )
    _ok(
        f"all {len(source_shas)} source commit(s) accounted for by patch-id "
        "(replayed or already-upstream)"
    )


def _subject(git: Git, sha: str) -> str:
    return git._run(["log", "-1", "--format=%s", sha]).stdout.strip()


def cmd_integrate(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    git = Git(repo)
    remote = args.remote
    branch = args.branch
    upstream_ref = f"{remote}/{branch}"
    ignore_globs = tuple(DEFAULT_IGNORE_GLOBS) + tuple(args.ignore or ())
    dry = args.dry_run

    mode = "DRY-RUN (no mutations)" if dry else "LIVE"
    _step(f"integrate [{mode}] repo={repo} upstream={upstream_ref}")

    # --- preflight: HEAD must be a real commit; record source state ---------
    head_before = git.head_sha()
    sym = git.symbolic_head()
    _say(f"    HEAD={git.short(head_before)} ({sym or 'detached'})")

    # --- step 1: fetch upstream --------------------------------------------
    _step("fetch upstream")
    if dry:
        _say(f"    would: git fetch {remote} {branch}")
    else:
        git.fetch(remote, branch)
    upstream_before = git.rev_parse(upstream_ref)
    _ok(f"{upstream_ref} = {git.short(upstream_before)}")

    # The source commits we are integrating: everything on HEAD not yet on
    # upstream. Captured with patch-ids BEFORE any rebase (hazard 1).
    source_range = f"{upstream_ref}..HEAD"
    source_shas = git.commits_in_range(source_range)
    if not source_shas:
        _ok("no commits to integrate (HEAD already on upstream); nothing to do")
        # Still verify tree cleanliness so an idempotent re-run reports state.
        dirty = _tracked_dirty(git, ignore_globs)
        if dirty:
            _warn(
                "working tree has tracked changes (not committed): "
                + ", ".join(f"{xy} {p}" for xy, p in dirty)
            )
        return EXIT_OK
    _say(
        f"    integrating {len(source_shas)} commit(s):\n"
        + "\n".join(f"      {git.short(s)}  {_subject(git, s)}" for s in source_shas)
    )
    pre_patch_ids = {sha: git.patch_id(sha) for sha in source_shas}

    # --- step 2: rebase onto upstream (only if behind) ----------------------
    _step(f"rebase onto {upstream_ref}")
    behind = git.commits_in_range(f"HEAD..{upstream_ref}")
    if not behind:
        _ok("HEAD already contains upstream tip; no rebase needed")
        new_range = source_range
    elif dry:
        _say(
            f"    would: git rebase {upstream_ref}  "
            f"(HEAD is behind by {len(behind)} commit(s))"
        )
        # In dry-run we cannot produce the post-rebase range; we VERIFY the
        # patch-ids against the current state as a best-effort preview.
        new_range = source_range
    else:
        proc = git._run(["rebase", upstream_ref], check=False)
        if proc.returncode != 0:
            # Abort a half-applied rebase so the tree is never left mid-conflict.
            git._run(["rebase", "--abort"], check=False)
            raise DriverError(
                f"rebase onto {upstream_ref} failed (conflicts?). Aborted the "
                "rebase to restore the pre-rebase HEAD. Resolve manually, then "
                f"re-run integrate.\n{proc.stdout.strip()}\n{proc.stderr.strip()}"
            )
        new_head = git.head_sha()
        new_range = f"{upstream_ref}..HEAD"
        _ok(f"rebased; HEAD {git.short(head_before)} -> {git.short(new_head)}")

    # --- step 3: verify no commits dropped (hazard 1) -----------------------
    _step("verify no dropped commits (patch-id survival)")
    if dry and behind:
        _warn(
            "dry-run cannot verify post-rebase patch-ids (no rebase performed); "
            "the live run verifies them"
        )
    else:
        _verify_no_dropped_commits(
            git, source_shas, pre_patch_ids, new_range, upstream_before
        )

    # --- step 4: content markers (hazard 8) ---------------------------------
    markers = [Marker.parse(spec) for spec in (args.marker or [])]
    if markers:
        _step("verify content markers")
        marker_failures: list[str] = []
        for m in markers:
            ok, desc = m.check(repo)
            if ok:
                _ok(desc)
            else:
                _fail(desc)
                marker_failures.append(desc)
        if marker_failures:
            raise DriverError(
                "content marker(s) failed (hazard 8): "
                + "; ".join(marker_failures)
                + ". The post-rebase tree does not contain the declared change "
                "markers — the integration is not what was intended."
            )

    # --- step 5: gates (hazard 10) ------------------------------------------
    gate_cmds: list[str] = []
    matched_rules: list[GateRule] = []
    require_fresh = False
    if args.no_gates:
        _step("gates SKIPPED (--no-gates)")
    else:
        gates_config_path = (
            Path(args.gates_config).resolve()
            if args.gates_config
            else repo / "tools" / GATES_CONFIG_NAME
        )
        config = GateConfig.load(gates_config_path)
        # Touched paths across the integrated commits drive change-class.
        touched = sorted(
            set(
                git._run(
                    ["diff", "--name-only", f"{upstream_before}..HEAD"]
                ).stdout.splitlines()
            )
        )
        gate_cmds, matched_rules = config.select(touched)
        require_fresh = any(r.require_fresh_toolchain for r in matched_rules)
        gate_cmds = list(gate_cmds) + list(args.extra_gate or [])
        _step(
            f"gates ({len(gate_cmds)} selected for {len(touched)} touched path(s); "
            f"matched rules: {', '.join(r.name for r in matched_rules) or 'none'})"
        )
        if not gate_cmds:
            _ok("no gates selected for this change-class")
        env = dict(os.environ)
        env.setdefault("MOLT_SESSION_ID", args.session_id)
        env.setdefault("PYTHONPATH", str(repo / "src"))
        for cmd in gate_cmds:
            if dry:
                _say(f"    would-run gate: {cmd}")
                continue
            rc = run_gate(cmd, repo, env)
            if rc != 0:
                raise DriverError(
                    f"gate failed (exit {rc}): {cmd}\n    Integration halts "
                    "before push; fix the gate and re-run."
                )
            _ok(f"gate passed: {cmd}")

    # Optional toolchain-freshness requirement (gate manifest opt-in).
    if require_fresh and args.toolchain_binary:
        _step("verify toolchain freshness (gate-required)")
        report = verify_toolchain(
            git,
            Path(args.toolchain_binary).resolve(),
            marker=args.toolchain_marker,
            probe_args=list(args.toolchain_probe_arg or []),
            rss_mb=args.rss_mb,
            timeout=args.timeout,
        )
        _print_toolchain_report(report)
        if report.fresh is False:
            raise DriverError(
                "toolchain binary is STALE (hazard 6): its mtime predates the "
                "newest Rust-source commit. A gate result on a stale binary is "
                "untrustworthy. Rebuild and re-run."
            )
    elif require_fresh:
        _warn(
            "a matched gate rule requires a fresh toolchain but no "
            "--toolchain-binary was given; freshness NOT verified"
        )

    # --- step 6: push (only after every gate is green) ----------------------
    _step(f"push to {upstream_ref}")
    head_to_push = git.head_sha()
    if dry:
        _say(
            f"    would: git push {remote} HEAD:{branch}  (tip {git.short(head_to_push)})"
        )
        _say("    would: confirm by fetch + ancestor check (hazard 2)")
        _say("    would: cleanup gate (hazard 3) after confirmation")
        _ok("dry-run complete: all pre-push checks passed; no mutation performed")
        return EXIT_OK
    if args.no_push:
        _ok(
            f"push SKIPPED (--no-push); tip {git.short(head_to_push)} ready. "
            "Run `molt_dev.py verify-push` after pushing externally."
        )
        return EXIT_OK
    push = git._run(["push", remote, f"HEAD:{branch}"], check=False)
    if push.returncode != 0:
        # Do NOT trust the exit code as final — but a non-zero exit with no
        # landed tip is a real failure; the confirm step below is authoritative.
        _warn(
            f"git push exited {push.returncode}; NOT trusting it (hazard 2). "
            "Confirming by fetch+ancestor instead.\n"
            f"{push.stderr.strip()}"
        )

    # --- step 7: confirm the push by FETCH + ANCESTOR (hazard 2) ------------
    _step("confirm push landed (fetch + ancestor, not exit code)")
    git.fetch(remote, branch)
    upstream_after = git.rev_parse(upstream_ref)
    landed = git.is_ancestor(head_to_push, upstream_ref)
    if not landed:
        raise DriverError(
            f"PUSH NOT CONFIRMED (hazard 2): {upstream_ref} is now "
            f"{git.short(upstream_after)} but does NOT contain the pushed tip "
            f"{git.short(head_to_push)} (ancestor check failed). The push did "
            "not land (a swallowed exit code, a race, or a rejected non-fast-"
            "forward). Cleanup is REFUSED. Investigate and re-run."
        )
    _ok(
        f"confirmed: {upstream_ref} ({git.short(upstream_after)}) contains tip "
        f"{git.short(head_to_push)}"
    )

    # --- step 8: cleanup gate (hazard 3) ------------------------------------
    if args.cleanup_worktree:
        _step("cleanup gate (refuses on unpushed/dirty)")
        rc = _cleanup_worktree(git, repo, upstream_ref, ignore_globs, force_sha=None)
        if rc != 0:
            return rc
    else:
        _ok("cleanup not requested (--cleanup-worktree to remove this worktree)")

    _ok("integrate complete: pushed + confirmed.")
    return EXIT_OK


def _print_toolchain_report(report: ToolchainReport) -> None:
    _say(f"    binary: {report.binary} (exists={report.binary_exists})")
    if report.binary_mtime is not None:
        _say(
            "    binary mtime: "
            + time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(report.binary_mtime))
        )
    if report.newest_rust_sha:
        _say(
            f"    newest Rust commit: {report.newest_rust_sha[:9]} "
            f"(epoch {report.newest_rust_epoch})"
        )
    if report.fresh is None:
        _warn("freshness UNKNOWN (missing binary or no reference commit)")
    elif report.fresh:
        _ok("binary is FRESH (mtime >= newest Rust commit)")
    else:
        _fail("binary is STALE (mtime < newest Rust commit)")
    if report.marker_required is not None:
        if report.marker_found:
            _ok(
                f"behavior marker present: {report.marker_required!r} "
                f"(probe exit {report.probe_exit})"
            )
        else:
            _fail(
                f"behavior marker ABSENT: {report.marker_required!r} "
                f"(probe exit {report.probe_exit})"
            )
    if report.differs_from_reference is not None:
        if report.differs_from_reference:
            _ok("binary DIFFERS from the reference (a genuine rebuild)")
        else:
            _warn(
                "binary is BYTE-IDENTICAL to the reference — the rebuild "
                "produced the same artifact (stale-binary suspicion)"
            )


# --------------------------------------------------------------------------
# Subcommand: secure-wip (hazard 4)
# --------------------------------------------------------------------------


def cmd_secure_wip(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    git = Git(repo)
    ignore_globs = tuple(DEFAULT_IGNORE_GLOBS) + tuple(args.ignore or ())
    _step(f"secure-wip repo={repo}")

    # Enumerate ALL tracked modifications (staged + unstaged + intentional
    # additions): both index ('staged') and worktree ('unstaged') changes, plus
    # added files that git is tracking ('A'). Untracked ('??') files are
    # captured only with --include-untracked (off by default: scratch stays out).
    to_add: list[str] = []
    skipped_ignored: list[str] = []
    untracked: list[str] = []
    for xy, path in git.status_porcelain():
        if _is_ignored(path, ignore_globs):
            skipped_ignored.append(path)
            continue
        if xy == "??":
            untracked.append(path)
            if args.include_untracked:
                to_add.append(path)
            continue
        to_add.append(path)

    if not to_add:
        _ok("no tracked modifications to secure (working tree clean)")
        if untracked and not args.include_untracked:
            _warn(
                "untracked files present (NOT captured; use --include-untracked): "
                + ", ".join(sorted(untracked))
            )
        return EXIT_OK

    to_add = sorted(set(to_add))
    _say(
        f"    staging {len(to_add)} path(s) into ONE recovery commit:\n"
        + "\n".join(f"      {p}" for p in to_add)
    )
    if skipped_ignored:
        _say("    excluded (ignore set): " + ", ".join(sorted(set(skipped_ignored))))
    if untracked and not args.include_untracked:
        _warn(
            "untracked files NOT captured (use --include-untracked): "
            + ", ".join(sorted(untracked))
        )

    if args.dry_run:
        _ok("dry-run: would `git add` the above paths and commit (no mutation)")
        return EXIT_OK

    # Stage with an EXPLICIT pathspec (never `git add -A`) so the ignore set is
    # truly honored and a partner's excluded churn is not swept in.
    git._run(["add", "--", *to_add])
    subject = args.message or (
        f"{WIP_MARKER}: salvage tracked WIP "
        f"({len(to_add)} path(s)) from {git.short(git.head_sha())}"
    )
    if not subject.startswith(WIP_MARKER):
        subject = f"{WIP_MARKER}: {subject}"
    body = (
        "Recovery commit created by tools/molt_dev.py secure-wip. Captures all "
        "tracked staged+unstaged modifications in one commit so a split index/"
        "worktree state cannot be half-lost. Review and reword/squash before "
        "landing."
    )
    # Scope the commit to the EXACT pathspec (not the whole index): if an
    # excluded path was already staged (a partner's churn, or a wasm sha file),
    # `git commit -- <paths>` leaves it out of THIS commit, so the ignore set is
    # honored even against a pre-staged index. Untracked-but-now-added paths
    # were just `git add`ed above so they are in the pathspec too.
    commit = git._run(
        ["commit", "--no-verify", "-m", subject, "-m", body, "--", *to_add],
        check=False,
    )
    if commit.returncode != 0:
        raise DriverError(
            f"secure-wip commit failed:\n{commit.stdout.strip()}\n"
            f"{commit.stderr.strip()}"
        )
    new_head = git.head_sha()
    _ok(f"secured WIP in {git.short(new_head)}: {subject}")
    return EXIT_OK


# --------------------------------------------------------------------------
# Subcommand: verify-push (hazard 2, standalone)
# --------------------------------------------------------------------------


def cmd_verify_push(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    git = Git(repo)
    remote = args.remote
    branch = args.branch
    upstream_ref = f"{remote}/{branch}"
    tip = args.tip or git.head_sha()
    tip_full = git.rev_parse(tip)
    _step(f"verify-push tip={git.short(tip_full)} -> {upstream_ref}")

    git.fetch(remote, branch)
    upstream_after = git.rev_parse(upstream_ref)
    landed = git.is_ancestor(tip_full, upstream_ref)
    payload = {
        "tip": tip_full,
        "upstream_ref": upstream_ref,
        "upstream_sha": upstream_after,
        "landed": landed,
    }
    if args.json:
        print(json.dumps(payload))
    if landed:
        _ok(
            f"{upstream_ref} ({git.short(upstream_after)}) CONTAINS tip "
            f"{git.short(tip_full)} — push confirmed"
        )
        return EXIT_OK
    _fail(
        f"{upstream_ref} ({git.short(upstream_after)}) does NOT contain tip "
        f"{git.short(tip_full)} — push NOT confirmed (do not clean up)"
    )
    return EXIT_FAIL


# --------------------------------------------------------------------------
# Subcommand: verify-toolchain (hazard 6, standalone)
# --------------------------------------------------------------------------


def cmd_verify_toolchain(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    git = Git(repo)
    binary = Path(args.binary).resolve()
    reference = Path(args.reference).resolve() if args.reference else None
    report = verify_toolchain(
        git,
        binary,
        marker=args.marker,
        probe_args=list(args.probe_arg or []),
        rss_mb=args.rss_mb,
        timeout=args.timeout,
        reference=reference,
    )
    if args.json:
        print(json.dumps(report.to_dict()))
    _print_toolchain_report(report)

    failed = False
    if args.require_fresh and report.fresh is not True:
        failed = True
    if args.marker is not None and report.marker_found is not True:
        failed = True
    if args.require_differs and report.differs_from_reference is not True:
        failed = True
    return EXIT_FAIL if failed else EXIT_OK


# --------------------------------------------------------------------------
# Subcommand: probe (hazard 9)
# --------------------------------------------------------------------------


def cmd_probe(args: argparse.Namespace) -> int:
    results: list[dict] = []
    for raw in args.file or []:
        results.append(probe_path(Path(raw)))
    for pid in args.pid or []:
        results.append(probe_pid(pid))
    if not results:
        raise DriverError("probe: give at least one --file or --pid", code=EXIT_USAGE)
    if args.json:
        print(json.dumps(results, indent=2))
    else:
        for r in results:
            if "pid" in r:
                _say(
                    f"    pid {r['pid']}: alive={r['alive']}"
                    + (f" ({r['detail']})" if r["detail"] else "")
                )
            elif r.get("exists"):
                _say(
                    f"    {r['path']}: size={r['size']} mtime={r['mtime_iso']} "
                    f"age={r['age_s']}s"
                )
            else:
                _say(f"    {r['path']}: MISSING")
    # Exit non-zero if any probed file is missing or any pid is dead, so the
    # subcommand is usable as a CI/health assertion.
    bad = any(
        ("pid" in r and not r["alive"]) or ("exists" in r and not r["exists"])
        for r in results
    )
    return EXIT_FAIL if bad else EXIT_OK


# --------------------------------------------------------------------------
# Subcommand: python-oracle (hazard 7)
# --------------------------------------------------------------------------


def cmd_python_oracle(args: argparse.Namespace) -> int:
    exe = resolve_python(args.python_version, prefer_uv=not args.no_uv)
    ok, full = _verify_interpreter_version(exe, args.python_version)
    if not ok:
        # resolve_python already verified; this is belt-and-suspenders.
        raise DriverError(
            f"resolved interpreter {exe} no longer reports {args.python_version} "
            f"(now {full!r})"
        )
    if args.json:
        print(json.dumps({"python": exe, "version": args.python_version, "full": full}))
    else:
        print(exe)
    _ok(f"pinned CPython {args.python_version}: {exe} ({full})")
    return EXIT_OK


# --------------------------------------------------------------------------
# Subcommands: detached-run / detached-verify (hazard 11)
# --------------------------------------------------------------------------

# State root for detached daemons. Per-name dirs hold: pid, sid, cmd.json,
# run.log, rc. The rc file is the ONLY proof of orderly completion — a dead
# pid with no rc is the hazard-11 died-silent class, and detached-verify
# reports it as such.
DETACHED_STATE_ROOT = Path(tempfile.gettempdir()) / "molt_dev_detached"

_DETACHED_NAME_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")


def _detached_state_dir(name: str, override: str | None) -> Path:
    if not _DETACHED_NAME_RE.fullmatch(name):
        raise DriverError(
            f"detached: name {name!r} must match {_DETACHED_NAME_RE.pattern}",
            code=EXIT_USAGE,
        )
    root = Path(override).resolve() if override else DETACHED_STATE_ROOT
    return root / name


def _atomic_write_text(path: Path, text: str) -> None:
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


def _exec_wait_rc(command: list[str], env: dict[str, str]) -> int:
    """Exec ``command`` in a child and return shell-style exit status."""
    if not command:
        os.write(1, b"detached-run: empty command\n")
        return 127
    child = os.fork()
    if child == 0:
        try:
            os.execvpe(command[0], command, env)
        except FileNotFoundError as exc:
            os.write(1, f"detached-run: exec failed: {exc}\n".encode())
            os._exit(127)
        except Exception as exc:  # noqa: BLE001 - child must report every exec death
            os.write(1, f"detached-run: exec crashed: {exc}\n".encode())
            os._exit(126)
    while True:
        try:
            _, status = os.waitpid(child, 0)
            break
        except InterruptedError:
            continue
    if os.WIFEXITED(status):
        return os.WEXITSTATUS(status)
    if os.WIFSIGNALED(status):
        return 128 + os.WTERMSIG(status)
    return 126


def _detached_daemonize(
    state: Path, command: list[str], cwd: Path, env: dict[str, str]
) -> int:
    """Double-fork + setsid; the grandchild runs `command` and writes rc.

    Returns (in the ORIGINAL process) the daemon pid read back from the state
    dir. The grandchild NEVER returns: it forks/execs the command, records the
    exit status (127 exec-failure / 126 crash sentinels included), and
    `os._exit`s, so no parent atexit/exception machinery runs twice.
    """
    pid_f = state / "pid"
    first = os.fork()
    if first == 0:
        # First child: new session, then fork the real daemon and exit so the
        # daemon is reparented to init (no controlling terminal, no harness
        # process-group membership).
        os.setsid()
        second = os.fork()
        if second > 0:
            os._exit(0)
        # The daemon (grandchild).
        try:
            fd = os.open(state / "run.log", os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
            os.dup2(fd, 1)
            os.dup2(fd, 2)
            null = os.open(os.devnull, os.O_RDONLY)
            os.dup2(null, 0)
            _atomic_write_text(state / "sid", str(os.getsid(0)))
            _atomic_write_text(pid_f, str(os.getpid()))
            os.chdir(cwd)
            try:
                rc = _exec_wait_rc(command, env)
            except Exception as exc:  # noqa: BLE001 — daemon must record ANY death
                os.write(1, f"detached-run: daemon crashed: {exc}\n".encode())
                rc = 126
            _atomic_write_text(state / "rc", str(rc))
        finally:
            os._exit(0)
    # Original process: reap the first child (exits immediately post-fork) and
    # wait — bounded — for the daemon's pid file to appear.
    os.waitpid(first, 0)
    deadline = time.monotonic() + 5.0
    daemon_pid: int | None = None
    while time.monotonic() < deadline:
        if pid_f.exists():
            raw_pid = pid_f.read_text(encoding="utf-8").strip()
            if raw_pid:
                daemon_pid = int(raw_pid)
                break
        time.sleep(0.05)
    if daemon_pid is None:
        raise DriverError(f"detached-run: daemon never wrote {pid_f} within 5s")
    return daemon_pid


def cmd_detached_run(args: argparse.Namespace) -> int:
    command = list(args.command or [])
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        raise DriverError("detached-run: give the command after `--`", code=EXIT_USAGE)
    state = _detached_state_dir(args.name, args.state_dir)
    pid_f, rc_f = state / "pid", state / "rc"
    if pid_f.exists():
        old_pid = int(pid_f.read_text(encoding="utf-8").strip() or "0")
        if old_pid and probe_pid(old_pid)["alive"] and not rc_f.exists():
            raise DriverError(
                f"detached-run: {args.name!r} is already RUNNING (pid {old_pid}). "
                "This driver NEVER kills — wait, detached-verify it, or use a "
                "new --name."
            )
        if not args.replace:
            raise DriverError(
                f"detached-run: state for {args.name!r} already exists at "
                f"{state} (finished or died). Pass --replace to clear DEAD "
                "state and respawn."
            )
        shutil.rmtree(state)
    state.mkdir(parents=True, exist_ok=True)
    cwd = Path(args.cwd).resolve() if args.cwd else Path.cwd()
    if not cwd.is_dir():
        raise DriverError(
            f"detached-run: --cwd {cwd} is not a directory", code=EXIT_USAGE
        )
    env = dict(os.environ)
    # Unbuffered IO so a group-kill cannot eat block-buffered progress (the
    # empty-log signature that made hazard 11 undiagnosable).
    env["PYTHONUNBUFFERED"] = "1"
    for kv in args.env or []:
        key, sep, value = kv.partition("=")
        if not sep:
            raise DriverError(
                f"detached-run: --env needs K=V, got {kv!r}", code=EXIT_USAGE
            )
        env[key] = value
    _atomic_write_text(
        state / "cmd.json",
        json.dumps(
            {
                "argv": command,
                "cwd": str(cwd),
                "start_unix": time.time(),
                "env_overrides": list(args.env or []),
            },
            indent=2,
        ),
    )
    daemon_pid = _detached_daemonize(state, command, cwd, env)
    _ok(f"detached {args.name!r} spawned: pid {daemon_pid}")
    _say(f"    state: {state}")
    _say(f"    log:   {state / 'run.log'}")
    _say("    REQUIRED next step, in a LATER tool call (teardown of THIS call")
    _say("    is exactly what hazard 11 is about):")
    _say(
        f"      python3 tools/molt_dev.py detached-verify --name {args.name}"
        f" --min-age-s {args.verify_min_age_hint}"
    )
    if args.json:
        print(
            json.dumps({"name": args.name, "pid": daemon_pid, "state_dir": str(state)})
        )
    return EXIT_OK


def cmd_detached_verify(args: argparse.Namespace) -> int:
    state = _detached_state_dir(args.name, args.state_dir)
    pid_f, rc_f, log_f = state / "pid", state / "rc", state / "run.log"
    if not pid_f.exists():
        raise DriverError(
            f"detached-verify: no state for {args.name!r} at {state} "
            "(was detached-run ever invoked?)"
        )
    pid = int(pid_f.read_text(encoding="utf-8").strip())
    log_probe = probe_path(log_f)
    age_s = round(time.time() - pid_f.stat().st_mtime, 1)
    result: dict = {
        "name": args.name,
        "pid": pid,
        "age_s": age_s,
        "log_size": log_probe.get("size", 0) if log_probe.get("exists") else 0,
        "state_dir": str(state),
    }
    if rc_f.exists():
        rc = int(rc_f.read_text(encoding="utf-8").strip())
        result["status"], result["rc"] = "done", rc
        if args.json:
            print(json.dumps(result))
        if rc == 0:
            _ok(f"detached {args.name!r}: DONE rc=0 (log {result['log_size']}B)")
            return EXIT_OK
        _fail(f"detached {args.name!r}: DONE rc={rc} (log {result['log_size']}B)")
        return EXIT_FAIL
    if probe_pid(pid)["alive"]:
        if age_s < args.min_age_s:
            result["status"] = "too-young"
            if args.json:
                print(json.dumps(result))
            _fail(
                f"detached {args.name!r}: alive but only {age_s}s old "
                f"(< --min-age-s {args.min_age_s}); the spawning call's "
                "teardown window may still reap it — re-verify later."
            )
            return EXIT_FAIL
        result["status"] = "running"
        if args.json:
            print(json.dumps(result))
        _ok(
            f"detached {args.name!r}: RUNNING (pid {pid}, {age_s}s, "
            f"log {result['log_size']}B)"
        )
        return EXIT_OK
    result["status"] = "died-silent"
    if args.json:
        print(json.dumps(result))
    _fail(
        f"detached {args.name!r}: DIED-SILENT — pid {pid} is gone and no rc "
        f"was written (hazard-11 group-kill class). Log may be truncated by "
        f"lost buffers: {log_f} ({result['log_size']}B)"
    )
    return EXIT_FAIL


# --------------------------------------------------------------------------
# Worktree cleanup (hazard 3) — used by integrate and as no standalone (the
# refusal logic is shared so a force-abandon is always auditable)
# --------------------------------------------------------------------------


def _cleanup_worktree(
    git: Git,
    repo: Path,
    upstream_ref: str,
    ignore_globs: tuple[str, ...],
    *,
    force_sha: str | None,
) -> int:
    """Remove the worktree at `repo` ONLY when safe (hazard 3).

    REFUSES when:
      * unpushed commits exist (upstream_ref..HEAD non-empty), OR
      * tracked staged/unstaged (non-ignored) changes exist.
    --force (force_sha set) overrides BOTH, but only when force_sha names the
    exact HEAD being abandoned (so an abandon is deliberate + auditable).
    """
    unpushed = git.commits_in_range(f"{upstream_ref}..HEAD")
    dirty = _tracked_dirty(git, ignore_globs)
    head = git.head_sha()

    if unpushed or dirty:
        reasons: list[str] = []
        if unpushed:
            reasons.append(
                f"{len(unpushed)} unpushed commit(s): "
                + ", ".join(git.short(s) for s in unpushed)
            )
        if dirty:
            reasons.append(
                f"{len(dirty)} tracked change(s): "
                + ", ".join(f"{xy} {p}" for xy, p in dirty)
            )
        if force_sha is None:
            _fail(
                "REFUSING worktree cleanup (hazard 3): "
                + "; ".join(reasons)
                + f". HEAD={git.short(head)}. To abandon anyway, pass "
                f"--force {git.short(head)} (naming the sha makes the abandon "
                "deliberate)."
            )
            return EXIT_FAIL
        if git.rev_parse(force_sha) != head:
            _fail(
                f"--force {force_sha} does not match HEAD {git.short(head)}; "
                "refusing (the force target must name the exact abandoned sha)."
            )
            return EXIT_FAIL
        _warn(
            f"--force: abandoning worktree at HEAD {git.short(head)} despite: "
            + "; ".join(reasons)
        )

    # Safe (or force-confirmed): remove the worktree via git (so the admin
    # bookkeeping in the main repo's .git/worktrees is updated too).
    main_repo = _main_worktree(git)
    if main_repo is None:
        _fail(
            "could not determine the main worktree to run `git worktree remove` "
            "from; refusing to delete files directly."
        )
        return EXIT_FAIL
    if main_repo.resolve() == repo.resolve():
        _fail("refusing to remove the MAIN worktree; this targets disposable worktrees")
        return EXIT_FAIL
    remove_args = ["worktree", "remove"]
    # `git worktree remove` refuses (without --force) on ANY modified/untracked
    # file, including the ignored-set churn (e.g. wasm sha sidecars) our OWN
    # safety policy already cleared. Our policy is the authority: if we got here
    # with residual churn that is purely ignored/untracked, OR the operator
    # passed --force, tell git to proceed. We never reach here with non-ignored
    # tracked changes (those are refused above), so this --force can only be
    # waving through churn we already deemed safe.
    has_residual_churn = bool(git.status_porcelain())
    if force_sha is not None or has_residual_churn:
        remove_args.append("--force")
    remove_args.append(str(repo))
    proc = _run_driver_command(
        ["git", "-C", str(main_repo), *remove_args],
        timeout=120.0,
    )
    if proc.returncode != 0:
        _fail(
            f"git worktree remove failed (exit {proc.returncode}): "
            f"{proc.stderr.strip()}"
        )
        return EXIT_FAIL
    _ok(f"removed worktree {repo}")
    return EXIT_OK


def _main_worktree(git: Git) -> Path | None:
    """The main (non-linked) worktree path, from `git worktree list --porcelain`.

    The first `worktree <path>` line in the porcelain list is the main worktree.
    Parsed from the stable --porcelain form (not the human list), per hazard 5.
    """
    out = git._run(["worktree", "list", "--porcelain"]).stdout
    for line in out.splitlines():
        if line.startswith("worktree "):
            return Path(line[len("worktree ") :].strip())
    return None


def _difftest_toolchain_env(
    root: Path, session: str, extra_env: list[str] | None
) -> dict[str, str]:
    """Build the env that roots frontend AND runtime/backend at the SAME tree.

    This is the hazard-12 fix in one place (and the unit-testable core): set
    MOLT_PROJECT_ROOT (runtime/backend build root + fingerprint tree) and
    prepend <root>/src to PYTHONPATH (frontend import root) so they can never
    diverge. ``extra_env`` items are ``KEY=VALUE`` passthroughs applied to both
    build and run (e.g. a trace flag).
    """
    env = os.environ.copy()
    env["MOLT_PROJECT_ROOT"] = str(root)
    existing_pp = env.get("PYTHONPATH")
    env["PYTHONPATH"] = str(root / "src") + (
        os.pathsep + existing_pp if existing_pp else ""
    )
    env["MOLT_SESSION_ID"] = session
    for kv in extra_env or ():
        if "=" not in kv:
            raise DriverError(
                f"difftest: --env expects KEY=VALUE, got {kv!r}", code=EXIT_USAGE
            )
        key, value = kv.split("=", 1)
        env[key] = value
    return env


def _difftest_capture(
    cmd: list[str], *, env: dict[str, str], cwd: Path, timeout: int
) -> tuple[int, bytes, bytes]:
    """Run `cmd` capturing raw BYTES (not text) so stdout compares are exact.

    Returns (returncode, stdout, stderr). A timeout is a LOUD failure, not a
    swallowed hang — it raises DriverError so a wedged build/run never reads as
    a passing diff.
    """
    proc = _run_driver_command_bytes(
        cmd,
        cwd=cwd,
        env=env,
        timeout=timeout,
        prefix="MOLT_DIFF",
    )
    if getattr(proc, "timed_out", False):
        raise DriverError(
            f"difftest: command timed out after {timeout}s: {' '.join(cmd)}"
        )
    return proc.returncode, proc.stdout or b"", proc.stderr or b""


def _difftest_streams_match(expected: bytes, actual: bytes) -> bool:
    """Compare captured program output with host-stdio newline parity.

    CPython on Windows translates ``print(..., "\\n")`` to CRLF when stdout is a
    pipe; Molt's native runtime writes LF. The main differential harness compares
    text after newline normalization, so ``difftest`` must not report a Windows
    false negative once exit codes and logical output match.
    """
    if os.name == "nt":
        expected = expected.replace(b"\r\n", b"\n")
        actual = actual.replace(b"\r\n", b"\n")
    return expected == actual


def cmd_difftest(args: argparse.Namespace) -> int:
    """Build a program through a CONSISTENTLY-ROOTED toolchain and differential-
    test its native/LLVM/WASM output against pinned CPython.

    Hazard 12 (split-root toolchain): ``PYTHONPATH=<root>/src`` redirects ONLY
    the Python frontend. The Rust runtime+backend build is anchored to
    ``MOLT_PROJECT_ROOT`` (and the runtime-staticlib fingerprint is computed
    against THAT tree). Setting PYTHONPATH without MOLT_PROJECT_ROOT silently
    builds the frontend from <root> but the runtime/backend from the canonical
    checkout — so a runtime/backend edit in <root> is NEVER compiled in (its
    fingerprint never changes → no rebuild → stale behavior credited to a fix
    that never shipped, the exact stale-binary misattribution of hazard 6 but
    on the SOURCE-TREE axis). difftest derives BOTH from a single --root so the
    frontend, runtime, and backend are one coherent tree, and forces a fresh
    --output per target so a stale artifact can't be re-run.
    """
    root = Path(args.root).resolve()
    if not (root / "src" / "molt" / "cli" / "__init__.py").exists():
        raise DriverError(
            f"difftest: --root {root} is not a molt checkout (no src/molt/cli/__init__.py)",
            code=EXIT_USAGE,
        )
    program = Path(args.program).resolve()
    if not program.exists():
        raise DriverError(f"difftest: program {program} not found", code=EXIT_USAGE)
    safe_run = root / "tools" / "safe_run.py"
    if not safe_run.exists():
        raise DriverError(f"difftest: missing {safe_run}")

    if not args.target:
        args.target = ["native"]
    # The interpreter must BOTH run the molt frontend (needs molt's install
    # deps: packaging, etc.) AND be the CPython oracle. The interpreter running
    # THIS driver is by construction the dev python that carries those deps, so
    # difftest uses it for both build and oracle — after VERIFYING it reports
    # the requested version (hazard 7) and can actually import molt from --root
    # (a loud refusal, never a mid-build ImportError credited as a test fail).
    # --oracle-python overrides the interpreter explicitly.
    cpython = args.oracle_python or sys.executable
    ok, full = _verify_interpreter_version(cpython, args.python_version)
    if not ok:
        raise DriverError(
            f"difftest: interpreter {cpython} reports {full!r}, not "
            f"{args.python_version}. Run molt_dev.py under python "
            f"{args.python_version} (it doubles as the byte-exact CPython "
            f"oracle), or pass --oracle-python.",
            code=EXIT_USAGE,
        )
    prog_args = list(args.prog_args or [])

    # One coherent toolchain root: frontend (PYTHONPATH) AND runtime/backend
    # (MOLT_PROJECT_ROOT) both resolve to --root. This is the hazard-12 fix.
    base_env = _difftest_toolchain_env(root, args.session, args.env)

    _step(f"difftest {program.name} root={root} targets={args.target}")
    _ok(f"toolchain rooted: MOLT_PROJECT_ROOT + PYTHONPATH -> {root}")

    # Loud refusal if the frontend isn't importable (deps gap), so a missing
    # dependency is never mistaken for a compiler bug in the diff.
    probe = _run_driver_command(
        [cpython, "-c", "import molt.cli"],
        env=base_env,
        cwd=root,
        timeout=30.0,
        prefix="MOLT_DIFF",
    )
    if probe.returncode != 0:
        last = (probe.stderr.strip().splitlines() or ["<no stderr>"])[-1]
        raise DriverError(
            f"difftest: {cpython} cannot import molt from {root} "
            f"(frontend deps missing?): {last}"
        )
    _ok(f"interpreter {cpython} ({full}) imports molt from {root}")

    # CPython oracle (the same verified interpreter; programs need no molt deps).
    cpy_rc, cpy_out, _ = _difftest_capture(
        [cpython, str(program), *prog_args],
        env=base_env,
        cwd=root,
        timeout=args.timeout,
    )
    _ok(
        f"CPython {args.python_version} oracle: exit={cpy_rc} ({len(cpy_out)} bytes stdout)"
    )

    failures: list[str] = []
    out_dir = Path(args.out_dir)
    if not out_dir.is_absolute():
        out_dir = root / out_dir
    for target in args.target:
        _step(f"target {target}")
        out_bin = out_dir / f"difftest_{program.stem}_{target}"
        out_bin.parent.mkdir(parents=True, exist_ok=True)
        if out_bin.exists():
            out_bin.unlink()  # never re-run a stale artifact (hazard 6)
        build_cmd = [
            cpython,
            "-m",
            "molt",
            "build",
            "--target",
            target,
            "--output",
            str(out_bin),
            str(program),
        ]
        brc, _bo, be = _difftest_capture(
            build_cmd, env=base_env, cwd=root, timeout=args.build_timeout
        )
        if brc != 0 or not out_bin.exists():
            tail = be.decode("utf-8", "replace").strip().splitlines()[-6:]
            _fail(f"{target}: BUILD FAILED (exit {brc})")
            for line in tail:
                _say(f"        {line}")
            failures.append(f"{target}: build")
            continue
        run_cmd = [
            cpython,
            str(safe_run),
            "--rss-mb",
            str(args.rss_mb),
            "--timeout",
            str(args.run_timeout),
            "--",
            str(out_bin),
            *prog_args,
        ]
        mrc, mout, _me = _difftest_capture(
            run_cmd, env=base_env, cwd=root, timeout=args.run_timeout + 30
        )
        out_match = _difftest_streams_match(cpy_out, mout)
        rc_match = mrc == cpy_rc
        if out_match and rc_match:
            _ok(f"{target}: PASS (exit={mrc}, stdout byte-identical to CPython)")
        else:
            _fail(
                f"{target}: MISMATCH molt(exit={mrc}) vs CPython(exit={cpy_rc}); "
                f"stdout_match={out_match} exit_match={rc_match}"
            )
            if not out_match:
                import difflib

                diff = difflib.unified_diff(
                    cpy_out.decode("utf-8", "replace").splitlines(),
                    mout.decode("utf-8", "replace").splitlines(),
                    fromfile="cpython",
                    tofile=f"molt:{target}",
                    lineterm="",
                )
                for line in list(diff)[:40]:
                    _say(f"        {line}")
            failures.append(f"{target}: output")

    if args.json:
        print(
            json.dumps(
                {
                    "program": str(program),
                    "root": str(root),
                    "targets": args.target,
                    "cpython_exit": cpy_rc,
                    "failures": failures,
                    "passed": not failures,
                }
            )
        )
    if failures:
        raise DriverError(f"difftest: {len(failures)} target(s) failed: {failures}")
    _ok(f"difftest PASS on all targets: {args.target}")
    return EXIT_OK


def cmd_cleanup(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    git = Git(repo)
    upstream_ref = f"{args.remote}/{args.branch}"
    ignore_globs = tuple(DEFAULT_IGNORE_GLOBS) + tuple(args.ignore or ())
    _step(f"cleanup worktree {repo} (upstream {upstream_ref})")
    # Refresh upstream so the unpushed check is honest.
    git.fetch(args.remote, args.branch)
    return _cleanup_worktree(
        git, repo, upstream_ref, ignore_globs, force_sha=args.force
    )


# --------------------------------------------------------------------------
# Argument parsing
# --------------------------------------------------------------------------


def _add_repo_arg(p: argparse.ArgumentParser) -> None:
    p.add_argument(
        "--repo",
        default=str(DEFAULT_REPO),
        help="repository / worktree to act on (default: this driver's repo)",
    )


def _add_remote_branch(p: argparse.ArgumentParser) -> None:
    p.add_argument(
        "--remote", default="origin", help="upstream remote (default origin)"
    )
    p.add_argument("--branch", default="main", help="upstream branch (default main)")


def build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(
        prog="molt_dev.py",
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = ap.add_subparsers(dest="command", required=True)

    # integrate
    p_int = sub.add_parser(
        "integrate",
        help="fetch->rebase->verify-commits->markers->gates->push->confirm->cleanup",
    )
    _add_repo_arg(p_int)
    _add_remote_branch(p_int)
    p_int.add_argument("--dry-run", action="store_true", help="plan without mutating")
    p_int.add_argument(
        "--no-gates", action="store_true", help="skip gate selection/execution"
    )
    p_int.add_argument(
        "--no-push",
        action="store_true",
        help="stop before pushing (run verify-push after pushing externally)",
    )
    p_int.add_argument(
        "--gates-config", help=f"path to {GATES_CONFIG_NAME} (default: tools/ in repo)"
    )
    p_int.add_argument(
        "--marker",
        action="append",
        help="content marker: 'exists:<path>' or 'contains:<path>::<needle>' "
        "(repeatable)",
    )
    p_int.add_argument(
        "--extra-gate",
        action="append",
        help="additional gate command to run (repeatable)",
    )
    p_int.add_argument(
        "--cleanup-worktree",
        action="store_true",
        help="after a confirmed push, remove this worktree (refuses if unsafe)",
    )
    p_int.add_argument(
        "--ignore",
        action="append",
        help="extra ignore glob for the dirty-tree/cleanup checks (repeatable)",
    )
    p_int.add_argument(
        "--session-id",
        default=os.environ.get("MOLT_SESSION_ID", "devtool"),
        help="MOLT_SESSION_ID to export for gates (default env or 'devtool')",
    )
    p_int.add_argument("--toolchain-binary", help="binary for gate-required freshness")
    p_int.add_argument(
        "--toolchain-marker", help="behavior marker for the freshness probe"
    )
    p_int.add_argument(
        "--toolchain-probe-arg",
        action="append",
        help="arg to the probe binary (repeatable)",
    )
    p_int.add_argument(
        "--rss-mb", type=int, default=2048, help="safe_run RSS cap (MiB)"
    )
    p_int.add_argument("--timeout", type=int, default=30, help="safe_run timeout (s)")
    p_int.set_defaults(func=cmd_integrate)

    # secure-wip
    p_wip = sub.add_parser(
        "secure-wip", help="commit ALL tracked modifications in one recovery commit"
    )
    _add_repo_arg(p_wip)
    p_wip.add_argument(
        "--dry-run", action="store_true", help="show plan, do not commit"
    )
    p_wip.add_argument(
        "-m", "--message", help="commit subject (WIP-RECOVERY prefix added)"
    )
    p_wip.add_argument(
        "--ignore", action="append", help="extra ignore glob (repeatable)"
    )
    p_wip.add_argument(
        "--include-untracked",
        action="store_true",
        help="also stage untracked files (default: tracked only)",
    )
    p_wip.set_defaults(func=cmd_secure_wip)

    # verify-push
    p_vp = sub.add_parser(
        "verify-push",
        help="confirm origin/<branch> contains a tip sha (fetch+ancestor)",
    )
    _add_repo_arg(p_vp)
    _add_remote_branch(p_vp)
    p_vp.add_argument("--tip", help="tip sha to confirm (default HEAD)")
    p_vp.add_argument("--json", action="store_true", help="emit JSON result to stdout")
    p_vp.set_defaults(func=cmd_verify_push)

    # verify-toolchain
    p_vt = sub.add_parser(
        "verify-toolchain", help="behavior-marker probe + binary-freshness report"
    )
    _add_repo_arg(p_vt)
    p_vt.add_argument("binary", help="path to the built binary to probe")
    p_vt.add_argument("--marker", help="behavior-marker substring to require in output")
    p_vt.add_argument(
        "--probe-arg", action="append", help="arg to pass to the binary (repeatable)"
    )
    p_vt.add_argument(
        "--require-fresh", action="store_true", help="exit non-zero if binary is stale"
    )
    p_vt.add_argument(
        "--reference",
        help="a prior binary to byte-compare against (detect a no-op rebuild)",
    )
    p_vt.add_argument(
        "--require-differs",
        action="store_true",
        help="exit non-zero if the binary is byte-identical to --reference",
    )
    p_vt.add_argument("--rss-mb", type=int, default=2048, help="safe_run RSS cap (MiB)")
    p_vt.add_argument("--timeout", type=int, default=15, help="safe_run timeout (s)")
    p_vt.add_argument("--json", action="store_true", help="emit JSON report to stdout")
    p_vt.set_defaults(func=cmd_verify_toolchain)

    # probe
    p_pr = sub.add_parser("probe", help="file size+mtime / pid liveness (python ops)")
    p_pr.add_argument("--file", action="append", help="file to stat (repeatable)")
    p_pr.add_argument(
        "--pid", action="append", type=int, help="pid to check (repeatable)"
    )
    p_pr.add_argument("--json", action="store_true", help="emit JSON to stdout")
    p_pr.set_defaults(func=cmd_probe)

    # python-oracle
    p_po = sub.add_parser(
        "python-oracle", help="resolve + PIN + verify a CPython version before use"
    )
    p_po.add_argument(
        "--python-version",
        required=True,
        help="required CPython 'MAJOR.MINOR' (e.g. 3.12)",
    )
    p_po.add_argument(
        "--no-uv",
        action="store_true",
        help="do not consult uv (PATH + sys.executable only)",
    )
    p_po.add_argument("--json", action="store_true", help="emit JSON to stdout")
    p_po.set_defaults(func=cmd_python_oracle)

    # detached-run / detached-verify (hazard 11)
    p_dr = sub.add_parser(
        "detached-run",
        help=(
            "spawn a command as a setsid daemon that survives harness "
            "detach (hazard 11); MUST be paired with detached-verify in a "
            "LATER tool call"
        ),
    )
    p_dr.add_argument("--name", required=True, help="daemon name (state-dir key)")
    p_dr.add_argument(
        "--state-dir",
        help=f"state root override (default: {DETACHED_STATE_ROOT}/<name>)",
    )
    p_dr.add_argument("--cwd", help="working directory for the command")
    p_dr.add_argument(
        "--env", action="append", metavar="K=V", help="env override (repeatable)"
    )
    p_dr.add_argument(
        "--replace",
        action="store_true",
        help="clear DEAD/finished prior state and respawn (never kills a live daemon)",
    )
    p_dr.add_argument(
        "--verify-min-age-hint",
        type=int,
        default=30,
        help="min-age-s echoed into the printed detached-verify instruction",
    )
    p_dr.add_argument("--json", action="store_true", help="emit JSON to stdout")
    p_dr.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        metavar="-- cmd ...",
        help="the command to daemonize (everything after --)",
    )
    p_dr.set_defaults(func=cmd_detached_run)

    p_dv = sub.add_parser(
        "detached-verify",
        help=(
            "prove a detached daemon outlived its spawning call: "
            "running / done(rc) / DIED-SILENT (hazard 11)"
        ),
    )
    p_dv.add_argument("--name", required=True, help="daemon name (state-dir key)")
    p_dv.add_argument(
        "--state-dir",
        help=f"state root override (default: {DETACHED_STATE_ROOT}/<name>)",
    )
    p_dv.add_argument(
        "--min-age-s",
        type=float,
        default=30.0,
        help="a RUNNING daemon younger than this is reported too-young (exit 1)",
    )
    p_dv.add_argument("--json", action="store_true", help="emit JSON to stdout")
    p_dv.set_defaults(func=cmd_detached_verify)

    # cleanup (standalone hazard-3 gate; integrate uses the shared logic)
    p_cl = sub.add_parser(
        "cleanup", help="remove a worktree, refusing on unpushed/dirty (hazard 3)"
    )
    _add_repo_arg(p_cl)
    _add_remote_branch(p_cl)
    p_cl.add_argument(
        "--ignore", action="append", help="extra ignore glob (repeatable)"
    )
    p_cl.add_argument(
        "--force",
        metavar="SHA",
        help="abandon despite unpushed/dirty; MUST name the HEAD sha being abandoned",
    )
    p_cl.set_defaults(func=cmd_cleanup)

    # difftest (hazard 12: split-root toolchain + differential vs pinned CPython)
    p_df = sub.add_parser(
        "difftest",
        help=(
            "build a program through a CONSISTENTLY-rooted toolchain "
            "(MOLT_PROJECT_ROOT+PYTHONPATH from one --root) and diff native/LLVM/"
            "WASM output vs pinned CPython (hazard 12)"
        ),
    )
    p_df.add_argument("program", help="Python program to build + differential-test")
    p_df.add_argument(
        "--root",
        default=str(DEFAULT_REPO),
        help="toolchain root: frontend AND runtime/backend build from here "
        "(sets MOLT_PROJECT_ROOT + PYTHONPATH). Default: this driver's repo.",
    )
    p_df.add_argument(
        "--target",
        action="append",
        choices=["native", "llvm", "wasm"],
        help="backend target (repeatable; default: native)",
    )
    p_df.add_argument(
        "--session",
        default=os.environ.get("MOLT_SESSION_ID", "difftest"),
        help="MOLT_SESSION_ID for build isolation (default env or 'difftest')",
    )
    p_df.add_argument(
        "--python-version",
        default="3.14",
        help="CPython oracle version, pinned+verified (default 3.14)",
    )
    p_df.add_argument(
        "--oracle-python",
        help="interpreter for build+oracle (default: the python running this "
        "driver, which carries the molt frontend deps); version-verified",
    )
    p_df.add_argument(
        "--out-dir",
        default=str(Path(tempfile.gettempdir()) / "molt_difftest"),
        help="where to write the built binaries",
    )
    p_df.add_argument(
        "--env",
        action="append",
        help="extra KEY=VALUE env passed to BOTH build and run (e.g. a trace "
        "flag); repeatable",
    )
    p_df.add_argument(
        "--rss-mb", type=int, default=1024, help="safe_run RSS cap MiB (default 1024)"
    )
    p_df.add_argument(
        "--run-timeout", type=int, default=15, help="safe_run wall-time s (default 15)"
    )
    p_df.add_argument(
        "--build-timeout",
        type=int,
        default=900,
        help="per-target build timeout s (default 900 — covers a runtime rebuild)",
    )
    p_df.add_argument(
        "--timeout", type=int, default=60, help="CPython oracle timeout s (default 60)"
    )
    p_df.add_argument("--json", action="store_true", help="emit JSON verdict to stdout")
    p_df.add_argument(
        "--arg",
        action="append",
        dest="prog_args",
        help="argument passed to BOTH the molt binary and CPython (repeatable). "
        "Explicit (not a trailing REMAINDER) so difftest's own options work in "
        "any position.",
    )
    p_df.set_defaults(func=cmd_difftest)

    return ap


def main(argv: list[str]) -> int:
    ap = build_parser()
    args = ap.parse_args(argv)
    try:
        return args.func(args)
    except DriverError as exc:
        _say("")
        _fail(str(exc))
        _say("")
        return exc.code


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
