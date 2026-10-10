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
* The runner is pure stdlib (Python >= 3.11 for tomllib). It shells out
  to ssh / scp / rsync / docker — the only requirement on the dev box is
  that those tools are on PATH.
* Read-only on the remote outside ``remote_dir``: every file it writes
  goes under that directory; cleanup deletes the directory at the end
  unless ``--keep-remote`` is passed.

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
from pathlib import Path
from collections.abc import Callable

# Repo root: tools/cross_run.py -> tools -> repo
REPO = Path(__file__).resolve().parents[1]
DEFAULT_INVENTORY = REPO / "tools" / "cross_hosts.toml"
DEFAULT_REPORT_DIR = REPO / "reports"
TOOLS_ROOT = REPO / "tools"

if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import harness_memory_guard  # noqa: E402
from molt.dx import ARTIFACT_ROOT_ENV, artifact_root  # noqa: E402

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

    def remote_path(self, *parts: str) -> str:
        sep = "\\" if self.is_windows else "/"
        return self.remote_dir.rstrip("/\\") + sep + sep.join(parts)


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
    env.setdefault(ARTIFACT_ROOT_ENV, str(artifact_root(REPO, env)))
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
        if self.host.shell == "powershell":
            mkdir = (
                f"if (-not (Test-Path -LiteralPath '{self.host.remote_dir}')) "
                f"{{ New-Item -ItemType Directory -Path '{self.host.remote_dir}' "
                f"-Force | Out-Null }}"
            )
        else:
            mkdir = f"mkdir -p {shlex.quote(self.host.remote_dir)}"
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
            dest = f"{self.host.user}@{self.host.hostname}:{self.host.remote_dir}/"
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
        if self.host.shell == "powershell":
            rm = (
                f"if (Test-Path -LiteralPath '{self.host.remote_dir}') "
                f"{{ Remove-Item -Recurse -Force -LiteralPath "
                f"'{self.host.remote_dir}' }}"
            )
        else:
            rm = f"rm -rf {shlex.quote(self.host.remote_dir)}"
        # Best-effort; cleanup failure is logged but not fatal.
        _guarded_run(
            self._remote_shell_invoke(rm),
            capture_output=True,
            text=True,
            timeout=60,
        )


