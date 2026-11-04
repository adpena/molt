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
        def wsl_root_command(self, script):
            assert "systemd=true" in script
            return '{"changed":false,"restart":false}'

        def windows_command(self, script):
            return json.dumps(
                {
                    "config": "[wsl2]\nmemory=24GB\nswap=4GB\nvmIdleTimeout=-1\n",
                    "disk_bytes": 80 * 1024**3,
                    "vhdx": host.wsl_vhdx,
                    "pending": False,
                }
            )

        def shell_command(self, script, *, timeout=60):
            scripts.append(script)
            if "lsblk -bdno SIZE" in script:
                return str(80 * 1024**3)
            if script.startswith("df -Pk / |"):
                return str(2 * 1024**2)
            if "flock -n" in script:
                return ""
            if "udevadm settle" in script:
                return '{"systemd":true,"root_uuid":"root-uuid"}'
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
    assert "uv run --locked molt doctor --json --strict" in install
    assert "--default-toolchain none" in install
    assert "rustup show active-toolchain" in install
    assert "git clone --" in install and "fetch origin" in install
    assert installs[0] == installs[1]
    assert "envs/molt-dx-py312" not in install
    if name == "molt-gate":
        assert any(script.startswith("df -Pk /mnt/c ") for script in scripts)


@pytest.mark.parametrize(
    "changed,restart", [(True, True), (False, True), (False, False)]
)
def test_guest_boot_configuration_restart_and_root_uuid_admission(changed, restart):
    events = []

    class Transport:
        def shell_command(self, script):
            events.append("lock" if "flock -n" in script else "admit")
            return (
                ""
                if "flock -n" in script
                else '{"systemd":true,"root_uuid":"root-uuid"}'
            )

        def wsl_root_command(self, script):
            events.append("configure")
            return json.dumps({"changed": changed, "restart": restart})

        def windows_command(self, script):
            events.append("restart")
            assert "--terminate 'molt-gate'" in script
            assert "--shutdown" not in script and "MoltCodonUbuntu" not in script

    result = trunk_loop.provision_wsl_guest(host_named("molt-gate"), Transport())
    assert events == ["lock", "configure", *(["restart"] if restart else []), "admit"]
    assert result == {
        "systemd": True,
        "root_uuid": "root-uuid",
        "config_changed": changed,
        "restarted": restart,
    }


def test_guest_missing_uuid_mapping_refuses_after_restart():
    class Transport:
        def shell_command(self, script):
            if "flock -n" in script:
                return ""
            raise RuntimeError(
                "Missing root filesystem UUID mapping: /dev/disk/by-uuid/ for /dev/sdd"
            )

        def wsl_root_command(self, script):
            return '{"changed":true,"restart":true}'

        def windows_command(self, script):
            pass

    with pytest.raises(
        RuntimeError, match="Missing root filesystem UUID mapping: /dev/disk/by-uuid/"
    ):
        trunk_loop.provision_wsl_guest(host_named("molt-gate"), Transport())


@pytest.mark.skipif(os.name == "nt", reason="POSIX guest admission script")
def test_guest_uuid_admission_reports_the_actual_missing_mapping():
    from tests.process_guard_common import run_custody_subject_process

    class Transport:
        def shell_command(self, script):
            script = script.replace("$(cat /proc/1/comm)", "systemd")
            script = script.replace("udevadm settle --timeout=30", ":")
            script = script.replace("$(findmnt -nro SOURCE /)", "/dev/missing-root")
            script = script.replace("$(findmnt -nro UUID /)", "missing-root-uuid")
            result = run_custody_subject_process(
                ["bash", "-s"], input=script, text=True, capture_output=True
            )
            assert result.returncode == 2
            assert result.stdout == ""
            assert (
                "Missing root filesystem UUID mapping: /dev/disk/by-uuid/missing-root-uuid for /dev/missing-root"
                in result.stderr
            )
            raise RuntimeError(result.stderr)

    with pytest.raises(RuntimeError, match="Missing root filesystem UUID mapping"):
        trunk_loop.admit_wsl_guest(host_named("molt-gate"), Transport())


