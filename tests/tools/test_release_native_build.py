"""Native provenance admission; synthetic headers are not executable proof."""

from __future__ import annotations

from dataclasses import asdict
import copy
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import sys
from types import SimpleNamespace

import pytest

from molt.exact_json import write_exact
from molt.process_environment import (
    NameRule,
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)
from molt.release_matrix import RUST_TARGET_BY_COORDINATE
from tests.native_artifact_fixtures import elf_header, macho_header, pe_header
from tests.process_guard_common import install_module_view
from tools.release import native_build
from tools.release.git_source_snapshot import GitSourceFile, GitSourceSnapshot

# Cargo runs in this file are fakes; build capacity is not under test.
pytestmark = pytest.mark.usefixtures("admitted_build_capacity")


@pytest.mark.parametrize("platform", ["linux", "macos", "windows"])
def test_native_release_rust_remap_keeps_one_encoded_token(tmp_path, platform):
    work = tmp_path / "work = with spaces"
    paths = dict.fromkeys(native_build._TOOL_ROLES, tmp_path / "tool")
    paths["linker_backend"] = tmp_path / "linker-backend"
    environment = {"SYSTEMROOT": str(tmp_path)}
    native_build._select_build_tools(
        environment,
        paths,
        work=work,
        triple="x86_64-pc-windows-msvc" if platform == "windows" else "test-target",
        platform=platform,
    )
    flags = native_build.FlagStringRule.CARGO_ENCODED.decode(
        environment["CARGO_ENCODED_RUSTFLAGS"]
    )
    assert flags[0] == f"--remap-path-prefix={work}=/molt"
    assert flags[1:3] == ("-C", f"linker={paths['linker']}")


def native_image(platform="linux", arch="x86_64"):
    if platform == "windows":
        return bytes(pe_header(machine=0x8664 if arch == "x86_64" else 0xAA64))
    if platform == "macos":
        return bytes(macho_header(cpu=0x01000007 if arch == "x86_64" else 0x0100000C))
    return bytes(elf_header(machine=62 if arch == "x86_64" else 183))


def _policy_files():
    root = Path(__file__).resolve().parents[2]
    return {
        "rust-toolchain.toml": b'[toolchain]\nchannel="1.96.1"\n',
        **{name: (root / name).read_bytes() for name in native_build.LLVM_POLICY_PATHS},
    }


def llvm_input_fixture(snapshot, *, arch, platform):
    """Independent transport facts; this is not an executable LLVM build proof."""
    from molt.llvm_toolchain import llvm_release

    release = llvm_release("22.1.8", Path(__file__).resolve().parents[2])
    assert release is not None
    return {
        "linkage": "static-llvm-explicit-system",
        "version": "22.1.8",
        "upstream_release": asdict(release),
        "policy_sha256": native_build.llvm_policy_sha256(
            item.as_record() for item in snapshot.files
        ),
        "targets": ["AArch64", "WebAssembly"]
        if arch in {"aarch64", "arm64"}
        else ["WebAssembly", "X86"],
        "link_plan": {
            "schema": "molt.llvm-native-link.v1",
            "target_os": platform,
            "target_arches": ["aarch64" if arch == "arm64" else arch],
            "host_target": RUST_TARGET_BY_COORDINATE[(platform, arch)],
            "host_abi": {
                "architecture": "aarch64" if arch == "arm64" else arch,
                "pointer_bits": 64,
                "header_bits": 64,
                "byte_order": "little",
            },
            "llvm_config": "${compiler/llvm-config}",
            "driver": "${compiler/llvm-driver}",
            "llvm_version": "22.1.8",
            "policy": None,
            "libraries": [
                {
                    "kind": "static",
                    "name": "LLVMCore",
                    "source": "sdk",
                    "path": "${compiler/llvm-sdk/lib/libLLVMCore.a}",
                    "size": 10,
                    "sha256": hashlib.sha256(b"LLVM bytes").hexdigest(),
                    "policy_admitted": None,
                }
            ],
        },
        "llvm_config": {
            "sha256": hashlib.sha256(b"configuration").hexdigest(),
            "size": 13,
        },
        "system_static_dependencies": [],
        "resources": {
            "digest": hashlib.sha256(
                b"independent SDK transport inventory"
            ).hexdigest(),
            "file_count": 3,
            "total_size": 29,
            "roots": [
                "compiler/llvm-config",
                "compiler/llvm-driver",
                "compiler/llvm-sdk/lib/libLLVMCore.a",
            ],
            "missing": [],
        },
    }


