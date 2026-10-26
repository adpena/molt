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
"did this file change" — is computed from
git plumbing (`rev-list`, `cat-file`, `patch-id`, `rev-parse`) captured as
bytes via subprocess, or from python file ops, NEVER from a human-readable
shell text tool whose output the proxy may rewrite. The plumbing commands emit
machine-stable IDs that no proxy reformats.

The hazard inventory -> countermeasure map (the spec this file implements)
-------------------------------------------------------------------------
  1. Rebase silently drops commits (patch-id dedup vs moved upstream)
       -> `integrate` computes the patch-id of every source commit BEFORE the
          rebase and verifies each one is present AFTER (either in the new
          range, or provably already-upstream by a patch-id match against the
          upstream tip range). A missing patch-id is a LOUD FAIL listing the
          dangling shas; integration never proceeds past a dropped commit.
          Without replay, exact captured HEAD identity and upstream ancestry
          prove preservation directly; no patch diffs or patch IDs are computed.
  2. Push exit codes lie (rtk/sandbox swallow; 144-detached pushes can SUCCEED)
       -> `verify-push` (and the push step of `integrate`) FETCHES the upstream
          and confirms origin/<branch> CONTAINS the pushed tip sha via
          `git merge-base --is-ancestor`. Cleanup is gated on THAT, never on
          the push command's exit code.
  3. Worktree cleanup loses work
       -> cleanup REFUSES when a commit of upstream..HEAD is not on upstream
          (molt_dev_change_ids.landing_representation): source merges require
          actual ancestry; ordinary commits also admit Change-Id or patch-id
          OR tracked staged/unstaged changes exist
          (excluding a configurable ignore set, default the legacy and keyed wasm sha256 files).
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
       -> Selected Cargo gates own their package/feature/profile test images
          against the captured source. Product builds admit compiler source,
          toolchain inputs and executable content through the existing compiler
          admission owner. Git dates and byte-identical output do not establish
          or refute that identity; there is no second post-gate binary check.
  7. .venv interpreter flips (3.12 <-> 3.14t across uv calls)
       -> `python-oracle` resolves the requested CPython version, PINS it, and
          verifies `sys.version` before returning the interpreter path;
          calibration/differential tooling routes through it.
  8. Content-marker verification
       -> declared `integrate --marker` checks (file-exists / file-contains)
          run post-rebase, pre-push, via python file ops.
  9. Liveness/recovery probes (agent transcripts, daemon pids)
       -> `probe` reports size+mtime (python `os.stat`) and pid liveness
          (`molt.process_model.pid_alive`; a zombie is not alive),
          replacing ad-hoc shell one-liners.
 10. Gate selection by change-class
       -> `integrate` reads tools/proof_plan.toml (touched-path glob ->
          required gate commands) and runs exactly the gates the change-class
          demands; --extra-gate selects another manifest-declared gate.
 11. Backgrounded long-runs die silently (observed 2026-06-06, twice in one
     session: the harness reaps run_in_background process groups at detach
     [exit 144], and sandboxed tool calls reap even setsid daemons at
     container teardown — both deaths lose block-buffered output, leaving an
     EMPTY log, NO exit status, and no way to distinguish "killed" from
     "still running" after the fact)
       -> `detached-run` starts a fresh setsid supervisor with unbuffered IO and writes
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
  probe             file size+mtime / pid liveness (python ops)
  python-oracle     resolve + PIN + verify a CPython version before use
  detached-run      spawn a setsid daemon that survives harness detach
                    (state dir: pid/sid/cmd.json/run.log/rc)
  detached-verify   prove the daemon outlived its spawning call; report
                    running / done(rc) / DIED-SILENT (the hazard-11 class)
  lane              start | retire | list lane/<slug> worktrees: create,
                    record and provision a lane; retire a landed or bundled
                    one completely (tools/molt_dev_lanes.py)
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path

# Bind both tools and molt before loading any repository module; an editable
# installation or an unrelated cwd must not select another checkout.
_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt import python_interpreter  # noqa: E402
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder  # noqa: E402

from tools.molt_dev_common import (  # noqa: E402
    _COMMANDS,
    DEFAULT_REPO,
    EXIT_FAIL,
    EXIT_OK,
    EXIT_USAGE,
    DriverError,
    PublicationRace,
    RebaseConflict,
    _fail,
    _run_fast_captured_bytes,
    _run_fast_captured_command,
    _ok,
    _run_captured_gate,
    _run_driver_command,
    _run_driver_command_bytes,
    _run_live_gate,
    _say,
    _step,
    _warn,
)
from tools import molt_dev_change_ids, molt_dev_controller, molt_dev_retire  # noqa: E402

from tools.git_identity import (  # noqa: E402
    clean_checkout_status_arguments,
    require_git_object_id,
)
from tools.git_identity import immutable_git_environment  # noqa: E402
from molt.file_locks import (  # noqa: E402
    _acquire_file_lock,
    _release_file_lock,
    _try_acquire_file_lock,
)
from molt.dx import configured_artifact_root, project_cargo_target_dir  # noqa: E402

from tools.molt_dev_detached import (  # noqa: E402
    cmd_detached_run,
    cmd_detached_verify,
)
from tools.molt_dev_probe import probe_path, probe_pid  # noqa: E402
from tools.molt_dev_lanes import add_lane_arguments  # noqa: E402
from tools.molt_dev_queue import add_queue_arguments  # noqa: E402
from tools import venv_exec  # noqa: E402
from tools.memory_guard_core.repro_context import argv_head  # noqa: E402
from tools.molt_dev_gates import (  # noqa: E402
    GateOutcome,
    GateRuntime,
    admit_selected_output_capacity,
    observe_gate_capacity,
    concurrent_gate_limit,
    display as _gate_display,
    gate_output_dir,
    gate_time_summary,
    retire_gate_logs,
    run_selected_gates,
)
from tools.molt_dev_policy import GateConfig, GateEnvironment, select_landing_gates  # noqa: E402
from tools.dirty_tree_policy import (  # noqa: E402
    DEFAULT_DIRTY_TREE_IGNORE_GLOBS as DEFAULT_IGNORE_GLOBS,
    is_ignored as _is_ignored,
)

