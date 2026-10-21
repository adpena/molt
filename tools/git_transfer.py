"""Bound a Git network transfer by its progress, never by wall clock (HF-414).

The one authority for the Git transfers (``fetch``, ``push``, ``pull``,
``clone``, ``ls-remote``, ``remote update``, ``submodule update``) that pass
the memory guard: ``guarded_completed_process`` applies it, so every tool
that runs Git through ``CommandExecutor``, the harness or
``HarnessExecutionContext`` gets it. A raw subprocess bypasses it, as it
bypasses the guard. A deadline on such a command measures the network and
the host; a transfer that moves nothing for ``TRANSFER_STALL_SECONDS`` has a
dead or wedged peer, or waits for a person.

- HTTP aborts below 1 byte/s over the stall bound (curl's low-speed limit,
  as Cargo's ``http.low-speed-limit``). A tighter value the user set stays:
  ``GIT_HTTP_LOW_SPEED_*`` is tightened in place, since Git reads it before
  any configuration; otherwise the bound joins the configuration after the
  user's own files, so the last plain ``http.lowSpeed*`` (tightened here)
  and any URL-scoped ``http.<url>.lowSpeed*`` (Git prefers the more specific
  key) keep their effect. Settings are read as Git reads them: the last
  value of each key, with the transfer's own ``-C`` and ``-c`` options.
- ssh runs through ``git_ssh_bound.sh``, which adds ConnectTimeout (the
  connect and the banner exchange) and server-alive probes (the session)
  only where the connection's own setting, as ``ssh -G`` reports it, is
  unset or looser. The user's ssh command is kept as written, in Git's
  precedence: ``GIT_SSH_COMMAND``, ``core.sshCommand``, then ``GIT_SSH``.
- An unattended transfer (no terminal can reach a person) never prompts:
  askpass programs are blanked, Git's terminal prompt and Git Credential
  Manager's interaction are off, credential helpers are asked not to
  interact, ssh runs in BatchMode, and the command ends once its output
  stops for the stall bound (CircleCI's ``no_output_timeout``), counting only
  time the guard watched, so a host freeze is not silence. Fetch, push,
  pull and clone report progress, so a moving transfer keeps its output
  moving; that output bound is what ends a credential helper that waits.

Prior art: Go's ``cmd/go`` runs Git with ``GIT_TERMINAL_PROMPT=0`` and
``GCM_INTERACTIVE=never``, and with BatchMode unless the user set an ssh
command; Cargo bounds its HTTP transfers by a low-speed limit.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import os
from pathlib import Path
import shlex

TRANSFER_STALL_SECONDS = 120

_TRANSFER_SUBCOMMANDS = frozenset({"fetch", "push", "pull", "clone", "ls-remote"})
_TRANSFER_ACTIONS = frozenset({("remote", "update"), ("submodule", "update")})
# These report progress with --progress; their output keeps moving while
# their transfer moves.
_PROGRESS_SUBCOMMANDS = frozenset({"fetch", "push", "pull", "clone"})
# Git global options that take the next argument as their value.
_VALUED_GLOBAL_OPTIONS = frozenset(
    {"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env"}
)
_SSH_PROGRAMS = frozenset({"ssh", "ssh.exe"})
_SETTINGS_PATTERN = r"^(core\.sshcommand|http\.lowspeedlimit|http\.lowspeedtime)$"


@dataclass(frozen=True)
class GitTransfer:
    """A Git command that talks to a remote, and where its subcommand sits."""

    subcommand: str
    index: int
    repository: str | None  # The last ``-C`` directory, if any.


@dataclass(frozen=True)
class TransportSettings:
    """The user's own transport choices, read from Git configuration."""

    ssh_command: str | None = None
    low_speed_limit: int | None = None
    low_speed_time: int | None = None


@dataclass(frozen=True)
class BoundedTransfer:
    argv: list[str]
    env: dict[str, str]
    no_output_timeout: float | None


