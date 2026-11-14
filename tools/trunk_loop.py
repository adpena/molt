#!/usr/bin/env python3
"""Run exact-source proof-plan commands on an inventoried farm host.

Stage 1: explicit run/collect; no scheduler, ledger mutation or automatic revert.
See docs/design/trunk_loop.md. The controller and tested commit are independent
identities: an older main is never modified to install this runner.
"""

from __future__ import annotations

import argparse
import atexit
import contextlib
from contextlib import chdir
from dataclasses import asdict, dataclass, fields
from datetime import UTC, datetime
import hashlib
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import shlex
import subprocess
import sys
import time
import traceback
from collections.abc import Mapping
from enum import StrEnum
from types import MappingProxyType
from typing import TYPE_CHECKING, Final, NoReturn
import zipfile

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.proof_receipts import SourceTreeState  # noqa: E402

if TYPE_CHECKING:
    from tools import cross_run

SCHEMA = "molt.trunk-run.v3"


class RunState(StrEnum):
    """A remote run's lifecycle in run.json (docs/design/state_machines.md)."""

    PREPARING = "preparing"  # record written; stage and checkouts being prepared
    LAUNCHED = "launched"  # the launcher acknowledged a worker
    COLLECTED = "collected"  # terminal receipt checked and evidence retained
    VANISHED = "vanished"  # the worker is gone and published no receipt
    ABANDONED = "abandoned"  # start failed before any launch was attempted


# PREPARING -> COLLECTED: the launch acknowledgement was lost, yet the worker
# ran and published its terminal receipt. Terminal states have no exits.
RUN_TRANSITIONS: Final[Mapping[RunState, frozenset[RunState]]] = MappingProxyType(
    {
        RunState.PREPARING: frozenset(
            {RunState.LAUNCHED, RunState.ABANDONED, RunState.COLLECTED}
        ),
        RunState.LAUNCHED: frozenset({RunState.COLLECTED, RunState.VANISHED}),
        RunState.COLLECTED: frozenset(),
        RunState.VANISHED: frozenset(),
        RunState.ABANDONED: frozenset(),
    }
)


class TerminalStatus(StrEnum):
    """How a run or a remote landing closed: the worker's terminal.json and the
    landing receipt.json share this vocabulary."""

    SUCCESS = "success"
    FAILURE = "failure"  # a selected proof failed for its own reason
    INFRASTRUCTURE = "infrastructure"  # the run stopped before a verdict


class InvalidRunTransition(RuntimeError):
    """A run record was asked to take an edge its lifecycle does not have."""


def transition(record: dict, target: RunState, **fields: object) -> None:
    """Move ``record`` to ``target`` with ``fields``, or refuse the edge."""
    current = RunState(record["state"])
    if target not in RUN_TRANSITIONS[current]:
        raise InvalidRunTransition(
            f"{record.get('run_id')}: {current} -> {target} is not a run transition"
        )
    record.update(state=target.value, **fields)


def failure_text(exc: BaseException) -> str:
    return f"{type(exc).__name__}: {exc}"


def authorities():
    from tools import cross_run

    return cross_run


def write_json(path: Path, payload: object) -> None:
    from molt.exact_json import write_exact

    write_exact(path, payload)


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def ps(value: str) -> str:
    if "\0" in value:
        raise ValueError("PowerShell argument contains NUL")
    return "'" + value.replace("'", "''") + "'"


def uv_python(uv: str, *argv: str) -> list[str]:
    return [uv, "run", "--locked", "python", "-B", *argv]


def bootstrap_uv(custody_root: str, *, windows: bool) -> str:
    """The farm host's pinned uv: the official installer's binary, alone in a
    directory named by its version under the host's custody root.

    Never the user-global ``~/.local/bin/uv``. Other projects on a farm host run
    that binary, and Molt must not replace their tool; on Windows a running
    ``uv.exe`` cannot be replaced at all (bat00, 2026-11-04). A new pin installs
    beside the old one, so provisioning never replaces a running uv. Farm
    commands name this path; a CIM-created Windows worker inherits the user's
    registry PATH, not the SSH session's, so a bare ``uv`` would select the
    user's binary.
    """
    from molt.tool_releases import tool_release

    version = tool_release("uv", _REPO_ROOT).version
    path_type = PureWindowsPath if windows else PurePosixPath
    executable = "uv.exe" if windows else "uv"
    return str(path_type(custody_root) / "bootstrap" / "uv" / version / executable)


def host_uv(host) -> str:
    return bootstrap_uv(host.custody_root, windows=host.is_windows)


def require_pinned_uv(path: str, pinned: str, host_name: str) -> None:
    """Refuse before testing unless ``path`` selects the host's pinned uv:
    proof commands run ``uv`` by name."""
    import shutil

    resolved = shutil.which("uv", path=path)
    if resolved is None or os.path.normcase(
        os.path.abspath(resolved)
    ) != os.path.normcase(pinned):
        raise RuntimeError(
            f"PATH selects uv {resolved!r}, not the pinned {pinned}; run: "
            f"tools/trunk_loop.py provision --host {host_name} --commit <commit>"
        )


# The apt packages the documented Linux workflow needs (docs/getting-started.md).
# No farm cell declares system CMake (proof-plan toolchains): Cargo builds take
# AWS-LC's cc builder (molt.cargo_execution_policy refuses its CMake builder), and
# only LLVM source bootstrap uses system CMake, checking at the point of use.
LINUX_SYSTEM_PACKAGES = (
    "git",
    "curl",
    "clang",
    "lld",
    "build-essential",
    "valgrind",
)
# A WSL gate guest also runs systemd and udev for root UUID admission (HF-761).
WSL_GUEST_PACKAGES = (*LINUX_SYSTEM_PACKAGES, "systemd", "systemd-sysv", "udev")


def provision_script(host, *, repository: str, commit: str) -> str:
    """The documented local workflow at ``commit``, with report-only system
    prerequisites.

    Official user installers own their binaries; uv owns Python and the lock.
    Never import Molt before uv has resolved the project's dependencies. ``uv``
    The official installer places the pinned uv at ``host_uv(host)``, never in
    the user's own install directory.
    Doctor and ``uv sync`` run at ``commit``: a fetch alone leaves the checkout
    at whatever commit it held, and doctor then judges stale code (macmini,
    2026-11-04).
    """
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        raise ValueError("provision needs a full commit id")
    from molt.tool_releases import tool_release

    uv = host_uv(host)
    uv_version = tool_release("uv", _REPO_ROOT).version
    checkout = host.checkout
    if host.is_windows and not (host.target or "").startswith("x86_64-"):
        # The vswhere query below names the x64 MSVC component; an ARM64 host
        # needs config/llvm_toolchain_arches.toml's ARM64 component.
        raise ValueError(f"{host.name}: Windows farm hosts are x86_64 only")
    if host.is_windows:
        return f"""$ErrorActionPreference='Stop'
$env:UV_PROJECT_ENVIRONMENT=$null
$env:VIRTUAL_ENV=$null
$env:MOLT_VENV=$null
# The public repository needs no credentials: a prompt fails instead of hanging
# (bat00, 2026-11-04: a fetch through the user's SSH key waited forever).
$env:GIT_TERMINAL_PROMPT='0'
$env:GIT_SSH_COMMAND='ssh -o BatchMode=yes'
$env:PATH={ps(str(PureWindowsPath(uv).parent) + ";")} + "$env:USERPROFILE\\.cargo\\bin;$env:PATH"
$uv={ps(uv)}
if (-not (Get-Command git -ErrorAction SilentlyContinue)) {{ throw 'Missing system prerequisite git; run: winget install --id Git.Git' }}
# Builds select MSVC through vswhere and build their own environment
# (molt.platform_toolchain.visual_studio_installation); no developer shell is
# needed. This mirrors that query because Molt is not importable before uv sync.
$vswhere=Join-Path ${{env:ProgramFiles(x86)}} 'Microsoft Visual Studio\\Installer\\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere) -or @(& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Count -ne 1) {{
 throw 'Missing MSVC Build Tools; run: winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"'
}}
# The pinned uv in its own versioned directory (the toolchain contract pins it).
# UV_UNMANAGED_INSTALL: that directory only, no PATH edit, no self-updater.
if (-not (Test-Path -LiteralPath $uv) -or ((& $uv --version) -split ' ')[1] -ne {ps(uv_version)}) {{
 $env:UV_UNMANAGED_INSTALL={ps(str(PureWindowsPath(uv).parent))}
 Invoke-RestMethod https://astral.sh/uv/{uv_version}/install.ps1 | Invoke-Expression
 Remove-Item Env:UV_UNMANAGED_INSTALL
 if (-not (Test-Path -LiteralPath $uv) -or ((& $uv --version) -split ' ')[1] -ne {ps(uv_version)}) {{ throw {ps("uv: the official installer did not place the pinned " + uv_version + " at " + uv)} }}
}}
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {{
 $installer=Join-Path $env:TEMP ('molt-rustup-'+[guid]::NewGuid().ToString('N')+'.exe')
 try {{
  Invoke-WebRequest https://win.rustup.rs/x86_64 -OutFile $installer
  & $installer -y --no-modify-path --default-toolchain none
  $installExit=$LASTEXITCODE
 }} finally {{
  if (Test-Path -LiteralPath $installer) {{ Remove-Item -LiteralPath $installer }}
 }}
 if ($installExit) {{ exit $installExit }}
}}
if (-not (Test-Path -LiteralPath {ps(checkout)})) {{
 New-Item -ItemType Directory -Force -Path {ps(host.custody_root)} | Out-Null
 & git clone -- {ps(repository)} {ps(checkout)}
 if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
}} else {{
 & git -C {ps(checkout)} fetch --no-tags -- {ps(repository)} {commit}
 if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
}}
if (& git -C {ps(checkout)} status --porcelain --untracked-files=no) {{ throw {ps(checkout + " has tracked edits; provision never discards them")} }}
& git -C {ps(checkout)} checkout --quiet --detach {commit}
if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
Set-Location -LiteralPath {ps(checkout)}
& rustup show active-toolchain
if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
& $uv sync --locked --group dev
if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
& $uv run --locked molt doctor --json --strict
if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
"""
    macos = host.os == authorities().HostOS.MACOS
    # Each missing tool names its own fix, so the message never asks for an
    # install the host already has.
    fixes = (
        {"cc": "xcode-select --install", "git": "xcode-select --install"}
        if macos
        else {
            tool: "sudo apt-get update && sudo apt-get install "
            + " ".join(LINUX_SYSTEM_PACKAGES)
            for tool in ("git", "curl", "clang")
        }
    )
    checks = "\n".join(
        (
            # /usr/bin/cc and /usr/bin/git are shims that exist without the
            # Command Line Tools; xcode-select -p is the real presence test.
            "xcode-select -p >/dev/null 2>&1 || missing_fix "
            if macos and tool in ("cc", "git")
            else f"command -v {tool} >/dev/null || missing_fix "
        )
        + shlex.quote(f"{tool}: run {fix}")
        for tool, fix in fixes.items()
    )
    # The owner approves package installation per host (its inventory record).
    return f"""set -eu
unset UV_PROJECT_ENVIRONMENT VIRTUAL_ENV MOLT_VENV
# The public repository needs no credentials: a prompt fails instead of hanging
# (bat00, 2026-11-04: a fetch through the user's SSH key waited forever).
export GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND='ssh -o BatchMode=yes'
{posix_path_head(host)}
uv={shlex.quote(uv)}
missing=""
missing_fix() {{ missing="$missing
 $1"; }}
{checks}
if test -n "$missing"; then
 printf 'Missing system prerequisites:%s\n' "$missing" >&2
 exit 2
fi
# The pinned uv in its own versioned directory: an older uv cannot install the
# pinned Python patch release ("No interpreter found"). UV_UNMANAGED_INSTALL:
# that directory only, no PATH or profile edit, no self-updater.
if ! test -x "$uv" || test "$("$uv" --version | cut -d' ' -f2)" != {shlex.quote(uv_version)}; then
 installer=$(mktemp)
 trap 'rm -f "$installer"' EXIT
 curl -LsSf https://astral.sh/uv/{uv_version}/install.sh -o "$installer"
 UV_UNMANAGED_INSTALL={shlex.quote(str(PurePosixPath(uv).parent))} sh "$installer"
 rm "$installer"
 test -x "$uv" && test "$("$uv" --version | cut -d' ' -f2)" = {shlex.quote(uv_version)} || {{ echo {shlex.quote("uv: the official installer did not place the pinned " + uv_version + " at " + uv)} >&2; exit 2; }}
fi
if ! command -v rustup >/dev/null; then
 installer=$(mktemp)
 trap 'rm -f "$installer"' EXIT
 curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$installer"
 sh "$installer" -y --no-modify-path --default-toolchain none
 rm "$installer"
fi
mkdir -p {shlex.quote(host.custody_root)}
if test -e {shlex.quote(checkout)}; then
 git -C {shlex.quote(checkout)} fetch --no-tags -- {shlex.quote(repository)} {commit}
else
 git clone -- {shlex.quote(repository)} {shlex.quote(checkout)}
fi
test -z "$(git -C {shlex.quote(checkout)} status --porcelain --untracked-files=no)" || {{ echo {shlex.quote(checkout + " has tracked edits; provision never discards them")} >&2; exit 2; }}
git -C {shlex.quote(checkout)} checkout --quiet --detach {commit}
cd {shlex.quote(checkout)}
rustup show active-toolchain
"$uv" sync --locked --group dev
"$uv" run --locked molt doctor --json --strict
"""


