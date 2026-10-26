from __future__ import annotations

from molt.cargo_execution_policy import cargo_command_options

from molt.process_environment import (
    NameRule,
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)

import hashlib
import json
import os
import subprocess
import sys
import tomllib
from collections.abc import Mapping
from dataclasses import replace
from pathlib import Path
from types import MappingProxyType

import pytest

from molt.cli import cargo_command as cargo_argv
from molt.cli import cargo_resource_custody as resource_custody
from molt.cli import runtime_cargo_plan as plans
from molt.cli import runtime_tool_selection
from molt.cli import rustc_probes
from molt.cli import cargo_target_cfg
from molt.cli import runtime_fingerprints as fingerprints
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.cli.runtime_build_identity import (
    _capture_plan_toolchain,
    _runtime_build_environment_identity,
    _verify_plan_toolchain_content,
)
from molt.cli.runtime_cargo_plan import _rust_resource_roots
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.cli.runtime_identity_schema import (
    RuntimeBuildIdentity,
    runtime_build_fingerprint,
)
from molt.exact_json import canonical_json_sha256
from molt.flag_strings import FlagStringError, FlagStringRule
from molt.llvm_toolchain import project_wasm_toolchain_environment
from molt import rust_toolchain
from molt import cargo_execution_policy
from molt.cargo_execution_policy import validate_cargo_native_dependency_environment
from tests.executable_test_support import write_mock_executable
from tests.rustc_test_support import (
    rustc_target_metadata_output,
    rustc_target_metadata_stdout as _metadata_stdout,
)
from tests.runtime_build_identity_helper import (
    RuntimeFixtureRoot,
    native_runtime_staticlib_identity,
    provisioned_wasi_sdk_fixture,
    runtime_build_identity,
)


def _reseal(value: dict) -> None:
    family = value["payload"]["family"]
    family["compile_digest"] = canonical_json_sha256(family["compile"])
    value["compile_digest"] = family["compile_digest"]
    value["family_digest"] = canonical_json_sha256(family)
    value["digest"] = canonical_json_sha256(value["payload"])


@pytest.mark.parametrize("staged", [False, True])
def test_native_dependency_source_default_uses_actual_cargo_config_projection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, staged: bool
) -> None:
    source = Path(__file__).resolve().parents[2]
    raw = (source / ".cargo/config.toml").read_bytes()
    assert tomllib.loads(raw.decode("utf-8"))["env"]["AWS_LC_SYS_USE_SYSTEM"] == "0"
    root = tmp_path / "source"
    (root / ".cargo").mkdir(parents=True)
    (root / ".cargo/config.toml").write_bytes(raw)
    (root / "Cargo.toml").write_text(
        '[package]\nname="fixture"\nversion="0.1.0"\n', encoding="utf-8"
    )
    work = tmp_path / "work"
    work.mkdir()
    monkeypatch.chdir(work if staged else root)
    inherited = {"CARGO_HOME": str(tmp_path / "cargo-home")}

    captured = plans.capture_cargo_configuration_inputs(
        root, env=inherited, cargo_command=["cargo", "build"]
    )

    assert captured[1]["AWS_LC_SYS_USE_SYSTEM"] == "0"
    assert inherited == {"CARGO_HOME": str(tmp_path / "cargo-home")}
    assert any(item.path == root / ".cargo/config.toml" for item in captured[3])
    assert not (work / ".cargo").exists()


@pytest.mark.parametrize(
    "controls",
    [
        {"AWS_LC_SYS_USE_SYSTEM": "1"},
        {"AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu": "yes"},
        {"AWS_LC_SYS_SYSTEM_DIR": ""},
        {"AWS_LC_SYS_SYSTEM_DIR_aarch64_unknown_linux_gnu": "/unowned"},
        {"AWS_LC_SYS_USE_SYSTEM": " 0 "},
        {"AWS_LC_SYS_USE_SYSTEM": ""},
        {"AWS_LC_SYS_INCLUDES": "/unowned"},
        {"AWS_LC_SYS_CMAKE_BUILDER": "1"},
        {"AWS_LC_SYS_STATIC": "false"},
        {"AWS_LC_SYS_NO_ASM": "1"},
        {"AWS_LC_SYS_NO_PREGENERATED_SRC": "1"},
        {"AWS_LC_SYS_EXTERNAL_BINDGEN": "1"},
        {"AWS_LC_SYS_NO_PREFIX": "1"},
        {"AWS_LC_SYS_PREGENERATING_BINDINGS": "1"},
        {"AWS_LC_SYS_SANITIZER": ""},
        {"AWS_LC_SYS_EFFECTIVE_TARGET": "different-unknown-linux-gnu"},
    ],
)
def test_native_dependency_unowned_capabilities_fail_before_tool_probes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, controls: dict[str, str]
) -> None:
    def probe_forbidden(*_args, **_kwargs):
        pytest.fail("unsupported native dependency selection must precede tool probes")

    monkeypatch.setattr(runtime_tool_selection, "runtime_tool_path", probe_forbidden)
    env = {"CARGO_HOME": str(tmp_path / "cargo-home"), **controls}
    before = dict(env)
    with pytest.raises(ValueError, match="AWS-LC") as raised:
        plans.resolve_runtime_cargo_plan(
            tmp_path,
            env=env,
            cargo_command=["cargo", "build", "--target", "aarch64-unknown-linux-gnu"],
            requested_target="aarch64-unknown-linux-gnu",
        )
    assert all(name in str(raised.value) for name in controls)
    assert env == before
    assert not list(tmp_path.iterdir())


def test_native_dependency_diagnostics_report_all_provider_and_private_controls() -> (
    None
):
    controls = {
        "AWS_LC_SYS_USE_SYSTEM": "0",
        "AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu": "1",
        "AWS_LC_SYS_SYSTEM_DIR": "/unowned",
        "AWS_LC_SYS_INCLUDES": "/headers",
    }
    with pytest.raises(ValueError, match="AWS-LC") as raised:
        validate_cargo_native_dependency_environment(
            controls,
            target="aarch64-unknown-linux-gnu",
            compiler_host="aarch64-unknown-linux-gnu",
        )
    assert all(
        name in str(raised.value)
        for name in controls
        if name != "AWS_LC_SYS_USE_SYSTEM"
    )


def test_native_dependency_target_precedence_preserves_shadowed_and_unused_values() -> (
    None
):
    controls = {
        "AWS_LC_SYS_USE_SYSTEM": "1",
        "AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu": "0",
        "AWS_LC_SYS_NO_ASM": "1",
        "AWS_LC_SYS_NO_ASM_aarch64_unknown_linux_gnu": "0",
        "AWS_LC_SYS_SYSTEM_DIR_x86_64_pc_windows_msvc": "/not-selected",
        "AWS_LC_SYS_INCLUDES_x86_64_unknown_linux_gnu": "/not-selected",
    }
    before = dict(controls)
    validate_cargo_native_dependency_environment(controls)
    validate_cargo_native_dependency_environment(
        controls, target="aarch64-unknown-linux-gnu"
    )
    assert controls == before


@pytest.mark.parametrize("rule", list(NameRule))
def test_native_dependency_target_suffix_matches_upstream_exact_spelling(rule) -> None:
    controls = ProcessEnvironment(
        {
            "AWS_LC_SYS_USE_SYSTEM": "0",
            "AWS_LC_SYS_USE_SYSTEM_AARCH64_UNKNOWN_LINUX_GNU": "1",
            "AWS_LC_SYS_USE_SYSTEM_aarch64-unknown-linux-gnu": "1",
        },
        rule=rule,
    )
    if rule is NameRule.WINDOWS:
        # Windows folds case before upstream's exact environment lookup.
        with pytest.raises(ValueError, match="AWS-LC"):
            validate_cargo_native_dependency_environment(
                controls, target="aarch64-unknown-linux-gnu"
            )
    else:
        validate_cargo_native_dependency_environment(
            controls, target="aarch64-unknown-linux-gnu"
        )


@pytest.mark.parametrize("false_value", ["0", "false", "NO", "Off"])
def test_native_dependency_source_controls_keep_the_cc_assembly_capability(
    false_value: str,
) -> None:
    controls = {
        "AWS_LC_SYS_USE_SYSTEM": false_value,
        "AWS_LC_SYS_CMAKE_BUILDER": "0",
        "AWS_LC_SYS_NO_ASM": "0",
        "AWS_LC_SYS_STATIC": "true",
        "AWS_LC_SYS_SMALL": "1",
        "AWS_LC_SYS_C_STD": "11",
        "AWS_LC_SYS_NO_JITTER_ENTROPY": "0",
        "AWS_LC_SYS_PROJECT_NOTE": "data, not an upstream control",
        "CC_aarch64_unknown_linux_gnu": "/captured/compiler",
        "CFLAGS": "-O3",
    }
    before = dict(controls)
    validate_cargo_native_dependency_environment(controls)
    assert controls == before


def test_native_dependency_explicit_cc_selection_owns_no_asm_and_sanitizer_precedence():
    controls = {
        "AWS_LC_SYS_USE_SYSTEM": "0",
        "AWS_LC_SYS_NO_ASM": "1",
        "AWS_LC_SYS_SANITIZER": "address",
        "AWS_LC_SYS_CMAKE_BUILDER_x86_64_unknown_linux_gnu": "0",
    }
    validate_cargo_native_dependency_environment(controls)
    validate_cargo_native_dependency_environment(
        controls, target="x86_64-unknown-linux-gnu"
    )
    with pytest.raises(ValueError, match="AWS_LC_SYS_NO_ASM") as raised:
        validate_cargo_native_dependency_environment(
            controls, target="aarch64-unknown-linux-gnu"
        )
    assert "AWS_LC_SYS_SANITIZER" in str(raised.value)


@pytest.mark.parametrize(
    "target,host", [(None, None), ("target", None), (None, "host")]
)
def test_native_dependency_bound_stage_requires_captured_target_and_host(target, host):
    with pytest.raises(ValueError, match="requires captured Rust target and host"):
        validate_cargo_native_dependency_environment(
            {}, target=target, compiler_host=host, native_bound=True
        )


@pytest.mark.parametrize("cross", [False, True])
@pytest.mark.parametrize("control", ["CC", "CXX", "CFLAGS"])
def test_native_dependency_bound_roles_ignore_inactive_controls_and_pinned_drivers(
    cross, control
):
    host = "x86_64-unknown-linux-gnu"
    target = "aarch64-unknown-linux-gnu" if cross else host
    active = "TARGET_" if cross else "HOST_"
    inactive = "HOST_" if cross else "TARGET_"
    controls = {
        "AWS_LC_SYS_USE_SYSTEM": "0",
        "AWS_LC_SYS_" + inactive + control: "/unused",
        "CC_" + target: "/captured/cc",
        "CXX_" + target: "/captured/cxx",
    }
    validate_cargo_native_dependency_environment(controls, target=target)
    validate_cargo_native_dependency_environment(
        controls, target=target, compiler_host=host, native_bound=True
    )
    selected = "AWS_LC_SYS_" + active + control
    controls[selected] = "/private"
    if control == "CFLAGS":
        with pytest.raises(ValueError, match=selected):
            validate_cargo_native_dependency_environment(
                controls, target=target, compiler_host=host, native_bound=True
            )
    else:
        # AWS writes only the normalized form; cc-rs still selects the pinned
        # raw-hyphen target driver, so the private value is ineffective.
        validate_cargo_native_dependency_environment(
            controls, target=target, compiler_host=host, native_bound=True
        )


@pytest.mark.parametrize("cross", [False, True])
def test_native_dependency_flag_rewrite_retains_existing_custody_and_refuses_conflicts(
    cross,
):
    host = "x86_64-unknown-linux-gnu"
    target = "aarch64-unknown-linux-gnu" if cross else host
    suffix = target.replace("-", "_")
    role = "TARGET_CFLAGS" if cross else "HOST_CFLAGS"
    controls = {"AWS_LC_SYS_USE_SYSTEM": "0", role: "-O2"}
    validate_cargo_native_dependency_environment(
        controls, target=target, compiler_host=host, native_bound=True
    )
    controls["CFLAGS_" + suffix] = "-O2"
    validate_cargo_native_dependency_environment(
        controls, target=target, compiler_host=host, native_bound=True
    )
    controls[role] = "-include /not-captured.h"
    with pytest.raises(ValueError, match=role):
        validate_cargo_native_dependency_environment(
            controls, target=target, compiler_host=host, native_bound=True
        )
    controls[role] = ""
    with pytest.raises(ValueError, match=role):
        validate_cargo_native_dependency_environment(
            controls, target=target, compiler_host=host, native_bound=True
        )
    del controls["CFLAGS_" + suffix]
    controls[role + "_" + suffix] = "-include /not-captured.h"
    with pytest.raises(ValueError, match=role + "_" + suffix):
        validate_cargo_native_dependency_environment(
            controls, target=target, compiler_host=host, native_bound=True
        )


def test_native_dependency_runtime_bound_stage_preserves_pinned_driver_and_rejects_flags(
    plan_root: Path,
) -> None:
    controls = {
        "AWS_LC_SYS_TARGET_CC": "/unused",
        "AWS_LC_SYS_CC": "/ineffective-private-cc",
    }
    plan = _plan(plan_root, env=controls)
    assert plan.environment["CC_x86_64-unknown-linux-gnu"] == str(plan.tools["cc"])
    assert all(plan.environment[name] == value for name, value in controls.items())
    with pytest.raises(ValueError, match="AWS_LC_SYS_CFLAGS"):
        _plan(plan_root, env={"AWS_LC_SYS_CFLAGS": "-include /not-captured.h"})


@pytest.mark.parametrize("field", ["sources", "toolchain", "common_config"])
def test_resealed_empty_compile_subauthorities_are_rejected(field: str) -> None:
    value = runtime_build_identity("shared").to_dict()
    value["payload"]["family"]["compile"][field] = {}
    _reseal(value)
    with pytest.raises(ValueError):
        RuntimeBuildIdentity.from_dict(value)
    with pytest.raises(ValueError):
        RuntimeBuildIdentity(
            value["digest"],
            value["compile_digest"],
            value["family_digest"],
            value["payload"],
        )


@pytest.mark.parametrize(
    "mutation",
    [
        "unknown-tool",
        "missing-archive",
        "bool-size",
        "foreign-path",
        "wrong-target",
        "unknown-config",
        "retired-python",
        "unknown-wrapper",
    ],
)
def test_resealed_toolchain_and_config_drift_is_rejected(mutation: str) -> None:
    value = runtime_build_identity("shared").to_dict()
    compile_payload = value["payload"]["family"]["compile"]
    toolchain = compile_payload["toolchain"]
    if mutation == "unknown-tool":
        toolchain["tools"]["unknown"] = toolchain["tools"]["rustc"]
    elif mutation == "missing-archive":
        toolchain["archives"].pop()
    elif mutation == "bool-size":
        toolchain["tools"]["rustc"]["size"] = True
    elif mutation == "foreign-path":
        toolchain["tools"]["rustc"]["entrypoint"] = "C:\\outside\\rustc.exe"
    elif mutation == "wrong-target":
        compile_payload["common_config"]["target_triple"] = "aarch64-apple-darwin"
    elif mutation == "unknown-config":
        compile_payload["common_config"]["legacy"] = True
    elif mutation == "retired-python":
        toolchain["tools"]["build_python"] = {}
    else:
        toolchain["wrappers"]["UNRECOGNIZED"] = toolchain["tools"]["rustc"]
    _reseal(value)
    with pytest.raises(ValueError):
        RuntimeBuildIdentity.from_dict(value)


def test_publication_action_change_reuses_only_compile() -> None:
    before = runtime_build_identity(
        "shared", "old-publication", compile_seed="same-compile"
    )
    after = runtime_build_identity(
        "shared", "new-publication", compile_seed="same-compile"
    )
    assert before.compile_digest == after.compile_digest
    assert before.family_digest != after.family_digest
    assert (
        runtime_build_fingerprint(before)["hash"]
        != runtime_build_fingerprint(after)["hash"]
    )
    for scope in ("compile", "member-output"):
        first = runtime_build_fingerprint(before, scope=scope)
        second = runtime_build_fingerprint(after, scope=scope)
        assert (first["hash"] == second["hash"]) is (scope == "compile")
        assert fingerprints._runtime_fingerprint_payload_is_valid(
            {"version": 4, **first}
        )


@pytest.mark.parametrize("scope", ["compile", "member", "member-output"])
def test_fingerprint_is_a_checked_projection_not_an_independent_claim(
    scope: str,
) -> None:
    identity = native_runtime_staticlib_identity()
    payload = {"version": 4, **runtime_build_fingerprint(identity, scope=scope)}
    assert fingerprints._runtime_fingerprint_payload_is_valid(payload)
    payload["hash"] = "f" * 64
    assert not fingerprints._runtime_fingerprint_payload_is_valid(payload)


@pytest.mark.parametrize("scope", [[], {}, 1, None, "unrecognized"])
def test_untrusted_fingerprint_scope_fails_without_type_leak(scope: object) -> None:
    payload = {
        "version": 4,
        **runtime_build_fingerprint(native_runtime_staticlib_identity()),
    }
    payload["build_identity_scope"] = scope
    assert not fingerprints._runtime_fingerprint_payload_is_valid(payload)


