from __future__ import annotations

from decimal import Decimal

import copy
from dataclasses import replace
import hashlib
import json
import os
import sys
from pathlib import Path

import pytest
from molt.process_environment import NameRule, ProcessEnvironment
from tests.test_python_external_custody import standard_finders  # noqa: F401
from tests.process_guard_common import run_guarded_test_process
from tools.git_identity import repository_source_paths

from tools.proof_queue_pkg import cargo_cache_custody as cache
from tools.proof_queue_pkg import (
    custody_cas,
    execution_custody,
    execution_environment,
    command_identity,
    execution_receipt_details,
    supervisor_custody,
    cargo_output_environment,
    command_admission,
)


pytestmark = pytest.mark.usefixtures("cargo_output_implementation_source")


@pytest.fixture(autouse=True)
def source_admission(monkeypatch):
    """Model neighbors' space; retain the real probe for actual Cargo oracles."""
    actual_free_space = cache.disk_capacity._default_measure_free_bytes
    monkeypatch.setattr(
        execution_environment,
        "repository_source_paths",
        lambda root, *, run_git, include_untracked=False: [root / "main.rs"],
    )
    monkeypatch.setattr(
        cache.disk_capacity,
        "_default_measure_free_bytes",
        lambda path: 1 << 50,
    )
    return actual_free_space


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


@pytest.mark.parametrize("fault", ["missing-appears", "external-hardlink"])
def test_git_source_capture_retains_missing_and_alias_custody(
    tmp_path, monkeypatch, fault
):
    # Exercise the real shared Git inventory and the existing capture owner,
    # rather than the synthetic single-file inventory used by Cargo fixtures.
    monkeypatch.setattr(
        execution_environment, "repository_source_paths", repository_source_paths
    )
    root = tmp_path / "source with spaces"
    root.mkdir()
    for arguments in (
        ("init", "--quiet"),
        ("add", "main.rs", "missing.rs", ".gitignore"),
    ):
        if arguments[0] == "add":
            (root / "main.rs").write_bytes(b"fn main() {}\n")
            (root / "missing.rs").write_bytes(b"missing input\n")
            (root / ".gitignore").write_text("ignored-cache/\n", encoding="utf-8")
        run_guarded_test_process(["git", *arguments], cwd=root, check=True, timeout=30)
    (root / "missing.rs").unlink()
    ignored = root / "ignored-cache"
    ignored.mkdir()
    (ignored / "mutable").write_bytes(b"not source")
    if fault == "external-hardlink":
        os.link(root / "main.rs", tmp_path / "unadmitted-alias")
        with pytest.raises(ValueError, match="aliases outside admitted source"):
            execution_environment.capture_source_content(
                source_root=root,
                env=dict(os.environ),
                overlays=(),
                cas_root=tmp_path / "cas",
                hash_workers=1,
            )
        return
    reference, telemetry, verify = execution_environment.capture_source_content(
        source_root=root,
        env=dict(os.environ),
        overlays=(),
        cas_root=tmp_path / "cas",
        hash_workers=1,
    )
    payload = custody_cas.read_ref(reference, expected_root=tmp_path / "cas")
    assert payload["missing"] == ["missing.rs"]
    assert {row["path"] for row in payload["files"]} == {"main.rs", ".gitignore"}
    assert telemetry["file_count"] == 2
    (ignored / "mutable").write_bytes(b"still not source")
    verify()
    (root / "missing.rs").write_bytes(b"appeared\n")
    with pytest.raises(ValueError, match="missing source input appeared"):
        verify()


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


def test_output_lifetime_is_provenance_not_compilation_input_identity(tmp_path):
    inputs = _inputs(tmp_path)
    first = cache.acquire(**inputs)
    first.close()
    second = cache.acquire(**inputs, cargo_output_lifetime="terminal-success")
    try:
        assert first.provenance["input_sha256"] == second.provenance["input_sha256"]
        assert first.provenance["cargo_output_lifetime"] == "retain"
        assert second.provenance["cargo_output_lifetime"] == "terminal-success"
        arguments = {
            key: inputs[key]
            for key in (
                "command",
                "outputs",
                "toolchains",
                "source_snapshot",
                "source_content",
            )
        }
        arguments.update(
            cas_root=inputs["result_root"] / "custody-cas",
            env=second.environment,
            source_root=str(inputs["source_root"]),
        )
        with pytest.raises(ValueError, match="output lifetime differs"):
            cache.validate_prelaunch(second.provenance, **arguments)
        cache.validate_prelaunch(
            second.provenance, **arguments, cargo_output_lifetime="terminal-success"
        )
    finally:
        second.close()


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


@pytest.mark.parametrize("rule", list(NameRule))
def test_cache_transport_filter_uses_child_environment_rule(rule):
    command = ["cargo", "test"]
    arguments = {
        "source": {},
        "toolchains": {},
        "command": command,
        "outputs": _outputs(command),
    }
    base = cache.input_identity(
        **arguments, env=ProcessEnvironment({"RUSTFLAGS": "flags"}, rule=rule)
    )
    changed = cache.input_identity(
        **arguments,
        env=ProcessEnvironment(
            {"RUSTFLAGS": "flags", "molt_guard_scratch_root": "semantic-on-posix"},
            rule=rule,
        ),
    )
    assert (base == changed) is (rule is NameRule.WINDOWS)


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
        assert scratch.guard_scratch(selected) == lease.target
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
            lease,
            closed=True,
            success=True,
            evidence={"authority": "fixture"},
            timings=scratch.ScratchFinalizationTimings(),
        )