def wsl_profile_script(host) -> str:
    """Read registered backing-disk identity and the Windows VM profile."""
    return f"""$ErrorActionPreference='Stop'
$key=Get-ChildItem HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Lxss | Where-Object {{ (Get-ItemProperty $_.PSPath).DistributionName -eq {ps(host.wsl_distribution)} }}
if (@($key).Count -ne 1) {{ throw 'WSL distribution identity is ambiguous' }}
$base=(Get-ItemProperty $key.PSPath).BasePath
$actual=[IO.Path]::Combine($base,'ext4.vhdx')
if ($actual -cne {ps(host.wsl_vhdx)}) {{ throw "WSL backing disk changed: $actual" }}
$config=[IO.File]::ReadAllText((Join-Path $env:USERPROFILE '.wslconfig'))
$pending=Join-Path $base '.molt-disk-maintenance'
if ((Test-Path -LiteralPath $pending) -and ([IO.File]::ReadAllText($pending) -cne 'molt.wsl-disk-maintenance.v1')) {{ throw 'Unknown WSL disk maintenance state' }}
[pscustomobject]@{{config=$config;vhdx=$actual;pending=(Test-Path -LiteralPath $pending)}} | ConvertTo-Json -Compress
"""


def admit_wsl_profile(host, transport, *, provision_disk: bool = False) -> dict:
    profile = json.loads(transport.windows_command(wsl_profile_script(host)))
    # A shrunk filesystem can still inhabit an oversized VHD. Query the root
    # block device, independently of both filesystem size and physical free space.
    profile["disk_bytes"] = int(
        transport.shell_command(
            'set -eu\nroot_device=$(findmnt -nro SOURCE /)\nlsblk -bdno SIZE "$root_device"'
        )
    )
    import configparser

    config = configparser.ConfigParser()
    config.read_string(profile["config"])
    expected = {
        "memory": f"{host.wsl_memory_gb}GB",
        "swap": f"{host.wsl_swap_gb}GB",
        "vmIdleTimeout": "-1",
    }
    changes = [
        f"{k}={v}"
        for k, v in expected.items()
        if config.get("wsl2", k, fallback=None) != v
    ]
    if changes:
        raise ValueError(
            "Windows settings change required; return to integrator before applying host profile: "
            + "; ".join(changes)
            + f" (observed disk bytes={profile['disk_bytes']}, budget GiB={host.wsl_disk_gb})"
        )
    # df's used blocks include the host-owned Cargo/uv caches on this root
    # filesystem. Reserve a whole proof build before shrinking or installing.
    used = int(transport.shell_command("df -Pk / | tail -1 | awk '{print $3}'")) * 1024
    budget = host.wsl_disk_gb * 1024**3
    profile["used_bytes"] = used
    if used + host.proof_build_gb * 1024**3 > budget:
        raise ValueError(
            f"{host.name}: {used} used bytes plus {host.proof_build_gb} GiB proof headroom "
            f"exceeds {host.wsl_disk_gb} GiB disk budget"
        )
    if profile["disk_bytes"] != budget or profile["pending"]:
        if not provision_disk:
            raise ValueError(
                f"{host.name}: disk cap differs; run trunk_loop.py provision --host {host.name}"
            )
        # The reviewed WSL engine shrinks ext4 before its VHDX (see the owning
        # design). Refuse unknown implementations rather than probing by mutation.
        reviewed = str(host.wsl_engine_version)
        version = transport.windows_command("wsl.exe --version").replace("\0", "")
        if not re.fullmatch(
            rf".*:\s*{re.escape(reviewed)}(?:\.0)?\s*",
            version.splitlines()[0] if version else "",
        ):
            raise ValueError(
                f"disk provisioning requires the reviewed WSL {reviewed} implementation "
                f"(tools/cross_hosts.toml wsl_engine_version for {host.name})"
            )
        name = ps(host.wsl_distribution)
        pending = ps(
            str(PureWindowsPath(host.wsl_vhdx).parent / ".molt-disk-maintenance")
        )
        stopped = (
            f"& wsl.exe --terminate {name}\nif ($LASTEXITCODE) {{ exit $LASTEXITCODE }}\n"
            "$running=@(& wsl.exe --list --running --quiet)\n"
            "if ($LASTEXITCODE) { exit $LASTEXITCODE }\n"
            "$running=@($running | ForEach-Object { ($_.Replace([string][char]0,'')).Trim() } | Where-Object { $_ })\n"
            "if ($running.Count) { throw ('Other WSL distributions are running; cannot release the VM for disk maintenance: '+($running -join ', ')) }\n"
            # vmIdleTimeout=-1 retains the VM's internal disk attachment even
            # after termination. Release only an idle VM with no running distro.
            "& wsl.exe --shutdown\nif ($LASTEXITCODE) { exit $LASTEXITCODE }"
        )
        # Do not interrupt an existing farm proof. Only this distro is stopped;
        # An idle VM release is refused if another distribution is running.
        transport.shell_command(
            f"set -eu\nlock={shlex.quote(host.remote_dir + '/trunk-worker.lock')}\n"
            'if test -f "$lock"; then flock -n "$lock" true; fi'
        )
        if profile["disk_bytes"] != budget:
            transport.windows_command(
                f"$ErrorActionPreference='Stop'\n"
                f"[IO.File]::WriteAllText({pending},'molt.wsl-disk-maintenance.v1')\n"
                f"{stopped}\n"
                f"& wsl.exe --manage {name} --resize {host.wsl_disk_gb}GB\n"
                "if ($LASTEXITCODE) { exit $LASTEXITCODE }",
                timeout=900,
            )
        transport.windows_command(
            f"$ErrorActionPreference='Stop'\n& wsl.exe -d {name} --user root -- fstrim -av\n"
            "if ($LASTEXITCODE) { exit $LASTEXITCODE }\n"
            f"{stopped}\n"
            f"& wsl.exe --manage {name} --compact\nif ($LASTEXITCODE) {{ exit $LASTEXITCODE }}\n"
            f"Remove-Item -LiteralPath {pending} -ErrorAction Stop",
            timeout=900,
        )
        return admit_wsl_profile(host, transport)
    return profile


def wsl_packages_script() -> str:
    """Install only the missing declared guest packages; report what it installed."""
    packages = " ".join(shlex.quote(package) for package in WSL_GUEST_PACKAGES)
    return f"""set -eu
missing=""
for package in {packages}; do
 dpkg-query -W -f='${{Status}}' "$package" 2>/dev/null | grep -q 'install ok installed' || missing="$missing $package"
done
if test -n "$missing"; then
 export DEBIAN_FRONTEND=noninteractive
 apt-get update >&2
 apt-get install -y --no-install-recommends $missing >&2
fi
printf '{{"installed":"%s"}}\\n' "$(echo $missing)"
"""


def wsl_systemd_script() -> str:
    """Preserve other guest settings; publish only a changed boot declaration."""
    return r"""set -eu
test -x /usr/lib/systemd/systemd && test /sbin/init -ef /usr/lib/systemd/systemd && command -v udevadm >/dev/null || {
 echo 'molt-gate requires the systemd, systemd-sysv and udev system prerequisites' >&2; exit 2;
}
config=/etc/wsl.conf
test ! -L "$config" || { echo 'Refusing symlink /etc/wsl.conf' >&2; exit 2; }
test ! -e "$config" || test -f "$config" || { echo 'Refusing non-file /etc/wsl.conf' >&2; exit 2; }
input=/dev/null
test ! -e "$config" || input="$config"
temp=$(mktemp /etc/.molt-wsl.conf.XXXXXX)
trap 'rm -f "$temp"' EXIT
if test -f "$config"; then cp -p -- "$config" "$temp"; else chmod 644 "$temp"; fi
awk '
 /^[[:space:]]*\[/ {
   if (boot && !setting) { print "systemd=true"; setting=1 }
   boot=(tolower($0) ~ /^[[:space:]]*\[boot\][[:space:]]*([#;].*)?$/)
   if (boot && ++sections > 1) exit 2
 }
 boot && tolower($0) ~ /^[[:space:]]*systemd[[:space:]]*=/ {
   if (++keys > 1) exit 2
   print "systemd=true"; setting=1; next
 }
 { print }
 END {
   if (sections > 1 || keys > 1) exit 2
   if (!sections) print "[boot]"
   if (!setting) print "systemd=true"
 }' "$input" > "$temp" || {
 echo 'Ambiguous [boot] systemd declaration in /etc/wsl.conf; preserved original' >&2; exit 2;
}
changed=false
if ! cmp -s "$temp" "$config"; then
 mv -- "$temp" "$config"
 changed=true
fi
restart=false
if test "$changed" = true || test "$(cat /proc/1/comm)" != systemd; then restart=true; fi
printf '{"changed":%s,"restart":%s}\n' "$changed" "$restart"
"""


