from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import sys

import pytest

from molt import compiler_distribution as distribution
from molt.cli import backend_binary, backend_execution, cargo_profiles
from molt.verified_subset import current_host_coordinate


@pytest.fixture
def installation(tmp_path: Path, monkeypatch) -> Path:
    source = tmp_path / "source"
    source.mkdir()
    files = []
    for name in (
        "Cargo.lock",
        "Cargo.toml",
        "pyproject.toml",
        "runtime/molt-backend/Cargo.toml",
        "runtime/molt-runtime/Cargo.toml",
        "src/molt/cli/__init__.py",
        "src/molt/compiler_distribution.py",
        "uv.lock",
    ):
        path = source / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"source")
        files.append(
            {
                "path": name,
                "mode": 0o100644,
                "blob_oid": "c" * 40,
                "size": 6,
                "sha256": hashlib.sha256(b"source").hexdigest(),
            }
        )
    system, arch = current_host_coordinate()
    binary = (
        tmp_path
        / "bin"
        / ("molt-backend.exe" if system == "windows" else "molt-backend")
    )
    binary.parent.mkdir()
    binary.write_bytes(b"compiler")
    launcher = binary.parent / ("molt.exe" if system == "windows" else "molt")
    launcher.write_bytes(b"launcher")
    payload = {
        "schema": distribution.MANIFEST_SCHEMA,
        "git": {"object_format": "sha1", "commit": "a" * 40, "tree": "b" * 40},
        "files": files,
        "compiler": {
            "path": "bin/" + binary.name,
            "size": 8,
            "sha256": hashlib.sha256(b"compiler").hexdigest(),
            "profile": "release",
            "features": list(distribution.PRODUCTION_COMPILER_FEATURES),
            "platform": system,
            "arch": arch,
        },
        "launcher": {
            "path": "bin/" + launcher.name,
            "size": 8,
            "sha256": hashlib.sha256(b"launcher").hexdigest(),
            "platform": system,
            "arch": arch,
        },
        "runtime": {
            "schema": distribution.RUNTIME_INVENTORY_SCHEMA,
            "platform": system,
            "arch": arch,
            "source": {"object_format": "sha1", "commit": "a" * 40, "tree": "b" * 40},
            "cells": [],
        },
    }
    (source / distribution.MANIFEST_NAME).write_text(
        json.dumps(payload), encoding="utf-8"
    )
    # Model a bundle executing its own sealed package. Separate wheel tests
    # exercise admission of a package outside the bundle's source directory.
    monkeypatch.setattr(
        distribution, "__file__", str(source / "src/molt/compiler_distribution.py")
    )
    return source


@pytest.mark.parametrize("guest", ["dev", "release"])
def test_host_profile_does_not_inherit_guest_overrides(monkeypatch, guest):
    monkeypatch.delenv("MOLT_BACKEND_PROFILE", raising=False)
    monkeypatch.delenv("MOLT_RELEASE_BACKEND_CARGO_PROFILE", raising=False)
    monkeypatch.setenv(f"MOLT_{guest.upper()}_CARGO_PROFILE", "guest-size")
    host, error = cargo_profiles._resolve_backend_profile()
    assert (host, error) == ("release", None)
    assert cargo_profiles._resolve_backend_cargo_profile_name(host) == ("release", None)
    assert cargo_profiles._resolve_cargo_profile_name(guest) == ("guest-size", None)


def test_all_installed_backends_share_one_admitted_compiler(installation, monkeypatch):
    monkeypatch.setattr(
        backend_binary,
        "_backend_fingerprint",
        lambda *a, **k: pytest.fail("installed compiler entered source rebuild"),
    )
    identities = set()
    for feature in distribution.PRODUCTION_COMPILER_FEATURES:
        binary = backend_execution._backend_bin_path(
            installation, "release", (feature,)
        )
        result = backend_binary._ensure_backend_binary(
            binary,
            cargo_timeout=1,
            json_output=True,
            cargo_profile="release",
            project_root=installation,
            backend_features=(feature,),
        )
        assert result.ok, result.message
        identities.add(result.cache_compiler_fingerprint)
    assert len(identities) == 1