def test_fingerprint_reader_rejects_duplicate_keys_and_restored_mtime(
    tmp_path: Path,
) -> None:
    path = tmp_path / "identity.json"
    payload = {
        "version": 4,
        **runtime_build_fingerprint(native_runtime_staticlib_identity()),
    }
    original = json.dumps(payload)
    path.write_text(original, encoding="utf-8")
    assert fingerprints._read_runtime_fingerprint(path) == payload
    before = path.stat()
    path.write_text(original.replace('"version": 4', '"version": 3'), encoding="utf-8")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert fingerprints._read_runtime_fingerprint(path) is None
    path.write_text('{"version":4,' + original[1:], encoding="utf-8")
    assert fingerprints._read_runtime_fingerprint(path) is None


@pytest.mark.parametrize("rule", list(NameRule))
@pytest.mark.parametrize("value", [None, "", "result-only-value"])
def test_cargo_artifact_fingerprint_uses_explicit_observed_environment(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    rule: NameRule,
    value: str | None,
) -> None:
    name = "Molt_Hf302_Result_Only"
    environment = ProcessEnvironment({} if value is None else {name: value}, rule=rule)
    observation = CargoEnvironmentInputs.capture(
        environment, (name,), local_unit_count=1
    )
    artifact = tmp_path / "runtime.bin"
    artifact.write_bytes(b"runtime-result")
    receipt = tmp_path / "runtime.json"
    plan = {"hash": "a" * 64, "rustc": "rustc-fixture"}
    fingerprints._write_runtime_fingerprint(
        receipt, plan, artifact=artifact, cargo_environment=observation
    )
    stored = fingerprints._read_runtime_fingerprint(receipt)
    assert stored is not None
    assert stored["version"] == 4
    assert stored["cargo_environment"] == {
        "schema": "molt.cargo-environment-inputs.v1",
        "name_rule": rule.value,
        "local_unit_count": 1,
        "variables": [
            {
                "name": name.upper() if rule is NameRule.WINDOWS else name,
                "sha256": None
                if value is None
                else hashlib.sha256(value.encode()).hexdigest(),
            }
        ],
    }
    assert "result-only-value" not in receipt.read_text(encoding="utf-8")
    monkeypatch.setenv(name, "different-ambient-value")
    assert fingerprints._runtime_artifact_fingerprint_matches(
        artifact,
        plan,
        receipt,
        environment=environment.updated({"UNOBSERVED_HF302_NAME": "ignored"}),
        require_artifact_digest=True,
    )
    assert not fingerprints._runtime_artifact_fingerprint_matches(
        artifact,
        plan,
        receipt,
        environment=environment.updated({name: "changed-explicit-value"}),
        require_artifact_digest=True,
    )
    other_rule = NameRule.EXACT if rule is NameRule.WINDOWS else NameRule.WINDOWS
    assert not fingerprints._runtime_artifact_fingerprint_matches(
        artifact,
        plan,
        receipt,
        environment=ProcessEnvironment(dict(environment), rule=other_rule),
        require_artifact_digest=True,
    )


@pytest.mark.parametrize("mutation", ["missing", "malformed"])
def test_artifact_receipt_cannot_omit_or_forge_cargo_observation(
    tmp_path: Path, mutation: str
) -> None:
    environment = ProcessEnvironment({"HF302_SCRIPT_INPUT": "selected"})
    observation = CargoEnvironmentInputs.capture(
        environment, ("HF302_SCRIPT_INPUT",), local_unit_count=1
    )
    artifact = tmp_path / "runtime.bin"
    artifact.write_bytes(b"runtime-result")
    receipt = tmp_path / "runtime.json"
    plan = {"hash": "a" * 64}
    fingerprints._write_runtime_fingerprint(
        receipt, plan, artifact=artifact, cargo_environment=observation
    )
    assert fingerprints._runtime_artifact_fingerprint_matches(
        artifact, plan, receipt, environment=environment, require_artifact_digest=True
    )
    stored = json.loads(receipt.read_text(encoding="utf-8"))
    if mutation == "missing":
        del stored["cargo_environment"]
    else:
        stored["cargo_environment"]["local_unit_count"] = 0
    receipt.write_text(json.dumps(stored), encoding="utf-8")

    assert fingerprints._read_runtime_fingerprint(receipt) is None
    for require_digest in (False, True):
        assert not fingerprints._runtime_artifact_fingerprint_matches(
            artifact,
            plan,
            receipt,
            environment=environment,
            require_artifact_digest=require_digest,
        )
    # Final-link projections remain valid, but cannot admit a Cargo result.
    link_projection = {"version": 4, **plan}
    assert fingerprints._runtime_fingerprint_payload_is_valid(link_projection)
    with pytest.raises(ValueError, match="Cargo environment inputs"):
        fingerprints._admitted_runtime_fingerprint(plan, link_projection)


def test_fingerprint_metadata_refresh_retains_admitted_observation_and_bytes(
    tmp_path: Path,
) -> None:
    environment = ProcessEnvironment({"HF302_SCRIPT_INPUT": "old-result"})
    observation = CargoEnvironmentInputs.capture(
        environment, ("HF302_SCRIPT_INPUT",), local_unit_count=1
    )
    artifact = tmp_path / "runtime.bin"
    artifact.write_bytes(b"runtime-result")
    receipt = tmp_path / "runtime.json"
    plan = {
        "hash": "a" * 64,
        "inputs_digest": "b" * 64,
        "source_state": {"head": "old"},
    }
    fingerprints._write_runtime_fingerprint(
        receipt, plan, artifact=artifact, cargo_environment=observation
    )
    stored = fingerprints._read_runtime_fingerprint(receipt)
    assert stored is not None
    request_observation = CargoEnvironmentInputs.capture(
        environment.updated({"HF302_SCRIPT_INPUT": "new-request"}),
        ("HF302_SCRIPT_INPUT",),
        local_unit_count=1,
    )
    admitted = fingerprints._admitted_runtime_fingerprint(
        {**plan, "cargo_environment": request_observation.to_dict()}, stored
    )
    assert admitted["cargo_environment"] == stored["cargo_environment"]
    refreshed = {**admitted, "inputs_digest": "c" * 64, "source_state": {"head": "new"}}
    fingerprints._refresh_runtime_fingerprint_metadata(receipt, refreshed)
    result = fingerprints._read_runtime_fingerprint(receipt)
    assert result is not None
    assert result["inputs_digest"] == "c" * 64
    assert result["source_state"] == {"head": "new"}
    assert result["cargo_environment"] == stored["cargo_environment"]
    assert result["artifact_content_identity"] == stored["artifact_content_identity"]
    assert artifact.read_bytes() == b"runtime-result"


@pytest.mark.parametrize("replacement", ["different", "missing", "malformed"])
def test_fingerprint_refresh_refuses_receipt_replacement_after_admission(
    tmp_path: Path, replacement: str
) -> None:
    environment = ProcessEnvironment({"HF302_SCRIPT_INPUT": "first-result"})
    observation = CargoEnvironmentInputs.capture(
        environment, ("HF302_SCRIPT_INPUT",), local_unit_count=1
    )
    artifact = tmp_path / "runtime.bin"
    artifact.write_bytes(b"identical-result-bytes")
    receipt = tmp_path / "runtime.json"
    plan = {"hash": "a" * 64, "inputs_digest": "b" * 64}
    fingerprints._write_runtime_fingerprint(
        receipt, plan, artifact=artifact, cargo_environment=observation
    )
    initial = fingerprints._read_runtime_fingerprint(receipt)
    assert initial is not None
    admitted = fingerprints._admitted_runtime_fingerprint(plan, initial)
    other = CargoEnvironmentInputs.capture(
        environment.updated({"HF302_SCRIPT_INPUT": "second-result"}),
        ("HF302_SCRIPT_INPUT",),
        local_unit_count=1,
    )
    fingerprints._write_runtime_fingerprint(
        receipt, plan, artifact=artifact, cargo_environment=other
    )
    replacement_receipt = fingerprints._read_runtime_fingerprint(receipt)
    assert replacement_receipt is not None
    assert replacement_receipt["cargo_environment"] == other.to_dict()
    assert (
        replacement_receipt["artifact_content_identity"]
        == initial["artifact_content_identity"]
    )
    if replacement != "different":
        payload = json.loads(receipt.read_text(encoding="utf-8"))
        if replacement == "missing":
            del payload["cargo_environment"]
        else:
            payload["cargo_environment"]["variables"][0]["sha256"] = "invalid"
        receipt.write_text(json.dumps(payload), encoding="utf-8")
    replaced_bytes = receipt.read_bytes()
    diagnostic = (
        "cannot change its Cargo inputs"
        if replacement == "different"
        else "lost custody"
    )
    with pytest.raises(ValueError, match=diagnostic):
        fingerprints._refresh_runtime_fingerprint_metadata(
            receipt, {**admitted, "inputs_digest": "c" * 64}
        )
    assert receipt.read_bytes() == replaced_bytes
    assert artifact.read_bytes() == b"identical-result-bytes"


@pytest.fixture
def plan_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, readonly_wasi_sdk
) -> Path:
    original_plan = _plan

    def plan_with_sdk(root, *, target=None, env=None, **kwargs):
        environment = ProcessEnvironmentBuilder({} if env is None else env)
        uses_c_sdk = target == "wasm32-wasip1" or (
            target == "wasm32-unknown-unknown" and "MOLT_WASI_C_ABI_PLAN" in environment
        )
        if uses_c_sdk and not any(
            key in environment for key in ("WASI_SDK_PATH", "WASI_SDK_PREFIX")
        ):
            environment["WASI_SDK_PATH"] = str(readonly_wasi_sdk().sdk)
        return original_plan(root, target=target, env=environment, **kwargs)

    monkeypatch.setattr(sys.modules[__name__], "_plan", plan_with_sdk)

    def executable(value: str, **_kwargs: object) -> Path:
        if Path(value).is_absolute() and Path(value).is_file():
            return Path(value)
        path = tmp_path / str(value).replace("/", "_").replace("\\", "_")
        if not path.exists():
            write_mock_executable(path, b"MZ" + str(value).encode())
        return path

    monkeypatch.setattr(runtime_tool_selection, "resolve_executable", executable)

    def find(value, *, cwd, **_kwargs):
        assert cwd == tmp_path
        return tmp_path / value

    for owner in (plans, runtime_tool_selection):
        monkeypatch.setattr(owner, "find_executable", find)

    def metadata_command(command, **kwargs):
        if tuple(command[1:]) == ("--version",):
            return subprocess.CompletedProcess(
                command, 0, "cargo 1.99.0 (fixture)\n", ""
            )
        if tuple(command[1:]) == ("-vV",):
            # The real resolver queries its admitted physical Rust compiler
            # before selecting target metadata. Match this fixture's host.
            assert Path(command[0]) == Path(kwargs["env"]["RUSTC"])
            assert Path(command[0]).is_file()
            return subprocess.CompletedProcess(
                command, 0, "rustc fixture\nhost: x86_64-unknown-linux-gnu\n", ""
            )
        assert "--print=file-names" in command
        assert kwargs["input"] == ""
        target = (
            command[command.index("--target") + 1] if "--target" in command else None
        )
        stdout, stderr = rustc_target_metadata_output(tmp_path, target=target)
        return subprocess.CompletedProcess(command, 0, stdout, stderr)

    monkeypatch.setattr(plans.process_guard, "run_completed_command", metadata_command)
    resources = tmp_path / "rust-resources"
    resources.mkdir()
    (resources / "libcore.rlib").write_bytes(b"rust-core")
    monkeypatch.setattr(
        plans,
        "_rust_resource_roots",
        lambda *args, **kwargs: (
            resource_custody.CargoResourceRoot("rust/test", resources),
        ),
    )
    return tmp_path


def _metadata(sysroot: Path, cfg: str = "unix\nselected\n"):
    from molt.cli.cargo_target_cfg import parse_rustc_target_metadata

    return parse_rustc_target_metadata(_metadata_stdout(sysroot, cfg), "")


def _plan(
    root: Path,
    *,
    env: Mapping[str, str] | None = None,
    args: tuple[str, ...] = (),
    target: str | None = None,
    **kwargs: object,
) -> plans.RuntimeCargoPlan:
    environment = ProcessEnvironmentBuilder({} if env is None else env)
    environment.setdefault("CARGO_HOME", str(root / "cargo-home"))
    return plans.resolve_runtime_cargo_plan(
        root,
        env=environment,
        cargo_command=("cargo", "rustc", *args),
        requested_target=target,
        host_target="x86_64-unknown-linux-gnu",
        **kwargs,
    )


def _with_remaps(root: Path, flags: tuple[str, ...]) -> tuple[str, ...]:
    # Independent expected policy for this fixture's default root/target layout.
    return (
        *flags,
        f"--remap-path-prefix={root}=/molt/source",
        f"--remap-path-prefix={root / 'target'}=/molt/build",
    )


def _config(root: Path, text: str) -> Path:
    directory = root / ".cargo"
    directory.mkdir(exist_ok=True)
    path = directory / "config.toml"
    path.write_text(text, encoding="utf-8")
    return path