def native_build_fixture(
    root, snapshot, *, platform="linux", arch="x86_64", epoch=1_700_000_000
):
    """Transport fixture with real schema/header admission and no host tools."""
    triple = RUST_TARGET_BY_COORDINATE[(platform, arch)]
    plans = native_build.component_plan(platform, arch)
    tools = {
        role: {
            "entrypoint": role,
            "content_filename": role,
            "size": 10,
            "sha256": hashlib.sha256(role.encode()).hexdigest(),
            "version": "",
        }
        for role in (
            "cargo",
            "rustc",
            "cc",
            "cxx",
            "ar",
            "ranlib",
            "linker",
            "linker_backend",
            "cmake",
            "ninja",
            *(("nasm",) if (platform, arch) == ("windows", "x86_64") else ()),
        )
    }
    tools["cargo"]["version"] = "cargo 1.96.1 (fixture 2026-01-01)"
    tools["rustc"]["version"] = f"rustc 1.96.1 (fixture 2026-01-01)\nhost: {triple}"
    receipt = {
        "schema": native_build.SCHEMA,
        "source": native_build.source_record(snapshot),
        "source_date_epoch": epoch,
        "target": {"platform": platform, "arch": arch, "rust_target": triple},
        "policy": {
            "revision": 2,
            "rust_channel": "1.96.1",
            "platform_versions": {
                "msvc": "14.44",
                "windows_sdk": "10.0.26100.0",
                "ucrt": "10.0.26100.0",
            }
            if platform == "windows"
            else {
                "sdk": "15.4",
                "deployment_target": "15.4",
                "sdk_settings_sha256": "e" * 64,
            }
            if platform == "macos"
            else {},
            "components": plans,
        },
        "tools": tools,
        "llvm": llvm_input_fixture(snapshot, arch=arch, platform=platform),
        "artifacts": {},
    }
    for role, plan in plans.items():
        data = native_image(platform, arch) + role.encode()
        binary = root / plan["path"]
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_bytes(data)
        receipt["artifacts"][role] = {
            "path": plan["path"],
            "sha256": hashlib.sha256(data).hexdigest(),
            "size": len(data),
        }
    write_exact(root / native_build.RECEIPT_NAME, receipt)
    return receipt


@pytest.fixture
def snapshot():
    entries = tuple(
        GitSourceFile(
            PurePosixPath(name),
            0o100644,
            "b" * 40,
            len(data),
            hashlib.sha256(data).hexdigest(),
        )
        for name, data in sorted(_policy_files().items())
    )
    return GitSourceSnapshot(
        "a" * 40, "c" * 40, "sha1", tuple(_policy_files()), entries
    )


@pytest.mark.parametrize("coordinate", list(RUST_TARGET_BY_COORDINATE))
def test_native_receipts_relocate_without_losing_architecture(
    tmp_path, snapshot, coordinate
):
    platform, arch = coordinate
    first = native_build_fixture(
        tmp_path / "first", snapshot, platform=platform, arch=arch
    )
    second = native_build_fixture(
        tmp_path / "second", snapshot, platform=platform, arch=arch
    )
    assert first == second
    assert (
        native_build.read_native_build(
            tmp_path / "second",
            snapshot=snapshot,
            expected_rust_channel="1.96.1",
            source_date_epoch=1_700_000_000,
            platform=platform,
            arch=arch,
        )
        == first
    )
    assert str(tmp_path) not in json.dumps(first)


@pytest.mark.parametrize("role", ["compiler", "launcher", "worker", "wasm_facts"])
def test_receipt_rejects_different_native_bytes(tmp_path, snapshot, role):
    receipt = native_build_fixture(tmp_path, snapshot)
    (tmp_path / receipt["artifacts"][role]["path"]).write_bytes(
        native_image() + b"substitution"
    )
    with pytest.raises(ValueError, match="binary differs"):
        native_build.read_native_build(
            tmp_path,
            snapshot=snapshot,
            expected_rust_channel="1.96.1",
            source_date_epoch=1_700_000_000,
            platform="linux",
            arch="x86_64",
        )


@pytest.mark.parametrize("role", ["worker", "wasm_facts"])
def test_native_tool_architecture_is_admitted_even_with_self_consistent_hash(
    tmp_path, snapshot, role
):
    receipt = native_build_fixture(tmp_path, snapshot)
    wrong = native_image("linux", "aarch64")
    record = receipt["artifacts"][role]
    (tmp_path / record["path"]).write_bytes(wrong)
    record.update(sha256=hashlib.sha256(wrong).hexdigest(), size=len(wrong))
    write_exact(tmp_path / native_build.RECEIPT_NAME, receipt)
    from molt.cli.native_binary import _NativeBinaryInvalid

    with pytest.raises(_NativeBinaryInvalid, match="header/target"):
        native_build.read_native_build(
            tmp_path,
            snapshot=snapshot,
            expected_rust_channel="1.96.1",
            source_date_epoch=1_700_000_000,
            platform="linux",
            arch="x86_64",
        )


