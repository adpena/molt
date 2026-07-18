from __future__ import annotations

import hashlib
import json
from dataclasses import replace
from pathlib import Path
import sys

import pytest

from tools import gen_proof_plan, proof_plan
from tools.proof_queue_pkg import evidence as proof_queue_evidence


PLAN = proof_plan.ProofPlan.load()


def _classes(*paths: str) -> dict[str, bool]:
    selected = {family.name for family in PLAN.select(paths).selected}
    return {family.name: family.name in selected for family in PLAN.families}


def test_manifest_is_complete_and_single_authority() -> None:
    assert PLAN.path.name == "proof_plan.toml"
    assert len(PLAN.families) == 10
    assert len(PLAN.commands) >= 70
    assert len(PLAN.matrix_cells) >= 14
    assert len(PLAN.toolchain_policies) >= 15
    assert all("metadata_mode" not in family.data for family in PLAN.families)
    assert all(family.data["required"] for family in PLAN.families)
    assert len(PLAN.local_rules) >= 30
    assert not (proof_plan.ROOT / "tools" / "molt_dev_gates.toml").exists()
    assert not (proof_plan.ROOT / "tools" / "ci_changed_paths.py").exists()


def test_generated_local_dx_projection_has_stable_command_ids() -> None:
    projection = json.loads(gen_proof_plan._json_projection(PLAN))
    assert projection["schema"] == "molt.proof-plan-projection.v3"
    assert projection["receipt_schema"] == "molt.proof-receipt.v2"
    assert projection["authority_inputs"] == list(PLAN.authority_inputs)
    assert projection["authority_sha256"] == proof_plan._authority_sha256(PLAN)
    assert projection["toolchain_policies"] == [
        policy.data for policy in PLAN.toolchain_policies
    ]
    local = projection["local"]
    assert local["commands"]["local.always.0"] == PLAN.always[0]
    first = PLAN.local_rules[0]
    projected = next(rule for rule in local["rules"] if rule["name"] == first["name"])
    assert projected["command_ids"] == [
        f"local.{first['name']}.{index}" for index, _ in enumerate(first["gates"])
    ]


def test_docs_only_change_skips_compiler_proofs() -> None:
    classes = _classes("docs/agent/INDEX.md")
    assert classes["repository_policy"] is True
    assert not any(
        selected for name, selected in classes.items() if name != "repository_policy"
    )


def test_python_source_change_selects_split_proof_topology() -> None:
    classes = _classes("src/molt/cli/runtime_wasm_cache.py")
    assert classes["repository_policy"] is True
    assert classes["python_static"] is True
    assert classes["python_unit"] is True
    assert classes["native_integration"] is True
    assert classes["wasm"] is True
    assert classes["rust"] is True
    assert classes["python_security"] is False
    assert classes["rust_security"] is False
    assert classes["formal"] is False


def test_runtime_leaf_change_runs_rust_without_llvm_or_formal() -> None:
    classes = _classes("runtime/molt-stdlib-text/src/tokenize.rs")
    assert classes["rust"] is True
    assert classes["llvm"] is False
    assert classes["formal"] is False


def test_midend_change_runs_complete_rust_llvm_formal_family() -> None:
    classes = _classes("runtime/molt-passes/src/tir/value_range.rs")
    assert classes["rust"] is True
    assert classes["llvm"] is True
    assert classes["formal"] is True


def test_luau_change_selects_formal_without_stale_backend_path() -> None:
    classes = _classes("runtime/molt-backend-luau/src/luau.rs")
    assert classes["formal"] is True
    assert classes["rust"] is True


def test_llvm_control_plane_changes_run_llvm_stack() -> None:
    for path in (
        "src/molt/llvm_toolchain.py",
        "config/llvm_toolchain_arches.toml",
        ".github/actions/setup-llvm/action.yml",
        "tools/bootstrap_llvm.py",
    ):
        assert _classes(path)["llvm"] is True, path


def test_selected_family_closes_transitive_dependencies_with_reason() -> None:
    selection = PLAN.select([".github/workflows/perf-gate.yml"])
    assert [family.name for family in selection.selected] == [
        "repository_policy",
        "rust",
        "llvm",
    ]
    assert selection.reasons["llvm"] == (".github/workflows/perf-gate.yml",)
    assert selection.reasons["rust"] == ("dependency:llvm",)


def test_dependency_cycles_are_rejected() -> None:
    families = tuple(
        replace(
            family,
            data={
                **family.data,
                "dependencies": (
                    ["llvm"]
                    if family.name == "rust"
                    else ["rust"]
                    if family.name == "llvm"
                    else family.data["dependencies"]
                ),
            },
        )
        for family in PLAN.families
    )
    errors = replace(PLAN, families=families).validate()
    assert "dependency cycle: rust -> llvm -> rust" in errors


