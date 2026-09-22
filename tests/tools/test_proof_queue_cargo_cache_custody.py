from __future__ import annotations

import copy
import json
import os
from pathlib import Path

import pytest

from tools.proof_queue_pkg import cargo_cache_custody as cache
from tools.proof_queue_pkg import (
    custody_cas,
    execution_environment,
    command_identity,
    execution_receipt_details,
    supervisor_custody,
    cargo_output_environment,
    command_admission,
)


@pytest.fixture(autouse=True)
def source_admission(monkeypatch):
    monkeypatch.setattr(
        execution_environment,
        "_git_source_paths",
        lambda root, env, **kwargs: [root / "main.rs"],
    )
    monkeypatch.setattr(
        cache.disk_capacity,
        "_default_measure_free_bytes",
        lambda path: 1 << 50,
    )


def _capture(inputs, overlays=(), *, hash_workers=1):
    reference, telemetry, verify = execution_environment.capture_source_content(
        source_root=inputs["source_root"],
        env=inputs["env"],
        overlays=overlays,
        cas_root=inputs["result_root"] / "custody-cas",
        hash_workers=hash_workers,
    )
    inputs["source_content"] = reference
    return telemetry, verify


def _outputs(command: list[str]) -> cargo_output_environment.CargoOutputEnvironment:
    """Derive output roles from admitted Cargo syntax, never execution rewriting."""
    return cargo_output_environment.CargoOutputEnvironment.for_invocation(
        command_admission.parse_cargo_invocation(command)
    )


