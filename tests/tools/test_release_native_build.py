"""Native provenance admission; synthetic headers are not executable proof."""

from __future__ import annotations

from contextlib import contextmanager
import copy
import hashlib
import json
from pathlib import Path, PurePosixPath
from types import SimpleNamespace

import pytest

from molt.exact_json import write_exact
from molt.release_matrix import RUST_TARGET_BY_COORDINATE
from tests.native_artifact_fixtures import elf_header, macho_header, pe_header
from tests.runtime_build_identity_helper import build_python_identity_fixture
from tools.release import native_build
from tools.release.git_source_snapshot import GitSourceFile, GitSourceSnapshot


def native_image(platform="linux", arch="x86_64"):
    if platform == "windows":
        return bytes(pe_header(machine=0x8664 if arch == "x86_64" else 0xAA64))
    if platform == "macos":
        return bytes(macho_header(cpu=0x01000007 if arch == "x86_64" else 0x0100000C))
    return bytes(elf_header(machine=62 if arch == "x86_64" else 183))


def native_build_fixture(
    root, snapshot, *, platform="linux", arch="x86_64", epoch=1_700_000_000
):
    """Transport fixture with real schema/header admission and no host tools."""
    triple = RUST_TARGET_BY_COORDINATE[(platform, arch)]
    plans = native_build.component_plan(platform, arch)
    python = build_python_identity_fixture()
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
            "python",
            "cmake",
            "ninja",
            *(("nasm",) if (platform, arch) == ("windows", "x86_64") else ()),
        )
    }
    tools["cargo"]["version"] = "cargo 1.96.1 (fixture 2026-01-01)"
    tools["rustc"]["version"] = f"rustc 1.96.1 (fixture 2026-01-01)\nhost: {triple}"
    tools["python"] = {**python["selected_executable"], "version": ""}
    receipt = {
        "schema": native_build.SCHEMA,
        "source": native_build.source_record(snapshot),
        "source_date_epoch": epoch,
        "target": {"platform": platform, "arch": arch, "rust_target": triple},
        "policy": {
            "revision": 1,
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
        "build_python": python,
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
    data = b'[toolchain]\nchannel="1.96.1"\n'
    entry = GitSourceFile(
        PurePosixPath("rust-toolchain.toml"),
        0o100644,
        "b" * 40,
        len(data),
        hashlib.sha256(data).hexdigest(),
    )
    return GitSourceSnapshot(
        "a" * 40, "c" * 40, "sha1", ("rust-toolchain.toml",), (entry,)
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


@pytest.mark.parametrize("role", ["compiler", "launcher", "worker"])
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


def test_worker_architecture_is_admitted_even_with_self_consistent_hash(
    tmp_path, snapshot
):
    receipt = native_build_fixture(tmp_path, snapshot)
    wrong = native_image("linux", "aarch64")
    record = receipt["artifacts"]["worker"]
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
        "python",
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
    elif mutation == "python":
        receipt["tools"]["python"]["sha256"] = "d" * 64
    else:
        receipt["schema"] = "molt.release-worker-build.v1"
    with pytest.raises(ValueError):
        native_build.validate_receipt(receipt)


def test_environment_drops_ambient_policy_and_isolates_cargo(tmp_path):
    source = tmp_path / "source"
    source.mkdir()
    (source / "rust-toolchain.toml").write_text('[toolchain]\nchannel="1.96.1"\n')
    inherited = {
        "PATH": "tools",
        "CARGO_HOME": "ambient",
        "CARGO_PROFILE_RELEASE_OPT_LEVEL": "0",
        "RUSTFLAGS": "-Ctarget-cpu=native",
        "RUSTC": "fake",
        "RUSTUP_TOOLCHAIN": "nightly",
        "CC": "fake",
        "CFLAGS": "-march=native",
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
    assert env["RUSTUP_TOOLCHAIN"] == "1.96.1"
    assert env["VCINSTALLDIR"] == "activated-vc"
    assert env["VSINSTALLDIR"] == "activated-vs"
    assert not {
        "RUSTFLAGS",
        "RUSTC",
        "CFLAGS",
        "CC",
        "PYTHONPATH",
        "MOLT_RUNTIME_FEATURES",
        "CARGO_PROFILE_RELEASE_OPT_LEVEL",
    }.intersection(env)


def test_build_cwd_rejects_ancestor_cargo_configuration(tmp_path):
    (tmp_path / "rust-toolchain.toml").write_text('[toolchain]\nchannel="1.96.1"\n')
    config = tmp_path / ".cargo" / "config.toml"
    config.parent.mkdir()
    config.write_text("[profile.release]\nopt-level=0\n")
    with pytest.raises(ValueError, match="ambient Cargo configuration") as failure:
        native_build.build_environment(tmp_path, tmp_path / "build", {}, epoch=123)
    assert str(config) in str(failure.value)
    assert "--build-root" in str(failure.value)


def test_explicit_build_root_avoids_unrelated_user_configuration(tmp_path):
    poisoned = tmp_path / "user-profile"
    config = poisoned / ".cargo/config.toml"
    config.parent.mkdir(parents=True)
    config.write_text("[build]\n")
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


def test_python_tool_cross_check_reaches_cross_check_after_valid_runtime(
    tmp_path, snapshot
):
    receipt = native_build_fixture(tmp_path, snapshot)
    receipt["tools"]["python"]["sha256"] = "d" * 64
    with pytest.raises(
        ValueError, match="Python closure differs from selected executable"
    ):
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


@pytest.mark.parametrize("failure", [None, "source", "tool", "cargo", "python"])
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
        (output / "rust-toolchain.toml").write_bytes(b'[toolchain]\nchannel="1.96.1"\n')
        return output

    monkeypatch.setattr(native_build, "materialize_git_source_snapshot", materialize)
    monkeypatch.setattr(
        native_build,
        "_tool_paths",
        lambda *_args, **_kwargs: {role: tmp_path / role for role in template["tools"]},
    )
    observations = []

    def identities(*_args):
        result = copy.deepcopy(template["tools"])
        if observations and failure == "tool":
            result["cargo"]["sha256"] = "d" * 64
        return result

    monkeypatch.setattr(native_build, "_tool_identities", identities)

    @contextmanager
    def python_scope(_state):
        def capture(_env):
            result = copy.deepcopy(template["build_python"])
            if observations and failure == "python":
                result["identity_sha256"] = "d" * 64
            return result

        yield SimpleNamespace(capture=capture)

    monkeypatch.setattr(native_build, "build_python_scope", python_scope)

    def cargo(argv, *, cwd, env, check):
        observations.append((argv, cwd, env))
        assert check and "--locked" in argv and "--target" in argv
        assert (
            "--manifest-path" in argv
            and Path(argv[argv.index("--manifest-path") + 1]).parent != tmp_path
        )
        if failure == "cargo":
            raise RuntimeError("build failed")
        profile = argv[argv.index("--profile") + 1]
        binary = argv[argv.index("--bin") + 1]
        output = cwd / "target" / "x86_64-unknown-linux-gnu" / profile / binary
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(native_image() + binary.encode())
        if failure == "source":
            (cwd / "source" / "rust-toolchain.toml").write_text("mutated")

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
        assert len(observations) == 3
        assert set(receipt["artifacts"]) == {"compiler", "launcher", "worker"}
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


@pytest.mark.parametrize("difference", ["same-root", "tool", "worker"])
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
    elif difference == "worker":
        (second / receipt["artifacts"]["worker"]["path"]).write_bytes(
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