def test_toolchain_setup_projection_drift_is_rejected() -> None:
    policies = tuple(
        replace(
            policy,
            data={
                **policy.data,
                "setup_evidence": [
                    '.github/workflows/ci.yml::version: "not-the-uv-contract"'
                ],
            },
        )
        if policy.name == "uv"
        else policy
        for policy in PLAN.toolchain_policies
    )
    errors = replace(PLAN, toolchain_policies=policies).validate()
    assert any("uv: setup evidence token missing" in error for error in errors)


def test_lockfiles_select_security_and_build_classes() -> None:
    cargo = _classes("Cargo.lock")
    uv = _classes("uv.lock")
    assert cargo["rust"] and cargo["llvm"] and cargo["rust_security"]
    assert not cargo["python_static"]
    assert uv["python_static"] and uv["python_unit"] and uv["python_security"]
    assert uv["rust"] and not uv["rust_security"]


def test_workflow_mechanics_select_only_owned_families() -> None:
    ci = _classes(".github/workflows/ci.yml")
    security = _classes(".github/workflows/security_hardening.yml")
    formal = _classes(".github/workflows/formal.yml")
    assert ci["python_static"] and ci["python_unit"] and ci["rust"] and ci["llvm"]
    assert security["python_security"] and security["rust_security"]
    assert formal["formal"]
    assert ci["repository_policy"]
    assert security["repository_policy"]
    assert formal["repository_policy"]


def test_authority_change_fails_closed_to_every_family() -> None:
    classes = _classes("tools/proof_plan.toml")
    assert all(classes.values())


def test_push_uses_before_after_instead_of_unconditionally_selecting_all(
    monkeypatch,
) -> None:
    calls: list[tuple[str, str]] = []

    def fake_diff(base: str, head: str, *, three_dot: bool = False) -> list[str]:
        assert three_dot is False
        calls.append((base, head))
        return ["src/molt/frontend/diagnostics.py"]

    monkeypatch.setattr(proof_plan, "_diff_paths", fake_diff)
    selection = proof_plan.selection_for_event(
        PLAN,
        event_name="push",
        base_ref="",
        event_path="",
        before="1" * 40,
        after="2" * 40,
    )
    assert calls == [("1" * 40, "2" * 40)]
    assert {family.name for family in selection.selected} == {
        "repository_policy",
        "python_static",
        "python_unit",
        "native_integration",
        "wasm",
        "rust",
    }


def test_diff_includes_deletions_and_both_sides_of_renames(monkeypatch) -> None:
    calls: list[list[str]] = []

    def fake_git(args: list[str]) -> str:
        calls.append(args)
        return (
            "D\0runtime/molt-runtime/src/legacy.rs\0"
            "R100\0runtime/molt-backend/src/old.rs\0docs/old.rs\0"
        )

    monkeypatch.setattr(
        proof_plan,
        "_run_git",
        fake_git,
    )
    paths = proof_plan._diff_paths("a" * 40, "b" * 40)
    assert calls == [
        [
            "diff",
            "--name-status",
            "-z",
            "--diff-filter=ACDMRTUXB",
            f"{'a' * 40}..{'b' * 40}",
        ]
    ]
    assert paths == [
        "runtime/molt-runtime/src/legacy.rs",
        "runtime/molt-backend/src/old.rs",
        "docs/old.rs",
    ]
    assert _classes(*paths)["rust"] is True


def test_forced_or_null_push_fails_closed(tmp_path: Path) -> None:
    event = tmp_path / "event.json"
    event.write_text(json.dumps({"forced": True}), encoding="utf-8")
    selection = proof_plan.selection_for_event(
        PLAN,
        event_name="push",
        base_ref="",
        event_path=str(event),
        before=proof_plan.NULL_SHA,
        after="2" * 40,
    )
    assert selection.selected == PLAN.families
    assert selection.fail_closed_reason is not None


def test_generated_matrix_records_selection_reason() -> None:
    selection = PLAN.select(["Cargo.lock"])
    outputs = proof_plan.family_outputs(PLAN, selection)
    matrix = json.loads(outputs["matrix"])["include"]
    by_name = {entry["name"]: entry for entry in matrix}
    assert by_name["rust"]["selected_by"] == ["Cargo.lock"]
    assert by_name["rust"]["resource_class"] == "compiler-build-resource"
    assert "rust.test.default-truth" in by_name["rust"]["command_ids"]
    assert "linux-x86_64-rust-wasi-dev" in by_name["rust"]["matrix_cells"]