def admit_wsl_guest(host, transport) -> dict:
    """Admit the real root UUID namespace, never fabricate a device alias."""
    return json.loads(
        transport.shell_command(
            r"""set -eu
test "$(cat /proc/1/comm)" = systemd || { echo 'molt-gate requires systemd as PID 1; run trunk_loop.py provision --host molt-gate' >&2; exit 2; }
udevadm settle --timeout=30
root_device=$(findmnt -nro SOURCE /)
uuid=$(findmnt -nro UUID /)
mapping=/dev/disk/by-uuid/$uuid
if test -z "$uuid" || ! test -b "$mapping" || ! test "$mapping" -ef "$root_device"; then
 echo "Missing root filesystem UUID mapping: /dev/disk/by-uuid/$uuid for $root_device" >&2; exit 2
fi
printf '{"systemd":true,"root_uuid":"%s"}\n' "$uuid"
"""
        )
    )


def provision_wsl_guest(host, transport) -> dict:
    # As for disk maintenance, never restart an active farm proof.
    transport.shell_command(
        f"set -eu\nlock={shlex.quote(host.remote_dir + '/trunk-worker.lock')}\n"
        'if test -f "$lock"; then flock -n "$lock" true; fi'
    )
    # The owner approves package installation per host (its inventory record);
    # elsewhere system packages stay an operator step.
    installed = (
        json.loads(transport.wsl_root_command(wsl_packages_script(), timeout=1800))[
            "installed"
        ].split()
        if host.install_system_packages
        else []
    )
    state = json.loads(transport.wsl_root_command(wsl_systemd_script()))
    if state["restart"]:
        transport.windows_command(
            f"& wsl.exe --terminate {ps(host.wsl_distribution)}\n"
            "if ($LASTEXITCODE) { exit $LASTEXITCODE }"
        )
    return {
        **admit_wsl_guest(host, transport),
        "packages_installed": installed,
        "config_changed": state["changed"],
        "restarted": state["restart"],
    }


def provision_host(args, *, transport=None) -> dict:
    cross = authorities()
    host = next(
        (h for h in cross.parse_inventory(args.inventory) if h.name == args.host), None
    )
    if host is None or host.transport != "ssh" or not host.checkout:
        raise ValueError("provision requires an inventoried SSH proof host")
    host_paths(host, "provision")
    transport = transport or cross.SSHTransport(host)
    # Report capacity BEFORE downloading a toolchain or cloning the repository.
    if host.is_windows:
        disk = "$d=Get-PSDrive C; [pscustomobject]@{free=$d.Free} | ConvertTo-Json -Compress"
        free = json.loads(transport.shell_command(disk))["free"]
    else:
        # ext4's virtual size is not Windows' available NVMe space.
        path = "/mnt/c" if host.wsl_distribution else host.custody_root
        # An uncreated checkout/custody root still belongs to its existing
        # ancestor's filesystem, which may differ from /.
        probe = (
            f"df -Pk {shlex.quote(path)}"
            if host.wsl_distribution
            else f'p={shlex.quote(path)}; while ! test -e "$p"; do p=$(dirname "$p"); done; df -Pk "$p"'
        )
        free = (
            int(transport.shell_command(probe + " | tail -1 | awk '{print $4}'")) * 1024
        )
    if free < host.proof_build_gb * 1024**3:
        raise ValueError(
            f"{host.name}: {free} bytes free; needs {host.proof_build_gb} GiB proof headroom"
        )
    profile = (
        admit_wsl_profile(host, transport, provision_disk=True)
        if host.wsl_distribution
        else None
    )
    guest = provision_wsl_guest(host, transport) if host.wsl_distribution else None
    repository = checked(["git", "-C", str(_REPO_ROOT), "remote", "get-url", "origin"])
    # The host fetches from origin, so the commit must be published there.
    commit = checked(
        [
            "git",
            "-C",
            str(_REPO_ROOT),
            "rev-parse",
            "--verify",
            args.commit + "^{commit}",
        ]
    )
    if not checked(
        [
            "git",
            "-C",
            str(_REPO_ROOT),
            "for-each-ref",
            "--contains",
            commit,
            "--format=%(refname)",
            "refs/remotes/origin",
        ]
    ):
        raise ValueError(
            f"{commit} is on no origin branch, and {host.name} fetches from origin; "
            "push it (or fetch origin) before provisioning"
        )
    uv = host_uv(host)
    result = transport.shell_command(
        provision_script(host, repository=repository, commit=commit),
        timeout=1800,
    )
    retention = json.loads(
        transport.shell_command(
            remote_argv(
                host,
                uv_python(uv, "-c", RETENTION_STATUS),
                cwd=host.checkout,
            )
        )
    )
    record = {
        "host": host.name,
        "commit": commit,
        "uv": uv,
        "free_bytes": free,
        "proof_build_gb": host.proof_build_gb,
        "wsl": profile,
        "guest": guest,
        "install_log": result,
        "retention": retention,
    }
    print(json.dumps(record), flush=True)
    return record


# Inspect only the existing maintenance owner's bounded epoch directory. No
# sweep is launched merely to report its status; absence is explicitly unknown.
RETENTION_STATUS = r"""
import json
from molt.default_paths import _default_molt_cache
root=_default_molt_cache()
epochs=root/'logs'/'cache_retention'/'automatic'
latest=max(epochs.iterdir(),key=lambda p:p.stat().st_mtime,default=None) if epochs.is_dir() else None
rc=latest/'rc' if latest is not None else None
print(json.dumps({'cache':str(root),'epoch':str(latest) if latest else None,
'returncode':int(rc.read_text().strip()) if rc is not None and rc.is_file() else None}))
"""


def posix_path_head(host) -> str:
    """Every farm shell's PATH: the pinned uv, then rustup's Cargo; never the
    user's ``~/.local/bin``, where another project's uv lives."""
    uv_dir = shlex.quote(str(PurePosixPath(host_uv(host)).parent))
    return f'export PATH={uv_dir}:"$HOME/.cargo/bin:$PATH"'


def remote_argv(host, argv: list[str], *, cwd: str | None = None) -> str:
    if host.is_windows:
        uv_dir = str(PureWindowsPath(host_uv(host)).parent)
        prefix = (
            "$ErrorActionPreference='Stop'; "
            f"$env:PATH={ps(uv_dir + ';')}"
            ' + "$env:USERPROFILE\\.cargo\\bin;$env:PATH"; '
        )
        if cwd:
            prefix += f"Set-Location -LiteralPath {ps(cwd)}; "
        return (
            prefix
            + "& "
            + " ".join(ps(a) for a in argv)
            + "; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }"
        )
    prefix = posix_path_head(host) + "; "
    if cwd is not None:
        prefix += f"cd {shlex.quote(cwd)} && "
    return prefix + shlex.join(argv)


@dataclass(frozen=True)
class ProcessProbe:
    """One observation of the process at a pid, as the remote OS stores it.

    ``started`` is the start stamp the kernel recorded at process creation; it
    is never recomputed from the wall clock, so it cannot drift. On Linux,
    ``instance`` names the PID namespace's instance: the kernel boot id and
    PID 1's start ticks. A restarted WSL distro is a new init under the same VM
    kernel, so its instance differs. ``instance_started_at`` is diagnostic only.
    """

    pid: int
    state: str
    started: str
    instance: str
    instance_started_at: str

    @classmethod
    def parse(cls, raw: str) -> ProcessProbe:
        data = json.loads(raw)
        names = {field.name for field in fields(cls)}
        if (
            not isinstance(data, dict)
            or set(data) != names
            or type(data["pid"]) is not int
            or data["pid"] <= 0
            or any(not isinstance(data[name], str) for name in names - {"pid"})
        ):
            raise ValueError(f"malformed worker identity probe: {raw!r}")
        return cls(
            **{k: v.strip() if isinstance(v, str) else v for k, v in data.items()}
        )

    @property
    def identity(self) -> tuple[int, str, str] | None:
        """A live process's identity; an absent, zombie or dead pid has none."""
        if not self.state or self.state[0] in "ZX":
            return None
        return (self.pid, self.started, self.instance)


def posix_probe(host) -> str:
    """Define ``molt_probe PID``, which prints one ProcessProbe JSON line.

    Every printed value is digits, hex, a state letter or a timestamp, so
    printf needs no JSON escaping. A missing pid prints empty state and start.
    """
    if host.os == authorities().HostOS.MACOS:
        # kern.boottime moves with sleep and clock steps: diagnostic only.
        return r"""molt_probe() {
 state=$(ps -o stat= -p "$1" 2>/dev/null || true)
 started=$(LC_ALL=C TZ=UTC ps -o lstart= -p "$1" 2>/dev/null || true)
 boot=$(sysctl -n kern.boottime | sed 's/^{ sec = \([0-9]*\),.*/\1/')
 printf '{"pid":%s,"state":"%s","started":"%s","instance":"","instance_started_at":"%s"}\n' "$1" "$(echo $state)" "$(echo $started)" "$(date -u -r "$boot" +%Y-%m-%dT%H:%M:%SZ)"
}
"""
    # proc(5): the fields after the last ") " start with state; starttime is
    # the 20th of them, in clock ticks since the kernel booted.
    return r"""molt_probe() {
 fields=$(sed -n 's/.*) //p' "/proc/$1/stat" 2>/dev/null || true)
 init=$(sed -n 's/.*) //p' /proc/1/stat | cut -d' ' -f20)
 btime=$(awk '$1 == "btime" {print $2}' /proc/stat)
 printf '{"pid":%s,"state":"%s","started":"%s","instance":"%s/%s","instance_started_at":"%s"}\n' "$1" "${fields%% *}" "$(printf '%s\n' "$fields" | cut -d' ' -f20)" "$(cat /proc/sys/kernel/random/boot_id)" "$init" "$(date -u -d "@$((btime + init / $(getconf CLK_TCK)))" +%Y-%m-%dT%H:%M:%SZ)"
}
"""


