#!/usr/bin/env python3
"""Distributed cross-platform test runner for molt.

Reads a host inventory (default: tools/cross_hosts.toml), cross-compiles a
small smoke corpus locally for each remote target, ships the binaries +
expected-output manifest to the host, executes them remotely, and
aggregates pass/fail/skip results into a single matrix.

Design:

* Compilation always happens on this build host. ``molt build --target
  <triple>`` is invoked once per (smoke-case, target) pair. The cross
  matrix is enumerated from the inventory's distinct ``target`` values,
  so two hosts on the same target share a build cache.
* Execution always happens on the remote (or in a docker container). The
  runner copies the produced binary + a JSON manifest of expected stdout,
  then invokes the binary remotely under the host's preferred shell.
* The runner is pure stdlib (Python >= 3.12). It shells out
  to ssh / scp / rsync / docker — the only requirement on the dev box is
  that those tools are on PATH.
* Each host run owns its staging allocations from preparation through exit,
  including failed setup and cancellation. SSH ``remote_dir`` is a staging
  parent: only a fresh nonce child with its matching owner marker is removed.
  Docker smoke containers use nonce/label custody: prove stopped
  (including PID zero), remove the exact container, then release its local bind
  source. Provider failures retain unproven resources and report cleanup errors.
  ``--keep-remote`` retains SSH children / Docker stage files for debugging;
  containers always retire. Local rename copies never modify build outputs.
  Hard owner termination or an unreachable provider can leave resources for
  explicit recovery; this is not a crash-recovery supervisor. This does not
  create, remove or unmount VM-wide filesystem shares.

Smoke corpus:

    smoke_arith       integer + float arithmetic, format roundtrip
    smoke_listcomp    list comprehension + filter + sum
    smoke_dict        dict literal, sorted keys, mutation
    smoke_class       __init__ / methods / attribute access
    smoke_exception   try/except/else/finally + raise
    smoke_typing      type hints + isinstance + tuple/list typing
    smoke_dataclass   @dataclass repr/eq round-trip
    smoke_t_string    PEP 701 f-string nested expressions and format spec

Each case is a Python source string with deterministic stdout. The
expected output is captured by running CPython locally before the cross
phase; the runner compares the remote stdout byte-for-byte.

Verification (loopback):

    cat > /tmp/cross_hosts_self.toml <<'EOF'
    [[host]]
    name = "self"
    hostname = "localhost"
    user = "$USER"
    target = "aarch64-apple-darwin"
    remote_dir = "/tmp/molt_test_self"
    EOF

    python3 tools/cross_run.py --inventory /tmp/cross_hosts_self.toml --smoke

Once you fill in your real LAN hosts in tools/cross_hosts.toml:

    python3 tools/cross_run.py --smoke
    python3 tools/cross_run.py --smoke --report  # also writes
                                                 # reports/cross_platform_matrix.<ts>.md
    python3 tools/cross_run.py --hosts raspi,intel-mac --smoke

Full compliance corpus is opt-in (slow):

    python3 tools/cross_run.py --full
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import re
import secrets
import datetime as _dt
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from abc import ABC, abstractmethod
from contextlib import contextmanager
from pathlib import Path
from collections.abc import Callable, Iterator
from typing import Literal

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

# Repo root: tools/cross_run.py -> tools -> repo
REPO = Path(__file__).resolve().parents[1]
DEFAULT_INVENTORY = REPO / "tools" / "cross_hosts.toml"
DEFAULT_REPORT_DIR = REPO / "reports"
TOOLS_ROOT = REPO / "tools"


from tools import harness_memory_guard  # noqa: E402
from molt.dx import ARTIFACT_ROOT_ENV, artifact_root  # noqa: E402
from molt.temporary_artifacts import OwnedTemporaryDirectory  # noqa: E402

VALID_SHELLS = ("bash", "sh", "powershell")
VALID_TRANSPORTS = ("ssh", "docker")


def _guarded_run(
    cmd: list[str],
    *,
    capture_output: bool = True,
    text: bool = True,
    timeout: float | None = None,
    env: dict[str, str] | None = None,
    cwd: str | Path | None = None,
    stdout_capture_path: Path | None = None,
    stderr_capture_path: Path | None = None,
    capture_tail_bytes: int | None = None,
    cancellation_requested: Callable[[], bool] | None = None,
) -> subprocess.CompletedProcess[str]:
    proc = harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_CROSS",
        cwd=cwd,
        env=env,
        capture_output=capture_output,
        text=text,
        timeout=timeout,
        **(
            {"cancellation_requested": cancellation_requested}
            if cancellation_requested is not None
            else {}
        ),
        **(
            {
                "stdout_capture_path": stdout_capture_path,
                "stderr_capture_path": stderr_capture_path,
                "capture_tail_bytes": capture_tail_bytes,
            }
            if stdout_capture_path is not None
            else {}
        ),
    )
    if (
        timeout is not None
        and proc.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
    ):
        raise subprocess.TimeoutExpired(
            cmd=cmd,
            timeout=timeout,
            output=proc.stdout,
            stderr=proc.stderr,
        )
    return proc


# ---------------------------------------------------------------------------
# Smoke corpus
# ---------------------------------------------------------------------------


@dataclasses.dataclass(frozen=True)
class Case:
    name: str
    source: str


SMOKE_CASES: tuple[Case, ...] = (
    Case(
        "smoke_arith",
        """\