@pytest.mark.parametrize(
    "name",
    [
        "TMPDIR",
        "TMP",
        "TEMP",
        "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
        "MOLT_BACKEND_MAX_PROCESS_RSS_GB",
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
    inputs["env"].update(
        TEMP="caller-temp",
        TMP="caller-tmp",
        TMPDIR="caller-tmpdir",
        MOLT_CARGO_BUILD_DIR_BASE=str(tmp_path / "unleased-intermediates"),
    )
    original = dict(inputs["env"])
    lease = cache.acquire(**inputs)
    try:
        outputs = inputs["outputs"]
        assert isinstance(lease.environment, ProcessEnvironment)
        assert lease.environment.rule is NameRule.host()
        assert lease.environment.child_block() == outputs.bind(
            original, target=lease.target
        )
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


@pytest.mark.parametrize(
    "name",
    [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        "MOLT_CARGO_BUILD_DIR_BASE",
        "TEMP",
        "TMP",
        "TMPDIR",
    ],
)
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
        env={"TEMP": "one", "RUSTFLAGS": "one", "MOLT_BACKEND_MAX_PROCESS_RSS_GB": "4"},
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
    assert outputs.names == (
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        "MOLT_CARGO_BUILD_DIR_BASE",
    )
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
        envelope, admitted_argv, environment_root=Path(sys.prefix)
    )
    assert command_admission._nested_command(execution_command) is None
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
        child_process_custody={
            "receipt": {
                "schema": execution_custody.CHILD_CUSTODY_RECEIPT_SCHEMA,
                "broker_complete": True,
                "events": [],
                "errors": [],
                "violations": [],
            }
        },
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
        **cache.cargo_output_layout.target_layout_fields(lease.provenance),
        **(
            {"cargo_output_root": lease.provenance["cargo_output_root"]}
            if "cargo_output_root" in lease.provenance
            else {}
        ),
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


@pytest.mark.parametrize("external", [False, True])
@pytest.mark.parametrize("sealed", [False, True])
def test_historical_target_lifecycle_preserves_receipts_and_new_acquisition_is_compact(
    tmp_path, monkeypatch, external, sealed
):
    inputs = _inputs(tmp_path)
    layout = cache.cargo_output_layout
    if external:
        output = tmp_path / "volume"
        output.mkdir()
        inputs["cargo_output_root"] = layout.declare_root(str(output))
        inputs["outputs"] = (
            cargo_output_environment.CargoOutputEnvironment.for_envelope(
                command_admission.envelope_for_command(
                    inputs["command"], cargo_output_root=str(output)
                )
            )
        )
    lease = cache.acquire(**inputs)
    # Construct a historical fixture; production never migrates old payloads
    # or rewrites their immutable evidence. Preserve the real lifecycle path.
    historical = layout.CargoOutputLayout.create(
        result_root=inputs["result_root"], declaration=inputs.get("cargo_output_root")
    ).target(
        lease.provenance["input_sha256"],
        lease.provenance["generation_id"],
        version=layout.recorded_target_layout({}),
    )
    historical.parent.mkdir(parents=True, exist_ok=True)
    lease.target.rename(historical)
    lease.target = historical
    lease.provenance.pop("cargo_target_layout")
    lease.provenance.update(path=str(historical), effective_target=str(historical))
    lease.owner.pop("cargo_target_layout")
    lease.owner["target"] = str(historical)
    lease.environment = inputs["outputs"].bind(inputs["env"], target=historical)
    cache._write_owner(lease.owner_path, lease.owner)
    pointer = cache.loads_exact(lease.pointer.read_text(encoding="utf-8"))
    pointer.pop("cargo_target_layout")
    pointer["target"] = str(historical)
    cache._atomic_json(lease.pointer, pointer)
    (historical / "artifact").write_bytes(b"historical output")
    intermediate = historical / ".molt-toolchains" / ("a" * 64) / "build" / "out"
    intermediate.mkdir(parents=True)
    (intermediate / "generated.rs").write_bytes(b"captured resource output")
    publication = lease.publish(_completed(lease) if sealed else {"phase": "failed"})
    lease.close()
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    assert "cargo_target_layout" not in projection
    immutable = Path(projection["terminal_receipt"]["path"])
    before = immutable.read_bytes()
    if sealed:
        with pytest.raises(cache.CargoInputClosureUnproven) as rejected:
            cache.acquire(**inputs)
        assert rejected.value.diagnostic["candidate_path"] == str(historical)
    dispose = (
        cache.retire_terminal_sealed if sealed else cache.reclaim_terminal_unsealed
    )
    result = dispose(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert result["state"] == ("retired-sealed" if sealed else "reclaimed")
    assert not historical.exists()
    assert immutable.read_bytes() == before
    inputs.update(run_id="successor", execution_nonce_sha256="d" * 64)
    successor = cache.acquire(**inputs)
    successor.close()
    assert successor.provenance["cargo_target_layout"] == layout.TARGET_LAYOUT
    assert successor.target.parent.name == "cargo-target"
    assert "cargo_target_layout" not in lease.provenance

    # Historical terminal verification is independent of mounted output media.
    def offline(value):
        raise OSError("output media offline")

    monkeypatch.setattr(layout, "validate_root", offline)
    assert cache.validate_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
        run_id="unit-one",
        execution_nonce_sha256="a" * 64,
    )["target"] == str(historical)


@pytest.mark.parametrize("phase", ["terminal", "reclaim", "retire"])
@pytest.mark.parametrize(
    "field,value",
    [
        ("cargo_target_layout", "molt.proof-cargo-target.v1"),
        ("cargo_target_layout", None),
        ("generation_owner", "foreign-owner"),
        ("cargo_output_root", {}),
        ("run_id", "foreign-run"),
    ],
)
def test_pointer_binding_precedes_terminal_mutation_or_deletion(
    tmp_path, phase, field, value
):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    (lease.target / "keep").write_bytes(b"must not delete")
    publication = lease.publish(
        _completed(lease) if phase == "retire" else {"phase": "failed"}
    )
    lease.close()
    terminal = dict(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )
    projection = (
        cache.record_terminal_receipt(**terminal) if phase != "terminal" else None
    )
    pointer = cache.loads_exact(lease.pointer.read_text(encoding="utf-8"))
    if value is None:
        pointer.pop(field)
    else:
        pointer[field] = value
    cache._atomic_json(lease.pointer, pointer)
    before_pointer = lease.pointer.read_bytes()
    before_owner = lease.owner_path.read_bytes()
    with pytest.raises(ValueError, match="binding mismatch"):
        if phase == "terminal":
            cache.record_terminal_receipt(**terminal)
        else:
            dispose = (
                cache.retire_terminal_sealed
                if phase == "retire"
                else cache.reclaim_terminal_unsealed
            )
            dispose(
                result_root=inputs["result_root"],
                provenance=lease.provenance,
                projection=projection,
            )
    assert (lease.target / "keep").read_bytes() == b"must not delete"
    assert lease.pointer.read_bytes() == before_pointer
    assert lease.owner_path.read_bytes() == before_owner


def test_compact_target_requires_exclusive_creation_even_if_empty(
    tmp_path, monkeypatch
):
    inputs = _inputs(tmp_path)
    derive = cache.cargo_output_layout.CargoOutputLayout.target
    targets = []

    def preexisting(self, *args, **kwargs):
        target = derive(self, *args, **kwargs)
        target.mkdir(parents=True)
        targets.append(target)
        return target

    monkeypatch.setattr(
        cache.cargo_output_layout.CargoOutputLayout, "target", preexisting
    )
    with pytest.raises(FileExistsError):
        cache.acquire(**inputs)
    assert targets and targets[0].is_dir()
    assert not list(targets[0].iterdir())


@pytest.mark.parametrize("missing", [False, True])
def test_active_publication_requires_its_current_pointer(tmp_path, missing):
    inputs = _inputs(tmp_path)
    lease = cache.acquire(**inputs)
    try:
        pointer = cache.loads_exact(lease.pointer.read_text(encoding="utf-8"))
        if missing:
            lease.pointer.unlink()
        else:
            pointer["target"] = str(tmp_path / "foreign")
            cache._atomic_json(lease.pointer, pointer)
        before = None if missing else lease.pointer.read_bytes()
        owner_before = lease.owner_path.read_bytes()
        with pytest.raises(ValueError, match="lost its current pointer"):
            lease.publish(_completed(lease))
        assert lease.owner_path.read_bytes() == owner_before
        assert not lease.published
        if missing:
            assert not lease.pointer.exists()
        else:
            assert lease.pointer.read_bytes() == before
    finally:
        lease.close()


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
        "denied-child",
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
        elif failure == "denied-child":
            expanded = execution_receipt_details.expand_context(
                context, cas_root=first.cas_root
            )
            child = expanded["child_process_custody"]["receipt"]
            child["events"] = [{"event": "child-process", "admitted": False}]
            child["violations"] = list(child["events"])
            result["receipt_context"] = execution_receipt_details.compact_context(
                expanded, cas_root=first.cas_root
            )
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
        original = lease.environment
        forged = original.updated({"RUSTFLAGS": "changed"})
        assert lease.environment is original
        assert forged.rule is original.rule
        assert forged["RUSTFLAGS"] == "changed"
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
    bind = PythonFileCaptureContext.bind_many

    def mutate(self, *args, **kwargs):
        result = bind(self, *args, **kwargs)
        if mutation == "add":
            (root / "added").mkdir()
        else:
            (root / "empty").rmdir()
        return result

    monkeypatch.setattr(PythonFileCaptureContext, "bind_many", mutate)
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


@pytest.mark.parametrize("failure_point", ["before-observation", "owner-write"])
def test_failed_publication_owner_write_keeps_structured_secondary_without_primary_callbacks(
    monkeypatch, tmp_path, failure_point
):
    def _hostile():
        called = []

        class Meta(type):
            @property
            def __name__(cls):
                called.append("name")
                raise RuntimeError("name callback")

        class Error(Exception, metaclass=Meta):
            @property
            def __notes__(self):
                called.append("notes")
                raise RuntimeError("notes callback")

            def __str__(self):
                called.append("str")
                raise RuntimeError("str callback")

        return Error("original"), called

    def _raise(error):
        raise error

    primary, calls = _hostile()
    cleanup, cleanup_calls = _hostile()
    monkeypatch.setattr(cache.cargo_output_layout, "validate_root", lambda *args: None)
    monkeypatch.setattr(cache, "_read_current_pointer", lambda *args, **kwargs: {})
    monkeypatch.setattr(cache, "_complete_custody", lambda *args, **kwargs: True)
    monkeypatch.setattr(
        cache.command_identity,
        "_directory_manifest_identity",
        lambda *args, **kwargs: _raise(primary),
    )
    lease = cache.CargoCacheLease(
        tmp_path,
        tmp_path,
        tmp_path,
        tmp_path,
        None,
        {"input_sha256": "input"},
        {},
        ProcessEnvironment({}),
    )
    publications = []

    def fail_publication(payload):
        publications.append(payload)
        raise cleanup

    persist_publication = lease._persist_publication
    persisted_owners = []

    def fail_owner_write(path, owner):
        publications.append(dict(owner["publication"]))
        raise cleanup

    if failure_point == "before-observation":
        monkeypatch.setattr(lease, "_persist_publication", fail_publication)
    else:
        monkeypatch.setattr(cache, "_write_owner", fail_owner_write)
    with pytest.raises(type(primary)) as caught:
        lease.publish({})
    assert caught.value is primary and publications[0]["state"] == "unsealed"
    assert publications[0]["error"] == "Error: original"
    assert lease.publication_outcome == {
        "state": "unsealed",
        "reason": "publication-failed",
        "error": "Error: original",
        "owner_persistence": {"state": "failed", "diagnostic": "Error: original"},
    }
    assert lease.owner_persisted is False
    assert lease.owner["publication"] == lease.publication_outcome
    assert (
        list(
            dict.items(
                BaseException.__dict__["__dict__"].__get__(primary, BaseException)
            )
        )
        == []
    )
    assert calls == cleanup_calls == []

    # The close retry must serialize the same failed outcome, including the
    # secondary failure, rather than replacing or losing its owned evidence.
    monkeypatch.setattr(lease, "_persist_publication", persist_publication)
    monkeypatch.setattr(
        cache, "_write_owner", lambda path, owner: persisted_owners.append(dict(owner))
    )
    monkeypatch.setattr(cache, "_release_file_lock", lambda lock: None)
    lease.close()
    assert persisted_owners[0]["publication"] == lease.publication_outcome
    assert lease.owner_persisted is True
    assert lease.closed is True
    assert calls == cleanup_calls == []


@pytest.mark.parametrize(
    "operation,member",
    [
        ("c", "selected-gcc"),
        ("c", "cc1"),
        ("c", "as"),
        ("c", "selected-ar"),
        ("c++", "selected-g++"),
        ("c++", "cc1plus"),
    ],
)
def test_native_build_process_closure_participates_in_cargo_cache_inputs(
    tmp_path, monkeypatch, operation, member
):
    from tests.tools.test_toolchain_capture import _native_build_capture_fixture
    from tools.proof_queue_pkg import toolchain_capture

    identity, tools, env, command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, operation=operation
    )
    arguments = {
        "source": {"root": str(tmp_path), "commit": "fixed"},
        "toolchains": {"rustc": identity},
        "command": command,
        "outputs": _outputs(command),
        "env": env,
    }
    arguments["outputs"] = replace(arguments["outputs"], cache_domain="cargo-workspace")
    before = cache.input_identity(**arguments)
    tools[member].write_bytes(b"replacement C build input")
    # A new generation must come from the actual capture producer, including
    # helper transcripts, rather than resealing stale closure metadata.
    images, selection = toolchain_capture.capture_rust_link_process_images(
        rustc=tools["rustc"],
        cargo=tools["cargo"],
        cwd=tmp_path,
        env=env,
        target=None,
        rustc_version="rustc 1.99.0\nhost: x86_64-unknown-linux-gnu\n",
        command_argv=command,
        admitted_command=command,
        native_units={"target": ["c"]} if operation == "c" else {"host": ["c++"]},
    )
    refreshed = dict(
        identity,
        process_images=[identity["process_images"][0], *images],
        link_selection=selection,
    )
    refreshed.pop("identity_sha256")
    refreshed["identity_sha256"] = cache.canonical_json_sha256(refreshed)
    arguments["toolchains"] = {"rustc": refreshed}
    after = cache.input_identity(**arguments)
    assert after != before
    assert cache._workspace_identity(after) != cache._workspace_identity(before)