@pytest.mark.skipif(os.name == "nt", reason="POSIX configuration publication")
@pytest.mark.parametrize(
    "original",
    [
        "",
        "[network]\nhostname=gate\n",
        "[boot]\nsystemd=false\n[interop]\nenabled=true\n",
        "[boot]\n[network]\nhostname=gate\n",
        "[boot]\nsystemd=false\nsystemd=true\n",
        "[boot]\nsystemd=true\n[boot]\nsystemd=false\n",
    ],
)
def test_guest_configuration_publication_preserves_settings_and_second_run_bytes(
    tmp_path, original
):
    from tests.process_guard_common import run_custody_subject_process

    config = tmp_path / "wsl.conf"
    # Run the real awk/publication script against an owned configuration file.
    # The OS prerequisites and PID 1 are external to this filesystem contract.
    script = trunk_loop.wsl_systemd_script()
    script = script[script.index("config=/etc/wsl.conf") :]
    script = script.replace("/etc/wsl.conf", str(config)).replace(
        "/etc/.molt-wsl.conf.", str(tmp_path / ".molt-wsl.conf.")
    )
    script = script.replace("$(cat /proc/1/comm)", "systemd")
    if original:
        config.write_text(original)
    first = run_custody_subject_process(
        ["bash", "-s"], input=script, text=True, capture_output=True
    )
    if original.count("[boot]") > 1 or original.count("systemd=") > 1:
        assert first.returncode == 2
        assert "Ambiguous [boot] systemd declaration" in first.stderr
        assert config.read_text() == original
        return
    assert first.returncode == 0, first.stderr
    published = config.read_bytes()
    assert b"systemd=true" in published
    assert all(
        line.encode() in published
        for line in original.splitlines()
        if not line.startswith("systemd=")
    )
    before = config.stat()
    second = run_custody_subject_process(
        ["bash", "-s"], input=script, text=True, capture_output=True
    )
    assert second.returncode == 0, second.stderr
    assert json.loads(second.stdout) == {"changed": False, "restart": False}
    assert config.read_bytes() == published
    assert config.stat().st_mtime_ns == before.st_mtime_ns


def test_wsl_root_transport_uses_stdin_and_only_the_inventoried_distro(monkeypatch):
    calls = []

    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        return subprocess.CompletedProcess(argv, 0, '{"changed":true}', "")

    monkeypatch.setattr(cross_run, "_guarded_run", run)
    transport = cross_run.SSHTransport(host_named("molt-gate"))
    script = "printf '%s' \"a'b with spaces\""
    assert transport.wsl_root_command(script) == '{"changed":true}'
    argv, kwargs = calls[0]
    assert argv[-8:] == [
        "wsl.exe",
        "-d",
        "molt-gate",
        "--user",
        "root",
        "--",
        "bash",
        "-s",
    ]
    assert kwargs["input"] == script + "\n"


@pytest.mark.parametrize("used_gib", [61, 81])
def test_used_budget_refuses_before_disk_mutation(used_gib):
    class Transport:
        def windows_command(self, script):
            assert "Get-ChildItem" in script
            return json.dumps(
                {
                    "config": "[wsl2]\nmemory=24GB\nswap=4GB\nvmIdleTimeout=-1\n",
                    "disk_bytes": 1024**4,
                    "pending": False,
                }
            )

        def shell_command(self, script, **_kwargs):
            if "lsblk -bdno SIZE" in script:
                return str(1024**4)
            assert script.startswith("df -Pk / |")
            return str(used_gib * 1024**2)

    with pytest.raises(ValueError, match="used bytes.*exceeds 80 GiB disk budget"):
        trunk_loop.admit_wsl_profile(
            host_named("molt-gate"), Transport(), provision_disk=True
        )


@pytest.mark.parametrize("interrupted", [False, True])
def test_wsl_shrink_trim_compact_then_readmit_and_second_admission_does_nothing(
    interrupted,
):
    events = []
    disk = 1024**4
    pending = False
    fail_compact = interrupted

    class Transport:
        def windows_command(self, script, **kwargs):
            nonlocal disk, pending, fail_compact
            if "Get-ChildItem" in script:
                return json.dumps(
                    {
                        "config": "[wsl2]\nmemory=24GB\nswap=4GB\nvmIdleTimeout=-1\n",
                        "pending": pending,
                    }
                )
            if script == "wsl.exe --version":
                return "WSL version: 3.0.3.0"
            events.append(script)
            assert "sparse" not in script
            assert "MoltCodonUbuntu" not in script
            assert (
                script.index("--terminate")
                < script.index("--list --running --quiet")
                < script.index("if ($running.Count)")
                < script.index("--shutdown")
                < script.index("--manage")
            )
            if "--resize 80GB" in script:
                assert "WriteAllText" in script
                pending = True
                assert "--terminate 'molt-gate'" in script
                disk = 80 * 1024**3
            else:
                assert "--user root -- fstrim -av" in script
                assert (
                    script.index("fstrim")
                    < script.index("--terminate")
                    < script.index("--compact")
                )
                if fail_compact:
                    fail_compact = False
                    raise RuntimeError("compaction interrupted")
                assert "Remove-Item -LiteralPath" in script
                pending = False
            return ""

        def shell_command(self, script):
            if "lsblk -bdno SIZE" in script:
                return str(disk)
            if "flock -n" in script:
                return ""
            assert script.startswith("df -Pk / |")
            return str(2 * 1024**2)

    transport = Transport()
    if interrupted:
        with pytest.raises(RuntimeError, match="compaction interrupted"):
            trunk_loop.admit_wsl_profile(
                host_named("molt-gate"), transport, provision_disk=True
            )
        assert pending
    first = trunk_loop.admit_wsl_profile(
        host_named("molt-gate"), transport, provision_disk=True
    )
    second = trunk_loop.admit_wsl_profile(
        host_named("molt-gate"), transport, provision_disk=True
    )
    assert first == second
    assert first["disk_bytes"] == 80 * 1024**3
    assert len(events) == (3 if interrupted else 2)


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