def test_environment_linker_overrides_config_and_cli_overrides_environment(
    plan_root: Path,
) -> None:
    _config(
        plan_root, '[target.x86_64-unknown-linux-gnu]\nlinker="configured-linker"\n'
    )
    environment = {"CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "environment-linker"}
    assert (
        _plan(plan_root, env=environment).tools["linker"].name == "environment-linker"
    )
    explicit = _plan(
        plan_root,
        env=environment,
        args=("--config", 'target.x86_64-unknown-linux-gnu.linker="cli-linker"'),
    )
    assert explicit.tools["linker"].name == "cli-linker"
    assert explicit.environment["CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER"] == str(
        explicit.tools["linker"]
    )


@pytest.mark.parametrize("target", [None, "wasm32-wasip1"])
@pytest.mark.parametrize("locked", [(), ("--locked",)])
def test_runtime_plan_locks_compiler_dependencies_before_rustc_passthrough(
    plan_root: Path, target: str | None, locked: tuple[str, ...]
) -> None:
    plan = _plan(
        plan_root,
        args=(
            *(("--target", target) if target is not None else ()),
            *locked,
            "--",
            "--print",
            "native-static-libs",
        ),
        target=target,
        # _plan supplies a complete synthetic SDK for WASM. Native C tools
        # remain separately authored host inputs.
        env={"MOLT_SKIP_CARGO_LOCK": "1"}
        | {name: "selected-" + name.lower() for name in ("CC", "CXX", "AR", "RANLIB")},
    )
    separator = plan.command.index("--")
    assert plan.command[:separator].count("--locked") == 1
    assert plan.command[separator : separator + 3] == (
        "--",
        "--print",
        "native-static-libs",
    )
    assert plan.command[separator + 3 :] == (
        ("-C", "link-self-contained=no", "-C", "linker-flavor=wasm-ld")
        if target == "wasm32-wasip1"
        else ()
    )


@pytest.mark.parametrize(
    "target",
    [
        "wasm32-wasip1",
        "wasm32-unknown-unknown",
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ],
)
def test_cfg_target_plan_uses_rustc_facts_and_pins_selected_tools(
    plan_root: Path,
    monkeypatch: pytest.MonkeyPatch,
    target: str,
) -> None:
    wasm = target.startswith("wasm32")
    fact_text = 'target_arch="wasm32"' if wasm else 'target_arch="native-test"'
    seen = []

    def probe(rustc, selected_target, flags, **kwargs):
        seen.append((selected_target, flags))
        return _metadata(plan_root, fact_text)

    monkeypatch.setattr(rustc_probes, "rust_target_metadata", probe)
    _config(
        plan_root,
        "[target.'cfg(not(target_arch=\"wasm32\"))']\nrustflags=[]\n"
        '[target.\'cfg(target_arch="wasm32")\']\nlinker="wasm-linker"\nrustflags=["--cfg","wasm_selected"]\n'
        '[build]\nrustflags=["--cfg","baseline"]\n',
    )
    plan = _plan(
        plan_root,
        args=("--target", target),
        target=target,
        env={name: "selected-" + name.lower() for name in ("CC", "CXX", "AR", "RANLIB")}
        | (
            {}
            if wasm or target == "x86_64-unknown-linux-gnu"
            else {
                f"CARGO_TARGET_{target.upper().replace('-', '_')}_LINKER": "cross-linker"
            }
        ),
    )
    expected = ("--cfg", "wasm_selected" if wasm else "baseline")
    if target == "wasm32-wasip1":
        expected = (
            "-L",
            "native=" + str(plan.wasi_c_abi.path("libc").parent),
            "-L",
            "native=" + str(plan.wasi_c_abi.path("compiler_rt").parent),
            "-C",
            "link-self-contained=no",
            "-C",
            "linker-flavor=wasm-ld",
            *expected,
        )
    assert plan.rustflags == _with_remaps(plan_root, expected)
    assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(plan.rustflags)
    assert seen and all(selected == target for selected, _ in seen)
    if target == "wasm32-wasip1":
        assert plan.tools["linker"] == Path(plan.environment["MOLT_WASM_LD"])
        assert all("wasm-linker" not in token for token in plan.command)
    elif wasm:
        assert plan.tools["linker"].name == "wasm-linker"
        assert any("wasm-linker" in token for token in plan.command)


@pytest.mark.parametrize("failure", ["exit", "empty", "mutated", "none"])
def test_cfg_probe_retains_selected_compiler_and_reports_failure(
    plan_root: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: str,
) -> None:
    rustc = plan_root / "cfg-rustc"
    write_mock_executable(rustc, b"compiler-before")
    expected = [
        str(rustc),
        *cargo_target_cfg.cargo_target_query_arguments(
            "wasm32-wasip1", ("--cfg", "selected")
        ),
    ]
    seen = []

    def run(command, **kwargs):
        seen.append(command)
        assert kwargs["cwd"] == plan_root
        assert kwargs["timeout"] == 30
        assert kwargs["input"] == ""
        assert "RUSTC_LOG" not in kwargs["env"]
        if failure == "mutated":
            rustc.write_bytes(b"compiler-after!")
        return subprocess.CompletedProcess(
            command,
            7 if failure == "exit" else 0,
            ""
            if failure == "empty"
            else _metadata_stdout(plan_root, 'target_arch="wasm32"\nselected\n'),
            "cfg diagnostic" if failure == "exit" else "",
        )

    monkeypatch.setattr(plans.process_guard, "run_completed_command", run)
    if failure == "none":
        metadata = rustc_probes.rust_target_metadata(
            rustc,
            "wasm32-wasip1",
            ("--cfg", "selected"),
            root=plan_root,
            env={"RUSTC_LOG": "debug"},
        )
        assert ("target_arch", "wasm32") in metadata.cfg
        assert ("selected", None) in metadata.cfg
    else:
        with pytest.raises((ValueError, OSError)) as exc:
            rustc_probes.rust_target_metadata(
                rustc,
                "wasm32-wasip1",
                ("--cfg", "selected"),
                root=plan_root,
                env={},
            )
        if failure == "exit":
            assert "cfg diagnostic" in str(exc.value)
    assert seen == [expected]


def test_cfg_linker_conflicts_are_rejected_unless_exact_target_wins(
    plan_root: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        rustc_probes,
        "rust_target_metadata",
        lambda *a, **k: _metadata(plan_root, "unix"),
    )
    _config(
        plan_root,
        "[target.'cfg(unix)']\nlinker=\"first\"\n"
        "[target.'cfg(all(unix))']\nlinker=\"second\"\n",
    )
    with pytest.raises(ValueError, match="multiple cfg linkers"):
        _plan(plan_root)
    plan = _plan(
        plan_root, env={"CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "exact"}
    )
    assert plan.tools["linker"].name == "exact"


def test_empty_wrapper_environment_disables_configured_wrapper(plan_root: Path) -> None:
    _config(plan_root, '[build]\nrustc-wrapper="configured-wrapper"\n')
    plan = _plan(plan_root, env={"RUSTC_WRAPPER": ""})
    assert not plan.wrappers
    assert plan.environment["RUSTC_WRAPPER"] == ""


@pytest.mark.parametrize("scope", ["build", "target.x86_64-unknown-linux-gnu"])
@pytest.mark.parametrize("override", [None, "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"])
def test_cargo_string_lists_merge_file_cli_and_environment_before_cfg_selection(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, scope: str, override: str | None
) -> None:
    _config(
        plan_root,
        f'[{scope}]\nrustflags=["--cfg","file"]\n'
        "[target.'cfg(all(file,cli,environment))']\nlinker=\"merged-linker\"\n",
    )
    selected_environment = (
        "CARGO_BUILD_RUSTFLAGS"
        if scope == "build"
        else "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS"
    )
    env = {selected_environment: "--cfg environment"}
    expected = ("--cfg", "file", "--cfg", "cli", "--cfg", "environment")
    if override is not None:
        env[override] = "--cfg override" if override == "RUSTFLAGS" else ""
        expected = ("--cfg", "override") if override == "RUSTFLAGS" else ()
    probes = []

    def metadata(rustc, target, flags, **kwargs):
        probes.append((target, flags))
        return _metadata(
            plan_root,
            "\n".join(
                (
                    "unix",
                    *(
                        value
                        for option, value in zip(flags, flags[1:])
                        if option == "--cfg"
                    ),
                )
            ),
        )

    monkeypatch.setattr(rustc_probes, "rust_target_metadata", metadata)
    plan = _plan(
        plan_root, env=env, args=("--config", f'{scope}.rustflags=["--cfg","cli"]')
    )
    assert plan.rustflags == _with_remaps(plan_root, expected)
    assert probes == [(None, expected), (None, _with_remaps(plan_root, expected))] * 2
    if override is None:
        assert plan.tools["linker"].name == "merged-linker"
    else:
        assert plan.tools["linker"].name != "merged-linker"


def test_encoded_flags_win_and_transform_is_applied_once(plan_root: Path) -> None:
    _config(plan_root, '[build]\nrustflags=["--cfg", "configured"]\n')
    seen = []

    def transform(flags: tuple[str, ...]) -> tuple[str, ...]:
        seen.append(flags)
        return (*flags, "--cfg", "molt")

    plan = _plan(
        plan_root,
        env={
            "CARGO_ENCODED_RUSTFLAGS": "--cfg\x1fencoded",
            "RUSTFLAGS": "--cfg ignored",
        },
        rustflags_transform=transform,
    )
    assert seen == [("--cfg", "encoded")]
    assert plan.rustflags == _with_remaps(
        plan_root, ("--cfg", "encoded", "--cfg", "molt")
    )
    assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(plan.rustflags)


@pytest.mark.parametrize(
    ("env", "selected"),
    [
        ({}, ("--cfg", "configured")),
        ({"RUSTFLAGS": "--cfg environment"}, ("--cfg", "environment")),
        (
            {"CARGO_ENCODED_RUSTFLAGS": "--cfg\x1fencoded", "RUSTFLAGS": "--cfg x"},
            ("--cfg", "encoded"),
        ),
    ],
)
def test_rust_cpu_policy_extends_the_flags_cargo_selects(
    plan_root: Path, env: dict[str, str], selected: tuple[str, ...]
) -> None:
    # Cargo's precedence (encoded > RUSTFLAGS > config) picks one source. The
    # native policy appends to that source's flags and never replaces them;
    # the plan pins the result for Cargo and for every identity that binds it.
    _config(plan_root, '[build]\nrustflags=["--cfg", "configured"]\n')

    baseline = _plan(
        plan_root, env=env, rustflags_transform=RustCpuPolicy.BASELINE.rustflags
    )
    native = _plan(
        plan_root, env=env, rustflags_transform=RustCpuPolicy.NATIVE.rustflags
    )

    assert baseline.rustflags == _with_remaps(plan_root, selected)
    assert native.rustflags == _with_remaps(
        plan_root, (*selected, "-C", "target-cpu=native")
    )
    for plan in (baseline, native):
        assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(
            plan.rustflags
        )


def test_normalized_cfg_metadata_is_reused_for_dependency_and_final_resources(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    host = "x86_64-unknown-linux-gnu"
    installed, dependency, final = (
        plan_root / name for name in ("installed", "dependency", "final")
    )
    for sysroot in (installed, dependency, final):
        libdir = sysroot / "lib" / "rustlib" / host / "lib"
        libdir.mkdir(parents=True)
        (libdir / "libcore.rlib").write_bytes(sysroot.name.encode())
    (installed / "lib" / "rustc_driver.dll").write_bytes(b"installed-driver")
    (installed / "bin").mkdir()
    search = plan_root / "search"
    search.mkdir()
    original = ("-L", "native=search")
    normalized = ("-L", "native=" + str(search))
    final_flags = (*_with_remaps(plan_root, normalized), "--cfg", "final_crate")
    _config(plan_root, "[target.'cfg(normalized)']\nlinker=\"normalized-linker\"\n")
    probes = []

    def metadata(rustc, target, flags, **kwargs):
        wrapped = bool(kwargs["wrappers"])
        probes.append((target, flags, wrapped))
        if not wrapped:
            return _metadata(installed, "unix")
        selected = final if flags == final_flags else dependency
        facts = "unix\nnormalized" if flags[:2] == normalized else "unix"
        return _metadata(selected, facts)

    # Fake the rustc query itself: the reuse under test is the memo above it.
    monkeypatch.setattr(rustc_probes, "rust_target_metadata", metadata)
    monkeypatch.setattr(plans, "_rust_resource_roots", _rust_resource_roots)
    plan = _plan(
        plan_root,
        env={"RUSTFLAGS": "-L native=search", "RUSTC_WRAPPER": "metadata-wrapper"},
        args=("--", "--cfg", "final_crate"),
    )
    assert plan.rustflags == _with_remaps(plan_root, normalized)
    assert plan.tools["linker"].name == "normalized-linker"
    # Resource discovery reuses the normalized cfg probe, while retaining a
    # distinct final-crate selection and the unwrapped installed driver.
    questions = [
        (None, original, True),
        (None, _with_remaps(plan_root, normalized), True),
        (None, (), False),
        (None, final_flags, True),
    ]
    assert probes == questions + questions
    _plan(
        plan_root,
        env={"RUSTFLAGS": "-L native=search", "RUSTC_WRAPPER": "metadata-wrapper"},
        args=("--", "--cfg", "final_crate"),
    )
    assert probes == questions * 4  # each plan owns fresh discovery and qualification
    roots = {entry.label: entry.path for entry in plan.rust_resources.roots}
    assert (
        roots["rust/target-libdir/0"] == dependency / "lib" / "rustlib" / host / "lib"
    )
    assert roots["rust/target-libdir/1"] == final / "lib" / "rustlib" / host / "lib"
    assert roots["rust/driver"] == installed / "lib"
    assert any(
        item.identity.path == installed / "lib" / "rustc_driver.dll"
        for item in plan.rust_resources.files
    )


def test_actual_host_target_flags_are_resolved_for_native(plan_root: Path) -> None:
    plan = _plan(
        plan_root, env={"CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS": "--cfg host"}
    )
    assert plan.rustflags == _with_remaps(plan_root, ("--cfg", "host"))


def test_configuration_parse_and_receipt_share_one_generation(plan_root: Path) -> None:
    path = _config(plan_root, '[build]\nrustflags="--cfg before"\n')
    plan = _plan(plan_root)
    before = path.stat()
    path.write_text('[build]\nrustflags="--cfg after!"\n', encoding="utf-8")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    before_identity = plan.configuration_identity()
    assert plan.configuration_identity() == before_identity
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("mutation", ["rustc", "resource", "config", "new-config"])
def test_native_capture_rechecks_custody_before_toolchain_projection(
    plan_root, mutation
):
    from molt.cli import runtime_build_identity as identity

    config = _config(plan_root, '[build]\nrustflags="--cfg before"\n')
    plan = _plan(plan_root)
    if mutation == "new-config":
        extra = plan_root / "cargo-home" / "config.toml"
        extra.parent.mkdir(exist_ok=True)
        extra.write_text('[build]\nrustflags="--cfg inserted"\n', encoding="utf-8")
    else:
        path = {
            "rustc": plan.tools["rustc"],
            "resource": plan_root / "rust-resources" / "libcore.rlib",
            "config": config,
        }[mutation]
        before = path.stat()
        path.write_bytes(path.read_bytes().replace(b"before", b"after!") + b"!")
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises((ValueError, OSError), match="changed"):
        identity.provision_native_runtime_toolchain_content_manifest(
            project_root=plan_root,
            env=plan.environment,
            target_triple=None,
            cargo_command=plan.command,
            cargo_plan=plan,
        )


def test_new_configuration_file_invalidates_live_plan(plan_root: Path) -> None:
    plan = _plan(plan_root)
    _config(plan_root, '[build]\nrustflags="--cfg inserted"\n')
    with pytest.raises(ValueError, match="selection changed"):
        plan.verify()


def test_compile_partition_excludes_only_link_arguments(plan_root: Path) -> None:
    (plan_root / "first.rsp").write_text("--export=first\n", encoding="utf-8")
    (plan_root / "second.rsp").write_text("--export=second\n", encoding="utf-8")
    first = _plan(
        plan_root, args=("--", "-C", "panic=abort", "-C", "link-arg=@first.rsp")
    )
    second = _plan(
        plan_root, args=("--", "-C", "panic=abort", "-C", "link-arg=@second.rsp")
    )
    assert first.partition_command()[0] == second.partition_command()[0]
    assert first.partition_command()[1] != second.partition_command()[1]
    assert "panic=abort" in first.partition_command()[0]
    assert (
        first.rust_resources.content_identity()
        == second.rust_resources.content_identity()
    )
    assert first.configuration_identity() == second.configuration_identity()
    assert first.project_link_arguments(
        first.partition_command()[1]
    ) != second.project_link_arguments(second.partition_command()[1])


def test_final_link_response_generation_is_fenced_even_after_byte_restore(
    plan_root: Path,
) -> None:
    response = plan_root / "exports.rsp"
    original = b"--export=first\n"
    response.write_bytes(original)
    plan = _plan(plan_root, args=("--", "-C", "link-arg=@" + str(response)))
    before = response.stat()
    response.write_bytes(b"--export=other\n")
    response.write_bytes(original)
    os.utime(response, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        plan.verify()
    with pytest.raises(ValueError, match="changed"):
        plan.project_link_arguments(plan.partition_command()[1])


def test_link_response_projection_consumes_capture_not_a_second_read(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    response = plan_root / "exports.rsp"
    response.write_bytes(b"--export=first\n")
    plan = _plan(plan_root, args=("--", "-C", "link-arg=@" + str(response)))
    monkeypatch.setattr(
        plans,
        "capture_stable_regular_file",
        lambda *args, **kwargs: pytest.fail(
            "response bytes were independently recaptured"
        ),
    )
    projected = plan.project_link_arguments(plan.partition_command()[1])
    assert any("@response:sha256=" in item for item in projected)


def test_link_response_aliases_share_one_bounded_byte_capture(tmp_path, monkeypatch):
    response = tmp_path / "exports.rsp"
    response.write_bytes(b"--export=first\n")
    original = resource_custody.capture_stable_regular_file
    captures = []

    def capture(path, **kwargs):
        assert (
            kwargs["max_bytes"] == resource_custody.RUNTIME_ARTIFACT_METADATA_MAX_BYTES
        )
        captures.append(path)
        return original(path, **kwargs)

    monkeypatch.setattr(resource_custody, "capture_stable_regular_file", capture)
    roots = (
        resource_custody.CargoResourceRoot("ordinary-first", response),
        resource_custody.CargoResourceRoot("rust/link-response/first", response),
        resource_custody.CargoResourceRoot("rust/link-response/second", response),
    )
    custody = resource_custody.CargoResourceCustody.capture(roots)
    assert captures == [response]
    assert len(custody.files) == 3
    assert all(item.identity is custody.files[0].identity for item in custody.files)


@pytest.mark.parametrize(
    "force,expected", [(False, "ambient-cc"), (True, "configured-cc")]
)
def test_cargo_environment_force_selects_and_pins_actual_build_tool(
    plan_root: Path, force: bool, expected: str
) -> None:
    _config(
        plan_root,
        '[env]\nCC={value="configured-cc",force=' + str(force).lower() + "}\n",
    )
    plan = _plan(plan_root, env={"CC": "ambient-cc"})
    assert plan.tools["cc"].name == expected
    assert plan.environment["CC_x86_64-unknown-linux-gnu"] == str(plan.tools["cc"])
    assert ('env."CC".force=false' in plan.command) is force


def test_cargo_environment_cli_value_origin_and_relative_resource_custody(
    plan_root: Path,
) -> None:
    resource = plan_root / "headers"
    resource.mkdir()
    header = resource / "api.h"
    header.write_bytes(b"header")
    _config(plan_root, '[env]\nSDK={value="unused",relative=true,force=true}\n')
    plan = _plan(plan_root, args=("--config", 'env.SDK.value="headers"'))
    assert plan.environment["SDK"] == str(resource)
    assert 'env."SDK".force=false' in plan.command
    assert any(item.identity.path == header for item in plan.rust_resources.files)
    header.write_bytes(b"changed header")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


def test_nonforced_cargo_environment_does_not_capture_ignored_relative_path(
    plan_root: Path,
) -> None:
    _config(plan_root, '[env]\nSDK={value="absent",relative=true}\n')
    plan = _plan(plan_root, env={"SDK": "ambient"})
    assert plan.environment["SDK"] == "ambient"
    assert not any(
        item.label.startswith("cargo/env/") for item in plan.rust_resources.files
    )


@pytest.mark.parametrize("rule", list(NameRule))
def test_environment_key_semantics_are_platform_gated(rule: NameRule) -> None:
    env = ProcessEnvironmentBuilder({"cc": "lower", "CC": "upper"}, rule=rule)
    assert env["cc"] == ("upper" if rule is NameRule.WINDOWS else "lower")
    assert len(env) == (1 if rule is NameRule.WINDOWS else 2)
    selected, forced, _roots = plans._apply_cargo_environment(
        {"env": {"cC": {"value": "configured", "force": True}}}, {}, env, {}
    )
    assert selected["CC"] == ("configured" if rule is NameRule.WINDOWS else "upper")
    assert forced == ("cC",)


@pytest.mark.parametrize("rule", list(NameRule))
def test_cargo_wrapper_policy_and_removal_keep_the_name_rule(rule):
    env = ProcessEnvironment(
        {"rustc_wrapper": "sccache", "Cargo_Incremental": "1"}, rule=rule
    )
    selected, policies = cargo_execution_policy.normalize_cargo_environment(
        env, cwd=None, default_incremental="1"
    )
    removed = cargo_execution_policy.without_sccache_compiler_wrappers(env)
    assert selected.rule is removed.rule is rule
    if rule is NameRule.WINDOWS:
        assert selected.child_block() == {"rustc_wrapper": "sccache"}
        assert policies == ("sccache-profile-incremental",)
        assert removed.child_block() == {"Cargo_Incremental": "1"}
    else:
        assert selected.child_block() == {
            "rustc_wrapper": "sccache",
            "Cargo_Incremental": "1",
            "CARGO_INCREMENTAL": "1",
        }
        assert policies == ("direct-rustc-enables-incremental",)
        assert removed == env
    assert dict(env) == {"rustc_wrapper": "sccache", "Cargo_Incremental": "1"}


@pytest.mark.parametrize("rule", list(NameRule))
def test_sccache_client_pins_case_preserving_temporary_names(
    tmp_path, monkeypatch, rule
):
    monkeypatch.setattr(cargo_execution_policy, "_SCCACHE_SERVER_SOCKETS", False)
    cache = tmp_path / "cache"
    temporary = tmp_path / "cache-tmp"
    env = ProcessEnvironment({"Sccache_Dir": str(cache), "Temp": "old"}, rule=rule)
    selected, policies = cargo_execution_policy.sccache_client_environment(env)
    assert isinstance(selected, ProcessEnvironment) and selected.rule is rule
    if rule is NameRule.WINDOWS:
        assert selected["Temp"] == str(temporary)
        assert selected["TMP"] == selected["TMPDIR"] == str(temporary)
        assert "TEMP" not in selected.child_block()
        assert policies == (
            "sccache-server-temp-dir",
            "sccache-server-private-endpoint",
        )
        assert temporary.is_dir()
    else:
        assert selected == env
        assert policies == ()
        assert not temporary.exists()
    assert dict(env) == {"Sccache_Dir": str(cache), "Temp": "old"}


def test_windows_cargo_environment_case_alias_cannot_bypass_ambient_precedence() -> (
    None
):
    env = ProcessEnvironmentBuilder({"CC": "ambient"}, rule=NameRule.WINDOWS)
    selected, forced, _roots = plans._apply_cargo_environment(
        {"env": {"cc": "configured"}}, {}, env, {}
    )
    assert selected["cc"] == "ambient"
    assert not forced
    with pytest.raises(ValueError, match="case-ambiguous"):
        plans._apply_cargo_environment({"env": {"cc": "one", "CC": "two"}}, {}, env, {})


def test_plan_owns_final_selected_inputs(plan_root: Path) -> None:
    _config(plan_root, '[env]\nCC={value="configured-cc",force=true}\n')
    plan = _plan(plan_root)
    assert plan.tools["cc"].name == "configured-cc"
    with pytest.raises(TypeError):
        plan.environment["CC"] = "other"
    assert any(item.entrypoint == plan.tools["cc"] for item in plan.executable_custody)
    plan.verify()


@pytest.mark.parametrize(
    "payload,diagnostic",
    [
        (b"@nested.rsp\n", "nests an unsupported response resource"),
        (b"--script=link.ld\n", "requires explicit resource custody"),
        (b"/LIBPATH:elsewhere\n", "requires explicit resource custody"),
        (b"library.a\n", "requires explicit resource custody"),
        (b"--export=has space\n", "unsafe whitespace"),
        (b"\xff\n", "not UTF-8"),
    ],
)
def test_unadmitted_link_response_resources_fail_before_execution(
    plan_root: Path, payload: bytes, diagnostic: str
) -> None:
    response = plan_root / "exports.rsp"
    response.write_bytes(payload)
    with pytest.raises(ValueError, match=diagnostic):
        _plan(plan_root, args=("--", "-C", "link-arg=@" + str(response)))


def test_custom_profile_debug_policy_comes_from_inheritance_not_name(
    plan_root: Path,
) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.futuristic]\ninherits="dev"\n[profile.debug_named]\ninherits="release"\n',
        encoding="utf-8",
    )
    plan = _plan(plan_root)
    assert plan.preserve_debug_for_profile("futuristic")
    assert not plan.preserve_debug_for_profile("debug_named")
    override = _plan(plan_root, env={"CARGO_PROFILE_FUTURISTIC_DEBUG": "0"})
    assert not override.preserve_debug_for_profile("futuristic")


@pytest.mark.parametrize(
    "role",
    [
        "cargo",
        "rustc",
        "cc",
        "cxx",
        "ar",
        "ranlib",
        "linker",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ],
)
def test_live_plan_fences_every_selected_tool_generation(
    plan_root: Path, role: str
) -> None:
    plan = _plan(
        plan_root,
        env={
            "RUSTC_WRAPPER": "wrapper",
            "RUSTC_WORKSPACE_WRAPPER": "workspace-wrapper",
        },
    )
    path = (plan.wrappers if role in plan.wrappers else plan.tools)[role]
    before = path.stat()
    path.write_bytes(b"X" * before.st_size)
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("mutation", ["same-size", "add", "remove"])
def test_live_plan_fences_resource_generation_and_membership(
    plan_root: Path, mutation: str
) -> None:
    plan = _plan(plan_root)
    resource = plan_root / "rust-resources" / "libcore.rlib"
    if mutation == "same-size":
        before = resource.stat()
        resource.write_bytes(b"X" * before.st_size)
        os.utime(resource, ns=(before.st_atime_ns, before.st_mtime_ns))
    elif mutation == "remove":
        resource.unlink()
    else:
        resource.with_name("new-codegen.dll").write_bytes(b"new")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


def test_inherited_profile_overrides_enter_identity_and_policy(plan_root: Path) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.release-output]\ninherits="release"\n', encoding="utf-8"
    )
    baseline = _plan(plan_root)
    inherited = _plan(
        plan_root,
        env={
            "CARGO_PROFILE_RELEASE_LTO": "fat",
            "CARGO_PROFILE_RELEASE_DEBUG": "2",
            "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL": "3",
        },
    )
    before = _runtime_build_environment_identity(
        baseline, cargo_profile="release-output"
    )
    after = _runtime_build_environment_identity(
        inherited, cargo_profile="release-output"
    )
    assert before != after
    assert "CARGO_PROFILE_RELEASE_LTO" in after
    assert "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL" in after
    assert inherited.preserve_debug_for_profile("release-output")
    assert not baseline.preserve_debug_for_profile("release-output")


def test_native_dependency_inputs_are_observed_instead_of_guessed_in_plan_identity(
    plan_root: Path,
) -> None:
    baseline = _plan(plan_root, env={"AWS_LC_SYS_SMALL": "0"})
    changed = _plan(plan_root, env={"AWS_LC_SYS_SMALL": "1"})
    unused = _plan(
        plan_root,
        env={
            "AWS_LC_SYS_SMALL": "0",
            "AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu": "1",
            "AWS_LC_SYS_PROJECT_NOTE": "",
            "HOST_CFLAGS_aarch64_unknown_linux_gnu": "-include /inactive.h",
            "TARGET_CFLAGS_aarch64_unknown_linux_gnu": "",
        },
    )
    before = _runtime_build_environment_identity(baseline, cargo_profile="dev")
    after = _runtime_build_environment_identity(changed, cargo_profile="dev")
    other = _runtime_build_environment_identity(unused, cargo_profile="dev")

    assert before == after == other
    names = (
        "AWS_LC_SYS_SMALL",
        "AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu",
        "AWS_LC_SYS_PROJECT_NOTE",
        "HOST_CFLAGS_aarch64_unknown_linux_gnu",
        "TARGET_CFLAGS_aarch64_unknown_linux_gnu",
    )
    recorded = [
        CargoEnvironmentInputs.capture(plan.environment, names, local_unit_count=1)
        for plan in (baseline, changed, unused)
    ]
    assert len({item.digest for item in recorded}) == 3
    before_values, after_values, other_values = (
        dict(item.variables) for item in recorded
    )
    key = baseline.environment.rule.key
    assert before_values["AWS_LC_SYS_SMALL"] == (
        "5feceb66ffc86f38d952786c6d696c79c2dbc239dd4e91b46729d73a27fb57e9"
    )
    assert after_values["AWS_LC_SYS_SMALL"] == (
        "6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b"
    )
    assert before_values["AWS_LC_SYS_PROJECT_NOTE"] is None
    assert other_values["AWS_LC_SYS_PROJECT_NOTE"] == (
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    )
    assert other_values[key("HOST_CFLAGS_aarch64_unknown_linux_gnu")] is not None
    assert other_values[key("TARGET_CFLAGS_aarch64_unknown_linux_gnu")] == (
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    )
    for plan, observation in zip((baseline, changed, unused), recorded, strict=True):
        observation.verify(plan.environment)
    with pytest.raises(ValueError, match="AWS_LC_SYS_SMALL"):
        recorded[0].verify(changed.environment)
    with pytest.raises(ValueError, match="AWS_LC_SYS_PROJECT_NOTE"):
        recorded[0].verify(unused.environment)


def test_native_dependency_implicit_target_uses_captured_rust_host(
    plan_root: Path,
) -> None:
    controls = {
        "AWS_LC_SYS_USE_SYSTEM": "1",
        "AWS_LC_SYS_USE_SYSTEM_x86_64_unknown_linux_gnu": "0",
    }
    plan = _plan(plan_root, env=controls)
    assert plan.target == "x86_64-unknown-linux-gnu"
    assert all(plan.environment[name] == value for name, value in controls.items())
    same_target = _plan(
        plan_root,
        env={"AWS_LC_SYS_EFFECTIVE_TARGET": "x86_64-unknown-linux-gnu"},
    )
    assert same_target.target == "x86_64-unknown-linux-gnu"
    with pytest.raises(ValueError, match="AWS_LC_SYS_EFFECTIVE_TARGET"):
        _plan(
            plan_root,
            env={"AWS_LC_SYS_EFFECTIVE_TARGET": "aarch64-unknown-linux-gnu"},
        )
    with pytest.raises(
        ValueError, match="AWS_LC_SYS_USE_SYSTEM_x86_64_unknown_linux_gnu"
    ):
        _plan(
            plan_root,
            env={
                "AWS_LC_SYS_USE_SYSTEM": "0",
                "AWS_LC_SYS_USE_SYSTEM_x86_64_unknown_linux_gnu": "1",
            },
        )


def test_native_dependency_empty_target_control_shadows_generic_and_is_observed(
    plan_root: Path,
) -> None:
    generic = {"AWS_LC_SYS_CMAKE_BUILDER": "1"}
    controls = {
        **generic,
        "AWS_LC_SYS_CMAKE_BUILDER_x86_64_unknown_linux_gnu": "",
    }
    selected = _plan(plan_root, env=controls)
    observation = CargoEnvironmentInputs.capture(
        selected.environment, controls, local_unit_count=1
    )
    values = dict(observation.variables)
    key = selected.environment.rule.key
    assert values[key("AWS_LC_SYS_CMAKE_BUILDER_x86_64_unknown_linux_gnu")] == (
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    )
    assert values["AWS_LC_SYS_CMAKE_BUILDER"] == (
        "6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b"
    )
    with pytest.raises(ValueError, match="AWS_LC_SYS_CMAKE_BUILDER"):
        _plan(plan_root, env=generic)
    baseline = _plan(plan_root)
    assert _runtime_build_environment_identity(selected, cargo_profile="dev") == (
        _runtime_build_environment_identity(baseline, cargo_profile="dev")
    )
    observation.verify(selected.environment)
    with pytest.raises(ValueError, match="AWS_LC_SYS_CMAKE_BUILDER"):
        observation.verify(baseline.environment)


def test_native_dependency_observation_changes_without_changing_plan_fingerprint(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import runtime_build_identity as builder
    from molt.cli.runtime_artifact_selection import RUNTIME_STATICLIB_ARTIFACTS
    from molt.cli.runtime_identity_schema import (
        RuntimePublicationTransform,
        runtime_publication_executor_paths,
    )

    for relative in runtime_publication_executor_paths(
        RuntimePublicationTransform.NATIVE_STATICLIB
    ):
        executor = plan_root / relative
        executor.parent.mkdir(parents=True, exist_ok=True)
        executor.write_text("# publication executor fixture\n", encoding="utf-8")

    admitted = native_runtime_staticlib_identity()
    family = admitted.payload["family"]
    monkeypatch.setattr(builder, "runtime_source_roots", lambda *_args: ())
    monkeypatch.setattr(
        builder,
        "_capture_runtime_build_sources",
        lambda *_args: family["compile"]["sources"],
    )
    monkeypatch.setattr(
        builder,
        "provision_native_runtime_toolchain_content_manifest",
        lambda **_kwargs: admitted.toolchain_manifest,
    )

    def resolve(controls):
        plan = _plan(plan_root, env=controls)
        identity = builder.resolve_native_runtime_build_identity(
            plan_root,
            env=plan.environment,
            cargo_profile="dev",
            target_triple=None,
            runtime_features=("stdlib_net",),
            cargo_command=plan.command,
            artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
            cargo_plan=plan,
        )
        return identity, plan.environment

    absent, absent_environment = resolve({})
    for controls in (
        {"AWS_LC_SYS_CMAKE_BUILDER_x86_64_unknown_linux_gnu": ""},
        {"HOST_CFLAGS_aarch64_unknown_linux_gnu": "-include /unused.h"},
        {"TARGET_CFLAGS_aarch64_unknown_linux_gnu": ""},
        {"HF302_ARBITRARY_BUILD_SCRIPT_INPUT": "dependency-selected"},
    ):
        present, present_environment = resolve(controls)
        assert absent.compile_digest == present.compile_digest
        assert absent.family_digest == present.family_digest
        for scope in ("compile", "member", "member-output"):
            assert (
                runtime_build_fingerprint(absent, scope=scope)["hash"]
                == (runtime_build_fingerprint(present, scope=scope)["hash"])
            )
        observation = CargoEnvironmentInputs.capture(
            present_environment, controls, local_unit_count=1
        )
        observation.verify(present_environment)
        with pytest.raises(
            ValueError, match=present_environment.rule.key(next(iter(controls)))
        ):
            observation.verify(absent_environment)


def test_profile_cli_inheritance_and_overlapping_prefixes_share_authority(
    plan_root: Path,
) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.release-output]\ninherits="release"\n', encoding="utf-8"
    )
    plan = _plan(
        plan_root,
        env={
            "CARGO_PROFILE_RELEASE_OUTPUT_DEBUG": "0",
            "CARGO_PROFILE_DEV_LTO": "thin",
        },
        args=("--config", 'profile.release-output.inherits="dev"'),
    )
    assert plan.profile_ancestry("release-output") == ("release-output", "dev")
    assert not plan.preserve_debug_for_profile("release-output")
    assert set(plan.profile_environment("release-output")) == {
        "CARGO_PROFILE_RELEASE_OUTPUT_DEBUG",
        "CARGO_PROFILE_DEV_LTO",
    }