def test_selected_ignored_differential_source_has_replay_capture_and_terminal_fences(
    tmp_path, monkeypatch
):
    inputs = _inputs(tmp_path)
    root = inputs["source_root"]
    ignored = root / "ignored.py"
    ignored.write_bytes(b"print('before')\n")
    # Existing source-admission fixture supplies only main.rs as Git-visible;
    # this selected file must enter through the real explicit overlay owner.
    monkeypatch.setattr(command_admission, "_REPO_ROOT", root)
    command = command_admission.selected_differential_lane_command(
        "runtime.public-consumers.native", cases=["ignored.py"]
    )
    envelope = command_admission.envelope_for_command(command)
    expected = command_admission.differential_source_bindings(envelope)
    assert expected == {"ignored.py": hashlib.sha256(b"print('before')\n").hexdigest()}
    source_ref, telemetry, verify = execution_environment.capture_source_content(
        source_root=root,
        env=inputs["env"],
        overlays=[ignored],
        cas_root=inputs["result_root"] / "custody-cas",
        hash_workers=1,
        expected_files=expected,
    )
    assert telemetry["file_count"] == 2
    recorded = custody_cas.read_ref(
        source_ref, expected_root=inputs["result_root"] / "custody-cas"
    )
    assert (
        next(row["sha256"] for row in recorded["files"] if row["path"] == "ignored.py")
        == expected["ignored.py"]
    )
    command_admission.validate_envelope(envelope, command)
    verify()
    ignored.write_bytes(b"print('after!')\n")
    # Independent replay, capture-gap, and live-generation controls all observe
    # the actual ignored input bytes through their existing production owners.
    with pytest.raises(ValueError, match="envelope"):
        command_admission.validate_envelope(envelope, command)
    with pytest.raises(ValueError, match="changed since command admission"):
        execution_environment.capture_source_content(
            source_root=root,
            env=inputs["env"],
            overlays=[ignored],
            cas_root=inputs["result_root"] / "custody-cas",
            hash_workers=1,
            expected_files=expected,
        )
    with pytest.raises(ValueError):
        verify()
    with pytest.raises(ValueError, match="missing from"):
        execution_environment.capture_source_content(
            source_root=root,
            env=inputs["env"],
            overlays=(),
            cas_root=inputs["result_root"] / "custody-cas",
            hash_workers=1,
            expected_files=expected,
        )