def windows_probe(pid: str) -> str:
    """Print one ProcessProbe JSON line for a PowerShell pid expression."""
    return (
        f"$probePid=[int]({pid}); "
        '$probe=Get-CimInstance Win32_Process -Filter "ProcessId=$probePid"; '
        # LastBootUpTime is derived from the clock: diagnostic only.
        "$boot=(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime(); "
        "[pscustomobject]@{pid=$probePid;"
        "state=$(if ($probe) {'R'} else {''});"
        "started=$(if ($probe) {$probe.CreationDate.ToUniversalTime().ToString('o')} else {''});"
        "instance='';"
        "instance_started_at=$boot.ToString(\"yyyy-MM-dd'T'HH:mm:ss'Z'\")} "
        "| ConvertTo-Json -Compress"
    )


# CIM passes this command line to wsl.exe, whose --exec mode splits it with
# CommandLineToArgvW and runs no shell. Admit only words that need no quoting.
_WSL_WORD = re.compile(r"[A-Za-z0-9_./-]+")


def wsl_launch_script(host, argv: list[str], directory: str, stage: str) -> str:
    """The script wsl.exe runs: publish identity, then become the worker.

    ``exec`` keeps the pid and start stamp, so the published identity is the
    worker's own. Its exit ends the wsl.exe client that holds the instance.
    """
    stage_path = PurePosixPath(stage)
    identity = shlex.quote(str(stage_path / "worker.identity"))
    return f"""set -eu
exec </dev/null >>{shlex.quote(str(stage_path / "worker.log"))} 2>&1
{posix_probe(host)}molt_probe $$ > {identity}.partial
mv -- {identity}.partial {identity}
{posix_path_head(host)}
cd {shlex.quote(directory)}
exec {shlex.join(argv)}
"""


def detached_script(host, argv: list[str], directory: str, stage: str) -> str:
    """Print the launcher acknowledgement, then the worker's ProcessProbe.

    The worker's terminal data, never the launcher, proves completion. Windows
    API contract: Create returns 0 and an out ProcessId. See Microsoft's
    Win32_Process.Create documentation; CIM avoids the OpenSSH session job.
    """
    cross = authorities()
    created = (
        "if ($result.ReturnValue -ne 0 -or $result.ProcessId -le 0) "
        '{ throw "CIM creation refused: $($result.ReturnValue)" }; '
        "$result | Select-Object ReturnValue,ProcessId | ConvertTo-Json -Compress; "
    )
    if host.launcher == cross.DetachedLauncher.WINDOWS_CIM:
        return (
            "$ErrorActionPreference='Stop'; "
            "$result=Invoke-CimMethod -ClassName Win32_Process -MethodName Create "
            "-Arguments @{CommandLine="
            + ps(subprocess.list2cmdline(argv))
            + ";CurrentDirectory="
            + ps(directory)
            + "}; "
            + created
            + windows_probe("$result.ProcessId")
        )
    if host.launcher == cross.DetachedLauncher.WSL_SESSION:
        launch = str(PurePosixPath(stage) / "launch.sh")
        identity = str(PurePosixPath(stage) / "worker.identity")
        if not all(
            _WSL_WORD.fullmatch(word)
            for word in (host.wsl_distribution or "", launch, identity)
        ):
            raise ValueError(f"{host.name}: WSL launch words must need no quoting")
        wsl = f"& wsl.exe -d {host.wsl_distribution} --exec"
        # Observe the client before the identity file: once the client is gone,
        # an identity still absent can never appear.
        return (
            "$ErrorActionPreference='Stop'; "
            "$result=Invoke-CimMethod -ClassName Win32_Process -MethodName Create "
            "-Arguments @{CommandLine="
            + ps(f"wsl.exe -d {host.wsl_distribution} --exec /bin/sh {launch}")
            + "}; "
            + created
            + "do { $alive=[bool](Get-Process -Id $result.ProcessId -ErrorAction SilentlyContinue); "
            f"{wsl} test -s {identity}; "
            f"if ($LASTEXITCODE -eq 0) {{ {wsl} cat {identity}; exit $LASTEXITCODE }}; "
            "Start-Sleep -Milliseconds 250 } while ($alive)"
        )
    if host.launcher == cross.DetachedLauncher.POSIX_SESSION:
        code = (
            "import subprocess,sys; "
            "p=subprocess.Popen(sys.argv[2:],cwd=sys.argv[1],start_new_session=True,"
            "stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); "
            "print(p.pid)"
        )
        launch = remote_argv(
            host, uv_python(host_uv(host), "-c", code, directory, *argv), cwd=directory
        )
        return (
            f"set -eu\n{posix_probe(host)}pid=$({launch})\n"
            'printf \'{"ReturnValue":0,"ProcessId":%s}\\n\' "$pid"\n'
            'molt_probe "$pid"\n'
        )
    raise ValueError(f"{host.name}: no admitted detached launcher")


def acknowledge(raw: str) -> tuple[int, ProcessProbe | None]:
    """The launcher's process id and the worker's launch-time identity.

    No identity means the worker exited before publishing one.
    """
    first, _, rest = raw.strip().partition("\n")
    result = json.loads(first)
    if (
        not isinstance(result, dict)
        or result.get("ReturnValue") != 0
        or type(result.get("ProcessId")) is not int
        or result["ProcessId"] <= 0
    ):
        raise ValueError("detached launcher did not acknowledge a process")
    return result["ProcessId"], ProcessProbe.parse(rest) if rest.strip() else None


def is_ancestor(commit: str, descendant: str, repo: Path) -> bool:
    result = authorities()._guarded_run(
        ["git", "-C", str(repo), "merge-base", "--is-ancestor", commit, descendant],
        cwd=repo,
        timeout=180,
    )
    if result.returncode not in (0, 1):
        raise RuntimeError(
            f"git merge-base --is-ancestor {commit} {descendant}: exit "
            f"{result.returncode}\n{result.stderr[-4000:]}"
        )
    return result.returncode == 0


def checked(argv: list[str], *, cwd: Path = _REPO_ROOT, timeout: int = 180) -> str:
    result = authorities()._guarded_run(argv, cwd=cwd, timeout=timeout)
    if result.returncode:
        raise RuntimeError(
            f"{shlex.join(argv)}: exit {result.returncode}\n{result.stderr[-4000:]}\n{result.stdout[-4000:]}"
        )
    return result.stdout.strip()


def exact_commit(revision: str, repo: Path = _REPO_ROOT) -> str:
    # Resolve before crossing a shell or using the value as an evidence path.
    commit = checked(
        ["git", "-C", str(repo), "rev-parse", "--verify", revision + "^{commit}"]
    )
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("expected one full Git commit")
    return commit


def host_paths(host, nonce: str) -> tuple[str, str]:
    path_type = PureWindowsPath if host.is_windows else PurePosixPath
    root = path_type(host.custody_root or "")
    checkout = path_type(host.checkout or "")
    stage_parent = path_type(host.remote_dir)
    if (
        not root.is_absolute()
        or checkout != root / "molt-src"
        or not stage_parent.is_relative_to(root)
        or stage_parent == root
    ):
        raise ValueError(
            f"{host.name}: checkout and staging must belong to its custody root"
        )
    if host.is_windows and root.drive.upper() != "C:":
        raise ValueError(
            "Windows source/toolchain custody requires C: per ORCHESTRATION"
        )
    return str(root / "worktrees" / ("trunk-" + nonce)), str(
        root / "worktrees" / ("trunk-controller-" + nonce)
    )


def selected_host(
    inventory: Path, name: str, cells: list[str], *, landing: bool = False
):
    cross = authorities()
    host = next((h for h in cross.parse_inventory(inventory) if h.name == name), None)
    if host is None:
        raise ValueError(f"unknown host {name!r}")
    if host.transport != "ssh" or not host.launcher or not host.proof_cells:
        raise ValueError(f"{name}: no admitted SSH proof host configuration")
    if landing:
        if host.os is not cross.HostOS.LINUX or cells != ["landing"]:
            raise ValueError(
                "remote landing gates require an admitted Linux proof host"
            )
    elif (
        not cells or len(set(cells)) != len(cells) or set(cells) - set(host.proof_cells)
    ):
        raise ValueError(f"{name}: select unique commands from {host.proof_cells}")
    host_paths(host, "preflight")
    if host.is_windows:
        if not host.proof_cargo_output_root or not host.proof_metadata_root:
            raise ValueError(
                "Windows proofs require explicit Cargo and metadata fixture roots"
            )
        root = PureWindowsPath(host.custody_root)
        fixture_roots = [
            PureWindowsPath(value)
            for value in (host.proof_cargo_output_root, host.proof_metadata_root)
        ]
        if any(
            not value.is_relative_to(root) or value == root for value in fixture_roots
        ):
            raise ValueError("proof fixture roots must belong to host custody")
        if any(a.is_relative_to(b) for a, b in (fixture_roots, fixture_roots[::-1])):
            raise ValueError("proof fixture roots must be disjoint")
    return host


# Executed with the SUBJECT's interpreter/import path and policy, never imported
# into the controller interpreter. The descriptor is data, not a second selector.
DESCRIBE = r"""
import dataclasses,json,pathlib,sys
root=pathlib.Path.cwd()
sys.path[:0]=[str(root),str(root/'src')]
from tools.proof_plan import ProofPlan,_topological_commands
from tools.toolchain_env import selected_toolchains
from molt.tool_releases import tool_release,host_asset
plan=ProofPlan.load()
explicit=sys.argv[1:2]==['--toolchains']
cells=[] if explicit else sys.argv[1:]
commands={c.id:c for cell in cells for c in _topological_commands(plan,command_id=cell)}
names=sys.argv[2:] if explicit else selected_toolchains(plan,commands=cells,family=None,matrix_cell=None,toolchains=[])
policies={p.name:p for p in plan.toolchain_policies}
tools={}
def release(name):
    if name in tools:return
    r=tool_release(name,root)
    tools[name]={'release':r.name,'asset':dataclasses.asdict(host_asset(r))}
    if r.runtime:release(r.runtime)
for name in names:
    p=policies[name]
    if p.projection in {'tool-release','runtime-tool-release'}:release(str(p.data['executable']))
print(json.dumps({'toolchains':names,'releases':tools,'commands':{k:dict(v.data) for k,v in commands.items()}}))
"""


def describe(
    host, transport, checkout: str, cells: list[str], *, toolchains=None
) -> dict:
    raw = transport.shell_command(
        remote_argv(
            host,
            uv_python(
                host_uv(host),
                "-c",
                DESCRIBE,
                *(cells if toolchains is None else ["--toolchains", *toolchains]),
            ),
            cwd=checkout,
        ),
        timeout=180,
    )
    result = json.loads(raw)
    if toolchains is None and set(cells) - set(result["commands"]):
        raise ValueError("subject policy omitted a selected command")
    return result