def git_transfer(command: Sequence[str]) -> GitTransfer | None:
    """The transfer ``command`` runs, or None when it is not a Git transfer."""
    if not command:
        return None
    executable = Path(command[0]).name.lower()
    if executable.removesuffix(".exe") != "git":
        return None
    index = 1
    repository: str | None = None
    while index < len(command):
        option = command[index]
        if option in _VALUED_GLOBAL_OPTIONS:
            if option == "-C" and index + 1 < len(command):
                repository = command[index + 1]
            index += 2
        elif option.startswith("-"):
            index += 1
        else:
            break
    if index >= len(command):
        return None
    subcommand = command[index]
    if subcommand in _TRANSFER_SUBCOMMANDS or (
        tuple(command[index : index + 2]) in _TRANSFER_ACTIONS
    ):
        return GitTransfer(subcommand, index, repository)
    return None


def prompt_reaches_a_person() -> bool:
    """Whether a prompt can reach a person: ssh and Git prompt on the terminal.

    A detached landing runner, and an agent's shell, have no controlling
    terminal; a developer's interactive command has one.
    """
    if os.name == "nt":
        import sys

        return bool(sys.stdin is not None and sys.stdin.isatty())
    try:
        descriptor = os.open("/dev/tty", os.O_RDWR | os.O_NOCTTY)
    except OSError:
        return False
    os.close(descriptor)
    return True


def settings_query(command: Sequence[str], transfer: GitTransfer) -> list[str]:
    """The read-only Git command that reports the user's transport settings.

    It carries the transfer's own global options (``-C``, ``-c``,
    ``--git-dir``), so it reads the configuration the transfer would.
    """
    return [*command[: transfer.index], "config", "--get-regexp", _SETTINGS_PATTERN]


def parse_settings(output: str) -> TransportSettings:
    """The value Git uses for each key: the last one read (system, global,
    repository, worktree, then the command line)."""
    values: dict[str, str] = {}
    for line in output.splitlines():
        key, _, value = line.partition(" ")
        values[key.lower()] = value
    return TransportSettings(
        ssh_command=values.get("core.sshcommand") or None,
        low_speed_limit=_whole_number(values.get("http.lowspeedlimit")),
        low_speed_time=_whole_number(values.get("http.lowspeedtime")),
    )


def _whole_number(raw: str | None) -> int | None:
    try:
        return None if raw is None else int(raw.strip())
    except ValueError:
        return None


def bound_transfer(
    command: Sequence[str],
    transfer: GitTransfer,
    env: Mapping[str, str],
    *,
    settings: TransportSettings,
    attended: bool,
    ssh_wrapper: Path,
    watch_output: bool = True,
) -> BoundedTransfer:
    """``command`` and its environment, bounded by progress.

    ``watch_output`` says the guard captures the command's output, which the
    unattended output bound reads.
    """
    bounded = dict(env)
    _bound_http(bounded, settings)
    _bound_ssh(bounded, settings, attended=attended, ssh_wrapper=ssh_wrapper)
    argv = list(command)
    no_output_timeout: float | None = None
    if not attended:
        bounded["GIT_TERMINAL_PROMPT"] = "0"
        # An empty GIT_ASKPASS also stops Git from reading core.askPass and
        # SSH_ASKPASS; ssh reads its own.
        bounded["GIT_ASKPASS"] = ""
        bounded["SSH_ASKPASS"] = ""
        bounded["SSH_ASKPASS_REQUIRE"] = "never"
        bounded["GCM_INTERACTIVE"] = "never"
        _add_config(bounded, "credential.interactive", "false")
    if not attended and watch_output:
        if transfer.subcommand in _PROGRESS_SUBCOMMANDS:
            argv.insert(transfer.index + 1, "--progress")
        if transfer.subcommand in _PROGRESS_SUBCOMMANDS | {"ls-remote"}:
            no_output_timeout = float(TRANSFER_STALL_SECONDS)
    return BoundedTransfer(argv, bounded, no_output_timeout)


def _bound_http(env: dict[str, str], settings: TransportSettings) -> None:
    limit = _whole_number(env.get("GIT_HTTP_LOW_SPEED_LIMIT"))
    seconds = _whole_number(env.get("GIT_HTTP_LOW_SPEED_TIME"))
    if limit is not None or seconds is not None:
        # Git's http.c reads these before any configuration: tighten them.
        env["GIT_HTTP_LOW_SPEED_LIMIT"], env["GIT_HTTP_LOW_SPEED_TIME"] = (
            _tighter_low_speed(limit, seconds)
        )
        return
    # Otherwise the bound joins the configuration, after the user's files. A
    # URL-scoped http.<url>.lowSpeed* still wins for its URL, since Git's
    # urlmatch prefers the more specific key; a plain one is tightened here.
    tightest = _tighter_low_speed(settings.low_speed_limit, settings.low_speed_time)
    _add_config(env, "http.lowSpeedLimit", tightest[0])
    _add_config(env, "http.lowSpeedTime", tightest[1])