@pytest.mark.parametrize("selected_cases", [False, True])
def test_python_driver_output_roles_survive_acquisition_and_parent_validation(
    tmp_path: Path, selected_cases: bool
) -> None:
    inputs = _inputs(tmp_path)
    lane = command_admission.proof_plan.ProofPlan.load().named_lane(
        "runtime.public-consumers.native"
    )
    argv = (
        command_admission.selected_differential_lane_command(
            lane.id, cases=lane.differential_cases[:1]
        )
        if selected_cases
        else list(lane.argv)
    )
    envelope = command_admission.envelope_for_command(argv)
    outputs = cargo_output_environment.CargoOutputEnvironment.for_envelope(envelope)
    inputs["command"] = command_admission._python_bootstrap_command(
        envelope, argv, environment_root=Path(sys.prefix)
    )
    inputs["outputs"] = outputs
    scratch = (tmp_path / "proof-scratch").resolve()
    scratch.mkdir()
    receipts = str(tmp_path / "differential-receipts")
    inputs["env"].update(
        MOLT_PROOF_SCRATCH_ROOT=str(scratch),
        MOLT_DIFF_ROOT=receipts,
        MOLT_DIFF_CARGO_TARGET_DIR="old-diff-target",
        MOLT_DIFF_TMPDIR="old-diff-scratch",
        MOLT_COMPAT_SCRATCH_ROOT="old-compat-scratch",
    )
    lease = cache.acquire(**inputs)
    try:
        bound = lease.environment
        assert bound["CARGO_TARGET_DIR"] == str(lease.target)
        assert bound["MOLT_DIFF_CARGO_TARGET_DIR"] == str(lease.target)
        assert bound["MOLT_DIFF_TMPDIR"] == str(scratch)
        assert bound["MOLT_COMPAT_SCRATCH_ROOT"] == str(scratch)
        assert bound["MOLT_DIFF_ROOT"] == receipts
        assert outputs.identity()["bindings"]["MOLT_DIFF_TMPDIR"] == {
            "role": "scratch-output"
        }
        assert outputs.identity()["bindings"]["MOLT_DIFF_CARGO_TARGET_DIR"] == {
            "role": "build-output"
        }
        identity = custody_cas.read_ref(
            lease.provenance["inputs"], expected_root=lease.cas_root
        )["identity"]
        identity_arguments = {
            "source": identity["source"],
            "toolchains": inputs["toolchains"],
            "command": inputs["command"],
            "outputs": outputs,
        }
        assert cache.input_identity(**identity_arguments, env=inputs["env"]) == identity
        other_nonce = {
            **inputs["env"],
            "MOLT_PROOF_SCRATCH_ROOT": str(tmp_path / "other-proof-nonce"),
            "MOLT_DIFF_CARGO_TARGET_DIR": "different-diff-target",
            "MOLT_DIFF_TMPDIR": "different-diff-scratch",
            "MOLT_COMPAT_SCRATCH_ROOT": "different-compat-scratch",
        }
        assert cache.input_identity(**identity_arguments, env=other_nonce) == identity
        assert (
            cache.input_identity(
                **identity_arguments, env={**other_nonce, "RUSTFLAGS": "-Cdebuginfo=1"}
            )
            != identity
        )
        final, _contract = execution_environment._cargo_output_environment_contract(
            bound,
            {"passed_names": [], "override_names": []},
            outputs=outputs,
            target=lease.target,
        )
        # Independently reconstruct from original admission after Python transport
        # rewriting, as the real terminal parent does.
        reconstructed = cargo_output_environment.CargoOutputEnvironment.for_envelope(
            envelope
        )

        def validate(env):
            cache.validate_prelaunch(
                lease.provenance,
                cas_root=lease.cas_root,
                command=inputs["command"],
                outputs=reconstructed,
                env=env,
                toolchains=inputs["toolchains"],
                source_root=str(inputs["source_root"]),
                source_snapshot=inputs["source_snapshot"],
                source_content=inputs["source_content"],
            )

        validate(final)
        for name in (
            "MOLT_DIFF_CARGO_TARGET_DIR",
            "MOLT_DIFF_TMPDIR",
            "MOLT_COMPAT_SCRATCH_ROOT",
        ):
            for value in (None, "escaped"):
                forged = dict(final)
                if value is None:
                    del forged[name]
                else:
                    forged[name] = value
                with pytest.raises(ValueError, match="output environment differs"):
                    validate(forged)
    finally:
        lease.close()


def _workspace_inputs(tmp_path):
    inputs = _inputs(tmp_path)
    plan = command_admission._proof_command_registry()["plan"]
    registered = next(
        c for c in plan.commands if c.id == "rust.test.runtime-cold-lifecycle"
    )
    envelope = command_admission.envelope_for_command(registered.argv)
    inputs["outputs"] = cargo_output_environment.CargoOutputEnvironment.for_envelope(
        envelope
    )
    # The driver has a separate, run-owned scratch boundary.
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    inputs["env"]["MOLT_PROOF_SCRATCH_ROOT"] = str(scratch)
    return inputs


def _finish_workspace(lease, inputs):
    publication = lease.publish(_completed(lease))
    lease.close()
    return cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=_terminal_receipt(lease, publication),
    )


def test_workspace_reuses_after_edits_but_old_receipt_cannot_delete_new_owner(tmp_path):
    inputs = _workspace_inputs(tmp_path)
    first = cache.acquire(**inputs)
    (first.target / "compiler-artifact").write_bytes(b"built")
    projection = _finish_workspace(first, inputs)
    inputs.update(run_id="unit-two", execution_nonce_sha256="d" * 64)
    (inputs["source_root"] / "main.rs").write_text(
        'fn main() {println!("edited");}\n', encoding="utf-8"
    )
    _capture(inputs)
    inputs["command"] = ["cargo", "test", "different-filter"]
    second = cache.acquire(**inputs)
    try:
        assert second.target == first.target
        assert second.provenance["input_sha256"] != first.provenance["input_sha256"]
        assert second.provenance["state"] == "reused"
        assert (second.target / "compiler-artifact").read_bytes() == b"built"
        cache.validate_terminal_receipt(
            result_root=inputs["result_root"],
            projection=projection,
            provenance=first.provenance,
            run_id="unit-one",
            execution_nonce_sha256="a" * 64,
        )
        # An old receipt is valid history, never current mutation authority.
        with pytest.raises(ValueError, match="owner/provenance"):
            cache._validate_terminal_generation_authority(
                result_root=inputs["result_root"],
                provenance=first.provenance,
                projection=projection,
                target=first.target,
                owner_path=first.owner_path,
            )
    finally:
        second.close()


@pytest.mark.parametrize("boundary", ["unacknowledged", "unstable", "environment"])
def test_workspace_refuses_unsafe_or_incompatible_reuse(tmp_path, boundary):
    inputs = _workspace_inputs(tmp_path)
    first = cache.acquire(**inputs)
    if boundary == "unstable":
        publication = first.publish({"phase": "failed"})
        first.close()
        cache.record_terminal_receipt(
            result_root=inputs["result_root"],
            provenance=first.provenance,
            terminal_receipt=_terminal_receipt(first, publication),
        )
    elif boundary == "unacknowledged":
        first.publish(_completed(first))
        first.close()
    else:
        _finish_workspace(first, inputs)
        inputs["env"]["RUSTFLAGS"] = "-Cdebuginfo=1"
    inputs.update(run_id="unit-two", execution_nonce_sha256="d" * 64)
    second = cache.acquire(**inputs)
    try:
        assert second.target != first.target
        assert second.provenance["state"] == "cold"
    finally:
        second.close()