@pytest.mark.parametrize(
    ("profile", "ancestry"),
    [
        ("release-output", ("release-output", "release")),
        ("release-size", ("release-size", "release-output", "release")),
        ("wasm-release", ("wasm-release", "release-size", "release-output", "release")),
    ],
)
def test_workspace_shipping_profile_ancestry_is_consumed_by_runtime_identity(
    plan_root: Path, profile: str, ancestry: tuple[str, ...]
) -> None:
    workspace_manifest = Path(__file__).resolve().parents[2] / "Cargo.toml"
    (plan_root / "Cargo.toml").write_bytes(workspace_manifest.read_bytes())
    baseline = _plan(plan_root)
    inherited = _plan(
        plan_root,
        env={
            "CARGO_PROFILE_RELEASE_OUTPUT_DEBUG": "2",
            "CARGO_PROFILE_RELEASE_SIZE_LTO": "off",
        },
    )
    assert inherited.profile_ancestry(profile) == ancestry
    expected = {"CARGO_PROFILE_RELEASE_OUTPUT_DEBUG"}
    if "release-size" in ancestry:
        expected.add("CARGO_PROFILE_RELEASE_SIZE_LTO")
    assert set(inherited.profile_environment(profile)) == expected
    assert inherited.preserve_debug_for_profile(profile)
    assert not baseline.preserve_debug_for_profile(profile)
    assert _runtime_build_environment_identity(inherited, cargo_profile=profile) != (
        _runtime_build_environment_identity(baseline, cargo_profile=profile)
    )


@pytest.mark.parametrize("source", ["manifest", "config", "cli-file", "cli-inline"])
def test_unselected_profile_namespace_does_not_pollute_runtime_identity(
    plan_root: Path, source: str
) -> None:
    manifest = plan_root / "Cargo.toml"
    selected = '[profile.release-output]\ninherits="release"\n'
    sibling = '[profile.release-size]\ninherits="release-output"\n'
    manifest.write_text(
        selected + (sibling if source == "manifest" else ""), encoding="utf-8"
    )
    args: tuple[str, ...] = ()
    if source == "config":
        config = plan_root / ".cargo" / "config.toml"
        config.parent.mkdir(exist_ok=True)
        config.write_text(sibling, encoding="utf-8")
    elif source == "cli-file":
        config = plan_root / "profiles.toml"
        config.write_text(sibling, encoding="utf-8")
        args = ("--config", str(config))
    elif source == "cli-inline":
        args = ("--config", 'profile.release-size.inherits="release-output"')
    selected_env = {
        "CARGO_PROFILE_RELEASE_OUTPUT_DEBUG": "2",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL": "3",
    }
    baseline = _plan(plan_root, args=args, env=selected_env)
    sibling_env = {
        "CARGO_PROFILE_RELEASE_SIZE_LTO": "off",
        "CARGO_PROFILE_RELEASE_SIZE_BUILD_OVERRIDE_DEBUG": "0",
        "CARGO_PROFILE_RELEASE_SIZE_FUTURE_CONTROL": "1",
        "CARGO_PROFILE_BENCH_DEBUG": "1",
    }
    with_sibling = _plan(plan_root, args=args, env=selected_env | sibling_env)
    assert with_sibling.profile_environment("release-output") == selected_env
    assert _runtime_build_environment_identity(
        baseline, cargo_profile="release-output"
    ) == _runtime_build_environment_identity(
        with_sibling, cargo_profile="release-output"
    )
    with pytest.raises(ValueError, match="RELEASE_SIZE_FUTURE_CONTROL"):
        with_sibling.profile_environment("release-size")


