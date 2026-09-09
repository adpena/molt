from __future__ import annotations

import copy
import os
from pathlib import Path

import pytest

from tools.proof_queue_pkg import cargo_cache_custody as cache
from tools.proof_queue_pkg import (
    custody_cas,
    execution_environment,
    command_identity,
    execution_receipt_details,
)


@pytest.fixture(autouse=True)
def source_admission(monkeypatch):
    monkeypatch.setattr(
        execution_environment,
        "_git_source_paths",
        lambda root, env, **kwargs: [root / "main.rs"],
    )


def _capture(inputs, overlays=()):
    reference, telemetry, verify = execution_environment.capture_source_content(
        source_root=inputs["source_root"],
        env=inputs["env"],
        overlays=overlays,
        cas_root=inputs["result_root"] / "custody-cas",
    )
    inputs["source_content"] = reference
    return telemetry, verify


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
        "timeout_s": 0.0,
        "source_snapshot": {"root": str(source), "commit": "initial"},
    }
    _capture(inputs)
    return inputs


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
        forged = copy.deepcopy(inputs["env"])
        forged["RUSTFLAGS"] = "changed"
        with pytest.raises(ValueError, match="identity mismatch"):
            cache.validate_prelaunch(
                lease.provenance,
                cas_root=lease.cas_root,
                command=inputs["command"],
                env=forged,
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
        cache.acquire(**inputs)


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
                env=inputs["env"],
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
                env=inputs["env"],
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