def _tighter_low_speed(limit: int | None, seconds: int | None) -> tuple[str, str]:
    # curl aborts below ``limit`` bytes/s sustained for ``seconds``: a higher
    # limit or a shorter time is tighter; zero turns curl's bound off.
    return (
        str(max(limit or 0, 1)),
        str(
            min(seconds, TRANSFER_STALL_SECONDS)
            if seconds is not None and seconds > 0
            else TRANSFER_STALL_SECONDS
        ),
    )


def _bound_ssh(
    env: dict[str, str],
    settings: TransportSettings,
    *,
    attended: bool,
    ssh_wrapper: Path,
) -> None:
    # Git's precedence (connect.c): GIT_SSH_COMMAND, core.sshCommand, GIT_SSH.
    command = env.get("GIT_SSH_COMMAND") or settings.ssh_command
    if command:
        if not _runs_ssh(command):
            return  # A wrapper or another client: left as configured.
        user_command = command  # Shell text, kept as written.
    else:
        program = env.get("GIT_SSH") or "ssh"
        if Path(program).name.lower() not in _SSH_PROGRAMS:
            return  # plink and others take other options.
        user_command = shlex.quote(program)
    prefix = shlex.join(
        [
            "sh",
            str(ssh_wrapper),
            str(TRANSFER_STALL_SECONDS),
            "0" if attended else "1",
        ]
    )
    env["GIT_SSH_COMMAND"] = f"{prefix} {user_command}"
    # Git's variant probe would run the wrapper with -G; it is OpenSSH.
    _add_config(env, "ssh.variant", "ssh")


def _runs_ssh(command: str) -> bool:
    try:
        words = shlex.split(command)
    except ValueError:
        return False
    return bool(words) and Path(words[0]).name.lower() in _SSH_PROGRAMS


def _add_config(env: dict[str, str], key: str, value: str) -> None:
    """Add one ``git -c`` setting through Git's environment protocol."""
    count = _whole_number(env.get("GIT_CONFIG_COUNT")) or 0
    env[f"GIT_CONFIG_KEY_{count}"] = key
    env[f"GIT_CONFIG_VALUE_{count}"] = value
    env["GIT_CONFIG_COUNT"] = str(count + 1)


SSH_WRAPPER = Path(__file__).with_name("git_ssh_bound.sh")


def bound_for_guard(
    command: Sequence[str],
    *,
    cwd: str | Path | None,
    env: Mapping[str, str] | None,
    capture_output: bool,
) -> BoundedTransfer | None:
    """The guard boundary's bound for ``command``, or None when it is no transfer.

    Every guarded command passes through ``guarded_completed_process``, so
    every Git transfer a tool starts through ``CommandExecutor``, the harness
    or ``HarnessExecutionContext`` gets this one bound. The output bound needs
    captured output; a command that inherits its streams keeps its transport
    bounds and prompts nothing when unattended, but nothing watches its output.
    """
    transfer = git_transfer(command)
    if transfer is None:
        return None
    source = os.environ if env is None else env
    bounded = bound_transfer(
        command,
        transfer,
        source,
        settings=read_settings(command, transfer, cwd=cwd, env=source),
        attended=prompt_reaches_a_person(),
        ssh_wrapper=SSH_WRAPPER,
        watch_output=capture_output,
    )
    return bounded


def read_settings(
    command: Sequence[str],
    transfer: GitTransfer,
    *,
    cwd: str | Path | None,
    env: Mapping[str, str],
) -> TransportSettings:
    """Read the user's transport settings with a bounded metadata probe."""
    from molt.process_environment import ProcessEnvironment
    from molt.process_guard import run_completed_command

    result = run_completed_command(
        settings_query(command, transfer),
        cwd=cwd,
        env=ProcessEnvironment(env),
        capture_output=True,
        text=True,
        encoding="utf-8",
        memory_guard_prefix=None,
    )
    return parse_settings(result.stdout or "")