@pytest.mark.parametrize(
    "profile", ["release-build-override", "release_build_override"]
)
def test_legal_ancestor_controls_survive_profile_namespace_overlap(
    plan_root: Path, profile: str
) -> None:
    (plan_root / "Cargo.toml").write_text(
        f'[profile.{profile}]\ninherits="dev"\n'
        '[profile.release-debug]\ninherits="dev"\n',
        encoding="utf-8",
    )
    shared = "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL"
    assertions = "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS"
    environment = {shared: "3", assertions: "true"}
    plan = _plan(plan_root, env=environment)
    # These are legal release controls even when unrelated longer profile names
    # exist. The shared key is also the custom profile's own opt-level control.
    assert plan.profile_environment("release") == environment
    assert plan.profile_environment(profile) == {shared: "3"}
    assert plan.profile_environment("dev") == {}
    assert plan.profile_environment("release-debug") == {}
    release_identity = _runtime_build_environment_identity(
        plan, cargo_profile="release"
    )
    custom_identity = _runtime_build_environment_identity(plan, cargo_profile=profile)
    assert release_identity[shared] == custom_identity[shared]
    changed = _plan(plan_root, env=environment | {shared: "2"})
    for selected in ("release", profile):
        assert (
            _runtime_build_environment_identity(changed, cargo_profile=selected)[shared]
            != release_identity[shared]
        )


@pytest.mark.parametrize("profile", ["release-output", "release_output"])
def test_profile_environment_aliases_share_cargo_key(
    plan_root: Path, profile: str
) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.release-output]\ninherits="release"\n'
        '[profile.release_output]\ninherits="dev"\n',
        encoding="utf-8",
    )
    environment = {"CARGO_PROFILE_RELEASE_OUTPUT_DEBUG": "2"}
    plan = _plan(plan_root, env=environment)
    assert plan.profile_environment(profile) == environment
    assert plan.preserve_debug_for_profile(profile)


@pytest.mark.parametrize("rule", list(NameRule))
def test_profile_projection_keeps_cargo_names_under_both_rules(plan_root, rule):
    environment = ProcessEnvironment({"cargo_profile_release_debug": "2"}, rule=rule)
    plan = _plan(plan_root, env=environment)
    assert plan.environment.rule is rule
    assert plan.profile_environment("release") == (
        {"CARGO_PROFILE_RELEASE_DEBUG": "2"} if rule is NameRule.WINDOWS else {}
    )
    assert plan.preserve_debug_for_profile("release") is (rule is NameRule.WINDOWS)
    unknown = replace(
        plan,
        environment=plan.environment.updated(
            {"cargo_profile_release_future_control": "1"}
        ),
    )
    if rule is NameRule.WINDOWS:
        with pytest.raises(ValueError, match="unsupported output-bearing"):
            unknown.profile_environment("release")
    else:
        assert unknown.profile_environment("release") == {}


@pytest.mark.parametrize(
    "control",
    [
        "CARGO_PROFILE_CUSTOM_FUTURE_CONTROL",
        "CARGO_PROFILE_RELEASE_FUTURE_CONTROL",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_FUTURE_CONTROL",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_BUILD_OVERRIDE_DEBUG",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_PANIC",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_LTO",
        "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_RPATH",
    ],
)
def test_unknown_inherited_profile_control_is_diagnosed(
    plan_root: Path, control: str
) -> None:
    (plan_root / "Cargo.toml").write_text(
        '[profile.custom]\ninherits="release"\n', encoding="utf-8"
    )
    with pytest.raises(ValueError, match="unsupported output-bearing"):
        _plan(plan_root, env={control: "1"}).profile_environment("custom")


@pytest.mark.parametrize(
    ("version", "returncode", "supported"),
    [
        ("cargo 1.90.0 (old)\n", 0, False),
        ("cargo 1.91.0 (supported)\n", 0, True),
        ("cargo 1.99.0 (failed probe)\n", 1, False),
        ("not a Cargo version\n", 0, False),
    ],
)
def test_resource_generation_requires_cargo_build_directory_capability(
    plan_root: Path,
    monkeypatch: pytest.MonkeyPatch,
    version: str,
    returncode: int,
    supported: bool,
) -> None:
    original = plans.process_guard.run_completed_command

    def probe(command, **kwargs):
        if tuple(command[1:]) == ("--version",):
            return subprocess.CompletedProcess(command, returncode, version, "")
        return original(command, **kwargs)

    monkeypatch.setattr(plans.process_guard, "run_completed_command", probe)
    if supported:
        plan = _plan(plan_root)
        plan.verify()
        assert plan.environment["CARGO_BUILD_BUILD_DIR"] == str(plan.build_directory)
    else:
        with pytest.raises(ValueError, match="Cargo 1.91 or newer"):
            _plan(plan_root)


def test_cli_tool_selectors_are_pinned_after_original_overrides(
    plan_root: Path,
) -> None:
    plan = _plan(
        plan_root,
        args=(
            "--config",
            'build.rustc="configured-rustc"',
            "--config",
            'build.rustc-wrapper="configured-wrapper"',
            "--",
            "-C",
            "panic=abort",
        ),
    )
    selected = cargo_command_options(plan.command)[1]
    pinned = cargo_argv._pinned_tool_configurations(
        plan.tools,
        plan.wrappers,
        plan.target,
        plan.forced_environment,
        plan.build_directory,
        plan.target_directory,
    )
    assert selected[-len(pinned) :] == tuple(raw for raw, _ in pinned)
    assert plan.command[-3:] == ("--", "-C", "panic=abort")
    # Host absolute paths are live execution inputs, not portable receipt data.
    assert str(plan_root) not in " ".join(plan.partition_command()[0])
    assert plan.configuration_arguments()[-len(pinned) :] == tuple(
        canonical_json_sha256(plans.tomllib.loads(logical)) for _, logical in pinned
    )


def test_rustup_proxy_is_pinned_but_custom_compiler_is_preserved(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, isolated_molt_cache
) -> None:
    from molt import process_guard

    suffix = ".exe" if os.name == "nt" else ""
    proxy = tmp_path / ("rustup" + suffix)
    selector = tmp_path / ("rustc" + suffix)
    compiler = tmp_path / ("selected-rustc" + suffix)
    write_mock_executable(proxy, b"MZrustup")
    write_mock_executable(selector, proxy.read_bytes())
    write_mock_executable(compiler, b"MZcompiler")
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(
            command, 0, stdout=str(compiler) + "\n", stderr=""
        )

    monkeypatch.setattr(process_guard, "run_completed_command", run)
    monkeypatch.setattr(
        rust_toolchain, "resolve_executable", lambda value, **kwargs: Path(value)
    )
    assert (
        rust_toolchain.resolve_rustup_proxy(
            selector, role="rustc", root=tmp_path, env={}
        )
        == compiler
    )
    assert calls == [[str(proxy), "which", "rustc"]]
    selector.write_bytes(b"MZcustom")
    assert (
        rust_toolchain.resolve_rustup_proxy(
            selector, role="rustc", root=tmp_path, env={}
        )
        == selector
    )
    assert len(calls) == 1


@pytest.mark.parametrize("mutation", ["rustc", "resource", "config"])
def test_supplied_manifest_cannot_attest_a_different_live_plan(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    plan = _plan(plan_root)
    content = {
        "tools": {
            item.label.split("/", 1)[1]: {
                "logical_name": item.label.split("/", 1)[1],
                **item.content_record(),
            }
            for item in plan.executable_custody
        },
        "wrappers": {},
        "sysroots": {},
        "effective_target": plan.target,
        "cargo_configuration": plan.configuration_identity(),
        "rust_resources": {
            "host_triple": plan.host_target,
            "selected_target": plan.target,
            "content": plan.rust_resources.content_identity(),
        },
    }
    _verify_plan_toolchain_content(plan, content)
    if mutation == "rustc":
        plan.rustc.write_bytes(b"MZnew compiler")
    elif mutation == "resource":
        (plan_root / "rust-resources" / "libcore.rlib").write_bytes(b"new core")
    else:
        _config(plan_root, '[build]\nrustflags="--cfg changed"\n')
    # Captured projections remain pure. Runtime admission owns the live
    # mutable-input fence before those facts can authorize execution.
    with pytest.raises(ValueError, match="changed"):
        _capture_plan_toolchain(plan)
    replacement = _plan(plan_root)
    with pytest.raises(ValueError, match="differs|differ"):
        _verify_plan_toolchain_content(replacement, content)


@pytest.mark.parametrize("selector", ["extern", "search", "codegen", "linker"])
def test_flag_selected_resources_are_captured_and_fenced(
    plan_root: Path, selector: str
) -> None:
    directory = plan_root / "custom resources"
    directory.mkdir()
    artifact = directory / "resource.bin"
    artifact.write_bytes(b"MZresource")
    flags = {
        "extern": ("--extern", "dep=" + str(artifact)),
        "search": ("-L", "native=" + str(directory)),
        "codegen": ("-Z", "codegen-backend=" + str(artifact)),
        "linker": ("-C", "linker=selected-linker"),
    }[selector]
    plan = _plan(plan_root, env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})
    selected = plan.tools["linker"] if selector == "linker" else artifact
    custody = (
        plan.executable_custody if selector == "linker" else plan.rust_resources.files
    )
    assert any(item.identity.path == selected for item in custody)
    selected.write_bytes(b"changed resource")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


def test_final_crate_resource_override_retains_dependency_custody(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first = plan_root / "dependency-sysroot"
    first.mkdir()
    seen = []

    def roots(*args, **kwargs):
        seen.append(kwargs["flag_lanes"])
        return ()

    monkeypatch.setattr(plans, "_rust_resource_roots", roots)
    plan = _plan(
        plan_root,
        env={
            "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(
                ("--sysroot", str(first), "-C", "linker=dependency-linker")
            )
        },
        args=("--", "-C", "linker=final-linker"),
    )
    assert seen == [
        (
            plan.rustflags,
            (*plan.rustflags, "-C", "linker=" + str(plan.tools["final_linker"])),
        )
    ]
    assert plan.tools["linker"].name == "dependency-linker"
    assert plan.tools["final_linker"].name == "final-linker"
    custody = {item.label: item.entrypoint for item in plan.executable_custody}
    assert custody["tool/linker"] == plan.tools["linker"]
    assert custody["tool/final_linker"] == plan.tools["final_linker"]
    assert not plan.rust_resources.files
    assert not any(label.endswith("/linker") for label, _ in plan.resource_paths)
    assert first in dict(plan.resource_paths).values()


def test_duplicate_sysroot_across_dependency_and_final_crate_is_diagnosed(
    plan_root: Path,
) -> None:
    sysroot = plan_root / "sysroot"
    sysroot.mkdir()
    with pytest.raises(ValueError, match="duplicate --sysroot"):
        _plan(
            plan_root,
            env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(("--sysroot", str(sysroot)))},
            args=("--", "--sysroot", str(sysroot)),
        )


@pytest.mark.parametrize("role", ["linker", "final_linker"])
def test_rust_linker_executable_mutation_is_fenced(plan_root: Path, role: str) -> None:
    plan = _plan(
        plan_root,
        env={"RUSTFLAGS": "-C linker=dependency-linker"},
        args=("--", "-C", "linker=final-linker"),
    )
    selected = plan.tools[role]
    before = selected.stat()
    selected.write_bytes(b"MZ" + b"x" * (before.st_size - 2))
    os.utime(selected, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("mutation", ["command", "flags", "missing-tool"])
def test_rust_linker_selector_must_reconcile_with_executable_custody(
    plan_root: Path, mutation: str
) -> None:
    plan = _plan(
        plan_root,
        env={"RUSTFLAGS": "-C linker=dependency-linker"},
        args=("--", "-C", "linker=final-linker"),
    )
    if mutation == "command":
        # Replace the admitted selector in place. Appending a duplicate is
        # already rejected by canonical flag admission and never reaches the
        # independent executable-custody reconciliation tested here.
        original = "linker=" + str(plan.tools["final_linker"])
        assert plan.command.count(original) == 1
        plan = replace(
            plan,
            command=tuple(
                "linker=" + str(plan.tools["linker"]) if token == original else token
                for token in plan.command
            ),
        )
    elif mutation == "flags":
        flags = ("-C", "linker=" + str(plan.tools["final_linker"]))
        plan = replace(
            plan,
            rustflags=flags,
            environment=plan.environment.updated(
                {"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)}
            ),
        )
    else:
        plan = replace(
            plan,
            tools=MappingProxyType(
                {
                    role: path
                    for role, path in plan.tools.items()
                    if role != "final_linker"
                }
            ),
        )
    with pytest.raises(ValueError, match="linker selection differs"):
        plan.verify()


def test_final_crate_inherits_dependency_linker_without_duplicate_role(
    plan_root: Path,
) -> None:
    plan = _plan(plan_root, env={"RUSTFLAGS": "-C linker=dependency-linker"})
    assert plan.tools["linker"].name == "dependency-linker"
    assert "final_linker" not in plan.tools
    assert not any(
        item.label == "tool/final_linker" for item in plan.executable_custody
    )
    assert not any(label.endswith("/linker") for label, _ in plan.resource_paths)


def test_final_crate_linker_last_selector_wins(plan_root: Path) -> None:
    plan = _plan(
        plan_root,
        args=("--", "-Clinker=missing/linker", "-C", "linker=final-linker"),
    )
    assert plan.tools["final_linker"].name == "final-linker"
    assert "-Clinker=missing/linker" not in plan.command


@pytest.mark.parametrize("final", [False, True])
@pytest.mark.parametrize("content", [b"#!/bin/sh\nexec ld\n", b"@echo off\nld.exe\n"])
def test_rust_linker_scripts_cannot_enter_executable_custody(
    plan_root: Path, final: bool, content: bytes
) -> None:
    linker = plan_root / "script-linker"
    write_mock_executable(linker, content)
    selector = ("-C", "linker=" + str(linker))
    with pytest.raises(ValueError, match="native executable, not a script"):
        _plan(
            plan_root,
            env={} if final else {"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(selector)},
            args=("--", *selector) if final else (),
        )


def test_final_linker_is_an_exact_portable_toolchain_role() -> None:
    value = runtime_build_identity("shared").to_dict()
    tools = value["payload"]["family"]["compile"]["toolchain"]["tools"]
    tools["final_linker"] = {**tools["rustc"], "logical_name": "final_linker"}
    _reseal(value)
    assert RuntimeBuildIdentity.from_dict(value).to_dict() == value


def test_last_codegen_selector_wins_without_opening_shadowed_path(
    plan_root: Path,
) -> None:
    selected = plan_root / "backend.dll"
    selected.write_bytes(b"MZbackend")
    plan = _plan(
        plan_root,
        env={
            "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(
                (
                    "-Zcodegen-backend=missing/backend.dll",
                    "-Z",
                    "codegen-backend=" + str(selected),
                )
            )
        },
    )
    assert plan.rustflags == _with_remaps(
        plan_root, ("-Z", "codegen-backend=" + str(selected))
    )
    assert any(item.identity.path == selected for item in plan.rust_resources.files)


@pytest.mark.parametrize(
    "flags,diagnostic",
    [
        (("--sysroot", "a", "--sysroot", "b"), "duplicate --sysroot"),
        (("--extern", "dep"), "explicit crate=artifact"),
        (("-L", "unknown=somewhere"), "resource is missing"),
        (("-Z", "codegen-backend="), "requires a resource selector"),
        (("@arguments.rsp",), "parsed argument custody"),
        (("-o", "@arguments.rsp"), "parsed argument custody"),
        (("--", "@arguments.rsp"), "parsed argument custody"),
    ],
)
def test_unresolved_rust_resource_selector_is_diagnosed(
    plan_root: Path, flags: tuple[str, ...], diagnostic: str
) -> None:
    with pytest.raises(ValueError, match=diagnostic):
        _plan(plan_root, env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})


@pytest.mark.parametrize("wrapped", [False, True])
def test_selected_sysroot_supplies_target_resources_not_default(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, wrapped: bool
) -> None:
    compiler = tmp_path / "rustc"
    write_mock_executable(compiler, b"MZcompiler")
    installed, selected = tmp_path / "installed", tmp_path / "selected"
    (installed / "lib" / "rustlib" / "host" / "lib").mkdir(parents=True)
    selected_lib = selected / "lib" / "rustlib" / "target" / "lib"
    selected_lib.mkdir(parents=True)
    wrappers = {}
    if wrapped:
        for role in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            wrapper = tmp_path / role
            write_mock_executable(wrapper, b"MZwrapper")
            wrappers[role] = wrapper
    expected_prefix = [*wrappers.values(), compiler]

    def probe(command, **kwargs):
        is_wrapped = wrapped and Path(command[0]) == expected_prefix[0]
        prefix = expected_prefix if is_wrapped else [compiler]
        assert [Path(arg) for arg in command[: len(prefix)]] == prefix
        args = command[len(prefix) :]
        assert args[:4] == ["-", "--crate-name", "___", "--print=file-names"]
        assert kwargs["input"] == ""
        # The wrapper redirects crate queries even without an explicit
        # --sysroot. A shortened resource query would incorrectly pick installed.
        value = (
            selected
            if "--target" in args and (is_wrapped or "--sysroot" in args)
            else installed
        )
        return subprocess.CompletedProcess(
            command, 0, stdout=_metadata_stdout(value), stderr=""
        )

    monkeypatch.setattr(plans.process_guard, "run_completed_command", probe)
    roots = plans._rust_resource_roots(
        compiler,
        root=tmp_path,
        env={},
        target="target",
        host="host",
        flag_lanes=((),) if wrapped else (("--sysroot", str(selected)),),
        wrappers=wrappers,
        target_argument="target",
    )
    target_roots = [
        entry.path for entry in roots if entry.label.startswith("rust/target-libdir/")
    ]
    assert target_roots == [selected_lib]
    assert (
        next(entry.path for entry in roots if entry.label == "rust/driver")
        == installed / "lib"
    )


@pytest.mark.parametrize("mutated", [None, "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"])
def test_cfg_probe_uses_and_fences_nested_cargo_wrappers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutated: str | None
) -> None:
    rustc = tmp_path / "rustc"
    write_mock_executable(rustc, b"MZcompiler")
    wrappers = {}
    for role in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        path = tmp_path / role
        write_mock_executable(path, b"MZwrapper")
        wrappers[role] = path

    def run(command, **kwargs):
        prefix = [*wrappers.values(), rustc]
        assert [Path(arg) for arg in command[: len(prefix)]] == prefix
        assert tuple(
            command[len(prefix) :]
        ) == cargo_target_cfg.cargo_target_query_arguments(
            "target", ("--cfg", "selected")
        )
        if mutated is not None:
            wrappers[mutated].write_bytes(b"MZchanged")
        return subprocess.CompletedProcess(command, 0, _metadata_stdout(tmp_path), "")

    monkeypatch.setattr(plans.process_guard, "run_completed_command", run)
    if mutated is None:
        assert ("selected", None) in rustc_probes.rust_target_metadata(
            rustc,
            "target",
            ("--cfg", "selected"),
            root=tmp_path,
            env={},
            wrappers=wrappers,
        ).cfg
    else:
        with pytest.raises((ValueError, OSError), match="changed"):
            rustc_probes.rust_target_metadata(
                rustc,
                "target",
                ("--cfg", "selected"),
                root=tmp_path,
                env={},
                wrappers=wrappers,
            )


