#!/usr/bin/env python3
"""Run exact-source proof-plan commands on an inventoried farm host.

Stage 1: explicit run/collect; no scheduler, ledger mutation or automatic revert.
See docs/design/trunk_loop.md. The controller and tested commit are independent
identities: an older main is never modified to install this runner.
"""

from __future__ import annotations

import argparse
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

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "molt.trunk-run.v1"


def authorities():
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))
    from tools.import_file import bind_repository_imports

    bind_repository_imports(__file__)
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


def remote_argv(host, argv: list[str], *, cwd: str | None = None) -> str:
    if host.is_windows:
        prefix = "$ErrorActionPreference='Stop'; "
        if cwd:
            prefix += f"Set-Location -LiteralPath {ps(cwd)}; "
        return (
            prefix
            + "& "
            + " ".join(ps(a) for a in argv)
            + "; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }"
        )
    prefix = "" if cwd is None else f"cd {shlex.quote(cwd)} && "
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
        return shlex.join([host.python, "-B", "-c", code, directory, *argv])
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


def checked(argv: list[str], *, cwd: Path = ROOT, timeout: int = 180) -> str:
    result = authorities()._guarded_run(argv, cwd=cwd, timeout=timeout)
    if result.returncode:
        raise RuntimeError(
            f"{shlex.join(argv)}: exit {result.returncode}\n{result.stderr[-4000:]}\n{result.stdout[-4000:]}"
        )
    return result.stdout.strip()