def seed_inputs(descriptor: dict, transport, local: Path) -> list[dict]:
    from molt.dx import canonical_toolchain_root
    from molt.tool_releases import provision_archive

    cache = (
        canonical_toolchain_root(_REPO_ROOT, require_exists=False)
        / "toolchains"
        / "downloads"
        / "sha256"
    )
    seeds = []
    for name, item in descriptor["releases"].items():
        if name == "uv":
            continue  # Official host prerequisite; never ship our own uv binary.
        asset = item["asset"]
        archive = provision_archive(
            url=asset["url"],
            size=asset["size"],
            sha256=asset["sha256"],
            downloads=cache / asset["sha256"] / str(asset["size"]),
        )
        # Transfer a per-run copy with a content-addressed name; keep the cache
        # immutable while scp observes it. Host verifies against SUBJECT pins.
        leaf = asset["sha256"] + ".archive"
        copy = local / leaf
        from molt.tool_releases import open_pinned_archive
        import shutil

        if not copy.exists():
            with (
                open_pinned_archive(
                    archive, size=asset["size"], sha256=asset["sha256"]
                ) as source,
                copy.open("xb") as dest,
            ):
                shutil.copyfileobj(source.stream, dest)
            transport.push([copy])
        seeds.append({"name": name, "asset": asset, "file": leaf})
    return seeds


def prepare_checkouts(
    host,
    transport,
    bundle: Path | None,
    subject: str,
    controller: str,
    checkout: str,
    control: str,
) -> None:
    """Add the run's subject and controller worktrees on the host.

    ``bundle`` carries the commits the host lacks; ``None`` when its checkout
    already contains both (provision checks out the commit it provisions).
    """
    imports: list[list[str]] = []
    if bundle is not None:
        transport.push([bundle])
        remote = transport.remote_path(bundle.name)
        imports = [
            ["git", "-C", host.checkout, "bundle", "verify", remote],
            ["git", "-C", host.checkout, "bundle", "unbundle", remote],
        ]
    # These are local object reads/imports, not network Git transfers. SSH/SCP
    # use cross_run's guarded transport; no unattended Git credentials are used.
    for argv in (
        *imports,
        ["git", "-C", host.checkout, "worktree", "add", "--detach", checkout, subject],
        [
            "git",
            "-C",
            host.checkout,
            "worktree",
            "add",
            "--detach",
            control,
            controller,
        ],
    ):
        transport.shell_command(remote_argv(host, argv), timeout=180)


def host_busy(host: "cross_run.Host", transport: "cross_run.Transport") -> bool:
    """Whether a worker holds the host lock: the disk-maintenance probe, read."""
    return (
        transport.shell_command(
            f"set -eu\nlock={shlex.quote(host.remote_dir + '/trunk-worker.lock')}\n"
            'if test -f "$lock" && ! flock -n "$lock" true; then echo busy; '
            "else echo idle; fi"
        )
        == "busy"
    )


def start(args, *, landing: dict | None = None, repo: Path = _REPO_ROOT) -> Path:
    cross = authorities()
    host = selected_host(
        args.inventory, args.host, args.cell, landing=landing is not None
    )
    # The owned retry: earlier terminal runs give back their allocations
    # before this run allocates (failures are recorded, never raised here).
    retire_terminal_runs(host.name, repo=repo)
    subject = exact_commit(args.commit, repo)
    controller = exact_commit("HEAD", repo)
    # The controller must be committed: copying dirty source would make the
    # worker identity fictional. Test the clean named controller, not a tar overlay.
    if checked(
        ["git", "-C", str(repo), "status", "--porcelain", "--untracked-files=no"]
    ):
        raise ValueError("commit controller changes before a farm run")
    checked(
        [
            "git",
            "-C",
            str(repo),
            "merge-base",
            "--is-ancestor",
            subject,
            controller,
        ]
    )
    transport = cross.SSHTransport(host)
    if host.wsl_distribution:
        admit_wsl_profile(host, transport)
        admit_wsl_guest(host, transport)
    checkout, control = host_paths(host, transport._nonce)
    from molt.dx import artifact_root

    run_id = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ") + "-" + transport._nonce
    run_root = (
        artifact_root(repo)
        / "evidence"
        / "trunk-loop"
        / subject
        / host.name
        / "runs"
        / run_id
    )
    run_root.mkdir(parents=True, exist_ok=False)
    from molt.disk_capacity import require_build_capacity

    require_build_capacity((run_root,))
    record = {
        "schema": SCHEMA,
        "commit": subject,
        "controller_commit": controller,
        "host": asdict(host),
        "nonce": transport._nonce,
        "stage": transport.execution_dir,
        "checkout": checkout,
        "controller": control,
        "cells": args.cell,
        "run_id": run_id,
        "created_at": datetime.now(UTC).isoformat(),
        "state": RunState.PREPARING.value,
    }
    record_path = run_root / "run.json"
    if landing is not None:
        record["landing"] = landing
    write_json(record_path, record)
    print(f"trunk-loop: recovery record {record_path}", flush=True)
    try:
        script = _prepare_launch(
            args, host, transport, record, record_path, run_root, repo, landing
        )
    except BaseException as exc:
        # No worker was launched: the run is over, with its reason.
        transition(
            record,
            RunState.ABANDONED,
            abandoned={
                "observed_at": datetime.now(UTC).isoformat(),
                "error": failure_text(exc),
            },
        )
        write_json(record_path, record)
        raise
    try:
        if host.launcher == cross.DetachedLauncher.WSL_SESSION:
            # A distro that must boot first delays the identity by seconds.
            raw = transport.windows_command(script, timeout=180)
        else:
            raw = transport.shell_command(script)
        pid, worker = acknowledge(raw)
    except BaseException as exc:
        # The launch command may have started a worker; collect can still
        # find its terminal receipt, so the run stays PREPARING.
        record["launch_error"] = failure_text(exc)
        write_json(record_path, record)
        raise
    transition(
        record,
        RunState.LAUNCHED,
        launcher_pid=pid,
        worker=asdict(worker) if worker is not None else None,
        launched_at=datetime.now(UTC).isoformat(),
    )
    write_json(record_path, record)
    print(
        f"trunk-loop: launched {host.name} pid={pid} "
        f"worker={worker.pid if worker is not None else 'exited'}; "
        f"collect --record {record_path}",
        flush=True,
    )
    return record_path


def _prepare_launch(
    args,
    host,
    transport,
    record: dict,
    record_path: Path,
    run_root: Path,
    repo,
    landing,
) -> str:
    """Everything before the launch command; returns the launch script."""
    cross = authorities()
    subject, controller = record["commit"], record["controller_commit"]
    checkout, control = record["checkout"], record["controller"]
    transport.prepare()
    remote_head = transport.shell_command(
        remote_argv(host, ["git", "-C", host.checkout, "rev-parse", "HEAD"])
    )
    if not re.fullmatch(r"[0-9a-f]{40}", remote_head):
        raise ValueError("cannot bind canonical checkout donor")
    checked(["git", "-C", str(repo), "cat-file", "-e", remote_head + "^{commit}"])
    # A host whose checkout already contains the subject and the controller
    # needs no bundle, and git refuses to create an empty one.
    bundle: Path | None = None
    if not all(
        is_ancestor(commit, remote_head, repo) for commit in (subject, controller)
    ):
        bundle = run_root / "source.bundle"
        checked(
            [
                "git",
                "-C",
                str(repo),
                "bundle",
                "create",
                str(bundle),
                "HEAD",
                "^" + remote_head,
            ],
            timeout=600,
        )
    prepare_checkouts(host, transport, bundle, subject, controller, checkout, control)
    descriptor = (
        describe(host, transport, checkout, args.cell, toolchains=landing["toolchains"])
        if landing is not None
        else describe(host, transport, checkout, args.cell)
    )
    record["selection"] = descriptor
    record["seeds"] = seed_inputs(descriptor, transport, run_root)
    write_json(record_path, record)
    transport.push([record_path])
    path_type = PureWindowsPath if host.is_windows else PurePosixPath
    argv = uv_python(
        host_uv(host),
        str(path_type(control) / "tools" / "trunk_loop.py"),
        "_worker",
        "--record",
        transport.remote_path("run.json"),
    )
    script = detached_script(host, argv, control, transport.execution_dir)
    if host.launcher == cross.DetachedLauncher.WSL_SESSION:
        launch = run_root / "launch.sh"
        launch.write_text(
            wsl_launch_script(host, argv, control, transport.execution_dir),
            encoding="utf-8",
            newline="\n",
        )
        transport.push([launch])
    return script


def record_host(record: dict):
    cross = authorities()
    data = dict(record["host"])
    data["os"] = cross.HostOS(data["os"])
    data["launcher"] = cross.DetachedLauncher(data["launcher"])
    data["proof_cells"] = tuple(data["proof_cells"])
    return cross.Host(**data)


def transfer_for_record(record: dict):
    cross = authorities()
    if record.get("schema") != SCHEMA or not re.fullmatch(
        r"[0-9a-f]{32}", record.get("nonce", "")
    ):
        raise ValueError("invalid run recovery record")
    host = record_host(record)
    checkout, control = host_paths(host, record["nonce"])
    transport = cross.SSHTransport(host)
    transport._nonce = record["nonce"]
    sep = "\\" if host.is_windows else "/"
    transport.execution_dir = (
        host.remote_dir.rstrip("/\\") + sep + "molt-cross-" + record["nonce"]
    )
    if (
        transport.execution_dir != record["stage"]
        or checkout != record["checkout"]
        or control != record["controller"]
    ):
        raise ValueError("run recovery paths do not match owned allocations")
    return host, transport


class WorkerVanished(RuntimeError):
    """The worker is gone and published no terminal receipt."""


def observe(host, transport, worker: ProcessProbe | None):
    """One round trip: the worker's process first, then its terminal receipt.

    A receipt still absent after the worker was seen gone can never appear;
    the reverse order would race the worker's last publication.
    """
    marker = transport.remote_path("terminal.json")
    if host.is_windows:
        probe = windows_probe(str(int(worker.pid))) if worker else "'null'"
        script = (
            f"{probe}\nif (Test-Path -LiteralPath {ps(marker)}) "
            f"{{ Get-Content -Raw -LiteralPath {ps(marker)} }}"
        )
    else:
        probe = (
            f"{posix_probe(host)}molt_probe {int(worker.pid)}"
            if worker
            else "echo null"
        )
        script = (
            f"set -eu\n{probe}\n"
            f"if test -f {shlex.quote(marker)}; then cat {shlex.quote(marker)}; fi"
        )
    first, _, rest = transport.shell_command(script).partition("\n")
    current = None if first.strip() == "null" else ProcessProbe.parse(first)
    return current, json.loads(rest) if rest.strip() else None