def test_installation_rejects_damage_even_when_rebuild_is_skipped(
    installation, monkeypatch
):
    installed = distribution.installed_compiler(installation)
    assert installed is not None
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    installed.binary.write_bytes(b"damaged!")
    result = backend_binary._ensure_backend_binary(
        installed.binary,
        cargo_timeout=1,
        json_output=True,
        cargo_profile="release",
        project_root=installation,
        backend_features=("native-backend",),
    )
    assert not result.ok and result.phase == "installed_compiler"
    assert "differs" in result.message


@pytest.mark.parametrize("damage", ["file", "extra", "empty-directory", "missing"])
def test_installed_source_closure_fails_closed(installation, damage):
    installed = distribution.installed_compiler(installation)
    assert installed is not None
    installed.verify_sources()
    if damage == "file":
        (installation / "Cargo.toml").write_bytes(b"edited")
    elif damage == "extra":
        (installation / "extra.py").write_text("pass")
    elif damage == "empty-directory":
        (installation / "extra").mkdir()
    else:
        (installation / "Cargo.toml").unlink()
    with pytest.raises(ValueError):
        installed.verify_sources()


@pytest.mark.parametrize(
    "recorded,installed,accepted",
    [
        (0o100644, 0o640, True),
        (0o100644, 0o600, True),
        (0o100755, 0o750, True),
        (0o100755, 0o700, True),
        (0o100644, 0o755, False),
        (0o100644, 0o664, False),
        (0o100755, 0o775, False),
        (0o100755, 0o644, False),
        (0o100755, 0o4755, False),
        (0o100644, 0o2644, False),
    ],
)
def test_installed_source_mode_accepts_umask_not_added_access(
    recorded, installed, accepted
):
    assert distribution._source_mode_matches_git(installed, recorded) is accepted


def test_guest_project_discovery_starts_at_entry_without_launcher_override(
    tmp_path, monkeypatch
):
    from molt.cli import project_roots

    project = tmp_path / "project"
    entry = project / "src" / "app.py"
    entry.parent.mkdir(parents=True)
    entry.write_text("print('hello')\n")
    (project / "pyproject.toml").write_text("[tool.molt]\n")
    unrelated_cwd = tmp_path / "unrelated"
    unrelated_cwd.mkdir()
    monkeypatch.chdir(unrelated_cwd)
    monkeypatch.delenv("MOLT_PROJECT_ROOT", raising=False)
    assert project_roots._find_project_root(entry) == project
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(unrelated_cwd))
    assert project_roots._find_project_root(entry) == unrelated_cwd


@pytest.mark.parametrize(
    "profile,features", [("dev-fast", ("native-backend",)), ("release", ("llvm",))]
)
def test_installed_compiler_never_silently_rebuilds_an_unshipped_variant(
    installation, profile, features
):
    installed = distribution.installed_compiler(installation)
    assert installed is not None
    with pytest.raises(ValueError):
        installed.verify_binary(features, profile)


def test_installed_manifest_rejects_noncanonical_os_arch_pair(installation):
    manifest = installation / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    # Both tokens are individually recognized, but Linux release coordinates
    # use aarch64. Accepting their arbitrary product bypasses the target matrix.
    payload["compiler"].update(platform="linux", arch="arm64", path="bin/molt-backend")
    manifest.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="invalid production compiler identity"):
        distribution.installed_compiler(installation)