def _receipt_for(command: proof_plan.ProofCommand) -> dict[str, object]:
    versions = {
        "python": "Python 3.12.13",
        "uv": "uv 0.11.24",
        "node": "v24.16.0",
        "rustc": "rustc 1.96.1",
        "cargo": "cargo 1.96.1",
        "clang": "clang version 22.1.8",
        "llvm-config": "22.1.8",
        "mlir-opt": "LLVM version 22.1.8",
        "lld": "LLD 22.1.8",
        "lean": "Lean (version 4.28.0)",
        "quint": "0.32.0",
        "cargo-deny": "cargo-deny 0.20.2",
        "cargo-audit": "cargo-audit 0.22.2",
    }
    policies = {policy.name: policy for policy in PLAN.toolchain_policies}
    toolchains: dict[str, dict[str, str]] = {}
    for name in command.toolchains:
        path = f"/toolchain/{name}"
        launcher_path = f"{path}/launcher"
        content_path = f"{path}/content"
        version = versions[name]
        launcher_sha256 = hashlib.sha256(
            f"{launcher_path}\0binary".encode()
        ).hexdigest()
        executable_sha256 = hashlib.sha256(f"{path}\0binary".encode()).hexdigest()
        toolchains[name] = {
            "path": path,
            "launcher_path": launcher_path,
            "launcher_sha256": launcher_sha256,
            "content_path": content_path,
            "version": version,
            "version_pattern": str(policies[name].data["version_pattern"]),
            "executable_sha256": executable_sha256,
            "identity_sha256": hashlib.sha256(
                (
                    f"{path}\0{launcher_path}\0{launcher_sha256}\0{content_path}\0"
                    f"{executable_sha256}\0{version}"
                ).encode()
            ).hexdigest(),
        }
    return {
        "schema": PLAN.receipt_schema,
        "authority_sha256": proof_plan._authority_sha256(PLAN),
        "source_commit": proof_plan._source_commit(),
        "source_tree_state": "clean",
        "family": command.family,
        "environment": {"os": "linux", "arch": "x86_64", "python": "3.12"},
        "toolchains": toolchains,
        "commands": [
            {
                "id": command.id,
                "family": command.family,
                "cell": command.data["cell"],
                "argv": list(command.argv),
                "cwd": str(command.data.get("cwd", ".")),
                "dependencies": list(command.dependencies),
                "tiers": list(command.data["tiers"]),
                "timeout_seconds": command.data["timeout_seconds"],
                "timeout_env": list(command.data.get("timeout_env", [])),
                "environment_overrides": dict(command.data.get("env", {})),
                "duration_seconds": 0.1,
                "peak_rss_bytes": 1024,
                "cache_disposition": "cold",
                "resource_class": command.data["resource_class"],
                "status": "success",
                "returncode": 0,
                "guard_metrics_schema": "molt.guarded-command-metrics.v1",
            }
        ],
        "executed_partitions": [command.id],
        "status": "success",
    }


def test_receipt_verdict_fails_selected_but_unexecuted_cells(tmp_path: Path) -> None:
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert errors == ["python.static.ty: required executable receipt is missing"]


def test_receipt_verdict_accepts_every_exact_selected_partition(tmp_path: Path) -> None:
    commands = [
        command for command in PLAN.commands if command.family == "python_static"
    ]
    for command in commands:
        (tmp_path / f"{command.id}.json").write_text(
            json.dumps(_receipt_for(command)), encoding="utf-8"
        )
    assert proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path) == []


def test_receipt_verdict_rejects_authority_and_command_drift(tmp_path: Path) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command)
    receipt["authority_sha256"] = "0" * 64
    receipt["commands"][0]["argv"] = ["true"]  # type: ignore[index]
    (tmp_path / "drift.json").write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert any("authority digest" in error for error in errors)
    assert any("required executable receipt is missing" in error for error in errors)


def test_every_authority_input_mutation_invalidates_receipt(
    tmp_path: Path, monkeypatch
) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command)
    (tmp_path / "receipt.json").write_text(json.dumps(receipt), encoding="utf-8")
    original = proof_plan._authority_sha256(PLAN)
    for relative in PLAN.authority_inputs:
        mutated = proof_plan._authority_sha256(
            PLAN, {relative: (proof_plan.ROOT / relative).read_bytes() + b"\0"}
        )
        assert mutated != original, relative
        with monkeypatch.context() as context:
            context.setattr(proof_plan, "_authority_sha256", lambda _plan: mutated)
            errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
        assert any("authority digest" in error for error in errors), relative