@pytest.mark.slow
@pytest.mark.usefixtures("standard_finders")
def test_workspace_real_cargo_warm_and_declared_edits(
    tmp_path, monkeypatch, source_admission
):
    import shutil
    import py_compile
    from tests.test_python_external_custody import _ExternalFixture
    from molt.python_environment_location import locate_current_python_environment
    from tools.proof_queue_pkg.process_image_capture import capture_image
    from tools.harness_memory_guard import guarded_completed_process

    # Modeled neighbors use admitted space; this real crate measures the disk.
    monkeypatch.setattr(
        cache.disk_capacity, "_default_measure_free_bytes", source_admission
    )
    cargo = shutil.which("cargo")
    if cargo is None:
        pytest.skip("real Cargo oracle requires Cargo")
    inputs = _workspace_inputs(tmp_path)
    source = inputs["source_root"]
    # Exercise the real editable-import inventory producer, including pyc.
    # The fixture supplies a .pth declaration and active external import root;
    # it does not fabricate hashes or exclude the checkout from custody.
    editable = _ExternalFixture(tmp_path)
    bytecode = editable.imports / "module.pyc"
    location = locate_current_python_environment()
    python_images = [
        capture_image(role, Path(location[field]), preserve_path=True)
        for role, field in (
            ("selected-interpreter", "selected_executable"),
            ("base-interpreter", "base_executable"),
        )
    ]
    (source / "Cargo.toml").write_text(
        '[package]\nname="queue-workspace-oracle"\nversion="0.1.0"\nedition="2021"\n'
        '[lib]\npath="main.rs"\n[workspace]\n',
        encoding="utf-8",
    )
    script = """fn main() {
        println!("cargo:rerun-if-changed=input.txt");
        println!("cargo:rerun-if-changed=abi.txt");
        println!("cargo:rerun-if-env-changed=WITNESS_ENV");
        println!("cargo:rustc-env=WITNESS_INPUT={}", std::fs::read_to_string("input.txt").unwrap());
        println!("cargo:rustc-env=WITNESS_ABI={}", std::fs::read_to_string("abi.txt").unwrap());
        println!("cargo:rustc-env=WITNESS_ENV={}", std::env::var("WITNESS_ENV").unwrap());
    }"""
    (source / "build.rs").write_text(script, encoding="utf-8")
    (source / "input.txt").write_text("one", encoding="utf-8")
    (source / "abi.txt").write_text("abi1", encoding="utf-8")
    (source / "driver.py").write_text("# first Python driver\n", encoding="utf-8")
    program = '#[test] fn witness() { println!("WITNESS={}:{}:{}:A", env!("WITNESS_INPUT"), env!("WITNESS_ABI"), env!("WITNESS_ENV")); }\n'
    (source / "main.rs").write_text(program, encoding="utf-8")
    ambient = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("CARGO_")
        and key not in {"RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"}
    }
    inputs["env"].update(ambient)
    inputs["env"].update(
        WITNESS_ENV="alpha",
        CARGO_INCREMENTAL="0",
        CARGO_LOG="cargo::core::compiler::fingerprint=info",
        MOLT_USE_SCCACHE="0",
        MOLT_PROOF_SCRATCH_ROOT=str(tmp_path / "scratch"),
        # The dependency-free fixture owns its headroom, not the runtime floor.
        MOLT_DISK_GUARD_HIGH_WATER_GB="1",
    )
    inputs["command"] = [
        cargo,
        "test",
        "--offline",
        "--message-format=json",
        "--",
        "--nocapture",
    ]
    overlays = [
        source / name
        for name in ("Cargo.toml", "build.rs", "input.txt", "abi.txt", "driver.py")
    ]
    rows = []
    first = None
    first_projection = None
    retained_binary_receipts = tmp_path / "binary-receipts"
    for index, (step, expected, fresh) in enumerate(
        [
            ("cold", "one:abi1:alpha:A", False),
            ("warm", "one:abi1:alpha:A", True),
            ("python-edit", "one:abi1:alpha:A", True),
            ("rust-edit", "one:abi1:alpha:B", False),
            ("declared-file-edit", "two:abi1:alpha:B", False),
            ("abi-edit", "two:abi2:alpha:B", False),
            ("declared-env-edit", "two:abi2:beta:B", False),
        ]
    ):
        if step == "python-edit":
            (source / "driver.py").write_text(
                "# changed Python driver\n", encoding="utf-8"
            )
            (editable.imports / "module.py").write_text(
                "VALUE = 200\n", encoding="utf-8"
            )
        elif step == "rust-edit":
            (source / "main.rs").write_text(
                program.replace(":A", ":B"), encoding="utf-8"
            )
        elif step == "declared-file-edit":
            (source / "input.txt").write_text("two", encoding="utf-8")
        elif step == "abi-edit":
            (source / "abi.txt").write_text("abi2", encoding="utf-8")
        elif step == "declared-env-edit":
            inputs["env"]["WITNESS_ENV"] = "beta"
        inputs.update(
            run_id=f"oracle-{index}",
            execution_nonce_sha256=hashlib.sha256(str(index).encode()).hexdigest(),
        )
        py_compile.compile(
            str(editable.imports / "module.py"), cfile=str(bytecode), doraise=True
        )
        external_custody = editable.capture()
        editable.validate(external_custody)
        inputs["toolchains"] = {
            "python": {
                "schema": "molt.proof-python-toolchain.v3",
                "location": location,
                "process_images": python_images,
                "environment": {"external_import_custody": external_custody},
                "file_custody": editable.context.file_custody(),
            }
        }
        captured_paths = {
            row["path"] for row in inputs["toolchains"]["python"]["file_custody"]
        }
        assert str(editable.imports / "module.py") in captured_paths
        assert str(bytecode) in captured_paths
        assert any(path.endswith(".pyc") for path in captured_paths)
        _capture(inputs, overlays=overlays)
        lease = cache.acquire(**inputs)
        if step == "python-edit":
            assert lease.provenance["input_sha256"] != first.provenance["input_sha256"]
        try:
            result = guarded_completed_process(
                inputs["command"],
                prefix="MOLT_QUEUE_REUSE_ORACLE",
                cwd=source,
                env=lease.environment,
                timeout=30,
            )
            assert result.returncode == 0, result.stderr
            assert "WITNESS=" + expected in result.stdout
            artifacts = [
                json.loads(line)
                for line in result.stdout.splitlines()
                if line.startswith('{"reason":')
            ]
            test_image = next(
                row
                for row in artifacts
                if row.get("reason") == "compiler-artifact" and row.get("executable")
            )
            assert test_image["fresh"] is fresh, (
                f"Cargo freshness: expected {fresh!r}, observed {test_image['fresh']!r}\n"
                f"step={step}, workspace={source}, target_dir={lease.target}\n"
                f"Cargo stdout:\n{result.stdout}\nCargo stderr:\n{result.stderr}"
            )
            from tools import cargo_test_binary_runner, run_cargo_test_truth

            if step == "cold":
                assert (
                    cargo_test_binary_runner.main(
                        [
                            "--timeout-seconds",
                            "10",
                            "--receipt-dir",
                            str(retained_binary_receipts),
                            "--run-id",
                            "original-cargo-image",
                            "--",
                            test_image["executable"],
                            "--nocapture",
                        ]
                    )
                    == 0
                )
            # Original executed paths stay original; later Cargo edits must not
            # invalidate byte evidence of the previous successful execution.
            assert (
                len(
                    run_cargo_test_truth.load_binary_receipts(
                        retained_binary_receipts, expected_run_id="original-cargo-image"
                    )
                )
                == 1
            )
            if step != "cold":
                assert lease.target == first.target
                assert lease.provenance["state"] == "reused"
            publication = lease.publish(_completed(lease))
        finally:
            lease.close()
        terminal = _terminal_receipt(lease, publication)
        terminal.update(command_returncode=0)
        terminal["queue_terminal"].update(
            status="passed", returncode=0, command_returncode=0
        )
        projection = cache.record_terminal_receipt(
            result_root=inputs["result_root"],
            provenance=lease.provenance,
            terminal_receipt=terminal,
        )
        if first is None:
            first, first_projection = lease, projection
        elif step == "warm":
            with pytest.raises(ValueError, match="owner/provenance"):
                cache.reclaim_terminal_unsealed(
                    result_root=inputs["result_root"],
                    provenance=first.provenance,
                    projection=first_projection,
                )
            assert lease.target.is_dir()
        rows.append(
            {
                "step": step,
                "fresh": fresh,
                "cargo_seconds": result.elapsed_s,
                "output": expected,
                "state": lease.provenance["state"],
            }
        )
    print("QUEUE_CARGO_WORKSPACE_ORACLE=" + json.dumps(rows, sort_keys=True))