def worker_vanished(
    record_path: Path,
    record: dict,
    host,
    transport,
    launched: ProcessProbe | None,
    current: ProcessProbe | None,
) -> NoReturn:
    """Record the evidence and fail at once; never wait on a vanished worker."""
    if launched is None or launched.identity is None:
        reason = "it exited before publishing its identity"
    elif current is not None and current.instance != launched.instance:
        scope = "distro" if host.wsl_distribution else "host"
        reason = (
            f"{scope} restarted at {current.instance_started_at} (worker launched "
            f"under the instance started at {launched.instance_started_at})"
        )
    elif current is None or not current.state:
        reason = "the process no longer exists"
    elif current.identity is None:
        reason = f"the process exited (state {current.state})"
    else:
        reason = f"the pid now names another process started {current.started}"
    log = record_path.parent / "worker.log"
    try:
        transport.pull("worker.log", log)
        retained = str(log)
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as exc:
        retained = f"unavailable ({exc})"
    pid = launched.pid if launched is not None else record.get("launcher_pid")
    transition(
        record,
        RunState.VANISHED,
        vanished={
            "observed_at": datetime.now(UTC).isoformat(),
            "reason": reason,
            "current": asdict(current) if current is not None else None,
            "worker_log": retained,
        },
    )
    write_json(record_path, record)
    raise WorkerVanished(
        f"worker {pid} on {host.name} is gone with no terminal receipt: {reason}; "
        f"remote stage {record['stage']} retained; worker log {retained}"
    )


# A cell's results may nest: a landing writes per-gate logs beside its receipt.
# Every run kind shares one
# bound; a member must stay inside a selected cell's directory.
MAX_RESULT_MEMBER_DEPTH = 16
MAX_RESULT_MEMBER_BYTES = 512 * 1024 * 1024


def admitted_result_member(item: zipfile.ZipInfo, cells) -> PurePosixPath:
    """The archive member's path inside its cell, or ValueError."""
    path = PurePosixPath(item.filename)
    if (
        "\\" in item.filename
        or path.is_absolute()
        or ".." in path.parts
        or not (2 <= len(path.parts) <= MAX_RESULT_MEMBER_DEPTH)
        or path.parts[0] not in cells
        or item.file_size > MAX_RESULT_MEMBER_BYTES
    ):
        raise ValueError(f"unexpected result archive member {item.filename}")
    return path


def collect(record_path: Path, *, wait: bool = True) -> int:
    record = json.loads(record_path.read_text(encoding="utf-8"))
    match RunState(record["state"]):
        case RunState.COLLECTED:
            # Terminal: answer from the record; the remote stage may be gone.
            return _recorded_collection(record_path, record)
        case RunState.VANISHED:
            raise WorkerVanished(
                f"{record['run_id']}: recorded vanished at "
                f"{record['vanished']['observed_at']}: {record['vanished']['reason']}"
            )
        case RunState.ABANDONED:
            raise RuntimeError(
                f"{record['run_id']}: start was abandoned before any launch "
                f"({record['abandoned']['error']}); nothing to collect"
            )
        case RunState.PREPARING | RunState.LAUNCHED:
            pass
    host, transport = transfer_for_record(record)
    launched = (
        ProcessProbe(**record["worker"]) if record.get("worker") is not None else None
    )
    while True:
        current, terminal = observe(host, transport, launched)
        if terminal is not None:
            break
        if "launcher_pid" not in record:
            raise RuntimeError(
                f"{record['run_id']}: launch was never acknowledged, so no worker "
                f"identity exists and no terminal receipt was published; remote "
                f"stage {record['stage']} retained"
            )
        if (
            launched is None
            or launched.identity is None
            or current is None
            or current.identity != launched.identity
        ):
            worker_vanished(record_path, record, host, transport, launched, current)
        if not wait:
            print("trunk-loop: completion unknown; retain stage and collect later")
            return 3
        print(
            f"trunk-loop: {record['run_id']} worker {launched.pid} alive; "
            "waiting for terminal receipt",
            flush=True,
        )
        time.sleep(20)
    if (
        terminal.get("commit") != record["commit"]
        or terminal.get("run_id") != record["run_id"]
    ):
        raise ValueError("terminal result does not bind this run")
    destination = record_path.parent / "results.zip"
    if destination.exists():
        if digest(destination) != terminal["archive_sha256"]:
            raise ValueError("retained result archive differs from terminal identity")
    else:
        partial = record_path.parent / "results.zip.partial"
        transport.pull("results.zip", partial)
        if digest(partial) != terminal["archive_sha256"]:
            raise ValueError(
                "collected result archive digest mismatch; retain partial transfer"
            )
        from molt.file_publication import durable_publish_exclusive

        durable_publish_exclusive(partial, destination)
    if digest(destination) != terminal["archive_sha256"]:
        raise ValueError("collected result archive digest mismatch")
    try:
        status = TerminalStatus(terminal.get("status"))
    except ValueError as exc:
        raise ValueError("unknown terminal status") from exc
    if status is not TerminalStatus.INFRASTRUCTURE and (
        {cell["id"] for cell in terminal["cells"]} != set(record["cells"])
        or not all(cell.get("receipt") is True for cell in terminal["cells"])
    ):
        raise ValueError("terminal result omits selected proof receipts")
    root = record_path.parents[2]
    with zipfile.ZipFile(destination) as archive:
        for cell in terminal["cells"]:
            receipt = json.loads(archive.read(cell["id"] + "/receipt.json"))
            if (
                receipt.get("source_commit") != record["commit"]
                or receipt.get("source_tree_state") != SourceTreeState.CLEAN
            ):
                raise ValueError(
                    "collected proof receipt has different or dirty source"
                )
        for item in archive.infolist():
            path = admitted_result_member(item, record["cells"])
            target = root / path.parts[0] / record["run_id"] / Path(*path.parts[1:])
            target.parent.mkdir(parents=True, exist_ok=True)
            content = archive.read(item)
            if target.exists():
                if target.read_bytes() != content:
                    raise ValueError(f"retained evidence changed: {target}")
            else:
                from molt.file_publication import atomic_write_bytes

                atomic_write_bytes(target, content, exclusive=True)
    transition(record, RunState.COLLECTED, terminal=terminal)
    write_json(record_path, record)
    retire_remote_run(record_path, record)
    print(f"trunk-loop: collected {root}; status={status}", flush=True)
    return 0 if status is TerminalStatus.SUCCESS else 1


def _recorded_collection(record_path: Path, record: dict) -> int:
    """Re-read a collected run from its retained, digest-checked archive."""
    terminal = record["terminal"]
    archive = record_path.parent / "results.zip"
    if not archive.is_file() or digest(archive) != terminal["archive_sha256"]:
        raise ValueError(
            f"{record['run_id']}: collected run lost or changed its retained archive"
        )
    status = TerminalStatus(terminal["status"])
    print(
        f"trunk-loop: {record['run_id']} already collected; status={status}", flush=True
    )
    retire_remote_run(record_path, record)
    return 0 if status is TerminalStatus.SUCCESS else 1


class RemoteRetirement(StrEnum):
    """What retirement did with a terminal run's remote allocations.

    Retirement is orthogonal to the run lifecycle: a COLLECTED, VANISHED or
    ABANDONED run is retired afterwards and never re-enters RunState.
    """

    RETIRED = "retired"  # stage, worktrees and proof roots removed
    DEFERRED = "deferred"  # the worker still runs; retried by the next sweep
    FAILED = "failed"  # an allocation refused removal; retried by the next sweep


_TERMINAL_RUN_STATES: Final = frozenset(
    {RunState.COLLECTED, RunState.VANISHED, RunState.ABANDONED}
)
_RETIREMENT_TIMEOUT_S: Final = 300.0


def retire_remote_run(record_path: Path, record: dict) -> RemoteRetirement:
    """Remove a terminal run's remote stage, worktrees and proof roots.

    The local record and evidence are complete before this runs; a failure is
    recorded and retried by the next sweep, never raised into a landing.
    Allocations are named only by the record, after the record's host matches
    the current inventory and its paths match the owned allocation rule.
    """
    if RunState(record["state"]) not in _TERMINAL_RUN_STATES:
        raise InvalidRunTransition(
            f"{record['run_id']}: only a terminal run retires its allocations"
        )
    previous = record.get("remote_retirement") or {}
    if previous.get("status") == RemoteRetirement.RETIRED:
        return RemoteRetirement.RETIRED
    try:
        status, detail = _retire_allocations(record_path, record)
    except Exception as exc:  # noqa: BLE001 - recorded, retried by the next sweep
        status, detail = RemoteRetirement.FAILED, failure_text(exc)
    record["remote_retirement"] = {
        "status": status,
        "observed_at": datetime.now(UTC).isoformat(),
        "detail": detail,
    }
    write_json(record_path, record)
    if status is not RemoteRetirement.RETIRED:
        print(
            f"trunk-loop: {record['run_id']} retirement {status}: {detail}", flush=True
        )
    return status


def _retire_allocations(
    record_path: Path, record: dict
) -> tuple[RemoteRetirement, str]:
    host, transport = transfer_for_record(record)
    _require_inventory_host(host)
    for path in (record["stage"], record["checkout"], record["controller"]):
        parts = (PureWindowsPath if host.is_windows else PurePosixPath)(path).parts
        if ".." in parts:
            raise ValueError(f"run allocation path is not normalized: {path}")
    worker = record.get("worker")
    if worker is not None:
        launched = ProcessProbe(**worker)
        current, _terminal = observe(host, transport, launched)
        if (
            launched.identity is not None
            and current is not None
            and current.identity == launched.identity
        ):
            return RemoteRetirement.DEFERRED, f"worker {launched.pid} still runs"
    log = record_path.parent / "worker.log"
    if not log.exists():
        with contextlib.suppress(
            OSError, RuntimeError, ValueError, subprocess.SubprocessError
        ):
            transport.pull("worker.log", log)
    allocations = [(record["checkout"], record["commit"])]
    if record.get("controller_commit"):
        allocations.append((record["controller"], record["controller_commit"]))
    script = worktree_retirement_script(host, allocations)
    if host.is_windows:
        transport.windows_command(script, timeout=_RETIREMENT_TIMEOUT_S)
    else:
        transport.shell_command(script, timeout=_RETIREMENT_TIMEOUT_S)
    transport.cleanup()
    return RemoteRetirement.RETIRED, "stage and worktrees removed"


def _require_inventory_host(host) -> None:
    cross = authorities()
    current = next(
        (
            h
            for h in cross.parse_inventory(_REPO_ROOT / "tools/cross_hosts.toml")
            if h.name == host.name
        ),
        None,
    )
    if current is None or (
        current.custody_root,
        current.checkout,
        current.remote_dir,
    ) != (
        host.custody_root,
        host.checkout,
        host.remote_dir,
    ):
        raise ValueError(
            f"{host.name}: the run record's custody paths differ from the inventory"
        )