# --------------------------------------------------------------------------
# Constants / locations
# --------------------------------------------------------------------------

# Recovery marker prefix for secure-wip commits, greppable so a salvaged WIP is
# never mistaken for a deliberate landing.
WIP_MARKER = "WIP-RECOVERY"


# --------------------------------------------------------------------------
# Output helpers live in molt_dev_common so extracted command-family modules
# share the same driver error, exit-code, and subprocess custody contract.
# --------------------------------------------------------------------------


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
        env = immutable_git_environment(os.environ)
        env["GIT_TERMINAL_PROMPT"] = "0"
        proc = _run_fast_captured_command(
            ["git", "-C", str(self.repo), *args],
            env=ProcessEnvironment(env),
            input_text=input_text,
        )
        if check and proc.returncode != 0:
            raise DriverError(
                f"git {' '.join(args)} failed (exit {proc.returncode}):\n"
                f"{proc.stdout.strip()}\n{proc.stderr.strip()}"
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


# --------------------------------------------------------------------------
# probe helpers live in molt_dev_probe and are re-exported here for tests/CLI.
# --------------------------------------------------------------------------

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


def _git_common_directory(repo: Path) -> Path | None:
    """Resolve the shared Git namespace once, including linked worktrees."""
    proc = _run_fast_captured_bytes(
        ["git", "rev-parse", "--git-common-dir"],
        cwd=repo,
        env=ProcessEnvironment(immutable_git_environment(os.environ)),
    )
    if proc.returncode != 0:
        return None
    if not proc.stdout.endswith(b"\n"):
        raise DriverError("Git common directory is not newline terminated")
    raw = proc.stdout[:-1]
    if os.name == "nt" and raw.endswith(b"\r"):
        raw = raw[:-1]
    common = Path(os.fsdecode(raw))
    return (common if common.is_absolute() else repo / common).resolve()


def _integration_lock_path(repo: Path) -> Path:
    common = _git_common_directory(repo)
    if common is None:
        raise DriverError("cannot resolve the Git common directory for integration")
    return common / "molt-integrate.lock"


def _resolve_repo_venv(repo: Path, env: Mapping[str, str]) -> Path | None:
    """Select one exact managed generation or an explicit external borrower."""
    if env.get("MOLT_VENV"):
        return venv_exec.resolve_venv(repo_root=repo, env=env)
    from molt import checkout_provisioning
    from molt.dx import DxConfigError, DxProject
    from molt.source_build_environment import load_development_environment
    from molt.source_build_environment_schema import SourceBuildEnvironmentError

    try:
        if env.get("UV_PROJECT_ENVIRONMENT", "").strip():
            selected = venv_exec.resolve_venv(repo_root=repo, env=env)
            return load_development_environment(
                selected, repo_root=repo, groups=DxProject(repo).dependency_groups()
            ).root
        spec = checkout_provisioning.checkout_input(
            "uv-environment", repo, env, guard_source_root=_COMMANDS.repo_root
        )
        if not checkout_provisioning.declares(spec):
            return None
        return checkout_provisioning.ensure_input(spec).environment_root
    except (
        checkout_provisioning.ProvisioningError,
        DxConfigError,
        SourceBuildEnvironmentError,
        venv_exec.VenvExecError,
        OSError,
        ValueError,
    ) as exc:
        raise DriverError(f"managed development image refused: {exc}") from exc


def prepare_gate_environment(
    repo: Path, env: ProcessEnvironment, *, requires_python: bool
) -> GateEnvironment:
    """Select once by declared demand; explicit selectors always validate."""
    from molt.dx import checkout_state_maintenance_environment

    selected = checkout_state_maintenance_environment(repo, env)
    if selected is not None:
        env = selected
    explicit = bool(
        env.get("MOLT_VENV") or env.get("UV_PROJECT_ENVIRONMENT", "").strip()
    )
    venv = _resolve_repo_venv(repo, env) if requires_python or explicit else None
    if venv is None:
        if requires_python:
            raise DriverError(
                "development image not selected; run "
                "`python -m molt.checkout_provisioning ensure uv-environment`"
            )
        return GateEnvironment(env, None, False)
    managed = not bool(env.get("MOLT_VENV"))
    try:
        venv_exec.venv_python(venv)
        activated = venv_exec.venv_env(venv=venv, env=env, managed=managed)
        if not managed:
            activated = activated.updated({"MOLT_VENV": str(venv)})
    except (venv_exec.VenvExecError, ValueError) as exc:
        raise DriverError(str(exc)) from exc
    return GateEnvironment(activated, venv, managed)


def _validate_gate_image(repo: Path, context: GateEnvironment) -> None:
    """Recheck the selected recipe and entrypoint before guarded admission.

    This cheap check is not a physical image attestation. The existing guarded
    command boundary still owns fresh admission for the actual launch.
    """
    if context.python_root is None:
        return
    if context.managed:
        from molt.dx import DxConfigError, DxProject
        from molt.source_build_environment import load_development_environment
        from molt.source_build_environment_schema import SourceBuildEnvironmentError

        try:
            load_development_environment(
                context.python_root,
                repo_root=repo,
                groups=DxProject(repo).dependency_groups(),
            )
        except (DxConfigError, SourceBuildEnvironmentError, OSError, ValueError) as exc:
            raise DriverError(f"managed development image refused: {exc}") from exc
    try:
        venv_exec.venv_python(context.python_root)
    except (venv_exec.VenvExecError, ValueError) as exc:
        raise DriverError(str(exc)) from exc


def run_gate(cmd: str | tuple[str, ...], repo: Path, context: GateEnvironment) -> int:
    """Run one shell gate or structured argument vector in the selected environment.

    The command is a shell string from the manifest (so it can use
    pipes/&&); generated pytest batches execute directly. Output streams
    live to the caller's stdout/stderr (no capture) so a long gate is observable.

    Activate the selected project environment once for the complete shell command,
    including compound commands and child tools. A generic non-Python gate can run
    without a project environment, but a bare Python gate cannot use host PATH as
    a substitute. Explicit invalid selections always fail before shell launch.
    """
    _say(f"    gate: {cmd if isinstance(cmd, str) else shlex.join(cmd)}")
    cmd, env = _gate_environment(cmd, repo, context)
    return _run_live_gate(cmd, repo=repo, env=env)


def run_captured_gate(
    cmd: str | tuple[str, ...],
    repo: Path,
    context: GateEnvironment,
    cancelled: Callable[[], bool],
    streams: tuple[Path, Path] | None = None,
) -> GateOutcome:
    """Run one landing gate in ``run_gate``'s environment, writing its streams
    to ``streams`` while it runs, under the executor's cancellation."""
    try:
        cmd, env = _gate_environment(cmd, repo, context)
    except BaseException as error:
        # The guard exclusively creates streams for admitted gates. A refusal
        # before that handoff still needs the empty evidence paths its event
        # publishes, without replacing its original admission failure.
        if streams is not None:
            for path in streams:
                try:
                    with path.open("x", encoding="utf-8"):
                        pass
                except OSError as output_error:
                    error.add_note(
                        f"cannot create refused gate stream {path}: {output_error}"
                    )
        raise
    return GateOutcome(
        *_run_captured_gate(
            cmd,
            repo=repo,
            env=env,
            cancelled=cancelled,
            streams=streams,
            # The guard repeats this label every keepalive; keep it short.
            label=f"gate {argv_head(cmd.split() if isinstance(cmd, str) else cmd)}",
        )
    )


def _gate_environment(
    cmd: str | tuple[str, ...], repo: Path, context: GateEnvironment
) -> tuple[str | tuple[str, ...], ProcessEnvironment]:
    _validate_gate_image(repo, context)
    env = context.environment
    shell_syntax = False
    if isinstance(cmd, str):
        try:
            if os.name == "nt":
                from molt.backend_daemon_custody import _split_windows_command

                words = list(_split_windows_command(cmd))
                shell_syntax = any(character in cmd for character in "&|<>^%!")
            else:
                lexer = shlex.shlex(cmd, posix=True, punctuation_chars=";&|<>()")
                lexer.whitespace_split = True
                words = list(lexer)
                shell_syntax = any(
                    word and set(word) <= set(";&|<>()") for word in words
                ) or any(character in cmd for character in "$`\n")
        except ValueError:
            # The shell owns syntax errors; do not replace their exit status.
            words = []
            shell_syntax = True
    else:
        words = list(cmd)
    head = words[0].lower() if words else ""
    from tools.proof_command_argv import is_python_command

    python_gate = is_python_command(head)
    shell_python = []
    if shell_syntax:
        # Inspect literal command heads without rewriting shell text. Dynamic
        # shell expansion remains the shell's responsibility; direct isolated
        # Python arms need structured argv so startup policy is explicit.
        starts = (
            index
            for index in range(len(words))
            if index == 0 or words[index - 1] in {";", "&&", "||", "|", "("}
        )
        for index in starts:
            if is_python_command(words[index]):
                end = next(
                    (
                        offset
                        for offset in range(index + 1, len(words))
                        if words[offset] in {";", "&&", "||", "|", ")"}
                    ),
                    len(words),
                )
                shell_python.append(words[index:end])
    venv = context.python_root
    if venv is None:
        if python_gate or shell_python:
            raise DriverError(
                "development image not selected; run "
                "`python -m molt.checkout_provisioning ensure uv-environment`"
            )
        return cmd, env
    managed = context.managed
    try:
        venv_exec.venv_python(venv)
        env = venv_exec.venv_env(venv=venv, env=env, managed=managed)
        if managed and shell_python:
            from molt.python_interpreter import (
                parse_python_invocation,
                python_environment_options,
            )
            from tools.proof_command_argv import (
                validate_python_environment_command,
            )

            if any(
                python_environment_options(parse_python_invocation(arm))[0]
                for arm in shell_python
            ):
                raise DriverError(
                    "isolated Python in a compound shell gate requires structured argv"
                )
            for arm in shell_python:
                validate_python_environment_command(
                    arm, environment_root=venv, environment=env
                )
        if python_gate:
            alias = Path(head).name
            if os.name == "nt" and not alias.endswith(".exe"):
                alias += ".exe"
            executable = venv_exec.venv_bin_dir(venv) / alias
            if not executable.is_file() or not os.access(executable, os.X_OK):
                raise venv_exec.VenvExecError(
                    f"virtualenv command {Path(head).name!r} not executable under "
                    f"{venv_exec.venv_bin_dir(venv)}; use the project Python alias"
                )
            if not (isinstance(cmd, str) and shell_syntax):
                cmd = tuple(
                    venv_exec.resolve_command(words, venv=venv, managed=managed)
                )
        # An external borrower keeps its explicit contract. Managed roots stay
        # selected as UV images rather than being relabeled as borrowed venvs.
        if not managed:
            env = env.updated({"MOLT_VENV": str(venv)})
    except (venv_exec.VenvExecError, ValueError) as exc:
        raise DriverError(str(exc)) from exc
    return cmd, env


# --------------------------------------------------------------------------
# Subcommand: integrate
# --------------------------------------------------------------------------


def _verify_no_dropped_commits(
    git: Git,
    source_shas: list[str],
    pre_patch_ids: dict[str, str | None],
    new_range: str,
    incoming_range: str,
) -> None:
    """Hazard 1: every source commit's patch-id must survive the rebase.

    For each pre-rebase source sha we computed its patch-id BEFORE the rebase.
    After the rebase, a source commit is accounted for iff its patch-id appears
    EITHER in the new branch range (it was replayed) OR already in the upstream
    (it landed upstream independently — the legitimate "moved upstream" dedup).
    Any source patch-id present in neither is a DROPPED commit -> LOUD FAIL.
    """
    new_pids = set(git.patch_ids_for_range(new_range))
    # The captured pre-rebase HEAD..upstream span contains precisely the
    # incoming commits that may independently carry a source patch. Deriving
    # it from the rebased range would compare upstream with itself and lose
    # legitimate deduplication evidence.
    upstream_pids = set(git.patch_ids_for_range(incoming_range))
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


def _assert_integration_source(git: Git, commit: str) -> None:
    """Use the proof executor's clean-checkout boundary, pinned to one commit.

    This is a boundary check, not a continuous filesystem monitor. Integration
    runs in an exclusive clean checkout; parallel work belongs in its own tree.
    """
    if git.head_sha() != commit:
        raise DriverError("integration source HEAD changed; refusing to publish")
    dirty = git._run(list(clean_checkout_status_arguments(null_terminated=True))).stdout
    if dirty:
        raise DriverError(
            "integration requires a clean selected source checkout; preserve WIP "
            "in its existing checkout and integrate from an exclusive clean checkout:\n"
            + dirty.replace("\0", "\n")
        )


def _gate_pycache_prefix(repo: Path, env: Mapping[str, str]) -> Path:
    """The DX bytecode prefix (`molt dx env`), refused inside the checkout.

    CPython validates each cached module by its source mtime and size. A
    clean exclusive checkout only changes between gate runs, through Git
    writes that set a new mtime.
    """
    from molt.dx import scratch_dir
    from molt.path_custody import host_path_is_within

    prefix = scratch_dir(repo, "pycache", env)
    if host_path_is_within(prefix, repo):
        raise DriverError(f"gate bytecode prefix is inside the checkout: {prefix}")
    return prefix


def _gate_runtime(
    git: Git,
    selected_commit: str,
    *,
    repo: Path,
    policy: str,
    config: GateConfig,
    environments: dict[tuple[str, ...], GateEnvironment],
    cargo_outputs: dict[tuple[str, ...], tuple[Path, ...]],
    capacity_requirements: dict[tuple[str, ...], int],
    progress: _IntegrationProgress,
    toolchains: dict[str, dict],
    receipt_kind: str = "gates",
) -> GateRuntime:
    def source_changes() -> tuple[str, ...]:
        status = git._run(list(clean_checkout_status_arguments(null_terminated=True)))
        return tuple(entry for entry in status.stdout.split("\0") if entry)

    # A commit's tree never changes; only HEAD needs observing again.
    tree = git.rev_parse(f"{selected_commit}^{{tree}}")

    def source_identity() -> dict[str, str]:
        if git.head_sha() != selected_commit:
            raise DriverError("integration source HEAD changed")
        return {"commit": selected_commit, "tree": tree}

    # Beside the integration lock, shared by every worktree, never inside one.
    receipt_path = (
        _integration_lock_path(repo).with_name("molt-integrate")
        / "receipts"
        / f"{selected_commit}.{receipt_kind}.json"
    )
    retire_gate_logs(receipt_path.parent, now=time.time())
    return GateRuntime(
        repo=repo,
        environments=environments,
        cargo_outputs=cargo_outputs,
        capacity_requirements=capacity_requirements,
        progress=progress,
        check_source=lambda: _assert_integration_source(git, selected_commit),
        source_changes=source_changes,
        source_identity=source_identity,
        policy_text=policy,
        executor_max_workers=config.executor_max_workers,
        resource_limits=config.resource_limits,
        toolchains=toolchains,
        receipt_path=receipt_path,
        output_dir=gate_output_dir(receipt_path),
        run_captured_gate=lambda *args: run_captured_gate(*args),
        observe_capacity=observe_gate_capacity,
        say=_say,
        ok=_ok,
    )


@dataclass
class _IntegrationProgress:
    """Advisory stdout events; absent terminal output never establishes success."""

    started: float = field(default_factory=time.monotonic)
    controller: str | None = None
    source_commit: str | None = None
    upstream_commit: str | None = None
    gate_total: int = 0
    # (amended, original) while resolved change citations are unpublished:
    # a failed landing restores the original, so its citations stay stable.
    unpublished_citation_amend: tuple[str, str] | None = None
    environments: dict[tuple[str, ...], GateEnvironment] = field(default_factory=dict)
    # Concurrent gates emit from their own threads; one line at a time.
    _lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    def emit(self, event: str, **details: object) -> None:
        with self._lock:
            self._emit(event, **details)

    def _emit(self, event: str, **details: object) -> None:
        now = time.monotonic()
        payload = {
            "schema": "molt.integration-progress.v1",
            "event": event,
            "advisory": True,
            "controller": self.controller,
            "source_commit": self.source_commit,
            "upstream_commit": self.upstream_commit,
            "gate_total": self.gate_total,
            "recorded_at": datetime.now(UTC).isoformat(),
            "monotonic_seconds": now,
            "elapsed_seconds": now - self.started,
            **details,
        }
        try:
            print(json.dumps(payload, ensure_ascii=True), flush=True)
        except OSError:
            # Diagnostics cannot replace a gate, custody or publication outcome.
            pass


def _restore_unpublished_citations(repo: Path, progress: _IntegrationProgress) -> None:
    """Return a failed landing's checkout to its unresolved change citations.

    The resolved form names commit ids that the next rebase changes; the
    `change:` form survives it. A landing that published keeps its amend.
    """
    if progress.unpublished_citation_amend is None:
        return
    amended, original = progress.unpublished_citation_amend
    git = Git(repo)
    if git.head_sha() != amended:
        _warn(
            f"left {git.short(git.head_sha())}; citations resolved in {git.short(amended)}"
        )
        return
    git._run(["reset", "--quiet", "--keep", original])
    _warn(f"landing failed: restored {git.short(original)} with its change citations")


def gate_environment(repo: Path, *, session_id: str) -> ProcessEnvironmentBuilder:
    """The one baseline environment every landing gate projects from."""
    from tools.proof_queue_pkg.command_identity import CANONICAL_EXECUTION_ENV

    env = immutable_git_environment(os.environ)
    from molt.dx import checkout_state_maintenance_environment

    selected = checkout_state_maintenance_environment(repo, env)
    if selected is not None:
        env.update(selected)
    # Gates are proof executions: bytecode a gate writes under src/ changes the
    # checkout's import topology while later gates read it (a fresh worktree's
    # first landing failed that way). The canonical environment forbids writes;
    # gates instead share the DX pycache prefix outside the checkout, so each
    # module compiles once per source version rather than once per gate.
    env.update(CANONICAL_EXECUTION_ENV)
    env.pop("PYTHONDONTWRITEBYTECODE", None)
    env["PYTHONPYCACHEPREFIX"] = str(_gate_pycache_prefix(repo, env))
    env["MOLT_SESSION_ID"] = session_id
    env["PYTHONPATH"] = os.pathsep.join((str(repo / "src"), str(repo)))
    # Cargo gates build into the target Molt's own builds of this checkout use.
    # Bare Cargo would compile cold into the worktree's ./target, which the
    # worktree cleanup then deletes, so every landing paid a full build.
    env.setdefault("CARGO_TARGET_DIR", str(project_cargo_target_dir(repo, env)))
    return env


def cmd_integrate(args: argparse.Namespace) -> int:
    repo = Path(args.repo).resolve()
    progress = _IntegrationProgress()
    returncode = None
    error: BaseException | None = None
    try:
        progress.controller = molt_dev_controller.integration_controller(args)
        if args.dry_run:
            returncode = _integrate_exclusive(args, progress=progress)
            return returncode
        lock_path = _integration_lock_path(repo)
        if args.wait_for_lock:
            # The landing queue's runner waits out another integrator's
            # landing; an interactive integrate still refuses at once.
            _say("    waiting for the repository integration lock")
            handle = _acquire_file_lock(
                lock_path, timeout_s=None, timeout_message="", poll_s=2.0
            )
        else:
            handle = _try_acquire_file_lock(lock_path)
        if handle is None:
            raise DriverError(
                "another integration owns this Git repository; no fetch or gates ran. "
                "Retry after it finishes; do not remove the integration lock file."
            )
        try:
            returncode = _integrate_exclusive(args, progress=progress)
            return returncode
        finally:
            _release_file_lock(handle)
    except BaseException as exc:
        error = exc
        _restore_unpublished_citations(repo, progress)
        raise
    finally:
        if error is not None:
            returncode = (
                error.code
                if isinstance(error, DriverError)
                else EXIT_USAGE
                if isinstance(error, ValueError)
                else 130
                if isinstance(error, KeyboardInterrupt)
                else None
            )
        progress.emit(
            "integration_finished",
            status=(
                "interrupted"
                if isinstance(error, KeyboardInterrupt)
                else "completed"
                if error is None and returncode == 0
                else "failed"
            ),
            returncode=returncode,
            dry_run=args.dry_run,
        )
        if not args.dry_run:
            seen: set[Path | None] = set()
            for context in progress.environments.values():
                root = configured_artifact_root(context.environment, relative_to=repo)
                if root not in seen:
                    seen.add(root)
                    _collect_driver_maintenance(repo, context.environment)


def _integrate_exclusive(
    args: argparse.Namespace, *, progress: _IntegrationProgress
) -> int:
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
    head_before = require_git_object_id(git.head_sha(), label="integration source")
    progress.source_commit = head_before
    _assert_integration_source(git, head_before)
    sym = git.symbolic_head()
    _say(f"    HEAD={git.short(head_before)} ({sym or 'detached'})")

    # --- step 1: fetch upstream --------------------------------------------
    _step("fetch upstream")
    if dry:
        _say(f"    would: git fetch {remote} {branch}")
    else:
        git.fetch(remote, branch)
    upstream_before = require_git_object_id(
        git.rev_parse(upstream_ref), label="integration upstream"
    )
    progress.upstream_commit = upstream_before
    if args.expected_upstream is not None:
        require_git_object_id(args.expected_upstream, label="expected upstream")
        if upstream_before != args.expected_upstream:
            raise PublicationRace(
                "expected upstream changed; re-check ownership before integrating"
            )
    _assert_integration_source(git, head_before)
    _ok(f"{upstream_ref} = {git.short(upstream_before)}")

    # The source commits we are integrating: everything on HEAD not yet on
    # upstream. Immutable commit identities preserve the entire source graph
    # when no rebase is needed; patch IDs are required only for actual replay.
    source_range = f"{upstream_before}..{head_before}"
    source_shas = git.commits_in_range(source_range)
    _assert_integration_source(git, head_before)
    if not source_shas:
        progress.emit("candidate_selected")
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

    # --- step 2: rebase onto upstream (only if behind) ----------------------
    _step(f"rebase onto {upstream_ref}")
    incoming_range = f"{head_before}..{upstream_before}"
    behind = git.commits_in_range(incoming_range)
    selected_commit = head_before
    if not behind:
        _ok("HEAD already contains upstream tip; no rebase needed")
    elif args.expected_upstream is not None:
        raise DriverError(
            "claim source does not contain expected upstream; re-check ownership without rebasing"
        )
    elif dry:
        _say(
            f"    would: git rebase {upstream_ref}  "
            f"(HEAD is behind by {len(behind)} commit(s))"
        )
        # No replay occurred, so there is no post-rebase survival claim.
    else:
        identity = git._run(["var", "GIT_COMMITTER_IDENT"], check=False)
        if identity.returncode:
            raise DriverError(
                "rebase requires a configured Git committer; executed=0. "
                "Set git config user.name and user.email (or GIT_COMMITTER_NAME/EMAIL).\n"
                + identity.stderr.strip()
            )
        pre_patch_ids = {sha: git.patch_id(sha) for sha in source_shas}
        _assert_integration_source(git, head_before)
        proc = git._run(["rebase", upstream_before], check=False)
        if proc.returncode != 0:
            # Abort a half-applied rebase so the tree is never left mid-conflict.
            git._run(["rebase", "--abort"], check=False)
            raise RebaseConflict(
                f"rebase onto {upstream_ref} failed (conflicts?). Aborted the "
                "rebase to restore the pre-rebase HEAD. Resolve manually, then "
                f"re-run integrate.\n{proc.stdout.strip()}\n{proc.stderr.strip()}"
            )
        selected_commit = require_git_object_id(
            git.head_sha(), label="selected integration commit"
        )
        progress.source_commit = selected_commit
        new_range = f"{upstream_before}..{selected_commit}"
        _ok(f"rebased; HEAD {git.short(head_before)} -> {git.short(selected_commit)}")
        _assert_integration_source(git, selected_commit)
        _step("verify no dropped commits (patch-id survival)")
        _verify_no_dropped_commits(
            git, source_shas, pre_patch_ids, new_range, incoming_range
        )

    _assert_integration_source(git, selected_commit)

    # --- step 3: verify source preservation (hazard 1) ----------------------
    if not behind:
        _step("verify unchanged source commit and upstream ancestry")
        if not git.is_ancestor(upstream_before, selected_commit):
            raise DriverError(
                "captured upstream is not an ancestor of the unchanged source"
            )
        _ok("captured HEAD unchanged; all source commits preserved by exact identity")
    elif dry:
        _warn(
            "dry-run cannot verify post-rebase patch-ids (no rebase performed); "
            "the live run verifies them"
        )

    # --- step 3b: stable change ids -----------------------------------------
    cited_commit = selected_commit
    selected_commit = molt_dev_change_ids.resolve_change_references(
        git,
        repo,
        upstream_before,
        selected_commit,
        dry=dry,
        assert_source=_assert_integration_source,
    )
    if selected_commit != cited_commit:
        progress.unpublished_citation_amend = (selected_commit, cited_commit)
    progress.source_commit = selected_commit

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
    policy, config, touched, gate_cmds, matched_rules = select_landing_gates(
        git,
        repo,
        selected_commit,
        upstream_before,
        extra=list(args.extra_gate or []),
    )
    progress.source_commit = selected_commit
    progress.gate_total = len(gate_cmds)
    progress.emit("candidate_selected")
    _step(
        f"gates ({len(gate_cmds)} selected for {len(touched)} touched path(s); "
        f"matched rules: {', '.join(r.name for r in matched_rules) or 'none'})"
    )
    env = gate_environment(repo, session_id=args.session_id)
    progress.environments = {(): GateEnvironment(ProcessEnvironment(env), None, False)}
    cargo_outputs: dict[tuple[str, ...], tuple[Path, ...]] = {}
    capacity_requirements: dict[tuple[str, ...], int] = {}
    if not dry:
        _assert_integration_source(git, selected_commit)
        prepared = config.prepare(
            gate_cmds,
            baseline=env,
            prepare_python=lambda baseline, needed: prepare_gate_environment(
                repo, baseline, requires_python=needed
            ),
        )
        toolchains = prepared.fingerprints
        environments = prepared.environments
        progress.environments = environments
        try:
            cargo_outputs = config.cargo_outputs(environments)
            capacity_requirements = admit_selected_output_capacity(
                cargo_outputs, environments
            )
        except (ValueError, OSError, KeyError) as exc:
            raise DriverError(
                f"selected output capacity failed; executed=0: {exc}"
            ) from exc
        _assert_integration_source(git, selected_commit)
    gate_seconds: list[tuple[float, str]] = []
    if dry:
        for gate in gate_cmds:
            _assert_integration_source(git, selected_commit)
            _say(f"    would-run gate: {_gate_display(gate.command)}")
    else:
        run_selected_gates(
            gate_cmds,
            _gate_runtime(
                git,
                selected_commit,
                repo=repo,
                policy=policy,
                config=config,
                environments=environments,
                cargo_outputs=cargo_outputs,
                capacity_requirements=capacity_requirements,
                progress=progress,
                toolchains=toolchains,
            ),
            gate_seconds,
            host_limit=lambda: concurrent_gate_limit(),
        )
    if gate_seconds:
        _say(gate_time_summary(gate_seconds))

    # --- step 6: push (only after every gate is green) ----------------------
    _step(f"push to {upstream_ref}")
    _assert_integration_source(git, selected_commit)
    head_to_push = selected_commit
    if dry:
        _say(
            f"    would: git push {remote} {head_to_push}:{branch}  (tip {git.short(head_to_push)})"
        )
        _say("    would: confirm by fetch + ancestor check (hazard 2)")
        _say("    would: cleanup gate (hazard 3) after confirmation")
        _ok(
            f"dry-run complete: hazard checks passed; {len(gate_cmds)} gate(s) "
            "listed, none run (--no-push runs them); no mutation performed"
        )
        return EXIT_OK
    if args.no_push:
        _ok(
            f"push SKIPPED (--no-push); tip {git.short(head_to_push)} ready. "
            "Run `molt_dev.py verify-push` after pushing externally."
        )
        return EXIT_OK
    push = git._run(
        ["push", remote, f"{head_to_push}:refs/heads/{branch}"], check=False
    )
    if push.returncode != 0:
        # Do NOT trust the exit code as final — but a non-zero exit with no
        # landed tip is a real failure; the confirm step below is authoritative.
        _warn(
            f"git push exited {push.returncode}; NOT trusting it (hazard 2). "
            "Confirming by fetch+ancestor instead.\n"
            f"{push.stdout.strip()}\n{push.stderr.strip()}"
        )

    # --- step 7: confirm the push by FETCH + ANCESTOR (hazard 2) ------------
    _step("confirm push landed (fetch + ancestor, not exit code)")
    git.fetch(remote, branch)
    upstream_after = git.rev_parse(upstream_ref)
    landed = git.is_ancestor(head_to_push, upstream_after)
    if not landed:
        error = PublicationRace if upstream_after != upstream_before else DriverError
        raise error(
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
    progress.unpublished_citation_amend = None

    # --- step 8: cleanup gate (hazard 3) ------------------------------------
    if args.cleanup_worktree:
        _step("cleanup gate (refuses on unpushed/dirty)")
        rc = molt_dev_retire.cleanup_worktree(
            git, repo, upstream_ref, ignore_globs, force_sha=None
        )
        if rc != 0:
            return rc
    else:
        _ok("cleanup not requested (--cleanup-worktree to remove this worktree)")

    _ok("integrate complete: pushed + confirmed.")
    return EXIT_OK


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
                    f"    pid {r['pid']}: alive={'unknown' if r['alive'] is None else r['alive']}"
                    + (f" ({r['detail']})" if r["detail"] else "")
                )
            elif r.get("exists"):
                _say(
                    f"    {r['path']}: size={r['size']} mtime={r['mtime_iso']} "
                    f"age={r['age_s']}s"
                )
            else:
                _say(f"    {r['path']}: MISSING")
    # Exit non-zero if any probed file is missing or any pid is dead/unknown, so the
    # subcommand is usable as a CI/health assertion.
    bad = any(
        ("pid" in r and r["alive"] is not True) or ("exists" in r and not r["exists"])
        for r in results
    )
    return EXIT_FAIL if bad else EXIT_OK


# --------------------------------------------------------------------------
# Subcommand: python-oracle (hazard 7)
# --------------------------------------------------------------------------


def cmd_python_oracle(args: argparse.Namespace) -> int:
    try:
        target = python_interpreter.parse_target_python_version(args.python_version)
        from molt.python_environment_identity import PythonIdentityVerifier

        with PythonIdentityVerifier() as verifier:
            resolved = python_interpreter.resolve_target_python(
                target,
                verifier=verifier,
                cwd=DEFAULT_REPO,
                prefer_uv=not args.no_uv,
            )
    except (ValueError, python_interpreter.PythonInterpreterError) as exc:
        raise DriverError(str(exc), code=EXIT_USAGE) from exc
    exe = resolved.executable
    if args.json:
        print(
            json.dumps(
                {
                    "python": exe,
                    "command": list(resolved.command),
                    "version": target.short,
                    "full": resolved.version,
                }
            )
        )
    else:
        print(exe)
    _ok(f"pinned CPython {target.short}: {exe} ({resolved.version})")
    return EXIT_OK


# --------------------------------------------------------------------------
# detached-run / detached-verify live in molt_dev_detached.
# --------------------------------------------------------------------------

# --------------------------------------------------------------------------
# Worktree cleanup (hazard 3) — used by integrate and as no standalone (the
# refusal logic is shared so a force-abandon is always auditable)
# --------------------------------------------------------------------------


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
) -> ProcessEnvironment:
    """Build the env that roots frontend AND runtime/backend at the SAME tree.

    This is the hazard-12 fix in one place (and the unit-testable core): set
    MOLT_PROJECT_ROOT (runtime/backend build root + fingerprint tree) and
    prepend <root>/src to PYTHONPATH (frontend import root) so they can never
    diverge. ``extra_env`` items are ``KEY=VALUE`` passthroughs applied to both
    build and run (e.g. a trace flag).
    """
    env = ProcessEnvironmentBuilder(os.environ)
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
    from molt.dx import checkout_state_maintenance_environment

    selected = checkout_state_maintenance_environment(root, env)
    return ProcessEnvironment(env) if selected is None else selected