def _inputs(tmp_path: Path) -> dict:
    source = tmp_path / "source"
    source.mkdir()
    (source / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
    inputs = {
        "result_root": tmp_path / "proofs",
        "source_root": source,
        "toolchains": {},
        "command": ["cargo", "test", "--offline"],
        "env": {"RUSTFLAGS": "-Cdebuginfo=0", "CARGO_TARGET_DIR": "requested"},
        "requested_target": "requested",
        "run_id": "unit-one",
        "execution_nonce_sha256": "a" * 64,
        "timeout_s": 0.0,
        "source_snapshot": {"root": str(source), "commit": "initial"},
    }
    inputs["outputs"] = _outputs(inputs["command"])
    _capture(inputs)
    return inputs


@pytest.mark.parametrize(
    "name",
    [
        "MOLT_GUARD_SCRATCH_ROOT",
        "MOLT_MEMORY_GUARD_ACTIVE",
        "MOLT_MEMORY_GUARD_PID",
        "MOLT_MEMORY_GUARD_TOKEN",
        "MOLT_MEMORY_GUARD_MARKER",
    ],
)
def test_guard_transport_changes_do_not_fork_cargo_compilation_identity(name):
    arguments = {"source": {}, "toolchains": {}, "command": ["cargo", "test"]}
    arguments["outputs"] = _outputs(arguments["command"])
    base = cache.input_identity(**arguments, env={"RUSTFLAGS": "-Cdebuginfo=0"})
    changed = cache.input_identity(
        **arguments, env={"RUSTFLAGS": "-Cdebuginfo=0", name: "per-run-custody"}
    )
    assert base == changed
    assert name in command_identity._QUEUE_CUSTODY_ENV_NAMES


def test_queue_environment_preserves_scratch_custody_without_admitting_secrets(
    tmp_path,
):
    from molt import temporary_artifacts as scratch

    state = tmp_path / "tmp" / "memory_guard"
    inherited = {
        "MOLT_MEMORY_GUARD_STATE_ROOT": str(state),
        "MOLT_MEMORY_GUARD_TOKEN": "a" * 32,
        "MOLT_MEMORY_GUARD_MARKER": str(state / "active" / "fixture.json"),
        "MOLT_API_TOKEN": "external-credential",
    }
    lease = scratch.acquire_guard_scratch(tmp_path, inherited)
    inherited[scratch.SCRATCH_ENV] = str(lease.target)
    try:
        selected, contract = execution_environment._deterministic_execution_environment(
            inherited, override_names=[]
        )
        assert scratch.guard_scratch(tmp_path, selected) == lease.target
        assert contract["omitted_names"] == ["MOLT_API_TOKEN"]
        for name in selected:
            assert execution_environment.environment_override_policy_error(
                {name: "forged"}
            )
        authority = execution_environment._execution_environment_authority(
            selected,
            applied_cargo_policies=[],
            fingerprint_key=b"fixture",
            contract=contract,
        )
        assert "a" * 32 not in json.dumps(authority)
        assert authority["variables"]["MOLT_MEMORY_GUARD_TOKEN"]["redacted"] is True
    finally:
        scratch.finish_guard_scratch(
            lease, closed=True, success=True, evidence={"authority": "fixture"}
        )


@pytest.mark.parametrize(
    "name",
    [
        "TMPDIR",
        "TMP",
        "TEMP",
        "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
        "MOLT_BACKEND_MAX_RSS_GB",
    ],
)
def test_caller_temp_and_backend_policy_remain_compilation_inputs(name):
    arguments = {"source": {}, "toolchains": {}, "command": ["cargo", "build"]}
    arguments["outputs"] = _outputs(arguments["command"])
    assert cache.input_identity(**arguments, env={name: "one"}) != cache.input_identity(
        **arguments, env={name: "two"}
    )


@pytest.mark.parametrize(
    "subcommand", ["test", "doc", "rustdoc", "build", "check", "run"]
)
def test_output_environment_roundtrips_acquisition_and_parent_policy(
    tmp_path, subcommand
):
    inputs = _inputs(tmp_path)
    inputs["command"] = ["cargo", "+fixture", subcommand, "--offline"]
    inputs["outputs"] = _outputs(inputs["command"])
    inputs["env"].update(TEMP="caller-temp", TMP="caller-tmp", TMPDIR="caller-tmpdir")
    original = dict(inputs["env"])
    lease = cache.acquire(**inputs)
    try:
        outputs = inputs["outputs"]
        assert lease.environment == outputs.bind(original, target=lease.target)
        assert inputs["env"] == original
        final_env, contract = execution_environment._cargo_output_environment_contract(
            lease.environment,
            {"passed_names": sorted(original), "override_names": []},
            outputs=outputs,
            target=lease.target,
        )
        assert set(final_env) == set(contract["passed_names"])
        assert contract["cargo_output_environment"] == outputs.identity()
        identity = custody_cas.read_ref(
            lease.provenance["inputs"], expected_root=lease.cas_root
        )["identity"]
        assert identity["output_environment"] == outputs.identity()
        assert str(lease.target) not in json.dumps(identity["output_environment"])
        # This is the parent consumer which rejected ci77 after its successful
        # command, not just a test of environment dictionary edits.
        cache.validate_prelaunch(
            lease.provenance,
            cas_root=lease.cas_root,
            command=inputs["command"],
            outputs=outputs,
            env=final_env,
            toolchains=inputs["toolchains"],
            source_root=str(inputs["source_root"]),
            source_snapshot=inputs["source_snapshot"],
            source_content=inputs["source_content"],
        )
    finally:
        lease.close()


@pytest.mark.parametrize("name", ["CARGO_TARGET_DIR", "TEMP", "TMP", "TMPDIR"])
@pytest.mark.parametrize("mutation", ["redirect", "remove"])
def test_symbolic_output_identity_never_admits_forged_actual_binding(
    tmp_path, name, mutation
):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    try:
        forged = dict(lease.environment)
        if mutation == "redirect":
            forged[name] = str(tmp_path / "unowned")
        else:
            del forged[name]
        with pytest.raises(ValueError, match="output environment differs"):
            cache.validate_prelaunch(
                lease.provenance,
                cas_root=lease.cas_root,
                command=inputs["command"],
                outputs=inputs["outputs"],
                env=forged,
                toolchains=inputs["toolchains"],
                source_root=str(inputs["source_root"]),
                source_snapshot=inputs["source_snapshot"],
                source_content=inputs["source_content"],
            )
    finally:
        lease.close()


@pytest.mark.parametrize("subcommand", ["test", "doc", "rustdoc"])
def test_owned_temporary_paths_are_roles_not_caller_values(subcommand):
    arguments = {"source": {}, "toolchains": {}, "command": ["cargo", subcommand]}
    arguments["outputs"] = _outputs(arguments["command"])
    base = cache.input_identity(**arguments, env={"TEMP": "one", "RUSTFLAGS": "one"})
    assert base == cache.input_identity(
        **arguments, env={"TEMP": "two", "TMPDIR": "added", "RUSTFLAGS": "one"}
    )
    assert base != cache.input_identity(
        **arguments, env={"TEMP": "one", "RUSTFLAGS": "two"}
    )
    assert base != cache.input_identity(
        **arguments,
        env={"TEMP": "one", "RUSTFLAGS": "one", "MOLT_BACKEND_MAX_RSS_GB": "4"},
    )


def test_output_variable_case_follows_host_environment_semantics(tmp_path):
    outputs = _outputs(["cargo", "test"])
    env = outputs.bind({"temp": "caller-lowercase"}, target=tmp_path)
    if os.name == "nt":
        assert "temp" not in env
        with pytest.raises(ValueError, match="output environment differs"):
            outputs.validate({**env, "temp": str(tmp_path)}, target=tmp_path)
    else:
        assert env["temp"] == "caller-lowercase"
        assert outputs.caller_environment(env) == {"temp": "caller-lowercase"}
    outputs.validate(env, target=tmp_path)


def test_output_policy_uses_parsed_operation_not_argument_spellings():
    outputs = _outputs(["cargo", "--config", "test", "build", "--package", "rustdoc"])
    assert outputs.names == ("CARGO_TARGET_DIR",)
    assert _outputs(
        ["cargo", "test", "--", "--target-dir=opaque-harness-argument"]
    ).documentation
    assert not _outputs(["cargo", "test", "--help"]).documentation


def test_admitted_delegated_outputs_survive_python_custody_rewriting(tmp_path):
    """The parent reconstructs Cargo output roles from admission, not transport."""
    admitted_argv = [
        "uv",
        "run",
        "--no-sync",
        "python",
        "tools/guarded_exec.py",
        "--",
        "cargo",
        "test",
    ]
    envelope = command_admission.envelope_for_command(admitted_argv)
    outputs = cargo_output_environment.CargoOutputEnvironment.for_envelope(envelope)
    execution_command = command_admission._python_bootstrap_command(
        envelope, admitted_argv
    )
    with pytest.raises(ValueError, match="direct Python target"):
        command_admission._nested_command(execution_command)
    with pytest.raises(ValueError):
        command_admission.parse_cargo_invocation(execution_command)

    inputs = _inputs(tmp_path)
    inputs["command"] = execution_command
    inputs["outputs"] = outputs
    lease = cache.acquire(**inputs)
    try:
        final_env, _contract = execution_environment._cargo_output_environment_contract(
            lease.environment,
            {"passed_names": [], "override_names": []},
            outputs=outputs,
            target=lease.target,
        )
        identity = custody_cas.read_ref(
            lease.provenance["inputs"], expected_root=lease.cas_root
        )["identity"]
        assert identity["command"] == execution_command
        assert identity["output_environment"] == outputs.identity()

        reconstructed = cargo_output_environment.CargoOutputEnvironment.for_envelope(
            envelope
        )
        cache.validate_prelaunch(
            lease.provenance,
            cas_root=lease.cas_root,
            command=execution_command,
            outputs=reconstructed,
            env=final_env,
            toolchains=inputs["toolchains"],
            source_root=str(inputs["source_root"]),
            source_snapshot=inputs["source_snapshot"],
            source_content=inputs["source_content"],
        )
        with pytest.raises(ValueError, match="identity mismatch"):
            cache.validate_prelaunch(
                lease.provenance,
                cas_root=lease.cas_root,
                command=[*execution_command, "--nocapture"],
                outputs=reconstructed,
                env=final_env,
                toolchains=inputs["toolchains"],
                source_root=str(inputs["source_root"]),
                source_snapshot=inputs["source_snapshot"],
                source_content=inputs["source_content"],
            )
        wrong_policy = cargo_output_environment.CargoOutputEnvironment(
            documentation=False
        )
        with pytest.raises(ValueError, match="identity mismatch"):
            cache.validate_prelaunch(
                lease.provenance,
                cas_root=lease.cas_root,
                command=execution_command,
                outputs=wrong_policy,
                env=final_env,
                toolchains=inputs["toolchains"],
                source_root=str(inputs["source_root"]),
                source_snapshot=inputs["source_snapshot"],
                source_content=inputs["source_content"],
            )
    finally:
        lease.close()


@pytest.mark.parametrize("hash_workers", [1, 4])
def test_source_inventory_uses_planned_workers_with_identical_content_and_fences(
    tmp_path, hash_workers
):
    inputs = _inputs(tmp_path)
    overlay = inputs["source_root"] / "overlay.rs"
    overlay.write_bytes(b"const VALUE: u8 = 7;\n")
    serial, _ = _capture(inputs, [overlay], hash_workers=1)
    reference = inputs["source_content"]
    telemetry, verify = _capture(inputs, [overlay], hash_workers=hash_workers)
    assert inputs["source_content"] == reference
    profile = telemetry["inventory_profile"]
    assert profile["hash_workers"] == hash_workers
    assert profile["hashed_files"] == telemetry["file_count"] == 2
    assert (
        profile["hashed_bytes"] == telemetry["bytes_hashed"] == serial["bytes_hashed"]
    )
    assert 0 <= profile["hash_seconds"] <= telemetry["capture_s"]
    verify()
    overlay.write_bytes(b"const VALUE: u8 = 8;\n")
    with pytest.raises(ValueError, match="changed"):
        verify()


def _completed(lease: cache.CargoCacheLease, *, returncode: int = 0) -> dict:
    context = {
        name: {"identical": True}
        for name in (
            "source_custody",
            "toolchain_custody",
            "execution_environment",
            "command_executable",
            "custody_authorities",
            "platform_process_custody",
        )
    }
    context["source_custody"]["ineligible_reasons"] = []
    identity = custody_cas.read_ref(
        lease.provenance["inputs"], expected_root=lease.cas_root
    )["identity"]
    source = identity["source"]
    context["source_custody"].update(
        prelaunch=source["git_snapshot"],
        content={
            "prelaunch": source["content"],
            "postcompletion": source["content"],
            "identical": True,
        },
    )
    context.update(
        run_id="unit-one",
        execution_nonce_sha256="a" * 64,
        process_supervisor={
            "supervisor_returncode": 0,
            "receipt": {
                "state": "COMPLETE",
                "complete": True,
                "violation_count": 0,
                "error_count": 0,
            },
        },
        live_input_custody={"stable": True, "event_count": 0, "error_count": 0},
        child_process_custody={"receipt": {"broker_complete": True}},
        execution_custody_session={"state": "DRAINED"},
        derived_root_custody={"prelaunch": [lease.provenance]},
    )
    return {
        "phase": "complete",
        "command_returncode": returncode,
        "receipt_context": execution_receipt_details.compact_context(
            context, cas_root=lease.cas_root
        ),
    }


def _terminal_receipt(
    lease: cache.CargoCacheLease,
    publication: dict,
    *,
    process_cleanup_safe: bool = True,
) -> dict:
    return {
        "schema": cache.TERMINAL_RECEIPT_SCHEMA,
        "run_id": lease.provenance["generation_run_id"],
        "execution_nonce_sha256": lease.provenance["execution_nonce_sha256"],
        "input_sha256": lease.provenance["input_sha256"],
        "generation_id": lease.provenance["generation_id"],
        "target": lease.provenance["path"],
        "execution_custody_sha256": "b" * 64,
        "cargo_cache_publication": publication,
        "process_cleanup_safe": process_cleanup_safe,
        "guard_receipt": {"sha256": "c" * 64},
        "process_supervisor": {"receipt": {"state": "COMPLETE"}},
        "queue_terminal": {
            "schema": supervisor_custody.QUEUE_TERMINAL_SCHEMA,
            "status": "failed",
            "returncode": 2,
            "command_returncode": None,
            "execution_error": None,
        },
    }


@pytest.mark.parametrize(
    "mutation", ["old-schema", "missing-outcome", "command-substitution"]
)
def test_terminal_contract_rejects_legacy_or_substituted_outcomes(tmp_path, mutation):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    lease.close()
    receipt = _terminal_receipt(lease, lease.publication_outcome)
    if mutation == "old-schema":
        receipt["schema"] = "molt.proof-cargo-cache-terminal-receipt.v1"
    elif mutation == "missing-outcome":
        receipt.pop("queue_terminal")
    else:
        receipt["command_returncode"] = 0
    before = lease.owner_path.read_bytes()
    with pytest.raises(
        ValueError, match="schema mismatch|final queue outcome|command result"
    ):
        cache.record_terminal_receipt(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            terminal_receipt=receipt,
        )
    assert lease.owner_path.read_bytes() == before
    assert lease.target.is_dir()


@pytest.mark.parametrize("returncode", [0, 1, 101])
def test_sealed_candidates_including_failed_tests_cannot_claim_warm_admission(
    tmp_path, monkeypatch, returncode
):
    inputs = _inputs(tmp_path)
    first = cache.acquire(**inputs)
    try:
        assert first.provenance["state"] == "cold"
        (first.target / "artifact.rlib").write_bytes(b"verified output")
        publication = first.publish(_completed(first, returncode=returncode))
        assert publication["state"] == "sealed"
        assert publication["purpose"] == "preserved-candidate"
        assert publication["reusable"] is False
    finally:
        first.close()
    inputs["run_id"] = "unit-two"
    inputs["env"].update(
        CARGO_TARGET_DIR="different-request", MOLT_PROOF_QUEUE_RUN_ID="new-run"
    )
    pointer_before = first.pointer.read_bytes()

    def no_target_scan(*args, **kwargs):
        pytest.fail("unproven input closure must reject before large target inventory")

    monkeypatch.setattr(
        command_identity, "_directory_manifest_identity", no_target_scan
    )
    read_ref = custody_cas.read_ref

    def no_candidate_manifest_read(reference, **kwargs):
        if reference == publication["seal"]:
            pytest.fail("unproven input closure must reject without manifest reread")
        return read_ref(reference, **kwargs)

    monkeypatch.setattr(custody_cas, "read_ref", no_candidate_manifest_read)
    with pytest.raises(cache.CargoInputClosureUnproven) as error:
        cache.acquire(**inputs)
    assert error.value.diagnostic["code"] == "cargo-input-closure-unproven"
    assert error.value.diagnostic["candidate_path"] == str(first.target)
    assert error.value.diagnostic["candidate_seal"] == publication["seal"]
    assert first.pointer.read_bytes() == pointer_before
    assert (first.target / "artifact.rlib").read_bytes() == b"verified output"
    # Admission failure releases the OS lease as well as preserving the candidate.
    with pytest.raises(cache.CargoInputClosureUnproven):
        cache.acquire(**inputs)


@pytest.mark.parametrize(
    "dimension", ["source", "revision", "toolchain", "command", "environment"]
)
def test_semantic_input_changes_never_admit_previous_generation(tmp_path, dimension):
    inputs = _inputs(tmp_path)
    first = cache.acquire(**inputs)
    try:
        first.publish(_completed(first))
    finally:
        first.close()
    if dimension == "source":
        (inputs["source_root"] / "main.rs").write_text(
            "fn main() { panic!(); }\n", encoding="utf-8"
        )
        _capture(inputs)
    elif dimension == "revision":
        inputs["source_snapshot"]["commit"] = "new-revision"
    elif dimension == "toolchain":
        inputs["toolchains"] = {
            "rustc": {"path": str(tmp_path / "rustc"), "sha256": "a" * 64, "size": 1}
        }
    elif dimension == "command":
        inputs["command"] += ["--target", "wasm32-unknown-unknown"]
        inputs["outputs"] = _outputs(inputs["command"])
    else:
        inputs["env"]["RUSTFLAGS"] = "-Cdebuginfo=1"
    second = cache.acquire(**inputs)
    try:
        assert second.provenance["state"] == "cold"
        assert second.target != first.target
    finally:
        second.close()


@pytest.mark.parametrize(
    "mutation", ["bytes", "extra", "missing", "directory", "mtime"]
)
def test_candidate_bytes_never_override_missing_input_closure(tmp_path, mutation):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    target = lease.target
    try:
        (target / "out").write_bytes(b"original")
        lease.publish(_completed(lease))
    finally:
        lease.close()
    if mutation == "bytes":
        (target / "out").write_bytes(b"modified")
    elif mutation == "extra":
        (target / "injected").write_bytes(b"payload")
    elif mutation == "missing":
        (target / "out").unlink()
    elif mutation == "directory":
        (target / "empty-extra").mkdir()
    else:
        metadata = (target / "out").stat()
        os.utime(
            target / "out", ns=(metadata.st_atime_ns, metadata.st_mtime_ns + 1_000_000)
        )
    with pytest.raises(
        cache.CargoInputClosureUnproven, match="cargo-input-closure-unproven"
    ):
        cache.acquire(**inputs)


@pytest.mark.parametrize(
    "failure",
    [
        "phase",
        "drain",
        "toolchain",
        "source",
        "monitor",
        "supervisor",
        "detail",
        "child",
    ],
)
def test_incomplete_or_unstable_execution_cannot_seal(tmp_path, failure):
    inputs = _inputs(tmp_path)
    first = cache.acquire(**inputs)
    try:
        (first.target / "partial").write_bytes(b"partial")
        result = _completed(first)
        context = result["receipt_context"]
        if failure == "phase":
            result["phase"] = "failed"
        elif failure == "drain":
            context["execution_custody_session"]["state"] = "RUNNING"
        elif failure in {"toolchain", "source"}:
            context[f"{failure}_custody"]["identical"] = False
        elif failure == "monitor":
            context["live_input_custody"]["stable"] = False
        elif failure == "detail":
            context.pop("execution_details")
        elif failure == "child":
            context["child_process_custody"]["receipt"]["broker_complete"] = False
        else:
            context["process_supervisor"]["receipt"]["complete"] = False
        publication = first.publish(result)
        assert publication["state"] == "unsealed"
        if failure == "detail":
            assert publication["reason"] == "invalid-execution-details"
            assert "no durable detail authority" in publication["error"]
    finally:
        first.close()
    second = cache.acquire(**inputs)
    try:
        assert second.target != first.target
        assert (first.target / "partial").exists()
        assert second.provenance["cold_reason"] == "prior-generation-unsealed"
    finally:
        second.close()


def test_target_lease_is_exclusive_and_released_after_failure(tmp_path):
    inputs = _inputs(tmp_path)
    first = cache.acquire(**inputs)
    try:
        with pytest.raises(RuntimeError, match="busy"):
            cache.acquire(**inputs)
    finally:
        first.close()
    second = cache.acquire(**inputs)
    second.close()


def test_cache_rejects_external_hardlinks(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    outside = tmp_path / "outside"
    outside.write_bytes(b"mutable outside owner")
    try:
        os.link(outside, lease.target / "escape")
        with pytest.raises(ValueError, match="hard links outside"):
            lease.publish(_completed(lease))
    finally:
        lease.close()


def test_cache_accepts_internal_cargo_hardlinks(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    try:
        (lease.target / "binary").write_bytes(b"binary")
        os.link(lease.target / "binary", lease.target / "binary-alias")
        assert lease.publish(_completed(lease))["state"] == "sealed"
    finally:
        lease.close()


def test_cache_rejects_symlink_entries(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    try:
        (lease.target / "real").write_bytes(b"real")
        try:
            (lease.target / "alias").symlink_to(lease.target / "real")
        except OSError as exc:
            pytest.skip(f"host cannot create symlink: {exc}")
        with pytest.raises(ValueError, match="link or junction"):
            lease.publish(_completed(lease))
    finally:
        lease.close()


def test_runner_input_binding_rejects_forged_semantic_environment(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    try:
        forged = copy.deepcopy(lease.environment)
        forged["RUSTFLAGS"] = "changed"
        with pytest.raises(ValueError, match="identity mismatch"):
            cache.validate_prelaunch(
                lease.provenance,
                cas_root=lease.cas_root,
                command=inputs["command"],
                outputs=inputs["outputs"],
                env=forged,
                toolchains=inputs["toolchains"],
                source_root=str(inputs["source_root"]),
                source_snapshot=inputs["source_snapshot"],
                source_content=inputs["source_content"],
            )
    finally:
        lease.close()


@pytest.mark.parametrize(
    "field,value",
    [
        ("generation_id", "0" * 16),
        ("generation_owner", "wrong-owner.json"),
        ("generation_run_id", ""),
        ("execution_nonce_sha256", "not-a-digest"),
    ],
)
def test_prelaunch_rejects_substituted_generation_provenance(tmp_path, field, value):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    try:
        forged = copy.deepcopy(lease.provenance)
        forged[field] = value
        with pytest.raises(ValueError, match="generation provenance"):
            cache.validate_prelaunch(
                forged,
                cas_root=lease.cas_root,
                command=inputs["command"],
                outputs=inputs["outputs"],
                env=lease.environment,
                toolchains=inputs["toolchains"],
                source_root=str(inputs["source_root"]),
                source_snapshot=inputs["source_snapshot"],
                source_content=inputs["source_content"],
            )
    finally:
        lease.close()


def test_cache_cannot_publish_after_releasing_exclusive_lease(tmp_path):
    lease = cache.acquire(**_inputs(tmp_path))
    lease.close()
    with pytest.raises(ValueError, match="exclusive lease"):
        lease.publish(_completed(lease))


def test_explicit_target_argument_cannot_bypass_effective_target(tmp_path):
    inputs = _inputs(tmp_path)
    inputs["command"] += ["--target-dir", str(tmp_path / "escape")]
    with pytest.raises(ValueError, match="bypasses proof target custody"):
        _outputs(inputs["command"])


def test_source_admission_excludes_local_cache_but_includes_explicit_overlay(tmp_path):
    inputs = _inputs(tmp_path)
    original = inputs["source_content"]
    ignored = inputs["source_root"] / "ignored-cache"
    ignored.write_bytes(b"not an admitted source")
    telemetry, verify = _capture(inputs)
    assert inputs["source_content"] == original
    assert telemetry["file_count"] == 1
    ignored.write_bytes(b"changed cache")
    verify()
    _capture(inputs, [ignored])
    assert inputs["source_content"] != original


def test_current_source_receipt_cannot_be_substituted_by_cache_inputs(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    try:
        (inputs["source_root"] / "main.rs").write_bytes(b"different dirty bytes")
        _capture(inputs)
        with pytest.raises(ValueError, match="identity mismatch"):
            cache.validate_prelaunch(
                lease.provenance,
                cas_root=lease.cas_root,
                command=inputs["command"],
                outputs=inputs["outputs"],
                env=lease.environment,
                toolchains=inputs["toolchains"],
                source_root=str(inputs["source_root"]),
                source_snapshot=inputs["source_snapshot"],
                source_content=inputs["source_content"],
            )
    finally:
        lease.close()


def test_source_content_terminal_fence_rejects_changed_bytes(tmp_path):
    inputs = _inputs(tmp_path)
    _, verify = _capture(inputs)
    (inputs["source_root"] / "main.rs").write_bytes(b"changed")
    with pytest.raises(ValueError):
        verify()


@pytest.mark.parametrize("mutation", ["add", "delete"])
def test_output_membership_is_fenced_during_hashing(tmp_path, monkeypatch, mutation):
    from molt.python_file_node_custody import PythonFileCaptureContext

    root = tmp_path / "output"
    root.mkdir()
    (root / "artifact").write_bytes(b"artifact")
    (root / "empty").mkdir()
    bind = PythonFileCaptureContext.bind

    def mutate(self, *args, **kwargs):
        result = bind(self, *args, **kwargs)
        if mutation == "add":
            (root / "added").mkdir()
        else:
            (root / "empty").rmdir()
        return result

    monkeypatch.setattr(PythonFileCaptureContext, "bind", mutate)
    with pytest.raises(ValueError, match="changed during inventory"):
        command_identity._directory_manifest_identity(
            root, label="test output", strict_owned=True
        )


def test_current_receipt_cannot_assert_unsupported_warm_admission(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    try:
        forged = copy.deepcopy(lease.provenance)
        forged["state"] = "reused"
        with pytest.raises(cache.CargoInputClosureUnproven):
            cache.validate_prelaunch(
                forged,
                cas_root=lease.cas_root,
                command=inputs["command"],
                outputs=inputs["outputs"],
                env=lease.environment,
                toolchains=inputs["toolchains"],
                source_root=str(inputs["source_root"]),
                source_snapshot=inputs["source_snapshot"],
                source_content=inputs["source_content"],
            )
    finally:
        lease.close()


def test_ignored_build_input_change_cannot_reuse_sealed_output(tmp_path):
    inputs = _inputs(tmp_path)
    ignored_input = inputs["source_root"] / "config.bin"
    ignored_input.write_bytes(b"first")
    original_content = inputs["source_content"]
    lease = cache.acquire(**inputs)
    try:
        (lease.target / "generated.rlib").write_bytes(b"compiled from first")
        lease.publish(_completed(lease))
    finally:
        lease.close()
    ignored_input.write_bytes(b"second")
    _capture(inputs)
    assert inputs["source_content"] == original_content
    with pytest.raises(cache.CargoInputClosureUnproven):
        cache.acquire(**inputs)
    assert (lease.target / "generated.rlib").read_bytes() == b"compiled from first"


def test_pending_retry_preserves_each_generation_owner(tmp_path):
    inputs = _inputs(tmp_path)
    first = cache.acquire(**inputs)
    first_owner = first.owner_path
    first.close()
    inputs.update(run_id="unit-two", execution_nonce_sha256="d" * 64)
    second = cache.acquire(**inputs)
    try:
        assert second.owner_path != first_owner
        first_payload = cache.loads_exact(first_owner.read_text(encoding="utf-8"))
        second_payload = cache.loads_exact(
            second.owner_path.read_text(encoding="utf-8")
        )
        assert first_payload["run_id"] == "unit-one"
        assert first_payload["lifecycle"] == "awaiting-terminal-receipt"
        assert second_payload["run_id"] == "unit-two"
        assert second_payload["lifecycle"] == "leased"
    finally:
        second.close()


def test_terminal_unsealed_reclaim_preserves_receipt_timings_and_tombstone(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "partial.rlib").write_bytes(b"partial")
    timings = lease.target / "cargo-timings"
    timings.mkdir()
    (timings / "cargo-timing.html").write_bytes(b"timing evidence")
    incomplete = _completed(lease)
    incomplete["receipt_context"]["execution_custody_session"]["state"] = "RUNNING"
    publication = lease.publish(incomplete)
    with pytest.raises(RuntimeError, match="busy"):
        cache.record_terminal_receipt(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            terminal_receipt=_terminal_receipt(lease, publication),
            timeout_s=0.0,
        )
    lease.close()

    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    assert projection["state"] == "terminal-unsealed-reclaimable"
    validated = cache.validate_terminal_receipt(
        result_root=inputs["result_root"],
        projection=projection,
        provenance=lease.provenance,
        run_id="unit-one",
        execution_nonce_sha256="a" * 64,
    )
    assert validated["cargo_cache_publication"] == publication
    inspection = cache.inspect_terminal_generation(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert inspection == {
        "state": "terminal-unsealed-reclaimable",
        "target": str(lease.target),
        "owner": str(lease.owner_path),
        "target_present": True,
        "terminal_state": "terminal-unsealed-reclaimable",
        "publication_state": "unsealed",
        "process_cleanup_safe": True,
        "reclaim_eligible": True,
        "reclaim_reason": "terminal-unsealed-cleanup-safe",
    }

    reclaimed = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert reclaimed["state"] == "reclaimed"
    assert not lease.target.exists()
    owner = cache.loads_exact(lease.owner_path.read_text(encoding="utf-8"))
    assert owner["lifecycle"] == "reclaimed"
    assert owner["terminal_receipt"] == projection["terminal_receipt"]
    cache.validate_terminal_receipt(
        result_root=inputs["result_root"],
        projection=projection,
        provenance=lease.provenance,
        run_id="unit-one",
        execution_nonce_sha256="a" * 64,
    )
    evidence = custody_cas.read_ref(
        reclaimed["evidence"], expected_root=inputs["result_root"] / "custody-cas"
    )
    timing_payload = custody_cas.read_ref(
        evidence["timings"], expected_root=inputs["result_root"] / "custody-cas"
    )
    timing_file = timing_payload["files"][0]["file"]
    custody_cas.verify_file_ref(
        timing_file, expected_root=inputs["result_root"] / "custody-cas"
    )
    assert (
        cache.reclaim_terminal_unsealed(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            projection=projection,
        )["idempotent"]
        is True
    )


@pytest.mark.parametrize("newer_generation", [False, True])
def test_reclaim_reentry_converges_only_the_current_pointer(
    tmp_path, monkeypatch, newer_generation
):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "partial").write_bytes(b"partial")
    publication = lease.publish({"phase": "failed"})
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    update_pointer = cache._update_pointer_if_current

    def fail_reclaimed_pointer(pointer, **kwargs):
        if kwargs.get("state") == "reclaimed":
            raise OSError("simulated pointer publication failure")
        return update_pointer(pointer, **kwargs)

    monkeypatch.setattr(cache, "_update_pointer_if_current", fail_reclaimed_pointer)
    with pytest.raises(OSError, match="pointer publication failure"):
        cache.reclaim_terminal_unsealed(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            projection=projection,
        )
    assert not lease.target.exists()
    owner = cache.loads_exact(lease.owner_path.read_text(encoding="utf-8"))
    assert owner["lifecycle"] == "reclaimed"

    monkeypatch.setattr(cache, "_update_pointer_if_current", update_pointer)
    if newer_generation:
        inputs.update(run_id="unit-two", execution_nonce_sha256="d" * 64)
        newer = cache.acquire(**inputs)
        newer.close()
        pointer_before = newer.pointer.read_bytes()
    outcome = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )

    assert outcome["state"] == "reclaimed"
    assert outcome["idempotent"] is True
    pointer = cache.loads_exact(lease.pointer.read_text(encoding="utf-8"))
    if newer_generation:
        assert lease.pointer.read_bytes() == pointer_before
        assert pointer["target"] == str(newer.target)
    else:
        assert pointer["state"] == "reclaimed"
        assert pointer["target"] == str(lease.target)


def test_interrupted_completed_delete_reentry_converges_current_pointer(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    artifact = lease.target / "partial"
    artifact.write_bytes(b"partial")
    publication = lease.publish({"phase": "failed"})
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    owner = cache.loads_exact(lease.owner_path.read_text(encoding="utf-8"))
    owner.update(lifecycle="reclaiming", reclaim_started_at=cache._utc_now())
    cache._write_owner(lease.owner_path, owner)
    artifact.unlink()
    lease.target.rmdir()

    outcome = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )

    assert outcome["state"] == "reclaimed"
    assert outcome["idempotent"] is True
    pointer = cache.loads_exact(lease.pointer.read_text(encoding="utf-8"))
    assert pointer["state"] == "reclaimed"
    assert pointer["target"] == str(lease.target)


def test_sealed_terminal_generation_is_retained_and_never_reclaimed(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "artifact.rlib").write_bytes(b"sealed")
    publication = lease.publish(_completed(lease))
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    assert projection["state"] == "terminal-sealed-retained"
    pointer = cache.loads_exact(lease.pointer.read_text(encoding="utf-8"))
    assert pointer["state"] == "sealed"
    assert pointer["seal"] == publication["seal"]
    inspection = cache.inspect_terminal_generation(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert inspection["state"] == "terminal-sealed-retained"
    assert inspection["reclaim_eligible"] is False
    assert inspection["reclaim_reason"] == "owner-lifecycle-not-reclaimable"
    result = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert result["state"] == "not-reclaimable"
    assert lease.target.is_dir()


def test_failed_sealed_terminal_generation_retires_with_receipt_and_timing_evidence(
    tmp_path,
):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "artifact.rlib").write_bytes(b"sealed failed output")
    timings = lease.target / "cargo-timings"
    timings.mkdir()
    (timings / "cargo-timing.html").write_bytes(b"timing evidence")
    publication = lease.publish(_completed(lease, returncode=101))
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )

    inspection = cache.inspect_terminal_sealed_retirement(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert inspection["retirement_eligible"] is True
    assert inspection["retirement_reason"] == "terminal-sealed-failed-cleanup-safe"
    assert inspection["terminal_status"] == "failed"
    assert inspection["retirement_policy"] == {
        "allow_passed": False,
        "eligible_terminal_statuses": ["failed"],
    }
    retired = cache.retire_terminal_sealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )

    assert retired["state"] == "retired-sealed"
    assert not lease.target.exists()
    owner = cache.loads_exact(lease.owner_path.read_text(encoding="utf-8"))
    assert owner["lifecycle"] == "retired-sealed"
    assert owner["terminal_receipt"] == projection["terminal_receipt"]
    assert owner["retirement"]["retirement_policy"] == inspection["retirement_policy"]
    assert owner["retirement"]["terminal_status"] == "failed"
    cache.validate_terminal_receipt(
        result_root=inputs["result_root"],
        projection=projection,
        provenance=lease.provenance,
        run_id="unit-one",
        execution_nonce_sha256="a" * 64,
    )
    evidence = custody_cas.read_ref(
        retired["evidence"], expected_root=inputs["result_root"] / "custody-cas"
    )
    assert evidence["kind"] == cache._SEALED_RETIREMENT_EVIDENCE_KIND
    assert evidence["retirement_policy"] == inspection["retirement_policy"]
    assert evidence["terminal_status"] == "failed"
    timing_payload = custody_cas.read_ref(
        evidence["timings"], expected_root=inputs["result_root"] / "custody-cas"
    )
    custody_cas.verify_file_ref(
        timing_payload["files"][0]["file"],
        expected_root=inputs["result_root"] / "custody-cas",
    )
    pointer = cache.loads_exact(lease.pointer.read_text(encoding="utf-8"))
    assert pointer["state"] == "retired-sealed"

    inputs.update(run_id="unit-two", execution_nonce_sha256="d" * 64)
    successor = cache.acquire(**inputs)
    try:
        assert successor.provenance["cold_reason"] == "prior-generation-retired-sealed"
    finally:
        successor.close()


def test_successful_sealed_terminal_generation_is_not_retirable(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "artifact.rlib").write_bytes(b"sealed success")
    publication = lease.publish(_completed(lease))
    lease.close()
    receipt = _terminal_receipt(lease, publication)
    receipt["queue_terminal"] = {
        "schema": supervisor_custody.QUEUE_TERMINAL_SCHEMA,
        "status": "passed",
        "returncode": 0,
        "command_returncode": 0,
        "execution_error": None,
    }
    receipt["command_returncode"] = 0
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=receipt,
    )

    inspection = cache.inspect_terminal_sealed_retirement(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert inspection["retirement_eligible"] is False
    assert inspection["retirement_reason"] == "terminal-run-not-failed"
    assert inspection["terminal_status"] == "passed"
    assert inspection["retirement_policy"] == {
        "allow_passed": False,
        "eligible_terminal_statuses": ["failed"],
    }
    outcome = cache.retire_terminal_sealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert outcome["state"] == "not-retirable"
    assert outcome["retirement_policy"] == inspection["retirement_policy"]
    assert outcome["terminal_status"] == "passed"
    assert lease.target.is_dir()


def test_explicit_policy_retires_successful_nonreusable_seal_under_lock(
    tmp_path, monkeypatch
):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "artifact.rlib").write_bytes(b"sealed success")
    timings = lease.target / "cargo-timings"
    timings.mkdir()
    (timings / "cargo-timing.html").write_bytes(b"successful timing evidence")
    publication = lease.publish(_completed(lease))
    lease.close()
    receipt = _terminal_receipt(lease, publication)
    receipt["queue_terminal"] = {
        "schema": supervisor_custody.QUEUE_TERMINAL_SCHEMA,
        "status": "passed",
        "returncode": 0,
        "command_returncode": 0,
        "execution_error": None,
    }
    receipt["command_returncode"] = 0
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=receipt,
    )

    inspection = cache.inspect_terminal_sealed_retirement(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
        allow_passed=True,
    )
    policy = {
        "allow_passed": True,
        "eligible_terminal_statuses": ["failed", "passed"],
    }
    assert inspection["retirement_eligible"] is True
    assert inspection["retirement_reason"] == "terminal-sealed-passed-cleanup-safe"
    assert inspection["retirement_policy"] == policy
    assert inspection["terminal_status"] == "passed"

    lock_held = False
    original_acquire = cache._acquire_file_lock
    original_release = cache._release_file_lock
    original_rejection = cache._sealed_retirement_rejection

    def tracked_acquire(*args, **kwargs):
        nonlocal lock_held
        handle = original_acquire(*args, **kwargs)
        lock_held = True
        return handle

    def tracked_release(handle):
        nonlocal lock_held
        try:
            original_release(handle)
        finally:
            lock_held = False

    def checked_rejection(**kwargs):
        assert lock_held is True
        assert kwargs["allow_passed"] is True
        return original_rejection(**kwargs)

    monkeypatch.setattr(cache, "_acquire_file_lock", tracked_acquire)
    monkeypatch.setattr(cache, "_release_file_lock", tracked_release)
    monkeypatch.setattr(cache, "_sealed_retirement_rejection", checked_rejection)
    retired = cache.retire_terminal_sealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
        allow_passed=True,
    )

    assert lock_held is False
    assert retired["state"] == "retired-sealed"
    assert retired["retirement_policy"] == policy
    assert retired["terminal_status"] == "passed"
    assert not lease.target.exists()
    owner = cache.loads_exact(lease.owner_path.read_text(encoding="utf-8"))
    assert owner["lifecycle"] == "retired-sealed"
    assert owner["terminal_receipt"] == projection["terminal_receipt"]
    assert owner["retirement"]["retirement_policy"] == policy
    assert owner["retirement"]["terminal_status"] == "passed"
    evidence = custody_cas.read_ref(
        retired["evidence"], expected_root=inputs["result_root"] / "custody-cas"
    )
    assert evidence["retirement_policy"] == policy
    assert evidence["terminal_status"] == "passed"
    assert evidence["terminal_receipt"] == projection["terminal_receipt"]
    assert evidence["output_manifest"]["file_count"] == 2
    timing_payload = custody_cas.read_ref(
        evidence["timings"], expected_root=inputs["result_root"] / "custody-cas"
    )
    custody_cas.verify_file_ref(
        timing_payload["files"][0]["file"],
        expected_root=inputs["result_root"] / "custody-cas",
    )
    cache.validate_terminal_receipt(
        result_root=inputs["result_root"],
        projection=projection,
        provenance=lease.provenance,
        run_id="unit-one",
        execution_nonce_sha256="a" * 64,
    )
    pointer = cache.loads_exact(lease.pointer.read_text(encoding="utf-8"))
    assert pointer["state"] == "retired-sealed"


@pytest.mark.parametrize(
    "mutation,expected",
    [
        ("reusable", "publication-may-be-reusable"),
        ("unsafe", "terminal-receipt-not-cleanup-safe"),
        ("nonterminal", "terminal-run-not-failed"),
        ("absent", "target-absent"),
    ],
)
def test_allow_passed_does_not_bypass_sealed_retirement_custody(mutation, expected):
    owner = {"lifecycle": "terminal-sealed-retained"}
    projection = {"state": "terminal-sealed-retained"}
    receipt = {
        "process_cleanup_safe": True,
        "queue_terminal": {"status": "passed"},
    }
    publication = {
        "state": "sealed",
        "purpose": "preserved-candidate",
        "reusable": False,
        "reuse_rejection_code": "cargo-input-closure-unproven",
    }
    target_present = True
    if mutation == "reusable":
        publication["reusable"] = True
    elif mutation == "unsafe":
        receipt["process_cleanup_safe"] = False
    elif mutation == "nonterminal":
        receipt["queue_terminal"]["status"] = "running"
    else:
        target_present = False
    assert (
        cache._sealed_retirement_rejection(
            owner=owner,
            projection=projection,
            receipt=receipt,
            publication=publication,
            target_present=target_present,
            allow_passed=True,
        )
        == expected
    )


def test_failed_sealed_retirement_rejects_output_drift_from_immutable_seal(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    artifact = lease.target / "artifact.rlib"
    artifact.write_bytes(b"sealed failed output")
    publication = lease.publish(_completed(lease, returncode=101))
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    artifact.write_bytes(b"mutated after seal")

    outcome = cache.retire_terminal_sealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert outcome["state"] == "retire-blocked"
    assert "differs from its immutable seal" in outcome["error"]
    assert lease.target.is_dir()


def test_failed_sealed_retirement_failure_is_persistently_blocked_without_retry(
    tmp_path, monkeypatch
):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "artifact.rlib").write_bytes(b"sealed failed output")
    publication = lease.publish(_completed(lease, returncode=101))
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    calls = 0

    def denied(path):
        nonlocal calls
        calls += 1
        return False, "simulated open target handle"

    monkeypatch.setattr(cache, "delete_path", denied)
    first = cache.retire_terminal_sealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    second = cache.retire_terminal_sealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert first["state"] == second["state"] == "retire-blocked"
    assert second["reason"] == "prior-retirement-failure"
    assert calls == 1
    assert lease.target.is_dir()


def test_inspection_is_read_only_and_does_not_hash_target(tmp_path, monkeypatch):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "partial").write_bytes(b"partial")
    publication = lease.publish({"phase": "failed"})
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    paths_before = sorted(
        str(path.relative_to(inputs["result_root"]))
        for path in inputs["result_root"].rglob("*")
    )
    owner_before = lease.owner_path.read_bytes()

    def forbidden(*args, **kwargs):
        pytest.fail("inspection must not lock or hash target output")

    monkeypatch.setattr(cache, "_acquire_file_lock", forbidden)
    monkeypatch.setattr(command_identity, "_directory_manifest_identity", forbidden)
    inspection = cache.inspect_terminal_generation(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )

    assert inspection["reclaim_eligible"] is True
    assert lease.owner_path.read_bytes() == owner_before
    assert (
        sorted(
            str(path.relative_to(inputs["result_root"]))
            for path in inputs["result_root"].rglob("*")
        )
        == paths_before
    )


def test_persisted_terminal_projection_is_required_for_inspect_and_reclaim(
    tmp_path, monkeypatch
):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "partial").write_bytes(b"partial")
    publication = lease.publish({"phase": "failed"})
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    substituted_receipt = _terminal_receipt(lease, publication)
    substituted_receipt["guard_receipt"] = {"sha256": "d" * 64}
    substituted_reference = cache._artifact(
        inputs["result_root"] / "custody-cas",
        cache._TERMINAL_RECEIPT_KIND,
        receipt=substituted_receipt,
    )
    owner = cache.loads_exact(lease.owner_path.read_text(encoding="utf-8"))
    owner["terminal_receipt"] = substituted_reference
    cache._write_owner(lease.owner_path, owner)

    def forbidden_delete(path):
        pytest.fail("projection mismatch must be rejected before deletion")

    monkeypatch.setattr(cache, "delete_path", forbidden_delete)
    with pytest.raises(ValueError, match="persisted terminal projection"):
        cache.inspect_terminal_generation(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            projection=projection,
        )
    with pytest.raises(ValueError, match="persisted terminal projection"):
        cache.reclaim_terminal_unsealed(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            projection=projection,
        )
    assert lease.target.is_dir()


