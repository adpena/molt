from __future__ import annotations

import importlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib
import zipfile

import pytest

from molt.cli import lockfiles, project_roots
from tests.cli.process_guard import run_cli_test_process

deps = importlib.import_module("molt.cli.deps")
ROOT = Path(__file__).resolve().parents[2]


@pytest.mark.parametrize("entry", ["directory", "file"])
def test_unmarked_project_stays_in_the_selected_directory(tmp_path, monkeypatch, entry):
    monkeypatch.delenv("MOLT_PROJECT_ROOT", raising=False)
    start = tmp_path if entry == "directory" else tmp_path / "main.py"
    assert project_roots._find_project_root(start) == tmp_path


def test_invalid_override_is_not_replaced_with_a_valid_project(
    tmp_path, monkeypatch, capsys
):
    (tmp_path / "pyproject.toml").write_text(
        "[project]\nname='valid'\n", encoding="utf-8"
    )
    missing = tmp_path / "missing"
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(missing))
    assert deps.deps(False, json_output=True) == 2
    assert str(missing) in json.loads(capsys.readouterr().out)["errors"][0]


@pytest.mark.parametrize("operation", ["install", "add", "deps", "vendor"])
def test_package_commands_cannot_mutate_a_sealed_compiler(
    tmp_path, monkeypatch, capsys, operation
):
    (tmp_path / "release-compiler-source.json").write_text("{}", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        "[project]\nname='compiler'\n", encoding="utf-8"
    )
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(tmp_path))
    if operation == "install":
        status = deps.install(["example"], json_output=True)
    elif operation == "add":
        status = deps.install_add(["example"], json_output=True)
    elif operation == "deps":
        status = deps.deps(False, json_output=True)
    else:
        status = deps.vendor(False, json_output=True)
    assert status == 2
    assert "immutable" in json.loads(capsys.readouterr().out)["errors"][0]
    assert not (tmp_path / ".molt-venv").exists()


def test_dependency_environment_requires_a_real_interpreter(tmp_path):
    (tmp_path / ".molt-venv").mkdir()
    with pytest.raises(RuntimeError, match="Invalid project environment"):
        deps._ensure_molt_venv(tmp_path)


def test_failed_add_cannot_claim_installed_and_persisted(tmp_path, monkeypatch, capsys):
    (tmp_path / "pyproject.toml").write_text(
        "[project]\nname='guest'\n", encoding="utf-8"
    )
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setenv("UV_NO_SYNC", "1")
    monkeypatch.setenv("UV_FROZEN", "1")
    monkeypatch.setattr(deps, "_ensure_uv", lambda: "uv")
    venv = tmp_path / ".molt-venv"
    monkeypatch.setattr(deps, "_ensure_molt_venv", lambda *a, **kw: (venv, False))

    def failed(cmd, **kwargs):
        assert cmd[1] == "add"
        assert Path(kwargs["env"]["UV_PROJECT_ENVIRONMENT"]) == venv
        assert "UV_NO_SYNC" not in kwargs["env"]
        assert "UV_FROZEN" not in kwargs["env"]
        return subprocess.CompletedProcess(cmd, 23, "", "cannot persist project")

    monkeypatch.setattr(deps, "_run_completed_command", failed)
    assert deps.install_add(["example"], json_output=True) == 2
    result = json.loads(capsys.readouterr().out)
    assert result["status"] == "error"
    assert "exit 23" in result["errors"][0]


@pytest.mark.parametrize("cargo", [False, True])
def test_locks_follow_the_declared_project_not_the_compiler(
    tmp_path, monkeypatch, capsys, cargo
):
    (tmp_path / "pyproject.toml").write_text(
        "[project]\nname='guest'\n", encoding="utf-8"
    )
    (tmp_path / "uv.lock").write_text("version=1\n", encoding="utf-8")
    monkeypatch.delenv("UV_NO_SYNC", raising=False)
    monkeypatch.delenv("MOLT_SKIP_CARGO_LOCK", raising=False)
    calls = []
    monkeypatch.setattr(lockfiles, "_verify_uv_lock", lambda root: calls.append(root))
    monkeypatch.setattr(
        lockfiles,
        "_verify_cargo_lock",
        lambda root: pytest.fail("unexpected compiler/Cargo dependency resolution"),
    )
    if cargo:
        (tmp_path / "Cargo.toml").write_text(
            "[package]\nname='guest'\n", encoding="utf-8"
        )
    status = lockfiles._check_lockfiles(tmp_path, True, [], True, False, "vendor")
    if cargo:
        assert status == 2
        assert "Cargo.lock" in json.loads(capsys.readouterr().out)["errors"][0]
    else:
        assert status is None
        assert calls == [tmp_path]