@pytest.mark.parametrize(
    "field,value",
    [("commit", "d" * 40), ("tree", "d" * 40), ("files_sha256", "d" * 64)],
)
def test_receipt_binds_complete_source_identity(tmp_path, snapshot, field, value):
    receipt = native_build_fixture(tmp_path, snapshot)
    receipt["source"][field] = value
    write_exact(tmp_path / native_build.RECEIPT_NAME, receipt)
    with pytest.raises(ValueError, match="differs from candidate"):
        native_build.read_native_build(
            tmp_path,
            snapshot=snapshot,
            expected_rust_channel="1.96.1",
            source_date_epoch=1_700_000_000,
            platform="linux",
            arch="x86_64",
        )


@pytest.mark.parametrize(
    "mutation",
    [
        "profile",
        "features",
        "boolean",
        "epoch",
        "tool",
        "path",
        "host",
        "obsolete-generator",
        "legacy",
    ],
)
def test_receipt_rejects_policy_and_identity_drift(tmp_path, snapshot, mutation):
    receipt = native_build_fixture(tmp_path, snapshot)
    if mutation == "profile":
        receipt["policy"]["components"]["compiler"]["profile"] = "release-output"
    elif mutation == "features":
        receipt["policy"]["components"]["compiler"]["features"] = []
    elif mutation == "boolean":
        receipt["policy"]["components"]["worker"]["default_features"] = 1
    elif mutation == "epoch":
        receipt["source_date_epoch"] = True
    elif mutation == "tool":
        del receipt["tools"]["linker_backend"]
    elif mutation == "path":
        receipt["tools"]["cargo"]["entrypoint"] = "/somewhere/cargo"
    elif mutation == "host":
        receipt["tools"]["rustc"]["version"] = (
            "rustc 1.96.1 (fixture)\nhost: aarch64-unknown-linux-gnu"
        )
    elif mutation == "obsolete-generator":
        receipt["build_python"] = {"identity_sha256": "d" * 64}
    else:
        receipt["schema"] = "molt.release-worker-build.v1"
    with pytest.raises(ValueError):
        native_build.validate_receipt(receipt)


def test_environment_drops_ambient_policy_and_isolates_cargo(tmp_path):
    source = tmp_path / "source"
    source.mkdir()
    (source / "rust-toolchain.toml").write_text(
        '[toolchain]\nchannel="1.96.1"\n', encoding="utf-8"
    )
    inherited = {
        "MOLT_USE_SCCACHE": "1",
        "RUSTC_WRAPPER": "sccache",
        "RUSTC_WORKSPACE_WRAPPER": "workspace-wrapper",
        "CARGO_BUILD_RUSTC_WRAPPER": "sccache",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER": "workspace-wrapper",
        "PATH": "tools",
        "CARGO_HOME": "ambient",
        "CARGO_INCREMENTAL": "1",
        "SOURCE_DATE_EPOCH": "17",
        "TZ": "Pacific/Honolulu",
        "LC_ALL": "ambient-locale",
        "CARGO_PROFILE_RELEASE_OPT_LEVEL": "0",
        "RUSTFLAGS": "-Ctarget-cpu=native",
        "RUSTC": "fake",
        "RUSTUP_TOOLCHAIN": "nightly",
        "CC": "fake",
        "CFLAGS": "-march=native",
        "CL": "/DUNRECORDED_POLICY",
        "_CL_": "/DUNRECORDED_POLICY",
        "PYTHONPATH": "injection",
        "MOLT_RUNTIME_FEATURES": "drift",
        "VCINSTALLDIR": "activated-vc",
        "VSINSTALLDIR": "activated-vs",
    }
    original = inherited.copy()
    env = native_build.build_environment(
        source, tmp_path / "work", inherited, epoch=123
    )
    assert inherited == original
    assert env["CARGO_HOME"] == str(tmp_path / "work" / "cargo-home")
    assert env["CARGO_INCREMENTAL"] == "0" and env["SOURCE_DATE_EPOCH"] == "123"
    assert env["LC_ALL"] == "C" and env["TZ"] == "UTC"
    assert env["MOLT_USE_SCCACHE"] == "0"
    assert all(
        env[name] == ""
        for name in (
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        )
    )
    assert env["RUSTUP_TOOLCHAIN"] == "1.96.1"
    assert env["VCINSTALLDIR"] == "activated-vc"
    assert env["VSINSTALLDIR"] == "activated-vs"
    assert not {
        "RUSTFLAGS",
        "RUSTC",
        "CFLAGS",
        "CL",
        "_CL_",
        "CC",
        "PYTHONPATH",
        "MOLT_RUNTIME_FEATURES",
        "CARGO_PROFILE_RELEASE_OPT_LEVEL",
    }.intersection(env)