def _difftest_capture(
    cmd: list[str], *, env: ProcessEnvironment, cwd: Path, timeout: int
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
    cpython_command = python_interpreter.explicit_python_command(
        args.oracle_python or sys.executable
    )
    try:
        target_python = python_interpreter.parse_target_python_version(
            args.python_version
        )
        from molt.python_environment_identity import PythonIdentityVerifier

        with PythonIdentityVerifier() as verifier:
            cpython_identity = python_interpreter.probe_python_command(
                cpython_command,
                verifier=verifier,
                cwd=root,
                timeout=30.0,
            )
    except (ValueError, python_interpreter.PythonInterpreterError) as exc:
        raise DriverError(f"difftest: {exc}", code=EXIT_USAGE) from exc
    if cpython_identity.major_minor != target_python.short:
        raise DriverError(
            f"difftest: interpreter {python_interpreter.format_python_command(cpython_command)} "
            f"reports {cpython_identity.version!r}, not "
            f"{args.python_version}. Run molt_dev.py under python "
            f"{args.python_version} (it doubles as the byte-exact CPython "
            f"oracle), or pass --oracle-python.",
            code=EXIT_USAGE,
        )
    cpython = cpython_identity.executable
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
    _ok(f"interpreter {cpython} ({cpython_identity.version}) imports molt from {root}")

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
    out_dir = (
        Path(tempfile.gettempdir()) / "molt_difftest"
        if args.out_dir is None
        else Path(args.out_dir)
    )
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
            ),
            flush=True,
        )
    if failures:
        raise DriverError(
            f"difftest: {len(failures)} target(s) failed: {failures}",
            maintenance_selection=(root, base_env),
        )
    _ok(f"difftest PASS on all targets: {args.target}")
    _collect_driver_maintenance(root, base_env)
    return EXIT_OK


