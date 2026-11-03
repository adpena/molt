from __future__ import annotations

from dataclasses import replace
import io
import json
import os
import subprocess
import tarfile
from types import SimpleNamespace

import pytest

from tools import cross_run, trunk_loop


def host_named(name):
    return next(
        h
        for h in cross_run.parse_inventory(cross_run.DEFAULT_INVENTORY)
        if h.name == name
    )


@pytest.mark.parametrize("name", ["macmini", "bat00", "molt-gate"])
def test_provision_transport_runs_documented_workflow_twice(name, monkeypatch):
    host = host_named(name)
    scripts = []

    class Transport:
        def windows_command(self, script):
            return json.dumps(
                {
                    "config": "[wsl2]\nmemory=24GB\nswap=4GB\nvmIdleTimeout=-1\n",
                    "disk_bytes": 80 * 1024**3,
                    "vhdx": host.wsl_vhdx,
                }
            )

        def shell_command(self, script, *, timeout=60):
            scripts.append(script)
            if "lsblk -bdno SIZE" in script:
                return str(80 * 1024**3)
            if script.startswith("$d=Get-PSDrive"):
                return json.dumps({"free": 30 * 1024**3})
            if script.startswith(("df -Pk", "p=")):
                return str(30 * 1024**2)
            if "cache_retention" in script:
                return '{"cache":"/host/cache","epoch":null,"returncode":null}'
            assert timeout == 1800
            return '{"ok":true}'

    monkeypatch.setattr(
        trunk_loop, "checked", lambda _argv: "https://github.com/adpena/molt.git"
    )
    args = SimpleNamespace(inventory=cross_run.DEFAULT_INVENTORY, host=name)
    first = trunk_loop.provision_host(args, transport=Transport())
    second = trunk_loop.provision_host(args, transport=Transport())
    assert first == second
    installs = [script for script in scripts if "uv sync --locked" in script]
    assert len(installs) == 2
    install = installs[0]
    assert "uv sync --locked --group dev" in install
    assert "uv run --locked molt doctor --json" in install
    assert "--default-toolchain none" in install
    assert "rustup show active-toolchain" in install
    assert "git clone --" in install and "fetch origin" in install
    assert installs[0] == installs[1]
    assert "envs/molt-dx-py312" not in install
    if name == "molt-gate":
        assert any(script.startswith("df -Pk /mnt/c ") for script in scripts)


def test_settings_stop_precedes_install_and_capacity(monkeypatch):
    class Transport:
        def windows_command(self, _script):
            return json.dumps(
                {
                    "config": "[wsl2]\nmemory=24GB\nswap=4GB\nvmIdleTimeout=-1\n",
                    "disk_bytes": 1024**4,
                }
            )

        def shell_command(self, script, **_kwargs):
            # Even if a filesystem is shrunk separately, its block device is
            # still the full 1 TiB: only the latter can prove the VHD cap.
            assert "lsblk -bdno SIZE" in script
            assert "findmnt -bno SIZE" not in script
            return str(1024**4)

    with pytest.raises(
        ValueError, match="Windows settings change required.*--resize 80GB"
    ):
        trunk_loop.provision_host(
            SimpleNamespace(inventory=cross_run.DEFAULT_INVENTORY, host="molt-gate"),
            transport=Transport(),
        )


def test_low_physical_space_refuses_before_install():
    class Transport:
        def shell_command(self, script, **_kwargs):
            assert script.startswith("$d=Get-PSDrive")
            return json.dumps({"free": 19 * 1024**3})

    with pytest.raises(ValueError, match="20 GiB proof headroom"):
        trunk_loop.provision_host(
            SimpleNamespace(inventory=cross_run.DEFAULT_INVENTORY, host="bat00"),
            transport=Transport(),
        )


def test_wsl_scripts_cross_windows_only_on_stdin(monkeypatch):
    transport = cross_run.SSHTransport(host_named("molt-gate"))
    calls = []

    def execute(argv, **kwargs):
        calls.append((argv, kwargs))
        return subprocess.CompletedProcess(
            argv, 0, stdout="literal result\n", stderr=""
        )

    monkeypatch.setattr(cross_run, "_guarded_run", execute)
    script = "printf '%s' \"apostrophe ' dollar $ slash \\\"\n"
    assert transport.shell_command(script) == "literal result"
    argv, kwargs = calls[0]
    assert argv[-6:] == ["wsl.exe", "-d", "molt-gate", "--", "bash", "-s"]
    assert script not in argv
    assert kwargs["input"] == script + "\n"


