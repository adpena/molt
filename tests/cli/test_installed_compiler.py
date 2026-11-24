from __future__ import annotations

from molt.process_environment import ProcessEnvironment

import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import sys
from dataclasses import replace
from typing import BinaryIO, Literal

import pytest

from molt import compiler_distribution as distribution
from molt.cli import backend_binary, backend_execution, cargo_profiles
from molt.toolchain_identity import StableRegularFileIdentity
from molt.verified_subset import current_host_coordinate
from molt.cli.rust_cpu_policy import RustCpuPolicy
from tests.release_lane_fixtures import stage_stdlib_source_summaries_fixture

# The bootstrap's own declaration selects the source modules a test bundle
# ships. Isolated execution, not this projection, proves the closure complete.
BOOTSTRAP_SOURCE_MODULES: tuple[str, ...] = runpy.run_path(
    str(Path(__file__).resolve().parents[2] / "packaging/bootstrap.py")
)["RELEASE_SOURCE_MODULES"]


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
    facts = binary.parent / (
        "molt-wasm-facts.exe" if system == "windows" else "molt-wasm-facts"
    )
    facts.write_bytes(b"facts")
    payload = {
        "schema": distribution.MANIFEST_SCHEMA,
        "stdlib_source_summaries": stage_stdlib_source_summaries_fixture(tmp_path),
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
        "wasm_facts": {
            "path": "bin/" + facts.name,
            "size": 5,
            "sha256": hashlib.sha256(b"facts").hexdigest(),
            "profile": "release",
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
            rust_cpu=RustCpuPolicy.BASELINE,
        )
        assert result.ok, result.message
        identities.add(result.cache_compiler_fingerprint)
    assert len(identities) == 1


def test_installed_compiler_rejects_the_native_cpu_policy(installation, monkeypatch):
    monkeypatch.setattr(
        backend_binary,
        "_backend_fingerprint",
        lambda *a, **k: pytest.fail("installed compiler entered source rebuild"),
    )
    installed = distribution.installed_compiler(installation)
    assert installed is not None

    # The shipped compiler was built for the baseline CPU; claiming native-arch
    # for it would report a host-tuned build that never happened.
    result = backend_binary._ensure_backend_binary(
        installed.binary,
        cargo_timeout=1,
        json_output=True,
        cargo_profile="release",
        project_root=installation,
        backend_features=("native-backend",),
        rust_cpu=RustCpuPolicy.NATIVE,
    )

    assert not result.ok and result.phase == "installed_compiler"
    assert "MOLT_PERF_PROFILE=native-arch" in result.message


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
        rust_cpu=RustCpuPolicy.BASELINE,
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
        (installation / "extra.py").write_text("pass", encoding="utf-8")
    elif damage == "empty-directory":
        (installation / "extra").mkdir()
    else:
        (installation / "Cargo.toml").unlink()
    with pytest.raises(ValueError):
        installed.verify_sources()


@pytest.mark.parametrize("mutation", ["rewrite", "replace", "mode", "extra"])
def test_installed_source_binds_inventory_to_stable_read(
    installation: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    if mutation == "mode" and os.name == "nt":
        pytest.skip("POSIX source permission semantics")
    compiler = distribution.installed_compiler(installation)
    assert compiler is not None
    victim = installation / "Cargo.lock"
    before = victim.stat()
    original = distribution.stable_regular_file_identity
    injected = False

    def capture(
        path: Path, *, label: str, expected_path_stat: os.stat_result | None = None
    ) -> StableRegularFileIdentity:
        nonlocal injected
        assert expected_path_stat is not None
        if path == victim:
            injected = True
            if mutation == "rewrite":
                victim.write_bytes(b"edited")
                os.utime(victim, ns=(before.st_atime_ns, before.st_mtime_ns))
            elif mutation == "replace":
                replacement = installation.parent / "replacement"
                replacement.write_bytes(b"source")
                os.utime(replacement, ns=(before.st_atime_ns, before.st_mtime_ns))
                os.replace(replacement, victim)
            elif mutation == "mode":
                victim.chmod(before.st_mode ^ 0o100)
            else:
                (victim.parent / "extra.py").write_bytes(b"")
        return original(path, label=label, expected_path_stat=expected_path_stat)

    monkeypatch.setattr(distribution, "stable_regular_file_identity", capture)
    with pytest.raises(ValueError):
        compiler.verify_sources()
    assert injected


def test_installed_source_reports_first_manifest_error(
    tmp_path: Path,
) -> None:
    expected = hashlib.sha256(b"ok\n").hexdigest()
    records = []
    for index in range(2):
        name = f"file-{index:03}.py"
        (tmp_path / name).write_bytes(b"bad")
        records.append({"path": name, "mode": 0o100644, "size": 3, "sha256": expected})
    with pytest.raises(
        ValueError, match="Compiler source content changed: file-000.py"
    ):
        distribution.verify_source_inventory(tmp_path, records)


@pytest.mark.usefixtures("isolated_molt_cache")
def test_installed_stdlib_summary_binds_source_package_and_target(
    installation: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import stdlib_source_summaries
    from molt.cli.module_graph_discovery import _load_module_import_scan
    from molt.cli.module_resolution import _ModuleResolutionCache
    from molt.cli.module_source import PythonSourceSnapshot
    from molt.cli.stdlib_source_summaries import (
        InstalledStdlibSourceSummaries,
        encode_stdlib_source_summaries,
    )
    from molt.compiler_analysis.python_import_scan import SourceImportPurpose
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission
    from molt.target_python import (
        _DEFAULT_TARGET_PYTHON_VERSION,
        _parse_source_for_target,
    )

    stdlib = installation / "src/molt/stdlib"
    module = stdlib / "parent" / "writer.py"
    module.parent.mkdir(parents=True)
    module.write_text(
        "from . import receiver\nreceiver.inserted = 'value'\n"
        "def writer():\n    globals()['local'] = 1\n",
        "utf-8",
    )
    platform_probe = module.with_name("platform_probe.py")
    platform_probe.write_text(
        "import sys\n"
        "if sys.platform == 'wasi':\n"
        "    import wasm_leaf\n"
        "else:\n"
        "    import native_leaf\n",
        "utf-8",
    )
    contents = encode_stdlib_source_summaries(stdlib)
    record = stage_stdlib_source_summaries_fixture(installation.parent, contents)
    metadata = installation / distribution.MANIFEST_NAME
    manifest = json.loads(metadata.read_text("utf-8"))
    manifest["stdlib_source_summaries"] = record
    metadata.write_text(json.dumps(manifest), "utf-8")
    compiler = distribution.installed_compiler(installation)
    assert compiler is not None
    syntax = InstalledStdlibSourceSummaries(compiler)
    snapshot = PythonSourceSnapshot.capture(module)
    target = _DEFAULT_TARGET_PYTHON_VERSION
    tree = _parse_source_for_target(snapshot.content, target_python=target)
    admission = _PythonAstDigestAdmission(tree)

    syntax.seed(snapshot, package="unrelated", target=target, admission=admission)
    assert not admission.namespace_summaries
    syntax.seed(snapshot, package="parent", target=target, admission=admission)
    summary = admission.namespace_summaries[target.feature_version, "parent"]
    assert summary.own_names == frozenset({"local"})
    assert [(write.module, write.name) for write in summary.foreign_writes] == [
        ("parent.receiver", "inserted")
    ]

    monkeypatch.setattr(
        stdlib_source_summaries, "compiler_source_root", lambda: installation
    )
    for foreign_writes, from_release in (((), True), (("receiver",), False)):
        loaded = _load_module_import_scan(
            module,
            module_name="parent.writer",
            is_package=False,
            import_scan_mode="module_init",
            purpose=SourceImportPurpose.EXECUTION,
            resolution_cache=_ModuleResolutionCache(),
            project_root=None,
            roots=[stdlib],
            stdlib_root=stdlib,
            stdlib_allowlist={"parent"},
            namespace_premise=PythonNamespacePremise(foreign_writes, 0, 0, (), ()),
        )
        assert loaded.cache_hit is from_release
        assert loaded.source_parsed is not from_release

    for platform, expected in ((sys.platform, "native_leaf"), ("wasi", "wasm_leaf")):
        loaded = _load_module_import_scan(
            platform_probe,
            module_name="parent.platform_probe",
            is_package=False,
            import_scan_mode="module_init",
            purpose=SourceImportPurpose.EXECUTION,
            resolution_cache=_ModuleResolutionCache(target_sys_platform=platform),
            project_root=None,
            roots=[stdlib],
            stdlib_root=stdlib,
            stdlib_allowlist={"parent"},
            namespace_premise=PythonNamespacePremise((), 0, 0, (), ()),
        )
        assert loaded.cache_hit
        assert loaded.scan.source_requests.imports == ("sys", expected)

    module.write_text("print('edited')\n", "utf-8")
    edited = PythonSourceSnapshot.capture(module)
    admission = _PythonAstDigestAdmission(
        _parse_source_for_target(edited.content, target_python=target)
    )
    syntax.seed(edited, package="parent", target=target, admission=admission)
    assert not admission.namespace_summaries

    stage_stdlib_source_summaries_fixture(installation.parent, contents + b"\n")
    with pytest.raises(ValueError, match="differ from their manifest"):
        InstalledStdlibSourceSummaries(compiler)


@pytest.mark.parametrize(
    "recorded,actual,accepted",
    [
        (0o100644, 0o644, True),
        (0o100644, 0o640, True),
        (0o100755, 0o750, True),
        # Under umask 002 a checkout and a pip install both write 664 or 775.
        (0o100644, 0o664, True),
        (0o100755, 0o775, True),
        # The executable bit and the special bits are what Git's mode binds.
        (0o100644, 0o755, False),
        (0o100755, 0o644, False),
        (0o100755, 0o4755, False),
        (0o100644, 0o2644, False),
        (0o100644, 0o1644, False),
    ],
)
def test_source_mode_follows_git_semantics(recorded, actual, accepted):
    assert distribution._source_mode_matches_git(actual, recorded) is accepted


def test_guest_project_discovery_starts_at_entry_without_launcher_override(
    tmp_path, monkeypatch
):
    from molt.cli import project_roots

    project = tmp_path / "project"
    entry = project / "src" / "app.py"
    entry.parent.mkdir(parents=True)
    entry.write_text("print('hello')\n", encoding="utf-8")
    (project / "pyproject.toml").write_text("[tool.molt]\n", encoding="utf-8")
    unrelated_cwd = tmp_path / "unrelated"
    unrelated_cwd.mkdir()
    monkeypatch.chdir(unrelated_cwd)
    monkeypatch.delenv("MOLT_PROJECT_ROOT", raising=False)
    assert project_roots._find_project_root(entry) == project
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(unrelated_cwd))
    assert project_roots._find_project_root(entry) == unrelated_cwd


@pytest.mark.parametrize(
    "profile,features", [("dev-fast", ("native-backend",)), ("release", ("polly",))]
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


def test_installed_source_admits_only_matching_launcher(installation):
    installed = distribution.installed_compiler(installation)
    assert installed is not None
    assert installed.verify_launcher()["sha256"] == installed.launcher["sha256"]
    launcher = installation.parent / installed.launcher["path"]
    launcher.write_bytes(b"tampered")
    with pytest.raises(ValueError, match="Installed launcher differs"):
        installed.verify_sources()


@pytest.mark.parametrize("damage", [None, "missing", "changed", "host"])
def test_installed_wasm_facts_admits_live_shipped_bytes(
    installation, monkeypatch, damage
):
    installed = distribution.installed_compiler(installation)
    assert installed is not None
    assert (
        installed.wasm_facts_binary
        == installation.parent / installed.wasm_facts["path"]
    )
    if damage == "missing":
        installed.wasm_facts_binary.unlink()
    elif damage == "changed":
        installed.wasm_facts_binary.write_bytes(b"other")
    elif damage == "host":
        monkeypatch.setattr(
            distribution, "current_host_coordinate", lambda: ("other", "host")
        )
    if damage is None:
        assert (
            installed.verify_wasm_facts()["sha256"]
            == hashlib.sha256(b"facts").hexdigest()
        )
    else:
        with pytest.raises((OSError, ValueError)):
            installed.verify_wasm_facts()


@pytest.mark.parametrize(
    "field,value",
    [
        ("path", "bin/molt-backend"),
        ("path", "../molt-wasm-facts"),
        ("sha256", "invalid"),
        ("size", True),
        ("size", 0),
        ("profile", "dev-fast"),
        ("platform", []),
        ("arch", []),
    ],
)
def test_installed_wasm_facts_manifest_rejects_invalid_records(
    installation, field, value
):
    manifest = installation / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    payload["wasm_facts"][field] = value
    manifest.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="invalid production WASM facts identity"):
        distribution.installed_compiler(installation)


@pytest.mark.parametrize("damage", ["v5", "absent", "extra", "target"])
def test_installed_manifest_requires_the_exact_wasm_facts_contract(
    installation, damage
):
    manifest = installation / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    if damage == "v5":
        payload["schema"] = "molt.release-compiler-source.v5"
    elif damage == "absent":
        del payload["wasm_facts"]
    elif damage == "extra":
        payload["wasm_facts"]["features"] = []
    else:
        payload["wasm_facts"].update(
            platform="windows", arch="x86_64", path="bin/molt-wasm-facts.exe"
        )
        if payload["compiler"]["platform"] == "windows":
            payload["wasm_facts"].update(
                platform="linux", arch="x86_64", path="bin/molt-wasm-facts"
            )
    manifest.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError):
        distribution.installed_compiler(installation)


def test_release_consumer_selects_native_launcher(tmp_path):
    from tools.release import verify_consumer

    expected = "molt.exe" if os.name == "nt" else "molt"
    assert verify_consumer._launcher(tmp_path) == [str(tmp_path / "bin" / expected)]


@pytest.mark.parametrize("explicit_home", [False, True])
def test_bootstrap_uses_locked_uv_environment_and_reuses_it(tmp_path, explicit_home):
    from tools.command_execution import CommandExecutor

    commands = CommandExecutor.for_file(__file__, stem="MOLT_PYTEST")

    # Real uv synchronization of a minimal source bundle proves transport and
    # source-only execution, not compiler semantics. No network or build.
    bundle = tmp_path / "bundle with spaces"
    source = bundle / "source"
    source.mkdir(parents=True)
    repository = Path(__file__).resolve().parents[2]
    bootstrap = (repository / "packaging/bootstrap.py").read_text(encoding="utf-8")
    (source / "pyproject.toml").write_text(
        '[project]\nname="molt"\nversion="0.0.1"\nrequires-python=">=3.12"\n',
        encoding="utf-8",
    )
    (source / "uv.lock").write_text(
        'version=1\nrevision=3\nrequires-python=">=3.12"\n'
        '[[package]]\nname="molt"\nversion="0.0.1"\nsource={virtual="."}\n',
        encoding="utf-8",
    )
    package = source / "src/molt"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text("", encoding="utf-8")
    (package / "cli").mkdir()
    (package / "cli/__init__.py").write_text("", encoding="utf-8")
    (package / "cli/__main__.py").write_text(
        "import json, os, subprocess, sys\n"
        "child = subprocess.check_output([sys.executable, '-c', 'import molt; print(molt.__file__)'], text=True).strip()\n"
        "print(json.dumps({'prefix':sys.prefix, 'args':sys.argv[1:], "
        "'isolated':sys.flags.isolated, 'ignore_environment':sys.flags.ignore_environment, "
        "'no_user_site':sys.flags.no_user_site, 'safe_path':sys.flags.safe_path, "
        "'hash_randomization':sys.flags.hash_randomization, 'cli':__file__, 'child_source':child, "
        "'home':os.environ.get('MOLT_HOME'), "
        "'runtime_store':os.environ.get('MOLT_INSTALLED_RUNTIME_STORE'), "
        "'project':os.environ.get('MOLT_PROJECT_ROOT')}))\n"
        "sys.exit(23)\n",
        encoding="utf-8",
    )
    for module in BOOTSTRAP_SOURCE_MODULES:
        shutil.copyfile(repository / f"src/molt/{module}.py", package / f"{module}.py")

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
                        *(
                            f"src/molt/{module}.py"
                            for module in BOOTSTRAP_SOURCE_MODULES
                        ),
                    )
                ],
            }
        ),
        encoding="utf-8",
    )
    project = tmp_path / "unrelated project"
    project.mkdir()
    (project / "pyproject.toml").write_text("not a valid project", encoding="utf-8")
    env = os.environ.copy()
    env.update(
        MOLT_CACHE=str(tmp_path / "cache"),
        UV_OFFLINE="1",
        UV_NO_VERIFY_HASHES="1",
        UV_PROJECT_ENVIRONMENT=str(tmp_path / "poison"),
        MOLT_INSTALLED_RUNTIME_STORE=str(tmp_path / "work/state/installed-runtime"),
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
            encoding="utf-8",
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
            encoding="utf-8",
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
        encoding="utf-8",
    )
    assert result.returncode != 0 and "No dependencies were changed" in result.stderr
    assert not (expected_home / "environments").exists()
    setup()
    first = json.loads(launch().stdout)
    assert first["isolated"] == 1 and first["args"] == ["literal [argument]"]
    assert first["ignore_environment"] == first["no_user_site"] == 1
    assert first["safe_path"] is True
    assert first["hash_randomization"] == 1
    assert first["project"] is None
    assert Path(first["cli"]).samefile(package / "cli/__main__.py")
    assert Path(first["child_source"]).samefile(package / "__init__.py")
    assert Path(first["home"]).samefile(expected_home)
    assert first["runtime_store"] == str(tmp_path / "work/state/installed-runtime")
    assert not (tmp_path / "work").exists()  # Bootstrap owns only its frontend.
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
        "Metadata-Version: 2.4\nName: unrequested\nVersion: 1.0\n", encoding="utf-8"
    )
    (extra / "RECORD").write_text(
        "unrequested-1.0.dist-info/METADATA,,\nunrequested-1.0.dist-info/RECORD,,\n",
        encoding="utf-8",
    )
    result = commands.run(
        command,
        cwd=project,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
        encoding="utf-8",
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
        encoding="utf-8",
    )
    assert (
        rejected.returncode != 0 and "outside the immutable bundle" in rejected.stderr
    )
    assert not nested.exists()

    # Tampering must fail before uv can repair or execute the altered inputs.
    for path in (
        launcher,
        *(package / f"{module}.py" for module in BOOTSTRAP_SOURCE_MODULES),
        source / "uv.lock",
    ):
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
                encoding="utf-8",
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
    entry.write_text("VALUE = 1\n", encoding="utf-8")
    (project / "pyproject.toml").write_text(
        '[project]\nname="guest"\nversion="0.1.0"\n', encoding="utf-8"
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
            env=ProcessEnvironment({}),
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


@pytest.fixture
def executable_reads(
    installation: Path, monkeypatch: pytest.MonkeyPatch
) -> dict[Path, int]:
    """Count actual digest traversals of the independently staged members."""
    from molt import toolchain_identity

    compiler = distribution.installed_compiler(installation)
    assert compiler is not None
    paths = (compiler.binary, compiler.wasm_facts_binary)
    objects = {(path.stat().st_dev, path.stat().st_ino): path for path in paths}
    reads = dict.fromkeys(paths, 0)
    original = toolchain_identity._sha256_stream

    def digest(stream: BinaryIO, *, max_bytes: int) -> str:
        metadata = os.fstat(stream.fileno())
        path = objects.get((metadata.st_dev, metadata.st_ino))
        if path is not None:
            reads[path] += 1
        return original(stream, max_bytes=max_bytes)

    monkeypatch.setattr(toolchain_identity, "_sha256_stream", digest)
    # A zero window lets real fixture generations be settled at enrollment;
    # the racy test below exercises the production window and aging separately.
    monkeypatch.setattr(toolchain_identity, "DIGEST_RACY_WINDOW_NS", 0)
    return reads


@pytest.mark.skipif(os.name == "nt", reason="settled POSIX observation cost contract")
def test_installed_consumers_share_one_exact_byte_capture(
    installation: Path,
    monkeypatch: pytest.MonkeyPatch,
    executable_reads: dict[Path, int],
) -> None:
    from molt.backend_binary_identity import identify_backend_binary
    from molt.backend_environment import CodegenSelection
    from molt.cli import backend_cache_setup, cache_fingerprints, compiler_identity
    from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
    from molt.cli.cache_keys import FunctionDigests
    from molt.cli.models import _ModuleGraphMetadata
    from molt.compilation_target import admit_target
    from molt.host_tool_products import HostToolProduct
    from molt.target_python import TargetPythonVersion

    monkeypatch.setattr(cache_fingerprints, "_compiler_root", lambda: installation)
    monkeypatch.setenv("MOLT_PORTABLE", "1")
    with cache_fingerprints._source_tree_fingerprint_transaction():
        fingerprint = cache_fingerprints._cache_fingerprint(
            backend_features=("native-backend",),
            cargo_profile="release",
            rust_cpu=RustCpuPolicy.BASELINE,
        )
        installed = compiler_identity.installed_compiler_admission(installation)
        assert installed is not None
        binary = installed.compiler.binary
        ensured = backend_binary._ensure_backend_binary(
            binary,
            cargo_timeout=1,
            json_output=True,
            cargo_profile="release",
            project_root=installation,
            backend_features=("native-backend",),
            rust_cpu=RustCpuPolicy.BASELINE,
        )
        assert ensured.ok, ensured.message
        assert ensured.installed_observation is installed.binary_observation
        backend_execution._backend_daemon_config_digest(
            installation, "release", backend_bin=binary, env={}
        )
        target = admit_target("native")
        setup = backend_cache_setup._prepare_backend_cache_setup(
            backend_bin=binary,
            backend_binary_observation=ensured.installed_observation,
            cache_enabled=False,
            ir={"functions": []},
            function_digests=FunctionDigests(),
            target=target,
            artifact_contract=resolve_backend_artifact_contract(
                target=target, emit_mode="obj"
            ),
            profile="dev",
            runtime_cargo_profile="dev-fast",
            backend_cargo_profile="release",
            emit_mode="obj",
            is_wasm=False,
            linked=False,
            project_root=installation.parent,
            cache_dir=None,
            output_artifact=installation.parent / "out.o",
            warnings=[],
            entry_module="app",
            module_graph_metadata=_ModuleGraphMetadata(
                logical_source_path_by_module={},
                namespace_portions_by_module={},
                module_is_package_by_module={},
                module_execution_kind_by_module={},
                frontend_module_costs=None,
                stdlib_like_by_module={},
                module_source_role_by_module={},
                scan_authority=None,
            ),
            target_python=TargetPythonVersion(3, 12, 0),
            backend_compiler_fingerprint=fingerprint,
            codegen=CodegenSelection(),
            rust_cpu=RustCpuPolicy.BASELINE,
        )
        assert (
            setup.backend_environment["MOLT_BACKEND_COMPILER_FINGERPRINT"]
            == fingerprint
        )
        assert executable_reads[binary] == 1
        assert executable_reads[installed.compiler.wasm_facts_binary] == 0
        first_digest = identify_backend_binary(
            binary, observed=installed.binary_observation
        )
        assert executable_reads[binary] == 1
        # Independently capture the canonical cold projection, then compare it.
        assert first_digest == identify_backend_binary(binary)
        assert executable_reads[binary] == 2
        facts = installed.host_tool_observation(HostToolProduct.WASM_FACTS)
        assert facts is installed.host_tool_observation(HostToolProduct.WASM_FACTS)
        assert executable_reads[installed.compiler.wasm_facts_binary] == 1


@pytest.mark.skipif(os.name == "nt", reason="settled POSIX observation cost contract")
def test_release_and_operation_boundaries_capture_bytes_exactly(
    installation: Path, executable_reads: dict[Path, int]
) -> None:
    from molt.cli import cache_fingerprints, compiler_identity

    compiler = distribution.installed_compiler(installation)
    assert compiler is not None
    with cache_fingerprints._source_tree_fingerprint_transaction():
        assert compiler_identity.installed_compiler_admission(installation)
        assert compiler_identity.installed_compiler_admission(installation)
        assert executable_reads[compiler.binary] == 1
        for _ in range(2):
            compiler.verify_binary(("native-backend",), "release")
            compiler.verify_wasm_facts()
        assert executable_reads[compiler.binary] == 3
        assert executable_reads[compiler.wasm_facts_binary] == 2
        assert compiler_identity.installed_compiler_admission(installation, fresh=True)
        assert executable_reads[compiler.binary] == 4
    with cache_fingerprints._source_tree_fingerprint_transaction():
        assert compiler_identity.installed_compiler_admission(installation)
    assert executable_reads[compiler.binary] == 5
    assert compiler_identity.installed_compiler_admission(installation)
    assert compiler_identity.installed_compiler_admission(installation)
    assert executable_reads[compiler.binary] == 7


@pytest.mark.parametrize("product", ["backend", "facts"])
@pytest.mark.parametrize("change", ["rewrite", "replace", "missing", "alias"])
def test_installed_observation_rejects_changed_generations(
    installation: Path,
    executable_reads: dict[Path, int],
    product: Literal["backend", "facts"],
    change: Literal["rewrite", "replace", "missing", "alias"],
) -> None:
    from molt.backend_binary_identity import identify_backend_binary
    from molt.cli import cache_fingerprints, compiler_identity
    from molt.host_tool_products import HostToolProduct

    with cache_fingerprints._source_tree_fingerprint_transaction():
        admitted = compiler_identity.installed_compiler_admission(installation)
        assert admitted is not None
        role = (
            HostToolProduct.BACKEND
            if product == "backend"
            else HostToolProduct.WASM_FACTS
        )
        observed = admitted.host_tool_observation(role)
        path = observed.identity.path
        before = path.stat()
        original = path.read_bytes()
        if change == "rewrite":
            path.write_bytes(b"!" * len(original))
            os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        elif change == "replace":
            replacement = path.with_suffix(".new")
            replacement.write_bytes(original)
            os.utime(replacement, ns=(before.st_atime_ns, before.st_mtime_ns))
            os.replace(replacement, path)
        elif change == "missing":
            path.unlink()
        else:
            replacement = path.with_suffix(".new")
            path.rename(replacement)
            try:
                path.symlink_to(replacement)
            except OSError:
                if os.name == "nt":
                    pytest.skip("host does not admit symbolic link creation")
                raise
        with pytest.raises((OSError, ValueError)):
            if product == "backend":
                compiler_identity.installed_compiler_admission(installation)
            else:
                admitted.host_tool_observation(role)
        with pytest.raises((OSError, ValueError)):
            identify_backend_binary(path, observed=observed)


@pytest.mark.parametrize("change", ["profile", "feature", "host", "manifest"])
def test_installed_observation_rechecks_current_selection(
    installation: Path,
    monkeypatch: pytest.MonkeyPatch,
    executable_reads: dict[Path, int],
    change: Literal["profile", "feature", "host", "manifest"],
) -> None:
    from molt.cli import cache_fingerprints, compiler_identity

    with cache_fingerprints._source_tree_fingerprint_transaction():
        assert compiler_identity.installed_compiler_admission(installation)
        if change == "host":
            monkeypatch.setattr(
                distribution, "current_host_coordinate", lambda: ("other", "host")
            )
        elif change == "manifest":
            path = installation / distribution.MANIFEST_NAME
            manifest = json.loads(path.read_text(encoding="utf-8"))
            manifest["compiler"]["sha256"] = hashlib.sha256(b"wrong!!!").hexdigest()
            path.write_text(json.dumps(manifest), encoding="utf-8")
        with pytest.raises(compiler_identity.CompilerIdentityError):
            compiler_identity.installed_compiler_admission(
                installation,
                ("polly",) if change == "feature" else None,
                "dev-fast" if change == "profile" else None,
            )


def test_installed_observation_binds_role_and_fresh_facts_manifest(
    installation: Path,
) -> None:
    from molt.backend_binary_identity import identify_backend_binary
    from molt.cli import cache_fingerprints, compiler_identity
    from molt.host_tool_products import HostToolProduct

    with cache_fingerprints._source_tree_fingerprint_transaction():
        admitted = compiler_identity.installed_compiler_admission(installation)
        assert admitted is not None
        backend = admitted.binary_observation
        facts = admitted.host_tool_observation(HostToolProduct.WASM_FACTS)
        with pytest.raises(ValueError):
            identify_backend_binary(facts.identity.path, observed=backend)
        with pytest.raises(ValueError):
            backend_binary._BackendBinaryEnsureResult(
                ok=True, binary=facts.identity.path, installed_observation=backend
            )
        path = installation / distribution.MANIFEST_NAME
        manifest = json.loads(path.read_text(encoding="utf-8"))
        manifest["wasm_facts"]["sha256"] = hashlib.sha256(b"other").hexdigest()
        path.write_text(json.dumps(manifest), encoding="utf-8")
        with pytest.raises(compiler_identity.CompilerIdentityError):
            admitted.host_tool_observation(HostToolProduct.WASM_FACTS)


def test_installed_release_bytes_ignore_forged_persistent_digest(
    installation: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import toolchain_identity
    from molt.cli.compiler_identity import (
        CompilerIdentityError,
        installed_compiler_admission,
    )

    compiler = distribution.installed_compiler(installation)
    assert compiler is not None
    compiler.binary.write_bytes(b"wrong!!!")
    actual = toolchain_identity.stable_regular_file_identity(
        compiler.binary, label="fixture"
    )
    forged = replace(actual, sha256=hashlib.sha256(b"compiler").hexdigest())
    consulted: list[Path] = []

    def cache_identity(
        self: toolchain_identity.FileDigestCache, path: Path, *, label: str
    ) -> toolchain_identity.StableRegularFileIdentity:
        consulted.append(path)
        return forged

    monkeypatch.setattr(toolchain_identity.FileDigestCache, "identity", cache_identity)
    with pytest.raises(CompilerIdentityError):
        installed_compiler_admission(installation)
    with pytest.raises(ValueError):
        compiler.verify_binary(("native-backend",), "release")
    assert consulted == []


def test_real_cli_entry_retains_isolated_startup_without_seed_restart(tmp_path):
    from tests.cli.process_guard import run_cli_test_process

    root = Path(__file__).resolve().parents[2]
    # A selected developer venv may preload its editable Molt from a .pth file.
    # -I deliberately keeps trusted environment site initialization. Use the
    # real bootstrap's dependency-only installation, never repair sys.modules.
    bundle = tmp_path / "dependency-bundle"
    source = bundle / "source"
    source.mkdir(parents=True)
    for name in ("pyproject.toml", "uv.lock"):
        shutil.copyfile(root / name, source / name)
    environment = tmp_path / "dependency-environment"
    setup_script = f"""
import os, runpy, shutil
from pathlib import Path
bootstrap = runpy.run_path({str(root / "packaging/bootstrap.py")!r})
uv = shutil.which('uv')
assert uv is not None, 'locked CLI dependency setup requires uv'
env = dict(os.environ)
# Match the existing bootstrap caller's resolver-policy precondition; retain
# process custody and only uv's operational cache/offline inputs.
for name in tuple(env):
    if name in {{'PYTHONPATH', 'PYTHONHOME', 'VIRTUAL_ENV'}} or (name.startswith('UV_') and name not in {{'UV_CACHE_DIR', 'UV_OFFLINE'}}):
        env.pop(name)
env['UV_OFFLINE'] = '1'
python = bootstrap['_prepare_environment'](
    uv, Path({str(bundle)!r}), Path({str(environment)!r}), env, install=True,
)
print(python)
"""
    # Offline installation reads uv's cache. Name the run context's canonical
    # cache, which dx installs for every managed run, instead of whatever cache
    # the ambient environment happens to leave for uv's default lookup.
    from molt import dx

    setup_env = dict(os.environ)
    setup_env.setdefault(
        "UV_CACHE_DIR", str(dx.artifact_root(root, setup_env) / ".uv-cache")
    )
    setup = run_cli_test_process(
        [sys.executable, "-I", "-B", "-c", setup_script],
        cwd=tmp_path,
        env=setup_env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert setup.returncode == 0, setup.stderr
    python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    assert Path(setup.stdout.strip()).samefile(python)
    marker = tmp_path / "ambient-imported"
    poison = tmp_path / "python-environment"
    user_site = poison / (
        "Python"
        + str(sys.version_info.major)
        + str(sys.version_info.minor)
        + "/site-packages"
        if os.name == "nt"
        else f"lib/python{sys.version_info.major}.{sys.version_info.minor}/site-packages"
    )
    user_site.mkdir(parents=True)
    poison_code = f"from pathlib import Path;Path({str(marker)!r}).touch()\n"
    for directory in (tmp_path, poison, user_site):
        for name in ("molt.py", "json.py", "sitecustomize.py", "usercustomize.py"):
            (directory / name).write_text(poison_code, encoding="utf-8")
    startup = poison / "startup.py"
    startup.write_text(poison_code, encoding="utf-8")
    script = f"""
import atexit, os, runpy, sys
from pathlib import Path
assert Path(sys.prefix).samefile({str(environment)!r})
assert 'molt' not in sys.modules, 'dependency environment preloaded another Molt'
sys.path.insert(0, {str(root / "src")!r})
assert sys.flags.isolated == sys.flags.ignore_environment == sys.flags.no_user_site == 1
assert sys.flags.safe_path and sys.flags.hash_randomization == 1
sys.argv = ['molt', '--help']
# The process entry ends this process; a restart through exec would lose
# these callbacks, so their output proves the startup was retained.
atexit.register(lambda: print('isolated-normal-exit'))
atexit.register(lambda: print('isolated-source::' + sys.modules['molt'].__file__))
runpy.run_module('molt.cli', run_name='__main__', alter_sys=True)
raise AssertionError('the molt process entry returned to its caller')
"""
    result = run_cli_test_process(
        [str(python), "-I", "-B", "-c", script],
        cwd=tmp_path,
        env={
            **os.environ,
            "PYTHONHASHSEED": "123",
            "PYTHONPATH": str(tmp_path),
            "PYTHONHOME": str(poison),
            "PYTHONUSERBASE": str(poison),
            "PYTHONSTARTUP": str(startup),
            "PYTHONINSPECT": "1",
            "PYTHONWARNINGS": "error",
            "VIRTUAL_ENV": str(poison),
            "UV_PROJECT_ENVIRONMENT": str(poison),
        },
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert "usage:" in result.stdout
    assert result.stdout.rstrip().endswith("isolated-normal-exit")
    origin = next(
        line.removeprefix("isolated-source::")
        for line in result.stdout.splitlines()
        if line.startswith("isolated-source::")
    )
    assert Path(origin).samefile(root / "src/molt/__init__.py")
    assert not marker.exists()


@pytest.mark.parametrize("tamper", [None, *BOOTSTRAP_SOURCE_MODULES])
def test_bootstrap_path_authority_executes_without_molt_importable(tmp_path, tamper):
    # Execute the real bootstrap boundary before any Molt environment exists.
    # Its source dependencies must be verified and loaded together in memory.
    from tests.cli.process_guard import run_cli_test_process
    from tests.process_guard_common import guard_custody_env

    root = Path(__file__).resolve().parents[2]
    cache = tmp_path / "selected-cache"
    bundle = tmp_path / "bundle"
    source = bundle / "source"
    source.mkdir(parents=True)
    launcher = bundle / "bin" / ("molt.exe" if os.name == "nt" else "molt")
    launcher.parent.mkdir()
    launcher.write_bytes(b"launcher identity")
    names = (
        "pyproject.toml",
        "uv.lock",
        *(f"src/molt/{module}.py" for module in BOOTSTRAP_SOURCE_MODULES),
    )
    marker = tmp_path / "executed-before-admission"
    records = []
    for name in names:
        path = source / name
        path.parent.mkdir(parents=True, exist_ok=True)
        content = (root / name).read_bytes()
        if tamper and path.suffix == ".py":
            # Every declared module has an observable execution effect. A
            # mismatched peer must be refused before any effect can run.
            content += f"\n__import__('pathlib').Path({str(marker)!r}).write_text('executed')\n".encode()
        path.write_bytes(content)
        records.append(
            {
                "path": name,
                "size": len(content),
                "sha256": hashlib.sha256(content).hexdigest(),
            }
        )
    git = {"object_format": "sha1", "commit": "a" * 40, "tree": "b" * 40}
    manifest = {
        "launcher": {
            "path": "bin/" + launcher.name,
            "size": launcher.stat().st_size,
            "sha256": hashlib.sha256(launcher.read_bytes()).hexdigest(),
        },
        "git": git,
        "files": records,
    }
    (source / distribution.MANIFEST_NAME).write_text(
        json.dumps(manifest), encoding="utf-8"
    )
    if tamper:
        path = source / f"src/molt/{tamper}.py"
        path.write_bytes(path.read_bytes() + b"\n# bytes differ from admitted source\n")
    code = (
        "import importlib.util, json, runpy, sys\n"
        "from pathlib import Path\n"
        "assert importlib.util.find_spec('molt') is None, 'molt is importable'\n"
        "ns = runpy.run_path(sys.argv[1], run_name='bootstrap_contract_probe')\n"
        "home, identity = ns['_release_inputs'](Path(sys.argv[2]))\n"
        "assert 'molt' not in sys.modules\n"
        "assert not any(name.startswith('_molt_bootstrap_inputs') for name in sys.modules)\n"
        "print(json.dumps({'home':str(home), 'identity':identity}))\n"
    )
    env = {
        name: value
        for name, value in os.environ.items()
        if not name.upper().startswith(("MOLT_", "PYTHON"))
    }
    env.update(guard_custody_env())
    env["MOLT_EXT_ROOT"] = str(tmp_path / "guard-artifacts")
    env["MOLT_CACHE"] = str(cache)
    result = run_cli_test_process(
        [
            sys.executable,
            "-B",
            "-I",
            "-S",
            "-c",
            code,
            str(root / "packaging/bootstrap.py"),
            str(bundle),
        ],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert not marker.exists()
    if tamper:
        assert result.returncode != 0 and result.stdout == ""
        assert (
            f"bundled src/molt/{tamper}.py differs from the release manifest"
            in result.stderr
        )
    else:
        assert result.returncode == 0, result.stderr
        observed = json.loads(result.stdout)
        assert Path(observed["home"]) == cache / "home"
        expected = hashlib.sha256(json.dumps(git, sort_keys=True).encode())
        for record in records:
            expected.update(bytes.fromhex(record["sha256"]))
        expected.update(
            json.dumps(
                [sys.executable, sys.version, sys.implementation.cache_tag]
            ).encode()
        )
        assert observed["identity"] == expected.hexdigest()


def test_installed_manifest_projection_expires_with_its_operation(
    installation: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import cache_fingerprints

    validate = distribution._validate_installed_manifest
    validations = 0

    def counted_validate(root: Path, raw: bytes) -> distribution.InstalledCompiler:
        nonlocal validations
        validations += 1
        return validate(root, raw)

    monkeypatch.setattr(distribution, "_validate_installed_manifest", counted_validate)
    expected = distribution.installed_compiler(installation)
    assert expected is not None
    assert validations == 1
    with cache_fingerprints._source_tree_fingerprint_transaction():
        for _ in range(3):
            assert distribution.installed_compiler(installation) == expected
        assert validations == 2
        with cache_fingerprints._source_tree_fingerprint_transaction(fresh=True):
            assert distribution.installed_compiler(installation) == expected
        assert validations == 3
        assert distribution.installed_compiler(installation) == expected
        assert validations == 3
    assert distribution.installed_compiler(installation) == expected
    assert validations == 4


def test_installed_manifest_projection_never_shares_mutable_results(
    installation: Path,
) -> None:
    from molt.cli import cache_fingerprints

    payload = json.loads((installation / distribution.MANIFEST_NAME).read_bytes())
    with cache_fingerprints._source_tree_fingerprint_transaction():
        for _ in range(3):
            compiler = distribution.installed_compiler(installation)
            assert compiler is not None
            assert compiler.record == payload["compiler"]
            assert compiler.files[0] == payload["files"][0]
            assert compiler.runtime == payload["runtime"]
            assert (
                compiler.stdlib_source_summaries == payload["stdlib_source_summaries"]
            )
            compiler.record["features"].append("caller-mutation")
            compiler.files[0]["sha256"] = "0" * 64
            compiler.runtime["source"]["commit"] = "0" * 40
            compiler.stdlib_source_summaries["sha256"] = "0" * 64


@pytest.mark.parametrize("replacement", [False, True])
def test_installed_manifest_projection_revalidates_changed_bytes(
    installation: Path, replacement: bool
) -> None:
    from molt.cli import cache_fingerprints

    path = installation / distribution.MANIFEST_NAME
    with cache_fingerprints._source_tree_fingerprint_transaction():
        initial = distribution.installed_compiler(installation)
        assert initial is not None
        metadata = path.stat()
        original = path.read_bytes()
        payload = json.loads(original)
        payload["git"]["commit"] = "d" * 40
        payload["runtime"]["source"]["commit"] = "d" * 40
        changed = json.dumps(payload).encode()
        assert len(changed) == len(original)
        destination = path.with_suffix(".replacement") if replacement else path
        destination.write_bytes(changed)
        os.utime(destination, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        if replacement:
            os.replace(destination, path)
        current = distribution.installed_compiler(installation)
        assert current is not None
        assert initial.source_sha == "a" * 40
        assert current.source_sha == "d" * 40
        # A valid earlier projection cannot hide subsequent schema damage.
        damaged = changed.replace(
            distribution.MANIFEST_SCHEMA.encode(),
            b"!" * len(distribution.MANIFEST_SCHEMA),
        )
        path.write_bytes(damaged)
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        with pytest.raises(ValueError, match="Invalid installed compiler manifest"):
            distribution.installed_compiler(installation)


@pytest.mark.parametrize("damage", ["missing", "link", "duplicate"])
def test_installed_manifest_projection_still_reads_the_live_boundary(
    installation: Path, monkeypatch: pytest.MonkeyPatch, damage: str
) -> None:
    from molt.cli import cache_fingerprints

    monkeypatch.setenv("MOLT_BUNDLE_ROOT", str(installation.parent))
    path = installation / distribution.MANIFEST_NAME
    with cache_fingerprints._source_tree_fingerprint_transaction():
        assert distribution.installed_compiler(installation) is not None
        original = path.read_bytes()
        if damage == "missing":
            path.unlink()
        elif damage == "link":
            target = path.with_suffix(".target")
            path.rename(target)
            try:
                path.symlink_to(target)
            except OSError:
                if os.name == "nt":
                    pytest.skip("host does not admit symbolic link creation")
                raise
        else:
            path.write_bytes(b'{"schema":"duplicate",' + original[1:])
        with pytest.raises((OSError, ValueError)):
            distribution.installed_compiler(installation)