@pytest.mark.skipif(sys.platform != "win32", reason="requires the MSVC C/C++ consumer")
def test_msvc_release_path_maps_are_effective_across_build_roots(tmp_path):
    from molt.verified_subset import current_host_coordinate
    from tests.process_guard_common import run_guarded_test_process

    # Keep the command's selected CMake/Ninja; SDK activation only owns MSVC.
    cmake = shutil.which("cmake")
    ninja = shutil.which("ninja")
    env = native_build.activate_msvc_environment(
        dict(os.environ), repo_root=Path(__file__).resolve().parents[2]
    )
    compiler = shutil.which("cl.exe", path=env["PATH"])
    assert compiler is not None, "native Windows release requires MSVC"
    librarian = shutil.which("lib.exe", path=env["PATH"])
    linker = shutil.which("link.exe", path=env["PATH"])
    assert cmake is not None and ninja is not None, (
        "native release requires CMake/Ninja"
    )
    # Cargo/Rust are not executed by this compiler-boundary control.
    assert librarian is not None and linker is not None, (
        "native release requires MSVC tools"
    )
    paths = dict.fromkeys(native_build._TOOL_ROLES, Path(sys.executable))
    paths.update(
        cc=Path(compiler),
        cxx=Path(compiler),
        cmake=Path(cmake),
        ninja=Path(ninja),
        ar=Path(librarian),
        ranlib=Path(librarian),
        linker=Path(linker),
    )
    objects = []
    cmake_objects = []
    for name in ("first", "second space '\u00e9#root"):
        work = tmp_path / name
        source = work / "source"
        source.mkdir(parents=True)
        files = [source / "probe.c", source / "probe.cpp"]
        for path in files:
            path.write_text(
                "const char *molt_path(void) { return __FILE__; }\n", encoding="utf-8"
            )
        selected = ProcessEnvironmentBuilder(env)
        native_build._select_build_tools(
            selected,
            paths,
            work=work,
            triple=RUST_TARGET_BY_COORDINATE[current_host_coordinate()],
            platform="windows",
        )
        selected = ProcessEnvironment(selected)
        # Decode at the real cc-rs boundary, then execute its compiler argv.
        # Distinct names keep the C and C++ object outputs independently owned.
        outputs = []
        for path, flag_name in zip(files, ("CFLAGS", "CXXFLAGS"), strict=True):
            output = work / (path.suffix[1:] + ".obj")
            flags = native_build.FlagStringRule.cc(selected).decode(selected[flag_name])
            result = run_guarded_test_process(
                [compiler, "/nologo", "/c", *flags, str(path), "/Fo" + str(output)],
                cwd=work,
                env=selected,
                check=True,
                timeout=60,
            )
            assert "option ignored" not in result.stderr
            body = output.read_bytes()
            assert b"/molt" in body, "__FILE__ must name the remapped source root"
            assert str(work).encode() not in body, (
                "producer paths must not enter objects"
            )
            outputs.append(body)
        objects.append(outputs)
        (source / "CMakeLists.txt").write_text(
            "cmake_minimum_required(VERSION 3.15)\n"
            "project(MoltPath C CXX)\n"
            "add_library(path_probe OBJECT probe.c probe.cpp)\n",
            encoding="utf-8",
        )
        build = work / "cmake-build"
        # Reproduce cmake-rs's decoded-argument join, then exercise CMake's real
        # compiler commands. Path-bearing flags must reach cl through CL.
        configure = [
            cmake,
            "-S",
            str(source),
            "-B",
            str(build),
            "-G",
            "Ninja",
            "-DCMAKE_MAKE_PROGRAM=" + Path(ninja).as_posix(),
            "-DCMAKE_C_COMPILER=" + Path(compiler).as_posix(),
            "-DCMAKE_CXX_COMPILER=" + Path(compiler).as_posix(),
            "-DCMAKE_C_FLAGS="
            + " ".join(
                native_build.FlagStringRule.cc(selected).decode(selected["CFLAGS"])
            ),
            "-DCMAKE_CXX_FLAGS="
            + " ".join(
                native_build.FlagStringRule.cc(selected).decode(selected["CXXFLAGS"])
            ),
            "-DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY",
            "-DCMAKE_BUILD_TYPE=Release",
        ]
        run_guarded_test_process(
            configure, cwd=work, env=selected, check=True, timeout=60
        )
        result = run_guarded_test_process(
            [cmake, "--build", str(build), "--verbose"],
            cwd=work,
            env=selected,
            timeout=60,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        assert "option ignored" not in result.stdout + result.stderr
        cmake_paths = sorted((build / "CMakeFiles" / "path_probe.dir").glob("*.obj"))
        assert len(cmake_paths) == 2, "CMake must compile both real translation units"
        bodies = [path.read_bytes() for path in cmake_paths]
        for body in bodies:
            assert b"/molt" in body
            assert str(work).encode() not in body
        cmake_objects.append(bodies)
    assert objects[0] == objects[1], "relocated C and C++ release objects must agree"
    assert cmake_objects[0] == cmake_objects[1], "relocated CMake objects must agree"


def test_release_refuses_msvc_environment_flag_overflow(tmp_path):
    paths = dict.fromkeys(native_build._TOOL_ROLES, tmp_path / "tool")
    # Windows counts UTF-16 units, so this is over the limit even though the
    # Python string has fewer than 1024 code points. No compiler may truncate it.
    with pytest.raises(ValueError, match="MSVC CL.*1024"):
        native_build._select_build_tools(
            {"SYSTEMROOT": str(tmp_path)},
            paths,
            work=tmp_path / ("\U0001f600" * 600),
            triple="x86_64-pc-windows-msvc",
            platform="windows",
        )


def test_build_cwd_rejects_ancestor_cargo_configuration(tmp_path):
    (tmp_path / "rust-toolchain.toml").write_text(
        '[toolchain]\nchannel="1.96.1"\n', encoding="utf-8"
    )
    config = tmp_path / ".cargo" / "config.toml"
    config.parent.mkdir()
    config.write_text("[profile.release]\nopt-level=0\n", encoding="utf-8")
    with pytest.raises(ValueError, match="ambient Cargo configuration") as failure:
        native_build.build_environment(tmp_path, tmp_path / "build", {}, epoch=123)
    assert str(config) in str(failure.value)
    assert "--build-root" in str(failure.value)


def test_explicit_build_root_avoids_unrelated_user_configuration(tmp_path):
    poisoned = tmp_path / "user-profile"
    config = poisoned / ".cargo/config.toml"
    config.parent.mkdir(parents=True)
    config.write_text("[build]\n", encoding="utf-8")
    clean = tmp_path / "builds"
    inherited = {"RUNNER_TEMP": str(poisoned / "temp")}
    with pytest.raises(ValueError, match="--build-root"):
        native_build.select_build_root(None, inherited)
    assert native_build.select_build_root(clean, inherited) == clean


def test_receipt_rejects_self_consistent_rust_channel_not_selected_by_source(
    tmp_path, snapshot
):
    receipt = native_build_fixture(tmp_path, snapshot)
    receipt["policy"]["rust_channel"] = "1.97.0"
    for role in ("cargo", "rustc"):
        receipt["tools"][role]["version"] = receipt["tools"][role]["version"].replace(
            "1.96.1", "1.97.0"
        )
    native_build.validate_receipt(receipt)
    write_exact(tmp_path / native_build.RECEIPT_NAME, receipt)
    with pytest.raises(ValueError, match="Rust channel differs from source snapshot"):
        native_build.read_native_build(
            tmp_path,
            snapshot=snapshot,
            expected_rust_channel="1.96.1",
            source_date_epoch=1_700_000_000,
            platform="linux",
            arch="x86_64",
        )


def test_native_build_receipt_rejects_the_retired_python_bound_schema(
    tmp_path, snapshot
):
    receipt = native_build_fixture(tmp_path, snapshot)
    assert "python" not in receipt["tools"] and "build_python" not in receipt
    receipt["schema"] = "molt.release-native-build.v4"
    with pytest.raises(ValueError, match="schema"):
        native_build.validate_receipt(receipt)


@pytest.mark.parametrize(
    "platform,arch,missing",
    [
        ("linux", "x86_64", "cmake"),
        ("macos", "arm64", "ninja"),
        ("windows", "x86_64", "nasm"),
    ],
)
def test_dependency_tools_fail_by_name_before_build(
    tmp_path, monkeypatch, platform, arch, missing
):
    def resolve(name, *, environment, label):
        if name == missing:
            raise ValueError(f"{label} is unavailable: {name}")
        return tmp_path / name

    monkeypatch.setattr(native_build, "resolve_executable", resolve)
    with pytest.raises(ValueError, match=f"{missing}.*required native dependency tool"):
        native_build._tool_paths(tmp_path, {}, platform=platform, arch=arch)


@pytest.mark.parametrize("failure", [None, "source", "tool", "cargo", "llvm", "binary"])
def test_producer_publishes_only_complete_verified_generation(
    tmp_path, snapshot, monkeypatch, failure
):
    import molt.verified_subset

    monkeypatch.setattr(
        molt.verified_subset, "current_host_coordinate", lambda: ("linux", "x86_64")
    )
    template = native_build_fixture(tmp_path / "template", snapshot)
    monkeypatch.setattr(native_build, "source_snapshot", lambda *_: snapshot)
    monkeypatch.setattr(
        native_build, "resolve_executable", lambda *_args, **_kwargs: tmp_path / "git"
    )

    def materialize(_snapshot, output, **_kwargs):
        output.mkdir()
        for name, data in _policy_files().items():
            path = output / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        return output

    monkeypatch.setattr(native_build, "materialize_git_source_snapshot", materialize)
    monkeypatch.setattr(
        native_build,
        "_tool_paths",
        lambda *_args, **_kwargs: {role: tmp_path / role for role in template["tools"]},
    )
    from molt import llvm_toolchain, llvm_link_plan
    from tools.release import binary_compatibility

    # Fake only external tools, policy provider and the SDK verifier. Retained file custody, projection,
    # native receipt admission and failure-atomic publication remain real.
    sdk = tmp_path / "sdk"
    config = sdk / "bin" / "llvm-config"
    archive = sdk / "lib" / "libLLVMCore.a"
    config.parent.mkdir(parents=True)
    archive.parent.mkdir(parents=True)
    config.write_bytes(b"configuration")
    archive.write_bytes(b"LLVMarchive")
    driver = sdk / "bin/clang++"
    driver.write_bytes(b"driver")
    (sdk / "lib/libstdc++.so").write_bytes(b"C++ ABI")
    monkeypatch.setattr(
        llvm_link_plan, "_driver_library_directories", lambda *_args: (sdk / "lib",)
    )
    install_module_view(monkeypatch, "sys", sys, llvm_link_plan, platform="linux")
    monkeypatch.setattr(
        binary_compatibility,
        "linux_shared_library_capability",
        lambda _arch: (
            {
                "tool": "auditwheel",
                "version": "6.8.2",
                "platform": "linux",
                "arch": "x86_64",
                "policy_name": "manylinux_2_39_x86_64",
                "policy_sha256": "a" * 64,
                "shared_libraries": ("libstdc++.so.6",),
            },
            frozenset({"libstdc++.so.6"}),
        ),
    )
    audit_observations = []

    def audit(root, **coordinate):
        audit_observations.append((root, coordinate))
        assert not (tmp_path / "published").exists()
        if failure == "binary":
            raise ValueError("independent final binary audit refusal")
        return SimpleNamespace(tag="manylinux_2_39_x86_64")

    monkeypatch.setattr(
        binary_compatibility, "derive_bundle_wheel_compatibility", audit
    )
    verification = SimpleNamespace(
        prefix=sdk,
        llvm_config=config,
        version="22.1.8",
        host_target="x86_64-unknown-linux-gnu",
        targets=("WebAssembly", "X86"),
        release=llvm_toolchain.llvm_release(
            "22.1.8", Path(__file__).resolve().parents[2]
        ),
        link_closure=("lib/libLLVMCore.a",),
        content_facts=(),
        library_facts=(
            llvm_toolchain.LlvmLibraryFact(
                "lib/libLLVMCore.a", 11, archive.stat().st_mtime_ns
            ),
        ),
        tool_versions=(
            llvm_toolchain.LlvmToolVersionFact(
                "clang++",
                "bin/clang++",
                "22.1.8",
                6,
                hashlib.sha256(b"driver").hexdigest(),
            ),
            llvm_toolchain.LlvmToolVersionFact(
                "llvm-config",
                "bin/llvm-config",
                "22.1.8",
                13,
                hashlib.sha256(b"configuration").hexdigest(),
            ),
        ),
    )
    monkeypatch.setattr(
        llvm_toolchain,
        "verify_available_llvm_toolchain",
        lambda *_args, **_kwargs: verification,
    )
    # This producer control models Linux. Darwin SDK selection has its own
    # actual selected-platform authority and source-level projection controls.
    monkeypatch.setattr(llvm_toolchain, "sys", SimpleNamespace(platform="linux"))
    observations = []

    def identities(*_args):
        result = copy.deepcopy(template["tools"])
        if observations and failure == "tool":
            result["cargo"]["sha256"] = "d" * 64
        return result

    monkeypatch.setattr(native_build, "_tool_identities", identities)

    def cargo(argv, *, cwd, env, check):
        observations.append((argv, cwd, env))
        assert check and "--locked" in argv and "--target" in argv
        assert (
            "--manifest-path" in argv
            and Path(argv[argv.index("--manifest-path") + 1]).parent != tmp_path
        )
        if failure == "cargo":
            raise RuntimeError("build failed")
        if argv[argv.index("--bin") + 1] == "molt-backend":
            assert env["LLVM_SYS_221_PREFIX"] == str(sdk)
            assert env["LLVM_CONFIG_PATH"] == str(config)
            assert set(argv[argv.index("--features") + 1].split(",")) == {
                "llvm",
                "luau-backend",
                "native-backend",
                "rust-backend",
                "wasm-backend",
            }
            if failure == "llvm":
                archive.write_bytes(b"substituted")
        else:
            assert "LLVM_SYS_221_PREFIX" not in env
            if argv[argv.index("--bin") + 1] == "molt-wasm-facts":
                assert argv[argv.index("--package") + 1] == "molt-wasm-facts"
                assert argv[argv.index("--features") + 1] == "host-tool"
                assert argv[argv.index("--profile") + 1] == "release"
        profile = argv[argv.index("--profile") + 1]
        binary = argv[argv.index("--bin") + 1]
        output = cwd / "target" / "x86_64-unknown-linux-gnu" / profile / binary
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(native_image() + binary.encode())
        if failure == "source":
            (cwd / "source" / "rust-toolchain.toml").write_text(
                "mutated", encoding="utf-8"
            )

    monkeypatch.setattr(native_build, "_COMMANDS", SimpleNamespace(run=cargo))
    output = tmp_path / "published"
    if failure:
        with pytest.raises((ValueError, RuntimeError)):
            native_build.produce_native_build(
                tmp_path,
                output,
                source_sha=snapshot.source_sha,
                source_date_epoch=1_700_000_000,
                platform="linux",
                arch="x86_64",
                build_root=tmp_path / "isolated-builds",
            )
        assert not output.exists()
    else:
        receipt = native_build.produce_native_build(
            tmp_path,
            output,
            source_sha=snapshot.source_sha,
            source_date_epoch=1_700_000_000,
            platform="linux",
            arch="x86_64",
            build_root=tmp_path / "isolated-builds",
        )
        assert len(observations) == 4
        assert len(audit_observations) == 1
        assert audit_observations[0][1] == {"platform": "linux", "arch": "x86_64"}
        assert set(receipt["artifacts"]) == {
            "compiler",
            "launcher",
            "worker",
            "wasm_facts",
        }
        assert {entry.name for entry in output.iterdir()} == {
            "bin",
            native_build.RECEIPT_NAME,
        }
        with pytest.raises(FileExistsError):
            native_build.produce_native_build(
                tmp_path,
                output,
                source_sha=snapshot.source_sha,
                source_date_epoch=1_700_000_000,
                platform="linux",
                arch="x86_64",
                build_root=tmp_path / "isolated-builds",
            )


@pytest.mark.parametrize("difference", ["same-root", "tool", "worker", "wasm_facts"])
def test_candidate_requires_independent_equal_receipts_and_bytes(
    tmp_path, snapshot, monkeypatch, difference
):
    from tools.release import release_authority

    first = tmp_path / "first"
    second = tmp_path / "second"
    native_build_fixture(first, snapshot)
    receipt = native_build_fixture(second, snapshot)
    if difference == "tool":
        receipt["tools"]["cc"]["sha256"] = "d" * 64
        write_exact(second / native_build.RECEIPT_NAME, receipt)
    elif difference in {"worker", "wasm_facts"}:
        (second / receipt["artifacts"][difference]["path"]).write_bytes(
            native_image() + b"different"
        )
    else:
        second = first
    runtime_first, runtime_second = (
        tmp_path / "runtime-first",
        tmp_path / "runtime-second",
    )
    runtime_first.mkdir()
    runtime_second.mkdir()
    monkeypatch.setattr(release_authority, "source_snapshot", lambda *_: snapshot)
    monkeypatch.setattr(release_authority, "snapshot_rust_channel", lambda *_: "1.96.1")
    with pytest.raises(
        ValueError, match="distinct output roots|not reproducible|binary differs"
    ):
        release_authority.assemble_candidate(
            target_id="linux-x86_64",
            version="0.0.001",
            source_sha=snapshot.source_sha,
            source_date_epoch=1_700_000_000,
            wheel=tmp_path / "unused.whl",
            primary_native_build=first,
            secondary_native_build=second,
            primary_runtime_cells=runtime_first,
            secondary_runtime_cells=runtime_second,
            output=tmp_path / "candidate",
        )
    assert not (tmp_path / "candidate").exists()


@pytest.mark.parametrize(
    "mutation",
    [
        "absent",
        "shared",
        "version",
        "source",
        "archive",
        "system",
        "config",
        "host",
        "boolean",
    ],
)
def test_native_receipt_requires_complete_static_llvm_inputs(
    tmp_path, snapshot, mutation
):
    receipt = native_build_fixture(tmp_path, snapshot)
    inputs = receipt["llvm"]
    if mutation == "absent":
        del receipt["llvm"]
    elif mutation == "shared":
        inputs["linkage"] = "prefer-static"
    elif mutation == "version":
        inputs["version"] = "23.1.0"
    elif mutation == "source":
        inputs["upstream_release"]["source_sha256"] = "f" * 64
    elif mutation == "archive":
        inputs["link_plan"]["libraries"][0]["path"] = (
            "${compiler/llvm-sdk/lib/libLLVM.so}"
        )
    elif mutation == "system":
        row = copy.deepcopy(inputs["link_plan"]["libraries"][0])
        row.update(source="system", path="${compiler/llvm-system/1}")
        inputs["link_plan"]["libraries"].append(row)
    elif mutation == "config":
        inputs["resources"]["roots"].remove("compiler/llvm-config")
    elif mutation == "host":
        inputs["targets"] = ["WebAssembly"]
    else:
        inputs["llvm_config"]["size"] = True
    with pytest.raises(ValueError):
        native_build.validate_receipt(receipt)


def test_native_receipt_binds_sdk_policy_to_original_source_inventory(
    tmp_path, snapshot
):
    receipt = native_build_fixture(tmp_path, snapshot)
    receipt["llvm"]["policy_sha256"] = "f" * 64
    write_exact(tmp_path / native_build.RECEIPT_NAME, receipt)
    with pytest.raises(ValueError, match="LLVM policy differs"):
        native_build.read_native_build(
            tmp_path,
            snapshot=snapshot,
            expected_rust_channel="1.96.1",
            source_date_epoch=1_700_000_000,
            platform="linux",
            arch="x86_64",
        )


@pytest.mark.parametrize("rule", list(NameRule))
def test_build_environment_filters_and_retains_child_name_rule(tmp_path, rule):
    source = tmp_path / "source"
    source.mkdir()
    (source / "rust-toolchain.toml").write_text(
        '[toolchain]\nchannel="1.96.1"\n', encoding="utf-8"
    )
    inherited = ProcessEnvironment(
        {
            "Path": "alternate",
            "PATH": "tools",
            "home": "shadow",
            "HOME": str(tmp_path / "home"),
            "Rustup_Home": str(tmp_path / "rustup"),
        },
        rule=rule,
    )
    env = native_build.build_environment(
        source, tmp_path / "work", inherited, epoch=123
    )
    assert {name: value for name, value in env.items() if name in inherited} == (
        {
            "Path": "tools",
            "home": str(tmp_path / "home"),
            "Rustup_Home": str(tmp_path / "rustup"),
        }
        if rule is NameRule.WINDOWS
        else {"PATH": "tools", "HOME": str(tmp_path / "home")}
    )
    assert env.rule is rule
    env["PATH"] = "selected tools"
    assert env["PATH"] == "selected tools"
    assert ("Path" in set(env)) is (rule is NameRule.WINDOWS)
    assert inherited["PATH"] == "tools"


@pytest.mark.parametrize("rule", list(NameRule))
def test_native_producer_removes_sdk_names_with_child_rule(tmp_path, monkeypatch, rule):
    import molt.verified_subset

    monkeypatch.setattr(
        molt.verified_subset, "current_host_coordinate", lambda: ("linux", "x86_64")
    )
    install_module_view(
        monkeypatch,
        "os",
        os,
        native_build,
        environ=ProcessEnvironment(
            {"SdkRoot": "mixed", "SDKROOT": "canonical"}, rule=rule
        ),
    )
    monkeypatch.setattr(native_build, "source_snapshot", lambda *_args: object())
    monkeypatch.setattr(
        native_build, "resolve_executable", lambda *_args, **_kwargs: tmp_path / "git"
    )

    def materialize(_snapshot, output, **_kwargs):
        output.mkdir()
        return output

    from tools.release import binary_compatibility

    capability = ({"shared_libraries": ()}, frozenset())
    monkeypatch.setattr(
        binary_compatibility,
        "linux_shared_library_capability",
        lambda _arch: capability,
    )

    def admit_llvm(_source, inherited, *, shared_capability):
        assert shared_capability is capability
        assert dict(inherited) == (
            {} if rule is NameRule.WINDOWS else {"SdkRoot": "mixed"}
        )
        raise RuntimeError("observed SDK environment")

    monkeypatch.setattr(native_build, "materialize_git_source_snapshot", materialize)
    monkeypatch.setattr(native_build, "admit_llvm_compiler_inputs", admit_llvm)
    output = tmp_path / "output"
    with pytest.raises(RuntimeError, match="observed SDK environment"):
        native_build.produce_native_build(
            tmp_path,
            output,
            source_sha="a" * 40,
            source_date_epoch=123,
            platform="linux",
            arch="x86_64",
            build_root=tmp_path / "builds",
        )
    assert not output.exists()


def test_native_receipt_rejects_consistent_foreign_sdk_pointer_abi(tmp_path, snapshot):
    # Architecture-family membership and an internally consistent SDK shape are
    # insufficient: this candidate's actual Rust target has 64-bit pointers.
    receipt = native_build_fixture(tmp_path, snapshot)
    plan = receipt["llvm"]["link_plan"]
    plan["host_target"] = "x86_64-unknown-linux-gnux32"
    plan["host_abi"].update(pointer_bits=32, header_bits=32)
    with pytest.raises(ValueError, match="LLVM.*ABI"):
        native_build.validate_receipt(receipt)