def test_unvalidated_failure_terminal_is_indeterminate_and_retained(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "partial").write_bytes(b"partial")
    publication = lease.publish({"phase": "failed"})
    lease.close()
    receipt = _terminal_receipt(lease, publication, process_cleanup_safe=False)
    receipt.pop("execution_custody_sha256")
    receipt.pop("guard_receipt")
    receipt.pop("process_supervisor")
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=receipt,
    )
    assert projection["state"] == "terminal-indeterminate-retained"
    assert (
        cache.reclaim_terminal_unsealed(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            projection=projection,
        )["state"]
        == "not-reclaimable"
    )
    assert lease.target.is_dir()


def test_reclaim_failure_is_persistently_blocked_without_retry(tmp_path, monkeypatch):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "partial").write_bytes(b"partial")
    publication = lease.publish({"phase": "failed"})
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    calls = 0

    def denied(path):
        nonlocal calls
        calls += 1
        return False, "simulated policy denial"

    monkeypatch.setattr(cache, "delete_path", denied)
    first = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    second = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert first["state"] == second["state"] == "reclaim-blocked"
    assert second["reason"] == "prior-reclaim-failure"
    assert calls == 1
    assert lease.target.is_dir()


@pytest.mark.parametrize("cas_mutation", ["missing", "tampered"])
def test_invalid_terminal_receipt_cas_blocks_reclaim_without_deleting(
    tmp_path, cas_mutation
):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "partial").write_bytes(b"partial")
    publication = lease.publish({"phase": "failed"})
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    receipt_path = Path(projection["terminal_receipt"]["path"])
    if cas_mutation == "missing":
        receipt_path.unlink()
    else:
        receipt_path.write_bytes(b"tampered")
    with pytest.raises(FileNotFoundError if cas_mutation == "missing" else ValueError):
        cache.reclaim_terminal_unsealed(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            projection=projection,
        )
    owner = cache.loads_exact(lease.owner_path.read_text(encoding="utf-8"))
    assert owner["lifecycle"] == "terminal-unsealed-reclaimable"
    assert lease.target.is_dir()