def worktree_retirement_script(host, allocations: list[tuple[str, str]]) -> str:
    """Remove each worktree only when git registers that exact path at the run's
    commit; anything else at the path is retained and fails the retirement."""
    if host.is_windows:
        rows = ",".join(f"@({ps(path)},{ps(commit)})" for path, commit in allocations)
        return (
            "$ErrorActionPreference='Stop'\n"
            f"$repo={ps(host.checkout)}\n"
            "$listing=@(git -C $repo worktree list --porcelain)\n"
            "if ($LASTEXITCODE) { exit $LASTEXITCODE }\n"
            f"foreach ($row in @({rows})) {{\n"
            " $path=$row[0]; $head=$row[1]; $found=$false; $tree=$null\n"
            " foreach ($line in $listing) {\n"
            "  if ($line.StartsWith('worktree ')) { $tree=$line.Substring(9).Replace('/','\\') }\n"
            "  elseif ($line.StartsWith('HEAD ') -and $tree -ieq $path -and $line.Substring(5) -ceq $head) { $found=$true }\n"
            " }\n"
            " if ($found) { git -C $repo worktree remove --force $path; if ($LASTEXITCODE) { exit $LASTEXITCODE } }\n"
            ' elseif (Test-Path -LiteralPath $path) { throw "$path is not this run\'s registered worktree at $head; retained" }\n'
            "}\n"
        )
    lines = [
        "set -eu",
        f"repo={shlex.quote(host.checkout)}",
        'listing=$(git -C "$repo" worktree list --porcelain)',
    ]
    for path, commit in allocations:
        lines.append(
            f"path={shlex.quote(path)}; head={shlex.quote(commit)}\n"
            'if printf \'%s\\n\' "$listing" | awk -v p="$path" -v c="$head" '
            '\'$1=="worktree" {w=substr($0, 10)} $1=="HEAD" {if (w==p && $2==c) f=1} END {exit !f}\'; then\n'
            ' git -C "$repo" worktree remove --force "$path"\n'
            'elif [ -e "$path" ] || [ -L "$path" ]; then\n'
            ' echo "$path is not this run\'s registered worktree at $head; retained" >&2; exit 3\n'
            "fi"
        )
    return "\n".join(lines) + "\n"


def retire_terminal_runs(
    host_name: str, *, repo: Path = _REPO_ROOT
) -> dict[str, RemoteRetirement]:
    """The owned retry: retire every terminal run of ``host_name`` not yet retired."""
    from molt.dx import artifact_root

    outcomes = {}
    pattern = f"evidence/trunk-loop/*/{host_name}/runs/*/run.json"
    for record_path in sorted(artifact_root(repo).glob(pattern)):
        record = json.loads(record_path.read_text(encoding="utf-8"))
        if record.get("schema") != SCHEMA:
            continue
        try:
            terminal = RunState(record["state"]) in _TERMINAL_RUN_STATES
        except ValueError:
            continue
        retired = (record.get("remote_retirement") or {}).get("status")
        if terminal and retired != RemoteRetirement.RETIRED:
            outcomes[record["run_id"]] = retire_remote_run(record_path, record)
    return outcomes


def worker_environment(record: dict) -> dict[str, str]:
    from molt.dx import RunContext
    from molt.process_environment import ProcessEnvironment

    # DX owns source/toolchain/artifact addresses. Do not inherit a service's
    # foreign virtualenv or cross-cell pytest arguments into the task.
    from molt.dx import CANONICAL_RUN_ENV_KEYS, DX_ENV_KEYS

    baseline = dict(os.environ)
    for key in {
        *CANONICAL_RUN_ENV_KEYS,
        *DX_ENV_KEYS,
        "VIRTUAL_ENV",
        "UV_PROJECT_ENVIRONMENT",
        "MOLT_VENV",
        "PYTHONPATH",
        "PYTEST_ADDOPTS",
        "TMP",
        "TEMP",
        "TMPDIR",
    }:
        baseline.pop(key, None)
    baseline.update(
        PYTHONDONTWRITEBYTECODE="1", PYTHONUTF8="1", PYTHONIOENCODING="utf-8"
    )
    root = Path(record["host"]["custody_root"])
    # One reusable Cargo workspace domain per host, independent of run worktree.
    baseline["CARGO_TARGET_DIR"] = str(root / "cache" / "cargo-workspace")
    baseline["MOLT_DIFF_CARGO_TARGET_DIR"] = baseline["CARGO_TARGET_DIR"]
    baseline["RUSTUP_AUTO_INSTALL"] = "0"
    # The same PATH head as every farm shell (posix_path_head): the pinned uv,
    # because proof commands run `uv` by name, then rustup's Cargo, which a CIM
    # worker's registry PATH may lack (rustup installs with --no-modify-path).
    baseline["PATH"] = os.pathsep.join(
        (
            str(Path(host_uv(record_host(record))).parent),
            str(Path.home() / ".cargo" / "bin"),
            baseline.get("PATH", ""),
        )
    )
    env = dict(
        RunContext(Path(record["checkout"])).dx_env(ProcessEnvironment(baseline))
    )
    # dx_env may project an unselected environment address. The existing
    # publisher must choose its immutable generation before venv_env binds it.
    env.pop("UV_PROJECT_ENVIRONMENT", None)
    return env


def fixture_roots(record: dict, env: dict[str, str]) -> dict:
    """Project CI's explicit native-fixture roles through their existing owner.

    These are physical output declarations, not toolchain pins. Never inherit
    another run's roots or choose a volume by a failed test/path budget.
    """
    from tools.proof_queue_pkg.cargo_output_layout import declare_root
    from molt.file_publication import resolve_owned_path

    declarations = {}
    for field, variable in (
        ("proof_cargo_output_root", "MOLT_PROOF_TEST_CARGO_OUTPUT_ROOT"),
        ("proof_metadata_root", "MOLT_PROOF_TEST_METADATA_ROOT"),
    ):
        env.pop(variable, None)
        selected = record["host"].get(field)
        if selected is None:
            continue
        # Root declarations contain durable identity in v2. A v1 worker keeps
        # its nonce directories in the parent and never sees these records.
        parent = resolve_owned_path(Path(selected)) / "v2"
        parent.mkdir(parents=True, exist_ok=True)
        directory = parent / record["nonce"][:16]
        directory.mkdir(exist_ok=False)
        declaration = declare_root(str(directory))
        write_json(
            directory / "trunk-owner.json",
            {
                "schema": SCHEMA,
                "run_id": record["run_id"],
                "nonce": record["nonce"],
                "role": field,
                "root": declaration,
            },
        )
        declarations[field] = declaration
        env[variable] = str(directory)
    return declarations


def provision(record: dict, stage: Path) -> dict[str, str]:
    from molt import checkout_provisioning
    from molt.dx import canonical_toolchain_root
    from molt.tool_releases import (
        provision_archive,
        provision_tool,
        tool_release,
        host_asset,
    )
    from tools.venv_exec import venv_env, venv_python

    checkout = Path(record["checkout"])
    env = worker_environment(record)
    roots = fixture_roots(record, env)
    tool_root = canonical_toolchain_root(checkout, env, require_exists=False)
    from molt.disk_capacity import require_build_capacity

    capacity_paths = [stage, checkout, tool_root]
    if record["host"].get("wsl_distribution"):
        capacity_paths.append(Path("/mnt/c"))
    require_build_capacity(tuple(capacity_paths), env=env)
    for seed in record["seeds"]:
        release = tool_release(seed["name"], checkout)
        asset = host_asset(release)
        if asdict(asset) != seed["asset"]:
            raise ValueError("transferred asset does not match the subject's manifest")
        archive = provision_archive(
            url=asset.url,
            size=asset.size,
            sha256=asset.sha256,
            downloads=tool_root / "toolchains" / "downloads",
            source=stage / seed["file"],
        )
        installed = provision_tool(release, tool_root, downloads=archive.parent)
        if release.runtime is None:
            env["PATH"] = (
                str(installed.executable.parent) + os.pathsep + env.get("PATH", "")
            )
    ensured = checkout_provisioning.ensure(
        "uv-environment", checkout, env, guard_source_root=_REPO_ROOT
    )
    if ensured.environment_root is None:
        raise ValueError("provisioning published no development image")
    python = venv_python(ensured.environment_root)
    env = dict(venv_env(venv=ensured.environment_root, env=env, managed=True))
    env["PYTHONPATH"] = os.pathsep.join((str(checkout), str(checkout / "src")))
    require_pinned_uv(
        env.get("PATH", ""), host_uv(record_host(record)), record["host"]["name"]
    )
    # Canonical subject admission checks the UNION before the first test. The
    # proof executor then retains its own per-command fingerprints/receipts.
    code = (
        "import json,os,pathlib,sys; from tools.proof_plan import ProofPlan,admit_toolchains; "
        "from tools.toolchain_env import selected_toolchains; p=ProofPlan.load(); "
        "n=json.loads(sys.argv[1]) if sys.argv[2]=='landing' else selected_toolchains(p,commands=sys.argv[3:],family=None,matrix_cell=None,toolchains=[]); "
        "a=admit_toolchains(p.toolchain_policies,n,python_executable=pathlib.Path(sys.executable),environ=os.environ,root=pathlib.Path.cwd()); "
        "print(json.dumps({'toolchains':n,'fingerprints':a.fingerprints}))"
    )
    result = authorities()._guarded_run(
        uv_python(
            host_uv(record_host(record)),
            "-c",
            code,
            json.dumps(record["selection"]["toolchains"]),
            "landing" if "landing" in record else "proof",
            *record["cells"],
        ),
        cwd=checkout,
        env=env,
        timeout=900,
    )
    (stage / "admission.log").write_text(
        result.stdout + result.stderr, encoding="utf-8"
    )
    if result.returncode:
        raise RuntimeError(
            "selected toolchain admission refused before testing; see admission.log\n"
            + result.stderr[-4000:]
        )
    write_json(
        stage / "provision.json",
        {
            "python": str(python),
            "toolchains": record["selection"]["toolchains"],
            "admission": json.loads(result.stdout),
            "fixture_roots": roots,
        },
    )
    return env


_WINDOWS_STD_HANDLES = ((-10, 0), (-11, 1), (-12, 2))  # STD_INPUT/OUTPUT/ERROR


