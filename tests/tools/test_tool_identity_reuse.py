"""Content-addressed toolchain identity reuse re-proves every byte it reuses."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

from tools import proof_plan
from tools.proof_queue_pkg import (
    command_admission,
    command_identity,
    process_image_capture,
)


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
    }
    probe = command_identity._probe_environment_selection(env)
    assert set(probe) == {
        "CARGO",
        "RUSTFLAGS",
        "CARGO_PROFILE_RELEASE_LTO",
        "CC_x86_64_unknown_linux_gnu",
        "PATH",
    }


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

    def resolve(root, role, *, environ):
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
def test_native_c_reuse_revalidates_actual_selection_and_images(
    tmp_path, monkeypatch, mutation
):
    from tests.tools.test_toolchain_capture import _native_c_capture_fixture
    from molt.exact_json import canonical_json_sha256
    from tools.proof_queue_pkg import toolchain_capture

    identity, tools, env, command, _calls = _native_c_capture_fixture(
        tmp_path, monkeypatch, armed=False
    )
    monkeypatch.setattr(
        command_identity, "_tool_configuration_identities", lambda *args, **kwargs: []
    )
    if mutation in {"compiler", "helper", "archiver"}:
        tools[
            {"compiler": "selected-gcc", "helper": "cc1", "archiver": "selected-ar"}[
                mutation
            ]
        ].write_bytes(b"changed")
    elif mutation == "selector":
        env["HOST_CC"] = str(tools["linker"])
    elif mutation == "helper-shadow":
        first = tmp_path / "earlier"
        first.mkdir()
        shadow = first / tools["as"].name
        shadow.write_bytes(b"another assembler")
        shadow.chmod(0o755)
        env["PATH"] = str(first) + os.pathsep + env["PATH"]
    elif mutation == "missing-unit":
        identity["link_selection"]["native_c"] = []
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
        native_c_units=["target"],
    ) is (mutation == "none")


@pytest.mark.parametrize("role", ["cargo", "rustc"])
def test_rust_reuse_resolves_current_component_before_cache_without_phase_reprobe(
    tmp_path, monkeypatch, role
):
    from molt import process_guard, rust_toolchain
    from tests.tools.test_toolchain_capture import _native_c_capture_fixture

    _identity, tools, environment, command, phase_calls = _native_c_capture_fixture(
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
    from tests.tools.test_toolchain_capture import _native_c_capture_fixture

    identity, _tools, env, command, _calls = _native_c_capture_fixture(
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
    target = selection["units"][0]
    target.update(
        artifact_selection={
            "cargo_crate_types": ["staticlib"],
            "manifest_crate_types": None,
            "rustc_crate_types": [],
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
    from tests.tools.test_toolchain_capture import _native_c_capture_fixture

    identity, tools, _env, _command, _calls = _native_c_capture_fixture(
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
    command_identity.toolchain_capture.validate_rust_link_selection(
        identity, command_argv=producer
    )
    with pytest.raises(ValueError, match="actual admission"):
        command_identity.toolchain_capture.validate_rust_link_selection(
            identity, command_argv=[str(tools["cargo"]), "check"]
        )