def test_production_environment_removes_developer_policy_overrides(tmp_path):
    from molt.cargo_execution_policy import CARGO_WRAPPER_ENV_NAMES
    from tools.release.build_compiler import production_environment

    (tmp_path / "rust-toolchain.toml").write_text('[toolchain]\nchannel="1.96.1"\n')
    inherited = {
        "CARGO_HOME": str(tmp_path / "cargo-home"),
        "CARGO_BUILD_JOBS": "2",
        "RUSTUP_TOOLCHAIN": "nightly",
        "CARGO_PROFILE_RELEASE_OPT_LEVEL": "0",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL": "0",
        "CARGO_PROFILE_RELEASE_LTO": "off",
        "CARGO_BUILD_TARGET": "wasm32-wasip1",
        "CARGO_ENCODED_RUSTFLAGS": "-C\x1ftarget-cpu=native",
        "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS": "-Copt-level=0",
        "RUSTC": "custom-rustc",
        **{name: "custom-wrapper" for name in CARGO_WRAPPER_ENV_NAMES},
    }
    original = inherited.copy()
    env = production_environment(tmp_path, inherited)
    assert inherited == original
    assert not any(name.startswith("CARGO_PROFILE_") for name in env)
    assert "CARGO_BUILD_TARGET" not in env and "RUSTC" not in env
    assert "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS" not in env
    assert env["RUSTUP_TOOLCHAIN"] == "1.96.1"
    assert env["CARGO_INCREMENTAL"] == "0"
    assert env["CARGO_ENCODED_RUSTFLAGS"] == f"--remap-path-prefix={tmp_path}=/molt"
    assert all(env[name] == "" for name in CARGO_WRAPPER_ENV_NAMES)
    assert env["CARGO_BUILD_JOBS"] == "2"


@pytest.mark.parametrize(
    "config",
    [
        "[profile.release]\nopt-level=0\n",
        "[profile.release.package.molt-backend]\nopt-level=0\n",
        '[env]\nCARGO_PROFILE_RELEASE_OPT_LEVEL={value="0", force=true}\n',
        '[env]\nRUSTFLAGS={value="-Ctarget-cpu=native", force=true}\n',
        '[build]\ntarget="wasm32-wasip1"\n',
        'include="unowned.toml"\n',
    ],
)
def test_production_compiler_rejects_config_overrides_before_build(tmp_path, config):
    from tools.release.build_compiler import production_environment

    cargo_home = tmp_path / "cargo-home"
    cargo_home.mkdir()
    (cargo_home / "config.toml").write_text(config)
    with pytest.raises(ValueError, match="overridden by Cargo config"):
        production_environment(tmp_path, {"CARGO_HOME": str(cargo_home)})


def test_installed_source_admits_only_matching_launcher(installation):
    installed = distribution.installed_compiler(installation)
    assert installed is not None
    assert installed.verify_launcher()["sha256"] == installed.launcher["sha256"]
    launcher = installation.parent / installed.launcher["path"]
    launcher.write_bytes(b"tampered")
    with pytest.raises(ValueError, match="Installed launcher differs"):
        installed.verify_sources()


def test_release_consumer_selects_native_launcher(tmp_path):
    from tools.release import verify_consumer

    expected = "molt.exe" if os.name == "nt" else "molt"
    assert verify_consumer._launcher(tmp_path) == [str(tmp_path / "bin" / expected)]