@pytest.mark.parametrize(
    "field,value",
    [("target", "/foreign/target"), ("generation_owner", "/foreign/owner.json")],
)
def test_workspace_refuses_foreign_terminal_pointer(tmp_path, field, value):
    inputs = _workspace_inputs(tmp_path)
    first = cache.acquire(**inputs)
    _finish_workspace(first, inputs)
    pointer = first.owner_path.parent.parent / "state.json"
    record = json.loads(pointer.read_text(encoding="utf-8"))
    record[field] = value
    pointer.write_text(json.dumps(record), encoding="utf-8")
    with pytest.raises(ValueError, match="(escaped|mismatch)"):
        cache.acquire(**inputs)
    assert first.target.is_dir()


def test_workspace_uses_actual_run_context_scratch_and_session_provenance(tmp_path):
    from molt.dx import RunContext, scratch_dir

    inputs = _workspace_inputs(tmp_path)
    source = inputs["source_root"]
    targets = []
    identities = []
    for session in ("generated-one", "generated-two", "pinned-one", "pinned-two"):
        base = {"PATH": os.environ.get("PATH", ""), "MOLT_USE_SCCACHE": "0"}
        if session.startswith("pinned"):
            base["MOLT_SESSION_ID"] = session
        env = RunContext(source, session_prefix=session).canonical_env(
            base, create_dirs=False
        )
        env = env.updated(
            {
                "MOLT_DIFF_ROOT": str(scratch_dir(source, "diff-" + session, env)),
                "MOLT_PROOF_SCRATCH_ROOT": str(tmp_path / "scratch"),
            }
        )
        inputs.update(
            env=env,
            run_id=session,
            execution_nonce_sha256=hashlib.sha256(session.encode()).hexdigest(),
        )
        lease = cache.acquire(**inputs)
        targets.append(lease.target)
        identities.append(lease.provenance["input_sha256"])
        _finish_workspace(lease, inputs)
    assert targets[0] == targets[1]
    assert len(set(targets[1:])) == 3
    assert identities[0] != identities[1]


@pytest.mark.parametrize("changed", ["interpreter", "location"])
def test_workspace_python_selection_still_partitions_storage(tmp_path, changed):
    from tools.proof_queue_pkg.process_image_capture import capture_image

    inputs = _workspace_inputs(tmp_path)
    interpreter = inputs["source_root"] / ".venv/bin/python"
    interpreter.parent.mkdir(parents=True)
    interpreter.write_bytes(b"selected interpreter")
    interpreter.chmod(0o755)

    def capture():
        image = capture_image("selected-interpreter", interpreter)
        return {
            "python": {
                "schema": "molt.proof-python-toolchain.v3",
                "location": {"prefix": str(interpreter.parent.parent)},
                "process_images": [image],
                "file_custody": [
                    {
                        "path": image["path"],
                        "size": image["size_bytes"],
                        "sha256": image["sha256"],
                    }
                ],
            }
        }

    inputs["toolchains"] = capture()
    first = cache.acquire(**inputs)
    _finish_workspace(first, inputs)
    if changed == "interpreter":
        interpreter.write_bytes(b"replacement interpreter")
    inputs["toolchains"] = capture()
    if changed == "location":
        inputs["toolchains"]["python"]["location"]["prefix"] = str(
            tmp_path / "other-env"
        )
    inputs.update(run_id="new-python-selection", execution_nonce_sha256="b" * 64)
    second = cache.acquire(**inputs)
    try:
        assert second.target != first.target
        assert second.provenance["state"] == "cold"
    finally:
        second.close()


@pytest.mark.parametrize("domain", ["cargo-workspace", "none"])
def test_actual_guard_memory_changes_reuse_only_development_workspace(tmp_path, domain):
    from tools.memory_guard_core.process_custody import (
        _inject_guard_memory_contract_env,
    )

    inputs = _workspace_inputs(tmp_path)
    inputs["outputs"] = replace(inputs["outputs"], cache_domain=domain)
    base = {
        **inputs["env"],
        "MOLT_SESSION_ID": "proof-native-build-cargo-native-runtime",
    }
    leases = []
    selected_envs = []
    for index, limit in enumerate((4.644984245300293, 4.67650032043457)):
        env = dict(base)
        _inject_guard_memory_contract_env(
            env,
            max_rss_kb=int(limit * 1024 * 1024),
            child_rlimit_kb=int(limit * 1024 * 1024),
        )
        selected, _contract = (
            execution_environment._deterministic_execution_environment(
                env, override_names=[]
            )
        )
        selected_envs.append(selected)
        inputs.update(
            env=selected,
            run_id=f"guard-{index}",
            execution_nonce_sha256=str(index) * 64,
        )
        lease = cache.acquire(**inputs)
        leases.append(lease)
        _finish_workspace(lease, inputs)
    assert all("MOLT_BACKEND_MEMORY_AVAILABLE_GB" not in env for env in selected_envs)
    assert (
        selected_envs[0]["MOLT_BACKEND_MAX_PROCESS_RSS_GB"]
        != selected_envs[1]["MOLT_BACKEND_MAX_PROCESS_RSS_GB"]
    )
    assert leases[0].provenance["input_sha256"] != leases[1].provenance["input_sha256"]
    assert (leases[0].target == leases[1].target) is (domain == "cargo-workspace")
    assert leases[1].provenance["state"] == (
        "reused" if domain == "cargo-workspace" else "cold"
    )


@pytest.mark.parametrize(
    "name,value",
    [
        ("RUSTFLAGS", "-Ctarget-cpu=native"),
        ("CARGO_PROFILE_RELEASE_LTO", "true"),
        ("CC", "/selected-other-compiler"),
        ("MOLT_SESSION_ID", "explicit-other-session"),
        ("MOLT_LLVM_PREFIX", "/different SDK"),
        ("LLVM_CONFIG_PATH", "/different SDK/bin/llvm-config"),
        ("LLVM_SYS_221_PREFIX", "/different llvm-sys search"),
        ("MLIR_SYS_220_PREFIX", "/different MLIR SDK"),
        ("TABLEGEN_220_PREFIX", "/different TableGen SDK"),
    ],
)
def test_workspace_compile_environment_and_pinned_session_still_partition(
    tmp_path, name, value
):
    inputs = _workspace_inputs(tmp_path)
    first = cache.acquire(**inputs)
    _finish_workspace(first, inputs)
    inputs["env"][name] = value
    inputs.update(run_id="changed-compilation-input", execution_nonce_sha256="b" * 64)
    second = cache.acquire(**inputs)
    try:
        assert second.target != first.target
        assert second.provenance["state"] == "cold"
    finally:
        second.close()