def _file_sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class DockerTransport(Transport):
    """Spin up a container per ``run`` invocation, mounting a stage dir."""

    def __init__(self, host: Host) -> None:
        super().__init__(host)
        self._stage: Path | None = None

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

    def run(self, remote_argv: list[str], *, timeout: int) -> tuple[int, str, str]:
        assert self._stage is not None
        # Translate /<remote_dir>/<file> into the mount path, which we
        # always make remote_dir for symmetry with ssh hosts.
        mount = self.host.remote_dir
        cmd = [
            "docker",
            "run",
            "--rm",
            "--pull=never",
            "-v",
            f"{self._stage}:{mount}",
            "-w",
            mount,
            self.host.container,
        ] + remote_argv
        proc = _guarded_run(cmd, capture_output=True, text=True, timeout=timeout)
        return proc.returncode, proc.stdout, proc.stderr

    def import_sealed_root(
        self, archive: Path, *, sha256: str, arch: str, evidence: Path
    ) -> dict[str, object]:
        """Import only the explicit sealed root; no image/base/network lookup."""
        binary = shutil.which("docker")
        if not binary:
            raise RuntimeError(
                "sealed guest requires an already installed Docker provider"
            )
        self._docker = str(Path(binary).resolve(strict=True))
        self._docker_sha256 = _file_sha256(Path(self._docker))
        self._sealed_evidence = evidence
        evidence.mkdir(parents=True, exist_ok=False)
        self._sealed_commands = []
        # Select the local Linux engine explicitly. Context files, credential
        # helpers and DOCKER_* selectors are development ambient inputs.
        self._docker_prefix = [
            self._docker,
            "--host",
            "unix:///var/run/docker.sock",
            "--config",
            str(evidence / "client-config"),
        ]
        (evidence / "client-config").mkdir()
        self._docker_env = {
            "PATH": "/absent",
            "HOME": "/absent",
            "LANG": "C",
            "LC_ALL": "C",
        }
        self._sealed_provider = self._sealed_provider_identity()
        validate_sealed_docker_provider(self._sealed_provider, arch=arch)
        if arch not in {"x86_64", "aarch64"}:
            raise ValueError("sealed Docker guest supports Linux x86_64/aarch64")
        docker_arch = {"x86_64": "amd64", "aarch64": "arm64"}[arch]
        if _file_sha256(archive) != sha256:
            raise ValueError("sealed root archive identity changed before import")
        imported = self._sealed_command(
            ["image", "import", "--platform", f"linux/{docker_arch}", str(archive)],
            timeout=120,
        )
        image_id = imported.stdout.strip()
        if not re.fullmatch(r"sha256:[0-9a-f]{64}", image_id):
            raise ValueError("Docker import did not return an exact image identity")
        self._sealed_image = image_id
        inspected = self._sealed_inspect("image", image_id)
        validate_sealed_docker_image(inspected, sha256=sha256, arch=arch)
        if _file_sha256(archive) != sha256:
            raise ValueError("sealed root archive changed during Docker import")
        return {
            "archive_sha256": sha256,
            "image": inspected,
            "docker": {
                "path": self._docker,
                "sha256": self._docker_sha256,
                "endpoint": "unix:///var/run/docker.sock",
                "provider": self._sealed_provider,
            },
        }

    def _sealed_command(
        self, args: list[str], *, timeout: int, capture: Path | None = None
    ):
        if _file_sha256(Path(self._docker)) != self._docker_sha256:
            raise ValueError("Docker provider changed during sealed execution")
        command = [*self._docker_prefix, *args]
        record = {"argv": command, "timeout_seconds": timeout}
        self._sealed_commands.append(record)
        log = self._sealed_evidence / "commands.json"
        log.write_text(
            json.dumps(self._sealed_commands, indent=2) + "\n", encoding="utf-8"
        )
        capture_paths = (
            []
            if capture is None
            else [capture.with_suffix(".stdout"), capture.with_suffix(".stderr")]
        )

        def capture_exceeded() -> bool:
            return any(
                path.exists() and path.stat().st_size > 16 * 1024 * 1024
                for path in capture_paths
            )

        result = _guarded_run(
            command,
            timeout=timeout,
            env=self._docker_env,
            cancellation_requested=capture_exceeded if capture_paths else None,
            **(
                {
                    "stdout_capture_path": capture.with_suffix(".stdout"),
                    "stderr_capture_path": capture.with_suffix(".stderr"),
                    "capture_tail_bytes": 4096,
                }
                if capture is not None
                else {}
            ),
        )
        record.update(
            returncode=result.returncode, stdout=result.stdout, stderr=result.stderr
        )
        log.write_text(
            json.dumps(self._sealed_commands, indent=2) + "\n", encoding="utf-8"
        )
        if result.returncode:
            raise RuntimeError(
                f"sealed Docker operation failed ({result.returncode}): {command!r}: {result.stderr}"
            )
        if capture_exceeded():
            raise ValueError("sealed Docker capture exceeded 16 MiB per stream")
        return result

    def _sealed_provider_identity(self) -> dict:
        version = json.loads(
            self._sealed_command(
                ["version", "--format", "{{json .Server}}"], timeout=30
            ).stdout
        )
        # Select only execution fields: full docker info can contain proxies.
        fields = (
            "OSType",
            "Architecture",
            "KernelVersion",
            "DefaultRuntime",
            "Runtimes",
            "SecurityOptions",
            "Driver",
        )
        template = (
            "{"
            + ",".join('"' + key + '":{{json .' + key + "}}" for key in fields)
            + "}"
        )
        info = json.loads(
            self._sealed_command(["info", "--format", template], timeout=30).stdout
        )
        return {"version": version, "info": info}

    def _sealed_inspect(self, kind: str, identifier: str) -> dict:
        value = json.loads(
            self._sealed_command([kind, "inspect", identifier], timeout=30).stdout
        )
        if (
            not isinstance(value, list)
            or len(value) != 1
            or not isinstance(value[0], dict)
        ):
            raise ValueError("Docker inspect did not return exactly one object")
        return value[0]

    def run_sealed(self, command: list[str], *, timeout: int) -> dict[str, object]:
        """Inspect before entry, capture output, prove stopped, then remove ours.

        Both writable destinations are bounded tmpfs. Evidence must leave through
        the canonical supervisor export stream before the container stops.
        """
        nonce = secrets.token_hex(16)
        name = "molt-release-" + nonce
        label = "org.molt.release-custody=" + nonce
        argv = [
            "container",
            "create",
            "--pull=never",
            "--name",
            name,
            "--label",
            label,
            "--hostname",
            "molt-guest",
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--cgroupns",
            "private",
            "--ipc",
            "private",
            "--runtime",
            "runc",
            "--shm-size",
            "16m",
            "--log-driver",
            "none",
            "--dns",
            "127.0.0.1",
            "--dns-search",
            ".",
            "--dns-option",
            "ndots:0",
            "--user",
            "65534:65534",
            "--memory",
            "1g",
            "--memory-swap",
            "1g",
            "--cpus",
            "1",
            "--pids-limit",
            "64",
            "--tmpfs",
            "/tmp:rw,noexec,nosuid,nodev,size=64m,mode=1777",
            "--tmpfs",
            "/evidence:rw,noexec,nosuid,nodev,size=16m,mode=1777",
            "--workdir",
            "/app",
            "--env",
            "PATH=/absent",
            "--env",
            "HOME=/absent",
            "--env",
            "LANG=C",
            "--env",
            "LC_ALL=C",
            "--entrypoint",
            command[0],
            self._sealed_image,
            *command[1:],
        ]
        created = self._sealed_command(argv, timeout=30)
        identifier = created.stdout.strip()
        if not re.fullmatch(r"[0-9a-f]{64}", identifier):
            raise ValueError("Docker create returned no exact container identity")
        before = self._sealed_inspect("container", identifier)
        # Ownership is established before any cleanup; arbitrary IDs never grant it.
        if (
            before.get("Id") != identifier
            or before.get("Config", {})
            .get("Labels", {})
            .get("org.molt.release-custody")
            != nonce
        ):
            raise ValueError("Docker container identity differs from the created owner")
        result = None
        try:
            validate_sealed_docker_configuration(
                before, image=self._sealed_image, command=command
            )
            result = self._sealed_command(
                ["container", "start", "--attach", identifier],
                timeout=timeout,
                capture=self._sealed_evidence / nonce,
            )
        finally:
            after = self._sealed_inspect("container", identifier)
            if (
                after.get("Id") != identifier
                or after.get("Config", {})
                .get("Labels", {})
                .get("org.molt.release-custody")
                != nonce
            ):
                raise ValueError(
                    "Docker cleanup lost exact container custody; retained"
                )
            if after.get("State", {}).get("Running") is True:
                self._sealed_command(["container", "kill", identifier], timeout=30)
                after = self._sealed_inspect("container", identifier)
            if (
                after.get("State", {}).get("Running") is not False
                or after.get("State", {}).get("Pid") != 0
            ):
                raise ValueError("Docker container did not close; retained")
            self._sealed_command(["container", "rm", identifier], timeout=30)
        if (
            result is None
            or after["State"].get("ExitCode") != 0
            or after["State"].get("OOMKilled") is not False
        ):
            raise ValueError("sealed guest did not finish successfully")
        return {
            "before": before,
            "after": after,
            "stdout_path": str(self._sealed_evidence / (nonce + ".stdout")),
            "stderr_path": str(self._sealed_evidence / (nonce + ".stderr")),
        }

    def remove_sealed_root(self) -> None:
        # The content ID returned by our import is the only image removable here.
        if image := getattr(self, "_sealed_image", None):
            self._sealed_command(["image", "rm", image], timeout=30)
            self._sealed_image = None
            if self._sealed_provider_identity() != self._sealed_provider:
                raise ValueError(
                    "Docker provider identity changed during sealed replay"
                )

    def cleanup(self) -> None:
        if self._stage and self._stage.exists():
            shutil.rmtree(self._stage, ignore_errors=True)