@pytest.mark.parametrize("explicit_home", [False, True])
def test_bootstrap_uses_locked_uv_environment_and_reuses_it(tmp_path, explicit_home):
    from tools.command_execution import CommandExecutor

    commands = CommandExecutor.for_file(__file__)

    # Real uv synchronization of a minimal source bundle proves transport and
    # source-only execution, not compiler semantics. No network or build.
    bundle = tmp_path / "bundle with spaces"
    source = bundle / "source"
    source.mkdir(parents=True)
    repository = Path(__file__).resolve().parents[2]
    bootstrap = (repository / "packaging/bootstrap.py").read_text(encoding="utf-8")
    (source / "pyproject.toml").write_text(
        '[project]\nname="molt"\nversion="0.0.1"\nrequires-python=">=3.12"\n'
    )
    (source / "uv.lock").write_text(
        'version=1\nrevision=3\nrequires-python=">=3.12"\n'
        '[[package]]\nname="molt"\nversion="0.0.1"\nsource={virtual="."}\n'
    )
    package = source / "src/molt"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text("")
    (package / "cli").mkdir()
    (package / "cli/__init__.py").write_text("")
    (package / "cli/__main__.py").write_text(
        "import json, os, subprocess, sys\n"
        "child = subprocess.check_output([sys.executable, '-c', 'import molt; print(molt.__file__)'], text=True).strip()\n"
        "print(json.dumps({'prefix':sys.prefix, 'args':sys.argv[1:], "
        "'isolated':sys.flags.isolated, 'cli':__file__, 'child_source':child, "
        "'home':os.environ.get('MOLT_HOME'), "
        "'project':os.environ.get('MOLT_PROJECT_ROOT')}))\n"
        "sys.exit(23)\n"
    )
    default_paths = package / "cli/default_paths.py"
    shutil.copyfile(repository / "src/molt/cli/default_paths.py", default_paths)

    def identity(path):
        return {
            "size": path.stat().st_size,
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }

    manifest = source / distribution.MANIFEST_NAME
    launcher = bundle / "bin" / ("molt.exe" if os.name == "nt" else "molt")
    launcher.parent.mkdir(parents=True)
    launcher.write_bytes(b"launcher")
    manifest.write_text(
        json.dumps(
            {
                "launcher": {"path": "bin/" + launcher.name, **identity(launcher)},
                "git": {"object_format": "sha1", "commit": "a" * 40, "tree": "b" * 40},
                "files": [
                    {"path": name, **identity(source / name)}
                    for name in (
                        "pyproject.toml",
                        "uv.lock",
                        "src/molt/cli/default_paths.py",
                    )
                ],
            }
        )
    )
    project = tmp_path / "unrelated project"
    project.mkdir()
    (project / "pyproject.toml").write_text("not a valid project")
    env = os.environ.copy()
    env.update(
        MOLT_CACHE=str(tmp_path / "cache"),
        UV_OFFLINE="1",
        UV_NO_VERIFY_HASHES="1",
        UV_PROJECT_ENVIRONMENT=str(tmp_path / "poison"),
    )
    env.pop("MOLT_HOME", None)
    if explicit_home:
        env["MOLT_HOME"] = str(tmp_path / "home")
    expected_home = tmp_path / "home" if explicit_home else tmp_path / "cache/home"
    env.pop("MOLT_PROJECT_ROOT", None)
    # Rust canonical paths carry the Windows extended prefix. It must survive
    # without entering a lossy file-URL or shell quoting boundary.
    bundle_argument = "\\\\?\\" + str(bundle) if os.name == "nt" else str(bundle)
    command = [
        sys.executable,
        "-I",
        "-B",
        "-c",
        bootstrap,
        bundle_argument,
        "literal [argument]",
    ]

    def launch():
        result = commands.run(
            command,
            cwd=project,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        assert result.returncode == 23, result.stderr
        return result

    def setup():
        result = commands.run(
            [*command[:-1], "setup", "--install-cli-dependencies"],
            cwd=project,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        assert result.returncode == 0, result.stderr
        assert "does not install Python or toolchains" in result.stderr

    result = commands.run(
        command,
        cwd=project,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    assert result.returncode != 0 and "No dependencies were changed" in result.stderr
    assert not (expected_home / "environments").exists()
    setup()
    first = json.loads(launch().stdout)
    assert first["isolated"] == 1 and first["args"] == ["literal [argument]"]
    assert first["project"] is None
    assert Path(first["cli"]).samefile(package / "cli/__main__.py")
    assert Path(first["child_source"]).samefile(package / "__init__.py")
    assert Path(first["home"]).samefile(expected_home)
    environment = Path(first["prefix"])
    python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    installed_identity = python.stat()
    site_packages = environment / (
        "Lib/site-packages"
        if os.name == "nt"
        else f"lib/python{sys.version_info.major}.{sys.version_info.minor}/site-packages"
    )
    # An extraneous installed distribution must be removed by exact sync, not
    # preserved as a second import authority by --inexact.
    extra = site_packages / "unrequested-1.0.dist-info"
    extra.mkdir()
    (extra / "METADATA").write_text(
        "Metadata-Version: 2.4\nName: unrequested\nVersion: 1.0\n"
    )
    (extra / "RECORD").write_text(
        "unrequested-1.0.dist-info/METADATA,,\nunrequested-1.0.dist-info/RECORD,,\n"
    )
    result = commands.run(
        command,
        cwd=project,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    assert result.returncode != 0 and extra.exists()  # no unsolicited repair
    setup()
    second = json.loads(launch().stdout)
    assert first == second
    assert python.stat().st_mtime_ns == installed_identity.st_mtime_ns
    assert python.stat().st_ino == installed_identity.st_ino
    assert not extra.exists() and not (site_packages / "molt").exists()
    assert environment.parent.samefile(expected_home / "environments")
    assert len(list(environment.parent.iterdir())) == 1
    assert not (source / ".venv").exists() and not (tmp_path / "poison").exists()

    # The Windows extended spelling and normal spelling name the same tree.
    # A nested home must not evade read-only bundle admission.
    nested = bundle / "mutable-state"
    nested_env = dict(env, MOLT_HOME=str(nested))
    rejected = commands.run(
        command,
        cwd=project,
        env=nested_env,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert (
        rejected.returncode != 0 and "outside the immutable bundle" in rejected.stderr
    )
    assert not nested.exists()

    # Tampering must fail before uv can repair or execute the altered inputs.
    for path in (launcher, default_paths, source / "uv.lock"):
        original = path.read_bytes()
        path.write_bytes(original + b"corruption")
        try:
            result = commands.run(
                command,
                cwd=project,
                env=env,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            assert result.returncode != 0 and result.stdout == ""
            assert "differs from the release manifest" in result.stderr
        finally:
            path.write_bytes(original)


@pytest.mark.parametrize("same_file", [False, True])
def test_installation_diagnostics_distinguish_copies_from_aliases(
    installation, monkeypatch, tmp_path, same_file
):
    from molt.cli.installation_diagnostics import installation_checks
    from molt.toolchain_identity import find_executable

    active = installation.parent / "bin" / ("molt.exe" if os.name == "nt" else "molt")
    active.chmod(0o755)
    alternate = tmp_path / "other manager" / active.name
    alternate.parent.mkdir()
    if same_file:
        alternate.hardlink_to(active)
    else:
        alternate.write_bytes(b"other installation")
        alternate.chmod(0o755)
    monkeypatch.setenv(
        "PATH",
        os.pathsep.join(
            (str(active.parent), str(alternate.parent), str(active.parent))
        ),
    )
    monkeypatch.setenv("PATHEXT", ".EXE")
    monkeypatch.setenv("NoDefaultCurrentDirectoryInExePath", "1")
    before = (active.read_bytes(), alternate.read_bytes(), dict(os.environ))
    checks = {check["name"]: check for check in installation_checks(installation)}
    check = checks["installation-molt"]
    assert check["selected"] == str(find_executable("molt", environment=os.environ))
    assert Path(check["selected"]).samefile(active)
    assert check["ok"] == same_file
    assert len(check["candidates"]) == (1 if same_file else 2)
    assert checks["molt-installation"]["source_root"] == str(installation)
    if not same_file:
        assert check["level"] == "warning"
        assert "Molt changes nothing" in check["advice"][-1]
    assert before == (active.read_bytes(), alternate.read_bytes(), dict(os.environ))


@pytest.mark.parametrize("command", ["run", "deploy"])
def test_installed_public_wrapper_cold_then_cache_hit_owns_admission(
    installation, tmp_path, monkeypatch, command
):
    import subprocess
    from molt.cli import (
        build_inputs,
        cache_fingerprints,
        compiler_identity,
        wrapper_build,
    )

    project = tmp_path / "guest"
    project.mkdir()
    entry = project / "app.py"
    entry.write_text("VALUE = 1\n")
    (project / "pyproject.toml").write_text(
        '[project]\nname="guest"\nversion="0.1.0"\n'
    )
    output = project / "compiled-program"
    monkeypatch.setattr(cache_fingerprints, "_compiler_root", lambda: installation)
    monkeypatch.setattr(
        wrapper_build, "_wrapper_build_default_binary_path", lambda resolved: output
    )
    scans = []
    verify_sources = distribution.InstalledCompiler.verify_sources

    def sources(compiler):
        scans.append(compiler.source_root)
        return verify_sources(compiler)

    monkeypatch.setattr(distribution.InstalledCompiler, "verify_sources", sources)

    def forbidden(*args, **kwargs):
        raise AssertionError("installed wrapper must not discover developer inputs")

    monkeypatch.setattr(compiler_identity, "backend_build_admission", forbidden)
    monkeypatch.setattr(cache_fingerprints, "_backend_source_paths", forbidden)
    monkeypatch.setattr(cache_fingerprints, "_frontend_tooling_source_paths", forbidden)
    children = []

    def compile_child(cmd, **kwargs):
        children.append(cmd)
        output.write_bytes(b"compiled output")
        payload = {
            "command": "build",
            "status": "ok",
            "data": {
                "output": str(output),
                "consumer_output": str(output),
                "artifacts": {"native": str(output)},
            },
        }
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(wrapper_build, "_run_completed_command", compile_child)

    def run_wrapper():
        resolved, error = build_inputs._resolve_wrapper_build_entry(
            file_path=str(entry),
            module=None,
            project_root=project,
            json_output=True,
            command=command,
            build_args=[],
            env={},
            source_cwd=project,
        )
        assert error is None and resolved is not None
        return wrapper_build._run_wrapper_build(
            file_path=str(entry),
            module=None,
            build_args=[],
            env={},
            project_root=project,
            json_output=True,
            command=command,
            verbose=False,
            resolved_build_entry=resolved,
        )

    # No manually opened fingerprint transaction or root pre-admission: this is
    # the shared public run/deploy consumer, including its real manifest lookup.
    cold, _, error = run_wrapper()
    assert error is None and cold is not None and len(children) == 1
    assert scans == [installation, installation]  # pre-child and fresh post-child
    scans.clear()
    warm, duration, error = run_wrapper()
    assert error is None and warm is not None and duration == 0
    assert len(children) == 1 and scans == [installation]
    assert warm.consumer_output == output
    # A new public operation must still reject mutated release executable bytes.
    installed = distribution.installed_compiler(installation)
    assert installed is not None
    metadata = installed.binary.stat()
    installed.binary.write_bytes(b"changed!")
    os.utime(installed.binary, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    rejected, _, error = run_wrapper()
    assert rejected is None and error is not None and len(children) == 1


def test_installed_source_admission_expires_at_operation_boundary(
    installation, monkeypatch
):
    from molt.cli import cache_fingerprints, compiler_identity

    monkeypatch.setattr(cache_fingerprints, "_compiler_root", lambda: installation)
    with cache_fingerprints._source_tree_fingerprint_transaction():
        assert cache_fingerprints._cache_tooling_fingerprint()
    (installation / "Cargo.toml").write_bytes(b"edited")
    with cache_fingerprints._source_tree_fingerprint_transaction():
        with pytest.raises(compiler_identity.CompilerIdentityError):
            cache_fingerprints._cache_tooling_fingerprint()