def test_wsl_transfer_preserves_binary_bytes_and_leaf_identity(tmp_path, monkeypatch):
    transport = cross_run.SSHTransport(host_named("molt-gate"))
    source = tmp_path / "source.bundle"
    payload = bytes(range(256)) * 4
    source.write_bytes(payload)
    calls = []

    def execute(argv, **kwargs):
        calls.append((argv, kwargs))
        if argv[-2] == "cat":
            return subprocess.CompletedProcess(argv, 0, stdout=payload, stderr=b"")
        with tarfile.open(fileobj=io.BytesIO(kwargs["input"])) as archive:
            assert archive.getnames() == ["source.bundle"]
            assert archive.extractfile("source.bundle").read() == payload
        return subprocess.CompletedProcess(argv, 0, stdout=b"", stderr=b"")

    monkeypatch.setattr(cross_run, "_guarded_run", execute)
    transport.push([source])
    destination = tmp_path / "collected.bundle"
    transport.pull("source.bundle", destination)
    assert destination.read_bytes() == payload
    assert all(kwargs["text"] is False for _, kwargs in calls)
    with pytest.raises(ValueError, match="literal stage filename"):
        transport.pull("../elsewhere", destination)


@pytest.mark.parametrize(
    "field,value",
    [
        ("wsl_vhdx", "E:\\Molt\\gate.vhdx"),
        ("wsl_distribution", "gate;echo"),
        ("wsl_disk_gb", 10),
        ("wsl_memory_gb", True),
    ],
)
def test_inventory_rejects_invalid_wsl_custody(tmp_path, field, value):
    import tomllib

    # Mutate the real inventory at its serialization boundary, preserving siblings.
    text = cross_run.DEFAULT_INVENTORY.read_text(encoding="utf-8")
    current = tomllib.loads(text)["host"][-1][field]
    if isinstance(current, str):
        line = next(line for line in text.splitlines() if line.startswith(field + " ="))
        text = text.replace(line, field + " = " + json.dumps(value))
    else:
        text = text.replace(f"{field} = {current}", f"{field} = {str(value).lower()}")
    inventory = tmp_path / "hosts.toml"
    inventory.write_text(text, encoding="utf-8")
    with pytest.raises(ValueError, match="invalid WSL"):
        cross_run.parse_inventory(inventory)


def test_inventory_rejects_retired_python_contract(tmp_path):
    inventory = tmp_path / "hosts.toml"
    inventory.write_text(
        cross_run.DEFAULT_INVENTORY.read_text(encoding="utf-8")
        + '\npython = "/handmade/python"\n',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="Python is owned by uv"):
        cross_run.parse_inventory(inventory)


@pytest.mark.parametrize("name", ["macmini", "molt-gate"])
@pytest.mark.skipif(os.name == "nt", reason="POSIX bootstrap script execution")
def test_existing_posix_install_does_no_bootstrap_work(tmp_path, name):
    from tests.process_guard_common import run_custody_subject_process

    root = tmp_path / "host"
    checkout = root / "molt-src"
    checkout.mkdir(parents=True)
    bin_dir = root / ".local/bin"
    bin_dir.mkdir(parents=True)
    log = root / "calls"
    for command in ("uv", "rustup", "git", "curl", "cc"):
        executable = bin_dir / command
        executable.write_text(
            "#!/bin/sh\n"
            '[ -z "${UV_PROJECT_ENVIRONMENT:-}${VIRTUAL_ENV:-}${MOLT_VENV:-}" ] || exit 93\n'
            f'printf "%s\\n" "{command} $*" >> "$HOME/calls"\n',
            encoding="utf-8",
        )
        executable.chmod(0o755)
    host = replace(host_named(name), custody_root=str(root), checkout=str(checkout))
    script = trunk_loop.provision_script(
        host, repository="https://example.test/repo.git", uv_version="0.13.5"
    )
    env = dict(
        os.environ,
        HOME=str(root),
        UV_PROJECT_ENVIRONMENT="/foreign-image",
        VIRTUAL_ENV="/handmade",
        MOLT_VENV="/handmade",
    )
    for _ in range(2):
        result = run_custody_subject_process(
            ["/bin/sh"], input=script, text=True, capture_output=True, env=env
        )
        assert result.returncode == 0, result.stderr
    calls = log.read_text(encoding="utf-8").splitlines()
    assert (
        calls
        == [
            f"git -C {checkout} fetch origin",
            "rustup show active-toolchain",
            "uv sync --locked --group dev",
            "uv run --locked molt doctor --json",
        ]
        * 2
    )