def test_authority_digest_is_lf_crlf_checkout_invariant() -> None:
    original = proof_plan._authority_sha256(PLAN)
    for relative in PLAN.authority_inputs:
        raw = (proof_plan.ROOT / relative).read_bytes()
        lf = raw.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        crlf = lf.replace(b"\n", b"\r\n")
        assert proof_plan._authority_sha256(PLAN, {relative: crlf}) == original


def test_receipt_verdict_rejects_source_commit_replay(tmp_path: Path) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command)
    receipt["source_commit"] = "0" * 40
    (tmp_path / "replayed.json").write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert any("source commit" in error for error in errors)


def test_receipt_verdict_rejects_dirty_source_tree_attestation(tmp_path: Path) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command)
    receipt["source_tree_state"] = "dirty"
    (tmp_path / "dirty.json").write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert any("source tree is not clean" in error for error in errors)


def test_receipt_verdict_enforces_toolchain_version_contract(tmp_path: Path) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command)
    uv = receipt["toolchains"]["uv"]  # type: ignore[index]
    uv["version"] = "uv 999.0.0"  # type: ignore[index]
    uv["identity_sha256"] = hashlib.sha256(  # type: ignore[index]
        (
            f"{uv['path']}\0{uv['launcher_path']}\0{uv['launcher_sha256']}\0"  # type: ignore[index]
            f"{uv['content_path']}\0{uv['executable_sha256']}\0{uv['version']}"  # type: ignore[index]
        ).encode()
    ).hexdigest()
    (tmp_path / "wrong-version.json").write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert any("uv version violates" in error for error in errors)


def test_executor_rejects_toolchain_version_outside_contract(monkeypatch) -> None:
    monkeypatch.setattr(
        proof_plan,
        "_version_fingerprint",
        lambda policy: {
            "path": f"/toolchain/{policy.name}",
            "launcher_path": f"/toolchain/{policy.name}",
            "launcher_sha256": "0" * 64,
            "content_path": f"/toolchain/{policy.name}",
            "version": "arbitrary 999",
            "version_pattern": policy.data["version_pattern"],
            "executable_sha256": "0" * 64,
            "identity_sha256": "0" * 64,
        },
    )
    with pytest.raises(ValueError, match="toolchain contract violation"):
        proof_plan.toolchain_fingerprints(PLAN, ("python", "uv"))


