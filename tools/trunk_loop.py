#!/usr/bin/env python3
"""Run exact-source proof-plan commands on an inventoried farm host.

Stage 1: explicit run/collect; no scheduler, ledger mutation or automatic revert.
See docs/design/trunk_loop.md. The controller and tested commit are independent
identities: an older main is never modified to install this runner.
"""

from __future__ import annotations

import argparse
import atexit
from contextlib import chdir
from dataclasses import asdict
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
import zipfile

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

SCHEMA = "molt.trunk-run.v2"


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


def uv_python(*argv: str) -> list[str]:
    return ["uv", "run", "--locked", "python", "-B", *argv]


def provision_script(host, *, repository: str, uv_version: str) -> str:
    """The documented local workflow, with report-only system prerequisites.

    Official user installers own their binaries; uv owns Python and the lock.
    Never import Molt before uv has resolved the project's dependencies.
    """
    checkout = host.checkout
    if host.is_windows:
        return f"""$ErrorActionPreference='Stop'
$env:UV_PROJECT_ENVIRONMENT=$null
$env:VIRTUAL_ENV=$null
$env:MOLT_VENV=$null
$env:PATH="$env:USERPROFILE\\.local\\bin;$env:USERPROFILE\\.cargo\\bin;$env:PATH"
foreach ($tool in @('git','cl.exe','link.exe')) {{
 if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {{
  throw 'Missing system toolchain; run: winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"; then use its developer shell'
 }}
}}
if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {{
 Invoke-RestMethod https://astral.sh/uv/{uv_version}/install.ps1 | Invoke-Expression
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
 & git -C {ps(checkout)} fetch origin
 if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
}}
Set-Location -LiteralPath {ps(checkout)}
& rustup show active-toolchain
if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
& uv sync --locked --group dev
if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
& uv run --locked molt doctor --json --strict
if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}
"""
    system_command = (
        "xcode-select --install; brew install cmake"
        if host.os == authorities().HostOS.MACOS
        else "sudo apt-get update && sudo apt-get install git curl clang lld cmake build-essential"
    )
    return f"""set -eu
unset UV_PROJECT_ENVIRONMENT VIRTUAL_ENV MOLT_VENV
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
for tool in git curl cc cmake; do
 command -v "$tool" >/dev/null || {{ echo {shlex.quote("Missing system prerequisites; run: " + system_command)} >&2; exit 2; }}
done
if ! command -v uv >/dev/null; then
 installer=$(mktemp)
 trap 'rm -f "$installer"' EXIT
 curl -LsSf https://astral.sh/uv/{uv_version}/install.sh -o "$installer"
 sh "$installer"
 rm "$installer"
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
 git -C {shlex.quote(checkout)} fetch origin
else
 git clone -- {shlex.quote(repository)} {shlex.quote(checkout)}
fi
cd {shlex.quote(checkout)}
rustup show active-toolchain
uv sync --locked --group dev
uv run --locked molt doctor --json --strict
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
    state = json.loads(transport.wsl_root_command(wsl_systemd_script()))
    if state["restart"]:
        transport.windows_command(
            f"& wsl.exe --terminate {ps(host.wsl_distribution)}\n"
            "if ($LASTEXITCODE) { exit $LASTEXITCODE }"
        )
    return {
        **admit_wsl_guest(host, transport),
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
    import tomllib

    releases = tomllib.loads(
        (_REPO_ROOT / "config/tool_releases.toml").read_text(encoding="utf-8")
    )
    repository = checked(["git", "-C", str(_REPO_ROOT), "remote", "get-url", "origin"])
    result = transport.shell_command(
        provision_script(
            host,
            repository=repository,
            uv_version=releases["tools"]["uv"]["version"],
        ),
        timeout=1800,
    )
    retention = json.loads(
        transport.shell_command(
            remote_argv(
                host,
                uv_python("-c", RETENTION_STATUS),
                cwd=host.checkout,
            )
        )
    )
    record = {
        "host": host.name,
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


def remote_argv(host, argv: list[str], *, cwd: str | None = None) -> str:
    if host.is_windows:
        prefix = (
            "$ErrorActionPreference='Stop'; "
            '$env:PATH="$env:USERPROFILE\\.local\\bin;$env:USERPROFILE\\.cargo\\bin;$env:PATH"; '
        )
        if cwd:
            prefix += f"Set-Location -LiteralPath {ps(cwd)}; "
        return (
            prefix
            + "& "
            + " ".join(ps(a) for a in argv)
            + "; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }"
        )
    prefix = 'export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"; '
    if cwd is not None:
        prefix += f"cd {shlex.quote(cwd)} && "
    return prefix + shlex.join(argv)


def detached_script(host, argv: list[str], directory: str) -> str:
    """The launcher acknowledges creation only; worker terminal data proves exit.

    Windows API contract: Create returns 0 and an out ProcessId. See Microsoft's
    Win32_Process.Create documentation; CIM avoids the OpenSSH session job.
    """
    cross = authorities()
    if host.launcher == cross.DetachedLauncher.WINDOWS_CIM:
        return (
            "$ErrorActionPreference='Stop'; "
            "$result=Invoke-CimMethod -ClassName Win32_Process -MethodName Create "
            "-Arguments @{CommandLine="
            + ps(subprocess.list2cmdline(argv))
            + ";CurrentDirectory="
            + ps(directory)
            + "}; "
            "if ($result.ReturnValue -ne 0 -or $result.ProcessId -le 0) "
            '{ throw "CIM creation refused: $($result.ReturnValue)" }; '
            "$result | Select-Object ReturnValue,ProcessId | ConvertTo-Json -Compress"
        )
    if host.launcher == cross.DetachedLauncher.POSIX_SESSION:
        code = (
            "import json,subprocess,sys; "
            "p=subprocess.Popen(sys.argv[2:],cwd=sys.argv[1],start_new_session=True,"
            "stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); "
            "print(json.dumps({'ReturnValue':0,'ProcessId':p.pid}))"
        )
        return remote_argv(host, uv_python("-c", code, directory, *argv), cwd=directory)
    raise ValueError(f"{host.name}: no admitted detached launcher")


def acknowledge(raw: str) -> int:
    result = json.loads(raw)
    if (
        not isinstance(result, dict)
        or result.get("ReturnValue") != 0
        or type(result.get("ProcessId")) is not int
        or result["ProcessId"] <= 0
    ):
        raise ValueError("detached launcher did not acknowledge a process")
    return result["ProcessId"]


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
    bundle: Path,
    subject: str,
    controller: str,
    checkout: str,
    control: str,
) -> None:
    transport.push([bundle])
    # These are local object reads/imports, not network Git transfers. SSH/SCP
    # use cross_run's guarded transport; no unattended Git credentials are used.
    for argv in (
        [
            "git",
            "-C",
            host.checkout,
            "bundle",
            "verify",
            transport.remote_path(bundle.name),
        ],
        [
            "git",
            "-C",
            host.checkout,
            "bundle",
            "unbundle",
            transport.remote_path(bundle.name),
        ],
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


def start(args, *, landing: dict | None = None, repo: Path = _REPO_ROOT) -> Path:
    cross = authorities()
    host = selected_host(
        args.inventory, args.host, args.cell, landing=landing is not None
    )
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
        "state": "preparing",
    }
    record_path = run_root / "run.json"
    if landing is not None:
        record["landing"] = landing
    write_json(record_path, record)
    print(f"trunk-loop: recovery record {record_path}", flush=True)
    transport.prepare()
    remote_head = transport.shell_command(
        remote_argv(host, ["git", "-C", host.checkout, "rev-parse", "HEAD"])
    )
    if not re.fullmatch(r"[0-9a-f]{40}", remote_head):
        raise ValueError("cannot bind canonical checkout donor")
    checked(["git", "-C", str(repo), "cat-file", "-e", remote_head + "^{commit}"])
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
        str(path_type(control) / "tools" / "trunk_loop.py"),
        "_worker",
        "--record",
        transport.remote_path("run.json"),
    )
    pid = acknowledge(transport.shell_command(detached_script(host, argv, control)))
    record.update(
        state="launched", launcher_pid=pid, launched_at=datetime.now(UTC).isoformat()
    )
    write_json(record_path, record)
    print(
        f"trunk-loop: launched {host.name} pid={pid}; collect --record {record_path}",
        flush=True,
    )
    return record_path


def transfer_for_record(record: dict):
    cross = authorities()
    if record.get("schema") != SCHEMA or not re.fullmatch(
        r"[0-9a-f]{32}", record.get("nonce", "")
    ):
        raise ValueError("invalid run recovery record")
    data = dict(record["host"])
    data["os"] = cross.HostOS(data["os"])
    data["launcher"] = cross.DetachedLauncher(data["launcher"])
    data["proof_cells"] = tuple(data["proof_cells"])
    host = cross.Host(**data)
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


def collect(record_path: Path, *, wait: bool = True) -> int:
    record = json.loads(record_path.read_text(encoding="utf-8"))
    host, transport = transfer_for_record(record)
    marker = transport.remote_path("terminal.json")
    while True:
        script = (
            f"if (Test-Path -LiteralPath {ps(marker)}) {{ Get-Content -Raw -LiteralPath {ps(marker)} }}"
            if host.is_windows
            else f"if test -f {shlex.quote(marker)}; then cat {shlex.quote(marker)}; fi"
        )
        raw = transport.shell_command(script)
        if raw:
            terminal = json.loads(raw)
            break
        if not wait:
            print("trunk-loop: completion unknown; retain stage and collect later")
            return 3
        print(
            f"trunk-loop: {record['run_id']} waiting for terminal receipt", flush=True
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
    if terminal.get("status") not in {"success", "failure", "infrastructure"}:
        raise ValueError("unknown terminal status")
    if terminal["status"] != "infrastructure" and (
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
                or receipt.get("source_tree_state") != "clean"
            ):
                raise ValueError(
                    "collected proof receipt has different or dirty source"
                )
        for item in archive.infolist():
            path = PurePosixPath(item.filename)
            if (
                path.is_absolute()
                or ".." in path.parts
                or not (2 <= len(path.parts) <= (16 if "landing" in record else 2))
                or path.parts[0] not in record["cells"]
                or item.file_size > 512 * 1024 * 1024
            ):
                raise ValueError(f"unexpected result archive member {item.filename}")
            target = root / path.parts[0] / record["run_id"] / Path(*path.parts[1:])
            target.parent.mkdir(parents=True, exist_ok=True)
            content = archive.read(item)
            if target.exists():
                if target.read_bytes() != content:
                    raise ValueError(f"retained evidence changed: {target}")
            else:
                from molt.file_publication import atomic_write_bytes

                atomic_write_bytes(target, content, exclusive=True)
    record.update(state="collected", terminal=terminal)
    write_json(record_path, record)
    print(f"trunk-loop: collected {root}; status={terminal['status']}", flush=True)
    return 0 if terminal["status"] == "success" else 1


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


def worker(record_path: Path) -> int:
    stage = record_path.parent
    # Redirect native file descriptors before loading repository authorities.
    # No PowerShell redirection (which writes UTF-16) and no inherited SSH pipe.
    with (stage / "worker.log").open("ab", buffering=0) as log:
        os.dup2(log.fileno(), 1)
        os.dup2(log.fileno(), 2)
    authorities()
    record = json.loads(record_path.read_text(encoding="utf-8"))
    transfer_for_record(record)
    status = "infrastructure"
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
        status = "success"
        for cell in record["cells"]:
            directory = stage / cell
            directory.mkdir(exist_ok=False)
            argv = uv_python(
                str(Path(record["checkout"]) / "tools" / "proof_plan.py"),
                "--run-command",
                cell,
                "--receipt",
                str(directory / "receipt.json"),
            )
            if "landing" in record:
                argv = uv_python(
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
                or receipt.get("source_tree_state") != "clean"
                or (
                    "landing" not in record
                    and cell not in {c.get("id") for c in receipt.get("commands", [])}
                )
            ):
                raise ValueError(
                    "proof receipt does not bind the selected clean source and cell"
                )
            cells.append({"id": cell, "returncode": returncode, "receipt": True})
            if "landing" in record and receipt.get("status") == "infrastructure":
                status = "infrastructure"
                break
            if receipt.get("execution", {}).get("global_stop_triggered"):
                status = "infrastructure"
                break
            if returncode:
                status = "failure"
                # Cells are independent selections; preserve this failure and
                # continue the next cell, never a dependent command batch.
    except Exception as exc:
        traceback.print_exc()
        status = "infrastructure"
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
    return 0 if status == "success" else 1


def publish_worker_result(record: dict, stage: Path, cells: list, status: str) -> None:
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
    if value and (hasattr(config,"workerinput") or not getattr(config.option,"numprocesses",0)):
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
    setup = sub.add_parser("provision")
    setup.add_argument(
        "--inventory", type=Path, default=_REPO_ROOT / "tools/cross_hosts.toml"
    )
    setup.add_argument("--host", required=True)
    get = sub.add_parser("collect")
    get.add_argument("--record", type=Path, required=True)
    get.add_argument("--no-wait", action="store_true")
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
        record = start(args)
        return 0 if args.no_wait else collect(record)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        print(f"trunk-loop: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