def test_timing_copy_aba_is_blocked_before_reclaim(tmp_path, monkeypatch):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    timings = lease.target / "cargo-timings"
    timings.mkdir()
    timing = timings / "cargo-timing.html"
    timing.write_bytes(b"original timing")
    publication = lease.publish({"phase": "failed"})
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    put_file = custody_cas.put_file

    def copy_transient_bytes(root, source, **kwargs):
        original = source.read_bytes()
        source.write_bytes(b"transient timing")
        try:
            return put_file(root, source, **kwargs)
        finally:
            source.write_bytes(original)

    monkeypatch.setattr(custody_cas, "put_file", copy_transient_bytes)
    result = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert result["state"] == "reclaim-blocked"
    assert "changed while being copied" in result["error"]
    assert lease.target.is_dir()


def test_owner_nonce_substitution_cannot_reclaim(tmp_path):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "partial").write_bytes(b"partial")
    publication = lease.publish({"phase": "failed"})
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    owner = cache.loads_exact(lease.owner_path.read_text(encoding="utf-8"))
    owner["execution_nonce_sha256"] = "f" * 64
    lease.owner_path.write_text(json.dumps(owner), encoding="utf-8")
    with pytest.raises(ValueError, match="owner/provenance"):
        cache.reclaim_terminal_unsealed(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            projection=projection,
        )
    assert lease.target.is_dir()


