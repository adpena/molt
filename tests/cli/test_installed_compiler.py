from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import zipfile

import pytest

from molt import compiler_distribution as distribution
from molt.cli import backend_binary, backend_execution, cargo_profiles
from molt.verified_subset import current_host_coordinate


@pytest.fixture
def installation(tmp_path: Path) -> Path:
    source = tmp_path / "source"
    source.mkdir()
    (source / "Cargo.toml").write_bytes(b"source")
    system, arch = current_host_coordinate()
    binary = (
        tmp_path
        / "bin"
        / ("molt-backend.exe" if system == "windows" else "molt-backend")
    )
    binary.parent.mkdir()
    binary.write_bytes(b"compiler")
    payload = {
        "schema": distribution.MANIFEST_SCHEMA,
        "git": {"object_format": "sha1", "commit": "a" * 40, "tree": "b" * 40},
        "files": [
            {
                "path": "Cargo.toml",
                "mode": 0o100644,
                "blob_oid": "c" * 40,
                "size": 6,
                "sha256": hashlib.sha256(b"source").hexdigest(),
            }
        ],
        "compiler": {
            "path": "bin/" + binary.name,
            "size": 8,
            "sha256": hashlib.sha256(b"compiler").hexdigest(),
            "profile": "release",
            "features": list(distribution.PRODUCTION_COMPILER_FEATURES),
            "platform": system,
            "arch": arch,
        },
        "wheel": {
            "filename": "molt-0.0.1-py3-none-any.whl",
            "size": 1,
            "sha256": "f" * 64,
        },
    }
    (source / distribution.MANIFEST_NAME).write_text(
        json.dumps(payload), encoding="utf-8"
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


@pytest.mark.parametrize("launcher_index", range(2 if os.name == "nt" else 1))
def test_shipped_launchers_preserve_arguments_cwd_isolation_and_exit_code(
    tmp_path, launcher_index
):
    from tools.release import build_bundle, verify_consumer

    # This checks real shell-to-bootstrap transport, not compiler acceptance.
    bundle = tmp_path / "bundle [literal] with spaces"
    bootstrap = bundle / "lib" / "molt" / "bootstrap.py"
    bootstrap.parent.mkdir(parents=True)
    bootstrap.write_text(
        "import json, os, sys\n"
        "print(json.dumps({'args':sys.argv[1:], 'isolated':sys.flags.isolated, "
        "'cwd':os.getcwd(), 'project':os.environ['MOLT_PROJECT_ROOT']}))\n"
        "sys.exit(23)\n"
    )
    if os.name == "nt":
        build_bundle._make_windows_wrapper(bundle)
    else:
        build_bundle._make_unix_wrapper(bundle / "bin" / "molt")
    launcher = verify_consumer._launchers(bundle)[launcher_index]
    assert launcher is not None
    project = tmp_path / "project with spaces"
    project.mkdir()
    env = os.environ.copy()
    env["PYTHON"] = sys.executable
    env["MOLT_HOME"] = str(tmp_path / "home")
    env.pop("MOLT_PROJECT_ROOT", None)
    arguments = ["build", "source [x] & y.py", "--output", "binary with spaces"]
    result = verify_consumer._COMMANDS.run(
        [*launcher, *arguments],
        cwd=project,
        env=env,
        text=True,
        encoding="utf-8",
        capture_output=True,
        timeout=30,
        check=False,
    )
    assert result.returncode == 23, result.stderr
    assert json.loads(result.stdout) == {
        "args": arguments,
        "isolated": 1,
        "cwd": str(project),
        "project": str(project),
    }


def test_bootstrap_uses_locked_uv_environment_and_reuses_it(tmp_path):
    from tools.command_execution import CommandExecutor

    commands = CommandExecutor.for_file(__file__)

    # A real install of a synthetic wheel proves bootstrap transport/isolation,
    # not compiler semantics. No network, compiler build or ambient project.
    bundle = tmp_path / "bundle with spaces"
    source = bundle / "source"
    source.mkdir(parents=True)
    bootstrap = bundle / "lib" / "molt" / "bootstrap.py"
    bootstrap.parent.mkdir(parents=True)
    shutil.copyfile(
        Path(__file__).resolve().parents[2] / "packaging/bootstrap.py", bootstrap
    )
    (source / "pyproject.toml").write_text(
        '[project]\nname="molt"\nversion="0.0.1"\nrequires-python=">=3.12"\n'
    )
    (source / "uv.lock").write_text(
        'version=1\nrevision=3\nrequires-python=">=3.12"\n'
        '[[package]]\nname="molt"\nversion="0.0.1"\nsource={virtual="."}\n'
    )
    wheel = bundle / "share" / "molt" / "wheels" / "molt-0.0.1-py3-none-any.whl"
    wheel.parent.mkdir(parents=True)
    with zipfile.ZipFile(wheel, "w") as archive:
        archive.writestr("molt/__init__.py", "")
        archive.writestr(
            "molt/cli.py",
            "import json, sys\n"
            "print(json.dumps({'prefix':sys.prefix, 'args':sys.argv[1:], "
            "'isolated':sys.flags.isolated}))\n"
            "sys.exit(23)\n",
        )
        archive.writestr(
            "molt-0.0.1.dist-info/METADATA",
            "Metadata-Version: 2.4\nName: molt\nVersion: 0.0.1\n",
        )
        archive.writestr(
            "molt-0.0.1.dist-info/WHEEL",
            "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        )
        record = "molt-0.0.1.dist-info/RECORD"
        archive.writestr(
            record, "".join(f"{name},,\n" for name in [*archive.namelist(), record])
        )

    def identity(path):
        return {
            "size": path.stat().st_size,
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }

    manifest = source / distribution.MANIFEST_NAME
    manifest.write_text(
        json.dumps(
            {
                "wheel": {"filename": wheel.name, **identity(wheel)},
                "files": [
                    {"path": name, **identity(source / name)}
                    for name in ("pyproject.toml", "uv.lock")
                ],
            }
        )
    )
    project = tmp_path / "unrelated project"
    project.mkdir()
    (project / "pyproject.toml").write_text("not a valid project")
    env = os.environ.copy()
    env.update(
        MOLT_HOME=str(tmp_path / "home"),
        UV_OFFLINE="1",
        UV_NO_VERIFY_HASHES="1",
        UV_PROJECT_ENVIRONMENT=str(tmp_path / "poison"),
    )
    command = [sys.executable, "-I", "-B", str(bootstrap), "literal [argument]"]

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

    first = json.loads(launch().stdout)
    assert first["isolated"] == 1 and first["args"] == ["literal [argument]"]
    environment = Path(first["prefix"])
    installed_cli = next(environment.rglob("molt/cli.py"))
    installed_identity = installed_cli.stat()
    second = json.loads(launch().stdout)
    assert first == second
    assert installed_cli.stat().st_mtime_ns == installed_identity.st_mtime_ns
    assert installed_cli.stat().st_ino == installed_identity.st_ino
    assert environment.parent == tmp_path / "home" / "environments"
    assert len(list(environment.parent.iterdir())) == 1
    assert not (source / ".venv").exists() and not (tmp_path / "poison").exists()

    # Tampering must fail before uv can repair or execute the altered inputs.
    for path in (wheel, source / "uv.lock"):
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