def _archival_generation(tmp_path, monkeypatch, *, external=False):
    import tarfile

    inputs = _inputs(tmp_path)
    if external:
        output = tmp_path / "volume"
        output.mkdir()
        inputs["cargo_output_root"] = cache.cargo_output_layout.declare_root(
            str(output)
        )
        inputs["outputs"] = (
            cargo_output_environment.CargoOutputEnvironment.for_envelope(
                command_admission.envelope_for_command(
                    inputs["command"], cargo_output_root=str(output)
                )
            )
        )
    lease = cache.acquire(**inputs)
    (lease.target / "empty").mkdir()
    image = lease.target / "unfinished-image"
    image.write_bytes(b"recoverable exact executable\x00\xff")
    image.chmod(0o755)
    publication = lease.publish({"phase": "failed"})
    lease.close()
    terminal = _terminal_receipt(lease, publication, process_cleanup_safe=False)
    projection = cache.record_terminal_receipt(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        terminal_receipt=terminal,
    )
    archive = tmp_path / "retained.tar"
    # Independent archive producer: tar metadata/contents come directly from
    # the filesystem, never from the product manifest or archive inspector.
    with tarfile.open(archive, "w", format=tarfile.PAX_FORMAT) as retained:
        for path in [lease.target, *sorted(lease.target.rglob("*"))]:
            info = retained.gettarinfo(
                str(path), arcname=path.relative_to(lease.target).as_posix()
            )
            info.pax_headers["mtime"] = str(
                Decimal(path.stat().st_mtime_ns) / 1_000_000_000
            )
            if path.is_file():
                with path.open("rb") as contents:
                    retained.addfile(info, contents)
            else:
                retained.addfile(info)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    # The native verification boundary has its own receipt-byte/closed controls.
    monkeypatch.setattr(
        cache,
        "_verify_archival_process_closure",
        lambda *a, **kw: {"custody_closed": True},
    )
    return inputs, lease, projection, archive, digest


def test_archival_reclaim_preserves_invalid_proof_and_recoverable_bytes(
    tmp_path, monkeypatch
):
    import tarfile

    inputs, lease, projection, archive, digest = _archival_generation(
        tmp_path, monkeypatch
    )
    receipt_path = Path(projection["terminal_receipt"]["path"])
    receipt_before = receipt_path.read_bytes()
    expected = (lease.target / "unfinished-image").read_bytes()
    ordinary = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
    )
    assert ordinary["state"] == "not-reclaimable" and lease.target.exists()
    kwargs = dict(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
        archive=archive,
        archive_sha256=digest,
    )
    result = cache.reclaim_terminal_unsealed(**kwargs)
    assert result["state"] == "reclaimed" and not lease.target.exists()
    assert receipt_path.read_bytes() == receipt_before
    assert projection["state"] == "terminal-indeterminate-retained"
    original = cache.validate_terminal_receipt(
        result_root=inputs["result_root"],
        projection=projection,
        provenance=lease.provenance,
        run_id="unit-one",
        execution_nonce_sha256="a" * 64,
    )
    assert original["process_cleanup_safe"] is False
    retained = custody_cas.read_ref(
        result["evidence"], expected_root=inputs["result_root"] / "custody-cas"
    )
    assert retained["qualification_eligible"] is False
    assert retained["archive"]["sha256"] == digest
    with tarfile.open(archive) as stream:
        assert stream.extractfile("unfinished-image").read() == expected
        assert stream.getmember("unfinished-image").mode == 0o755
        assert stream.getmember("empty").isdir()
    assert cache.reclaim_terminal_unsealed(**kwargs)["idempotent"] is True
    with pytest.raises(ValueError, match="reference changed"):
        cache.reclaim_terminal_unsealed(**{**kwargs, "archive_sha256": "0" * 64})


@pytest.mark.parametrize(
    "damage",
    [
        "digest",
        "bytes",
        "omission",
        "extra",
        "mode",
        "mtime",
        "link",
        "duplicate",
        "traversal",
        "dir-mode",
        "dir-mtime",
        "duplicate-root",
    ],
)
def test_archival_reclaim_refuses_invalid_archive_before_removal(
    tmp_path, monkeypatch, damage
):
    import io
    import tarfile

    inputs, lease, projection, archive, digest = _archival_generation(
        tmp_path, monkeypatch
    )
    if damage == "digest":
        digest = "0" * 64
    else:
        with tarfile.open(archive) as original:
            rows = [
                (row, original.extractfile(row).read() if row.isfile() else None)
                for row in original
            ]
        info, data = next(row for row in rows if row[0].name == "unfinished-image")
        if damage == "dir-mode":
            next(row[0] for row in rows if row[0].name == "empty").mode = 0o777
        if damage == "dir-mtime":
            next(row[0] for row in rows if row[0].name == "empty").pax_headers[
                "mtime"
            ] = "0"
        if damage == "bytes":
            data = b"X" * len(data)
        if damage == "mode":
            info.mode = 0o600
        if damage == "mtime":
            info.pax_headers["mtime"] = "0"
        if damage == "link":
            info.type, info.linkname = tarfile.SYMTYPE, "outside"
        if damage == "traversal":
            info.name = "../unfinished-image"
        with tarfile.open(archive, "w", format=tarfile.PAX_FORMAT) as output:
            for entry, payload in rows:
                if entry is info:
                    if damage == "omission":
                        continue
                    payload = data
                output.addfile(
                    entry, io.BytesIO(payload) if payload is not None else None
                )
                if entry.name == "." and damage == "duplicate-root":
                    output.addfile(entry)
                if entry is info and damage == "duplicate":
                    output.addfile(entry, io.BytesIO(data))
            if damage == "extra":
                extra = tarfile.TarInfo("unexpected")
                output.addfile(extra, io.BytesIO())
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    monkeypatch.setattr(
        cache,
        "delete_path",
        lambda path: pytest.fail("invalid archive cannot authorize removal"),
    )
    result = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
        archive=archive,
        archive_sha256=digest,
    )
    assert result["state"] == "reclaim-blocked"
    assert (
        lease.target / "unfinished-image"
    ).read_bytes() == b"recoverable exact executable\x00\xff"


@pytest.mark.parametrize(
    "failure", ["closure", "owner", "busy", "replacement", "delete"]
)
def test_archival_reclaim_preserves_target_on_custody_or_publication_failure(
    tmp_path, monkeypatch, failure
):
    import shutil

    inputs, lease, projection, archive, digest = _archival_generation(
        tmp_path, monkeypatch
    )
    kwargs = dict(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
        archive=archive,
        archive_sha256=digest,
        timeout_s=0,
    )
    if failure == "closure":

        def refusal(*a, **kw):
            raise ValueError("physical closure absent")

        monkeypatch.setattr(cache, "_verify_archival_process_closure", refusal)
    elif failure == "owner":
        owner = json.loads(lease.owner_path.read_text(encoding="utf-8"))
        owner["execution_nonce_sha256"] = "b" * 64
        cache._write_owner(lease.owner_path, owner)
        with pytest.raises(ValueError, match="binding mismatch"):
            cache.reclaim_terminal_unsealed(**kwargs)
        assert lease.target.exists()
        return
    elif failure == "busy":
        lock = cache._acquire_file_lock(
            lease.owner_path.parent.parent / "target.lock",
            timeout_s=0,
            timeout_message="test busy",
        )
        try:
            with pytest.raises(RuntimeError, match="busy"):
                cache.reclaim_terminal_unsealed(**kwargs)
        finally:
            cache._release_file_lock(lock)
        assert lease.target.exists()
        return
    elif failure == "replacement":
        original = cache._preserve_timings

        def replace(path, **kw):
            renamed = path.with_name(path.name + "-replaced")
            path.rename(renamed)
            shutil.copytree(renamed, path, copy_function=shutil.copy2)
            return original(path, **kw)

        monkeypatch.setattr(cache, "_preserve_timings", replace)
    else:
        monkeypatch.setattr(
            cache, "delete_path", lambda path: (False, "injected refusal")
        )
    result = cache.reclaim_terminal_unsealed(**kwargs)
    assert result["state"] == "reclaim-blocked" and lease.target.exists()
    if failure == "delete":
        owner = json.loads(lease.owner_path.read_text(encoding="utf-8"))
        preserved = custody_cas.read_ref(
            owner["reclaim"]["evidence"],
            expected_root=inputs["result_root"] / "custody-cas",
        )
        assert preserved["archive"]["sha256"] == digest