a = 7
b = 5
print(a + b)
print(a * b)
print(a - b)
print(a // b, a % b)
print(round(3.14159, 2))
print(2 ** 10)
""",
    ),
    Case(
        "smoke_listcomp",
        """\
squares = [x * x for x in range(10)]
evens = [x for x in squares if x % 2 == 0]
print(squares)
print(evens)
print(sum(evens))
""",
    ),
    Case(
        "smoke_dict",
        """\
d = {"alpha": 1, "beta": 2, "gamma": 3}
d["delta"] = 4
for key in sorted(d):
    print(key, d[key])
print(len(d))
""",
    ),
    Case(
        "smoke_class",
        """\
class Counter:
    def __init__(self, start: int = 0) -> None:
        self.value = start
    def bump(self, by: int = 1) -> int:
        self.value += by
        return self.value
c = Counter(10)
print(c.bump())
print(c.bump(5))
print(c.value)
""",
    ),
    Case(
        "smoke_exception",
        """\
def divide(a: int, b: int) -> int:
    try:
        return a // b
    except ZeroDivisionError:
        return -1
    finally:
        print("checked", a, b)
print(divide(10, 2))
print(divide(7, 0))
try:
    raise ValueError("boom")
except ValueError as exc:
    print("caught", exc)
""",
    ),
    Case(
        "smoke_typing",
        """\
def total(items: list[int]) -> int:
    return sum(items)
xs: list[int] = [1, 2, 3, 4]
print(total(xs))
print(isinstance(xs, list))
print(isinstance(xs[0], int))
pair: tuple[int, str] = (1, "a")
print(pair[0], pair[1])
""",
    ),
    Case(
        "smoke_dataclass",
        """\
from dataclasses import dataclass

@dataclass
class Point:
    x: int
    y: int

p = Point(3, 4)
q = Point(3, 4)
print(p)
print(p == q)
print(p.x + p.y)
""",
    ),
    Case(
        "smoke_t_string",
        """\
items = ["alpha", "beta", "gamma"]
name = "world"
print(f"hello {name}")
print(f"first is {items[0]}")
print(f"{'hello':>10}")
print(f"{3.14159:.2f}")
print(f"len={len(items)} sum={sum(len(s) for s in items)}")
""",
    ),
)


# ---------------------------------------------------------------------------
# Inventory model
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class Host:
    name: str
    target: str
    transport: str
    # ssh
    hostname: str | None = None
    user: str | None = None
    ssh_key: str | None = None
    ssh_port: int = 22
    shell: str = "bash"
    # docker
    container: str | None = None
    # both
    remote_dir: str = "/tmp/molt_test"

    @property
    def is_windows(self) -> bool:
        return self.shell == "powershell" or "windows" in self.target


def _expand(maybe_path: str | None) -> str | None:
    if maybe_path is None:
        return None
    return os.path.expanduser(os.path.expandvars(maybe_path))


def parse_inventory(path: Path) -> list[Host]:
    if not path.exists():
        raise FileNotFoundError(f"inventory not found: {path}")
    raw = tomllib.loads(path.read_text(encoding="utf-8"))
    hosts_raw = raw.get("host", [])
    if not isinstance(hosts_raw, list) or not hosts_raw:
        raise ValueError(
            f"inventory {path} has no [[host]] entries; cannot run cross matrix"
        )
    seen: set[str] = set()
    hosts: list[Host] = []
    for idx, entry in enumerate(hosts_raw):
        if not isinstance(entry, dict):
            raise ValueError(f"[[host]] #{idx} is not a table")
        name = entry.get("name")
        if not name or not isinstance(name, str):
            raise ValueError(f"[[host]] #{idx} missing 'name'")
        if name in seen:
            raise ValueError(f"duplicate host name {name!r}")
        seen.add(name)
        target = entry.get("target")
        if not target or not isinstance(target, str):
            raise ValueError(f"host {name!r} missing 'target'")
        transport = entry.get("transport", "ssh")
        if transport not in VALID_TRANSPORTS:
            raise ValueError(
                f"host {name!r} transport {transport!r} not in {VALID_TRANSPORTS}"
            )
        shell = entry.get("shell", "bash")
        if shell not in VALID_SHELLS:
            raise ValueError(f"host {name!r} shell {shell!r} not in {VALID_SHELLS}")
        remote_dir = entry.get("remote_dir") or (
            "/work" if transport == "docker" else "/tmp/molt_test"
        )
        host = Host(
            name=name,
            target=target,
            transport=transport,
            hostname=entry.get("hostname"),
            user=entry.get("user"),
            ssh_key=_expand(entry.get("ssh_key")),
            ssh_port=int(entry.get("ssh_port", 22)),
            shell=shell,
            container=entry.get("container"),
            remote_dir=remote_dir,
        )
        if transport == "ssh":
            if not host.hostname or not host.user:
                raise ValueError(
                    f"host {name!r}: ssh transport requires 'hostname' and 'user'"
                )
        elif transport == "docker":
            if not host.container:
                raise ValueError(
                    f"host {name!r}: docker transport requires 'container'"
                )
        hosts.append(host)
    return hosts


# ---------------------------------------------------------------------------
# Local build
# ---------------------------------------------------------------------------


def _python_for_build() -> str:
    """Locate the python interpreter that hosts the molt CLI."""
    venv = REPO / ".venv" / "bin" / "python3"
    if venv.exists():
        return str(venv)
    return sys.executable


def _host_target_triple() -> str:
    """Best-effort rust target triple for the build host."""
    proc = _guarded_run(["rustc", "-vV"], capture_output=True, text=True, timeout=15)
    if proc.returncode == 0:
        for line in proc.stdout.splitlines():
            if line.startswith("host:"):
                return line.split(":", 1)[1].strip()
    # Fall back to a platform-derived guess. Used only when rustc is not
    # on PATH, which is itself a build error we'd surface later anyway.
    machine = os.uname().machine if hasattr(os, "uname") else ""
    if sys.platform == "darwin":
        return ("aarch64" if machine == "arm64" else "x86_64") + "-apple-darwin"
    if sys.platform.startswith("linux"):
        arch = "aarch64" if machine in ("aarch64", "arm64") else "x86_64"
        return f"{arch}-unknown-linux-gnu"
    if sys.platform == "win32":
        return "x86_64-pc-windows-msvc"
    return ""


_HOST_TRIPLE: str | None = None


def _local_compile(
    case: Case, target: str, work: Path, *, verbose: bool
) -> tuple[Path | None, str]:
    """Cross-compile a smoke case for the given target. Returns (binary_path, log)."""
    global _HOST_TRIPLE
    if _HOST_TRIPLE is None:
        _HOST_TRIPLE = _host_target_triple()
    case_dir = work / target / case.name
    case_dir.mkdir(parents=True, exist_ok=True)
    src_path = case_dir / f"{case.name}.py"
    src_path.write_text(case.source, encoding="utf-8")
    out_dir = case_dir / "out"
    out_dir.mkdir(exist_ok=True)
    # Compliance harness convention: molt build with --out-dir produces
    # <out>/<stem>_molt as the native binary. For non-native targets the
    # extension may differ; we glob for any executable matching the stem.
    cmd: list[str] = [
        _python_for_build(),
        "-m",
        "molt.cli",
        "build",
        str(src_path),
        "--out-dir",
        str(out_dir),
        "--release",
    ]
    if target != _HOST_TRIPLE:
        cmd.extend(["--target", target])
    env = os.environ.copy()
    env.setdefault("PYTHONPATH", str(REPO / "src"))
    env[ARTIFACT_ROOT_ENV] = str(artifact_root(REPO, env))
    started = time.monotonic()
    proc = _guarded_run(
        cmd,
        capture_output=True,
        text=True,
        env=env,
        cwd=str(REPO),
        timeout=600,
    )
    elapsed = time.monotonic() - started
    log = (
        f"$ {' '.join(shlex.quote(c) for c in cmd)}\n"
        f"# exit={proc.returncode} elapsed={elapsed:.1f}s\n"
        f"--- stdout (tail) ---\n{proc.stdout[-1500:]}\n"
        f"--- stderr (tail) ---\n{proc.stderr[-1500:]}"
    )
    if proc.returncode != 0:
        return None, log
    # Locate produced binary. Try _molt suffix first (native compliance shape),
    # then fall back to any executable matching the stem.
    candidates = sorted(out_dir.glob(f"{case.name}_molt*"))
    if not candidates:
        candidates = sorted(out_dir.glob(f"{case.name}*"))
    candidates = [
        p for p in candidates if p.is_file() and p.suffix not in (".o", ".obj")
    ]
    if not candidates:
        return None, log + f"\n--- error ---\nno binary produced under {out_dir}"
    return candidates[0], log


def _local_expected(case: Case) -> str:
    """Run the case under host CPython to capture the reference stdout."""
    proc = _guarded_run(
        [sys.executable, "-c", case.source],
        capture_output=True,
        text=True,
        timeout=15,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"CPython failed to run smoke case {case.name!r}: {proc.stderr[:300]}"
        )
    return proc.stdout


# ---------------------------------------------------------------------------
# Remote transport
# ---------------------------------------------------------------------------


class Transport(ABC):
    """Abstract remote transport — ssh or docker run."""

    def __init__(self, host: Host) -> None:
        self.host = host
        self._nonce = secrets.token_hex(16)
        sep = "\\" if host.is_windows else "/"
        self.execution_dir = (
            host.remote_dir.rstrip("/\\") + sep + "molt-cross-" + self._nonce
            if host.transport == "ssh"
            else host.remote_dir
        )

    def remote_path(self, *parts: str) -> str:
        sep = "\\" if self.host.is_windows else "/"
        return self.execution_dir.rstrip("/\\") + sep + sep.join(parts)

    # Common helpers ------------------------------------------------------

    def name(self) -> str:
        return self.host.name

    # API -----------------------------------------------------------------

    @abstractmethod
    def prepare(self) -> None:  # pragma: no cover - subclass
        pass

    @abstractmethod
    def push(self, local_files: list[Path]) -> None:  # pragma: no cover
        pass

    @abstractmethod
    def run(self, remote_argv: list[str], *, timeout: int) -> tuple[int, str, str]:
        pass

    @abstractmethod
    def cleanup(self) -> None:  # pragma: no cover
        pass


class SSHTransport(Transport):
    """Run commands and copy files via ssh + scp."""

    def _ssh_base(self) -> list[str]:
        cmd = [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ConnectTimeout=15",
            "-p",
            str(self.host.ssh_port),
        ]
        if self.host.ssh_key:
            cmd.extend(["-i", self.host.ssh_key])
        cmd.append(f"{self.host.user}@{self.host.hostname}")
        return cmd

    def _scp_base(self) -> list[str]:
        cmd = [
            "scp",
            "-q",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ConnectTimeout=15",
            "-P",
            str(self.host.ssh_port),
        ]
        if self.host.ssh_key:
            cmd.extend(["-i", self.host.ssh_key])
        return cmd

    def _remote_shell_invoke(self, command: str) -> list[str]:
        """Wrap ``command`` for the remote shell. ``command`` is already a
        single shell-encoded string in the remote's syntax."""
        if self.host.shell == "powershell":
            # On Windows OpenSSH, the default shell may be cmd.exe. Force
            # powershell so multi-line scripts work uniformly.
            return self._ssh_base() + [
                "powershell",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                command,
            ]
        return self._ssh_base() + [command]

    def prepare(self) -> None:
        marker = self.remote_path(".molt-owner")
        if self.host.shell == "powershell":
            parent, directory, marker = (
                "'" + value.replace("'", "''") + "'"
                for value in (self.host.remote_dir, self.execution_dir, marker)
            )
            mkdir = (
                "$ErrorActionPreference = 'Stop'; "
                f"New-Item -ItemType Directory -Path {parent} -Force | Out-Null; "
                f"New-Item -ItemType Directory -Path {directory} | Out-Null; "
                f"try {{ [IO.File]::WriteAllText({marker}, '{self._nonce}') }} "
                "catch { $prepareError = $_; "
                f"try {{ Remove-Item -Recurse -Force -LiteralPath {directory} }} "
                'catch { throw "prepare: $prepareError; cleanup: $_" }; '
                "throw $prepareError }"
            )
        else:
            directory = shlex.quote(self.execution_dir)
            marker = shlex.quote(marker)
            # Exclusive mkdir establishes the owner before writing its marker.
            # A marker-write failure retires only that just-created allocation.
            mkdir = (
                f"umask 077; mkdir -p {shlex.quote(self.host.remote_dir)} && "
                f"mkdir {directory} && {{ "
                f"printf %s {self._nonce} > {marker} || {{ "
                f"rm -f {marker}; rmdir {directory}; exit 1; }}; }}"
            )
        proc = _guarded_run(
            self._remote_shell_invoke(mkdir),
            capture_output=True,
            text=True,
            timeout=60,
        )
        if proc.returncode != 0:
            raise RuntimeError(
                f"ssh prepare failed for {self.host.name}: "
                f"exit={proc.returncode} stderr={proc.stderr[:300]}"
            )

    def push(self, local_files: list[Path]) -> None:
        if not local_files:
            return
        # scp each file individually so a single bad path doesn't sink the
        # whole batch; this keeps error reporting per-file.
        for src in local_files:
            dest = f"{self.host.user}@{self.host.hostname}:{self.execution_dir}/"
            cmd = self._scp_base() + [str(src), dest]
            proc = _guarded_run(cmd, capture_output=True, text=True, timeout=180)
            if proc.returncode != 0:
                raise RuntimeError(
                    f"scp {src.name} -> {self.host.name} failed: "
                    f"exit={proc.returncode} stderr={proc.stderr[:300]}"
                )

    def run(self, remote_argv: list[str], *, timeout: int) -> tuple[int, str, str]:
        if self.host.shell == "powershell":
            # Quote arguments using PowerShell single-quote rules.
            ps_args = []
            for a in remote_argv:
                ps_args.append("'" + a.replace("'", "''") + "'")
            command = "& " + " ".join(ps_args)
            full = self._remote_shell_invoke(command)
        else:
            command = " ".join(shlex.quote(a) for a in remote_argv)
            full = self._remote_shell_invoke(command)
        proc = _guarded_run(full, capture_output=True, text=True, timeout=timeout)
        return proc.returncode, proc.stdout, proc.stderr

    def cleanup(self) -> None:
        marker = self.remote_path(".molt-owner")
        if self.host.shell == "powershell":
            directory, marker = (
                "'" + value.replace("'", "''") + "'"
                for value in (self.execution_dir, marker)
            )
            rm = (
                "$ErrorActionPreference = 'Stop'; "
                f"if (Test-Path -LiteralPath {directory}) {{ "
                f"if (((Get-Item -Force -LiteralPath {directory}).Attributes "
                "-band [IO.FileAttributes]::ReparsePoint) -or "
                f"((Get-Item -Force -LiteralPath {marker}).Attributes "
                "-band [IO.FileAttributes]::ReparsePoint)) "
                "{ throw 'SSH staging indirection differs; retained' }; "
                f"if ([IO.File]::ReadAllText({marker}) -cne '{self._nonce}') "
                "{ throw 'SSH staging ownership differs; retained' }; "
                f"Remove-Item -Recurse -Force -LiteralPath {directory} }}"
            )
        else:
            directory = shlex.quote(self.execution_dir)
            marker = shlex.quote(marker)
            rm = (
                f"if [ -e {directory} ] || [ -L {directory} ]; then "
                f"[ ! -L {directory} ] && [ ! -L {marker} ] && "
                f'[ "$(cat {marker})" = {self._nonce} ] || '
                "{ echo 'SSH staging ownership differs; retained' >&2; exit 1; }; "
                f"rm -rf {directory}; fi"
            )
        proc = _guarded_run(
            self._remote_shell_invoke(rm),
            capture_output=True,
            text=True,
            timeout=60,
        )
        if proc.returncode != 0:
            raise RuntimeError(
                f"ssh cleanup failed for {self.host.name} at {self.execution_dir}: "
                f"exit={proc.returncode} stderr={proc.stderr[:300]}"
            )


def _file_sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class DockerTransport(Transport):
    """Spin up a container per ``run`` invocation, mounting a stage dir."""

    def __init__(self, host: Host) -> None:
        super().__init__(host)
        self._stage: Path | None = None
        self._container_pending = False

    def prepare(self) -> None:
        # Verify docker is available locally.
        if shutil.which("docker") is None:
            raise RuntimeError("docker not found on PATH for docker transport")
        # A mutable tag or a missing image is not execution authority. Provision
        # the exact digest out of band; cross execution never pulls implicitly.
        if not self.host.container or not re.fullmatch(
            r"(?:[^\s@]+@)?sha256:[0-9a-f]{64}", self.host.container
        ):
            raise ValueError("docker transport requires an immutable image digest")
        proc = _guarded_run(
            ["docker", "image", "inspect", self.host.container],
            timeout=30,
        )
        if proc.returncode != 0:
            raise RuntimeError(
                f"pinned Docker image is absent: {self.host.container}; provision explicitly; no automatic pull"
            )
        # Create a stage directory we will mount as remote_dir.
        self._stage = Path(tempfile.mkdtemp(prefix="molt_cross_docker_"))

    def push(self, local_files: list[Path]) -> None:
        assert self._stage is not None
        for src in local_files:
            shutil.copy2(src, self._stage / src.name)
            os.chmod(self._stage / src.name, 0o755)

    def _docker_command(
        self, args: list[str], *, timeout: int
    ) -> subprocess.CompletedProcess[str]:
        proc = _guarded_run(["docker", *args], timeout=timeout)
        if proc.returncode != 0:
            raise RuntimeError(
                f"Docker {args[0]} failed: exit={proc.returncode} "
                f"stderr={proc.stderr[:300]}"
            )
        return proc

    @staticmethod
    def _inspect(execute: Callable, kind: str, identifier: str) -> dict:
        value = json.loads(execute([kind, "inspect", identifier], timeout=30).stdout)
        if (
            not isinstance(value, list)
            or len(value) != 1
            or not isinstance(value[0], dict)
        ):
            raise ValueError("Docker inspect did not return exactly one object")
        return value[0]

    @staticmethod
    def _require_container_owner(
        value: dict, identifier: str, name: str, nonce: str
    ) -> None:
        if (
            value.get("Id") != identifier
            or value.get("Name") != "/" + name
            or value.get("Config", {}).get("Labels", {}).get("org.molt.release-custody")
            != nonce
        ):
            raise ValueError("Docker cleanup lost exact container custody; retained")

    @contextmanager
    def _owned_container(
        self, execute: Callable, create_args: list[str], *, nonce: str
    ) -> Iterator[dict]:
        """One container lifetime for ordinary runs and sealed replay.

        The daemon outlives its client. Register custody before create, recover
        an unacknowledged create only by our name AND nonce label, and never
        release a bind source until exact-container closure has been proved.
        """
        name = "molt-run-" + nonce
        label = "org.molt.release-custody"
        identifier = None
        snapshots: dict = {}
        if self._container_pending:
            raise RuntimeError("Earlier Docker container retirement unproved; retained")
        self._container_pending = True
        primary: BaseException | None = None
        try:
            created = execute(
                [
                    "container",
                    "create",
                    "--pull=never",
                    "--name",
                    name,
                    "--label",
                    label + "=" + nonce,
                    *create_args,
                ],
                timeout=30,
            )
            identifier = created.stdout.strip()
            if not re.fullmatch(r"[0-9a-f]{64}", identifier):
                identifier = None
                raise ValueError("Docker create returned no exact container identity")
            before = self._inspect(execute, "container", identifier)
            self._require_container_owner(before, identifier, name, nonce)
            snapshots["before"] = before
            yield snapshots
        except BaseException as exc:
            primary = exc
            raise
        finally:
            try:
                if identifier is None:
                    # The client may have timed out after daemon-side creation.
                    found = execute(
                        [
                            "container",
                            "ls",
                            "--all",
                            "--quiet",
                            "--no-trunc",
                            "--filter",
                            "name=" + name,
                            "--filter",
                            "label=" + label + "=" + nonce,
                        ],
                        timeout=30,
                    ).stdout.split()
                    if len(found) > 1 or any(
                        not re.fullmatch(r"[0-9a-f]{64}", item) for item in found
                    ):
                        raise ValueError(
                            "Docker create recovery has ambiguous custody; retained"
                        )
                    if not found:
                        # No terminal acknowledgement exists. A daemon request
                        # can still finish after this empty lookup; absence now
                        # grants no authority to delete a future bind source.
                        raise RuntimeError(
                            f"Docker creation outcome unknown for {name}: "
                            "create unacknowledged and no exact ID observed; retained"
                        )
                    identifier = found[0]
                if identifier is not None:
                    after = self._inspect(execute, "container", identifier)
                    self._require_container_owner(after, identifier, name, nonce)
                    if after.get("State", {}).get("Running") is True:
                        execute(["container", "kill", identifier], timeout=30)
                        after = self._inspect(execute, "container", identifier)
                        self._require_container_owner(after, identifier, name, nonce)
                    if (
                        after.get("State", {}).get("Running") is not False
                        or after.get("State", {}).get("Pid") != 0
                    ):
                        raise ValueError("Docker container did not close; retained")
                    snapshots["after"] = after
                    execute(["container", "rm", identifier], timeout=30)
                self._container_pending = False
            except BaseException as cleanup:
                if primary is not None:
                    raise BaseExceptionGroup(
                        "Docker execution and cleanup failed", [primary, cleanup]
                    ) from None
                raise

    def run(self, remote_argv: list[str], *, timeout: int) -> tuple[int, str, str]:
        assert self._stage is not None and self.host.container is not None
        create_args = [
            "-v",
            f"{self._stage}:{self.execution_dir}",
            "-w",
            self.execution_dir,
            self.host.container,
            *remote_argv,
        ]
        with self._owned_container(
            self._docker_command, create_args, nonce=secrets.token_hex(16)
        ) as snapshots:
            proc = _guarded_run(
                ["docker", "container", "start", "--attach", snapshots["before"]["Id"]],
                capture_output=True,
                text=True,
                timeout=timeout,
            )
        return proc.returncode, proc.stdout, proc.stderr

    def cleanup(self) -> None:
        if self._container_pending:
            raise RuntimeError(
                "Docker container retirement unproved; bind source retained"
            )
        if self._stage is not None:
            shutil.rmtree(self._stage)
            self._stage = None


# Sealed release execution uses the existing system manager, not a container daemon.
# This is development proof infrastructure; no part is installed in guest products.
SEALED_SYSTEMD_SCHEMA = "molt.sealed-systemd.v1"
_SEALED_ENV = ("PATH=/absent", "HOME=/absent", "LANG=C", "LC_ALL=C")


@dataclasses.dataclass(frozen=True)
class SealedExecutionProjection:
    """One closed filesystem/resource policy, shared by launch and admission.

    Replay retains its original fixed limits. Compilation receives explicit
    limits and a disposable noexec workspace; imported images remain read-only.
    A recorded projection is evidence to compare with the caller's policy, never
    permission to increase that policy's budgets.
    """

    mode: Literal["replay", "compilation"]
    memory_max_bytes: int
    workspace_max_bytes: int
    runtime_max_seconds: int

    def __post_init__(self) -> None:
        if type(self.mode) is not str or self.mode not in {"replay", "compilation"}:
            raise ValueError("sealed execution mode is invalid")
        for name in ("memory_max_bytes", "workspace_max_bytes", "runtime_max_seconds"):
            value = getattr(self, name)
            limit = (
                (2**63 - 1) // 1000000 if name == "runtime_max_seconds" else 2**63 - 1
            )
            if type(value) is not int or not 0 < value <= limit:
                raise ValueError(
                    f"sealed execution {name} must be a bounded positive integer"
                )
        if self.mode == "replay" and (
            self.memory_max_bytes,
            self.workspace_max_bytes,
            self.runtime_max_seconds,
        ) != (1073741824, 67108864, 60):
            raise ValueError("sealed replay execution must retain the fixed policy")

    @property
    def stdout_max_bytes(self) -> int:
        """Replay text stays small; compile exports use the declared workspace.

        ZIP32 is the existing release archive authority. This bound is only a
        transport limit; it cannot widen the imported projection or guest memory.
        """
        return (
            16 * 1024 * 1024
            if self.mode == "replay"
            else min(self.workspace_max_bytes, 2 * 1024 * 1024 * 1024 - 1)
        )

    @classmethod
    def compilation(
        cls,
        *,
        memory_max_bytes: int,
        workspace_max_bytes: int,
        runtime_max_seconds: int,
    ) -> SealedExecutionProjection:
        return cls(
            "compilation", memory_max_bytes, workspace_max_bytes, runtime_max_seconds
        )

    def to_dict(self) -> dict[str, str | int]:
        return {
            "schema": "molt.sealed-execution.v1",
            "mode": self.mode,
            "memory_max_bytes": self.memory_max_bytes,
            "workspace_max_bytes": self.workspace_max_bytes,
            "runtime_max_seconds": self.runtime_max_seconds,
        }

    @classmethod
    def from_dict(cls, value: object) -> SealedExecutionProjection:
        if (
            not isinstance(value, dict)
            or set(value)
            != {
                "schema",
                "mode",
                "memory_max_bytes",
                "workspace_max_bytes",
                "runtime_max_seconds",
            }
            or value["schema"] != "molt.sealed-execution.v1"
        ):
            raise ValueError("sealed execution projection schema is invalid")
        return cls(
            value["mode"],
            value["memory_max_bytes"],
            value["workspace_max_bytes"],
            value["runtime_max_seconds"],
        )


SEALED_REPLAY_EXECUTION = SealedExecutionProjection("replay", 1073741824, 67108864, 60)


def _sealed_nonce(value: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{32}", value):
        raise ValueError("sealed systemd nonce is invalid")
    return value


def sealed_systemd_properties(
    nonce: str,
    root: str,
    *,
    projection: SealedExecutionProjection = SEALED_REPLAY_EXECUTION,
) -> dict[str, str]:
    """Closed launch contract, projected to both systemd and the receiver."""
    _sealed_nonce(nonce)
    if not re.fullmatch(r"/run/molt-release-[0-9a-f]{32}/rootfs", root):
        raise ValueError("sealed root must be in its system-manager-owned directory")
    if type(projection) is not SealedExecutionProjection:
        raise ValueError("sealed execution requires its typed projection")
    properties = {
        "Description": "molt-sealed:" + nonce,
        "Type": "exec",
        "ExitType": "cgroup",
        "RemainAfterExit": "yes",
        "Restart": "no",
        "RootDirectory": root,
        "RootImage": "",
        "RootEphemeral": "yes",
        "WorkingDirectory": "/app",
        "User": "65534",
        "Group": "65534",
        "SupplementaryGroups": "",
        "PAMName": "",
        "SetLoginEnvironment": "no",
        "Environment": " ".join(_SEALED_ENV),
        "EnvironmentFile": "",
        "PassEnvironment": "",
        "ExecSearchPath": "",
        "PrivatePIDs": "yes",
        "PrivateUsers": "yes",
        "PrivateNetwork": "yes",
        "PrivateIPC": "yes",
        "PrivateMounts": "yes",
        "PrivateDevices": "yes",
        "ProtectHostname": "yes",
        "ProtectControlGroups": "strict",
        "ProtectKernelTunables": "yes",
        "ProtectKernelLogs": "yes",
        "ProtectClock": "yes",
        "MountAPIVFS": "yes",
        "ProtectSystem": "strict",
        "ReadOnlyPaths": "/",
        "ReadWritePaths": "",
        "NoExecPaths": "/tmp /evidence",
        "InaccessiblePaths": "-/run/host -/proc/kcore -/sys/firmware",
        "TemporaryFileSystem": "/tmp:rw,noexec,nosuid,nodev,size=64m,mode=1777 /evidence:rw,noexec,nosuid,nodev,size=16m,mode=1777 /dev/shm:rw,noexec,nosuid,nodev,size=16m,mode=1777",
        "BindPaths": "",
        "BindReadOnlyPaths": "",
        "MountImages": "",
        "ExtensionImages": "",
        "ExtensionDirectories": "",
        "NetworkNamespacePath": "",
        "IPCNamespacePath": "",
        "CapabilityBoundingSet": "",
        "AmbientCapabilities": "",
        "NoNewPrivileges": "yes",
        "RestrictSUIDSGID": "yes",
        "RestrictNamespaces": "yes",
        "DevicePolicy": "closed",
        "DeviceAllow": "",
        "MemoryMax": str(projection.memory_max_bytes),
        "MemorySwapMax": "0",
        "TasksMax": "64",
        "CPUQuota": "100%",
        "KillMode": "control-group",
        "SendSIGKILL": "yes",
        "TimeoutStartSec": "60s",
        "TimeoutStopSec": "10s",
        "RuntimeMaxSec": f"{projection.runtime_max_seconds}s",
        "Delegate": "no",
        "StandardInput": "null",
        "FileDescriptorStoreMax": "0",
        "LimitFSIZE": "16777216",
        "ExecStartPre": "",
        "ExecStartPost": "",
        "ExecStop": "",
        "ExecStopPost": "",
        "ExecCondition": "",
    }
    if projection.mode == "compilation":
        properties.update(
            WorkingDirectory="/work",
            ReadWritePaths="/work",
            NoExecPaths="/evidence /work",
            LimitFSIZE=str(projection.workspace_max_bytes),
        )
        # Installed bundles and frontend environments may have lexical /tmp
        # coordinates. Preserve them in the read-only input root; the compiled
        # command gets TMPDIR=/work/tmp and no writable /tmp overlay.
        properties["TemporaryFileSystem"] = " ".join(
            row
            for row in properties["TemporaryFileSystem"].split()
            if not row.startswith("/tmp:")
        )
        properties["TemporaryFileSystem"] += (
            " /work:rw,noexec,nosuid,nodev,"
            f"size={projection.workspace_max_bytes},mode=1777"
        )
    return properties


def _systemd_observed_properties(properties: dict[str, str]) -> dict[str, object]:
    expected = dict(properties)
    for name in ("CPUQuota", "TimeoutStartSec", "TimeoutStopSec", "RuntimeMaxSec"):
        del expected[name]
    expected.update(
        CPUQuotaPerSecUSec=1000000,
        TimeoutStartUSec=60000000,
        TimeoutStopUSec=10000000,
        RuntimeMaxUSec=int(properties["RuntimeMaxSec"].removesuffix("s")) * 1000000,
    )
    expected["ProtectControlGroups"] = True
    expected["ProtectControlGroupsEx"] = "strict"
    expected["EnvironmentFiles"] = []
    del expected["EnvironmentFile"]
    for name in (
        "RootEphemeral",
        "RemainAfterExit",
        "SetLoginEnvironment",
        "PrivateUsers",
        "PrivateNetwork",
        "PrivateIPC",
        "PrivateMounts",
        "PrivateDevices",
        "ProtectHostname",
        "ProtectKernelTunables",
        "ProtectKernelLogs",
        "ProtectClock",
        "MountAPIVFS",
        "NoNewPrivileges",
        "RestrictSUIDSGID",
        "SendSIGKILL",
        "Delegate",
    ):
        expected[name] = properties[name] == "yes"
    for name in (
        "MemoryMax",
        "MemorySwapMax",
        "TasksMax",
        "FileDescriptorStoreMax",
        "LimitFSIZE",
    ):
        expected[name] = int(properties[name])
    for name in (
        "Environment",
        "SupplementaryGroups",
        "PassEnvironment",
        "ExecSearchPath",
        "ReadOnlyPaths",
        "ReadWritePaths",
        "NoExecPaths",
        "InaccessiblePaths",
        "BindPaths",
        "BindReadOnlyPaths",
        "MountImages",
        "ExtensionImages",
        "ExtensionDirectories",
        "DeviceAllow",
        "ExecStartPre",
        "ExecStartPost",
        "ExecStop",
        "ExecStopPost",
        "ExecCondition",
    ):
        expected[name] = properties[name].split() if properties[name] else []
    expected["TemporaryFileSystem"] = [
        row.split(":", 1) for row in properties["TemporaryFileSystem"].split()
    ]
    expected.update(
        CapabilityBoundingSet=0,
        AmbientCapabilities=0,
        RestrictNamespaces=0,
        JoinsNamespaceOf=[],
        TriggeredBy=[],
        OpenFile=[],
        ExtraFileDescriptorNames=[],
        NFileDescriptorStore=0,
        UserNamespacePath="",
        BindLogSockets=False,
        RootDirectoryStartOnly=False,
        ExecReload=[],
        NotifyAccess="none",
        LimitFSIZESoft=int(properties["LimitFSIZE"]),
        PrivateUsersEx="self",
        ProtectHostnameEx=["yes", ""],
    )
    return expected


def parse_systemd_observation(raw: str) -> dict[str, object]:
    # Keep the original typed D-Bus documents. `systemctl show` silently omits
    # empty structured properties and cannot prove omission rejection.
    from molt.exact_json import loads_exact

    if not isinstance(raw, str) or len(raw.encode()) > 4 * 1024 * 1024:
        raise ValueError("systemd observation exceeds its bounded protocol")
    envelope = loads_exact(raw)
    if not isinstance(envelope, dict) or set(envelope) != {"Unit", "Service"}:
        raise ValueError("systemd observation omits an interface")
    result = {}
    for interface in ("Unit", "Service"):
        document = loads_exact(envelope[interface])
        if (
            not isinstance(document, dict)
            or document.get("type") != "a{sv}"
            or set(document) != {"type", "data"}
            or not isinstance(document["data"], list)
            or len(document["data"]) != 1
            or not isinstance(document["data"][0], dict)
        ):
            raise ValueError("systemd observation is not a typed property dictionary")
        for key, value in document["data"][0].items():
            if (
                key in result
                or not isinstance(value, dict)
                or set(value) != {"type", "data"}
            ):
                raise ValueError("systemd observation repeats or corrupts a field")
            signature, data = value["type"], value["data"]
            if (
                signature == "b"
                and type(data) is not bool
                or signature in {"u", "t", "i", "x", "q", "n", "y"}
                and type(data) is not int
                or signature in {"s", "o", "g"}
                and not isinstance(data, str)
            ):
                raise ValueError("systemd observation has an invalid scalar type")
            if (
                not isinstance(signature, str)
                or not signature
                or (signature.startswith("a") and not isinstance(data, (list, dict)))
            ):
                raise ValueError("systemd observation has an invalid container type")
            if key == "Job" and (
                signature != "(uo)"
                or not isinstance(data, list)
                or len(data) != 2
                or type(data[0]) is not int
                or not 0 <= data[0] <= 0xFFFFFFFF
                or not isinstance(data[1], str)
                or not data[1].startswith("/")
            ):
                raise ValueError("systemd observation has an invalid job identity")
            result[key] = data
    return result


def _sealed_systemd_invocation(value: object) -> list[int]:
    if (
        not isinstance(value, list)
        or len(value) != 16
        or any(type(octet) is not int or not 0 <= octet <= 255 for octet in value)
        or not any(value)
    ):
        raise ValueError("sealed systemd invocation is not a nonzero 128-bit identity")
    return value


def validate_sealed_systemd_provider(value: object, *, arch: str) -> None:
    """Selected provenance only; effective per-invocation properties admit the API.

    No package-patch allowlist substitutes for observed isolation capability.
    """
    if (
        not isinstance(value, dict)
        or set(value) != {"schema", "version", "platform", "arch", "kernel", "tools"}
        or value["schema"] != SEALED_SYSTEMD_SCHEMA
        or not isinstance(value["version"], str)
        or not 1 <= len(value["version"]) <= 128
        or value["version"].strip() != value["version"]
        or value["platform"] != "linux"
        or arch not in {"x86_64", "aarch64"}
        or value["arch"] != arch
        or not isinstance(value["kernel"], str)
        or not value["kernel"]
        or not isinstance(value["tools"], dict)
        or set(value["tools"])
        != {
            "/usr/bin/systemd-run",
            "/usr/bin/systemctl",
            "/usr/bin/busctl",
            "/usr/bin/tar",
            "/usr/bin/true",
        }
        or any(
            not isinstance(h, str) or not re.fullmatch(r"[0-9a-f]{64}", h)
            for h in value["tools"].values()
        )
    ):
        raise ValueError(
            "sealed systemd selected provider provenance is outside the source contract"
        )


def validate_sealed_systemd_configuration(
    raw: str,
    *,
    nonce: str,
    root: str,
    command: list[str],
    projection: SealedExecutionProjection = SEALED_REPLAY_EXECUTION,
) -> dict[str, object]:
    observed = parse_systemd_observation(raw)
    expected = _systemd_observed_properties(
        sealed_systemd_properties(nonce, root, projection=projection)
    )
    for key, value in expected.items():
        if key not in observed or json.dumps(
            observed[key], sort_keys=True
        ) != json.dumps(value, sort_keys=True):
            raise ValueError(f"sealed systemd effective configuration differs: {key}")
    # These arguments are closed policy paths, not arbitrary shell input.
    if (
        len(command) != 6
        or command[:3] != ["/bin/molt-proof-supervisor", "run-export", "--policy"]
        or not re.fullmatch(r"/policies/[A-Za-z0-9_.-]+\.json", command[3])
        or command[4:] != ["--receipt", "/evidence/receipt.json"]
    ):
        raise ValueError("sealed supervisor command is outside the release policy")
    execution = observed.get("ExecStart")
    if (
        not isinstance(execution, list)
        or len(execution) != 1
        or not isinstance(execution[0], list)
        or len(execution[0]) != 10
        or execution[0][:2] != [command[0], command]
        or execution[0][2] is not False
        or any(type(value) is not int for value in execution[0][3:])
    ):
        raise ValueError("sealed systemd executable differs from the retained policy")
    _sealed_systemd_invocation(observed.get("InvocationID"))
    if observed.get("Id") != f"molt-release-{nonce}.service":
        raise ValueError("sealed systemd invocation custody is invalid")
    if observed.get("Transient") is not True or observed.get("DropInPaths") != []:
        raise ValueError("sealed systemd invocation has external unit configuration")
    return observed


def validate_sealed_systemd_terminal(raw: str, *, invocation: list[int]) -> None:
    value = parse_systemd_observation(raw)
    invocation = _sealed_systemd_invocation(invocation)
    observed_invocation = _sealed_systemd_invocation(value.get("InvocationID"))
    if (
        any(
            type(value.get(key)) is not int
            for key in (
                "MainPID",
                "ControlPID",
                "ExecMainCode",
                "ExecMainStatus",
                "NRestarts",
            )
        )
        or observed_invocation != invocation
        or value.get("ActiveState") != "active"
        or value.get("SubState") != "exited"
        or value.get("MainPID") != 0
        or value.get("ControlPID") != 0
        or value.get("ExecMainCode") != 1
        or value.get("ExecMainStatus") != 0
        or value.get("Result") != "success"
        or value.get("NRestarts") != 0
    ):
        raise ValueError("sealed systemd execution has no successful terminal custody")


def sealed_systemd_import_properties(
    nonce: str, sha256: str, input_file: str
) -> dict[str, str]:
    _sealed_nonce(nonce)
    if (
        not isinstance(sha256, str)
        or not re.fullmatch(r"[0-9a-f]{64}", sha256)
        or not re.fullmatch(r"/proc/[1-9][0-9]*/fd/[0-9]+", input_file)
    ):
        raise ValueError("sealed import has no exact immutable input coordinate")
    return {
        "Description": f"molt-sealed-import:{nonce}:{sha256}",
        "Type": "oneshot",
        "RemainAfterExit": "yes",
        "RuntimeDirectory": f"molt-release-{nonce}/rootfs",
        "RuntimeDirectoryMode": "0755",
        "RuntimeDirectoryPreserve": "no",
        "User": "root",
        "Group": "root",
        "NoNewPrivileges": "yes",
        "CapabilityBoundingSet": "",
        "AmbientCapabilities": "",
        "PrivateNetwork": "yes",
        "PrivateIPC": "yes",
        "PrivateDevices": "yes",
        "PrivateMounts": "yes",
        "ProtectSystem": "strict",
        "ProtectHome": "yes",
        "ProtectKernelTunables": "yes",
        "ProtectKernelLogs": "yes",
        "ProtectControlGroups": "strict",
        "RestrictSUIDSGID": "yes",
        "RestrictNamespaces": "yes",
        "ReadWritePaths": f"/run/molt-release-{nonce}/rootfs",
        "StandardInput": "file:" + input_file,
        "StandardOutput": "null",
        "StandardError": "null",
        "Environment": "LANG=C LC_ALL=C",
        "EnvironmentFile": "",
        "PassEnvironment": "",
        "UMask": "0022",
        "TimeoutStartSec": "120s",
        "TimeoutStopSec": "10s",
        "Restart": "no",
        "MemoryMax": "2147483648",
        "MemorySwapMax": "0",
        "TasksMax": "16",
        "CPUQuota": "100%",
        "LimitFSIZE": "1073741824",
        "KillMode": "control-group",
        "SendSIGKILL": "yes",
        "Delegate": "no",
        "ExecStartPre": "",
        "ExecStartPost": "",
        "ExecStopPost": "",
        "ExecCondition": "",
        # Keep imported runtime data alive until its explicit stop job.
        "ExecStop": "/usr/bin/true",
    }


def validate_sealed_systemd_import(
    raw: str, *, nonce: str, sha256: str, root: str
) -> dict:
    properties = sealed_systemd_import_properties(nonce, sha256, "/proc/1/fd/0")
    if root != f"/run/molt-release-{nonce}/rootfs":
        raise ValueError("sealed import root belongs to another archive")
    expected = {
        k: v
        for k, v in properties.items()
        if k
        not in {
            "StandardInput",
            "EnvironmentFile",
            "TimeoutStartSec",
            "TimeoutStopSec",
            "CPUQuota",
            "ExecStop",
        }
    }
    for key in (
        "RemainAfterExit",
        "NoNewPrivileges",
        "PrivateNetwork",
        "PrivateIPC",
        "PrivateDevices",
        "PrivateMounts",
        "ProtectKernelTunables",
        "ProtectKernelLogs",
        "RestrictSUIDSGID",
        "SendSIGKILL",
        "Delegate",
    ):
        expected[key] = properties[key] == "yes"
    for key in ("MemoryMax", "MemorySwapMax", "TasksMax", "LimitFSIZE"):
        expected[key] = int(properties[key])
    for key in (
        "RuntimeDirectory",
        "ReadWritePaths",
        "Environment",
        "PassEnvironment",
        "ExecStartPre",
        "ExecStartPost",
        "ExecStopPost",
        "ExecCondition",
    ):
        expected[key] = properties[key].split()
    expected.update(
        CapabilityBoundingSet=0,
        AmbientCapabilities=0,
        RestrictNamespaces=0,
        ProtectControlGroups=True,
        ProtectControlGroupsEx="strict",
        UMask=0o22,
        RuntimeDirectoryMode=0o755,
        StandardInput="file",
        EnvironmentFiles=[],
        TimeoutStartUSec=120000000,
        TimeoutStopUSec=10000000,
        CPUQuotaPerSecUSec=1000000,
        LimitFSIZESoft=1073741824,
        JoinsNamespaceOf=[],
        TriggeredBy=[],
        Transient=True,
        DropInPaths=[],
        Id=f"molt-release-import-{nonce}.service",
    )
    observed = parse_systemd_observation(raw)
    for key, value in expected.items():
        if key not in observed or json.dumps(
            observed[key], sort_keys=True
        ) != json.dumps(value, sort_keys=True):
            raise ValueError(f"sealed import effective configuration differs: {key}")
    command = [
        "/usr/bin/tar",
        "--extract",
        "--file=-",
        "--directory=" + root,
        "--no-same-owner",
        "--same-permissions",
        "--delay-directory-restore",
    ]
    for name, argv in (("ExecStart", command), ("ExecStop", ["/usr/bin/true"])):
        rows = observed.get(name)
        if (
            not isinstance(rows, list)
            or len(rows) != 1
            or not isinstance(rows[0], list)
            or len(rows[0]) != 10
            or rows[0][:2] != [argv[0], argv]
            or rows[0][2] is not False
            or any(type(x) is not int for x in rows[0][3:])
        ):
            raise ValueError("sealed import executable differs")
    invocation = _sealed_systemd_invocation(observed.get("InvocationID"))
    validate_sealed_systemd_terminal(raw, invocation=invocation)
    return observed


def validate_sealed_systemd_stop(
    record: object, *, unit: str, invocation: list[int]
) -> None:
    invocation = _sealed_systemd_invocation(invocation)
    if (
        not isinstance(record, dict)
        or set(record)
        != {
            "unit",
            "invocation",
            "before",
            "stop_returncode",
            "stop_stdout",
            "stop_stderr",
            "load_state",
            "after",
        }
        or record["unit"] != unit
        or json.dumps(record["invocation"]) != json.dumps(invocation)
    ):
        raise ValueError("sealed teardown belongs to another invocation")
    before = parse_systemd_observation(record["before"])
    if (
        before.get("Id") != unit
        or json.dumps(before.get("InvocationID")) != json.dumps(invocation)
        or type(record["stop_returncode"]) is not int
        or record["stop_returncode"] != 0
        or record["stop_stdout"] != ""
        or record["stop_stderr"] != ""
    ):
        raise ValueError("sealed teardown stop job is unproved")
    if record["load_state"] == "not-found\n" and record["after"] is None:
        return
    if record["load_state"] != "loaded\n" or not isinstance(record["after"], str):
        raise ValueError("sealed teardown final state is unproved")
    after = parse_systemd_observation(record["after"])
    if (
        after.get("Id") != unit
        or json.dumps(after.get("InvocationID")) != json.dumps(invocation)
        or after.get("ActiveState") != "inactive"
        or after.get("SubState") != "dead"
        or type(after.get("MainPID")) is not int
        or type(after.get("ControlPID")) is not int
        or after.get("MainPID") != 0
        or after.get("ControlPID") != 0
    ):
        raise ValueError("sealed teardown has live or foreign custody")


def _sealed_owned_root(root: Path) -> list[dict]:
    import stat
    from tools.release import execution_root

    rows = execution_root.root_inventory(root)
    # Inventory normalizes executable modes; admission must separately reject
    # write bits, ownership drift, special bits and mutable ancestor directories.
    for ancestor in (root.parent, root, *root.rglob("*")):
        metadata = ancestor.lstat()
        if metadata.st_uid != 0 or metadata.st_gid != 0 or metadata.st_mode & 0o7022:
            raise ValueError(
                "imported root is not exclusively owned by the system manager"
            )
        if ancestor not in (root.parent, root) and stat.S_IMODE(
            metadata.st_mode
        ) not in (0o444, 0o555):
            raise ValueError("imported root permissions differ from the sealed archive")
    return rows


class SealedRootTransport:
    """One nonce-owned systemd import/service lifetime; privileged tools only.

    All Python remains unprivileged. No helper changes namespaces or capabilities.
    Explicit operator authorization is required for the fixed sudo systemd-run and
    systemctl invocations. There is no automatic setup, fallback or policy change.
    """

    def __init__(self):
        self._owned: dict[str, list[int]] = {}
        self._unresolved: set[str] = set()
        self._root_unit: str | None = None
        self._execution: SealedExecutionProjection | None = None

    @staticmethod
    def _control(args: list[str], *, privileged: bool = False, timeout: int = 30):
        prefix = ["/usr/bin/sudo", "--non-interactive", "--"] if privileged else []
        result = _guarded_run(
            prefix + args,
            capture_output=True,
            text=True,
            timeout=timeout,
            env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"},
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"sealed systemd control refused: {result.stderr[:1000]}"
            )
        return result

    def _show(self, unit: str) -> tuple[str, dict[str, object]]:
        from molt.exact_json import loads_exact

        address = self._control(
            [
                "/usr/bin/busctl",
                "--json=short",
                "call",
                "org.freedesktop.systemd1",
                "/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager",
                "GetUnit",
                "s",
                unit,
            ]
        ).stdout
        obj = loads_exact(address)
        if (
            obj.get("type") != "o"
            or not isinstance(obj.get("data"), list)
            or len(obj["data"]) != 1
            or not str(obj["data"][0]).startswith("/org/freedesktop/systemd1/unit/")
        ):
            raise ValueError("systemd unit has no manager-owned object")
        records = {}
        for interface in ("Unit", "Service"):
            records[interface] = self._control(
                [
                    "/usr/bin/busctl",
                    "--json=short",
                    "call",
                    "org.freedesktop.systemd1",
                    obj["data"][0],
                    "org.freedesktop.DBus.Properties",
                    "GetAll",
                    "s",
                    "org.freedesktop.systemd1." + interface,
                ]
            ).stdout
        raw = json.dumps(records, sort_keys=True)
        return raw, parse_systemd_observation(raw)

    def _provider(self) -> dict:
        # Selected-file provenance only. The host kernel and system manager are
        # the trusted execution provider, as the Docker daemon was previously.
        # Guest used-byte identity remains the native supervisor authority.
        import platform
        import stat
        from molt.toolchain_identity import capture_stable_regular_file

        tools = {}
        for path in (
            "/usr/bin/systemd-run",
            "/usr/bin/systemctl",
            "/usr/bin/busctl",
            "/usr/bin/tar",
            "/usr/bin/true",
        ):
            selected = Path(path).resolve(strict=True)
            metadata = selected.stat()
            if metadata.st_uid != 0 or metadata.st_mode & (stat.S_IWGRP | stat.S_IWOTH):
                raise ValueError("sealed provider is not protected by system ownership")
            identity, _ = capture_stable_regular_file(
                selected, max_bytes=64 * 1024 * 1024, label="systemd provider"
            )
            tools[path] = identity.sha256
        return {
            "schema": SEALED_SYSTEMD_SCHEMA,
            "version": self._control(
                ["/usr/bin/systemctl", "show", "--property=Version", "--value"]
            ).stdout.strip(),
            "platform": sys.platform,
            "arch": platform.machine(),
            "kernel": platform.release(),
            "tools": tools,
        }

    def _claim(self, unit: str, description: str) -> str:
        raw, value = self._show(unit)
        invocation = _sealed_systemd_invocation(value.get("InvocationID"))
        if value.get("Id") != unit or value.get("Description") != description:
            raise ValueError("systemd unit creation lost nonce custody; retained")
        self._owned[unit] = invocation
        return raw

    def _stop(self, unit: str) -> dict:
        before, value = self._show(unit)
        if (
            _sealed_systemd_invocation(value.get("InvocationID")) != self._owned[unit]
            or value.get("Id") != unit
        ):
            raise ValueError("systemd unit identity changed; resources retained")
        result = self._control(
            ["/usr/bin/systemctl", "stop", "--", unit], privileged=True
        )
        state = self._control(
            [
                "/usr/bin/systemctl",
                "show",
                "--property=LoadState",
                "--value",
                "--",
                unit,
            ]
        ).stdout
        # The blocking stop job is the manager's teardown authority. A transient
        # unit may already be garbage-collected; do not fabricate cgroup.events.
        after = None if state == "not-found\n" else self._show(unit)[0]
        record = {
            "unit": unit,
            "invocation": self._owned[unit],
            "before": before,
            "stop_returncode": result.returncode,
            "stop_stdout": result.stdout,
            "stop_stderr": result.stderr,
            "load_state": state,
            "after": after,
        }
        validate_sealed_systemd_stop(record, unit=unit, invocation=self._owned[unit])
        del self._owned[unit]
        self._unresolved.discard(unit)
        return record

    def import_sealed_root(
        self,
        archive: Path,
        *,
        sha256: str,
        arch: str,
        evidence: Path,
        projection: SealedExecutionProjection = SEALED_REPLAY_EXECUTION,
    ) -> dict:
        from molt.toolchain_identity import capture_stable_regular_file
        from tools.release import execution_root

        if type(projection) is not SealedExecutionProjection:
            raise ValueError("sealed import requires its typed execution projection")
        if (
            self._execution is not None
            or self._root_unit is not None
            or self._owned
            or self._unresolved
        ):
            raise ValueError("sealed import has existing root or unresolved custody")
        if sys.platform != "linux":
            raise RuntimeError(
                "sealed systemd execution requires an admitted native Linux runner"
            )
        self._provider_identity = self._provider()
        validate_sealed_systemd_provider(self._provider_identity, arch=arch)
        identity, data = capture_stable_regular_file(
            archive, label="sealed archive", max_bytes=1024 * 1024 * 1024
        )
        if identity.sha256 != sha256:
            raise ValueError("sealed archive changed before immutable import")
        self._source_root = archive.parent / "rootfs"
        self._files = execution_root.root_inventory(self._source_root)
        execution_root.validate_sealed_tar(
            archive,
            self._source_root,
            expected_archive={"sha256": sha256, "size": identity.size},
            projection=projection,
        )
        nonce = secrets.token_hex(16)
        unit = f"molt-release-import-{nonce}.service"
        description = f"molt-sealed-import:{nonce}:{sha256}"
        self._root = f"/run/molt-release-{nonce}/rootfs"
        if Path(self._root).exists():
            raise ValueError("sealed import runtime directory already exists")
        evidence.mkdir(parents=True, exist_ok=False)
        self._evidence = evidence
        import fcntl

        fd = os.memfd_create("molt-sealed-root", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
        try:
            with os.fdopen(os.dup(fd), "wb") as stream:
                stream.write(data)
            os.lseek(fd, 0, os.SEEK_SET)
            seals = (
                fcntl.F_SEAL_SEAL
                | fcntl.F_SEAL_SHRINK
                | fcntl.F_SEAL_GROW
                | fcntl.F_SEAL_WRITE
            )
            fcntl.fcntl(fd, fcntl.F_ADD_SEALS, seals)
            if fcntl.fcntl(fd, fcntl.F_GET_SEALS) != seals:
                raise ValueError("sealed archive is not immutable")
            properties = sealed_systemd_import_properties(
                nonce, sha256, f"/proc/{os.getpid()}/fd/{fd}"
            )
            command = [
                "/usr/bin/systemd-run",
                "--quiet",
                "--expand-environment=no",
                "--unit=" + unit,
            ]
            command += [
                "--property=" + key + "=" + value for key, value in properties.items()
            ]
            command += [
                "--",
                "/usr/bin/tar",
                "--extract",
                "--file=-",
                "--directory=" + self._root,
                "--no-same-owner",
                "--same-permissions",
                "--delay-directory-restore",
            ]
            # The system manager opens the sealed memfd while its unprivileged owner
            # keeps it alive. The tar service receives it only as stdin.
            intent = {
                "unit": unit,
                "description": description,
                "archive_sha256": sha256,
                "root": self._root,
                "command": command,
                "execution": projection.to_dict(),
            }
            with (evidence / (nonce + ".intent.json")).open("x") as stream:
                json.dump(intent, stream, sort_keys=True)
            self._unresolved.add(unit)
            try:
                self._control(command, privileged=True, timeout=130)
            finally:
                # Recover an acknowledged or uncertain dispatch only by the
                # exact nonce/description, never by a PID or directory name.
                if (
                    self._control(
                        [
                            "/usr/bin/systemctl",
                            "show",
                            "--property=LoadState",
                            "--value",
                            "--",
                            unit,
                        ]
                    ).stdout.strip()
                    != "not-found"
                ):
                    observed = self._claim(unit, description)
                    self._root_unit = unit
        finally:
            os.close(fd)
        imported = _sealed_owned_root(Path(self._root))
        if imported != self._files:
            raise ValueError("root-owned imported image differs from sealed archive")
        validate_sealed_systemd_import(
            observed, nonce=nonce, sha256=sha256, root=self._root
        )
        self._import = {
            "archive_sha256": sha256,
            "root": self._root,
            "files": imported,
            "nonce": nonce,
            "observation": observed,
            "provider": self._provider_identity,
            "execution": projection.to_dict(),
        }
        self._execution = projection
        return self._import

    def run_sealed(
        self,
        command: list[str],
        *,
        timeout: int,
        projection: SealedExecutionProjection = SEALED_REPLAY_EXECUTION,
    ) -> dict:
        from concurrent.futures import ThreadPoolExecutor

        if (
            type(projection) is not SealedExecutionProjection
            or self._execution != projection
        ):
            raise ValueError(
                "sealed execution differs from the imported root projection"
            )
        if (
            _sealed_owned_root(Path(self._root)) != self._files
            or self._provider() != self._provider_identity
        ):
            raise ValueError("sealed source or provider drifted before execution")
        nonce = secrets.token_hex(16)
        unit = f"molt-release-{nonce}.service"
        properties = sealed_systemd_properties(nonce, self._root, projection=projection)
        # --pipe passes only safe stdio descriptors over D-Bus; service manager
        # owns FD closure and process creation. RemainAfterExit retains evidence.
        argv = [
            "/usr/bin/sudo",
            "--non-interactive",
            "--",
            "/usr/bin/systemd-run",
            "--quiet",
            "--pipe",
            "--expand-environment=no",
            "--unit=" + unit,
        ]
        argv += ["--property=" + k + "=" + v for k, v in properties.items()]
        argv += ["--", *command]
        stdout = self._evidence / (nonce + ".stdout")
        stderr = self._evidence / (nonce + ".stderr")
        stream_limits = (
            (stdout, projection.stdout_max_bytes),
            (stderr, 16 * 1024 * 1024),
        )

        def streams_exceeded() -> bool:
            return any(
                path.exists() and path.stat().st_size > bound
                for path, bound in stream_limits
            )

        proc = None
        pool = ThreadPoolExecutor(max_workers=1, thread_name_prefix="molt-sealed-guard")
        stopped = False
        try:
            with (self._evidence / (nonce + ".intent.json")).open("x") as stream:
                json.dump(
                    {
                        "unit": unit,
                        "description": "molt-sealed:" + nonce,
                        "command": argv,
                        "execution": projection.to_dict(),
                    },
                    stream,
                    sort_keys=True,
                )
            self._unresolved.add(unit)
            proc = pool.submit(
                _guarded_run,
                argv,
                timeout=timeout + 30,
                env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"},
                stdout_capture_path=stdout,
                stderr_capture_path=stderr,
                capture_tail_bytes=4096,
                cancellation_requested=streams_exceeded,
            )
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if streams_exceeded():
                    raise ValueError("sealed output exceeded its bounded transport")
                if (
                    self._control(
                        [
                            "/usr/bin/systemctl",
                            "show",
                            "--property=LoadState",
                            "--value",
                            "--",
                            unit,
                        ]
                    ).stdout.strip()
                    == "not-found"
                ):
                    if proc.done():
                        raise RuntimeError(
                            "systemd launch refused before unit creation"
                        )
                    time.sleep(0.05)
                    continue
                raw, value = self._show(unit)
                if value.get("InvocationID") and unit not in self._owned:
                    self._claim(unit, "molt-sealed:" + nonce)
                if (
                    value.get("SubState") in {"exited", "failed", "dead"}
                    and unit in self._owned
                ):
                    break
                if proc.done():
                    raise RuntimeError(
                        "systemd launch exited before retained terminal observation"
                    )
                time.sleep(0.05)
            else:
                raise TimeoutError("sealed systemd execution deadline expired")
            validate_sealed_systemd_configuration(
                raw,
                nonce=nonce,
                root=self._root,
                command=command,
                projection=projection,
            )
            validate_sealed_systemd_terminal(raw, invocation=self._owned[unit])
            # ExitType=cgroup makes the manager observe cgroup emptiness
            # before SERVICE_EXITED. It prunes the cgroup at that point.
            stop = self._stop(unit)
            stopped = True
            if proc.result(timeout=15).returncode != 0:
                raise ValueError("sealed systemd pipe transport failed")
            if streams_exceeded():
                raise ValueError("sealed output exceeded its bounded transport")
        finally:
            primary = sys.exception()
            try:
                if not stopped and unit not in self._owned and proc is not None:
                    if (
                        self._control(
                            [
                                "/usr/bin/systemctl",
                                "show",
                                "--property=LoadState",
                                "--value",
                                "--",
                                unit,
                            ]
                        ).stdout.strip()
                        != "not-found"
                    ):
                        self._claim(unit, "molt-sealed:" + nonce)
                if unit in self._owned:
                    self._stop(unit)
                if unit in self._unresolved:
                    raise RuntimeError(
                        "systemd creation outcome unknown; imported source and intent retained"
                    )
                if proc is not None:
                    # Exact service stop releases the owned systemd-run waiter.
                    proc.result(timeout=15)
            except BaseException as cleanup:
                if primary is not None:
                    raise BaseExceptionGroup(
                        "sealed invocation and cleanup failed", [primary, cleanup]
                    ) from None
                raise
            finally:
                pool.shutdown(wait=True)
        if (
            _sealed_owned_root(Path(self._root)) != self._files
            or self._provider() != self._provider_identity
        ):
            raise ValueError(
                "root-owned source or selected provider changed during ephemeral execution"
            )
        return {
            "nonce": nonce,
            "execution": projection.to_dict(),
            "observation": raw,
            "teardown": stop,
            "stdout_path": str(stdout),
            "stderr_path": str(stderr),
        }

    def remove_sealed_root(self) -> None:
        if any(unit != self._root_unit for unit in self._unresolved):
            raise RuntimeError(
                "systemd creation outcome unknown; imported root retained"
            )
        if self._root_unit is not None:
            if any(unit != self._root_unit for unit in self._owned):
                raise ValueError(
                    "guest service custody unresolved; imported root retained"
                )
            stop = self._stop(self._root_unit)
            if Path(self._root).exists():
                raise ValueError("import runtime directory retirement is unproved")
            if hasattr(self, "_import"):
                self._import["teardown"] = stop
            self._root_unit = None
            self._execution = None


def transport_for(host: Host) -> Transport:
    if host.transport == "ssh":
        return SSHTransport(host)
    if host.transport == "docker":
        return DockerTransport(host)
    raise AssertionError(f"unreachable transport {host.transport!r}")


# ---------------------------------------------------------------------------
# Result aggregation
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class CaseResult:
    case: str
    status: str  # PASS | FAIL | SKIP
    duration: float
    detail: str = ""


@dataclasses.dataclass
class HostResult:
    host: Host
    cases: list[CaseResult] = dataclasses.field(default_factory=list)
    setup_error: str | None = None
    cleanup_error: str | None = None

    def passed(self) -> int:
        return sum(1 for c in self.cases if c.status == "PASS")

    def failed(self) -> int:
        return sum(1 for c in self.cases if c.status == "FAIL")

    def skipped(self) -> int:
        return sum(1 for c in self.cases if c.status == "SKIP")


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------


def _normalize_stdout(s: str) -> str:
    # Windows CRLF -> LF; trim trailing whitespace per line and final EOL.
    lines = [line.rstrip() for line in s.replace("\r\n", "\n").splitlines()]
    return "\n".join(lines).strip()


def _run_one_host(
    host: Host,
    cases: list[Case],
    builds: dict[tuple[str, str], tuple[Path | None, str]],
    expected: dict[str, str],
    *,
    keep_remote: bool,
    verbose: bool,
) -> HostResult:
    result = HostResult(host=host)
    transport = transport_for(host)
    local_stage: OwnedTemporaryDirectory | None = None
    primary: BaseException | None = None
    try:
        phase = "transport.prepare"
        try:
            transport.prepare()
            # Renaming must never publish copies beside shared build outputs.
            phase = "local staging"
            push_files: list[Path] = []
            case_to_remote: dict[str, str] = {}
            for c in cases:
                binary, _ = builds[(host.target, c.name)]
                if binary is None:
                    continue
                remote_name = f"{c.name}{binary.suffix}"
                staged = binary
                if binary.name != remote_name:
                    if local_stage is None:
                        local_stage = OwnedTemporaryDirectory(prefix="molt_cross_push_")
                    staged = Path(local_stage.name) / remote_name
                    shutil.copy2(binary, staged)
                    os.chmod(staged, 0o755)
                push_files.append(staged)
                case_to_remote[c.name] = remote_name
            phase = "transport.push"
            transport.push(push_files)
        except Exception as exc:
            result.setup_error = f"{phase} failed: {exc}"
            for c in cases:
                result.cases.append(CaseResult(c.name, "SKIP", 0.0, result.setup_error))
            return result

        for c in cases:
            binary, log = builds[(host.target, c.name)]
            if binary is None:
                result.cases.append(
                    CaseResult(
                        case=c.name,
                        status="SKIP",
                        duration=0.0,
                        detail=f"local build failed for target={host.target}\n"
                        + log[-800:],
                    )
                )
                continue
            remote_name = case_to_remote[c.name]
            remote_path = transport.remote_path(remote_name)
            t0 = time.monotonic()
            try:
                code, out, err = transport.run([remote_path], timeout=60)
            except subprocess.TimeoutExpired:
                result.cases.append(
                    CaseResult(
                        case=c.name,
                        status="FAIL",
                        duration=60.0,
                        detail="remote execution timed out after 60s",
                    )
                )
                continue
            duration = time.monotonic() - t0
            actual = _normalize_stdout(out)
            want = _normalize_stdout(expected[c.name])
            if code != 0:
                result.cases.append(
                    CaseResult(
                        case=c.name,
                        status="FAIL",
                        duration=duration,
                        detail=f"exit={code}\nstderr:\n{err[-400:]}\nstdout:\n{out[-400:]}",
                    )
                )
            elif actual != want:
                result.cases.append(
                    CaseResult(
                        case=c.name,
                        status="FAIL",
                        duration=duration,
                        detail=(
                            "stdout mismatch\n"
                            f"--- expected ---\n{want}\n"
                            f"--- actual ---\n{actual}"
                        ),
                    )
                )
            else:
                result.cases.append(
                    CaseResult(case=c.name, status="PASS", duration=duration, detail="")
                )
    except BaseException as exc:
        primary = exc
        raise
    finally:
        errors: list[BaseException] = []
        if not keep_remote:
            try:
                transport.cleanup()
            except BaseException as exc:
                errors.append(exc)
        if local_stage is not None:
            try:
                local_stage.cleanup()
            except BaseException as exc:
                errors.append(exc)
        if errors:
            if primary is not None:
                raise BaseExceptionGroup(
                    "Cross-run execution and cleanup failed", [primary, *errors]
                ) from None
            # Ordinary cleanup failures belong beside setup/case diagnostics.
            # Cancellation or another BaseException must still escape.
            if any(not isinstance(exc, Exception) for exc in errors):
                raise BaseExceptionGroup("Cross-run cleanup failed", errors)
            result.cleanup_error = "; ".join(str(exc) for exc in errors)
    return result


def _print_matrix(results: list[HostResult], cases: list[Case]) -> None:
    sys.stdout.write("\n")
    total_pass = total_fail = total_skip = 0
    case_width = max(len(c.name) for c in cases)
    for r in results:
        sys.stdout.write(f"[CROSS] target={r.host.target} host={r.host.name}\n")
        for c in r.cases:
            sys.stdout.write(
                f"  {c.case.ljust(case_width)}  {c.status:<4}  {c.duration:.1f}s\n"
            )
            if c.status == "PASS":
                total_pass += 1
            elif c.status == "FAIL":
                total_fail += 1
            else:
                total_skip += 1
        if r.setup_error:
            sys.stdout.write(f"  [setup error] {r.setup_error}\n")
        if r.cleanup_error:
            sys.stdout.write(f"  [cleanup error] {r.cleanup_error}\n")
    sys.stdout.write("\nMatrix:\n")
    for r in results:
        n = len(r.cases)
        line = f"  {r.host.name} / {r.host.target}    {r.passed()}/{n} PASS"
        if r.failed():
            line += f"  ({r.failed()} FAIL)"
        if r.skipped():
            line += f"  ({r.skipped()} SKIP)"
        sys.stdout.write(line + "\n")
    sys.stdout.write(
        f"TOTAL: {len(results)} host{'s' if len(results) != 1 else ''}, "
        f"{total_pass + total_fail + total_skip} test"
        f"{'s' if (total_pass + total_fail + total_skip) != 1 else ''}, "
        f"{total_pass} passed, {total_fail} failed"
    )
    if total_skip:
        sys.stdout.write(f", {total_skip} skipped")
    sys.stdout.write("\n")
    sys.stdout.flush()


def _write_report(
    results: list[HostResult], cases: list[Case], report_dir: Path
) -> Path:
    report_dir.mkdir(parents=True, exist_ok=True)
    ts = _dt.datetime.now().strftime("%Y%m%d_%H%M%S")
    path = report_dir / f"cross_platform_matrix.{ts}.md"
    lines: list[str] = []
    lines.append(f"# Cross-platform matrix — {ts}")
    lines.append("")
    lines.append(f"Hosts: {len(results)}; cases: {len(cases)}")
    lines.append("")
    header = "| host | target | " + " | ".join(c.name for c in cases) + " |"
    sep = "|" + "---|" * (2 + len(cases))
    lines.append(header)
    lines.append(sep)
    for r in results:
        cells = []
        by_name = {c.case: c for c in r.cases}
        for c in cases:
            cell = by_name.get(c.name)
            cells.append(cell.status if cell else "n/a")
        lines.append(f"| {r.host.name} | {r.host.target} | " + " | ".join(cells) + " |")
    lines.append("")
    # Per-failure detail.
    any_detail = False
    for r in results:
        for c in r.cases:
            if c.status == "FAIL" or (c.status == "SKIP" and c.detail):
                if not any_detail:
                    lines.append("## Diagnostics")
                    lines.append("")
                    any_detail = True
                lines.append(f"### {r.host.name} / {c.case} — {c.status}")
                lines.append("")
                lines.append("```")
                lines.append(c.detail.strip() or "(no detail)")
                lines.append("```")
                lines.append("")
        for kind, diagnostic in (
            ("setup", r.setup_error),
            ("cleanup", r.cleanup_error),
        ):
            if diagnostic is None:
                continue
            if not any_detail:
                lines.append("## Diagnostics")
                lines.append("")
                any_detail = True
            lines.append(f"### {r.host.name} — {kind} error")
            lines.append("")
            lines.append("```")
            lines.append(diagnostic)
            lines.append("```")
            lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")
    return path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Cross-platform distributed test runner for molt"
    )
    parser.add_argument(
        "--inventory",
        default=str(DEFAULT_INVENTORY),
        help="path to cross_hosts.toml (default: tools/cross_hosts.toml)",
    )
    parser.add_argument(
        "--smoke",
        action="store_true",
        help="run the smoke corpus (default if no corpus selected)",
    )
    parser.add_argument(
        "--full",
        action="store_true",
        help="run the full compliance corpus (slow; not yet implemented)",
    )
    parser.add_argument(
        "--hosts",
        default="",
        help="comma-separated subset of host names from the inventory",
    )
    parser.add_argument(
        "--report",
        action="store_true",
        help="write a markdown matrix to reports/cross_platform_matrix.<ts>.md",
    )
    parser.add_argument(
        "--report-dir",
        default=str(DEFAULT_REPORT_DIR),
        help="directory to write the markdown report into",
    )
    parser.add_argument(
        "--keep-remote",
        action="store_true",
        help="retain this run’s SSH child / Docker stage files (containers still retire)",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="emit extra build/transport diagnostics",
    )
    args = parser.parse_args(argv)

    if args.full:
        # The compliance pytest suite needs an installed pytest on each
        # remote, which is incompatible with the read-only sandbox the
        # smoke corpus enforces. Until we ship a self-contained
        # compliance-runner binary, refuse explicitly rather than
        # pretending to support it.
        sys.stderr.write(
            "error: --full corpus is not yet wired up. Use --smoke for now.\n"
        )
        return 2
    if not args.smoke:
        args.smoke = True  # default

    cases = list(SMOKE_CASES)

    # Capture expected outputs once via host CPython.
    expected: dict[str, str] = {}
    for c in cases:
        expected[c.name] = _local_expected(c)

    inventory_path = Path(args.inventory).expanduser().resolve()
    hosts = parse_inventory(inventory_path)
    if args.hosts.strip():
        wanted = {n.strip() for n in args.hosts.split(",") if n.strip()}
        unknown = wanted - {h.name for h in hosts}
        if unknown:
            sys.stderr.write(
                f"error: --hosts references unknown names: {sorted(unknown)}\n"
            )
            return 2
        hosts = [h for h in hosts if h.name in wanted]
    if not hosts:
        sys.stderr.write("error: no hosts selected\n")
        return 2

    # Build matrix: one (target, case) -> binary mapping. Two hosts on
    # the same target reuse the same binary, the runner just ships it
    # twice.
    targets = sorted({h.target for h in hosts})
    work = Path(tempfile.mkdtemp(prefix="molt_cross_build_"))
    builds: dict[tuple[str, str], tuple[Path | None, str]] = {}
    sys.stdout.write(
        f"[cross_run] inventory={inventory_path} hosts={len(hosts)} "
        f"targets={','.join(targets)} cases={len(cases)}\n"
    )
    sys.stdout.flush()
    for target in targets:
        for c in cases:
            sys.stdout.write(f"[cross_run] build target={target} case={c.name} ... ")
            sys.stdout.flush()
            t0 = time.monotonic()
            binary, log = _local_compile(c, target, work, verbose=args.verbose)
            elapsed = time.monotonic() - t0
            if binary is None:
                sys.stdout.write(f"FAIL ({elapsed:.1f}s)\n")
                if args.verbose:
                    sys.stdout.write(log + "\n")
            else:
                sys.stdout.write(f"ok ({elapsed:.1f}s, {binary.name})\n")
            sys.stdout.flush()
            builds[(target, c.name)] = (binary, log)

    # Execute on each host.
    results: list[HostResult] = []
    for host in hosts:
        r = _run_one_host(
            host,
            cases,
            builds,
            expected,
            keep_remote=args.keep_remote,
            verbose=args.verbose,
        )
        results.append(r)

    _print_matrix(results, cases)
    if args.report:
        report = _write_report(results, cases, Path(args.report_dir))
        sys.stdout.write(f"\nReport: {report}\n")

    # Exit non-zero if anything failed; SKIP alone is OK so the loopback
    # smoke run still exits 0 when every case passed.
    any_fail = any(any(c.status == "FAIL" for c in r.cases) for r in results)
    any_host_err = any(r.setup_error or r.cleanup_error for r in results)
    return 1 if (any_fail or any_host_err) else 0


if __name__ == "__main__":
    sys.exit(main())