# --------------------------------------------------------------------------
# Subcommand: fix
# --------------------------------------------------------------------------


def cmd_fix(args: argparse.Namespace) -> int:
    """Apply every mechanical repair the read-only gates ask for, in one pass.

    Pre-commit hooks stay read-only (they must never rewrite another agent's
    unstaged work); this is the one command their failures point to.
    """
    repo = Path(args.repo).resolve()
    context = prepare_gate_environment(
        repo, ProcessEnvironment(os.environ), requires_python=True
    )
    python = "python"
    steps = (
        ("pin text encodings", f"{python} tools/encoding_gate.py --fix"),
        ("format Python", f"{python} -m ruff format ."),
        # One interpreter renders every CI-checkable manifest generator in
        # upstream order (tools/generators.py), then proves them current.
        ("regenerate stale generated outputs", f"{python} tools/generators.py write"),
        ("verify generated outputs", f"{python} tools/generators.py check"),
    )
    for name, command in steps:
        _step(name)
        if run_gate(command, repo, context) != 0:
            _fail(f"fix: {name} failed")
            _collect_driver_maintenance(repo, context.environment)
            return 1
    _say("fix: encodings pinned, formatted, every generated output current")
    _collect_driver_maintenance(repo, context.environment)
    return 0


def _collect_driver_maintenance(repo: Path, env: ProcessEnvironment) -> None:
    if not env.get("MOLT_PROOF_QUEUE"):
        from molt.dx import collect_checkout_state_after_result

        collect_checkout_state_after_result(env, cwd=repo)


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
        "--no-push",
        action="store_true",
        help="stop before pushing (run verify-push after pushing externally)",
    )
    p_int.add_argument(
        "--expected-upstream",
        help="require this full upstream commit; refuse rebase for ownership claims",
    )
    p_int.add_argument(
        "--candidate-controller",
        action="store_true",
        help="run this checkout's own driver instead of main's pinned controller; "
        "only for a reviewed controller repair or input-contract adoption "
        "(all gates remain mandatory; the progress stream records it)",
    )
    # Set only by the pinned-controller handoff; names the controller this
    # process must be (molt_dev_controller.verify_pinned_controller).
    molt_dev_controller.add_pinned_controller_arguments(p_int)
    # Set only by the landing queue's runner (tools/molt_dev_queue.py).
    p_int.add_argument("--wait-for-lock", action="store_true", help=argparse.SUPPRESS)
    p_int.add_argument(
        "--marker",
        action="append",
        help="content marker: 'exists:<path>' or 'contains:<path>::<needle>' "
        "(repeatable)",
    )
    p_int.add_argument(
        "--extra-gate",
        action="append",
        help="additional manifest-declared gate command to run (repeatable)",
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
    p_int.set_defaults(func=cmd_integrate)

    add_queue_arguments(
        sub, add_repo_arg=_add_repo_arg, add_remote_branch=_add_remote_branch
    )

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
        help="state root override (default: system temp/molt_dev_detached/<name>)",
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
    p_dr.add_argument(
        "--json",
        action="store_true",
        help="emit launch state and verify_argv for the required later call",
    )
    p_dr.add_argument(
        "argv",
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
        help="state root override (default: system temp/molt_dev_detached/<name>)",
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
    p_cl.set_defaults(func=molt_dev_retire.cmd_cleanup)

    # lane: orchestrator-owned lane worktrees (tools/molt_dev_lanes.py)
    p_ln = sub.add_parser("lane", help="start, retire or list lane/<slug> worktrees")
    add_lane_arguments(
        p_ln, add_repo_arg=_add_repo_arg, add_remote_branch=_add_remote_branch
    )

    # fix: the one command read-only gate failures point to
    p_fx = sub.add_parser(
        "fix",
        help="pin encodings, format, and regenerate stale generated outputs",
    )
    _add_repo_arg(p_fx)
    p_fx.set_defaults(func=cmd_fix)

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
        default=None,
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
        molt_dev_controller.resume_controller_cwd(args, driver_file=__file__)
        if molt_dev_controller.runs_from_pinned_controller(args):
            return molt_dev_controller.run_from_pinned_controller(args, argv)
        return args.func(args)
    except (DriverError, ValueError) as exc:
        _say("")
        _fail(str(exc))
        _say("")
        if isinstance(exc, DriverError) and exc.maintenance_selection is not None:
            repo, environment = exc.maintenance_selection
            _collect_driver_maintenance(repo, environment)
        return exc.code if isinstance(exc, DriverError) else EXIT_USAGE


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