@pytest.mark.parametrize("kind", ["native", "shared", "reloc", "toolchain"])
def test_validated_runtime_receipts_reuse_only_exact_immutable_instances(
    kind: str,
) -> None:
    receipt = (
        native_runtime_staticlib_identity()
        if kind == "native"
        else runtime_build_identity("shared" if kind == "toolchain" else kind)
    )
    if kind == "toolchain":
        receipt = receipt.toolchain_manifest
    cls = type(receipt)
    assert cls.from_dict(receipt) is receipt
    detached = receipt.to_dict()
    admitted = cls.from_dict(detached)
    detached["payload"].clear()
    assert admitted == receipt
    with pytest.raises(ValueError):
        cls.from_dict(detached)
    assert dict(receipt) == receipt.to_dict()
    projected = receipt["payload"]
    projected.clear()
    assert receipt == admitted
    with pytest.raises(KeyError):
        receipt["not-a-field"]


@pytest.mark.parametrize("kind", ["build", "toolchain"])
def test_runtime_receipt_subclasses_cannot_bypass_admission(kind: str) -> None:
    receipt = runtime_build_identity("shared")
    if kind == "toolchain":
        receipt = receipt.toolchain_manifest
    cls = type(receipt)

    class UnvalidatedReceipt(cls):
        def __post_init__(self) -> None:
            pass

    values = {key: getattr(receipt, key) for key in ("digest", "payload")}
    if kind == "build":
        values.update(
            compile_digest=receipt.compile_digest, family_digest=receipt.family_digest
        )
    values["digest"] = "0" * 64
    unvalidated = UnvalidatedReceipt(**values)
    with pytest.raises(ValueError, match="digest"):
        cls.from_dict(unvalidated)


@pytest.mark.parametrize("kind", ["shared", "reloc", "staticlib"])
def test_expected_runtime_identity_admits_only_its_canonical_content(kind) -> None:
    expected = (
        native_runtime_staticlib_identity()
        if kind == "staticlib"
        else runtime_build_identity(kind)
    )
    wire = expected.to_dict()
    assert RuntimeBuildIdentity.from_dict(wire, expected=expected) is expected
    assert RuntimeBuildIdentity.from_dict(expected, expected=expected) is expected
    # Admission retains the trusted owner, never the caller's wire backing.
    wire["payload"].clear()
    with pytest.raises(ValueError, match="shape is invalid"):
        RuntimeBuildIdentity.from_dict(wire, expected=expected)
    assert RuntimeBuildIdentity.from_dict(expected.to_dict()) == expected


@pytest.mark.parametrize(
    "mutation",
    ["digest", "compile_digest", "family_digest", "extra", "resealed", "bool"],
)
def test_expected_runtime_identity_rejects_changed_or_resealed_claims(mutation) -> None:
    expected = runtime_build_identity("shared")
    wire = expected.to_dict()
    if mutation in {"digest", "compile_digest", "family_digest"}:
        wire[mutation] = "0" * 64
    elif mutation == "extra":
        wire["extra"] = None
    else:
        member = wire["payload"]["family"]["members"]["shared"]
        if mutation == "bool":
            # Python equality conflates True with 1; canonical JSON does not.
            member["publication"]["preserve_debug"] = int(
                member["publication"]["preserve_debug"]
            )
        else:
            member["publication"]["transform"] = "different-publication"
            _reseal(wire)
    with pytest.raises(ValueError):
        RuntimeBuildIdentity.from_dict(wire, expected=expected)


def test_expected_runtime_identity_reuses_its_admitted_facts(monkeypatch) -> None:
    from molt.cli import runtime_identity_schema as schema

    expected = runtime_build_identity("shared")
    wire = expected.to_dict()

    def no_second_semantic_admission(*_args, **_kwargs):
        raise AssertionError("trusted identity was semantically re-admitted")

    monkeypatch.setattr(
        schema, "_validated_runtime_build_payload", no_second_semantic_admission
    )
    assert RuntimeBuildIdentity.from_dict(wire, expected=expected) is expected


def test_owned_frozen_json_derivations_preserve_wire_and_isolation(monkeypatch) -> None:
    from molt.cli import runtime_identity_schema as schema
    from molt.exact_json import canonical_json_bytes

    mutable = {"nested": {"values": [1, 1.0, -0.0, True, None, "\u2603"]}}
    expected_bytes = canonical_json_bytes(mutable)
    expected_digest = canonical_json_sha256(mutable)
    proxy = MappingProxyType(mutable)
    frozen = schema._freeze_json(proxy)
    assert schema._canonical_json(frozen).encode("utf-8") == expected_bytes
    encoded = []
    original = schema.canonical_json_sha256

    def counted(value, **kwargs):
        encoded.append(value)
        return original(value, **kwargs)

    monkeypatch.setattr(schema, "canonical_json_sha256", counted)
    assert schema._digest(frozen) == expected_digest
    assert schema._digest(frozen) == expected_digest
    assert len(encoded) == 1
    mutable["nested"]["values"][0] = 9
    assert schema._digest(frozen) == expected_digest
    assert schema._digest(proxy) == canonical_json_sha256(mutable)
    assert schema._digest(proxy) != expected_digest
    detached = schema._thaw_json(frozen)
    detached["nested"]["values"].clear()
    assert schema._canonical_json(frozen).encode("utf-8") == expected_bytes
    with pytest.raises(TypeError):
        frozen["nested"]["values"][0] = 8
    with pytest.raises(TypeError):
        frozen["nested"]["extra"] = 8


def test_frozen_json_subclass_cannot_claim_owned_immutability() -> None:
    from molt.cli import runtime_identity_schema as schema

    class Unowned(schema._FrozenJsonObject):
        def __post_init__(self):
            pass

    source = {"nested": {"value": 1}}
    unowned = Unowned(source)
    frozen = schema._freeze_json(unowned)
    source["nested"]["value"] = 2
    assert schema._thaw_json(frozen) == {"nested": {"value": 1}}
    assert schema._digest(unowned) == canonical_json_sha256(source)


@pytest.mark.parametrize(
    "invalid", [{1: "key"}, {"x": float("nan")}, {"x": float("inf")}, {"x": object()}]
)
def test_owned_frozen_json_rejects_inexact_values(invalid) -> None:
    from molt.cli import runtime_identity_schema as schema

    with pytest.raises((TypeError, ValueError)):
        schema._freeze_json(invalid)


@pytest.mark.parametrize("kind", ["str", "int", "float", "key"])
def test_owned_json_rejects_scalar_subclasses_and_forged_digest(kind) -> None:
    from molt.cli import runtime_identity_schema as schema

    conversions = []

    class MutableStr(str):
        def __str__(self):
            conversions.append("str")
            return "changed" if self.changed else "value"

    class MutableInt(int):
        def __int__(self):
            conversions.append("int")
            return -1 if self.changed else 1

    class MutableFloat(float):
        def __float__(self):
            conversions.append("float")
            return float("nan") if self.changed else 1.0

    class Unowned(schema._FrozenJsonObject):
        def __post_init__(self):
            pass

    scalar = {
        "str": lambda: MutableStr("value"),
        "int": lambda: MutableInt(1),
        "float": lambda: MutableFloat(1.0),
        "key": lambda: MutableStr("key"),
    }[kind]()
    scalar.changed = False
    source = {scalar: 1} if kind == "key" else {"value": scalar}
    foreign = Unowned(source)
    object.__setattr__(foreign, "_cached_digest", "0" * 64)
    object.__setattr__(foreign, "_validated_schemas", frozenset({("forged",)}))
    for changed in (False, True):
        scalar.changed = changed
        for value in (source, foreign):
            with pytest.raises(TypeError):
                schema._freeze_json(value)
            with pytest.raises(TypeError):
                schema._digest(value)
    assert conversions == []


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_runtime_family_shares_immutable_capture_and_exports_detached_wire(
    monkeypatch, target
) -> None:
    from molt.cli import runtime_build_identity as builder
    from molt.cli import runtime_identity_schema as schema

    original = (
        native_runtime_staticlib_identity()
        if target == "native"
        else runtime_build_identity("shared")
    )
    wire = original.to_dict()
    validations = []
    validate = schema._validated_executable_content_identity

    def counted(value, *, logical_name):
        validations.append(logical_name)
        return validate(value, logical_name=logical_name)

    monkeypatch.setattr(schema, "_validated_executable_content_identity", counted)
    captured = RuntimeBuildIdentity.from_dict(wire)
    family = captured.payload["family"]
    compilation = family["compile"]
    manifest = captured.toolchain_manifest
    tools = compilation["toolchain"]["tools"]
    assert "build_python" not in tools
    initial_validations = tuple(validations)
    assert set(initial_validations) == set(tools)
    assert len(initial_validations) == len(tools)
    members = tuple(
        schema.RuntimeBuildMemberPlan(
            kind=kind,
            resolved_rustflags=tuple(member["resolved_rustflags"]),
            link_args=tuple(member["link_args"]),
            publication=schema.RuntimePublicationAction.from_dict(
                member["publication"]
            ),
        )
        for kind, member in family["members"].items()
    )
    identities = builder._resolve_runtime_build_family_identities(
        sources=compilation["sources"],
        toolchain_manifest=manifest,
        target_triple=compilation["common_config"]["target_triple"],
        common_config=compilation["common_config"],
        members=members,
    )
    assert len(identities) == (1 if target == "native" else 2)
    assert all(
        identity.payload["family"] is identities[0].payload["family"]
        for identity in identities
    )
    assert (
        identities[0].payload["family"]["compile"]["toolchain"]
        is manifest.payload["toolchain"]
    )
    for identity in identities:
        assert (
            identity.toolchain_manifest.payload["toolchain"] is compilation["toolchain"]
        )
        wire = identity.to_dict()
        assert canonical_json_sha256(wire["payload"]) == identity.digest
        assert (
            canonical_json_sha256(wire["payload"]["family"]) == identity.family_digest
        )
        assert (
            canonical_json_sha256(wire["payload"]["family"]["compile"])
            == identity.compile_digest
        )
        wire["payload"]["family"]["compile"]["toolchain"].clear()
        assert identity.payload["family"]["compile"]["toolchain"]
    # Projections share one admitted immutable graph. A new wire graph still
    # receives its own complete executable content validation.
    assert tuple(validations) == initial_validations
    detached = original.to_dict()
    assert RuntimeBuildIdentity.from_dict(detached) == original
    assert tuple(validations) == initial_validations * 2


@pytest.mark.parametrize("origin", ["mutable", "proxy", "subclass"])
def test_schema_admission_does_not_trust_foreign_success_markers(origin):
    from molt.cli import runtime_identity_schema as schema
    from tests.runtime_build_identity_helper import runtime_toolchain_content_manifest

    source = runtime_toolchain_content_manifest().to_dict()["payload"]["toolchain"]
    owned = schema._freeze_json(source)
    schema._validated_runtime_toolchain_content(owned, target_triple="native")

    class Unowned(schema._FrozenJsonObject):
        def __post_init__(self):
            pass

    value = source
    if origin == "proxy":
        value = MappingProxyType(source)
    elif origin == "subclass":
        value = Unowned(source)
        object.__setattr__(value, "_validated_schemas", owned._validated_schemas)
    assert (
        schema._validated_runtime_toolchain_content(value, target_triple="native")
        is value
    )
    source["tools"]["rustc"]["size"] = -1
    with pytest.raises(ValueError, match="runtime tool rustc identity is invalid"):
        schema._validated_runtime_toolchain_content(value, target_triple="native")
    assert (
        schema._validated_runtime_toolchain_content(owned, target_triple="native")
        is owned
    )
    assert owned["tools"]["rustc"]["size"] >= 0


def test_schema_admission_never_caches_failure_or_confuses_schemas(monkeypatch):
    from molt.cli import runtime_identity_schema as schema
    from tests.runtime_build_identity_helper import runtime_toolchain_content_manifest

    source = runtime_toolchain_content_manifest().to_dict()["payload"]["toolchain"]
    source["tools"]["rustc"]["size"] = -1
    invalid = schema._freeze_json(source)
    validations = []
    validate = schema._validated_executable_content_identity

    def counted(value, *, logical_name):
        if logical_name == "rustc":
            validations.append(value)
        return validate(value, logical_name=logical_name)

    monkeypatch.setattr(schema, "_validated_executable_content_identity", counted)
    for _ in range(2):
        with pytest.raises(ValueError, match="runtime tool rustc identity is invalid"):
            schema._validated_runtime_toolchain_content(invalid, target_triple="native")
    assert len(validations) == 2
    valid = schema._freeze_json(
        runtime_toolchain_content_manifest().to_dict()["payload"]["toolchain"]
    )
    schema._validated_runtime_toolchain_content(valid, target_triple="native")
    with pytest.raises(ValueError, match="manifest content shape is invalid"):
        schema._validated_toolchain_manifest_payload(valid)


def test_toolchain_admission_keeps_target_and_outer_digest_boundaries() -> None:
    from molt.cli import runtime_identity_schema as schema

    identity = native_runtime_staticlib_identity()
    manifest = identity.toolchain_manifest
    toolchain = manifest.payload["toolchain"]
    assert (
        schema._validated_runtime_toolchain_content(toolchain, target_triple="native")
        is toolchain
    )
    assert (
        schema._validated_runtime_toolchain_content(
            toolchain, target_triple=toolchain["effective_target"]
        )
        is toolchain
    )
    with pytest.raises(ValueError, match="Rust target identity is invalid"):
        schema._validated_runtime_toolchain_content(
            toolchain, target_triple="wasm32-wasip1"
        )
    # Valid descendants do not authorize a new claimed digest for their parent.
    with pytest.raises(ValueError, match="manifest digest is invalid"):
        schema.RuntimeToolchainContentManifest("0" * 64, manifest.payload)
    with pytest.raises(ValueError, match="build identity digest is invalid"):
        schema.RuntimeBuildIdentity(
            "0" * 64, identity.compile_digest, identity.family_digest, identity.payload
        )


def test_receipt_admission_rejects_scalar_subclass_claims_and_targets() -> None:
    from molt.cli import runtime_identity_schema as schema

    class MutableClaim(str):
        changed = False
        __hash__ = str.__hash__

        def __eq__(self, other):
            return self.changed or str.__eq__(self, other)

    identity = native_runtime_staticlib_identity()
    manifest = identity.toolchain_manifest
    for changed in (False, True):
        MutableClaim.changed = changed
        for field in ("digest", "compile_digest", "family_digest"):
            claim = MutableClaim(getattr(identity, field))
            with pytest.raises(ValueError, match="digest shape is invalid"):
                replace(identity, **{field: claim})
            wire = identity.to_dict()
            wire[field] = claim
            with pytest.raises(ValueError, match="identity is incomplete"):
                schema.RuntimeBuildIdentity.from_dict(wire, expected=identity)
        with pytest.raises(ValueError, match="manifest digest is invalid"):
            replace(manifest, digest=MutableClaim(manifest.digest))
        with pytest.raises(ValueError, match="toolchain target is invalid"):
            schema._validated_runtime_toolchain_content(
                manifest.payload["toolchain"], target_triple=MutableClaim("native")
            )
        for receipt in (identity, manifest):
            wire = receipt.to_dict()
            wire["schema"] = MutableClaim(wire["schema"])
            with pytest.raises(ValueError, match="schema is invalid"):
                type(receipt).from_dict(wire)