@pytest.mark.parametrize(
    "memory,version,diagnostic",
    [
        (32, "WSL version: 3.0.3.0", "Windows settings change required"),
        (24, "WSL version: 3.0.2.0\nKernel version: 3.0.3.0", "reviewed WSL 3.0.3"),
    ],
)
def test_wsl_disk_mutation_refuses_unreviewed_profile_or_engine(
    memory, version, diagnostic
):
    class Transport:
        def windows_command(self, script):
            if "Get-ChildItem" in script:
                return json.dumps(
                    {
                        "config": f"[wsl2]\nmemory={memory}GB\nswap=4GB\nvmIdleTimeout=-1\n",
                        "pending": False,
                    }
                )
            assert script == "wsl.exe --version"
            return version

        def shell_command(self, script):
            if "lsblk -bdno SIZE" in script:
                return str(1024**4)
            assert script.startswith("df -Pk / |")
            return str(2 * 1024**2)

    with pytest.raises(ValueError, match=diagnostic):
        trunk_loop.admit_wsl_profile(
            host_named("molt-gate"), Transport(), provision_disk=True
        )


@pytest.mark.parametrize("wsl", [False, True])
def test_worker_capacity_boundary_precedes_tool_installation(
    tmp_path, monkeypatch, wsl
):
    from pathlib import Path
    from molt import checkout_provisioning, disk_capacity, dx

    stage, checkout, tools = (
        tmp_path / name for name in ("stage", "checkout", "tools")
    )
    record = {
        "checkout": str(checkout),
        "host": {"wsl_distribution": "molt-gate" if wsl else None},
        "seeds": [],
    }
    monkeypatch.setattr(trunk_loop, "worker_environment", lambda _record: {})
    monkeypatch.setattr(dx, "canonical_toolchain_root", lambda *_args, **_kwargs: tools)

    def capacity(paths, **_kwargs):
        assert set(paths) == {stage, checkout, tools} | (
            {Path("/mnt/c")} if wsl else set()
        )
        raise RuntimeError("physical capacity refused")

    monkeypatch.setattr(disk_capacity, "require_build_capacity", capacity)
    monkeypatch.setattr(
        checkout_provisioning,
        "ensure",
        lambda *_args, **_kwargs: pytest.fail(
            "capacity must precede tool installation"
        ),
    )
    with pytest.raises(RuntimeError, match="physical capacity refused"):
        trunk_loop.provision(record, stage)


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
@pytest.mark.parametrize("doctor_error", [False, True])
@pytest.mark.skipif(os.name == "nt", reason="POSIX bootstrap script execution")
def test_existing_posix_install_does_no_bootstrap_work(tmp_path, name, doctor_error):
    from tests.process_guard_common import run_custody_subject_process

    root = tmp_path / "host"
    checkout = root / "molt-src"
    checkout.mkdir(parents=True)
    bin_dir = root / ".local/bin"
    bin_dir.mkdir(parents=True)
    log = root / "calls"
    for command in ("uv", "rustup", "git", "curl", "cc", "cmake"):
        executable = bin_dir / command
        executable.write_text(
            "#!/bin/sh\n"
            '[ -z "${UV_PROJECT_ENVIRONMENT:-}${VIRTUAL_ENV:-}${MOLT_VENV:-}" ] || exit 93\n'
            f'printf "%s\\n" "{command} $*" >> "$HOME/calls"\n'
            + (
                'case "$*" in\n'
                ' "run --locked molt doctor --json --strict") exit 7;;\n'
                ' "run --locked molt doctor --json") echo \'{"status":"error"}\';;\n'
                "esac\n"
                if command == "uv" and doctor_error
                else ""
            ),
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
        assert result.returncode == (7 if doctor_error else 0), result.stderr
    calls = log.read_text(encoding="utf-8").splitlines()
    assert (
        calls
        == [
            f"git -C {checkout} fetch origin",
            "rustup show active-toolchain",
            "uv sync --locked --group dev",
            "uv run --locked molt doctor --json --strict",
        ]
        * 2
    )