def test_owner_publication_write_failure_releases_lock_and_preserves_outcome(
    tmp_path, monkeypatch
):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "artifact.rlib").write_bytes(b"sealed")
    write_owner = cache._write_owner
    failed = False

    def fail_once(path, owner):
        nonlocal failed
        if not failed and owner.get("lifecycle") == "awaiting-terminal-receipt":
            failed = True
            raise OSError("simulated full disk")
        write_owner(path, owner)

    monkeypatch.setattr(cache, "_write_owner", fail_once)
    with pytest.raises(OSError, match="simulated full disk"):
        lease.publish(_completed(lease))
    assert lease.publication_outcome["state"] == "sealed"
    lease.close()
    persisted = cache.loads_exact(lease.owner_path.read_text(encoding="utf-8"))
    assert persisted["publication"] == lease.publication_outcome
    inputs.update(run_id="unit-two", execution_nonce_sha256="d" * 64)
    second = cache.acquire(**inputs)
    second.close()


def test_legacy_inventory_is_read_only_and_fail_closed(tmp_path):
    result_root = tmp_path / "proofs"
    legacy = result_root / "cargo-cache" / ("a" * 64) / ("b" * 16) / "target"
    legacy.mkdir(parents=True)
    (legacy / "artifact").write_bytes(b"historical")
    pointer = legacy.parent.parent / "state.json"
    pointer.write_text(
        '{"state":"pending","run_id":"guessed-owner"}\n', encoding="utf-8"
    )

    inventory = cache.inventory_legacy_generations(result_root)
    assert inventory == [
        {
            "state": "legacy-owner-absent-retained",
            "input_sha256": "a" * 64,
            "generation_id": "b" * 16,
            "target": str(legacy),
            "reason": "persistent per-generation owner metadata is absent",
        }
    ]
    assert legacy.is_dir()
    assert not (legacy.parent / "owner.json").exists()