@pytest.mark.parametrize("origin", ["capture", "wire"])
def test_toolchain_manifest_owns_one_canonical_payload(monkeypatch, origin) -> None:
    from molt.cli import runtime_identity_schema as schema

    original = native_runtime_staticlib_identity().toolchain_manifest
    wire = original.to_dict()
    encoded = []
    encode = schema.canonical_json_sha256

    def counted(value, **kwargs):
        if isinstance(value, Mapping) and set(value) == {
            "target_triple",
            "toolchain",
        }:
            encoded.append(value)
        return encode(value, **kwargs)

    monkeypatch.setattr(schema, "canonical_json_sha256", counted)
    admitted = (
        schema.RuntimeToolchainContentManifest.from_payload(wire["payload"])
        if origin == "capture"
        else schema.RuntimeToolchainContentManifest.from_dict(wire)
    )
    assert admitted.to_dict() == wire
    assert len(encoded) == 1
    assert encoded[0] is admitted.payload
    wire["payload"]["toolchain"]["tools"].clear()
    assert admitted == original
    with pytest.raises(ValueError, match="toolchain tools are invalid"):
        schema.RuntimeToolchainContentManifest.from_payload(wire["payload"])


def test_pure_freestanding_rust_does_not_select_c_sdk(plan_root, monkeypatch):
    monkeypatch.setattr(
        plans,
        "apply_provisioned_wasm_toolchain",
        lambda *a, **k: pytest.fail("freestanding Rust selected a C SDK"),
    )
    plan = _plan(
        plan_root,
        target="wasm32-unknown-unknown",
        args=("--target", "wasm32-unknown-unknown"),
    )
    assert plan.wasi_c_abi is None
    assert "MOLT_WASI_C_ABI_PLAN" not in plan.environment


def test_sdk_members_use_receipt_without_mutable_resource_capture(
    plan_root, monkeypatch
):
    from molt import toolchain_identity

    captured = []
    original = toolchain_identity.FileDigestCache.identity

    def identity(self, path, *, label):
        captured.append(Path(path).resolve())
        return original(self, path, label=label)

    monkeypatch.setattr(toolchain_identity.FileDigestCache, "identity", identity)
    plan = _plan(plan_root, target="wasm32-wasip1", args=("--target", "wasm32-wasip1"))
    assert plan.wasi_c_abi is not None
    # The spy sees every mutable resource capture, so its silence is evidence.
    assert captured
    for _role, path, size, digest in plan.wasi_c_abi.files:
        assert captured.count(Path(path).resolve()) == 0
        assert not any(item.entrypoint == path for item in plan.rust_resources.files)
        fact = plan.wasi_sdk.facts["members"][_role]
        assert (fact["size"], fact["sha256"]) == (size, digest)


@pytest.mark.parametrize(
    "override",
    [None, "foreign-linker", "driver-flavor", "unstable-flavor", "self-contained"],
)
def test_explicit_freestanding_c_provider_uses_selected_raw_linker(
    plan_root, override, readonly_wasi_sdk
):
    from molt import llvm_toolchain

    installation = readonly_wasi_sdk()
    c_abi = llvm_toolchain.wasi_c_abi_plan(installation)
    environment = {"MOLT_WASI_C_ABI_PLAN": c_abi.encode()}
    if override == "foreign-linker":
        environment["CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER"] = "foreign-linker"
    elif override == "driver-flavor":
        environment["RUSTFLAGS"] = "-Clinker-flavor=wasm-lld-cc"
    elif override == "unstable-flavor":
        environment["RUSTFLAGS"] = "-Clinker-flavor=wasm-lld"
    elif override == "self-contained":
        environment["RUSTFLAGS"] = "-Clink-self-contained=yes"

    def resolve():
        return _plan(
            plan_root,
            target="wasm32-unknown-unknown",
            args=("--target", "wasm32-unknown-unknown"),
            env=environment,
        )

    if override is not None:
        with pytest.raises(
            ValueError,
            match="selected SDK raw linker|differs from the selected WASI SDK|conflicts with linker-flavor|conflicts with link-self-contained",
        ):
            resolve()
    else:
        plan = resolve()
        assert plan.wasi_c_abi == c_abi
        assert plan.tools["linker"] == c_abi.linker
        assert plan.rustflags == (
            "-L",
            "native=" + str(c_abi.path("libc").parent),
            "-L",
            "native=" + str(c_abi.path("compiler_rt").parent),
            "-C",
            "link-self-contained=no",
            "-C",
            "linker-flavor=wasm-ld",
            f"--remap-path-prefix={plan_root}=/molt/source",
            f"--remap-path-prefix={plan_root / 'target'}=/molt/build",
        )
        assert plan.environment["CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER"] == str(
            c_abi.linker
        )
        plan.verify()


@pytest.mark.parametrize("source", ["target", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"])
def test_selected_sdk_search_context_precedes_user_roots_without_recapture(
    plan_root, monkeypatch, source, readonly_wasi_sdk
):
    foreign = plan_root / "foreign-libraries"
    foreign.mkdir()
    (foreign / "libc.a").write_bytes(b"foreign libc must not shadow selected SDK")
    flags = ("-L", "native=" + str(foreign), "--cfg", "user_preserved")
    key = "CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS" if source == "target" else source
    # Each variable carries the flags as Cargo reads that variable.
    rule = {
        "target": FlagStringRule.CARGO_LIST,
        "RUSTFLAGS": FlagStringRule.CARGO_RUSTFLAGS,
        "CARGO_ENCODED_RUSTFLAGS": FlagStringRule.CARGO_ENCODED,
    }[source]
    value = rule.encode(flags, variable=key)
    installation = readonly_wasi_sdk()
    captured = []
    original = resource_custody.CargoResourceRoot.walk

    def walk(resource):
        assert not resource.path.is_relative_to(installation.sdk), (
            "managed SDK recursively recaptured"
        )
        captured.append(resource.path)
        return original(resource)

    monkeypatch.setattr(resource_custody.CargoResourceRoot, "walk", walk)
    plan = _plan(
        plan_root,
        target="wasm32-wasip1",
        args=("--target", "wasm32-wasip1"),
        env={key: value},
    )
    libc_dir = plan.wasi_c_abi.path("libc").parent
    builtins_dir = plan.wasi_c_abi.path("compiler_rt").parent
    assert plan.rustflags[:6] == (
        "-L",
        "native=" + str(libc_dir),
        "-L",
        "native=" + str(builtins_dir),
        "-L",
        "native=" + str(foreign),
    )
    assert "user_preserved" in plan.rustflags
    assert foreign in captured
    assert {
        path for label, path in plan.logical_paths if label.startswith("wasi/search/")
    } == {libc_dir, builtins_dir}
    plan.verify()
    (foreign / "libc.a").write_bytes(b"mutated user input")
    with pytest.raises(ValueError):
        plan.verify()


def test_sdk_search_context_cannot_be_removed_from_retained_plan(plan_root):
    from dataclasses import replace

    plan = _plan(plan_root, target="wasm32-wasip1", args=("--target", "wasm32-wasip1"))
    flags = plan.rustflags[4:]
    damaged = replace(
        plan,
        rustflags=flags,
        environment=plan.environment.updated(
            {"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)}
        ),
    )
    with pytest.raises(ValueError, match="admitted target context"):
        damaged.verify()


def test_windows_environment_names_keep_target_c_selectors_through_the_plan(
    plan_root, readonly_wasi_sdk
):
    # Windows preserves CC_wasm32-wasip1 and selects every case variant.
    # Hosted plans once lost that rule when pinning their build directory,
    # and verify() refused their C tools.
    windows = ProcessEnvironmentBuilder(
        {
            "CARGO_HOME": str(plan_root / "cargo-home"),
            "WASI_SDK_PATH": str(readonly_wasi_sdk().sdk),
        },
        rule=NameRule.WINDOWS,
    )
    plan = plans.resolve_runtime_cargo_plan(
        plan_root,
        env=windows,
        cargo_command=("cargo", "rustc", "--target", "wasm32-wasip1"),
        requested_target="wasm32-wasip1",
        host_target="x86_64-unknown-linux-gnu",
    )
    assert plan.environment.rule is NameRule.WINDOWS
    assert "CC_wasm32-wasip1" in set(plan.environment)
    assert "CC_WASM32-WASIP1" in plan.environment
    for role in ("cc", "cxx", "ar", "ranlib"):
        selector = f"{role.upper()}_wasm32-wasip1"
        assert plan.environment[selector] == os.fspath(plan.tools[role])
    assert plan.environment["CARGO_BUILD_BUILD_DIR"] == os.fspath(plan.build_directory)
    plan.verify()


def _private_sdk(root: Path):
    root.mkdir()
    return provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(root))


def test_sdk_paths_that_shell_quoting_changes_reach_rustc_unchanged(
    plan_root, tmp_path
):
    # The SDK projection once shell-quoted CARGO_TARGET_*_RUSTFLAGS, which
    # Cargo splits at whitespace and never unquotes: every Windows SDK path (a
    # backslash) reached rustc as "'native=C:\...'" and the plan refused it.
    name = "sdk'quote" if os.name == "nt" else "sdk'quote\\back"
    root = (tmp_path / name).resolve(strict=False)
    sdk = _private_sdk(root)
    projected = project_wasm_toolchain_environment(
        sdk, environ={}, rust_target="wasm32-wasip1"
    )
    flags = FlagStringRule.CARGO_LIST.decode(
        projected["CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS"]
    )
    plan = _plan(
        plan_root,
        target="wasm32-wasip1",
        env={"WASI_SDK_PATH": str(sdk.sdk)},
        args=("--target", "wasm32-wasip1"),
    )
    for vector in (flags, plan.rustflags):
        searched = [
            Path(operand.removeprefix("native="))
            for option, operand in zip(vector, vector[1:], strict=False)
            if option == "-L" and operand.startswith("native=")
        ]
        assert searched
        assert all(path.is_dir() and path.is_relative_to(root) for path in searched)
    plan.verify()


def test_sdk_path_cargo_cannot_carry_fails_before_any_build(plan_root, tmp_path):
    sdk = _private_sdk(tmp_path / "sdk with space")
    with pytest.raises(
        FlagStringError, match="CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS cannot carry"
    ):
        project_wasm_toolchain_environment(sdk, environ={}, rust_target="wasm32-wasip1")


def test_cargo_environment_updates_keep_their_name_rule_and_never_mutate():
    windows = ProcessEnvironment({"Path": "a"}, rule=NameRule.WINDOWS)
    updated = windows.updated({"cc_wasm32-wasip1": "clang"})
    assert dict(updated) == {"Path": "a", "cc_wasm32-wasip1": "clang"}
    assert updated["CC_wasm32-wasip1"] == "clang" and updated.rule is NameRule.WINDOWS
    assert dict(windows) == {"Path": "a"}
    posix = ProcessEnvironment({"CC": "gcc"}, rule=NameRule.EXACT)
    assert dict(posix.updated({"cc": "lowercase"})) == {"CC": "gcc", "cc": "lowercase"}
    assert posix.get("Cc") is None


def test_a_plan_refuses_an_environment_without_its_name_rule(plan_root):
    plan = _plan(plan_root)
    with pytest.raises(TypeError, match="must be a ProcessEnvironment"):
        replace(plan, environment=MappingProxyType(dict(plan.environment)))


@pytest.mark.parametrize("simd,freestanding", [(True, False), (False, True)])
@pytest.mark.parametrize(
    "flag_origin", ["target", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]
)
def test_combined_runtime_plan_applies_target_policy_after_cargo_features(
    plan_root, simd, freestanding, flag_origin, readonly_wasi_sdk
):
    from molt.cli import runtime_wasm_build_spec as specs

    _config(
        plan_root,
        '[target.wasm32-wasip1]\nrustflags=["-C","target-feature=-reference-types"]\n',
    )
    sdk = readonly_wasi_sdk()
    environment = {
        "CARGO_HOME": str(plan_root / "cargo-home"),
        "WASI_SDK_PATH": str(sdk.sdk),
        "MOLT_BUILD_PYTHON": sys.executable,
    }
    if flag_origin != "target":
        separator = "\x1f" if flag_origin == "CARGO_ENCODED_RUSTFLAGS" else " "
        environment[flag_origin] = separator.join(
            ("-C", "target-feature=-reference-types")
        )
    shared = specs._RuntimeWasmBuildSpec(
        publication_name="runtime.wasm",
        requested_cargo_profile="dev-fast",
        cargo_profile="dev-fast",
        profile_dir="dev-fast",
        incremental_enabled=False,
        env=ProcessEnvironment(environment),
        artifact_selection=specs.RUNTIME_CDYLIB_ARTIFACTS,
        runtime_exports=(),
        link_flags=(),
        cargo_rustflags=(),
        fingerprint_rustflags=(),
        no_default_features=True,
        wasm_cargo_features=(),
        fingerprint_features=(),
        fingerprint_path=plan_root / "shared.fingerprint.json",
        target_root=plan_root / "target",
        stored_fingerprint=None,
        fingerprint=None,
        staticlib_fingerprint=None,
    )
    reloc = shared._replace(artifact_selection=specs.RUNTIME_STATICLIB_ARTIFACTS)
    shared, reloc = specs._resolve_runtime_wasm_cargo_specs(
        plan_root,
        shared,
        reloc,
        simd_enabled=simd,
        freestanding=freestanding,
    )
    assert shared.cargo_plan is reloc.cargo_plan
    plan = shared.cargo_plan
    assert plan is not None
    assert plan.host_target == "x86_64-unknown-linux-gnu"
    # Literal expectations independent of the producer and SIMD receipt reader.
    feature = (
        "target-feature=-reference-types,+simd128"
        if simd
        else "target-feature=-reference-types,-simd128"
    )
    assert plan.rustflags.count("target-feature=-reference-types") == 1
    assert feature in plan.rustflags
    assert ('getrandom_backend="unsupported"' in plan.rustflags) is freestanding
    assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(plan.rustflags)
    assert shared.cargo_rustflags == reloc.cargo_rustflags
    index = plan.command.index("--crate-type")
    assert plan.command[index : index + 2] == ("--crate-type", "staticlib,cdylib")
    assert shared.fingerprint_rustflags == shared.cargo_rustflags
    assert reloc.fingerprint_rustflags == reloc.cargo_rustflags
    plan.verify()
    # Capture binds the resolved environment before the actual runtime caller
    # constructs its command again. Re-enter that caller, not plan.command
    # (which already contains the resolver's pinned Cargo configuration).
    repeated_shared, repeated_reloc = specs._resolve_runtime_wasm_cargo_specs(
        plan_root,
        shared,
        reloc,
        simd_enabled=simd,
        freestanding=freestanding,
    )
    repeated = repeated_shared.cargo_plan
    assert repeated is not None
    assert repeated is repeated_reloc.cargo_plan
    assert repeated.rustflags == plan.rustflags
    assert repeated.command == plan.command
    assert repeated.environment == plan.environment
    assert repeated.configuration_identity() == plan.configuration_identity()
    assert repeated_shared.fingerprint_rustflags == shared.fingerprint_rustflags
    assert repeated_reloc.fingerprint_rustflags == reloc.fingerprint_rustflags
    repeated.verify()


@pytest.mark.parametrize(
    "flag_origin", ["target", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]
)
def test_standalone_cpython_abi_plan_applies_simd_after_cargo_features(
    plan_root, monkeypatch, flag_origin, readonly_wasi_sdk
):
    from molt.cli import runtime_wasm_build_support as support

    _config(
        plan_root,
        '[target.wasm32-wasip1]\nrustflags=["-C","target-feature=-reference-types"]\n',
    )
    sdk = readonly_wasi_sdk()
    environment = {
        "CARGO_HOME": str(plan_root / "cargo-home"),
        "WASI_SDK_PATH": str(sdk.sdk),
        "MOLT_BUILD_PYTHON": sys.executable,
    }
    if flag_origin != "target":
        separator = "\x1f" if flag_origin == "CARGO_ENCODED_RUSTFLAGS" else " "
        environment[flag_origin] = separator.join(
            ("-C", "target-feature=-reference-types")
        )
    monkeypatch.setattr(support, "_cargo_build_env", lambda: dict(environment))
    monkeypatch.setattr(
        support, "_cargo_target_root", lambda root: plan_root / "target"
    )
    monkeypatch.setattr(
        support,
        "_runtime_fingerprint_path",
        lambda *args: plan_root / "cpython.fingerprint.json",
    )

    class Planned(Exception):
        pass

    def capture_plan(*args, **kwargs):
        # Reach the real producer's transform and actual Cargo-plan admission,
        # then stop before build-Python setup, runtime identity or Cargo work.
        raise Planned(plans.resolve_runtime_cargo_plan(*args, **kwargs))

    monkeypatch.setattr(support, "resolve_runtime_cargo_plan", capture_plan)
    with pytest.raises(Planned) as stopped:
        support._ensure_wasm_cpython_abi_staticlib(
            project_root=plan_root,
            json_output=True,
            cargo_profile="dev-fast",
            cargo_timeout=1,
        )
    plan = stopped.value.args[0]
    assert plan.host_target == "x86_64-unknown-linux-gnu"
    assert plan.rustflags.count("target-feature=-reference-types") == 1
    assert "target-feature=-reference-types,+simd128" in plan.rustflags
    assert not any("getrandom_backend=" in flag for flag in plan.rustflags)
    assert plan.environment["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(plan.rustflags)
    assert "molt-lang-cpython-abi" in plan.command
    index = plan.command.index("--crate-type")
    assert plan.command[index : index + 2] == ("--crate-type", "staticlib")
    plan.verify()
    monkeypatch.setattr(support, "_cargo_build_env", lambda: dict(plan.environment))
    with pytest.raises(Planned) as stopped_again:
        support._ensure_wasm_cpython_abi_staticlib(
            project_root=plan_root,
            json_output=True,
            cargo_profile="dev-fast",
            cargo_timeout=1,
        )
    repeated = stopped_again.value.args[0]
    assert repeated.rustflags == plan.rustflags
    assert repeated.command == plan.command
    assert repeated.environment == plan.environment
    assert repeated.configuration_identity() == plan.configuration_identity()
    repeated.verify()


@pytest.mark.parametrize("spelling", ["-C", "-Cjoined", "--codegen", "--codegen="])
@pytest.mark.parametrize("selector", ["linker", "link-arg"])
@pytest.mark.parametrize("final", [False, True])
def test_codegen_alias_resources_have_one_custody_and_partition(
    plan_root, spelling, selector, final
):
    artifact = plan_root / "selected resource.bin"
    artifact.write_bytes(
        b"--export=first\n" if selector == "link-arg" else b"MZbackend"
    )
    operand = (
        "selected-linker"
        if selector == "linker"
        else "@" + str(artifact)
        if selector == "link-arg"
        else str(artifact)
    )
    value = selector + "=" + operand
    flags = (
        (spelling, value)
        if spelling in {"-C", "--codegen"}
        else (("-C" if spelling == "-Cjoined" else spelling) + value,)
    )
    plan = _plan(
        plan_root,
        args=("--", *flags) if final else (),
        env={} if final else {"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)},
    )
    lane = plan.command[plan.command.index("--") + 1 :] if final else plan.rustflags
    selected = (
        plan.tools["final_linker" if final else "linker"]
        if selector == "linker"
        else artifact
    )
    expected = (
        "-C",
        selector + "=" + ("@" if selector == "link-arg" else "") + str(selected),
    )
    assert lane == (expected if final else _with_remaps(plan_root, expected))
    if final and selector == "link-arg":
        compile_command, linking = plan.partition_command()
        assert linking == lane and value not in compile_command
        assert "@response:sha256=" in plan.project_link_arguments(linking)[1]
    plan.verify()
    selected.write_bytes(b"changed admitted input")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("spelling", ["-C", "-Cjoined", "--codegen", "--codegen="])
@pytest.mark.parametrize(
    "option", ["linker-flavor=wasm-lld-cc", "link-self-contained=yes"]
)
def test_wasi_codegen_alias_conflict_fails_at_selected_mode(
    plan_root, spelling, option
):
    flags = (
        (spelling, option)
        if spelling in {"-C", "--codegen"}
        else (("-C" if spelling == "-Cjoined" else spelling) + option,)
    )
    with pytest.raises(ValueError, match="WASI external-libc mode conflicts"):
        _plan(
            plan_root,
            target="wasm32-wasip1",
            args=("--target", "wasm32-wasip1"),
            env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)},
        )


@pytest.mark.parametrize(
    "operand", ["-Clinker=tools/root", "--codegen=linker=tools/root"]
)
def test_runtime_search_operand_cannot_select_a_codegen_tool(plan_root, operand):
    directory = plan_root / operand
    directory.mkdir(parents=True)
    artifact = directory / "libopaque.a"
    artifact.write_bytes(b"opaque search input")
    flags = ("-L", operand, "--out-dir", operand, "--remap-path-prefix", operand)
    plan = _plan(plan_root, env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})
    assert plan.rustflags == _with_remaps(
        plan_root, ("-L", "all=" + str(directory), *flags[2:])
    )
    assert any(item.identity.path == artifact for item in plan.rust_resources.files)
    assert all(not str(path).endswith("tools/root") for path in plan.tools.values())
    artifact.write_bytes(b"changed opaque input")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


@pytest.mark.parametrize("output", ["-C", "-Lnative=unselected", "--codegen"])
def test_runtime_resource_and_partition_passes_preserve_output_operand(
    plan_root, output
):
    plan = _plan(plan_root, args=("--", "-o", output, "--codegen=panic=abort"))
    compile_command, link_args = plan.partition_command()
    assert compile_command[-5:] == ("--", "-o", output, "-C", "panic=abort")
    assert link_args == ()
    plan.verify()


@pytest.mark.parametrize(
    "cluster,option",
    [
        ("-gC", "linker=selected-linker"),
        ("-vC", "link_arg=@exports.rsp"),
        ("-gZ", "codegen_backend=backend.dll"),
    ],
)
def test_clustered_resource_options_keep_prefix_and_fence_selected_input(
    plan_root, cluster, option
):
    (plan_root / "exports.rsp").write_bytes(b"--export=first\n")
    (plan_root / "backend.dll").write_bytes(b"MZbackend")
    plan = _plan(plan_root, args=("--", cluster + option))
    compile_command, link_args = plan.partition_command()
    assert cluster[:-1] in compile_command
    if "link_arg=" in option:
        assert link_args == ("-C", "link-arg=@" + str(plan_root / "exports.rsp"))
        selected = plan_root / "exports.rsp"
    elif "codegen_backend=" in option:
        assert compile_command[-2:] == (
            "-Z",
            "codegen-backend=" + str(plan_root / "backend.dll"),
        )
        selected = plan_root / "backend.dll"
    else:
        selected = plan.tools["final_linker"]
        assert compile_command[-2:] == ("-C", "linker=" + str(selected))
    plan.verify()
    selected.write_bytes(b"changed clustered resource")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


def test_codegen_key_aliases_obey_last_selector_without_reading_shadowed_path(
    plan_root,
):
    selected = plan_root / "backend.dll"
    selected.write_bytes(b"MZbackend")
    flags = ("-gZcodegen_backend=missing.dll", "-Z", "codegen-backend=" + str(selected))
    plan = _plan(plan_root, env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})
    assert plan.rustflags == _with_remaps(
        plan_root, ("-g", "-Z", "codegen-backend=" + str(selected))
    )
    assert any(item.identity.path == selected for item in plan.rust_resources.files)