def validate_sealed_docker_provider(value: dict, *, arch: str) -> None:
    """Admit the concrete local Linux engine capability, not a CLI hash alone.

    Engine/kernel are trusted execution providers, captured for qualification.
    Guest executable and library bytes are separately pinned from source.
    """
    if not isinstance(value, dict) or set(value) != {"version", "info"}:
        raise ValueError("sealed Docker provider identity is invalid")
    version, info = value["version"], value["info"]
    api = str(version.get("ApiVersion", "")).split(".")
    compatible = (
        len(api) == 2
        and all(part.isdecimal() for part in api)
        and tuple(map(int, api)) >= (1, 41)
    )
    machine = {"x86_64": ("amd64", "x86_64"), "aarch64": ("arm64", "aarch64")}.get(arch)
    if (
        not compatible
        or machine is None
        or version.get("Os") != "linux"
        or version.get("Arch") != machine[0]
        or info.get("OSType") != "linux"
        or info.get("Architecture") != machine[1]
        or not version.get("Version")
        or not version.get("GitCommit")
        or not info.get("KernelVersion")
        or info["KernelVersion"] != version.get("KernelVersion")
        or "runc" not in info.get("Runtimes", {})
        or not any(
            str(item).startswith("name=seccomp,")
            for item in info.get("SecurityOptions", [])
        )
    ):
        raise ValueError(
            "sealed Docker provider lacks the declared Linux/runc/seccomp capability"
        )