def exact_commit(revision: str) -> str:
    # Resolve before crossing a shell or using the value as an evidence path.
    commit = checked(
        ["git", "-C", str(ROOT), "rev-parse", "--verify", revision + "^{commit}"]
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


def selected_host(inventory: Path, name: str, cells: list[str]):
    cross = authorities()
    host = next((h for h in cross.parse_inventory(inventory) if h.name == name), None)
    if host is None:
        raise ValueError(f"unknown host {name!r}")
    if (
        host.transport != "ssh"
        or not host.python
        or not host.launcher
        or not host.proof_cells
    ):
        raise ValueError(f"{name}: no admitted SSH proof host configuration")
    if not cells or len(set(cells)) != len(cells) or set(cells) - set(host.proof_cells):
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
cells=sys.argv[1:]
commands={c.id:c for cell in cells for c in _topological_commands(plan,command_id=cell)}
names=selected_toolchains(plan,commands=cells,family=None,matrix_cell=None,toolchains=[])
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


def describe(host, transport, checkout: str, cells: list[str]) -> dict:
    raw = transport.shell_command(
        remote_argv(host, [host.python, "-B", "-c", DESCRIBE, *cells], cwd=checkout),
        timeout=180,
    )
    result = json.loads(raw)
    if set(cells) - set(result["commands"]):
        raise ValueError("subject policy omitted a selected command")
    return result


def seed_inputs(descriptor: dict, transport, local: Path) -> list[dict]:
    from molt.dx import canonical_toolchain_root
    from molt.tool_releases import provision_archive

    cache = (
        canonical_toolchain_root(ROOT, require_exists=False)
        / "toolchains"
        / "downloads"
        / "sha256"
    )
    seeds = []
    for name, item in descriptor["releases"].items():
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


def start(args) -> Path:
    cross = authorities()
    host = selected_host(args.inventory, args.host, args.cell)
    subject = exact_commit(args.commit)
    controller = exact_commit("HEAD")
    # The controller must be committed: copying dirty source would make the
    # worker identity fictional. Test the clean named controller, not a tar overlay.
    if checked(
        ["git", "-C", str(ROOT), "status", "--porcelain", "--untracked-files=no"]
    ):
        raise ValueError("commit controller changes before a farm run")
    checked(
        ["git", "-C", str(ROOT), "merge-base", "--is-ancestor", subject, controller]
    )
    transport = cross.SSHTransport(host)
    checkout, control = host_paths(host, transport._nonce)
    from molt.dx import artifact_root

    run_id = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ") + "-" + transport._nonce
    run_root = (
        artifact_root(ROOT)
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
    write_json(record_path, record)
    print(f"trunk-loop: recovery record {record_path}", flush=True)
    transport.prepare()
    remote_head = transport.shell_command(
        remote_argv(host, ["git", "-C", host.checkout, "rev-parse", "HEAD"])
    )
    if not re.fullmatch(r"[0-9a-f]{40}", remote_head):
        raise ValueError("cannot bind canonical checkout donor")
    checked(["git", "-C", str(ROOT), "cat-file", "-e", remote_head + "^{commit}"])
    bundle = run_root / "source.bundle"
    checked(
        [
            "git",
            "-C",
            str(ROOT),
            "bundle",
            "create",
            str(bundle),
            "HEAD",
            "^" + remote_head,
        ],
        timeout=600,
    )
    prepare_checkouts(host, transport, bundle, subject, controller, checkout, control)
    descriptor = describe(host, transport, checkout, args.cell)
    record["selection"] = descriptor
    record["seeds"] = seed_inputs(descriptor, transport, run_root)
    write_json(record_path, record)
    transport.push([record_path])
    path_type = PureWindowsPath if host.is_windows else PurePosixPath
    argv = [
        host.python,
        "-B",
        str(path_type(control) / "tools" / "trunk_loop.py"),
        "_worker",
        "--record",
        transport.remote_path("run.json"),
    ]
    pid = acknowledge(
        transport.shell_command(detached_script(host, argv, transport.execution_dir))
    )
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
    record = json.loads(record_path.read_text())
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
                or len(path.parts) != 2
                or path.parts[0] not in record["cells"]
                or item.file_size > 512 * 1024 * 1024
            ):
                raise ValueError(f"unexpected result archive member {item.filename}")
            target = root / path.parts[0] / record["run_id"] / path.name
            target.parent.mkdir(parents=True, exist_ok=True)
            content = archive.read(item)
            if target.exists():
                if target.read_bytes() != content:
                    raise ValueError(f"retained evidence changed: {target}")
            else:
                with target.open("xb") as stream:
                    stream.write(content)
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
    baseline["CARGO_HOME"] = str(root / ".cargo")
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
        parent = resolve_owned_path(Path(selected))
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


def provision(record: dict, stage: Path) -> tuple[Path, dict[str, str]]:
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

    require_build_capacity((stage, checkout, tool_root), env=env)
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
        "uv-environment", checkout, env, guard_source_root=ROOT
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
        "n=selected_toolchains(p,commands=sys.argv[1:],family=None,matrix_cell=None,toolchains=[]); "
        "a=admit_toolchains(p.toolchain_policies,n,python_executable=pathlib.Path(sys.executable),environ=os.environ,root=pathlib.Path.cwd()); "
        "print(json.dumps({'toolchains':n,'fingerprints':a.fingerprints}))"
    )
    result = authorities()._guarded_run(
        [str(python), "-B", "-c", code, *record["cells"]],
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
    return python, env


def worker(record_path: Path) -> int:
    stage = record_path.parent
    # Redirect native file descriptors before loading repository authorities.
    # No PowerShell redirection (which writes UTF-16) and no inherited SSH pipe.
    with (stage / "worker.log").open("ab", buffering=0) as log:
        os.dup2(log.fileno(), 1)
        os.dup2(log.fileno(), 2)
    authorities()
    record = json.loads(record_path.read_text())
    transfer_for_record(record)
    status = "infrastructure"
    cells = []
    lock = None
    try:
        if (stage / ".molt-owner").read_text().strip() != record["nonce"]:
            raise ValueError("remote staging ownership changed")
        from molt.file_locks import _acquire_file_lock

        lock = _acquire_file_lock(
            stage.parent / "trunk-worker.lock",
            timeout_s=0,
            timeout_message="another trunk worker owns this host",
        )
        for path, commit in (
            (Path(record["checkout"]), record["commit"]),
            (ROOT, record["controller_commit"]),
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
        python, env = provision(record, stage)
        (stage / "trunk_timings.py").write_text(TIMINGS_PLUGIN, encoding="utf-8")
        env["PYTHONPATH"] += os.pathsep + str(stage)
        status = "success"
        for cell in record["cells"]:
            directory = stage / cell
            directory.mkdir(exist_ok=False)
            argv = [
                str(python),
                "-B",
                str(Path(record["checkout"]) / "tools" / "proof_plan.py"),
                "--run-command",
                cell,
                "--receipt",
                str(directory / "receipt.json"),
            ]
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
            with (directory / "run.log").open("wb") as log:
                result = subprocess.run(
                    argv,
                    cwd=record["checkout"],
                    env=child,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                )
            (directory / "rc").write_text(
                str(result.returncode) + "\n", encoding="ascii"
            )
            receipt = json.loads((directory / "receipt.json").read_text())
            if (
                receipt.get("source_commit") != record["commit"]
                or receipt.get("source_tree_state") != "clean"
                or cell not in {c.get("id") for c in receipt.get("commands", [])}
            ):
                raise ValueError(
                    "proof receipt does not bind the selected clean source and cell"
                )
            cells.append({"id": cell, "returncode": result.returncode, "receipt": True})
            if receipt.get("execution", {}).get("global_stop_triggered"):
                status = "infrastructure"
                break
            if result.returncode:
                status = "failure"
                # Cells are independent selections; preserve this failure and
                # continue the next cell, never a dependent command batch.
    except BaseException:
        traceback.print_exc()
        status = "infrastructure"
    finally:
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
                for item in sorted((stage / cell).iterdir()):
                    if item.is_file():
                        output.write(item, cell + "/" + item.name)
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
        if lock is not None:
            from molt.file_locks import _release_file_lock

            _release_file_lock(lock)
    return 0 if status == "success" else 1


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
        "--inventory", type=Path, default=ROOT / "tools" / "cross_hosts.toml"
    )
    run.add_argument("--host", required=True)
    run.add_argument("--commit", required=True)
    run.add_argument("--cell", action="append", required=True)
    run.add_argument("--no-wait", action="store_true")
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
        if args.action == "collect":
            return collect(args.record, wait=not args.no_wait)
        record = start(args)
        return 0 if args.no_wait else collect(record)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        print(f"trunk-loop: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