def test_runtime_metadata_preserves_resource_owned_unstable_backend_and_confines_outputs(
    plan_root, monkeypatch
):
    backend = plan_root / "backend.dll"
    backend.write_bytes(b"MZbackend")
    flags = ("-Z", "codegen-backend=" + str(backend))
    plan = _plan(plan_root, env={"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})
    assert any(item.identity.path == backend for item in plan.rust_resources.files)
    rustc = plan_root / "metadata-rustc"
    write_mock_executable(rustc, b"MZcompiler")
    seen = []

    def run(command, **kwargs):
        seen.append(command)
        assert command[command.index("-Z") + 1] == "codegen-backend=" + str(backend)
        assert str(plan_root / "user-output") not in command
        assert command[command.index("--emit") + 1] == "metadata"
        assert command[command.index("-o") + 1].endswith("probe")
        assert kwargs["input"] == ""
        return subprocess.CompletedProcess(command, 0, _metadata_stdout(plan_root), "")

    monkeypatch.setattr(plans.process_guard, "run_completed_command", run)
    metadata = rustc_probes.rust_target_metadata(
        rustc,
        "target",
        (
            *plan.rustflags,
            "--emit=metadata=" + str(plan_root / "user-output"),
            "-o",
            str(plan_root / "user-output"),
        ),
        root=plan_root,
        env={},
    )
    assert ("selected", None) in metadata.cfg
    assert len(seen) == 1
    plan.verify()
    backend.write_bytes(b"changed backend")
    with pytest.raises(ValueError, match="changed"):
        plan.verify()


def _rewrite_with_restored_mtime(path: Path, value: bytes) -> None:
    before = path.stat()
    assert len(value) == before.st_size
    path.write_bytes(value)
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    after = path.stat()
    assert (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) == (
        after.st_dev,
        after.st_ino,
        after.st_size,
        after.st_mtime_ns,
    )


@pytest.mark.parametrize("changed", ["compiler", "wrapper", "workspace", "backend"])
def test_metadata_questions_are_fresh_for_each_plan(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, changed: str
) -> None:
    backend = plan_root / "selected-backend"
    backend.write_bytes(b"old")
    wrapper = plan_root / "selected-wrapper"
    workspace = plan_root / "selected-workspace"
    write_mock_executable(wrapper, b"MZold")
    write_mock_executable(workspace, b"MZold")
    _config(plan_root, "[target.'cfg(changed)']\nlinker=\"changed-linker\"\n")
    observed = []

    def query(rustc, target, flags, **kwargs):
        paths = [rustc, backend, *kwargs["wrappers"].values()]
        altered = any(b"new" in path.read_bytes() for path in paths)
        observed.append(altered)
        return _metadata(plan_root, "changed" if altered else "unix")

    monkeypatch.setattr(rustc_probes, "rust_target_metadata", query)
    environment = {
        "RUSTFLAGS": "-C codegen-backend=" + str(backend),
        "RUSTC_WRAPPER": str(wrapper),
        "RUSTC_WORKSPACE_WRAPPER": str(workspace),
    }
    first = _plan(plan_root, env=environment)
    selected = {
        "compiler": first.rustc,
        "wrapper": wrapper,
        "workspace": workspace,
        "backend": backend,
    }[changed]
    original = selected.read_bytes()
    replacement = (
        b"new" if changed == "backend" else original[:2] + b"new" + original[5:]
    )
    _rewrite_with_restored_mtime(selected, replacement)
    second = _plan(plan_root, env=environment)
    assert first.tools["linker"].name != "changed-linker"
    assert second.tools["linker"].name == "changed-linker"
    # Each plan discovers and qualifies both the selected and remapped vectors.
    assert observed == [False, False, False, False, True, True, True, True]


@pytest.mark.parametrize("phase", ["before_capture", "during_qualification"])
def test_metadata_discovery_must_agree_under_captured_resources(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, phase: str
) -> None:
    backend = plan_root / "selected-backend"
    backend.write_bytes(b"old")
    _config(plan_root, "[target.'cfg(old)']\nrustflags=[]\n")
    calls = 0

    def query(rustc, target, flags, **kwargs):
        nonlocal calls
        calls += 1
        fact = backend.read_text(encoding="utf-8")
        if calls == (1 if phase == "before_capture" else 3):
            _rewrite_with_restored_mtime(backend, b"new")
        return _metadata(plan_root, fact)

    monkeypatch.setattr(rustc_probes, "rust_target_metadata", query)
    with pytest.raises(ValueError, match="changed"):
        _plan(plan_root, env={"RUSTFLAGS": "-C codegen-backend=" + str(backend)})
    assert calls == 3


def test_provisional_backend_remains_in_plan_custody(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    provisional, final = (plan_root / name for name in ("provisional", "final"))
    provisional.write_bytes(b"old")
    final.write_bytes(b"end")
    _config(
        plan_root,
        "[build]\nrustflags="
        + json.dumps(["-C", "codegen-backend=" + str(provisional)])
        + "\n[target.'cfg(selected)']\nrustflags="
        + json.dumps(["-C", "codegen-backend=" + str(final)])
        + "\n",
    )
    monkeypatch.setattr(
        rustc_probes,
        "rust_target_metadata",
        lambda *args, **kwargs: _metadata(plan_root, "selected"),
    )
    selected = _plan(plan_root)
    assert provisional in {item.identity.path for item in selected.rust_resources.files}
    assert final in {item.identity.path for item in selected.rust_resources.files}
    _rewrite_with_restored_mtime(provisional, b"new")
    with pytest.raises(ValueError, match="changed"):
        selected.verify()


@pytest.mark.parametrize("wrapped", [False, True])
def test_metadata_memo_uses_actual_wrapper_command(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, wrapped: bool
) -> None:
    host = "x86_64-unknown-linux-gnu"
    libdir = plan_root / "lib" / "rustlib" / host / "lib"
    libdir.mkdir(parents=True)
    (libdir / "libcore.rlib").write_bytes(b"core")
    (plan_root / "bin").mkdir()
    observed = []

    def query(rustc, target, flags, **kwargs):
        observed.append(bool(kwargs["wrappers"]))
        return _metadata(plan_root, "unix")

    monkeypatch.setattr(rustc_probes, "rust_target_metadata", query)
    monkeypatch.setattr(plans, "_rust_resource_roots", _rust_resource_roots)
    _plan(plan_root, env={"RUSTC_WRAPPER": "wrapper"} if wrapped else {})
    assert observed == (
        [False, True, False, True] if wrapped else [False, False, False, False]
    )


@pytest.mark.parametrize("explicit", [False, True])
def test_metadata_declared_target_search_is_retained_in_plan(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, explicit: bool
) -> None:
    for target in ("x86_64-unknown-linux-gnu", "custom"):
        libdir = plan_root / "lib" / "rustlib" / target / "lib"
        libdir.mkdir(parents=True)
        (libdir / "libcore.rlib").write_bytes(b"core")
    (plan_root / "bin").mkdir()
    monkeypatch.setattr(plans, "_rust_resource_roots", _rust_resource_roots)
    search = plan_root / "custom-targets"
    search.mkdir()
    spec = search / "custom.json"
    spec.write_bytes(b'{"value":"old"}')
    unrelated = search / "unrelated.json"
    unrelated.write_bytes(b"not a selected target")
    nested = search / "build-output" / "large-object"
    nested.parent.mkdir()
    nested.write_bytes(b"not a target specification")
    monkeypatch.setattr(
        rustc_probes,
        "rust_target_metadata",
        lambda *args, **kwargs: _metadata(plan_root, "unix"),
    )
    environment = {"RUST_TARGET_PATH": str(search)}
    if explicit:
        environment.update(
            {name: "selected-" + name.lower() for name in ("CC", "CXX", "AR", "RANLIB")}
        )
        environment["CARGO_TARGET_CUSTOM_LINKER"] = "selected-linker"
    selected = _plan(
        plan_root,
        env=environment,
        target="custom" if explicit else None,
        args=("--target", "custom") if explicit else (),
    )
    captured = {item.identity.path for item in selected.rust_resources.files}
    assert (spec in captured) is explicit
    assert unrelated not in captured and nested not in captured
    unrelated.write_bytes(b"unrelated target can change")
    nested.write_bytes(b"build products can change")
    selected.verify()
    _rewrite_with_restored_mtime(spec, b'{"value":"new"}')
    if explicit:
        with pytest.raises(ValueError, match="changed"):
            selected.verify()
    else:
        selected.verify()


@pytest.mark.parametrize("final", ["print", "response", "backend"])
def test_wasi_metadata_combines_admitted_lanes_without_readmitting_them(
    plan_root: Path, monkeypatch: pytest.MonkeyPatch, final: str
) -> None:
    # Restore the real root/metadata walk: the general plan fixture substitutes
    # it and therefore cannot expose combined-vector admission failures.
    for target in ("x86_64-unknown-linux-gnu", "wasm32-wasip1"):
        libdir = plan_root / "lib" / "rustlib" / target / "lib"
        libdir.mkdir(parents=True)
        (libdir / "libcore.rlib").write_bytes(b"core")
    (plan_root / "bin").mkdir()
    monkeypatch.setattr(plans, "_rust_resource_roots", _rust_resource_roots)
    _config(plan_root, "[target.'cfg(target_arch=\"wasm32\")']\nrustflags=[]\n")
    seen = []

    def query(rustc, target, flags, **kwargs):
        seen.append((target, flags))
        return _metadata(plan_root, 'target_arch="wasm32"')

    monkeypatch.setattr(rustc_probes, "rust_target_metadata", query)
    resource = None
    if final == "response":
        resource = plan_root / "runtime.rsp"
        resource.write_text("--no-entry\n--import-memory\n", encoding="utf-8")
        final_args = ("-C", "link-arg=@" + str(resource))
    elif final == "backend":
        resource = plan_root / "selected-backend"
        resource.write_bytes(b"old")
        final_args = ("-C", "codegen-backend=" + str(resource))
    else:
        final_args = ("--print", "native-static-libs")
    plan = _plan(
        plan_root,
        target="wasm32-wasip1",
        args=("--target", "wasm32-wasip1", "--", *final_args),
    )
    passthrough = plan.command[plan.command.index("--") + 1 :]
    combined = (*plan.rustflags, *passthrough)
    for option in ("link-self-contained=no", "linker-flavor=wasm-ld"):
        assert plan.rustflags.count(option) == 1
        assert passthrough.count(option) == 1
        assert combined.count(option) == 2
    assert passthrough[: len(final_args)] == final_args
    # Each exact question still has one discovery and one fenced qualification.
    assert seen.count(("wasm32-wasip1", combined)) == 2
    plan.verify()
    if resource is not None:
        custody = plan.link_resources if final == "response" else plan.rust_resources
        assert resource in {item.identity.path for item in custody.files}
        resource.write_bytes(b"changed resource")
        with pytest.raises(ValueError, match="changed"):
            plan.verify()


@pytest.mark.parametrize("final", [False, True])
@pytest.mark.parametrize("option", ["link-self-contained=no", "linker-flavor=wasm-ld"])
def test_wasi_duplicate_explicit_modes_inside_one_lane_still_fail(
    plan_root: Path, final: bool, option: str
) -> None:
    duplicate = ("-C", option, "-C", option)
    with pytest.raises(ValueError, match="WASI external-libc mode conflicts"):
        _plan(
            plan_root,
            target="wasm32-wasip1",
            args=("--target", "wasm32-wasip1", *(("--", *duplicate) if final else ())),
            env={} if final else {"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(duplicate)},
        )
