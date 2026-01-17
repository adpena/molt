import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]


def _base_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    return env


def _run_cli(args: list[str]) -> subprocess.CompletedProcess[str]:
    cmd = [sys.executable, "-m", "molt.cli", *args]
    return subprocess.run(
        cmd,
        cwd=ROOT,
        env=_base_env(),
        capture_output=True,
        text=True,
    )


def _cross_target_triple() -> str | None:
    system = platform.system()
    arch = platform.machine().lower()
    arch_map = {
        "arm64": "aarch64",
        "aarch64": "aarch64",
        "x86_64": "x86_64",
        "amd64": "x86_64",
    }
    mapped = arch_map.get(arch)
    if mapped is None:
        return None
    if system == "Darwin":
        return f"{mapped}-apple-darwin"
    if system == "Linux":
        return f"{mapped}-unknown-linux-gnu"
    return None


def test_cli_doctor_json() -> None:
    res = _run_cli(["doctor", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["schema_version"]
    assert payload["status"] in {"ok", "error"}
    assert isinstance(payload["data"].get("checks"), list)


def test_cli_run_json(tmp_path: Path) -> None:
    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")
    res = _run_cli(["run", "--json", str(script)])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["data"]["returncode"] == 0
    assert "ok" in payload["data"].get("stdout", "")


def test_cli_vendor_dry_run_json() -> None:
    res = _run_cli(["vendor", "--dry-run", "--allow-non-tier-a", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "vendor"
    assert "vendor" in payload["data"]


def test_cli_package_verify_roundtrip(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": ["net"],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    capabilities_path = tmp_path / "caps.json"
    capabilities_path.write_text(json.dumps({"capabilities": ["net"]}))
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--capabilities",
            str(capabilities_path),
            "--json",
        ]
    )
    assert res.returncode == 0
    assert package_path.exists()

    res = _run_cli(
        [
            "verify",
            "--package",
            str(package_path),
            "--require-checksum",
            "--require-deterministic",
            "--capabilities",
            str(capabilities_path),
            "--json",
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"


def test_cli_build_cross_target_with_zig(tmp_path: Path) -> None:
    target_triple = _cross_target_triple()
    if target_triple is None:
        pytest.skip("Cross-target triples are only defined for Darwin/Linux here.")
    if shutil.which("zig") is None:
        pytest.skip("zig is required for cross-target linking.")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for backend compilation.")

    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")
    output = tmp_path / "hello_molt"

    res = _run_cli(
        [
            "build",
            "--target",
            target_triple,
            "--out-dir",
            str(tmp_path),
            "--output",
            str(output),
            "--json",
            str(script),
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"
    assert payload["data"]["target_triple"] == target_triple
    assert Path(payload["data"]["output"]).exists()


def test_cli_completion_bash_json() -> None:
    res = _run_cli(["completion", "--shell", "bash", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "completion"
    assert payload["data"]["shell"] == "bash"
    assert "complete -F _molt_complete" in payload["data"]["script"]


def test_cli_config_json() -> None:
    res = _run_cli(["config", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "config"
    assert payload["status"] == "ok"
    assert "root" in payload["data"]
    assert "sources" in payload["data"]


def test_cli_completion_includes_build_flags() -> None:
    res = _run_cli(["completion", "--shell", "bash", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    script = payload["data"]["script"]
    assert "--emit" in script
    assert "--rebuild" in script