def bind_standard_streams(
    log_path: Path, *, set_std_handle=None, get_osfhandle=None
) -> None:
    """Give the worker real standard streams: stdin from the null device, output to the log.

    A CIM-created Windows process starts with invalid standard handles. dup2
    rebinds only the C runtime descriptors, so `subprocess` children without
    explicit stdio still duplicate the invalid OS handles and fail with
    WinError 6 (the 2026-11-04 bat00 run lost sys.stderr this way). On Windows
    the OS standard handles are pointed at the same descriptors, which is what
    the POSIX launchers already get from `exec </dev/null >>worker.log 2>&1`.
    """
    null = os.open(os.devnull, os.O_RDONLY)
    os.dup2(null, 0)
    os.close(null)
    with log_path.open("ab", buffering=0) as log:
        os.dup2(log.fileno(), 1)
        os.dup2(log.fileno(), 2)
    if (set_std_handle is None) != (get_osfhandle is None):
        raise TypeError("pass both set_std_handle and get_osfhandle, or neither")
    if os.name == "nt" or set_std_handle is not None:
        if set_std_handle is None:
            import ctypes
            import msvcrt

            kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

            def set_std_handle(which: int, handle: int) -> None:
                if not kernel32.SetStdHandle(which, handle):
                    raise ctypes.WinError(ctypes.get_last_error())

            get_osfhandle = msvcrt.get_osfhandle
        for which, fd in _WINDOWS_STD_HANDLES:
            set_std_handle(which, get_osfhandle(fd))
    # Python's stream objects were built from the startup handles; rebuild them.
    sys.stdin = open(0, encoding="utf-8", closefd=False)
    sys.stdout = open(
        1, "w", encoding="utf-8", errors="backslashreplace", buffering=1, closefd=False
    )
    sys.stderr = open(
        2, "w", encoding="utf-8", errors="backslashreplace", buffering=1, closefd=False
    )


def worker(record_path: Path) -> int:
    stage = record_path.parent
    # Bind standard streams before loading repository authorities. No
    # PowerShell redirection (which writes UTF-16) and no inherited SSH pipe.
    bind_standard_streams(stage / "worker.log")
    authorities()
    record = json.loads(record_path.read_text(encoding="utf-8"))
    transfer_for_record(record)
    uv = host_uv(record_host(record))
    status = TerminalStatus.INFRASTRUCTURE
    cells = []
    lock = None
    unresolved_child = False
    try:
        if (stage / ".molt-owner").read_text(encoding="utf-8").strip() != record[
            "nonce"
        ]:
            raise ValueError("remote staging ownership changed")
        from molt.file_locks import _acquire_file_lock

        lock = _acquire_file_lock(
            stage.parent / "trunk-worker.lock",
            timeout_s=0,
            timeout_message="another trunk worker owns this host",
        )
        from molt.file_locks import _release_file_lock

        def release_host_lock():
            _release_file_lock(lock)

        # The canonical waiter registers its wait later: on shutdown the
        # child closes before its source use and then this host lease.
        atexit.register(release_host_lock)
        for path, commit in (
            (Path(record["checkout"]), record["commit"]),
            (_REPO_ROOT, record["controller_commit"]),
        ):
            if checked(["git", "-C", str(path), "rev-parse", "HEAD"]) != commit:
                raise ValueError("remote source identity changed")
            if checked(
                [
                    "git",
                    "-C",
                    str(path),
                    "status",
                    "--porcelain",
                    "--untracked-files=no",
                ]
            ):
                raise ValueError("remote source has tracked edits")
        env = provision(record, stage)
        (stage / "trunk_timings.py").write_text(TIMINGS_PLUGIN, encoding="utf-8")
        env["PYTHONPATH"] += os.pathsep + str(stage)
        status = TerminalStatus.SUCCESS
        for cell in record["cells"]:
            directory = stage / cell
            directory.mkdir(exist_ok=False)
            argv = uv_python(
                uv,
                str(Path(record["checkout"]) / "tools" / "proof_plan.py"),
                "--run-command",
                cell,
                "--receipt",
                str(directory / "receipt.json"),
            )
            if "landing" in record:
                argv = uv_python(
                    uv,
                    "-m",
                    "tools.trunk_landing",
                    "--record",
                    str(record_path),
                    "--output",
                    str(directory),
                )
            write_json(
                directory / "cmd.json",
                {"argv": argv, "commit": record["commit"], "cwd": record["checkout"]},
            )
            child = dict(env)
            child["PYTEST_ADDOPTS"] = (
                "--junitxml="
                + shlex.quote(str(directory / "junit.xml"))
                + " --durations=0 -p trunk_timings --trunk-durations="
                + shlex.quote(str(directory))
            )
            from tools.detached_execution import _exec_wait_rc

            with (
                chdir(record["checkout"]),
                (directory / "run.log").open("w", encoding="utf-8", buffering=1) as log,
            ):
                returncode = _exec_wait_rc(argv, child, log)
            (directory / "rc").write_text(str(returncode) + "\n", encoding="ascii")
            receipt = json.loads(
                (directory / "receipt.json").read_text(encoding="utf-8")
            )
            if (
                receipt.get("source_commit") != record["commit"]
                or receipt.get("source_tree_state") != SourceTreeState.CLEAN
                or (
                    "landing" not in record
                    and cell not in {c.get("id") for c in receipt.get("commands", [])}
                )
            ):
                raise ValueError(
                    "proof receipt does not bind the selected clean source and cell"
                )
            copy_declared_evidence(receipt, Path(record["checkout"]), directory)
            cells.append({"id": cell, "returncode": returncode, "receipt": True})
            if (
                "landing" in record
                and TerminalStatus(receipt.get("status"))
                is TerminalStatus.INFRASTRUCTURE
            ):
                status = TerminalStatus.INFRASTRUCTURE
                break
            if receipt.get("execution", {}).get("global_stop_triggered"):
                status = TerminalStatus.INFRASTRUCTURE
                break
            if returncode:
                status = TerminalStatus.FAILURE
                # Cells are independent selections; preserve this failure and
                # continue the next cell, never a dependent command batch.
    except Exception as exc:
        traceback.print_exc()
        status = TerminalStatus.INFRASTRUCTURE
        unresolved_child = getattr(exc, "owned_process", None) is not None
    except BaseException:
        # Resume control interruption after the canonical waiter's atexit
        # closure; an interrupt cannot authorize a terminal completion.
        unresolved_child = True
        raise
    finally:
        if not unresolved_child:
            publish_worker_result(record, stage, cells, status)
            if lock is not None:
                atexit.unregister(release_host_lock)
                release_host_lock()
    return 0 if status is TerminalStatus.SUCCESS else 1


def copy_declared_evidence(receipt: dict, checkout: Path, directory: Path) -> None:
    """Carry each command's declared evidence into the cell's stage.

    The run checkout is retired after collection and the archive holds only
    ``stage/<cell>/``. The receipt's evidence snapshot names each path and its
    digests, so the collector can verify the copies.
    """
    import shutil

    from molt.portable_paths import portable_relative_path

    for command in receipt.get("commands", ()):
        for snapshot in command.get("evidence_outputs") or ():
            relative = portable_relative_path(snapshot["path"])
            source = checkout / relative
            target = directory / "evidence" / relative
            if source.is_dir():
                shutil.copytree(source, target, symlinks=True)
            elif source.is_file():
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)


def publish_worker_result(
    record: dict, stage: Path, cells: list, status: TerminalStatus
) -> None:
    """Publish completion only after the existing detached owner closed."""
    sys.stdout.flush()
    sys.stderr.flush()
    for cell in record["cells"]:
        directory = stage / cell
        directory.mkdir(exist_ok=True)
        for name in ("worker.log", "admission.log", "provision.json"):
            source = stage / name
            if source.exists():
                import shutil

                shutil.copyfile(source, directory / name)
    archive = stage / "results.zip"
    with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED) as output:
        for cell in record["cells"]:
            for item in sorted((stage / cell).rglob("*")):
                if item.is_file():
                    output.write(
                        item, cell + "/" + item.relative_to(stage / cell).as_posix()
                    )
    write_json(
        stage / "terminal.json",
        {
            "schema": SCHEMA,
            "commit": record["commit"],
            "run_id": record["run_id"],
            "status": status,
            "cells": cells,
            "archive_sha256": digest(archive),
            "finished_at": datetime.now(UTC).isoformat(),
        },
    )


# Diagnostic duration stream survives an interrupted pytest session that never
# finalizes JUnit. Each process owns its file; xdist workers never share a writer.
# Failure classification remains exclusively ci_triage's JUnit/guard reader.
TIMINGS_PLUGIN = """import json,os,pathlib
_output=None
def pytest_addoption(parser):
    parser.addoption("--trunk-durations",default=None)
def pytest_configure(config):
    global _output
    value=config.getoption("--trunk-durations")
    runs_tests=hasattr(config,"workerinput") or not getattr(config.option,"numprocesses",0)
    if runs_tests:
        # Configure only this run: a test that launches pytest must not inherit
        # -p trunk_timings without its path, or this run's --junitxml.
        os.environ.pop("PYTEST_ADDOPTS",None)
    if value and runs_tests:
        _output=pathlib.Path(value)/("durations-"+str(os.getpid())+".jsonl")
def pytest_runtest_logreport(report):
    if _output is not None:
        with _output.open("a",encoding="utf-8") as stream:
            stream.write(json.dumps({"nodeid":report.nodeid,"phase":report.when,"duration_seconds":report.duration})+"\\n")
"""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    run = sub.add_parser("run")
    run.add_argument(
        "--inventory", type=Path, default=_REPO_ROOT / "tools" / "cross_hosts.toml"
    )
    run.add_argument("--host", required=True)
    run.add_argument("--commit", required=True)
    run.add_argument("--cell", action="append", required=True)
    run.add_argument("--no-wait", action="store_true")
    run.add_argument(
        "--if-idle",
        action="store_true",
        help="skip, staging nothing, while a worker holds the host lock",
    )
    setup = sub.add_parser("provision")
    setup.add_argument(
        "--inventory", type=Path, default=_REPO_ROOT / "tools/cross_hosts.toml"
    )
    setup.add_argument("--host", required=True)
    setup.add_argument(
        "--commit",
        required=True,
        help="commit (on origin) whose doctor and lock the host provisions",
    )
    get = sub.add_parser("collect")
    get.add_argument("--record", type=Path, required=True)
    get.add_argument("--no-wait", action="store_true")
    retire = sub.add_parser(
        "retire", help="retire every terminal run of a host that is not yet retired"
    )
    retire.add_argument("--host", required=True)
    internal = sub.add_parser("_worker", help=argparse.SUPPRESS)
    internal.add_argument("--record", type=Path, required=True)
    args = parser.parse_args(argv)
    if args.action == "_worker":
        return worker(args.record)
    authorities()
    try:
        if args.action == "provision":
            provision_host(args)
            return 0
        if args.action == "collect":
            return collect(args.record, wait=not args.no_wait)
        if args.action == "retire":
            outcomes = retire_terminal_runs(args.host)
            for run_id, status in outcomes.items():
                print(f"trunk-loop: {run_id} {status}")
            return (
                0
                if all(s is RemoteRetirement.RETIRED for s in outcomes.values())
                else 1
            )
        if args.if_idle:
            host = selected_host(args.inventory, args.host, args.cell)
            if host_busy(host, authorities().SSHTransport(host)):
                # A periodic measurement never queues behind or competes with
                # a landing gate: before any retirement or staging, the slot
                # is skipped (docs/design/trunk_loop.md, the instruction driver).
                print(f"trunk-loop: {host.name} busy; nothing staged, not measured")
                return 0
        record = start(args)
        return 0 if args.no_wait else collect(record)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        print(f"trunk-loop: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
