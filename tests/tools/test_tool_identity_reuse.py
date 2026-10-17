"""Content-addressed toolchain identity reuse re-proves every byte it reuses."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from dataclasses import replace

import pytest

from tools import proof_plan
from tools.proof_queue_pkg import (
    command_admission,
    command_identity,
    process_image_capture,
)

pytestmark = pytest.mark.usefixtures("admitted_build_capacity")


def test_selected_content_capture_versions_content_and_rechecks_resolver_before_reuse(
    tmp_path, monkeypatch
):
    uv = tmp_path / "uv"
    uv.write_bytes(b"unchanged resolver launcher")
    first_python = tmp_path / "python-first"
    first_python.write_bytes(b"first unchanged interpreter")
    second_python = tmp_path / "python-second"
    second_python.write_bytes(b"second interpreter")
    selected = first_python
    policy = proof_plan.ToolchainPolicy(
        "selected-content",
        {
            "name": "selected-content",
            "executable": "uv",
            "version_args": ["--version"],
            "version_from_content_path": True,
            "version_pattern": r"^Python 3\.15\.[0-9]+$",
            "content_path_command": [
                "uv",
                "python",
                "find",
                "--no-python-downloads",
                "3.15",
            ],
        },
        tmp_path,
    )
    plan = replace(proof_plan.ProofPlan.load(), toolchain_policies=(policy,))
    calls = []

    def run(command, *, cwd, env, timeout=30):
        argv = list(command)
        calls.append(argv)
        if "find" in argv:
            output = str(selected) + "\n"
        elif argv[0] == str(first_python):
            output = "Python 3.15.1\n"
        elif argv[0] == str(second_python):
            output = "Python 3.15.2\n"
        else:
            output = "uv 0.13.1\n"
        return subprocess.CompletedProcess(argv, 0, output, "")

    monkeypatch.setattr(
        command_identity, "_which_in_command_environment", lambda *args, **kwargs: uv
    )
    monkeypatch.setattr(command_identity, "_run_captured", run)
    telemetry = []

    def capture():
        identity = command_identity._tool_identity(
            plan,
            policy.name,
            {"python": None},
            [sys.executable, "-c", "pass"],
            cwd=tmp_path,
            env={},
            reuse_root=tmp_path / "reuse",
            reuse_telemetry=telemetry,
        )
        command_identity._validate_toolchain_identity(plan, policy.name, identity)
        return identity

    first = capture()
    assert first["version"] == "Python 3.15.1"
    assert first["path"] == str(uv) and first["content_path"] == str(first_python)
    assert capture() == first
    assert [t["state"] for t in telemetry] == ["miss", "hit"]
    selected = second_python
    second = capture()
    assert second["version"] == "Python 3.15.2"
    assert second["content_path"] == str(second_python)
    assert telemetry[-1]["state"] == "miss"
    assert len([argv for argv in calls if "find" in argv]) == 3
    assert [
        argv for argv in calls if argv[0] in {str(first_python), str(second_python)}
    ] == [[str(first_python), "--version"], [str(second_python), "--version"]]


def _git_probe(tmp_path: Path) -> tuple[dict[str, object], list[str], dict[str, str]]:
    if shutil.which("git") is None:
        pytest.skip("git toolchain is not installed")
    command = ["git", "status"]
    envelope = command_admission.envelope_for_command(command)
    env = {
        name: value
        for name, value in os.environ.items()
        if name not in command_identity.OPERATIONAL_CARGO_NAMES
    }
    env["TMPDIR"] = str(tmp_path)
    return envelope, command, env


def _identity(
    tmp_path: Path,
    reuse_root: Path,
    *,
    env_overrides: dict[str, str] | None = None,
) -> tuple[dict[str, object], list[dict[str, object]]]:
    envelope, command, env = _git_probe(tmp_path)
    telemetry: list[dict[str, object]] = []
    identity = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "git",
        envelope,
        command,
        cwd=tmp_path,
        env={**env, **(env_overrides or {})},
        reuse_root=reuse_root,
        reuse_telemetry=telemetry,
    )
    return identity, telemetry


def test_second_capture_reuses_the_record_without_running_the_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    reuse_root = tmp_path / "tool-identity"
    first, first_telemetry = _identity(tmp_path, reuse_root)
    assert [row["state"] for row in first_telemetry] == ["miss"]
    assert first_telemetry[0]["reason"] == "absent"
    record = Path(str(first_telemetry[0]["record"]))
    assert record.is_file()
    stored = json.loads(record.read_text(encoding="utf-8"))
    assert stored["schema"] == command_identity.TOOL_IDENTITY_REUSE_SCHEMA
    assert stored["identity"] == first

    def forbidden(*args: object, **kwargs: object) -> None:
        raise AssertionError("a reused identity must not run its version probe")

    monkeypatch.setattr(command_identity, "_run_captured", forbidden)
    second, second_telemetry = _identity(tmp_path, reuse_root)
    assert second == first
    assert [row["state"] for row in second_telemetry] == ["hit"]
    assert second_telemetry[0]["revalidated_images"] == len(first["process_images"])


def test_reuse_misses_when_a_recorded_image_no_longer_hashes_the_same(
    tmp_path: Path,
) -> None:
    reuse_root = tmp_path / "tool-identity"
    first, first_telemetry = _identity(tmp_path, reuse_root)
    record = Path(str(first_telemetry[0]["record"]))
    stored = json.loads(record.read_text(encoding="utf-8"))
    stored["identity"]["process_images"][0]["sha256"] = "0" * 64
    record.write_text(json.dumps(stored), encoding="utf-8")

    second, second_telemetry = _identity(tmp_path, reuse_root)
    assert second == first
    assert second_telemetry[0]["state"] == "miss"
    assert second_telemetry[0]["reason"] == "revalidation-drift"
    # The fresh capture replaces the drifted record.
    restored = json.loads(record.read_text(encoding="utf-8"))
    assert restored["identity"] == first


def test_reuse_key_binds_resolution_environment_and_probe_cwd(
    tmp_path: Path,
) -> None:
    reuse_root = tmp_path / "tool-identity"
    _identity(tmp_path, reuse_root)
    _, changed_path = _identity(
        tmp_path,
        reuse_root,
        env_overrides={"PATH": os.environ["PATH"] + os.pathsep + str(tmp_path)},
    )
    assert changed_path[0]["state"] == "miss"
    assert changed_path[0]["reason"] == "absent"
    other_cwd = tmp_path / "elsewhere"
    other_cwd.mkdir()
    envelope, command, env = _git_probe(tmp_path)
    telemetry: list[dict[str, object]] = []
    command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "git",
        envelope,
        command,
        cwd=other_cwd,
        env=env,
        reuse_root=reuse_root,
        reuse_telemetry=telemetry,
    )
    assert telemetry[0]["state"] == "miss"
    assert len(list(reuse_root.glob("*.json"))) == 3


def test_malformed_or_oversized_records_degrade_to_a_fresh_capture(
    tmp_path: Path,
) -> None:
    reuse_root = tmp_path / "tool-identity"
    first, first_telemetry = _identity(tmp_path, reuse_root)
    record = Path(str(first_telemetry[0]["record"]))
    record.write_text("{not json", encoding="utf-8")
    second, telemetry = _identity(tmp_path, reuse_root)
    assert second == first
    assert telemetry[0] == {
        **telemetry[0],
        "state": "miss",
        "reason": "absent",
    }
    record.write_bytes(b"{" + b" " * command_identity._MAX_REUSE_RECORD_BYTES + b"}")
    _, telemetry = _identity(tmp_path, reuse_root)
    assert telemetry[0]["state"] == "miss"


def test_revalidation_rejects_changed_bytes_and_foreign_fields(tmp_path: Path) -> None:
    image = tmp_path / ("tool.exe" if sys.platform == "win32" else "tool")
    image.write_bytes(b"#!/bin/sh\nexit 0\n")
    image.chmod(0o755)
    row = process_image_capture.capture_image("probe", image)
    material: dict[str, object] = {
        "path": str(image),
        "launcher_sha256": row["sha256"],
        "content_path": str(image),
        "executable_sha256": row["sha256"],
        "version": "probe 1.0",
        "probe_cwd": str(tmp_path),
        "policy_sha256": "a" * 64,
        "configuration_files": [],
        "process_images": [row],
    }
    material["identity_sha256"] = command_identity.hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    current = command_identity._reused_identity_is_current(
        proof_plan.ToolchainPolicy("probe", {}),
        material,
        cwd=tmp_path,
        env={},
        command_argv=["probe"],
    )
    assert current

    foreign = {**material, "runtime": {"execPath": str(image)}}
    assert not command_identity._reused_identity_is_current(
        proof_plan.ToolchainPolicy("probe", {}),
        foreign,
        cwd=tmp_path,
        env={},
        command_argv=["probe"],
    )

    tampered = dict(material)
    tampered["version"] = "probe 2.0"
    assert not command_identity._reused_identity_is_current(
        proof_plan.ToolchainPolicy("probe", {}),
        tampered,
        cwd=tmp_path,
        env={},
        command_argv=["probe"],
    )

    image.write_bytes(b"#!/bin/sh\nexit 1\n")
    assert not command_identity._reused_identity_is_current(
        proof_plan.ToolchainPolicy("probe", {}),
        material,
        cwd=tmp_path,
        env={},
        command_argv=["probe"],
    )


def test_compile_environment_selection_is_one_authority() -> None:
    env = {
        "CARGO": "/selected/cargo",
        "RUSTFLAGS": "-C opt-level=1",
        "CARGO_TARGET_DIR": "/elsewhere",
        "CARGO_PROFILE_RELEASE_LTO": "fat",
        "CC_x86_64_unknown_linux_gnu": "clang",
        "PATH": "/usr/bin",
        "MOLT_CUSTOM": "1",
        "AWS_LC_SYS_SMALL": "0",
        "AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu": "0",
    }
    selected = command_identity.compile_environment_selection(
        env, configured_names=["MOLT_CUSTOM"]
    )
    assert set(selected) == {
        "CARGO",
        "RUSTFLAGS",
        "CARGO_PROFILE_RELEASE_LTO",
        "CC_x86_64_unknown_linux_gnu",
        "MOLT_CUSTOM",
        "AWS_LC_SYS_SMALL",
        "AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu",
    }
    probe = command_identity._probe_environment_selection(env)
    assert set(probe) == {
        "CARGO",
        "RUSTFLAGS",
        "CARGO_PROFILE_RELEASE_LTO",
        "CC_x86_64_unknown_linux_gnu",
        "PATH",
        "AWS_LC_SYS_SMALL",
        "AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu",
    }


@pytest.mark.parametrize(
    "name",
    [
        "RUSTDOCFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS",
        "RUSTC_BOOTSTRAP",
        "__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS",
        "CARGO_UNSTABLE_PROFILE_RUSTFLAGS",
        "AWS_LC_SYS_SMALL",
        "AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu",
        "AWS_LC_SYS_PROJECT_NOTE",
        "HOST_CFLAGS_aarch64_unknown_linux_gnu",
        "TARGET_CFLAGS_aarch64_unknown_linux_gnu",
    ],
)
def test_rust_operation_environment_changes_invalidate_probe_reuse(name):
    assert command_identity._probe_environment_selection(
        {name: "first"}
    ) != command_identity._probe_environment_selection({name: "second"})
    assert name in command_identity.compile_environment_selection({name: "first"})


def test_native_dependency_empty_target_controls_remain_compile_and_probe_inputs():
    generic = {"AWS_LC_SYS_CMAKE_BUILDER": "1"}
    control = "AWS_LC_SYS_CMAKE_BUILDER_x86_64_unknown_linux_gnu"
    selected = {**generic, control: ""}
    compile_environment = command_identity.compile_environment_selection(selected)
    assert compile_environment[control] == (
        "12ae32cb1ec02d01eda3581b127c1fee3b0dc53572ed6baf239721a03d82e126"
    )
    assert compile_environment != command_identity.compile_environment_selection(
        generic
    )
    assert command_identity._probe_environment_selection(selected) != (
        command_identity._probe_environment_selection(generic)
    )


@pytest.mark.parametrize("explicit", [False, True])
@pytest.mark.parametrize("selected_name", ["cargo", "custom-cargo"])
def test_cargo_capture_reuse_and_rust_link_probe_share_bound_payload(
    tmp_path, monkeypatch, explicit, selected_name
):
    from tools.proof_queue_pkg import toolchain_capture

    suffix = ".exe" if os.name == "nt" else ""
    paths = {}
    for role in ("selected", "explicit", "decoy"):
        directory = tmp_path / role
        directory.mkdir()
        path = directory / ((selected_name if role == "selected" else "cargo") + suffix)
        path.write_bytes((role + " cargo image").encode())
        path.chmod(0o755)
        paths[role] = path
    rustc = tmp_path / ("rustc" + suffix)
    rustc.write_bytes(b"independent rustc image")
    rustc.chmod(0o755)
    payload = [
        str(paths["explicit"]) if explicit else "cargo",
        "build",
        "--target",
        "wasm32-wasip1",
    ]
    command = [sys.executable, "tools/guarded_exec.py", "--", *payload]
    env = {
        "CARGO": str(paths["selected"]),
        "RUSTC": str(rustc),
        "PATH": str(paths["decoy"].parent),
    }
    envelope = command_admission.envelope_for_command(command)
    exact = command_identity._exact_command(envelope, cwd=proof_plan.ROOT, env=env)
    command_identity._bind_delegated_command(
        envelope, exact, cwd=proof_plan.ROOT, env=env
    )
    expected = paths["explicit"] if explicit else paths["selected"]
    versions = []

    def version(argv, **kwargs):
        versions.append(list(argv))
        return subprocess.CompletedProcess(argv, 0, "cargo 1.99.0\n", "")

    monkeypatch.setattr(command_identity, "_run_captured", version)
    plan = proof_plan.ProofPlan.load()
    telemetry = []
    first = command_identity._tool_identity(
        plan,
        "cargo",
        envelope,
        exact,
        cwd=proof_plan.ROOT,
        env=env,
        reuse_root=tmp_path / "reuse",
        reuse_telemetry=telemetry,
    )
    second = command_identity._tool_identity(
        plan,
        "cargo",
        envelope,
        exact,
        cwd=proof_plan.ROOT,
        env=env,
        reuse_root=tmp_path / "reuse",
        reuse_telemetry=telemetry,
    )
    # The selected launcher is a custody coordinate, while the Rust version
    # probe names the OS-resolved physical component.
    assert first == second and first["path"] == process_image_capture._image_path_key(
        expected
    )
    assert [row["state"] for row in telemetry] == ["miss", "hit"]
    assert versions == [[str(expected.resolve(strict=True)), "--version"]]
    # The Rust link capture must use that same executable for metadata/build
    # probes, even though CARGO and PATH conflict with an explicit payload.
    probes = []

    def capture(**kwargs):
        probes.append(kwargs)
        return [], {}

    monkeypatch.setattr(toolchain_capture, "capture_rust_link_process_images", capture)
    policy = next(row for row in plan.toolchain_policies if row.name == "rustc")
    command_identity._capture_tool_identity(
        policy,
        "rustc",
        envelope,
        exact,
        path=rustc,
        selected_content_path=rustc,
        content_resolver_identity=None,
        probe_cwd=proof_plan.ROOT,
        policy_sha256=command_identity.canonical_json_sha256(policy.data),
        cwd=proof_plan.ROOT,
        env=env,
    )
    assert len(probes) == 1
    assert probes[0]["cargo"] == expected
    assert probes[0]["target"] == "wasm32-wasip1"
    assert probes[0]["command_argv"][0] == str(expected)
    # Content mutation defeats warm reuse rather than preserving a stale image.
    expected.write_bytes(expected.read_bytes() + b" changed")
    changed = []
    third = command_identity._tool_identity(
        plan,
        "cargo",
        envelope,
        exact,
        cwd=proof_plan.ROOT,
        env=env,
        reuse_root=tmp_path / "reuse",
        reuse_telemetry=changed,
    )
    assert changed[0]["state"] == "miss"
    assert third["executable_sha256"] != first["executable_sha256"]


def test_python_declaring_cargo_uses_path_independently_of_explicit_hook(
    tmp_path, monkeypatch
):
    selected = tmp_path / ("cargo.exe" if os.name == "nt" else "cargo")
    selected.write_bytes(b"declared dependency Cargo")
    selected.chmod(0o755)
    decoy = tmp_path / "explicit-cargo"
    decoy.write_bytes(b"explicit Cargo hook")
    decoy.chmod(0o755)
    command = [sys.executable, "-c", "pass"]
    envelope = command_admission.envelope_for_command(command)
    monkeypatch.setattr(
        command_identity,
        "_capture_tool_identity",
        lambda *args, **kwargs: {"path": str(kwargs["path"])},
    )
    identity = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "cargo",
        envelope,
        command,
        cwd=tmp_path,
        env={"CARGO": str(decoy), "PATH": str(selected.parent)},
    )
    assert identity == {"path": process_image_capture._image_path_key(selected)}


def test_wasi_compiler_identity_selects_sdk_before_outer_command_or_path(
    tmp_path, monkeypatch
):
    from molt import llvm_toolchain

    compiler = tmp_path / "sdk" / "clang"
    compiler.parent.mkdir()
    compiler.write_bytes(b"SDK image")
    selected = []

    def resolve(root, role, *, environ, cwd):
        # The request cwd anchors a relative tool-state selection.
        assert root == proof_plan.ROOT
        assert cwd == tmp_path
        assert environ == {}
        selected.append(role)
        return compiler

    monkeypatch.setattr(llvm_toolchain, "resolve_wasi_sdk_tool", resolve)
    monkeypatch.setattr(
        command_identity,
        "_resolve_outer_executable",
        lambda *a, **k: pytest.fail("native outer compiler selected"),
    )
    monkeypatch.setattr(
        command_identity,
        "_which_in_command_environment",
        lambda *a, **k: pytest.fail("native PATH selected"),
    )
    monkeypatch.setattr(
        command_identity,
        "_capture_tool_identity",
        lambda *a, **k: {"path": str(k["path"])},
    )
    result = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "wasi-clang",
        {},
        ["clang", "example.c"],
        cwd=tmp_path,
        env={},
    )
    assert result["path"] == process_image_capture._image_path_key(compiler)
    assert selected == ["clang"]


@pytest.mark.parametrize(
    "mutation",
    [
        "header",
        "new-header",
        "compiler-rt",
        "linker",
        "missing-closure",
        "empty-resources",
        "missing-lib",
        "missing-sysroot",
        "substituted-resource",
        "missing-helper",
        "substituted-helper",
        "malformed-helper",
        "missing-receipt-key",
        "substituted-receipt",
        "substituted-sdk",
        "substituted-selection",
    ],
)
def test_sdk_identity_roundtrip_reuses_only_complete_current_closure(
    tmp_path, monkeypatch, mutation
):
    import subprocess

    from molt.llvm_toolchain import wasi_c_abi_plan
    from tests.runtime_build_identity_helper import (
        RuntimeFixtureRoot,
        provisioned_wasi_sdk_fixture,
    )

    installation = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    abi = wasi_c_abi_plan(installation)
    plan = proof_plan.ProofPlan.load()
    env = {"WASI_SDK_PATH": str(installation.sdk)}
    probes = []

    def version(argv, **kwargs):
        # The fixture contains real image bytes but is not an executing SDK.
        # Only the version process is substituted; selection/capture/cache/receiver are real.
        probes.append(tuple(argv))
        assert tuple(argv) == (
            str(process_image_capture.custody_path(abi.driver)),
            "--version",
        )
        return subprocess.CompletedProcess(argv, 0, "clang version 23.1.0\n", "")

    monkeypatch.setattr(command_identity, "_run_captured", version)

    def capture():
        telemetry = []
        identity = command_identity._tool_identity(
            plan,
            "wasi-clang",
            {},
            ["clang", "input.c"],
            cwd=tmp_path,
            env=env,
            reuse_root=tmp_path / "reuse",
            reuse_telemetry=telemetry,
        )
        command_identity._validate_toolchain_identity(plan, "wasi-clang", identity)
        return identity, telemetry

    first, miss = capture()
    second, hit = capture()
    assert first == second
    assert miss[0]["state"] == "miss"
    assert hit[0]["state"] == "hit"
    assert len(probes) == 1
    record_path = Path(miss[0]["record"])
    file_mutation = mutation in {"header", "new-header", "compiler-rt", "linker"}
    if file_mutation:
        path = {
            "header": abi.include / "errno.h",
            "new-header": abi.include / "added.h",
            "compiler-rt": abi.path("compiler_rt"),
            "linker": abi.linker,
        }[mutation]
        path.write_bytes((path.read_bytes() if path.exists() else b"") + b"changed")
    else:
        record = json.loads(record_path.read_text(encoding="utf-8"))
        identity = record["identity"]
        closure = identity["wasi_sdk"]
        if mutation == "missing-closure":
            del identity["wasi_sdk"]
        elif mutation == "empty-resources":
            closure["generation"]["facts"]["resources"] = {}
        elif mutation == "missing-lib":
            del closure["generation"]["facts"]["resources"]["lib"]
        elif mutation == "missing-sysroot":
            del closure["generation"]["facts"]["resources"]["share/wasi-sysroot"]
        elif mutation == "substituted-resource":
            unrelated = tmp_path / "unrelated"
            unrelated.mkdir()
            closure["generation"]["facts"]["resources"]["lib"] = (
                command_identity._directory_manifest_identity(
                    unrelated, label="independent resource"
                )
            )
        elif mutation == "missing-helper":
            identity["process_images"].pop()
        elif mutation == "substituted-helper":
            identity["process_images"][-1] = dict(
                identity["process_images"][0], role="wasi-sdk-wasm-ld"
            )
        elif mutation == "malformed-helper":
            identity["process_images"][-1]["role"] = []
        elif mutation == "missing-receipt-key":
            del closure["receipt"]["path"]
        elif mutation == "substituted-receipt":
            unrelated = tmp_path / "another-receipt.json"
            unrelated.write_text("{}", encoding="utf-8")
            closure["receipt"] = command_identity._file_identity(unrelated)
        elif mutation == "substituted-sdk":
            closure["sdk"] = str(tmp_path / "foreign" / "sdk")
        elif mutation == "substituted-selection":
            identity["path"] = str(
                installation.sdk / "bin" / ("clang++" + abi.driver.suffix)
            )
        else:
            pytest.fail(f"unhandled independent mutation {mutation}")
        identity.pop("identity_sha256")
        identity["identity_sha256"] = command_identity.canonical_json_sha256(identity)
        record_path.write_text(json.dumps(record), encoding="utf-8")
        with pytest.raises(ValueError, match="SDK"):
            command_identity._validate_toolchain_identity(plan, "wasi-clang", identity)
    if file_mutation:
        from tools.proof_queue_pkg import toolchain_capture

        if mutation == "linker":
            with pytest.raises(ValueError, match="SDK helper"):
                capture()
        else:
            third, reused = capture()
            assert third == first and reused[0]["state"] == "hit"
            with pytest.raises(ValueError, match="provisioned generation"):
                toolchain_capture.capture_wasi_sdk_resources(third["wasi_sdk"])
        return
    third, repaired = capture()
    assert repaired[0]["state"] == "miss"
    assert repaired[0]["reason"] == (
        "selection-drift"
        if mutation == "substituted-selection"
        else "revalidation-drift"
    )
    assert len(probes) == 2
    assert (third == first) == (not file_mutation)


def test_native_identity_rejects_extra_sdk_closure_even_with_valid_digest(tmp_path):
    reuse_root = tmp_path / "reuse"
    first, telemetry = _identity(tmp_path, reuse_root)
    record_path = Path(telemetry[0]["record"])
    record = json.loads(record_path.read_text(encoding="utf-8"))
    identity = record["identity"]
    identity["wasi_sdk"] = {}
    identity.pop("identity_sha256")
    identity["identity_sha256"] = command_identity.canonical_json_sha256(identity)
    record_path.write_text(json.dumps(record), encoding="utf-8")
    with pytest.raises(ValueError, match="SDK closure differs"):
        command_identity._validate_toolchain_identity(
            proof_plan.ProofPlan.load(), "git", identity
        )
    second, telemetry = _identity(tmp_path, reuse_root)
    assert telemetry[0]["state"] == "miss"
    assert telemetry[0]["reason"] == "revalidation-drift"
    assert second == first
    assert "wasi_sdk" not in second


@pytest.mark.parametrize(
    "mutation",
    [
        "none",
        "compiler",
        "helper",
        "archiver",
        "selector",
        "helper-shadow",
        "missing-unit",
    ],
)
@pytest.mark.parametrize("operation", ["c", "c++"])
def test_native_build_reuse_revalidates_actual_selection_and_images(
    tmp_path, monkeypatch, mutation, operation
):
    from tests.tools.test_toolchain_capture import _native_build_capture_fixture
    from molt.exact_json import canonical_json_sha256
    from tools.proof_queue_pkg import toolchain_capture

    identity, tools, env, command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, armed=False, operation=operation
    )
    monkeypatch.setattr(
        command_identity, "_tool_configuration_identities", lambda *args, **kwargs: []
    )
    requirements = {"target": ["c"]} if operation == "c" else {"host": ["c++"]}
    driver, frontend, selector = (
        ("selected-gcc", "cc1", "HOST_CC")
        if operation == "c"
        else ("selected-g++", "cc1plus", "HOST_CXX")
    )
    if mutation in {"compiler", "helper", "archiver"}:
        tools[
            {"compiler": driver, "helper": frontend, "archiver": "selected-ar"}[
                mutation
            ]
        ].write_bytes(b"changed")
    elif mutation == "selector":
        env[selector] = str(tools["linker"])
    elif mutation == "helper-shadow":
        first = tmp_path / "earlier"
        first.mkdir()
        shadow = first / tools["as"].name
        shadow.write_bytes(b"another assembler")
        shadow.chmod(0o755)
        env["PATH"] = str(first) + os.pathsep + env["PATH"]
    elif mutation == "missing-unit":
        identity["link_selection"]["native_build"] = []
        material = {
            key: value for key, value in identity.items() if key != "identity_sha256"
        }
        identity["identity_sha256"] = canonical_json_sha256(material)
    monkeypatch.setattr(
        toolchain_capture,
        "_run_rust_link_probe",
        lambda *args, **kwargs: pytest.fail("warm reuse executed a phase probe"),
    )
    assert command_identity._reused_identity_is_current(
        proof_plan.ToolchainPolicy("rustc", {}),
        identity,
        cwd=tmp_path,
        env=env,
        command_argv=command,
        native_units=requirements,
    ) is (mutation == "none")


@pytest.mark.parametrize("role", ["cargo", "rustc"])
def test_rust_reuse_resolves_current_component_before_cache_without_phase_reprobe(
    tmp_path, monkeypatch, role
):
    from molt import process_guard, rust_toolchain
    from tests.tools.test_toolchain_capture import _native_build_capture_fixture

    _identity, tools, environment, command, phase_calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, required=False
    )
    proxy_dir = tmp_path / "proxies"
    proxy_dir.mkdir()
    suffix = ".exe" if os.name == "nt" else ""
    proxy = proxy_dir / (role + suffix)
    rustup = proxy_dir / ("rustup" + suffix)
    for path in (proxy, rustup):
        path.write_bytes(b"same content-proven rustup proxy")
        path.chmod(0o755)
    selected = [tools[role]]
    resolutions, versions = [], []

    def which(argv, **kwargs):
        assert argv == [process_image_capture._image_path_key(rustup), "which", role]
        resolutions.append(list(argv))
        return subprocess.CompletedProcess(argv, 0, str(selected[0]) + "\n", "")

    def version(argv, **kwargs):
        assert argv[0] == str(selected[0].resolve(strict=True))
        versions.append(list(argv))
        return subprocess.CompletedProcess(
            argv, 0, f"{role} 1.99.0\nhost: x86_64-unknown-linux-gnu\n", ""
        )

    monkeypatch.setattr(process_guard, "run_completed_command", which)
    monkeypatch.setattr(command_identity, "_run_captured", version)
    monkeypatch.setattr(
        command_identity, "_which_in_command_environment", lambda *args, **kwargs: proxy
    )
    monkeypatch.setattr(
        command_identity, "_tool_configuration_identities", lambda *args, **kwargs: []
    )
    envelope = command_admission.envelope_for_command(command)
    environment = (
        {**environment, "CARGO": str(tools["cargo"])}
        if role == "rustc"
        else environment
    )
    plan = proof_plan.ProofPlan.load()

    def capture():
        telemetry = []
        value = command_identity._tool_identity(
            plan,
            role,
            envelope,
            command,
            cwd=tmp_path,
            env=environment,
            reuse_root=tmp_path / "reuse",
            reuse_telemetry=telemetry,
        )
        return value, telemetry

    # The declared Cargo command's outer token must resolve to the proxy too.
    monkeypatch.setattr(
        command_identity,
        "_resolve_outer_executable",
        lambda value, **kwargs: (
            tools["cargo"] if value == str(tools["cargo"]) else proxy
        ),
    )
    first, miss = capture()
    phases = len(phase_calls)
    second, hit = capture()
    assert hit[0]["state"] == "hit" and second == first
    assert len(resolutions) == 2 and len(versions) == 1 and len(phase_calls) == phases
    replacement = tmp_path / (role + "-replacement" + suffix)
    replacement.write_bytes(b"independent newly selected Rust component")
    replacement.chmod(0o755)
    selected[0] = replacement
    third, changed = capture()
    assert (
        changed[0]["state"] == "miss"
        and changed[0]["key_sha256"] != miss[0]["key_sha256"]
    )
    assert first["launcher_sha256"] == third["launcher_sha256"]
    assert third["content_path"] == str(replacement.resolve(strict=True))
    assert first["executable_sha256"] != third["executable_sha256"]
    assert len(resolutions) == 3 and len(versions) == 2
    # Explicit physical tools do not invoke rustup, including a warm capture.
    for _ in range(2):
        assert (
            rust_toolchain.resolve_rustup_proxy(
                replacement, role=role, root=tmp_path, env=environment
            )
            == replacement
        )
    assert len(resolutions) == 3


@pytest.mark.parametrize("primary", [False, True])
def test_cargo_rustc_environment_selector_precedence(tmp_path, monkeypatch, primary):
    suffix = ".exe" if os.name == "nt" else ""
    lower, higher = (
        tmp_path / ("cargo-rustc" + suffix),
        tmp_path / ("primary-rustc" + suffix),
    )
    for path in (lower, higher):
        path.write_bytes(b"selected physical compiler")
        path.chmod(0o755)
    env = {"CARGO_BUILD_RUSTC": str(lower)}
    if primary:
        env["RUSTC"] = str(higher)
    monkeypatch.setattr(
        command_identity,
        "_which_in_command_environment",
        lambda *args, **kwargs: pytest.fail("explicit Rust selection used PATH"),
    )
    monkeypatch.setattr(
        command_identity,
        "_capture_tool_identity",
        lambda *args, **kwargs: {"content_path": str(kwargs["selected_content_path"])},
    )
    command = ["cargo", "build"]
    result = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "rustc",
        command_admission.envelope_for_command(command),
        command,
        cwd=tmp_path,
        env=env,
    )
    assert result["content_path"] == str(
        (higher if primary else lower).resolve(strict=True)
    )


def test_generator_rustfmt_capture_uses_path_independently_of_cargo_hook(
    tmp_path, monkeypatch
):
    selected = tmp_path / "path" / ("rustfmt.exe" if os.name == "nt" else "rustfmt")
    selected.parent.mkdir()
    decoy = tmp_path / "cargo-formatter"
    decoy.write_bytes(b"Cargo-only formatter")
    decoy.chmod(0o755)
    selected.write_bytes(b"selected formatter")
    selected.chmod(0o755)
    monkeypatch.setattr(
        command_identity,
        "_capture_tool_identity",
        lambda *args, **kwargs: {"path": str(kwargs["selected_content_path"])},
    )
    command = [sys.executable, "-c", "pass"]
    value = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "rustfmt",
        command_admission.envelope_for_command(command),
        command,
        cwd=tmp_path,
        env={"RUSTFMT": str(decoy), "PATH": str(selected.parent)},
    )
    assert value == {"path": str(selected.resolve(strict=True))}


@pytest.mark.parametrize("role", ["cargo", "rustc", "rustfmt", "git", "node"])
@pytest.mark.parametrize("delegated", [False, True])
def test_registered_tool_identity_uses_bound_payload_before_dependency_selectors(
    tmp_path, monkeypatch, role, delegated
):
    selected, decoy = tmp_path / "selected", tmp_path / "decoy"
    for directory in (selected, decoy):
        directory.mkdir()
        image = directory / (role + (".exe" if os.name == "nt" else ""))
        image.write_bytes(directory.name.encode())
        image.chmod(0o755)
    name = role + (".exe" if os.name == "nt" else "")
    executable = selected / name
    payload = [str(executable), "--version"]
    command = (
        [sys.executable, "tools/guarded_exec.py", "--", *payload]
        if delegated
        else payload
    )
    env = {"PATH": str(decoy), role.upper(): str(decoy / name)}
    envelope = command_admission.envelope_for_command(command)
    exact = command_identity._exact_command(envelope, cwd=proof_plan.ROOT, env=env)
    command_identity._bind_delegated_command(
        envelope, exact, cwd=proof_plan.ROOT, env=env
    )
    monkeypatch.setattr(
        command_identity,
        "_capture_tool_identity",
        lambda *args, **kwargs: {"path": str(kwargs["path"])},
    )
    assert command_identity._tool_identity(
        proof_plan.ProofPlan.load(), role, envelope, exact, cwd=proof_plan.ROOT, env=env
    ) == {"path": process_image_capture._image_path_key(executable)}


@pytest.mark.parametrize("owner", ["python", "cargo", "delegated-cargo"])
def test_python_rustc_metadata_dependency_has_independent_reuse_identity(
    tmp_path, monkeypatch, owner
):
    from molt import process_guard
    from tools.proof_queue_pkg import toolchain_capture

    suffix = ".exe" if os.name == "nt" else ""
    path_bin = tmp_path / "path"
    path_bin.mkdir()
    proxy, rustup, cargo = (
        path_bin / (role + suffix) for role in ("rustc", "rustup", "cargo")
    )
    primary, first, second = (
        tmp_path / (name + suffix)
        for name in ("configured-compiler", "path-component-one", "path-component-two")
    )
    for path in (proxy, rustup, cargo, primary, first, second):
        path.write_bytes(b"proxy" if path in (proxy, rustup) else path.name.encode())
        path.chmod(0o755)
    selected = [first]

    def resolve(command, **kwargs):
        assert command == [str(rustup), "which", "rustc"]
        return subprocess.CompletedProcess(command, 0, str(selected[0]) + "\n", "")

    versions = []

    def version(command, **kwargs):
        assert command[0] == str(primary.resolve(strict=True))
        versions.append(command)
        return subprocess.CompletedProcess(
            command, 0, "rustc 1.99.0\nhost: x86_64-unknown-linux-gnu\n", ""
        )

    monkeypatch.setattr(process_guard, "run_completed_command", resolve)
    monkeypatch.setattr(command_identity, "_run_captured", version)
    from types import SimpleNamespace
    from tests.tools.test_toolchain_capture import _rust_metadata_probe

    linker = tmp_path / ("retained-linker" + suffix)
    linker.write_bytes(b"independent linker image")
    linker.chmod(0o755)
    phases = []

    def phase(command, **kwargs):
        phases.append(list(command))
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        return subprocess.CompletedProcess(
            command, 0, json.dumps(str(linker)) + "\n", ""
        )

    # Substitute compiler output only: the real v4 capture, structural receiver,
    # image validation and warm-reuse accounting remain in the tested path.
    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=phase))
    command = (
        [sys.executable, "-c", "pass"] if owner == "python" else ["cargo", "build"]
    )
    if owner == "delegated-cargo":
        command = [sys.executable, "tools/guarded_exec.py", "--", *command]
    envelope = command_admission.envelope_for_command(command)
    environment = {"RUSTC": str(primary), "PATH": str(path_bin)}

    def capture():
        telemetry = []
        identity = command_identity._tool_identity(
            proof_plan.ProofPlan.load(),
            "rustc",
            envelope,
            command,
            cwd=tmp_path,
            env=environment,
            reuse_root=tmp_path / "reuse",
            reuse_telemetry=telemetry,
        )
        assert identity["path"] == process_image_capture._image_path_key(primary)
        assert identity["content_path"] == str(primary.resolve(strict=True))
        assert identity["version"] == "rustc 1.99.0\nhost: x86_64-unknown-linux-gnu"
        return identity, telemetry[0]

    initial, initial_event = capture()
    cold_phases = len(phases)
    warm, warm_event = capture()
    assert len(phases) == cold_phases
    assert warm == initial and warm_event["state"] == "hit"
    assert len(versions) == 1
    dependency_paths = {
        row["path"]
        for row in initial["process_images"]
        if row["role"] == "rustc-path-metadata"
    }
    assert dependency_paths == (
        {
            process_image_capture._image_path_key(proxy),
            process_image_capture._image_path_key(first),
        }
        if owner == "python"
        else set()
    )
    selected[0] = second
    changed, event = capture()
    assert event["state"] == ("miss" if owner == "python" else "hit")
    assert (event["key_sha256"] != initial_event["key_sha256"]) is (owner == "python")
    assert changed["executable_sha256"] == initial["executable_sha256"]
    if owner == "python":
        assert {
            row["path"]
            for row in changed["process_images"]
            if row["role"] == "rustc-path-metadata"
        } == {
            process_image_capture._image_path_key(proxy),
            process_image_capture._image_path_key(second),
        }


def test_rust_reuse_binds_archive_claim_to_actual_producer_command(
    tmp_path, monkeypatch
):
    import copy
    from molt.exact_json import canonical_json_sha256
    from tests.tools.test_toolchain_capture import _native_build_capture_fixture

    identity, _tools, env, command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, required=False
    )
    policy = next(
        row
        for row in proof_plan.ProofPlan.load().toolchain_policies
        if row.name == "rustc"
    )
    identity["configuration_files"] = command_identity._tool_configuration_identities(
        "rustc", cwd=tmp_path, env=env, command_argv=command
    )

    def seal(value):
        value.pop("identity_sha256", None)
        value["identity_sha256"] = canonical_json_sha256(value)

    seal(identity)
    assert command_identity._reused_identity_is_current(
        policy, identity, cwd=tmp_path, env=env, command_argv=command
    )
    forged = copy.deepcopy(identity)
    selection = forged["link_selection"]
    archive = ["cargo", "rustc", "--lib", "--crate-type", "staticlib"]
    selection["admitted_command"] = archive
    selection["producer_command"] = archive
    selection["command_semantics_sha256"] = canonical_json_sha256(archive)
    for unit in selection["units"]:
        unit["command_semantics_sha256"] = canonical_json_sha256(archive)
        unit["flag_context"]["command"] = archive
    target = selection["units"][0]
    target.update(
        artifact_selection={
            "cargo_crate_types": ["staticlib"],
            "manifest_crate_types": None,
            "rustc_crate_types": [],
            "test_harness": False,
            "link_required": False,
        },
        selected_process_count=0,
        process_resolution=[],
        process_image_refs=[],
        link_argv_sha256=canonical_json_sha256([]),
    )
    # The host still owns the shared image; this is a coherent archive claim
    # for a different command, not an accidentally incomplete image fixture.
    command_identity.toolchain_capture.validate_rust_link_selection(
        forged, command_argv=archive
    )
    seal(forged)
    assert not command_identity._reused_identity_is_current(
        policy, forged, cwd=tmp_path, env=env, command_argv=command
    )
    with pytest.raises(ValueError, match="actual admission"):
        command_identity.toolchain_capture.revalidate_rust_link_process_images(
            forged, target=None, command_argv=command
        )


@pytest.mark.parametrize("transport", ["venv", "uv-project"])
def test_rust_receiver_binds_modeled_wrapper_to_its_exact_payload(
    tmp_path, monkeypatch, transport
):
    from molt.exact_json import canonical_json_sha256
    from tests.tools.test_toolchain_capture import _native_build_capture_fixture

    identity, tools, _env, _command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, required=False
    )
    wrapper = "venv_exec.py" if transport == "venv" else "uv_project_env.py"
    submitted = [
        "python",
        "tools/" + wrapper,
        *(["--python", "3.12"] if transport == "uv-project" else []),
        "cargo",
        "build",
    ]
    envelope = command_admission.envelope_for_command(submitted)
    assert envelope["argv"] == ["cargo", "build"]
    producer = [str(tools["cargo"]), "build"]
    selection = identity["link_selection"]
    selection["admitted_command"] = submitted
    selection["producer_command"] = producer
    selection["command_semantics_sha256"] = canonical_json_sha256(producer)
    for unit in selection["units"]:
        unit["command_semantics_sha256"] = canonical_json_sha256(producer)
        unit["flag_context"]["command"] = producer
    command_identity.toolchain_capture.validate_rust_link_selection(
        identity, command_argv=producer
    )
    with pytest.raises(ValueError, match="actual admission"):
        command_identity.toolchain_capture.validate_rust_link_selection(
            identity, command_argv=[str(tools["cargo"]), "check"]
        )


@pytest.mark.skipif(os.name == "nt", reason="POSIX LLVM multicall symlink regression")
@pytest.mark.parametrize("direct", [False, True])
def test_compiler_strip_capture_binds_lexical_role_and_physical_image(
    tmp_path, monkeypatch, direct
):
    from molt import llvm_wasi_tools
    from molt.exact_json import canonical_json_sha256

    physical = tmp_path / "llvm-objcopy"
    physical.write_bytes(b"real selected LLVM multicall bytes")
    physical.chmod(0o755)
    lexical = tmp_path / "llvm-strip"
    lexical.symlink_to(physical.name)
    monkeypatch.setattr(
        llvm_wasi_tools, "llvm_tool_candidates", lambda *a, **k: (lexical,)
    )
    calls = []

    def probe(command, **kwargs):
        calls.append(list(command))
        return subprocess.CompletedProcess(command, 0, "LLVM version 22.1.8", "")

    monkeypatch.setattr(command_identity, "_run_captured", probe)
    plan = proof_plan.ProofPlan.load()
    command = [str(lexical), "--version"] if direct else ["compiler-driver"]
    envelope = {"argv": command, "toolchains": ["llvm-strip"]}
    if direct:
        monkeypatch.setattr(
            llvm_wasi_tools,
            "llvm_tool_candidates",
            lambda *a, **k: pytest.fail("direct tool consulted compiler selection"),
        )
    telemetry = []

    def capture():
        return command_identity._tool_identity(
            plan,
            "llvm-strip",
            envelope,
            command,
            cwd=tmp_path,
            env={},
            reuse_root=tmp_path / "reuse",
            reuse_telemetry=telemetry,
        )

    identity = capture()
    command_identity._validate_toolchain_identity(plan, "llvm-strip", identity)
    assert calls == [[str(lexical), "--version"]]
    assert identity["path"] == str(lexical)
    assert identity["content_path"] == str(physical)
    assert {row["path"] for row in identity["process_images"]} == {
        str(lexical),
        str(physical),
    }
    assert capture() == identity and len(calls) == 1
    assert telemetry[-1]["state"] == "hit"
    omitted = dict(
        identity,
        process_images=[
            row for row in identity["process_images"] if row["path"] != str(physical)
        ],
    )
    with pytest.raises(ValueError, match="content image is outside"):
        command_identity._validate_toolchain_identity(plan, "llvm-strip", omitted)
    record_path = Path(telemetry[-1]["record"])
    record = json.loads(record_path.read_text(encoding="utf-8"))
    omitted.pop("identity_sha256")
    omitted["identity_sha256"] = canonical_json_sha256(omitted)
    record["identity"] = omitted
    record_path.write_text(json.dumps(record), encoding="utf-8")
    assert capture() == identity and len(calls) == 2
    assert telemetry[-1]["state"] == "miss"


def selected_runtime_fixture(tmp_path, monkeypatch):
    """Finite installation providers drive the real runtime/file/native authority.

    Selection and root locations are fixture boundaries. No identity, inventory,
    digest, file fence or native-dependency result is mocked.
    """
    import copy
    import types
    import threading
    import unicodedata
    import venv

    from molt import python_runtime_identity as runtime
    from molt import python_runtime_admission
    from molt.python_file_node_custody import PythonFileCaptureContext

    minor = f"{sys.version_info.major}.{sys.version_info.minor}"
    policy = next(
        p
        for p in proof_plan.ProofPlan.load().toolchain_policies
        if p.python_identity_role == "runtime" and p.data["python_version"] == minor
    )
    uv = tmp_path / ("uv.exe" if os.name == "nt" else "uv")
    uv.write_bytes(b"unchanged fixture selector image")
    installations = {}
    for name in ("first", "second"):
        root = tmp_path / name
        venv.EnvBuilder(with_pip=False, symlinks=os.name != "nt").create(root)
        executable = root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        library = root / "fixture-stdlib"
        library.mkdir()
        (library / "input.py").write_text("answer = 42\n", encoding="utf-8")
        # Standalone CPython may build Unicode into its executable. The finite
        # provider cell uses a real native image in either host configuration;
        # it declares that file through the actual dynamic-provider boundary.
        native_source = Path(
            getattr(unicodedata, "__file__", sys._base_executable)
        ).resolve()
        unicode_file = library / ("unicodedata-provider" + native_source.suffix)
        shutil.copyfile(native_source, unicode_file)
        installations[str(executable)] = (library, unicode_file)
    selected = [Path(next(iter(installations)))]
    real_sys = runtime.sys
    real_library = runtime._runtime_library()
    owners = []
    runtime_capture_lock = threading.RLock()

    class FiniteAdmission:
        def __init__(self, *, with_custody=False):
            assert with_custody is True
            self.context = PythonFileCaptureContext()
            self.captured = None
            self.selected = None
            owners.append(self)

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            self.context.close()

        def capture(self, env):
            executable = env["MOLT_BUILD_PYTHON"]
            library, unicode_file = installations[executable]
            proxy = types.SimpleNamespace(**vars(real_sys))
            proxy.executable = executable
            proxy._base_executable = str(Path(executable).resolve())
            proxy.base_prefix = str(Path(executable).parent.parent)
            proxy.prefix = proxy.base_prefix
            proxy.path = [str(library)]
            # A fresh process would import this selected Unicode provider. Its
            # missing file must fail even though interpreter/banner are intact.
            with runtime_capture_lock, monkeypatch.context() as scope:
                scope.setattr(runtime, "sys", proxy)
                scope.setattr(
                    runtime,
                    "unicodedata",
                    types.SimpleNamespace(__file__=str(unicode_file)),
                )
                scope.setattr(runtime, "_runtime_library", lambda: real_library)
                roles = (
                    "stdlib",
                    "platstdlib",
                    "base-dlls" if os.name == "nt" else "base-lib-dynload",
                )
                scope.setattr(
                    runtime,
                    "_base_runtime_paths",
                    lambda: dict.fromkeys(roles, library),
                )
                scope.setattr(
                    runtime,
                    "_runtime_import_candidates",
                    lambda _base, _platform: ([("import-directory-0", library, 0)], []),
                )
                if self.captured is None:
                    capture = runtime.capture_current_python_runtime(
                        capture_context=self.context, with_custody=True
                    )
                    self.selected = runtime.current_python_runtime_selection()
                    self.captured = {
                        "runtime": capture["identity"],
                        "capture": capture,
                        "startup_selection": self.selected,
                    }
                else:
                    assert not runtime.startup_selection_difference(
                        self.selected,
                        runtime.current_python_runtime_selection(),
                        admit_new_images=False,
                    )
                    self.context.verify()
                return copy.deepcopy(self.captured)

    monkeypatch.setattr(
        python_runtime_admission, "BuildPythonAdmission", FiniteAdmission
    )
    monkeypatch.setattr(
        command_identity, "_which_in_command_environment", lambda *a, **kw: uv
    )
    original_run = command_identity._run_captured

    def run(command, **kwargs):
        argv = list(command)
        if argv[0] == str(uv):
            return subprocess.CompletedProcess(argv, 0, str(selected[0]) + "\n", "")
        if argv[-1:] == ["--runtime-selection"]:
            with FiniteAdmission(with_custody=True) as owner:
                data = owner.capture({"MOLT_BUILD_PYTHON": argv[0]})[
                    "startup_selection"
                ]
            return subprocess.CompletedProcess(argv, 0, json.dumps(data), "")
        return original_run(command, **kwargs)

    monkeypatch.setattr(command_identity, "_run_captured", run)
    plan = replace(proof_plan.ProofPlan.load(), toolchain_policies=(policy,))
    return types.SimpleNamespace(
        plan=plan,
        policy=policy,
        uv=uv,
        selected=selected,
        installations=installations,
        admission_class=FiniteAdmission,
        owners=owners,
        minor=minor,
    )


@pytest.mark.parametrize("spelling", ["relative", "absolute-with-parent"])
def test_selected_runtime_reuse_preserves_canonical_entrypoint_from_probe_cwd(
    tmp_path, monkeypatch, spelling
):
    from tests.executable_test_support import custody_spelling

    fixture = selected_runtime_fixture(tmp_path, monkeypatch)
    selected = fixture.selected[0]
    if os.name != "nt":
        assert selected.is_symlink(), "the POSIX cell must retain a real venv alias"
    probe_directory = tmp_path / "probe-cwd"
    probe_directory.mkdir()
    command_directory = tmp_path / "command-cwd"
    command_directory.mkdir()
    relative = os.path.relpath(selected, probe_directory)
    emitted = relative if spelling == "relative" else str(probe_directory / relative)
    expected = custody_spelling(selected)
    fixture.installations[expected] = fixture.installations[str(selected)]
    policy = replace(
        fixture.policy,
        root=tmp_path,
        data={**fixture.policy.data, "probe_cwd": "probe-cwd"},
    )
    plan = replace(fixture.plan, toolchain_policies=(policy,))
    original_run = command_identity._run_captured
    resolutions = []
    versions = []

    def run(command, **kwargs):
        argv = list(command)
        assert kwargs["cwd"] == probe_directory
        if argv[0] == str(fixture.uv):
            resolutions.append(argv)
            return subprocess.CompletedProcess(argv, 0, emitted + "\n", "")
        if argv[-1:] == ["--version"]:
            versions.append(argv)
        return original_run(command, **kwargs)

    monkeypatch.setattr(command_identity, "_run_captured", run)
    telemetry = []

    def capture():
        result = command_identity._tool_identity(
            plan,
            policy.name,
            {"python": None},
            [sys.executable, "-c", "pass"],
            cwd=command_directory,
            env=os.environ,
            reuse_root=tmp_path / "reuse",
            reuse_telemetry=telemetry,
        )
        command_identity._validate_toolchain_identity(plan, policy.name, result)
        return result

    first = capture()
    assert capture() == first
    assert first["content_path"] == expected
    assert first["python_runtime"]["launch_argv"] == [expected, "-B", "-I", "-S"]
    assert (
        first["executable_sha256"] == hashlib.sha256(selected.read_bytes()).hexdigest()
    )
    assert versions == [[expected, "--version"]]
    assert len(resolutions) == 2, "warm reuse must still select the current alias"
    assert [row["state"] for row in telemetry] == ["miss", "hit"]


@pytest.mark.parametrize(
    "change", ["unicode-bytes", "runtime-bytes", "missing-unicode"]
)
def test_selected_runtime_reuse_binds_unicode_and_runtime_inputs(
    tmp_path, monkeypatch, change
):
    runtime_root = tmp_path / "runtime-é"
    runtime_root.mkdir()
    fixture = selected_runtime_fixture(runtime_root, monkeypatch)
    telemetry = []

    def capture():
        result = command_identity._tool_identity(
            fixture.plan,
            fixture.policy.name,
            {"python": None},
            [sys.executable, "-c", "pass"],
            cwd=tmp_path,
            env=os.environ,
            reuse_root=tmp_path / "reuse",
            reuse_telemetry=telemetry,
        )
        command_identity._validate_toolchain_identity(
            fixture.plan, fixture.policy.name, result
        )
        return result

    before = capture()
    assert capture() == before
    assert [row["state"] for row in telemetry] == ["miss", "hit"]
    uv_bytes = fixture.uv.read_bytes()
    interpreter_bytes = fixture.selected[0].read_bytes()
    library, unicode_file = fixture.installations[str(fixture.selected[0])]
    retained = fixture.admission_class(with_custody=True)
    environment = {"MOLT_BUILD_PYTHON": str(fixture.selected[0])}
    admitted = retained.capture(environment)
    profile = retained.context.inventory_profile()
    assert retained.capture(environment) == admitted
    assert retained.context.inventory_profile() == profile
    changed = unicode_file if "unicode" in change else library / "input.py"
    if change == "missing-unicode":
        changed.unlink()
        with pytest.raises((OSError, ValueError)):
            retained.capture(environment)
        retained.__exit__(None, None, None)
        with pytest.raises((OSError, ValueError)):
            capture()
    else:
        # Keep the native header/declarations valid, change an input byte whose
        # independent content oracle is the complete file SHA, not its banner.
        with changed.open("ab") as stream:
            stream.write(b"\nchanged runtime input\n")
        with pytest.raises((OSError, ValueError)):
            retained.capture(environment)
        retained.__exit__(None, None, None)
        after = capture()
        assert after["identity_sha256"] != before["identity_sha256"]
        assert after["version"] == before["version"]
        assert telemetry[-1]["state"] == "miss"
    assert fixture.uv.read_bytes() == uv_bytes
    assert fixture.selected[0].read_bytes() == interpreter_bytes


@pytest.mark.parametrize("policy", ["semantic", "zlib-wasi", "wasi-lifecycle"])
@pytest.mark.parametrize("mutation", [None, "missing-operation", "selected-image"])
def test_runtime_gate_reuse_preserves_actual_operation_inventory_without_reprobe(
    tmp_path, monkeypatch, policy, mutation
):
    from tests.tools.test_toolchain_capture import (
        _native_build_capture_fixture,
        _runtime_gate_registered_command,
    )
    from tools.proof_queue_pkg import toolchain_capture
    from molt.exact_json import canonical_json_sha256

    submitted = _runtime_gate_registered_command(policy)
    identity, tools, env, command, calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, registered_command=submitted, armed=False
    )
    requirements = command_admission.envelope_for_command(submitted)[
        "cargo_native_units"
    ]
    monkeypatch.setattr(
        command_identity, "_tool_configuration_identities", lambda *args, **kwargs: []
    )
    before = list(calls)
    if mutation == "missing-operation":
        identity["link_selection"]["operations"].pop()
        identity.pop("identity_sha256")
        identity["identity_sha256"] = canonical_json_sha256(identity)
    elif mutation == "selected-image":
        tools["linker"].write_bytes(b"different host linker")
    monkeypatch.setattr(
        toolchain_capture,
        "_run_rust_link_probe",
        lambda *args, **kwargs: pytest.fail(
            "warm runtime-gate reuse executed a compiler probe"
        ),
    )
    assert command_identity._reused_identity_is_current(
        proof_plan.ToolchainPolicy("rustc", {}),
        identity,
        cwd=tmp_path,
        env=env,
        command_argv=command,
        native_units=requirements,
    ) is (mutation is None)
    assert calls == before