def validate_sealed_docker_image(value: dict, *, sha256: str, arch: str) -> None:
    """Moby import's sole uncompressed layer digest must be the sealed tar."""
    docker_arch = {"x86_64": "amd64", "aarch64": "arm64"}.get(arch)
    if (
        not docker_arch
        or not isinstance(value, dict)
        or not re.fullmatch(r"sha256:[0-9a-f]{64}", str(value.get("Id")))
        or value.get("Os") != "linux"
        or value.get("Architecture") != docker_arch
        or value.get("RootFS") != {"Type": "layers", "Layers": ["sha256:" + sha256]}
        or any(
            value.get("Config", {}).get(key)
            for key in (
                "Env",
                "Cmd",
                "Entrypoint",
                "Volumes",
                "WorkingDir",
                "User",
                "Healthcheck",
                "OnBuild",
            )
        )
    ):
        raise ValueError(
            "sealed image does not name the exact root/platform/configuration"
        )


def validate_sealed_docker_configuration(
    value: dict, *, image: str, command: list[str]
) -> None:
    """The receiver and the pre-entry check share one effective launch contract."""
    host, config = value.get("HostConfig", {}), value.get("Config", {})
    expected_env = {"PATH=/absent", "HOME=/absent", "LANG=C", "LC_ALL=C"}
    expected_tmpfs = {
        "/tmp": "rw,noexec,nosuid,nodev,size=64m,mode=1777",
        "/evidence": "rw,noexec,nosuid,nodev,size=16m,mode=1777",
    }
    if (
        value.get("Image") != image
        or value.get("Path") != command[0]
        or value.get("Args") != command[1:]
        or config.get("Entrypoint") != [command[0]]
        or config.get("Cmd") != command[1:]
        or config.get("WorkingDir") != "/app"
        or config.get("User") != "65534:65534"
        or set(config.get("Env", [])) != expected_env
        or config.get("Hostname") != "molt-guest"
        or config.get("Volumes")
        or config.get("Healthcheck")
        or config.get("Tty") is not False
        or config.get("OpenStdin") is not False
        or host.get("NetworkMode") != "none"
        or host.get("ReadonlyRootfs") is not True
        or host.get("Privileged") is not False
        or host.get("CapAdd")
        or host.get("CapDrop") != ["ALL"]
        or host.get("SecurityOpt") != ["no-new-privileges"]
        or host.get("PidMode") != ""
        or host.get("IpcMode") != "private"
        or host.get("UTSMode") != ""
        or host.get("UsernsMode") not in ("", "private")
        or host.get("CgroupnsMode") != "private"
        or host.get("Runtime") != "runc"
        or host.get("ShmSize") != 16777216
        or host.get("LogConfig") != {"Type": "none", "Config": {}}
        or host.get("Dns") != ["127.0.0.1"]
        or host.get("DnsSearch") != ["."]
        or host.get("DnsOptions") != ["ndots:0"]
        or host.get("Binds")
        or host.get("Devices")
        or host.get("VolumesFrom")
        or host.get("DeviceRequests")
        or host.get("DeviceCgroupRules")
        or host.get("PortBindings")
        or host.get("PublishAllPorts")
        or host.get("Tmpfs") != expected_tmpfs
        or host.get("Memory") != 1073741824
        or host.get("MemorySwap") != 1073741824
        or host.get("PidsLimit") != 64
        or host.get("NanoCpus") != 1000000000
        or host.get("AutoRemove") is not False
        or host.get("RestartPolicy", {}).get("Name") != "no"
        or value.get("State", {}).get("Running") is not False
    ):
        raise ValueError(
            "sealed Docker effective configuration violates guest isolation"
        )
    mounts = value.get("Mounts")
    if not isinstance(mounts, list) or any(
        row.get("Type") != "tmpfs" or row.get("Destination") not in expected_tmpfs
        for row in mounts
    ):
        raise ValueError("sealed Docker root exposes an unadmitted host mount")
    # Standard private proc/dev/sys mounts and Docker's three generated /etc
    # files are explicit engine inputs, not members of the imported layer.
    # No provider may relax the proc/sys write boundary or add host mounts.
    readonly = host.get("ReadonlyPaths", [])
    masked = host.get("MaskedPaths", [])
    if not {
        "/proc/bus",
        "/proc/fs",
        "/proc/irq",
        "/proc/sys",
        "/proc/sysrq-trigger",
    } <= set(readonly) or not {"/proc/kcore", "/sys/firmware"} <= set(masked):
        raise ValueError("sealed Docker provider lacks protected proc/sys mounts")
    networks = value.get("NetworkSettings", {}).get("Networks", {})
    if set(networks) - {"none"}:
        raise ValueError("sealed Docker root has an external network")


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
    try:
        transport.prepare()
    except Exception as exc:
        result.setup_error = f"transport.prepare failed: {exc}"
        for c in cases:
            result.cases.append(
                CaseResult(
                    case=c.name,
                    status="SKIP",
                    duration=0.0,
                    detail=result.setup_error,
                )
            )
        return result

    # Push every binary that compiled successfully.
    push_files: list[Path] = []
    case_to_remote: dict[str, str] = {}
    for c in cases:
        binary, _ = builds[(host.target, c.name)]
        if binary is None:
            continue
        # Force a stable remote name regardless of local extension.
        ext = binary.suffix
        remote_name = f"{c.name}{ext}"
        # Stage with the remote_name so push lands the right basename.
        staged = binary.parent / remote_name
        if staged != binary:
            shutil.copy2(binary, staged)
            os.chmod(staged, 0o755)
        push_files.append(staged)
        case_to_remote[c.name] = remote_name

    try:
        transport.push(push_files)
    except Exception as exc:
        result.setup_error = f"transport.push failed: {exc}"
        for c in cases:
            result.cases.append(
                CaseResult(
                    case=c.name,
                    status="SKIP",
                    duration=0.0,
                    detail=result.setup_error,
                )
            )
        if not keep_remote:
            transport.cleanup()
        return result

    try:
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
            remote_path = host.remote_path(remote_name)
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
    finally:
        if not keep_remote:
            transport.cleanup()
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
        if r.setup_error:
            if not any_detail:
                lines.append("## Diagnostics")
                lines.append("")
                any_detail = True
            lines.append(f"### {r.host.name} — setup error")
            lines.append("")
            lines.append("```")
            lines.append(r.setup_error)
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
        help="leave the remote_dir in place after the run (debugging)",
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
    any_setup_err = any(r.setup_error for r in results)
    return 1 if (any_fail or any_setup_err) else 0


if __name__ == "__main__":
    sys.exit(main())