def _wheel(root: Path, name: str, dependency: str = "") -> Path:
    path = root / f"{name}-1.0-py3-none-any.whl"
    info = f"{name}-1.0.dist-info"
    with zipfile.ZipFile(path, "w") as wheel:
        wheel.writestr(f"{name}.py", f"VALUE = {name!r}\n")
        wheel.writestr(
            f"{info}/METADATA",
            f"Metadata-Version: 2.1\nName: {name}\nVersion: 1.0\n"
            + (f"Requires-Dist: {dependency}\n" if dependency else ""),
        )
        wheel.writestr(
            f"{info}/WHEEL",
            "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        )
        wheel.writestr(f"{info}/RECORD", "")
    return path


def test_public_dependency_workflow_uses_only_the_user_project(tmp_path, monkeypatch):
    assert shutil.which("uv"), "The repository dependency workflow requires uv."
    project = tmp_path / "guest project"
    nested = project / "src"
    nested.mkdir(parents=True)
    compiler = ROOT
    sentinel = compiler / "pyproject.toml"
    compiler_bytes = sentinel.read_bytes()
    compiler_venv_exists = (compiler / ".molt-venv").exists()
    pyproject = project / "pyproject.toml"
    pyproject.write_text(
        "[project]\nname='guest'\nversion='1.0'\nrequires-python='>=3.12'\ndependencies=['payload']\n",
        encoding="utf-8",
    )
    payload = project / "payload"
    payload.mkdir()
    (payload / "value.py").write_text("VALUE=42\n", encoding="utf-8")
    (project / "uv.lock").write_text(
        "version=1\n[[package]]\nname='payload'\nversion='1.0'\n"
        "[package.source]\npath='payload'\n",
        encoding="utf-8",
    )
    env = os.environ.copy()
    env.update(
        {
            "PYTHONPATH": str(ROOT / "src"),
            "PYTHONHASHSEED": "0",
            "MOLT_SOURCE_ROOT": str(compiler),
            "UV_OFFLINE": "1",
            "UV_NO_SYNC": "1",
            "UV_PYTHON_DOWNLOADS": "never",
            "UV_CACHE_DIR": str(tmp_path / "uv-cache"),
        }
    )
    env.pop("MOLT_PROJECT_ROOT", None)
    env.pop("UV_PROJECT", None)

    def cli(*args):
        result = run_cli_test_process(
            [sys.executable, "-B", "-m", "molt.cli", *args, "--json"],
            cwd=nested,
            env=env,
            timeout=90,
        )
        if result.returncode != 0:
            pytest.fail(f"CLI {args}: {result.stdout} {result.stderr}")
        return json.loads(result.stdout)["data"]

    assert cli("deps")["dependencies"][0]["name"] == "payload"
    cli("vendor", "--no-deterministic")
    assert (project / "vendor/local/payload/value.py").read_text(
        encoding="utf-8"
    ) == "VALUE=42\n"
    assert not (nested / "vendor").exists()

    pyproject.write_text(
        "[project]\nname='guest'\nversion='1.0'\nrequires-python='>=3.12'\ndependencies=[]\n",
        encoding="utf-8",
    )
    # The vendor fixture above is a source-plan lock, not a uv-generated lock.
    # The real add operation starts without it and must generate its own lock.
    (project / "uv.lock").unlink()
    _wheel(project, "molt_project_leaf")
    demo = _wheel(project, "molt_project_demo", "molt_project_leaf==1.0")
    noise = _wheel(project, "molt_project_noise")
    requirements = project / "requirements.txt"
    requirements.write_text("-r included.txt\n", encoding="utf-8")
    (project / "included.txt").write_text(
        "--find-links " + project.as_uri() + "\n" + demo.as_uri() + "\n",
        encoding="utf-8",
    )
    cli("install", "-r", str(requirements))
    cli("install", "../" + noise.name)
    site = (
        project / ".molt-venv/Lib/site-packages"
        if os.name == "nt"
        else next((project / ".molt-venv/lib").glob("python*/site-packages"))
    )
    assert (site / "molt_project_demo.py").exists()
    assert (site / "molt_project_leaf.py").exists()
    assert (site / "molt_project_noise.py").exists()
    assert cli("install")["sync"] is True
    assert (site / "molt_project_demo.py").exists()
    assert (site / "molt_project_leaf.py").exists()
    assert not (site / "molt_project_noise.py").exists()

    cli("install", "add", "../" + noise.name)
    data = tomllib.loads(pyproject.read_text(encoding="utf-8"))
    assert any("molt-project-noise" in item for item in data["project"]["dependencies"])
    assert (site / "molt_project_noise.py").exists()
    assert (project / "uv.lock").exists()
    # Replay the same workflow after persistence: uv source mappings and locked
    # local artifacts must survive while additional requirements remain admitted.
    cli("install")
    assert (site / "molt_project_demo.py").exists()
    assert (site / "molt_project_leaf.py").exists()
    assert (site / "molt_project_noise.py").exists()
    assert sentinel.read_bytes() == compiler_bytes
    assert (compiler / ".molt-venv").exists() == compiler_venv_exists
    assert not (project / ".venv").exists()

    requirements.write_text("", encoding="utf-8")
    pyproject.write_text(
        "[project]\nname='guest'\nversion='1.0'\nrequires-python='>=3.12'\ndependencies=[]\n",
        encoding="utf-8",
    )
    cli("install", "--sync")
    assert not (site / "molt_project_demo.py").exists()
    assert not (site / "molt_project_leaf.py").exists()
    assert not (site / "molt_project_noise.py").exists()