@pytest.mark.parametrize(
    "changed", [None, "binary", "policy", "policy-target", "receipt", "nonce", "open"]
)
def test_archival_native_closure_is_bound_to_original_image_and_receipt(
    tmp_path, monkeypatch, changed
):
    result_root = tmp_path / "result"
    result_root.mkdir()
    image = tmp_path / "supervisor"
    image.write_bytes(b"independently retained verifier image")
    artifact = custody_cas.put_file(
        result_root / "custody-cas", image, logical_name="supervisor", executable=True
    ).as_dict()
    native = {"complete": False, "state": "INCOMPLETE", "nonce_sha256": "a" * 64}
    policy = result_root / "original-policy.json"
    policy.write_text(
        json.dumps(
            {
                "derived_roots": [
                    {"role": "build-output", "path": str(tmp_path / "target")}
                ]
            }
        ),
        encoding="utf-8",
    )
    recorded = result_root / "original-receipt.json"
    recorded.write_text(json.dumps(native), encoding="utf-8")
    process = dict(
        binary={k: artifact[k] for k in ("path", "sha256", "size_bytes")},
        binary_artifact=artifact,
        policy=command_identity._file_identity(policy),
        receipt_file=command_identity._file_identity(recorded),
        receipt=native,
    )
    terminal = {
        "process_supervisor": process,
        "execution_nonce_sha256": "a" * 64,
        "target": str(tmp_path / "target"),
    }
    calls = []

    def verify(**kwargs):
        calls.append(kwargs)
        assert kwargs["require_closed"] is True
        assert kwargs["binary"] == Path(artifact["path"])
        if changed == "open":
            raise ValueError(
                "native proof supervisor has not established physical closure"
            )
        return json.loads(recorded.read_text(encoding="utf-8"))

    monkeypatch.setattr(supervisor_custody, "_validated_supervisor_receipt", verify)
    if changed == "binary":
        process["binary"] = {**process["binary"], "sha256": "b" * 64}
    if changed == "policy":
        policy.write_text('{"changed":"policy"}', encoding="utf-8")
    if changed == "policy-target":
        policy.write_text(
            json.dumps(
                {
                    "derived_roots": [
                        {"role": "build-output", "path": str(tmp_path / "other-target")}
                    ]
                }
            ),
            encoding="utf-8",
        )
        process["policy"] = command_identity._file_identity(policy)
    if changed == "receipt":
        recorded.write_text('{"complete":true}', encoding="utf-8")
    if changed == "nonce":
        terminal["execution_nonce_sha256"] = "b" * 64
    if changed is None:
        result = cache._verify_archival_process_closure(
            terminal, result_root=result_root
        )
        assert result["custody_closed"] is True
        assert native["complete"] is False
        assert len(calls) == 1
    else:
        with pytest.raises(ValueError):
            cache._verify_archival_process_closure(terminal, result_root=result_root)
        if changed in {"binary", "policy", "policy-target", "receipt"}:
            assert calls == []


@pytest.mark.parametrize("replacement", ["archive", "target"])
def test_archival_reclaim_fences_custody_after_owner_publication(
    tmp_path, monkeypatch, replacement
):
    import shutil

    inputs, lease, projection, archive, digest = _archival_generation(
        tmp_path, monkeypatch
    )
    write_owner = cache._write_owner

    def publish(path, owner):
        write_owner(path, owner)
        if owner["lifecycle"] == "reclaiming":
            selected = archive if replacement == "archive" else lease.target
            renamed = selected.with_name(selected.name + "-original")
            selected.rename(renamed)
            if selected == archive:
                shutil.copy2(renamed, selected)
            else:
                shutil.copytree(renamed, selected, copy_function=shutil.copy2)

    monkeypatch.setattr(cache, "_write_owner", publish)
    monkeypatch.setattr(
        cache,
        "delete_path",
        lambda path: pytest.fail("replaced custody cannot authorize deletion"),
    )
    kwargs = dict(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
        archive=archive,
        archive_sha256=digest,
    )
    if replacement == "archive":
        with pytest.raises(ValueError, match="changed"):
            cache.reclaim_terminal_unsealed(**kwargs)
    else:
        assert cache.reclaim_terminal_unsealed(**kwargs)["state"] == "reclaim-blocked"
    assert lease.target.exists()
    assert (
        json.loads(lease.owner_path.read_text(encoding="utf-8"))["lifecycle"]
        == "reclaim-blocked"
    )


def test_archive_directory_metadata_is_opt_in_to_existing_inventory(tmp_path):
    root = tmp_path / "tree"
    root.mkdir()
    child = root / "empty"
    child.mkdir(mode=0o700)
    plain = command_identity._directory_manifest_identity(
        root, label="fixture", strict_owned=True
    )
    assert set(plain) == {
        "root",
        "file_count",
        "files",
        "directories",
        "manifest_sha256",
    }
    assert plain["manifest_sha256"] == cache.canonical_json_sha256(
        {"files": [], "directories": ["empty"]}
    )
    detail = command_identity._directory_manifest_identity(
        root, label="fixture", strict_owned=True, directory_metadata=True
    )
    assert (
        detail["files"] == plain["files"]
        and detail["directories"] == plain["directories"]
    )
    assert detail["directory_metadata"] == [
        {
            "relative_path": ".",
            "mode": root.stat().st_mode & 0o7777,
            "mtime_ns": root.stat().st_mtime_ns,
        },
        {
            "relative_path": "empty",
            "mode": child.stat().st_mode & 0o7777,
            "mtime_ns": child.stat().st_mtime_ns,
        },
    ]


def test_archival_reclaim_refuses_archive_in_external_sibling_target(
    tmp_path, monkeypatch
):
    inputs, lease, projection, archive, digest = _archival_generation(
        tmp_path, monkeypatch, external=True
    )
    sibling = cache.cargo_output_layout.CargoOutputLayout.create(
        result_root=tmp_path / "other-proofs", declaration=inputs["cargo_output_root"]
    ).target(
        "d" * 64, "b" * 16, version=cache.cargo_output_layout.recorded_target_layout({})
    )
    sibling.mkdir(parents=True)
    admitted_archive = sibling / archive.name
    archive.rename(admitted_archive)
    assert not admitted_archive.is_relative_to(lease.target.parent)
    assert not admitted_archive.is_relative_to(inputs["result_root"])
    monkeypatch.setattr(
        cache,
        "delete_path",
        lambda path: pytest.fail("managed archive cannot authorize removal"),
    )
    result = cache.reclaim_terminal_unsealed(
        result_root=inputs["result_root"],
        provenance=lease.provenance,
        projection=projection,
        archive=admitted_archive,
        archive_sha256=digest,
    )
    assert result["state"] == "reclaim-blocked"
    assert "outside managed output and proof roots" in result["error"]
    assert lease.target.exists() and admitted_archive.exists()