def test_executor_emits_measured_receipt(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(proof_plan, "_source_tree_state", lambda: "clean")
    cell = proof_plan.MatrixCell(
        "local-executor-cell",
        {
            "id": "local-executor-cell",
            "os": proof_plan._normalized_os(),
            "arch": proof_plan._normalized_arch(),
            "python": f"{sys.version_info.major}.{sys.version_info.minor}",
            "backend": "python-tooling",
            "target": "host",
            "profile": "test",
        },
    )
    command = proof_plan.ProofCommand(
        "python.static.synthetic",
        {
            "id": "python.static.synthetic",
            "family": "python_static",
            "cell": cell.id,
            "tiers": ["test"],
            "resource_class": "python-static",
            "timeout_seconds": 10,
            "cache_domain": "none",
            "dependencies": [],
            "timeout_env": ["MOLT_INNER_TIMEOUT"],
            "env": {"MOLT_EXECUTOR_MARKER": "canonical"},
            "argv": [
                sys.executable,
                "-c",
                "import os; assert os.environ['MOLT_INNER_TIMEOUT'] == '10'; "
                "assert os.environ['MOLT_EXECUTOR_MARKER'] == 'canonical'",
            ],
            "toolchains": ["python"],
        },
    )
    receipt_path = tmp_path / "receipt.json"
    test_plan = replace(PLAN, matrix_cells=(cell,), commands=(command,))
    assert proof_plan.execute_commands(test_plan, (command,), receipt_path) == 0
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["status"] == "success"
    assert receipt["source_tree_state"] == "clean"
    assert receipt["executed_partitions"] == [command.id]
    record = receipt["commands"][0]
    assert record["duration_seconds"] > 0
    assert record["peak_rss_bytes"] > 0
    assert record["cache_disposition"] == "not-applicable"
    assert record["timeout_env"] == ["MOLT_INNER_TIMEOUT"]
    assert record["environment_overrides"] == {"MOLT_EXECUTOR_MARKER": "canonical"}
    assert receipt["toolchains"]


def test_executor_refuses_uncommitted_source_attestation(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(proof_plan, "_source_tree_state", lambda: "dirty")
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    with pytest.raises(ValueError, match="clean source tree"):
        proof_plan.execute_commands(PLAN, (command,), tmp_path / "receipt.json")
    assert not (tmp_path / "receipt.json").exists()


def test_executor_rejects_source_mutation_during_partition(
    tmp_path: Path, monkeypatch
) -> None:
    states = iter(("clean", "clean", "dirty"))
    monkeypatch.setattr(proof_plan, "_source_tree_state", lambda: next(states))
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )
    monkeypatch.setattr(
        proof_plan,
        "_run_command",
        lambda command, _metrics: {
            "id": command.id,
            "status": "success",
            "returncode": 0,
        },
    )
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt_path = tmp_path / "receipt.json"
    assert proof_plan.execute_commands(PLAN, (command,), receipt_path) == 2
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["status"] == "failure"
    assert receipt["commands"][0]["source_tree_state_after"] == "dirty"
    assert receipt["executed_partitions"] == []


def test_heavy_queue_projects_the_same_receipt_schema(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(proof_plan, "_source_tree_state", lambda: "dirty")
    summary = tmp_path / "summary.json"
    summary.write_text(json.dumps({"peak_total": {"rss_kb": 64}}), encoding="utf-8")
    receipt = proof_queue_evidence._queue_proof_receipt(
        {
            "logical_id": "heavy-native",
            "status": "passed",
            "returncode": 0,
            "command_json": json.dumps(["cargo", "test"]),
            "cwd": str(tmp_path),
            "resource_family": "native-build",
            "started_at": "2026-07-18T00:00:00+00:00",
            "elapsed_s": 1.25,
            "summary_json": str(summary),
        }
    )
    assert receipt["schema"] == PLAN.receipt_schema
    assert receipt["authority_kind"] == "proof-queue-dynamic-command"
    assert receipt["source_tree_state"] == "dirty"
    assert receipt["executed_partitions"] == ["queue.heavy-native"]
    assert receipt["commands"][0]["peak_rss_bytes"] == 64 * 1024  # type: ignore[index]


def test_heavy_queue_reuses_receipt_context_across_rows(monkeypatch) -> None:
    calls: list[tuple[str, ...]] = []

    def fake_context(toolchains: tuple[str, ...]) -> dict[str, object]:
        calls.append(toolchains)
        return {
            "schema": PLAN.receipt_schema,
            "authority_sha256": "a" * 64,
            "source_commit": "b" * 40,
            "source_tree_state": "clean",
            "environment": {"os": "linux", "arch": "x86_64", "python": "3.12"},
            "toolchains": {name: {} for name in toolchains},
        }

    monkeypatch.setattr(proof_queue_evidence, "_queue_receipt_context", fake_context)
    row = {
        "logical_id": "heavy-native",
        "status": "passed",
        "returncode": 0,
        "command_json": json.dumps(["cargo", "test"]),
        "cwd": ".",
        "resource_family": "native-build",
        "started_at": "2026-07-18T00:00:00+00:00",
        "elapsed_s": 1.25,
        "summary_json": None,
    }
    contexts: dict[tuple[str, ...], dict[str, object]] = {}
    proof_queue_evidence._queue_proof_receipt(row, contexts=contexts)
    proof_queue_evidence._queue_proof_receipt(row, contexts=contexts)
    assert calls == [("python", "cargo", "rustc")]


def test_formal_is_required_now_that_cross_workflow_receipts_are_aggregated() -> None:
    formal = next(family for family in PLAN.families if family.name == "formal")
    assert formal.data["required"] is True
    assert {command.id for command in PLAN.commands if command.family == "formal"} == {
        "formal.lean.build",
        "formal.lean.sorry-baseline",
        "formal.quint.models",
        "formal.correspondence",
    }


def test_replay_quantifies_avoided_launches(monkeypatch) -> None:
    monkeypatch.setattr(
        proof_plan,
        "_run_git",
        lambda _args: "a\nb\n",
    )
    monkeypatch.setattr(
        proof_plan,
        "_diff_paths",
        lambda base, head: (
            ["src/molt/frontend/diagnostics.py"]
            if head == "a"
            else ["runtime/molt-runtime/src/lib.rs"]
        ),
    )
    replay = proof_plan.replay_recent_commits(PLAN, 2)
    assert replay["families"]["python_static"]["selected"] == 2
    assert replay["families"]["rust"]["selected"] == 2
    assert replay["families"]["rust_security"]["selected"] == 0
    assert replay["families"]["rust_security"]["avoidable_percent"] == 100.0
