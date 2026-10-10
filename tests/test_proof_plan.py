from __future__ import annotations
import shutil

import ast
import hashlib
import json
import os
from dataclasses import replace
from datetime import datetime, timedelta
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
from typing import Any
from types import SimpleNamespace

import pytest

from molt import tool_releases
from molt.cargo_execution_policy import PROOF_COMMAND_TIMEOUT_ENV
from molt.python_environment_identity import python_capture_authority_paths
from tools import (
    check_subprocess_guard_coverage,
    gen_proof_plan,
    proof_plan,
    toolchain_probe,
)
from tools.proof_queue_pkg import command_admission, supervisor_custody
from tools.proof_queue_pkg import custody as proof_queue_custody
from tools.proof_queue_pkg import evidence as proof_queue_evidence
from tests.process_guard_common import install_module_view, run_guarded_test_process


PLAN = proof_plan.ProofPlan.load()
_LEAN_PIN = next(
    policy.data["setup_value"]
    for policy in PLAN.toolchain_policies
    if policy.name == "lean"
)


def test_execution_authority_covers_its_transitive_python_imports() -> None:
    """Receipt-producing and classifying consumers must invalidate together."""
    root = Path(__file__).resolve().parents[1]
    pending = [
        "tools/proof_plan.py",
        "tools/proof_executor.py",
        "tools/guarded_exec.py",
        "tools/memory_guard.py",
        "tools/gen_proof_plan.py",
        "tools/generator_io.py",
    ]
    seen: set[str] = set()
    while pending:
        relative = pending.pop()
        if relative in seen:
            continue
        seen.add(relative)
        parts = Path(relative).with_suffix("").parts
        if parts[0] == "src":
            parts = parts[1:]
        package = list(parts[:-1])
        for node in ast.walk(ast.parse((root / relative).read_bytes())):
            modules = []
            if isinstance(node, ast.Import):
                modules = [alias.name for alias in node.names]
            elif isinstance(node, ast.ImportFrom):
                base = node.module or ""
                if node.level:
                    base = ".".join(
                        package[: len(package) - node.level + 1]
                        + ([base] if base else [])
                    )
                modules = [base] + [
                    base + "." + alias.name for alias in node.names if alias.name != "*"
                ]
            for module in modules:
                if module != "molt" and not module.startswith(("tools.", "molt.")):
                    continue
                parent = root / "src" if module.startswith("molt") else root
                path = parent.joinpath(*module.split("."))
                for candidate in (path.with_suffix(".py"), path / "__init__.py"):
                    if candidate.is_file():
                        pending.append(candidate.relative_to(root).as_posix())
                        break
    assert seen <= set(PLAN.authority_inputs), sorted(seen - set(PLAN.authority_inputs))
    # Fingerprint selection belongs to core toolchain authorities. Importing a
    # CLI helper executes the facade and pulls the compiler/frontend into proofs.
    assert not any(path.startswith("src/molt/cli/") for path in seen)


def test_python_capture_source_closure_is_proof_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = Path(__file__).resolve().parents[1]
    capture_paths = python_capture_authority_paths()
    monkeypatch.chdir(tmp_path)
    assert python_capture_authority_paths() == capture_paths
    assert len(capture_paths) == len(set(capture_paths))
    assert {
        root / "src/molt/__init__.py",
        root / "src/molt/_version.py",
        root / "src/sitecustomize.py",
        root / "src/molt/pytest_memory_guard_bootstrap.py",
        root / "src/molt/source_root.py",
        root / "src/molt/temporary_artifacts.py",
        root / "src/molt/file_deletion.py",
        root / "src/molt/file_locks.py",
        root / "src/molt/memory_guard_paths.py",
        root / "src/molt/process_spawn.py",
        root / "pyproject.toml",
    }.issubset(capture_paths)
    for path in capture_paths:
        assert path.is_file()
        assert path.relative_to(root).as_posix() in PLAN.authority_inputs
        if path.suffix != ".py":
            continue
        for node in ast.walk(ast.parse(path.read_bytes(), filename=str(path))):
            modules: list[str] = []
            if isinstance(node, ast.Import):
                modules = [alias.name for alias in node.names]
            elif isinstance(node, ast.ImportFrom):
                module = node.module or ""
                if node.level:
                    assert node.level == 1
                    module = f"molt.{module}".rstrip(".")
                modules = [module]
                if module == "molt":
                    modules.extend(f"molt.{alias.name}" for alias in node.names)
            for module in modules:
                if module == "molt":
                    dependency = root / "src/molt/__init__.py"
                elif module.startswith("molt."):
                    dependency = root / "src" / (module.replace(".", "/") + ".py")
                else:
                    continue
                assert dependency in capture_paths, (path, dependency)


def _sealed_terminal_row(row: dict[str, Any]) -> dict[str, Any]:
    """Construct one completed queue row and seal its matching parent outcome."""
    run_id = row["run_id"]
    context = json.loads(row["receipt_context_json"])
    toolchains = context.get("toolchains")
    if isinstance(toolchains, dict):
        context["toolchain_custody"] = {
            "prelaunch": toolchains,
            "postcompletion": toolchains,
            "identical": True,
        }
    envelope = context.get("command_envelope")
    process_closure = (
        envelope.get("process_closure") if isinstance(envelope, dict) else None
    )
    if (
        sys.platform == "win32"
        and isinstance(process_closure, dict)
        and process_closure.get("descendants") == "declared-toolchains"
    ):
        platform_images = [
            {
                "path": sys.executable,
                "sha256": hashlib.sha256(sys.executable.encode()).hexdigest(),
            }
        ]
        context["platform_process_custody"] = {
            "prelaunch": platform_images,
            "postcompletion_sha256": (
                supervisor_custody._canonical_payload_sha256(platform_images)
            ),
            "identical": True,
        }
    context.update(
        {
            "run_id": run_id,
            "execution_nonce_sha256": "9" * 64,
            "queue_terminal": {
                "schema": supervisor_custody.QUEUE_TERMINAL_SCHEMA,
                "status": row["status"],
                "returncode": row["returncode"],
                "command_returncode": row["returncode"],
                "execution_error": None,
            },
        }
    )
    context["terminal_evidence_sha256"] = supervisor_custody.terminal_evidence_sha256(
        context,
        run_id=run_id,
        returncode=row["returncode"],
    )
    return {
        **row,
        "finished_at": (
            datetime.fromisoformat(row["started_at"])
            + timedelta(seconds=row["elapsed_s"])
        ).isoformat(),
        "receipt_context_json": json.dumps(context),
    }


def _classes(*paths: str) -> dict[str, bool]:
    selected = {family.name for family in PLAN.select(paths).selected}
    return {family.name: family.name in selected for family in PLAN.families}


def test_manifest_is_complete_and_single_authority() -> None:
    assert PLAN.path.name == "proof_plan.toml"
    assert len(PLAN.families) == 11
    assert len(PLAN.scheduled_families) == 8
    assert len(PLAN.commands) >= 84
    assert len(PLAN.matrix_cells) >= 17
    assert len(PLAN.toolchain_policies) >= 15
    assert "src/molt/cargo_execution_policy.py" in PLAN.authority_inputs
    assert PLAN.executor_max_workers == 4
    assert PLAN.inventory_hash_workers == 12
    assert {policy.name: policy.max_parallel for policy in PLAN.resource_policies} == {
        "compiler-build-resource": 1,
        "formal-tools": 2,
        "network-audit": 2,
        "python-static": 1,
        "python-tests": 2,
        "repository-policy": 4,
        "scheduled-suite": 4,
        "wasm-runtime": 2,
    }
    assert all("metadata_mode" not in family.data for family in PLAN.families)
    assert all(family.data["required"] for family in PLAN.families)
    assert all(family.data["dependencies"] == [] for family in PLAN.families)
    assert all(
        family.data["admission_workflow"] == ".github/workflows/ci.yml"
        for family in PLAN.families
    )
    assert all(cell.data["runner"] for cell in PLAN.matrix_cells)
    assert len(PLAN.local_rules) >= 30
    assert not (proof_plan.ROOT / "tools" / "molt_dev_gates.toml").exists()
    assert not (proof_plan.ROOT / "tools" / "ci_changed_paths.py").exists()


def test_lean_cache_is_ignored_untracked_build_state() -> None:
    tracked = proof_plan._run_git(["ls-files", "formal/lean/.lake"])
    assert tracked.strip() == ""
    assert "formal/lean/.lake/" in (proof_plan.ROOT / ".gitignore").read_text(
        encoding="utf-8"
    )


def test_generated_local_dx_projection_has_stable_command_ids() -> None:
    projection = json.loads(gen_proof_plan._json_projection(PLAN))
    assert projection["schema"] == "molt.proof-plan-projection.v7"
    assert projection["receipt_schema"] == "molt.proof-receipt.v4"
    assert projection["authority_inputs"] == list(PLAN.authority_inputs)
    assert projection["authority_sha256"] == proof_plan._authority_sha256(PLAN)
    assert projection["toolchain_policies"] == [
        policy.data for policy in PLAN.toolchain_policies
    ]
    assert projection["cargo_execution_policy"]["timeout_seconds_by_class"] == {
        "cold": 1200,
        "cross-check": 240,
        "integration": 600,
        "suite": 1800,
        "shipping": 9000,
        "warm": 300,
    }
    assert projection["cargo_execution_policy"]["measurement_job_id"] == 89_813_773_652
    assert (
        projection["cargo_execution_policy"]["owning_proof_timeout_environment"]
        == PROOF_COMMAND_TIMEOUT_ENV
    )
    assert projection["cargo_environment_policy"] == {
        "wrapper_environment_names": [
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        ],
        "incident_run_id": 30_211_145_633,
        "incident_job_id": 89_817_499_999,
        "incident_commit": "20b046b79bd4ca64a8c859f737f6e330377bcc4e",
        "incident_command": "cargo metadata --locked --format-version 1",
    }
    assert projection["executor"]["max_workers"] == 4
    assert projection["executor"]["inventory_hash_workers"] == 12
    assert projection["executor"]["resource_policies"] == [
        {"name": policy.name, "max_parallel": policy.max_parallel}
        for policy in PLAN.resource_policies
    ]
    assert projection["scheduled_families"] == [
        family.data for family in PLAN.scheduled_families
    ]
    timeout_envelopes = projection["executor"]["github_job_timeout_envelopes"]
    scheduled_envelopes = projection["executor"]["scheduled_job_timeout_envelopes"]
    # Projections follow the canonical topology. Exact scheduling arithmetic is
    # covered by the synthetic dependency/resource tests, not a second copy of
    # every production command's aggregate timeout here.
    for families, projected_envelopes in (
        (
            tuple(
                family
                for family in PLAN.families
                if family.data["executor"] == "github-job"
            ),
            timeout_envelopes,
        ),
        (PLAN.scheduled_families, scheduled_envelopes),
    ):
        assert set(projected_envelopes) == {family.name for family in families}
        for family in families:
            envelope = PLAN.timeout_envelope(family.name)
            budget = int(family.data["timeout_minutes"]) * 60
            reserve = int(family.data["job_reserve_seconds"])
            assert projected_envelopes[family.name] == {
                "budget_seconds": budget,
                "projected_makespan_seconds": envelope.projected_makespan_seconds,
                "critical_path_seconds": envelope.critical_path_seconds,
                "resource_capacity_floor_seconds": envelope.resource_capacity_floor_seconds,
                "job_reserve_seconds": reserve,
                "required_job_seconds": envelope.projected_makespan_seconds + reserve,
                "headroom_seconds": budget
                - envelope.projected_makespan_seconds
                - reserve,
            }
    matrix_envelopes = projection["executor"]["github_matrix_timeout_envelopes"]
    matrix_families = tuple(
        family for family in PLAN.families if family.data["executor"] == "github-matrix"
    )
    assert set(matrix_envelopes) == {family.name for family in matrix_families}
    for family in matrix_families:
        budget = int(family.data["timeout_minutes"]) * 60
        assert set(matrix_envelopes[family.name]) == set(PLAN.family_cells(family.name))
        for cell, projected in matrix_envelopes[family.name].items():
            envelope = PLAN.timeout_envelope(family.name, matrix_cell=cell)
            assert projected["budget_seconds"] == budget
            assert (
                projected["projected_makespan_seconds"]
                == envelope.projected_makespan_seconds
            )
            reserve = int(family.data["job_reserve_seconds"])
            assert projected["job_reserve_seconds"] == reserve
            assert projected["required_job_seconds"] == (
                envelope.projected_makespan_seconds + reserve
            )
            assert projected["headroom_seconds"] == (
                budget - envelope.projected_makespan_seconds - reserve
            )
            assert projected["headroom_seconds"] >= 0
    local = projection["local"]
    assert local["commands"]["local.always.0"] == PLAN.always[0]
    first = PLAN.local_rules[0]
    projected = next(rule for rule in local["rules"] if rule["name"] == first["name"])
    assert projected["command_ids"] == [
        f"local.{first['name']}.{index}" for index, _ in enumerate(first["gates"])
    ]


def test_compiler_runtime_partition_preserves_disjoint_test_and_tool_ownership() -> (
    None
):
    commands = {command.id: command for command in PLAN.commands}
    core = commands["rust.test.ir-wasm-runtime-authorities"]
    complement = commands["rust.test.compiler-authorities"]

    def test_packages(command: proof_plan.ProofCommand) -> set[str]:
        argv = command.argv
        return {argv[index + 1] for index, arg in enumerate(argv[:-1]) if arg == "-p"}

    assert test_packages(core) == {"molt-passes", "molt-backend-wasm", "molt-runtime"}
    assert not test_packages(core) & test_packages(complement)
    assert core.dependencies == complement.dependencies == ()
    assert set(core.toolchains) == {"cargo", "node", "wasm-ld"}
    assert "--lib" in core.argv
    assert [
        core.argv[index + 1]
        for index, arg in enumerate(core.argv[:-1])
        if arg == "--test"
    ] == ["ownership_memory_contracts", "test_builtins"]
    assert "--bins" not in core.argv
    assert "--include-ignored" not in core.argv
    assert {
        complement.argv[index + 1]
        for index, arg in enumerate(complement.argv[:-1])
        if arg == "--test"
    } == {
        "generated_artifact_custody",
        "ir_contract_validation",
        "native_artifact_facts",
    }
    for command in (core, complement):
        assert "--nocapture" in command.argv[command.argv.index("--") + 1 :]
    assert "profile.dev-fast.package.molt-runtime.opt-level=0" in core.argv
    filters = core.argv[core.argv.index("--") + 1 :]
    assert {
        "wasm_abi::",
        "async_rt::channels::",
        "async_rt::net_stubs::",
        "intrinsics::registry::",
    } <= set(filters)
    assert {
        "tir::",
        "wasm::",
        "call::",
        "object::",
        "arena::",
        "builtins::attr::",
        "builtins::classes::",
    }.issubset(filters)


def test_compression_export_proofs_select_both_feature_coordinates() -> None:
    from tools import run_cargo_test_truth

    commands = {command.id: command for command in PLAN.commands}
    truth = commands["rust.test.default-truth"]
    assert truth.argv == (
        "uv",
        "run",
        "--frozen",
        "python3",
        "tools/run_cargo_test_truth.py",
    )
    # The canonical workspace test command includes test_builtins with default
    # features and wasm_cdylib_exports, whose existing artifact owner tests the
    # compression-disabled/enabled WASM pair. A --lib-only rewrite loses both.
    assert run_cargo_test_truth.CANONICAL_COMMAND == (
        "cargo",
        "test",
        "--locked",
        "--workspace",
        "--tests",
        "--no-fail-fast",
    )
    micro = commands["rust.test.ir-wasm-runtime-authorities"]
    cargo = micro.argv[: micro.argv.index("--")]
    filters = micro.argv[micro.argv.index("--") + 1 :]
    assert "--no-default-features" in cargo and "--no-run" not in cargo
    assert cargo[cargo.index("--features") + 1].split(",") == [
        "molt-passes/native-backend",
        "molt-passes/wasm-backend",
        "molt-passes/test-util",
        "molt-backend-wasm/test-util",
        "molt-runtime/stdlib_micro",
        "molt-runtime/builtin_complex",
        "molt-runtime/builtin_set",
    ]
    assert "test_builtins" in [
        cargo[index + 1] for index, arg in enumerate(cargo[:-1]) if arg == "--test"
    ]
    assert "test_raw_compression_c_exports_are_linkable" in filters


def test_extension_admission_proof_executes_required_public_resolver_witness() -> None:
    import tomllib

    command = next(
        item
        for item in PLAN.commands
        if item.id == "rust.test.runtime-extension-admission"
    )
    cargo, libtest = (
        command.argv[: command.argv.index("--")],
        command.argv[command.argv.index("--") + 1 :],
    )
    assert cargo[:2] == ("cargo", "test")
    assert cargo[cargo.index("-p") + 1] == "molt-runtime"
    assert cargo.count("-p") == 1 and cargo.count("--lib") == 1
    assert "--no-default-features" not in cargo and "--no-run" not in cargo
    assert cargo[cargo.index("--features") + 1] == "cext_loader"
    target = cargo[cargo.index("--target") + 1]
    assert target == "x86_64-unknown-linux-gnu"
    assert libtest == ("builtins::platform::tests::", "--nocapture", "--test-threads=1")
    config = {}
    for index, argument in enumerate(cargo[:-1]):
        if argument == "--config":
            config.update(tomllib.loads(cargo[index + 1]))
    runner = config["target"][target]["runner"]
    assert runner[:4] == [
        "uv",
        "run",
        "--frozen",
        "python3",
    ]
    root = Path(__file__).resolve().parents[1]
    package_root = root / "runtime/molt-runtime"
    assert (
        package_root / runner[4]
    ).resolve() == root / "tools/cargo_test_binary_runner.py"
    receipt_root = (package_root / runner[runner.index("--receipt-dir") + 1]).resolve()
    assert runner[-1] == "--"
    assert [
        runner[index + 1]
        for index, argument in enumerate(runner[:-1])
        if argument == "--require-passed-test"
    ] == [
        "builtins::platform::tests::resolver_admission_tests::"
        "public_import_resolver_miss_and_error_do_not_load_extension_candidates"
    ]
    assert command.data["cell"] == "linux-x86_64-rust-native-dev"
    assert command.dependencies == ()
    assert command.data["evidence_outputs"] == [
        receipt_root.relative_to(root).as_posix()
    ]
    job = proof_plan._workflow_job_block(
        (root / ".github/workflows/ci.yml").read_text(encoding="utf-8"),
        "rust-build-unit-smoke",
    )
    assert job is not None
    assert command.data["evidence_outputs"][0] + "/" in job
    assert proof_plan.cargo_native_c_units(command.data) == ("target",)
    assert {"python", "uv", "rustc", "cargo"} <= set(PLAN.required_toolchains(command))


def test_shipping_runtime_gate_requires_full_parallel_and_fresh_child_accounting() -> (
    None
):
    commands = {command.id: command for command in PLAN.commands}
    core = commands["rust.test.ir-wasm-runtime-authorities"]
    shipping = commands["rust.test.runtime-cold-lifecycle"]
    assert shipping.family == core.family
    assert shipping.data["cell"] == "linux-x86_64-rust-native-release-output"
    assert shipping.data["tiers"] == core.data["tiers"]
    assert shipping.dependencies == ()
    assert shipping.data["timeout_budget"] == "shipping"
    assert shipping.data["timeout_seconds"] == 9000
    assert shipping.data["resource_class"] == core.data["resource_class"]
    assert {"python", "uv", "cargo", "rustc", "git"} <= set(
        PLAN.required_toolchains(shipping)
    )
    assert shipping.argv == (
        "uv",
        "run",
        "--frozen",
        "python3",
        "tools/run_runtime_test_gate.py",
        "--profile",
        "release-output",
        "--build-timeout-seconds",
        "6300",
        "--child-timeout-seconds",
        "120",
        "--parallel-threads",
        "8",
    )
    for path in (
        "tools/run_runtime_test_gate.py",
        "tests/tools/test_runtime_test_gate.py",
        "tests/tools/test_cargo_test_binary_discovery.py",
    ):
        assert path in PLAN.authority_inputs
        assert _classes(path)["rust"] is True


def test_runtime_descendant_authority_is_hashed_and_its_tests_execute() -> None:
    for path in (
        "tools/runtime_descendant_receipts.py",
        "tests/tools/test_runtime_descendant_receipts.py",
        "tests/runtime_descendant_test_support.py",
    ):
        assert path in PLAN.authority_inputs
        assert _classes(path)["rust"] is True
    # The Rust producer is test source of the runtime family it records.
    assert _classes("runtime/test_support/captured_runtime_children.rs")["rust"] is True
    executed = {part for command in PLAN.commands for part in command.argv}
    for path in (
        "tests/tools/test_runtime_descendant_receipts.py",
        "tests/tools/test_runtime_test_gate.py",
        "tests/tools/test_cargo_test_truth.py",
    ):
        assert path in executed


def test_libtest_accounting_is_hashed_and_selects_rust_consumers() -> None:
    path = "tools/libtest_results.py"
    assert path in PLAN.authority_inputs
    assert _classes(path)["rust"] is True


@pytest.mark.parametrize(
    "path", ["Cargo.toml", ".cargo/config.toml", "runtime/molt-wasm-host/Cargo.toml"]
)
def test_cargo_profile_authority_selects_its_complete_contract_tests(path: str) -> None:
    assert _classes(path)["python_unit"] is True
    command = next(
        command
        for command in PLAN.commands
        if command.id == "python.unit.runtime-artifacts"
    )
    for test_path in (
        "tests/cli/test_backend_manifest_contract.py",
        "tests/cli/test_runtime_build_identity.py",
        "tests/cli/test_runtime_family_authority.py",
        "tests/test_cargo_workspace.py",
        "tests/test_cli_build_profile_policy.py",
    ):
        assert command.argv.count(test_path) == 1


def test_docs_only_change_skips_compiler_proofs() -> None:
    classes = _classes("docs/agent/INDEX.md")
    assert classes["repository_policy"] is True
    assert not any(
        selected for name, selected in classes.items() if name != "repository_policy"
    )


def test_llvm_proofs_select_implementation_libtests_and_driver_link_consumer() -> None:
    commands = {command.id: command for command in PLAN.commands}
    owners = {
        "llvm.test.lowering": {"molt-backend-native", "molt-backend"},
        "llvm.clippy.backend": {"molt-backend", "molt-backend-native"},
        "linker.test.generated-object-admission": {"molt-backend"},
    }
    for command_id, expected_packages in owners.items():
        argv = commands[command_id].argv
        packages = {
            argv[index + 1] for index, arg in enumerate(argv[:-1]) if arg == "-p"
        }
        assert packages == expected_packages, command_id
    lowering = commands["llvm.test.lowering"].argv
    assert "--lib" in lowering
    assert "llvm_backend::lowering" in lowering
    assert "llvm_backend::runtime_imports" in lowering
    assert lowering[lowering.index("--test") + 1] == "ir_contract_validation"
    assert "direct_checked_backends_share_generated_shape_rejection" in lowering
    assert (
        "retired_operations_are_rejected_at_wire_isolated_and_checked_backend_boundaries"
        in lowering
    )
    assert "molt-backend/llvm" in lowering[lowering.index("--features") + 1].split(",")
    linkage = commands["linker.test.generated-object-admission"].argv
    assert linkage[linkage.index("--test") + 1] == "llvm_generated_object_linkage"


def test_python_source_change_selects_split_proof_topology() -> None:
    classes = _classes("src/molt/cli/runtime_wasm_cache.py")
    assert classes["repository_policy"] is True
    assert classes["python_static"] is True
    assert classes["python_unit"] is True
    assert classes["native_integration"] is True
    assert classes["wasm"] is True
    assert classes["rust"] is False
    assert classes["python_security"] is False
    assert classes["rust_security"] is False
    assert classes["formal"] is False


@pytest.mark.parametrize(
    "path",
    [
        "tests/tools/test_proof_queue_native_receipts.py",
        "tests/proof_queue_custody_test_support.py",
        "tests/python_environment_test_support.py",
        "tools/proof_queue_pkg/runner.py",
        "tools/proof_supervisor/src/main.rs",
    ],
)
def test_native_receipt_contract_selects_existing_integration_batch(path: str) -> None:
    assert _classes(path)["native_integration"] is True
    selector = "tests/tools/test_proof_queue_native_receipts.py"
    owners = [command for command in PLAN.commands if selector in command.argv]
    assert len(owners) == 1
    command = owners[0]
    assert command.family == "native_integration"
    assert command.data["resource_class"] == "compiler-build-resource"
    # It is the first compiler build on the PR tier, where bench-cli does not run.
    assert command.data["timeout_budget"] == "cold"


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
        "config/release_acceptance_matrix.toml",
        "src/molt/release_lanes.py",
        "src/molt/cli/compiler_identity.py",
        "src/molt/cli/installed_runtime.py",
        "tools/release/native_build.py",
        "vendor/llvm/LICENSE.TXT",
    ):
        assert _classes(path)["llvm"] is True, path


def test_release_lane_changes_exercise_installed_and_source_consumers() -> None:
    commands = {command.id: command for command in PLAN.commands}
    for path in ("src/molt/release_lanes.py", "tools/release/native_build.py"):
        classes = _classes(path)
        assert classes["repository_policy"] and classes["python_unit"]
    for selector in (
        "tests/test_release_lanes.py",
        "tests/tools/test_release_native_build.py",
        "tests/tools/test_release_installed_distribution.py",
        "tests/tools/test_release_exit_gate.py",
    ):
        assert commands["repository.release-supply-chain"].argv.count(selector) == 1
    for suffix in ("", ".macos"):
        argv = commands[f"python.unit.runtime-artifacts{suffix}"].argv
        for selector in (
            "tests/test_llvm_toolchain.py",
            "tests/cli/test_compiler_identity.py",
            "tests/cli/test_installed_compiler.py",
            "tests/cli/test_installed_runtime.py",
            "tests/cli/test_cli_backend_output_pipeline_authority.py",
            "tests/cli/test_native_object_publication.py::test_native_object_publication_is_one_admitted_transaction",
        ):
            assert argv.count(selector) == 1


def test_selected_family_does_not_pull_unrelated_proof_families() -> None:
    selection = PLAN.select([".github/workflows/perf-gate.yml"])
    assert [family.name for family in selection.selected] == [
        "repository_policy",
        "llvm",
    ]
    assert selection.reasons["llvm"] == (".github/workflows/perf-gate.yml",)
    assert "rust" not in selection.reasons


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


def test_matrix_command_dependencies_cannot_cross_runner_cells() -> None:
    commands = tuple(
        replace(
            command,
            data={
                **command.data,
                "dependencies": ["portability.queue.linux"],
            },
        )
        if command.id == "portability.queue.macos"
        else command
        for command in PLAN.commands
    )

    errors = replace(PLAN, commands=commands).validate()
    assert (
        "portability.queue.macos: matrix command dependency "
        "'portability.queue.linux' crosses runner cells"
    ) in errors


def test_python_unit_runs_the_same_partitions_on_linux_and_macos() -> None:
    family = next(family for family in PLAN.families if family.name == "python_unit")
    assert family.data["executor"] == "github-matrix"
    cells = {cell.id: cell for cell in PLAN.matrix_cells}
    by_cell: dict[str, list[proof_plan.ProofCommand]] = {}
    for command in PLAN.commands:
        if command.family == "python_unit":
            by_cell.setdefault(command.data["cell"], []).append(command)
    assert {
        cell: (cells[cell].data["os"], cells[cell].data["runner"]) for cell in by_cell
    } == {
        "linux-x86_64-py312-unit": ("linux", "ubuntu-latest"),
        "macos-arm64-py312-unit": ("macos", "macos-14"),
    }
    linux = by_cell["linux-x86_64-py312-unit"]
    macos = by_cell["macos-arm64-py312-unit"]

    def contract(command: proof_plan.ProofCommand) -> dict[str, object]:
        return {
            key: value
            for key, value in command.data.items()
            if key not in {"id", "cell"}
        }

    # One macOS twin per Linux partition, identical apart from identity.
    assert [command.id + ".macos" for command in linux] == [
        command.id for command in macos
    ]
    assert [contract(command) for command in linux] == [
        contract(command) for command in macos
    ]


def test_each_matrix_family_receives_only_its_own_runner_cells() -> None:
    # tools/** selects both matrix families at once.
    outputs = proof_plan.family_outputs(PLAN, PLAN.select(["tools/proof_queue.py"]))
    assert "matrix" not in outputs
    unit = json.loads(outputs["python_unit_matrix"])["include"]
    assert [(entry["cell"], entry["runner"]) for entry in unit] == [
        ("linux-x86_64-py312-unit", "ubuntu-latest"),
        ("macos-arm64-py312-unit", "macos-14"),
    ]
    for entry in unit:
        assert entry["family"] == "python_unit"
        commands = proof_plan._topological_commands(
            PLAN, family="python_unit", matrix_cell=entry["cell"]
        )
        assert [command.id for command in commands] == entry["command_ids"]
    portability = json.loads(outputs["platform_portability_matrix"])["include"]
    assert portability
    assert all(entry["family"] == "platform_portability" for entry in portability)


def test_local_matrix_family_run_selects_only_the_host_cell(monkeypatch) -> None:
    monkeypatch.setattr(proof_plan, "_normalized_os", lambda: "macos")
    monkeypatch.setattr(proof_plan, "_normalized_arch", lambda: "aarch64")
    assert proof_plan.host_matrix_cell(PLAN, "python_unit") == "macos-arm64-py312-unit"
    monkeypatch.setattr(proof_plan, "_normalized_os", lambda: "linux")
    with pytest.raises(ValueError) as error:
        proof_plan.host_matrix_cell(PLAN, "python_unit")
    assert str(error.value) == (
        "python_unit runs one job per matrix cell and no unique cell matches host "
        "linux/aarch64; pass --matrix-cell with one of: "
        "linux-x86_64-py312-unit (linux/x86_64), macos-arm64-py312-unit (macos/aarch64)"
    )


def test_run_family_without_cell_executes_only_the_host_cell(
    tmp_path, monkeypatch, capsys
) -> None:
    executed: list[str] = []

    def record(_plan, commands, _receipt):
        executed.extend(command.id for command in commands)
        return 0

    monkeypatch.setattr(proof_plan, "execute_commands", record)
    monkeypatch.setattr(proof_plan, "_normalized_os", lambda: "macos")
    monkeypatch.setattr(proof_plan, "_normalized_arch", lambda: "aarch64")
    receipt = tmp_path / "receipt.json"
    assert (
        proof_plan.main(["--run-family", "python_unit", "--receipt", str(receipt)]) == 0
    )
    assert executed == [
        command.id
        for command in PLAN.commands
        if command.family == "python_unit"
        and command.data["cell"] == "macos-arm64-py312-unit"
    ]
    assert "runs host matrix cell macos-arm64-py312-unit" in capsys.readouterr().err


def test_github_job_family_cannot_span_runners() -> None:
    families = tuple(
        replace(family, data={**family.data, "executor": "github-job"})
        if family.name == "python_unit"
        else family
        for family in PLAN.families
    )
    errors = replace(PLAN, families=families).validate()
    assert (
        "python_unit: a github-job runs every command on one runner, but its "
        "cells name ['macos-14', 'ubuntu-latest']; use executor github-matrix"
    ) in errors


def test_matrix_job_must_consume_its_own_family_matrix(tmp_path) -> None:
    workflow = tmp_path / "ci.yml"
    workflow.write_text(
        (proof_plan.ROOT / ".github/workflows/ci.yml")
        .read_text(encoding="utf-8")
        .replace(
            "outputs.python_unit_matrix) }}", "outputs.platform_portability_matrix) }}"
        ),
        encoding="utf-8",
    )
    families = tuple(
        replace(family, data={**family.data, "workflow": str(workflow)})
        if family.name == "python_unit"
        else family
        for family in PLAN.families
    )
    errors = replace(PLAN, families=families).validate()
    assert (
        "python_unit: matrix workflow job does not contain 'matrix: "
        "${{ fromJSON(needs.classify-changes.outputs.python_unit_matrix) }}'"
    ) in errors


def test_matrix_family_budget_binds_each_cell() -> None:
    commands = tuple(
        replace(command, data={**command.data, "timeout_seconds": 2401})
        if command.id == "python.unit.harness.macos"
        else command
        for command in PLAN.commands
    )
    errors = replace(PLAN, commands=commands).validate()
    # Commands take the first free slot in declaration order. Harness (2401 s)
    # holds slot 1; custody (300), binding (300), frontend (600), CLI (900)
    # and surface contracts (600) fill slot 2 until 2700 s; runtime-artifacts
    # (600) takes slot 1 at 2401 s and ends at 3001 s; the 120 s boundary
    # partition takes slot 2 at 2700 s. The makespan is 3001 s. The Linux job
    # is unchanged, so only the macOS cell exceeds its 41-minute budget.
    assert [error for error in errors if "timeout envelope" in error] == [
        "python_unit: projected resource-aware timeout envelope 3001s in matrix "
        "cell macos-arm64-py312-unit plus job reserve 60s exceeds GitHub job budget 2460s"
    ]


@pytest.mark.parametrize(
    ("family_name", "reserve"),
    [
        ("wasm", None),
        ("wasm", True),
        ("python_unit", 0),
        ("python_unit", -1),
        ("nightly_determinism", 1.5),
        ("nightly_determinism", "60"),
    ],
)
def test_job_reserve_requires_an_explicit_positive_integer(
    family_name, reserve
) -> None:
    def alter(family):
        if family.name != family_name:
            return family
        data = dict(family.data)
        if reserve is None:
            del data["job_reserve_seconds"]
        else:
            data["job_reserve_seconds"] = reserve
        return replace(family, data=data)

    plan = replace(
        PLAN,
        families=tuple(alter(family) for family in PLAN.families),
        scheduled_families=tuple(alter(family) for family in PLAN.scheduled_families),
    )
    assert (
        f"{family_name}: job_reserve_seconds must be a positive integer"
        in plan.validate()
    )


def test_job_reserve_is_not_a_workflow_wide_budget() -> None:
    families = tuple(
        replace(family, data={**family.data, "job_reserve_seconds": 60})
        if family.name == "formal"
        else family
        for family in PLAN.families
    )
    assert (
        "formal: job_reserve_seconds requires a modeled job"
        in replace(PLAN, families=families).validate()
    )


@pytest.mark.parametrize(
    ("command_id", "deadline", "expected_error"),
    [
        (
            "python.static.ty",
            300,
            "python_static: projected resource-aware timeout envelope 901s "
            "plus job reserve 60s exceeds GitHub job budget 960s",
        ),
        (
            "nightly.shards.profile-feedback",
            300,
            "nightly_shard_profile_feedback: projected resource-aware timeout "
            "envelope 301s plus job reserve 300s exceeds scheduled job budget 600s",
        ),
    ],
)
def test_command_schedule_cannot_consume_the_job_reserve(
    command_id, deadline, expected_error
) -> None:
    def at_deadline(value):
        return replace(
            PLAN,
            commands=tuple(
                replace(command, data={**command.data, "timeout_seconds": value})
                if command.id == command_id
                else command
                for command in PLAN.commands
            ),
        )

    # The selected command, existing sibling work and reserve fill the workflow cap.
    # One more second of command work consumes the reserved time.
    assert at_deadline(deadline).validate() == []
    assert at_deadline(deadline + 1).validate() == [expected_error]


def test_github_job_timeout_covers_resource_aware_dag_envelope() -> None:
    for family in PLAN.families:
        if family.data["executor"] != "github-job":
            continue
        envelope = PLAN.timeout_envelope(family.name)
        budget = int(family.data["timeout_minutes"]) * 60
        assert (
            envelope.projected_makespan_seconds
            + int(family.data["job_reserve_seconds"])
            <= budget
        )
        assert envelope.critical_path_seconds <= envelope.projected_makespan_seconds
        assert max(envelope.resource_capacity_floor_seconds.values()) <= (
            envelope.projected_makespan_seconds
        )

    # Both first-build fixtures may compile from an empty dependency cache.
    # On main they serialize under the same compiler resource; the unrelated
    # Python custody row runs beside them and cannot make either build warm.
    native_builds = [
        command
        for command in PLAN.commands
        if command.id
        in {"native.integration.bench-cli", "native.integration.capability-manifest"}
    ]
    native_build_seconds = sum(
        int(command.data["timeout_seconds"]) for command in native_builds
    )
    native_envelope = PLAN.timeout_envelope("native_integration")
    assert native_envelope.projected_makespan_seconds == native_build_seconds
    assert (
        native_envelope.resource_capacity_floor_seconds["compiler-build-resource"]
        == native_build_seconds
    )

    repository_declared = sum(
        int(command.data["timeout_seconds"])
        for command in PLAN.commands
        if command.family == "repository_policy"
    )
    repository_budget = next(
        int(family.data["timeout_minutes"]) * 60
        for family in PLAN.families
        if family.name == "repository_policy"
    )
    assert repository_declared > repository_budget
    assert (
        PLAN.timeout_envelope("repository_policy").projected_makespan_seconds
        < repository_budget
    )

    families = tuple(
        replace(
            family,
            data={**family.data, "timeout_minutes": 1},
        )
        if family.name == "repository_policy"
        else family
        for family in PLAN.families
    )
    errors = replace(PLAN, families=families).validate()
    assert any(
        "repository_policy: projected resource-aware timeout envelope" in error
        for error in errors
    )


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


def test_linker_process_helper_policy_requires_unique_basenames() -> None:
    policies = tuple(
        replace(
            policy,
            data={
                **policy.data,
                "linker_process_helpers": {"nested/link.exe": ["../vctip.exe"]},
            },
        )
        if policy.name == "rustc"
        else policy
        for policy in PLAN.toolchain_policies
    )
    errors = replace(PLAN, toolchain_policies=policies).validate()
    assert any("linker helper key must be a basename" in error for error in errors)


def test_linker_build_tool_policy_requires_typed_unique_basenames() -> None:
    policies = tuple(
        replace(
            policy,
            data={
                **policy.data,
                "linker_build_tools": {
                    "link.exe": {"nested/cl.exe": "compiler", "cl.exe": "compiler"}
                },
            },
        )
        if policy.name == "rustc"
        else policy
        for policy in PLAN.toolchain_policies
    )
    errors = replace(PLAN, toolchain_policies=policies).validate()
    assert any("must map unique basenames" in error for error in errors)


def test_cargo_toolchain_declares_complete_process_dependency_closure() -> None:
    cargo = next(policy for policy in PLAN.toolchain_policies if policy.name == "cargo")

    assert cargo.data["dependencies"] == ["rustc", "git"]
    assert PLAN.toolchain_closure(["cargo"]) == ("cargo", "rustc", "git")
    cargo_command = next(
        command for command in PLAN.commands if command.argv[:2] == ("cargo", "build")
    )
    required = PLAN.required_toolchains(cargo_command)
    assert set(required) == {*cargo_command.toolchains, "git"}
    assert len(required) == len(set(required))
    assert required.index("git") > required.index("cargo")
    rustc = next(policy for policy in PLAN.toolchain_policies if policy.name == "rustc")
    assert rustc.data["linker_build_tools"] == {
        "link.exe": {
            "cl.exe": "rust-build-c-compiler",
            "lib.exe": "rust-build-archiver",
        }
    }


@pytest.mark.parametrize(
    "consumer_id",
    ["wasm.compile.hello", "wasm.compile.comprehension", "wasm.compile.sieve"],
)
def test_wasm_backend_prewarm_admits_the_consumer_compiler(
    consumer_id: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    by_id = {command.id: command for command in PLAN.commands}
    prewarm = by_id["wasm.build.backend"]
    consumer = by_id[consumer_id]
    # Guest dev output does not select the host compiler's profile. The CLI
    # admission command owns that selection and its content/probe receipts;
    # bare Cargo output, even with matching features, cannot prewarm it.
    assert prewarm.argv[:5] == consumer.argv[:5]
    assert prewarm.argv[5:] == (
        "internal-backend-build",
        "--target",
        "wasm",
        "--json",
    )
    assert consumer.argv[consumer.argv.index("--target") + 1] == "wasm"
    assert consumer.argv[consumer.argv.index("--build-profile") + 1] == "dev"
    assert {"python", "uv", "rustc", "cargo", "ld.lld", "wasm-ld"}.issubset(
        PLAN.required_toolchains(prewarm)
    )
    dependency_order = [
        command.id
        for command in proof_plan._topological_commands(PLAN, command_id=consumer_id)
    ]
    assert dependency_order.index(prewarm.id) < dependency_order.index(consumer_id)
    assert (prewarm.data["timeout_budget"], prewarm.data["timeout_seconds"]) == (
        "cold",
        1200,
    )
    assert (consumer.data["timeout_budget"], consumer.data["timeout_seconds"]) == (
        "warm",
        300,
    )
    # Both entry points must inherit host-profile/session overrides identically
    # and the selected compiler wrapper while disabling the backend daemon.
    inherited = {
        "MOLT_BACKEND_PROFILE": "release",
        "MOLT_RELEASE_BACKEND_CARGO_PROFILE": "release-fast",
        "MOLT_SESSION_ID": "wasm-prewarm-test",
        "CARGO_TARGET_DIR": "/owned/cargo-target",
    }
    for name, value in inherited.items():
        monkeypatch.setenv(name, value)
    monkeypatch.setenv("RUSTC_WRAPPER", "/opt/cache/sccache")
    monkeypatch.setenv("MOLT_BACKEND_DAEMON", "1")
    for command in (prewarm, consumer):
        environment, _policies = proof_plan._command_environment(
            PLAN, command, command.data["timeout_seconds"]
        )
        assert {name: environment[name] for name in inherited} == inherited
        assert environment["RUSTC_WRAPPER"] == "/opt/cache/sccache"
        assert environment["CARGO_INCREMENTAL"] == "0"
        assert environment["MOLT_BACKEND_DAEMON"] == "0"


def test_wasm_python_consumers_share_prebuild_entrypoint_and_wrapper_selection(
    monkeypatch,
) -> None:
    rows = {row.id: row for row in PLAN.commands if row.family == "wasm"}
    expected_pytest_rows = {
        "wasm.host.runner-fixtures",
        "wasm.test.startup-lifecycle",
        "wasm.test.linker-admission",
        "wasm.test.control-flow",
        "wasm.integration.split-runtime",
        "wasm.integration.host-exports.gpu-kernel",
        "wasm.integration.host-exports.attribute-error",
        "wasm.integration.host-exports.tinygrad-dtype",
        "wasm.integration.host-exports.tinygrad-tensor",
        "wasm.integration.host-exports.tensor-row-ops",
        "wasm.test.freestanding-e2e",
        "wasm.test.finally-pending-observer-parity",
    }
    assert {
        name for name, row in rows.items() if "pytest" in row.argv
    } == expected_pytest_rows
    for name in expected_pytest_rows:
        assert rows[name].argv[:5] == (
            "python3",
            "tools/venv_exec.py",
            "python3",
            "-m",
            "pytest",
        )
    for name in (
        "wasm.build.backend",
        "wasm.build.shared-runtime",
        "wasm.build.split-runtime-release",
    ):
        assert rows[name].argv[:5] == (
            "python3",
            "tools/venv_exec.py",
            "python3",
            "-m",
            "molt.cli",
        )
    # The proof chooses a wrapper once; prebuild and consumer rows must not
    # silently erase that choice. The shared policy still disables incremental.
    monkeypatch.setenv("RUSTC_WRAPPER", "/selected/sccache")
    monkeypatch.setenv("CARGO_INCREMENTAL", "1")
    monkeypatch.setenv("MOLT_BUILD_PYTHON", "/selected/build-python")
    for row in rows.values():
        environment, _policies = proof_plan._command_environment(
            PLAN, row, row.data["timeout_seconds"]
        )
        assert environment["RUSTC_WRAPPER"] == "/selected/sccache"
        assert environment["CARGO_INCREMENTAL"] == (
            "0" if "cargo" in row.toolchains else "1"
        )
        assert environment["MOLT_BUILD_PYTHON"] == "/selected/build-python"


def test_wasm_host_export_applications_have_independent_cold_partitions() -> None:
    expected: dict[str, tuple[str, set[str]]] = {
        "wasm.integration.host-exports.gpu-kernel": (
            "test_split_runtime_compiled_gpu_kernel_vector_add_matches_expected_output",
            set(),
        ),
        "wasm.integration.host-exports.attribute-error": (
            "test_linked_host_export_attribute_error_does_not_return_none",
            {"wasm.build.shared-runtime"},
        ),
        "wasm.integration.host-exports.tinygrad-dtype": (
            "test_linked_host_export_imports_tinygrad_dtype_class",
            set(),
        ),
        "wasm.integration.host-exports.tinygrad-tensor": (
            "test_linked_host_export_imports_tinygrad_tensor_module",
            set(),
        ),
        "wasm.integration.host-exports.tensor-row-ops": (
            "test_linked_host_export_tensor_row_ops_accept_equivalent_float_dtype",
            set(),
        ),
    }
    rows = {
        row.id: row
        for row in PLAN.commands
        if row.id.startswith("wasm.integration.host-exports")
    }
    assert set(rows) == set(expected)
    for name, (test_name, runtime_dependencies) in expected.items():
        row = rows[name]
        assert row.argv[6:] == (f"tests/test_wasm_split_runtime.py::{test_name}",)
        assert row.data["timeout_budget"] == "cold"
        assert {"node", "wasi-clang"}.issubset(PLAN.required_toolchains(row))
        # Each program retains its own import closure. The GPU/tinygrad cells
        # compile their exact feature generation; AttributeError consumes the
        # admitted micro pair. Node needs no native host or other runtime tier.
        assert set(row.dependencies) == {"wasm.build.backend", *runtime_dependencies}
        assert {
            selected.id
            for selected in proof_plan._topological_commands(PLAN, command_id=name)
        } == {name, "wasm.build.backend", *runtime_dependencies}

    # The shared compiler resource serializes cold application rows. Allocate
    # their declared work in the existing job, without extending a child bound.
    compiler_seconds = sum(
        int(row.data["timeout_seconds"])
        for row in PLAN.commands
        if row.family == "wasm"
        and row.data["resource_class"] == "compiler-build-resource"
    )
    envelope = PLAN.timeout_envelope("wasm")
    assert envelope.projected_makespan_seconds == compiler_seconds
    assert (
        envelope.resource_capacity_floor_seconds["compiler-build-resource"]
        == compiler_seconds
    )
    family = next(family for family in PLAN.families if family.name == "wasm")
    assert int(family.data["timeout_minutes"]) * 60 >= (
        compiler_seconds + int(family.data["job_reserve_seconds"])
    )


def test_wasm_lifecycle_consumers_are_enrolled_with_required_node() -> None:
    rows = {row.id: row for row in PLAN.commands}
    startup = rows["wasm.test.startup-lifecycle"]
    assert set(startup.argv[6:]) == {
        "tests/test_wasm_startup_failures.py",
        "tests/test_generate_worker.py",
        "tests/test_browser_asset_closure.py",
        "tests/test_wasm_reserved_callable_arity.py",
        "tests/test_wasm_split_runtime.py::test_split_failure_retains_distinct_bytes_and_publication",
    }
    assert {"pr", "main"} <= set(startup.data["tiers"])
    assert "node" in PLAN.required_toolchains(startup)
    assert not startup.dependencies
    runner = rows["wasm.host.runner-fixtures"]
    assert runner.dependencies == ("wasm.build.backend",)
    assert {arg for arg in runner.argv if arg.startswith("tests/")} == {
        "tests/test_wasm_runner_table_base.py"
    }
    assert not set(startup.argv[6:]) & {
        arg for arg in runner.argv if arg.startswith("tests/")
    }
    for path in startup.argv[6:]:
        assert "wasm" in {
            family.name for family in PLAN.select([path.split("::", 1)[0]]).selected
        }
    split = rows["wasm.integration.split-runtime"]
    assert set(split.argv[6:]) == {
        "tests/test_wasm_split_runtime.py::TestSplitRuntimeArtifacts",
        "tests/test_wasm_split_runtime.py::TestWorkerJsContent",
        "tests/test_wasm_split_runtime.py::TestManifestJson",
        "tests/test_wasm_split_runtime.py::TestRuntimeCacheability",
        "tests/test_browser_vfs.py",
    }
    assert "node" in PLAN.required_toolchains(split)
    assert split.evidence_outputs == ("proof-receipts/evidence/split-runtime",)
    assert any(
        "tests/test_wasm_split_runtime.py::test_split_failure_retains_distinct_bytes_and_publication"
        in command.argv
        for command in PLAN.commands
        if command.id == "wasm.test.startup-lifecycle"
    )
    assert {"pr", "main"} <= set(split.data["tiers"])
    assert "wasm.build.host" not in {
        row.id for row in proof_plan._topological_commands(PLAN, command_id=split.id)
    }


def test_import_from_codec_receivers_execute_both_targets() -> None:
    from tools.compat import test_policy

    row = next(
        row for row in PLAN.commands if row.id == "wasm.test.import-from-codec-parity"
    )
    receivers = (
        "tests/differential/basic/from_import_missing_name.py",
        "tests/differential/stdlib/cpython312plus_api_gap_submodule_encodings_oem_87baaa74.py",
        "tests/differential/stdlib/cpython312plus_api_gap_submodule_encodings_mbcs_35072d4b.py",
    )
    assert row.argv[:4] == (
        "python3",
        "tools/venv_exec.py",
        "python3",
        "tests/molt_diff.py",
    )
    assert row.argv[-3:] == receivers
    assert row.evidence_outputs == ("proof-receipts/evidence/import-from-codec",)
    for option, value in (
        ("--target", "native,wasm"),
        ("--jobs", "1"),
        ("--build-profile", "dev"),
        ("--stdlib-profile", "full"),
        ("--python-version", "3.12"),
        ("--molt-target-python", "3.12"),
    ):
        assert row.argv[row.argv.index(option) + 1] == value
    assert "--no-retry-oom" in row.argv
    assert "--warm-cache" not in row.argv
    assert set(row.dependencies) == {"wasm.build.backend", "wasm.build.shared-runtime"}
    assert {"node", "ld.lld", "wasm-ld", "wasm-tools", "wasi-clang"} <= set(
        PLAN.required_toolchains(row)
    )
    assert {"pr", "main"} <= set(row.data["tiers"])
    assert row.data["resource_class"] == "compiler-build-resource"
    assert row.data["timeout_budget"] == "cold"
    family = next(family for family in PLAN.families if family.name == "wasm")
    for receiver in receivers:
        assert receiver in family.inputs
        assert "wasm" in {family.name for family in PLAN.select([receiver]).selected}
        metadata = test_policy.parse_metadata(
            Path(__file__).resolve().parents[1] / receiver
        )
        # The runner's ordinary expectation policy cannot turn one of these
        # required semantic failures into xfail, skip, or approximate stdout.
        assert not metadata.expect_molt_fail
        assert metadata.stdout_mode == "exact"
        for backend in ("native", "wasm"):
            assert (
                test_policy.exclusion_reason(
                    metadata,
                    python_version=(3, 12),
                    platform_tags={"linux", "posix"},
                    architecture="x86_64",
                    backend=backend,
                )
                is None
            )


def test_wasm_e2e_commands_bind_complete_child_toolchain_closure() -> None:
    by_id = {command.id: command for command in PLAN.commands}
    freestanding = by_id["wasm.test.freestanding-e2e"]
    parity = by_id["wasm.test.finally-pending-observer-parity"]

    assert freestanding.argv[-2:] == (
        "tests/test_wasm_freestanding.py::test_freestanding_produces_no_wasi_imports",
        "tests/test_wasm_freestanding.py::test_freestanding_binary_is_valid_wasm",
    )
    assert {"python", "uv", "rustc", "cargo", "wasm-ld", "wasm-tools"}.issubset(
        PLAN.required_toolchains(freestanding)
    )
    assert parity.argv[-1].endswith("test_finally_pending_observer_native_wasm_parity")
    assert {"clang", "lld-link", "wasm-ld", "wasm-tools"}.issubset(
        PLAN.required_toolchains(parity)
    )
    harness = by_id["python.unit.harness"]
    assert "tests/test_finally_pending_observer_harness.py" in harness.argv
    assert "tests/test_finally_pending_observer_parity.py" not in harness.argv


def test_actual_wasm_linker_fixtures_have_one_provisioned_lane() -> None:
    commands = {command.id: command for command in PLAN.commands}
    actual = commands["wasm.test.linker-admission"]
    names = {
        "test_primary_runtime_fixture_passes_actual_relocatable_admission",
        "test_run_wasm_ld_rejects_shared_primary_before_publication",
        "test_wasm_module_identity_survives_distinct_staging_paths",
        "test_existing_alias_binding_controls_actual_llvm_resolution",
    }
    selectors = {f"tests/test_wasm_link_validation.py::{name}" for name in names}
    assert set(arg for arg in actual.argv if "::" in arg) == selectors
    assert actual.family == "wasm"
    assert actual.data["cell"] == "linux-x86_64-py312-wasm-dev"
    assert set(actual.data["tiers"]) == {"pr", "main"}
    assert {"python", "uv", "wasm-ld"} <= set(PLAN.required_toolchains(actual))
    assert "-m" not in actual.argv[actual.argv.index("pytest") + 1 :]
    assert {
        command.id for command in PLAN.commands if selectors & set(command.argv)
    } == {actual.id}

    root = Path(__file__).resolve().parents[1]
    module = ast.parse((root / "tests/test_wasm_link_validation.py").read_bytes())
    actual_functions = {
        node.name: node
        for node in module.body
        if isinstance(node, ast.FunctionDef) and node.name in names
    }
    assert set(actual_functions) == names
    for node in actual_functions.values():
        assert any(
            ast.unparse(decorator) == "pytest.mark.slow"
            for decorator in node.decorator_list
        )
    for suffix in ("", ".macos"):
        unit = commands[f"python.unit.runtime-artifacts{suffix}"]
        assert any(pair == ("-m", "not slow") for pair in zip(unit.argv, unit.argv[1:]))
        assert "tests/test_wasm_link_validation.py" in unit.argv
        assert not (selectors & set(unit.argv))


def test_git_toolchain_declares_lossless_process_image_probe() -> None:
    git = next(policy for policy in PLAN.toolchain_policies if policy.name == "git")

    assert git.data["process_image_probes"] == [["--version"]]


@pytest.mark.parametrize("probes", [[], [[]], [[""]], [["--version"], ["--version"]]])
def test_toolchain_process_image_probes_fail_closed(probes: list[list[str]]) -> None:
    policies = tuple(
        replace(policy, data={**policy.data, "process_image_probes": probes})
        if policy.name == "git"
        else policy
        for policy in PLAN.toolchain_policies
    )

    if probes == []:
        assert not any(
            "process_image_probes" in error
            for error in replace(PLAN, toolchain_policies=policies).validate()
        )
    else:
        assert any(
            "process_image_probes" in error
            for error in replace(PLAN, toolchain_policies=policies).validate()
        )


@pytest.mark.parametrize(
    ("dependencies", "expected"),
    [
        (["missing-toolchain"], "unknown toolchain dependency"),
        (["cargo"], "toolchain dependency cycle"),
    ],
)
def test_toolchain_dependency_graph_fails_closed(
    dependencies: list[str], expected: str
) -> None:
    policies = tuple(
        replace(policy, data={**policy.data, "dependencies": dependencies})
        if policy.name == "cargo"
        else policy
        for policy in PLAN.toolchain_policies
    )

    assert any(
        expected in error
        for error in replace(PLAN, toolchain_policies=policies).validate()
    )


def test_node_policy_pins_the_tool_release() -> None:
    policy = next(policy for policy in PLAN.toolchain_policies if policy.name == "node")
    version = tool_releases.tool_release("node").version
    assert policy.data["setup_value"] == version
    assert re.fullmatch(str(policy.data["version_pattern"]), f"v{version}")
    assert not re.fullmatch(str(policy.data["version_pattern"]), f"v{version}1")
    assert (
        f'config/tool_releases.toml::version = "{version}"'
        in policy.data["setup_evidence"]
    )


def test_wasm_tools_identity_accepts_only_pinned_release_build_metadata() -> None:
    policy = next(
        policy for policy in PLAN.toolchain_policies if policy.name == "wasm-tools"
    )
    pattern = str(policy.data["version_pattern"])

    version = tool_releases.tool_release("wasm-tools").version
    assert policy.data["setup_value"] == version
    assert re.fullmatch(pattern, f"wasm-tools {version}")
    assert re.fullmatch(pattern, f"wasm-tools {version} (7fc33f279 2026-09-10)")
    assert not re.fullmatch(pattern, f"wasm-tools {version}1")
    assert not re.fullmatch(pattern, f"wasm-tools {version} (local build)")


def test_source_extension_toolchain_requires_target_context_capture() -> None:
    policy = next(
        policy
        for policy in PLAN.toolchain_policies
        if policy.name == "source-extension"
    )

    assert policy.identity_kind == "target-derived"
    assert policy.data["identity_provider"] == "source-extension"
    assert "identity_kind" not in policy.data
    assert "setup_value" not in policy.data
    assert "executable" not in policy.data
    assert "version_args" not in policy.data
    with pytest.raises(ValueError, match="requires target-context capture"):
        proof_plan._version_fingerprint(policy)
    with pytest.raises(ValueError, match="requires target-context capture"):
        proof_plan.toolchain_fingerprints(PLAN, ("source-extension",))


def test_toolchain_documentation_projects_provider_and_executable_contracts() -> None:
    markdown = gen_proof_plan._markdown_projection(PLAN)
    for policy in PLAN.toolchain_policies:
        data = policy.data
        prefix = f"| `{policy.name}` | `{policy.identity_kind}` | "
        if policy.identity_kind == "target-derived":
            row = (
                prefix
                + f"`{data['identity_provider']}` | `{data['version_pattern']}` | "
                "— | — | — |"
            )
            assert "setup_value" not in data
        else:
            row = (
                prefix + f"— | `{data['version_pattern']}` | "
                f"`{data.get('probe_cwd', '.')}` | `{data['setup_value']}` | "
                f"{len(data['setup_evidence'])} |"
            )
        assert row in markdown.splitlines()
    projection = json.loads(gen_proof_plan._json_projection(PLAN))
    assert projection["toolchain_policies"] == [
        policy.data for policy in PLAN.toolchain_policies
    ]


@pytest.mark.parametrize(
    ("updates", "expected"),
    [
        ({"identity_kind": "target-derived"}, "must not declare identity_kind"),
        ({"identity_provider": "ambient"}, "identity_provider must be one of"),
        ({"executable": "cc"}, "must not declare executable"),
        ({"version_args": ["--version"]}, "must not declare version_args"),
        ({"setup_value": "target-derived"}, "must not declare setup_value"),
    ],
)
def test_target_derived_toolchain_rejects_executable_probe_fields(
    updates: dict[str, object], expected: str
) -> None:
    policies = tuple(
        replace(policy, data={**policy.data, **updates})
        if policy.name == "source-extension"
        else policy
        for policy in PLAN.toolchain_policies
    )

    assert any(
        error.startswith("source-extension:") and expected in error
        for error in replace(PLAN, toolchain_policies=policies).validate()
    )


def test_executable_toolchain_requires_non_empty_version_probe() -> None:
    policies = tuple(
        replace(
            policy,
            data={
                key: value
                for key, value in policy.data.items()
                if key != "version_args"
            },
        )
        if policy.name == "uv"
        else policy
        for policy in PLAN.toolchain_policies
    )

    assert (
        "uv: executable identity requires non-empty version_args"
        in replace(PLAN, toolchain_policies=policies).validate()
    )


def test_static_command_rejects_target_derived_toolchain() -> None:
    command = PLAN.commands[0]
    commands = tuple(
        replace(
            candidate,
            data={
                **candidate.data,
                "toolchains": [*candidate.toolchains, "source-extension"],
            },
        )
        if candidate.id == command.id
        else candidate
        for candidate in PLAN.commands
    )

    assert any(
        error.startswith(f"{command.id}: static command cannot use target-derived")
        for error in replace(PLAN, commands=commands).validate()
    )


@pytest.mark.parametrize(
    "wrapper_env",
    [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    ],
)
def test_sccache_environment_policy_covers_every_rust_proof_family(
    monkeypatch,
    tmp_path: Path,
    wrapper_env: str,
) -> None:
    monkeypatch.setenv(wrapper_env, "/opt/cache/sccache")
    monkeypatch.setenv("CARGO_INCREMENTAL", "1")
    # A run's scratch TMPDIR must not reach the shared sccache server (HF-105).
    monkeypatch.setenv("SCCACHE_DIR", str(tmp_path / "artifacts" / ".sccache"))
    for name in ("TMPDIR", "TMP", "TEMP"):
        monkeypatch.setenv(name, str(tmp_path / "run-scratch"))
    server_temp = str(tmp_path / "artifacts" / ".sccache-tmp")
    rust_commands = [
        command
        for command in PLAN.commands
        if {"rustc", "cargo"}.issubset(command.toolchains)
        and command.data.get("env", {}).get(wrapper_env) != ""
    ]

    assert {command.family for command in rust_commands} == {
        "llvm",
        "native_integration",
        "nightly_determinism",
        "nightly_shard_prepare",
        "nightly_verification_t3",
        "platform_portability",
        "python_unit",
        "repository_policy",
        "rust",
        "rust_security",
        "runtime_candidate_costs",
        "wasm",
    }
    for command in rust_commands:
        environment, applied = proof_plan._command_environment(PLAN, command, 30)
        assert environment["CARGO_INCREMENTAL"] == "0", command.id
        assert applied == (
            "sccache-disables-incremental",
            "sccache-server-temp-dir",
        ), command.id
        assert {environment[name] for name in ("TMPDIR", "TMP", "TEMP")} == {
            server_temp
        }, command.id


def test_compiler_build_commands_use_shared_timeout_budgets() -> None:
    compiler_commands = [
        command
        for command in PLAN.commands
        if command.data["resource_class"] == "compiler-build-resource"
    ]
    assert compiler_commands
    assert all(command.data.get("timeout_budget") for command in compiler_commands)
    assert {
        command.id: (command.data["timeout_budget"], command.data["timeout_seconds"])
        for command in compiler_commands
        if command.id
        in {
            "wasm.build.host",
            "native.integration.bench-cli",
            "native.integration.capability-manifest",
            "rust.clippy.wasi32",
            "rust.test.default-truth",
            "llvm.build.backend",
            "mlir.test.backend",
        }
    } == {
        "wasm.build.host": ("cold", 1200),
        "native.integration.bench-cli": ("cold", 1200),
        "native.integration.capability-manifest": ("cold", 1200),
        "rust.clippy.wasi32": ("cold", 1200),
        "rust.test.default-truth": ("suite", 1800),
        "llvm.build.backend": ("cold", 1200),
        "mlir.test.backend": ("warm", 300),
    }
    for command in compiler_commands:
        environment, _applied = proof_plan._command_environment(
            PLAN, command, int(command.data["timeout_seconds"])
        )
        assert environment[PROOF_COMMAND_TIMEOUT_ENV] == str(
            command.data["timeout_seconds"]
        )


def test_compiler_build_command_cannot_restore_an_explicit_timeout_lane() -> None:
    commands = tuple(
        replace(
            command,
            data={
                **command.data,
                "timeout_budget": None,
                "timeout_seconds": 300,
            },
        )
        if command.id == "wasm.build.host"
        else command
        for command in PLAN.commands
    )

    errors = replace(PLAN, commands=commands).validate()

    assert "wasm.build.host: compiler-build-resource requires timeout_budget" in errors


def test_command_cannot_override_owning_proof_timeout_environment() -> None:
    commands = tuple(
        replace(
            current,
            data={
                **current.data,
                "env": {PROOF_COMMAND_TIMEOUT_ENV: "1"},
            },
        )
        if current.id == "wasm.build.host"
        else current
        for current in PLAN.commands
    )

    errors = replace(PLAN, commands=commands).validate()

    assert f"wasm.build.host: env cannot override {PROOF_COMMAND_TIMEOUT_ENV}" in errors


def test_command_direct_rustc_override_does_not_apply_sccache_policy(
    monkeypatch,
) -> None:
    monkeypatch.setenv("RUSTC_WRAPPER", "/opt/cache/sccache")
    monkeypatch.setenv("CARGO_INCREMENTAL", "1")
    template = next(
        command for command in PLAN.commands if command.id == "wasm.build.host"
    )
    command = replace(
        template,
        data={**template.data, "env": {"RUSTC_WRAPPER": ""}},
    )

    environment, applied = proof_plan._command_environment(PLAN, command, 30)

    assert environment["RUSTC_WRAPPER"] == ""
    assert environment["CARGO_INCREMENTAL"] == "1"
    assert applied == ()


@pytest.mark.parametrize(
    ("probe_cwd", "expected"),
    [
        ("./formal/lean", "canonical repository-relative directory"),
        ("../outside", "canonical repository-relative directory"),
        ("formal/does-not-exist", "must resolve inside the repository"),
        ("formal/lean/lean-toolchain", "is not a directory"),
    ],
)
def test_toolchain_probe_cwd_is_existing_repo_contained_directory(
    probe_cwd: str, expected: str
) -> None:
    policies = tuple(
        replace(policy, data={**policy.data, "probe_cwd": probe_cwd})
        if policy.name == "lean"
        else policy
        for policy in PLAN.toolchain_policies
    )
    errors = replace(PLAN, toolchain_policies=policies).validate()
    assert any(
        error.startswith("lean: probe_cwd") and expected in error for error in errors
    )


def test_lean_toolchain_probe_cwd_is_project_authority() -> None:
    lean = next(policy for policy in PLAN.toolchain_policies if policy.name == "lean")
    assert lean.data["probe_cwd"] == "formal/lean"
    assert lean.data["setup_evidence"] == [
        f"formal/lean/lean-toolchain::leanprover/lean4:v{lean.data['setup_value']}",
        '.github/actions/setup-lean/action.yml::toolchain install "$toolchain"',
        ".github/workflows/formal.yml::uses: ./.github/actions/setup-lean",
    ]


@pytest.mark.parametrize("name", ["wasm-ld", "llvm-nm", "ld.lld"])
def test_toolchain_fingerprint_selects_sdk_only_for_declared_wasm_role(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    import molt.llvm_toolchain as llvm_toolchain

    selected = tmp_path / name
    selected.write_bytes(b"selected role bytes")
    policy = next(policy for policy in PLAN.toolchain_policies if policy.name == name)
    monkeypatch.setenv("MOLT_WASM_LD", str(selected))
    monkeypatch.setenv("MOLT_LLVM_NM", "not-the-native-reader")
    selections = []

    def sdk_role(root, role, *, environ):
        assert name == "wasm-ld"
        assert root == proof_plan.ROOT and role == "wasm-ld"
        assert environ["MOLT_WASM_LD"] == str(selected)
        selections.append("sdk")
        return selected

    def native_role(requested):
        assert name != "wasm-ld", "SDK role must not resolve through native PATH"
        assert requested == name
        selections.append("native")
        return str(selected)

    monkeypatch.setattr(llvm_toolchain, "resolve_wasi_sdk_tool", sdk_role)
    install_module_view(monkeypatch, "shutil", shutil, proof_plan, which=native_role)
    commands = []

    def run(command, **_kwargs):
        commands.append(command)
        version = "22.1.0" if name == "wasm-ld" else "22.1.8"
        banner = f"LLVM version {version}" if name == "llvm-nm" else f"LLD {version}"
        return proof_plan.subprocess.CompletedProcess(command, 0, banner, "")

    install_module_view(monkeypatch, "subprocess", subprocess, proof_plan, run=run)
    actual = proof_plan._version_fingerprint(policy)
    assert actual is not None
    assert actual["path"] == str(selected)
    assert (
        actual["executable_sha256"]
        == hashlib.sha256(b"selected role bytes").hexdigest()
    )
    assert commands == [[str(selected), "--version"]]
    assert selections == (["sdk"] if name == "wasm-ld" else ["native"])


def test_missing_sdk_role_is_a_toolchain_preflight_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import molt.llvm_toolchain as llvm_toolchain

    def missing(*_args, **_kwargs):
        raise llvm_toolchain.LlvmToolchainConfigError("provision SDK explicitly")

    monkeypatch.setattr(llvm_toolchain, "resolve_wasi_sdk_tool", missing)
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        proof_plan,
        which=lambda *_a, **_k: pytest.fail("no PATH fallback"),
    )
    with pytest.raises(
        ValueError, match="wasm-ld toolchain selection failed.*provision SDK explicitly"
    ):
        proof_plan.toolchain_fingerprints(PLAN, ("wasm-ld",))


def test_toolchain_content_and_version_probes_share_declared_cwd(monkeypatch) -> None:
    policy = proof_plan.ToolchainPolicy(
        "probe",
        {
            "executable": "probe",
            "probe_cwd": "formal/lean",
            "version_args": ["--version"],
            "version_pattern": r"version 4\.28\.0",
            "content_path_command": ["probe-content"],
        },
    )
    calls: list[tuple[tuple[str, ...], Path]] = []

    def fake_run(argv, *, cwd, **_kwargs):
        command = tuple(argv)
        calls.append((command, cwd))
        output = (
            "lean-toolchain\n"
            if command == ("probe-content",)
            else f"Lean (version {_LEAN_PIN})\n"
        )
        return proof_plan.subprocess.CompletedProcess(argv, 0, output)

    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        proof_plan,
        which=lambda _requested: sys.executable,
    )
    install_module_view(monkeypatch, "subprocess", subprocess, proof_plan, run=fake_run)

    fingerprint = proof_plan._version_fingerprint(policy)
    expected_cwd = (proof_plan.ROOT / "formal/lean").resolve()
    assert fingerprint is not None
    assert calls == [
        (("probe-content",), expected_cwd),
        ((sys.executable, "--version"), expected_cwd),
    ]
    assert fingerprint["probe_cwd"] == "formal/lean"
    assert fingerprint["content_path"] == str(
        (expected_cwd / "lean-toolchain").resolve()
    )


def test_toolchain_content_probe_ignores_provisioner_stderr(
    tmp_path: Path, monkeypatch
) -> None:
    content = tmp_path / "canonical-tool"
    content.write_bytes(b"canonical payload")
    policy = proof_plan.ToolchainPolicy(
        "probe",
        {
            "executable": "probe",
            "version_args": ["--version"],
            "version_pattern": r"probe 1\.0",
            "content_path_command": ["probe-content"],
        },
    )

    def fake_run(argv, **_kwargs):
        if tuple(argv) == ("probe-content",):
            return proof_plan.subprocess.CompletedProcess(
                argv,
                0,
                f"{content}\n",
                "info: syncing freshly provisioned toolchain\n",
            )
        return proof_plan.subprocess.CompletedProcess(argv, 0, "probe 1.0\n", "")

    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        proof_plan,
        which=lambda _requested: sys.executable,
    )
    install_module_view(monkeypatch, "subprocess", subprocess, proof_plan, run=fake_run)

    fingerprint = proof_plan._version_fingerprint(policy)
    assert fingerprint is not None
    assert fingerprint["content_path"] == str(content.resolve())
    assert (
        fingerprint["executable_sha256"]
        == hashlib.sha256(b"canonical payload").hexdigest()
    )


def test_toolchain_path_probe_has_one_shared_exact_decoder(tmp_path: Path) -> None:
    executable = tmp_path / "bin" / "tool"
    executable.parent.mkdir()
    executable.write_bytes(b"tool")

    assert (
        toolchain_probe.resolve_single_file_path("bin/tool\n", probe_cwd=tmp_path)
        == executable.resolve()
    )
    with pytest.raises(ValueError, match="exactly one"):
        toolchain_probe.resolve_single_file_path(
            "bin/tool\nother-tool\n", probe_cwd=tmp_path
        )
    with pytest.raises(ValueError, match="did not name a file"):
        toolchain_probe.resolve_single_file_path("bin\n", probe_cwd=tmp_path)


def test_manifest_rejects_missing_repository_command_inputs() -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.unit.harness"
    )
    broken = replace(
        command,
        data={**command.data, "argv": [*command.argv, "tests/does_not_exist.py"]},
    )
    commands = tuple(
        broken if candidate.id == command.id else candidate
        for candidate in PLAN.commands
    )
    errors = replace(PLAN, commands=commands).validate()
    assert any(
        "repository input does not exist: 'tests/does_not_exist.py'" in error
        for error in errors
    )


def test_lockfiles_select_security_and_build_classes() -> None:
    cargo = _classes("Cargo.lock")
    uv = _classes("uv.lock")
    assert cargo["rust"] and cargo["llvm"] and cargo["rust_security"]
    assert not cargo["python_static"]
    assert uv["python_static"] and uv["python_unit"] and uv["python_security"]
    assert not uv["rust"] and not uv["rust_security"]


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


def test_merge_group_selects_full_plan_without_unknown_event_fallback() -> None:
    selection = proof_plan.selection_for_event(
        PLAN,
        event_name="merge_group",
        base_ref="",
        event_path="",
        before="",
        after="2" * 40,
    )

    assert selection.selected == PLAN.families
    assert selection.fail_closed_reason is None
    assert all(
        reasons == ("merge_group: full proof plan",)
        for reasons in selection.reasons.values()
    )


def test_generated_matrix_records_selection_reason() -> None:
    selection = PLAN.select(["Cargo.lock"])
    outputs = proof_plan.family_outputs(PLAN, selection)
    topology = json.loads(outputs["topology"])["include"]
    by_name = {entry["name"]: entry for entry in topology}
    assert by_name["rust"]["selected_by"] == ["Cargo.lock"]
    assert by_name["rust"]["resource_class"] == "compiler-build-resource"
    assert by_name["rust"]["dependencies"] == []
    for family in selection.selected:
        if family.name not in by_name:
            continue
        record = by_name[family.name]
        if family.data["executor"] in {"github-job", "github-matrix"}:
            assert record["job_reserve_seconds"] == family.data["job_reserve_seconds"]
        else:
            assert "job_reserve_seconds" not in record
    assert by_name["rust"]["admission_job"] == "rust-build-unit-smoke"
    assert by_name["rust"]["admission_needs"] == ["classify-changes"]
    assert "rust.test.default-truth" in by_name["rust"]["command_ids"]
    assert "linux-x86_64-rust-wasi-dev" in by_name["rust"]["matrix_cells"]
    # A Rust input also selects the hosted portability matrix, whose macOS
    # Rust cell carries exactly the workspace lint and the runtime gate.
    matrix = json.loads(outputs["platform_portability_matrix"])["include"]
    assert {entry["family"] for entry in matrix} == {"platform_portability"}
    assert all(entry["selected_by"] == ["Cargo.lock"] for entry in matrix)
    rust_cells = [entry for entry in matrix if entry["backend"] == "rust"]
    assert [
        (entry["cell"], entry["runner"], entry["target"]) for entry in rust_cells
    ] == [
        (
            "linux-aarch64-py312-rust-native-dev",
            "ubuntu-24.04-arm",
            "aarch64-unknown-linux-gnu",
        ),
        ("macos-arm64-py312-rust-native-dev", "macos-14", "aarch64-apple-darwin"),
    ]
    assert [entry["command_ids"] for entry in rust_cells] == [
        ["portability.rust.linux-aarch64.clippy-workspace"],
        [
            "portability.rust.macos.clippy-workspace",
            "portability.rust.macos.runtime-gate",
        ],
    ]
    # Cargo.lock is not a Python unit input, so that matrix starts no runner.
    assert json.loads(outputs["python_unit_matrix"]) == {"include": []}


def test_generated_platform_matrix_is_runner_executable_and_cell_exact() -> None:
    selection = PLAN.select(["tools/proof_queue.py"])
    outputs = proof_plan.family_outputs(PLAN, selection)
    matrix = json.loads(outputs["platform_portability_matrix"])["include"]

    assert [(entry["os"], entry["runner"]) for entry in matrix] == [
        ("linux", "ubuntu-latest"),
        ("linux", "ubuntu-24.04-arm"),
        ("macos", "macos-14"),
        ("macos", "macos-14"),
        ("windows", "windows-2022"),
    ]
    assert all(entry["family"] == "platform_portability" for entry in matrix)
    assert {entry["cell"]: entry["command_ids"] for entry in matrix} == {
        "linux-x86_64-py312-queue-portability": [
            "portability.completion.linux",
            "portability.queue.linux",
            "portability.cargo-link.linux",
            "portability.cargo-custody.linux",
        ],
        "macos-arm64-py312-queue-portability": [
            "portability.queue.macos",
            "portability.cargo-link.macos",
            "portability.ir.macos",
            "portability.cargo-custody.macos",
        ],
        "linux-aarch64-py312-rust-native-dev": [
            "portability.rust.linux-aarch64.clippy-workspace",
        ],
        "macos-arm64-py312-rust-native-dev": [
            "portability.rust.macos.clippy-workspace",
            "portability.rust.macos.runtime-gate",
        ],
        "windows-x86_64-py312-queue-portability": [
            "portability.queue.windows",
            "portability.cargo-link.windows",
            "portability.ir.windows",
            "portability.cargo-custody.windows",
            "portability.headers.windows",
        ],
    }
    for entry in matrix:
        commands = proof_plan._topological_commands(
            PLAN,
            family="platform_portability",
            matrix_cell=entry["cell"],
        )
        assert [command.id for command in commands] == entry["command_ids"]
        assert all(command.data["cell"] == entry["cell"] for command in commands)


@pytest.mark.parametrize(
    "path",
    [
        "include/molt/Python.h",
        "include/molt/shared/_data_api.h",
        "runtime/molt-cpython-abi/include/Python.h",
        "tests/cli/test_c_api_headers.py",
    ],
)
def test_header_changes_execute_real_windows_data_imports(path: str) -> None:
    assert _classes(path)["platform_portability"]
    command = next(
        command
        for command in PLAN.commands
        if command.id == "portability.headers.windows"
    )
    assert command.data["cell"] == "windows-x86_64-py312-queue-portability"
    assert set(command.data["tiers"]) == {"pr", "main"}
    assert (
        "tests/cli/test_c_api_headers.py::test_optimize_flag_headers_share_data_across_translation_units"
        in command.data["argv"]
    )
    assert "hosted-clang-cl" in PLAN.required_toolchains(command)


@pytest.mark.parametrize(
    "path",
    ["tools/windows_process_api.py", "tests/tools/test_windows_process_api.py"],
)
def test_windows_process_binding_selects_and_executes_portability_proof(
    path: str,
) -> None:
    assert path in PLAN.authority_inputs
    assert _classes(path)["platform_portability"]
    commands = [
        command
        for command in PLAN.commands
        if command.id.startswith("portability.queue.")
    ]
    assert len(commands) == 3
    assert all(
        "tests/tools/test_windows_process_api.py" in command.data["argv"]
        for command in commands
    )


def test_portability_streams_node_outcomes_before_possible_timeout() -> None:
    commands = [
        command
        for command in PLAN.commands
        if command.id.startswith("portability.queue.")
    ]
    assert len(commands) == 3
    for command in commands:
        argv = command.data["argv"]
        python_index = argv.index("python")
        assert argv[python_index : python_index + 5] == [
            "python",
            "-u",
            "-m",
            "pytest",
            "-v",
        ]
        assert "-q" not in argv


def _receipt_for(
    command: proof_plan.ProofCommand, evidence_root: Path | None = None
) -> dict[str, Any]:
    policies = {policy.name: policy for policy in PLAN.toolchain_policies}
    # Each tool's --version spelling around the plan's pinned setup value, so
    # the fixture tracks every pin bump without a second copy of the versions.
    spellings = {
        "python": "Python {}",
        "uv": "uv {}",
        "node": "v{}",
        "rustc": "rustc {}",
        "cargo": "cargo {}",
        "lune": "lune {}",
        "clang": "clang version {}",
        "ld.lld": "LLD {} (compatible with GNU linkers)",
        "llvm-config": "{}",
        "mlir-opt": "LLVM version {}",
        "lean": "Lean (version {})",
        "quint": "{}",
        "cargo-deny": "cargo-deny {}",
        "cargo-audit": "cargo-audit {}",
    }
    versions = {
        name: spelling.format(policies[name].data["setup_value"])
        for name, spelling in spellings.items()
    }
    versions["git"] = "git version 2.53.0"
    toolchains: dict[str, dict[str, str]] = {}
    for name in PLAN.required_toolchains(command):
        path = f"/toolchain/{name}"
        launcher_path = f"{path}/launcher"
        content_path = f"{path}/content"
        version = versions[name]
        probe_cwd = str(policies[name].data.get("probe_cwd", "."))
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
            "probe_cwd": probe_cwd,
            "executable_sha256": executable_sha256,
            "identity_sha256": hashlib.sha256(
                (
                    f"{path}\0{launcher_path}\0{launcher_sha256}\0{content_path}\0"
                    f"{executable_sha256}\0{version}\0{probe_cwd}"
                ).encode()
            ).hexdigest(),
        }
    evidence_bytes = b'{"schema":"test.evidence"}'
    evidence_digest = hashlib.sha256(evidence_bytes).hexdigest()
    evidence_outputs = [
        {
            "path": relative,
            "kind": "file",
            "sha256": evidence_digest,
            "total_size_bytes": len(evidence_bytes),
            "files": [
                {
                    "path": Path(relative).name,
                    "sha256": evidence_digest,
                    "size": len(evidence_bytes),
                }
            ],
        }
        for relative in command.evidence_outputs
    ]
    if evidence_root is not None:
        for relative in command.evidence_outputs:
            output = evidence_root / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(evidence_bytes)
    return {
        "schema": PLAN.receipt_schema,
        "authority_sha256": proof_plan._authority_sha256(PLAN),
        "source_commit": proof_plan._source_commit(),
        "source_tree": proof_plan._source_identity()["tree"],
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
                "declared_evidence_outputs": list(command.evidence_outputs),
                "evidence_outputs": evidence_outputs,
                "duration_seconds": 0.1,
                "peak_rss_bytes": 1024,
                "cache_disposition": "cold",
                "resource_class": command.data["resource_class"],
                "timeout_budget": command.data.get("timeout_budget"),
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
    selected = [
        command.id for command in PLAN.commands if command.family == "python_static"
    ]
    assert len(selected) > 1
    assert errors == [
        f"{command_id}: required executable receipt is missing"
        for command_id in selected
    ]


def test_cache_disposition_never_infers_restore_hit_from_directory_existence(
    tmp_path: Path, monkeypatch
) -> None:
    command = next(
        command
        for command in PLAN.commands
        if str(command.data["cache_domain"]).startswith("cargo")
    )
    target = tmp_path / "target"
    target.mkdir()
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    assert proof_plan._cache_disposition(command) == "unknown"


def test_receipt_verdict_accepts_every_exact_selected_partition(tmp_path: Path) -> None:
    commands = [
        command for command in PLAN.commands if command.family == "python_static"
    ]
    for command in commands:
        (tmp_path / f"{command.id}.json").write_text(
            json.dumps(_receipt_for(command, tmp_path)), encoding="utf-8"
        )
    assert proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path) == []


def test_scheduled_verdict_requires_every_command_and_evidence_output(
    tmp_path: Path,
) -> None:
    scheduled_names = [family.name for family in PLAN.scheduled_families]
    scheduled_commands = [
        command for command in PLAN.commands if command.family in scheduled_names
    ]
    for command in scheduled_commands:
        (tmp_path / f"{command.id}.json").write_text(
            json.dumps(_receipt_for(command, tmp_path)), encoding="utf-8"
        )

    assert proof_plan.verify_receipts(PLAN, scheduled_names, tmp_path) == []

    terminal = next(
        command for command in scheduled_commands if command.evidence_outputs
    )
    receipt_path = tmp_path / f"{terminal.id}.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    receipt["commands"][0]["evidence_outputs"] = []
    receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, scheduled_names, tmp_path)
    assert any("evidence outputs do not match authority" in error for error in errors)


def test_receipt_verdict_rejects_authority_and_command_drift(tmp_path: Path) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command, tmp_path)
    receipt["authority_sha256"] = "0" * 64
    receipt["commands"][0]["argv"] = ["true"]  # type: ignore[index]
    (tmp_path / "drift.json").write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert any("authority digest" in error for error in errors)
    assert any("required executable receipt is missing" in error for error in errors)


def test_receipt_verdict_rehashes_downloaded_evidence_bytes(tmp_path: Path) -> None:
    command = next(
        command
        for command in PLAN.commands
        if command.family == "nightly_conformance" and command.evidence_outputs
    )
    receipt = _receipt_for(command, tmp_path)
    (tmp_path / "receipt.json").write_text(json.dumps(receipt), encoding="utf-8")
    assert proof_plan.verify_receipts(PLAN, ["nightly_conformance"], tmp_path) == []

    (tmp_path / command.evidence_outputs[0]).write_text(
        '{"schema":"tampered"}', encoding="utf-8"
    )
    errors = proof_plan.verify_receipts(PLAN, ["nightly_conformance"], tmp_path)
    assert any(
        "downloaded evidence bytes do not match receipt" in error for error in errors
    )


@pytest.fixture
def authority_input_bytes() -> dict[str, bytes]:
    """Read each input once per test; every variant still hashes the full closure."""
    return {
        relative: (proof_plan.ROOT / relative).read_bytes()
        for relative in PLAN.authority_inputs
    }


def test_every_authority_input_mutation_invalidates_receipt(
    tmp_path: Path, monkeypatch, authority_input_bytes: dict[str, bytes]
) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command, tmp_path)
    (tmp_path / "receipt.json").write_text(json.dumps(receipt), encoding="utf-8")
    original = proof_plan._authority_sha256(PLAN)
    assert proof_plan._authority_sha256(PLAN, authority_input_bytes) == original
    for relative, raw in authority_input_bytes.items():
        mutated = proof_plan._authority_sha256(
            PLAN, {**authority_input_bytes, relative: raw + b"\0"}
        )
        assert mutated != original, relative
        with monkeypatch.context() as context:
            context.setattr(proof_plan, "_authority_sha256", lambda _plan: mutated)
            errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
        assert any("authority digest" in error for error in errors), relative


def test_authority_digest_is_lf_crlf_checkout_invariant(
    authority_input_bytes: dict[str, bytes],
) -> None:
    original = proof_plan._authority_sha256(PLAN)
    assert proof_plan._authority_sha256(PLAN, authority_input_bytes) == original
    for relative, raw in authority_input_bytes.items():
        lf = raw.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        crlf = lf.replace(b"\n", b"\r\n")
        assert (
            proof_plan._authority_sha256(
                PLAN, {**authority_input_bytes, relative: crlf}
            )
            == original
        ), relative


def test_receipt_verdict_rejects_source_commit_replay(tmp_path: Path) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command, tmp_path)
    receipt["source_commit"] = "0" * 40
    (tmp_path / "replayed.json").write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert any("source commit" in error for error in errors)


@pytest.mark.parametrize("tree", [None, "0" * 40])
def test_receipt_verdict_rejects_missing_or_mismatched_source_tree(
    tmp_path: Path, tree: str | None
) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command, tmp_path)
    if tree is None:
        del receipt["source_tree"]
    else:
        receipt["source_tree"] = tree
    (tmp_path / "wrong-tree.json").write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert any("source tree identity" in error for error in errors)


def test_receipt_verdict_rejects_dirty_source_tree_attestation(tmp_path: Path) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command, tmp_path)
    receipt["source_tree_state"] = "dirty"
    (tmp_path / "dirty.json").write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert any("source tree is not clean" in error for error in errors)


def test_receipt_verdict_enforces_toolchain_version_contract(tmp_path: Path) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt = _receipt_for(command, tmp_path)
    uv = receipt["toolchains"]["uv"]  # type: ignore[index]
    uv["version"] = "uv 999.0.0"  # type: ignore[index]
    uv["identity_sha256"] = hashlib.sha256(  # type: ignore[index]
        (
            f"{uv['path']}\0{uv['launcher_path']}\0{uv['launcher_sha256']}\0"  # type: ignore[index]
            f"{uv['content_path']}\0{uv['executable_sha256']}\0{uv['version']}\0"  # type: ignore[index]
            f"{uv['probe_cwd']}"  # type: ignore[index]
        ).encode()
    ).hexdigest()
    (tmp_path / "wrong-version.json").write_text(json.dumps(receipt), encoding="utf-8")
    errors = proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    assert any("uv version violates" in error for error in errors)


def test_receipt_verdict_enforces_toolchain_probe_cwd_contract(tmp_path: Path) -> None:
    command = next(
        command for command in PLAN.commands if command.id == "formal.lean.build"
    )
    receipt = _receipt_for(command, tmp_path)
    lean = receipt["toolchains"]["lean"]  # type: ignore[index]
    lean["probe_cwd"] = "."  # type: ignore[index]
    lean["identity_sha256"] = hashlib.sha256(  # type: ignore[index]
        (
            f"{lean['path']}\0{lean['launcher_path']}\0{lean['launcher_sha256']}\0"  # type: ignore[index]
            f"{lean['content_path']}\0{lean['executable_sha256']}\0"  # type: ignore[index]
            f"{lean['version']}\0{lean['probe_cwd']}"  # type: ignore[index]
        ).encode()
    ).hexdigest()
    (tmp_path / "wrong-probe-cwd.json").write_text(
        json.dumps(receipt), encoding="utf-8"
    )
    errors = proof_plan.verify_receipts(PLAN, ["formal"], tmp_path)
    assert any("invalid lean toolchain identity" in error for error in errors)


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
            "probe_cwd": str(policy.data.get("probe_cwd", ".")),
            "executable_sha256": "0" * 64,
            "identity_sha256": "0" * 64,
        },
    )
    with pytest.raises(ValueError, match="toolchain contract violation"):
        proof_plan.toolchain_fingerprints(PLAN, ("python", "uv"))


def test_toolchain_fingerprint_domains_serialize_shared_provisioners(
    monkeypatch,
) -> None:
    active_rustup = 0
    overlap_detected = False
    lock = threading.Lock()
    calls: list[str] = []

    def fake_fingerprint(policy: proof_plan.ToolchainPolicy) -> dict[str, str]:
        nonlocal active_rustup, overlap_detected
        if policy.data.get("fingerprint_domain") == "rustup":
            with lock:
                overlap_detected |= active_rustup != 0
                active_rustup += 1
            time.sleep(0.02)
            with lock:
                active_rustup -= 1
        calls.append(policy.name)
        return {
            "path": f"/toolchain/{policy.name}",
            "launcher_path": f"/toolchain/{policy.name}",
            "launcher_sha256": "0" * 64,
            "content_path": f"/toolchain/{policy.name}",
            "version": f"{policy.name} {policy.data['setup_value']}",
            "version_pattern": str(policy.data["version_pattern"]),
            "probe_cwd": str(policy.data.get("probe_cwd", ".")),
            "executable_sha256": "0" * 64,
            "identity_sha256": "0" * 64,
        }

    monkeypatch.setattr(proof_plan, "_version_fingerprint", fake_fingerprint)
    fingerprints = proof_plan.toolchain_fingerprints(PLAN, ("rustc", "cargo"))
    assert set(fingerprints) == {"rustc", "cargo"}
    assert calls == ["rustc", "cargo"]
    assert overlap_detected is False


def test_executor_emits_measured_receipt(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
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
    assert receipt["execution"]["schema"] == "molt.proof-plan-dag-executor.v2"
    assert receipt["execution"]["peak_active_commands"] == 1
    assert receipt["execution"]["global_stop_triggered"] is False


def test_provisioned_lean_fingerprint_admits_formal_build_receipt(
    tmp_path: Path, monkeypatch
) -> None:
    """A provisioned exact Lean identity must cross the receipt scheduling gate."""
    command = next(
        command for command in PLAN.commands if command.id == "formal.lean.build"
    )
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    observed_toolchains: list[tuple[str, ...]] = []

    def fake_fingerprints(
        _plan: proof_plan.ProofPlan, names: tuple[str, ...]
    ) -> dict[str, dict[str, str]]:
        observed_toolchains.append(names)
        return {
            name: {
                "identity_sha256": "0" * 64,
                "version": f"Lean (version {_LEAN_PIN})"
                if name == "lean"
                else f"{name} synthetic",
            }
            for name in names
        }

    monkeypatch.setattr(proof_plan, "toolchain_fingerprints", fake_fingerprints)
    monkeypatch.setattr(
        proof_plan,
        "_run_command",
        lambda _plan, current, _cancel: _successful_synthetic_record(current),
    )
    receipt_path = tmp_path / "formal-lean-receipt.json"

    assert proof_plan.execute_commands(PLAN, (command,), receipt_path) == 0
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert observed_toolchains == [tuple(command.data["toolchains"])]
    assert receipt["status"] == "success"
    assert receipt["executed_partitions"] == ["formal.lean.build"]
    assert receipt["execution"]["scheduled_commands"] == 1
    assert receipt["commands"][0]["status"] == "success"


def _synthetic_executor_command(
    command_id: str,
    *,
    dependencies: list[str] | None = None,
    resource_class: str = "resource-a",
) -> proof_plan.ProofCommand:
    return proof_plan.ProofCommand(
        command_id,
        {
            "id": command_id,
            "family": "synthetic",
            "cell": "synthetic-cell",
            "tiers": ["test"],
            "resource_class": resource_class,
            "timeout_seconds": 10,
            "cache_domain": "none",
            "dependencies": dependencies or [],
            "argv": [sys.executable, "-c", "pass"],
            "toolchains": ["python"],
        },
    )


def _synthetic_executor_plan(
    commands: tuple[proof_plan.ProofCommand, ...],
    *,
    limits: dict[str, int],
    max_workers: int = 2,
) -> proof_plan.ProofPlan:
    return replace(
        PLAN,
        commands=commands,
        executor_max_workers=max_workers,
        resource_policies=tuple(
            proof_plan.ResourcePolicy(name, limit) for name, limit in limits.items()
        ),
    )


def test_timeout_envelope_models_dependencies_and_resource_capacity() -> None:
    commands = (
        _synthetic_executor_command("synthetic.a"),
        _synthetic_executor_command("synthetic.b", resource_class="resource-b"),
        _synthetic_executor_command("synthetic.a-sibling"),
        _synthetic_executor_command("synthetic.after-a", dependencies=["synthetic.a"]),
    )
    plan = _synthetic_executor_plan(
        commands,
        limits={"resource-a": 1, "resource-b": 1},
    )
    envelope = plan.timeout_envelope("synthetic")
    assert envelope.projected_makespan_seconds == 30
    assert envelope.critical_path_seconds == 20
    assert envelope.resource_capacity_floor_seconds == {
        "resource-a": 30,
        "resource-b": 10,
    }
    projected = gen_proof_plan._envelope_record(
        plan, "synthetic", 60, job_reserve_seconds=10
    )
    assert projected == {
        "budget_seconds": 60,
        "projected_makespan_seconds": 30,
        "critical_path_seconds": 20,
        "resource_capacity_floor_seconds": {"resource-a": 30, "resource-b": 10},
        "job_reserve_seconds": 10,
        "required_job_seconds": 40,
        "headroom_seconds": 20,
    }


def _successful_synthetic_record(
    command: proof_plan.ProofCommand,
) -> dict[str, object]:
    return {
        **proof_plan._base_command_record(command),
        "started_at": "2026-07-18T00:00:00+00:00",
        "duration_seconds": 0.01,
        "peak_rss_bytes": 1,
        "cache_disposition": "not-applicable",
        "status": "success",
        "returncode": 0,
        "guard_metrics_schema": "molt.guarded-command-metrics.v1",
        "evidence_outputs": [],
    }


def test_executor_hashes_declared_evidence_and_rejects_zero_work(
    tmp_path: Path, monkeypatch
) -> None:
    relative = f"proof-results/tests/{tmp_path.name}/result.json"
    output = proof_plan.ROOT / relative
    producer = _synthetic_executor_command("synthetic.evidence")
    producer = replace(
        producer,
        data={
            **producer.data,
            "evidence_outputs": [relative],
            "argv": [
                sys.executable,
                "-c",
                "from pathlib import Path; "
                f"p=Path({str(output)!r}); p.parent.mkdir(parents=True, exist_ok=True); "
                "p.write_text('{\"ok\":true}', encoding='utf-8')",
            ],
        },
    )
    plan = _synthetic_executor_plan((producer,), limits={"resource-a": 1})
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )
    try:
        receipt_path = tmp_path / "receipt.json"
        assert proof_plan.execute_commands(plan, (producer,), receipt_path) == 0
        record = json.loads(receipt_path.read_text(encoding="utf-8"))["commands"][0]
        assert record["declared_evidence_outputs"] == [relative]
        assert record["evidence_outputs"][0]["path"] == relative
        assert record["evidence_outputs"][0]["total_size_bytes"] > 0

        missing = replace(
            producer,
            id="synthetic.missing-evidence",
            data={
                **producer.data,
                "id": "synthetic.missing-evidence",
                "argv": [sys.executable, "-c", "pass"],
            },
        )
        missing_plan = _synthetic_executor_plan((missing,), limits={"resource-a": 1})
        missing_receipt = tmp_path / "missing.json"
        assert (
            proof_plan.execute_commands(missing_plan, (missing,), missing_receipt) == 2
        )
        missing_record = json.loads(missing_receipt.read_text(encoding="utf-8"))[
            "commands"
        ][0]
        assert missing_record["status"] == "failure"
        assert "evidence output is missing" in missing_record["evidence_error"]
    finally:
        proof_plan._clear_evidence_outputs(producer)


def test_executor_schedules_dependencies_and_resources_with_deterministic_receipts(
    tmp_path: Path, monkeypatch
) -> None:
    commands = (
        _synthetic_executor_command("synthetic.a"),
        _synthetic_executor_command("synthetic.b", resource_class="resource-b"),
        _synthetic_executor_command("synthetic.a-sibling"),
        _synthetic_executor_command("synthetic.after-a", dependencies=["synthetic.a"]),
    )
    plan = _synthetic_executor_plan(commands, limits={"resource-a": 1, "resource-b": 1})
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )
    active_by_resource = {"resource-a": 0, "resource-b": 0}
    peak_by_resource = {"resource-a": 0, "resource-b": 0}
    start_order: list[str] = []
    events: list[str] = []
    lock = threading.Lock()

    def fake_run(
        _plan: proof_plan.ProofPlan,
        command: proof_plan.ProofCommand,
        _cancel: threading.Event,
    ) -> dict[str, object]:
        resource = str(command.data["resource_class"])
        with lock:
            start_order.append(command.id)
            events.append(f"start:{command.id}")
            active_by_resource[resource] += 1
            peak_by_resource[resource] = max(
                peak_by_resource[resource], active_by_resource[resource]
            )
        time.sleep(0.03 if command.id == "synthetic.a" else 0.01)
        with lock:
            active_by_resource[resource] -= 1
            events.append(f"finish:{command.id}")
        return _successful_synthetic_record(command)

    monkeypatch.setattr(proof_plan, "_run_command", fake_run)
    receipt_path = tmp_path / "receipt.json"
    assert proof_plan.execute_commands(plan, commands, receipt_path) == 0
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert [record["id"] for record in receipt["commands"]] == [
        command.id for command in commands
    ]
    assert receipt["executed_partitions"] == [command.id for command in commands]
    assert receipt["execution"]["peak_active_commands"] == 2
    assert receipt["execution"]["peak_active_by_resource"] == {
        "resource-a": 1,
        "resource-b": 1,
    }
    assert peak_by_resource == {"resource-a": 1, "resource-b": 1}
    assert start_order[:2] == ["synthetic.a", "synthetic.b"]
    assert events.index("start:synthetic.after-a") > events.index("finish:synthetic.a")


@pytest.mark.parametrize("status, returncode", [("failure", 7), ("timeout", 124)])
def test_executor_partition_failure_preserves_independent_work_and_blocks_dependents(
    tmp_path: Path,
    monkeypatch,
    status: str,
    returncode: int,
) -> None:
    commands = (
        _synthetic_executor_command("synthetic.fail"),
        _synthetic_executor_command("synthetic.live", resource_class="resource-b"),
        _synthetic_executor_command(
            "synthetic.blocked", dependencies=["synthetic.fail"]
        ),
        _synthetic_executor_command(
            "synthetic.blocked-child", dependencies=["synthetic.blocked"]
        ),
        _synthetic_executor_command("synthetic.independent"),
        _synthetic_executor_command(
            "synthetic.after-live", dependencies=["synthetic.live"]
        ),
    )
    plan = _synthetic_executor_plan(commands, limits={"resource-a": 1, "resource-b": 1})
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )
    live_started = threading.Event()

    def fake_run(_plan, command, cancel):
        if command.id == "synthetic.fail":
            assert live_started.wait(timeout=1)
            return {
                **_successful_synthetic_record(command),
                "status": status,
                "returncode": returncode,
                "failure_scope": "partition",
                "failure_reason": "ordinary command failure",
            }
        if command.id == "synthetic.live":
            live_started.set()
            assert not cancel.wait(timeout=0.05)
        assert not command.id.startswith("synthetic.blocked")
        return _successful_synthetic_record(command)

    monkeypatch.setattr(proof_plan, "_run_command", fake_run)
    receipt_path = tmp_path / "receipt.json"
    assert proof_plan.execute_commands(plan, commands, receipt_path) == returncode
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["status"] == "failure"
    assert [record["id"] for record in receipt["commands"]] == [
        command.id for command in commands
    ]
    assert [record["status"] for record in receipt["commands"]] == [
        status,
        "success",
        "skipped",
        "skipped",
        "success",
        "success",
    ]
    assert receipt["commands"][2]["blocked_by"] == ["synthetic.fail"]
    assert receipt["commands"][3]["blocked_by"] == ["synthetic.fail"]
    assert receipt["execution"]["global_stop_triggered"] is False
    assert receipt["execution"]["cancelled_commands"] == 0
    assert receipt["execution"]["skipped_commands"] == 2


def test_actual_rust_roots_continue_after_failure_without_overlapping_capacity(
    tmp_path: Path, monkeypatch
) -> None:
    """Exercise the declared Rust DAG with finite real guarded child bodies."""
    expected = [
        "rust.test.default-truth",
        "rust.test.compiler-authorities",
        "rust.test.ir-wasm-runtime-authorities",
        "rust.test.runtime-cold-lifecycle",
        "rust.test.runtime-extension-admission",
    ]
    declared = tuple(command for command in PLAN.commands if command.id in expected)
    assert [command.id for command in declared] == expected
    marker = tmp_path / "events.jsonl"
    lease = tmp_path / "active-child"
    body = tmp_path / "finite-rust-partition.py"
    body.write_text(
        "import json, os, pathlib, sys, time\n"
        "marker, lease = map(pathlib.Path, sys.argv[1:3])\n"
        "identity = sys.argv[3]\n"
        "fd = os.open(lease, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)\n"
        "def record(event):\n"
        "    with marker.open('a', encoding='utf-8') as output:\n"
        "        output.write(json.dumps([event, identity]) + '\\n')\n"
        "try:\n"
        "    record('start')\n"
        "    time.sleep(0.03)\n"
        "    record('finish')\n"
        "finally:\n"
        "    os.close(fd)\n"
        "    lease.unlink()\n"
        "raise SystemExit(7 if identity == 'rust.test.default-truth' else 0)\n",
        encoding="utf-8",
    )
    commands = tuple(
        replace(
            command,
            data={
                **command.data,
                "argv": [
                    sys.executable,
                    str(body),
                    str(marker),
                    str(lease),
                    command.id,
                ],
                "toolchains": ["python"],
                "timeout_seconds": 30,
                # These finite scheduler children emit the fixture marker, not
                # Cargo receipts. Never clear or claim real repository evidence.
                "evidence_outputs": [],
            },
        )
        for command in declared
    )
    # Preserve actual declaration dependencies, executor fanout and capacity.
    # Source/toolchain admission is supplied by this unit fixture; execution and
    # failure classification still use the real executor, guard and children.
    plan = replace(PLAN, commands=commands)
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )
    receipt_path = tmp_path / "receipt.json"
    assert proof_plan.execute_commands(plan, commands, receipt_path) == 7
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["status"] == "failure"
    # The receipt's successful-partition inventory intentionally omits failures.
    assert receipt["executed_partitions"] == expected[1:]
    assert [record["id"] for record in receipt["commands"]] == expected
    assert receipt["execution"]["scheduled_commands"] == len(expected)
    assert receipt["execution"]["completed_commands"] == len(expected)
    assert [record["status"] for record in receipt["commands"]] == [
        "failure",
        "success",
        "success",
        "success",
        "success",
    ]
    assert receipt["commands"][0]["failure_scope"] == "partition"
    assert receipt["execution"]["global_stop_triggered"] is False
    assert receipt["execution"]["peak_active_commands"] == 1
    assert (
        receipt["execution"]["peak_active_by_resource"]["compiler-build-resource"] == 1
    )
    assert [
        json.loads(line) for line in marker.read_text(encoding="utf-8").splitlines()
    ] == [[event, identity] for identity in expected for event in ("start", "finish")]
    assert not lease.exists()


def test_executor_does_not_convert_control_plane_interrupts_into_records(
    tmp_path: Path, monkeypatch
) -> None:
    command = _synthetic_executor_command("synthetic.interrupt")
    plan = _synthetic_executor_plan((command,), limits={"resource-a": 1})
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )

    def interrupt(
        _plan: proof_plan.ProofPlan,
        _command: proof_plan.ProofCommand,
        _cancel: threading.Event,
    ) -> dict[str, object]:
        raise KeyboardInterrupt

    monkeypatch.setattr(proof_plan, "_run_command", interrupt)
    with pytest.raises(KeyboardInterrupt):
        proof_plan.execute_commands(plan, (command,), tmp_path / "receipt.json")


@pytest.mark.parametrize("agent_home", [".codex", ".claude"])
def test_executor_global_stop_uses_guard_custody_to_reap_live_process_tree(
    tmp_path: Path, monkeypatch, agent_home: str
) -> None:
    # Keep the real child cancellation oracle active from ordinary CI checkouts:
    # a project data path in argv is not a host-helper identity. The actual
    # interpreter may itself also live under an agent-managed worktree.
    child_pid_path = tmp_path / agent_home / "worktrees" / "molt" / "guarded-child.pid"
    child_pid_path.parent.mkdir(parents=True)
    fail = _synthetic_executor_command("synthetic.fail")
    live = _synthetic_executor_command("synthetic.live", resource_class="resource-b")
    fail = replace(
        fail,
        data={
            **fail.data,
            "argv": [
                sys.executable,
                "-c",
                "import pathlib, sys, time\n"
                f"marker = pathlib.Path({str(child_pid_path)!r})\n"
                "deadline = time.monotonic() + 5.0\n"
                "while True:\n"
                "    try:\n"
                "        ready = marker.read_text().strip().isdecimal()\n"
                "    except FileNotFoundError:\n"
                "        ready = False\n"
                "    if ready:\n"
                "        break\n"
                "    if time.monotonic() >= deadline:\n"
                "        print('live child readiness deadline expired', file=sys.stderr)\n"
                "        raise SystemExit(97)\n"
                "    time.sleep(0.01)\n"
                "raise SystemExit(130)\n",
            ],
        },
    )
    child_code = "import time; time.sleep(60)"
    live_code = (
        "import pathlib, subprocess, sys, time; "
        f"child=subprocess.Popen([sys.executable, '-c', {child_code!r}]); "
        f"pathlib.Path({str(child_pid_path)!r}).write_text(str(child.pid)); "
        "time.sleep(60)"
    )
    live = replace(live, data={**live.data, "argv": [sys.executable, "-c", live_code]})
    commands = (fail, live)
    plan = _synthetic_executor_plan(commands, limits={"resource-a": 1, "resource-b": 1})
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )

    started = time.monotonic()
    receipt_path = tmp_path / "receipt.json"
    assert proof_plan.execute_commands(plan, commands, receipt_path) == 130
    assert time.monotonic() - started < 20.0
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert [record["status"] for record in receipt["commands"]] == [
        "failure",
        "cancelled",
    ]
    assert child_pid_path.is_file()
    child_pid = int(child_pid_path.read_text(encoding="utf-8"))
    deadline = time.monotonic() + 5.0
    while proof_queue_custody._pid_alive(child_pid) and time.monotonic() < deadline:
        time.sleep(0.05)
    assert not proof_queue_custody._pid_alive(child_pid)


def test_executor_process_custody_is_classified_by_subprocess_guard() -> None:
    allowlist = tuple(
        entry
        for entry in check_subprocess_guard_coverage.ALLOWLIST
        if entry.path == "tools/proof_plan.py"
    )
    audit = check_subprocess_guard_coverage.audit_paths(
        [proof_plan.ROOT / "tools" / "proof_plan.py"],
        root=proof_plan.ROOT,
        allowlist=allowlist,
        text_paths=(),
    )
    assert audit.ok
    assert audit.unexpected == ()
    assert audit.stale_allowlist == ()
    assert audit.expanded_allowlist == ()


def test_executor_refuses_uncommitted_source_attestation(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ("?? stray.py",))
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    with pytest.raises(ValueError, match=r"clean source tree.*: \?\? stray\.py"):
        proof_plan.execute_commands(PLAN, (command,), tmp_path / "receipt.json")
    assert not (tmp_path / "receipt.json").exists()


def test_source_tree_changes_names_every_dirty_entry(
    tmp_path: Path, monkeypatch
) -> None:
    def git(*args: str) -> None:
        run_guarded_test_process(
            ["git", "-C", str(tmp_path), *args], check=True, capture_output=True
        )

    git("init", "-q")
    git("config", "user.email", "proof@example.invalid")
    git("config", "user.name", "Proof")
    (tmp_path / "tracked.py").write_text("x = 1\n", encoding="utf-8")
    git("add", "tracked.py")
    git("commit", "-q", "-m", "seed")
    monkeypatch.setattr(proof_plan, "ROOT", tmp_path)
    assert proof_plan._source_tree_changes() == ()

    (tmp_path / "tracked.py").write_text("x = 2\n", encoding="utf-8")
    (tmp_path / "new file é.py").write_text("", encoding="utf-8")
    assert sorted(proof_plan._source_tree_changes()) == [
        " M tracked.py",
        "?? new file é.py",
    ]


def test_executor_preflight_error_is_visible_and_receipted(
    tmp_path, monkeypatch, capsys
):
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())

    def reject_toolchain(_plan, _names):
        raise ValueError("toolchain contract violation: node version mismatch")

    monkeypatch.setattr(proof_plan, "toolchain_fingerprints", reject_toolchain)
    command = next(
        command for command in PLAN.commands if command.id == "python.static.ty"
    )
    receipt_path = tmp_path / "preflight.json"
    assert proof_plan.execute_commands(PLAN, (command,), receipt_path) == 2
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["commands"] == []
    assert receipt["status"] == "failure"
    message = capsys.readouterr().err
    assert "stage=toolchain-preflight executed=0" in message
    assert receipt["errors"][0] in message
    assert str(receipt_path) in message


def test_executor_rejects_source_mutation_during_partition(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    states = iter(((), (), ("?? tools/stray.py", " M README.md")))
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: next(states))
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )
    monkeypatch.setattr(
        proof_plan,
        "_run_command",
        lambda _plan, command, _cancel: {
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
    assert receipt["commands"][0]["source_tree_state_after"] == "changed"
    reason = "source tree changed or is dirty: ?? tools/stray.py,  M README.md"
    assert receipt["commands"][0]["failure_reason"] == reason
    assert receipt["executed_partitions"] == []
    # The log names the failing command and the paths without the receipt.
    message = capsys.readouterr().err
    assert f"proof-plan: failed python.static.ty: {reason}" in message
    assert f"status=failure failed=1; receipt={receipt_path}" in message


def test_heavy_queue_projects_the_same_receipt_schema(
    tmp_path: Path, monkeypatch
) -> None:
    del monkeypatch
    summary = tmp_path / "summary.json"
    summary.write_text(json.dumps({"peak_total": {"rss_kb": 64}}), encoding="utf-8")
    command = ["cargo", "test"]
    envelope = command_admission.envelope_for_command(command)
    toolchains = {
        name: {"identity_sha256": hashlib.sha256(name.encode()).hexdigest()}
        for name in envelope["toolchains"]
    }
    receipt: Any = proof_queue_evidence._queue_proof_receipt(
        _sealed_terminal_row(
            {
                "run_id": "heavy-native-run",
                "logical_id": "heavy-native",
                "status": "passed",
                "returncode": 0,
                "command_json": json.dumps(command),
                "command_envelope_json": json.dumps(envelope),
                "cwd": str(tmp_path),
                "resource_family": "native-build",
                "started_at": "2026-07-18T00:00:00+00:00",
                "elapsed_s": 1.25,
                "summary_json": str(summary),
                "receipt_context_json": json.dumps(
                    {
                        "schema": PLAN.receipt_schema,
                        "authority_sha256": proof_plan._authority_sha256(PLAN),
                        "source_commit": proof_plan._source_commit(),
                        "source_tree": "d" * 40,
                        "source_tree_state": "clean",
                        "environment": {
                            "os": "windows",
                            "arch": "x86_64",
                            "python": "3.12",
                        },
                        "toolchains": toolchains,
                        "command_envelope": envelope,
                        "command_envelope_sha256": hashlib.sha256(
                            json.dumps(
                                envelope, sort_keys=True, separators=(",", ":")
                            ).encode()
                        ).hexdigest(),
                        "python_interpreters": {
                            "queue_control_plane": {"version": "3.14.3"},
                            "proof_command": {"version": "3.12.13"},
                        },
                        "source_custody": {
                            "evidence_eligible": True,
                            "ineligible_reasons": [],
                        },
                        "guard_receipt": {"sha256": "e" * 64},
                    }
                ),
            }
        )
    )
    assert receipt["schema"] == PLAN.receipt_schema
    assert receipt["authority_kind"] == "proof-queue-dynamic-command"
    assert receipt["source_tree_state"] == "clean"
    assert receipt["executed_partitions"] == ["queue.heavy-native"]
    assert receipt["commands"][0]["peak_rss_bytes"] == 64 * 1024  # type: ignore[index]


def test_heavy_queue_reuses_persisted_execution_receipt_context() -> None:
    command = ["cargo", "test"]
    envelope = command_admission.envelope_for_command(command)
    toolchains = {
        name: {"identity_sha256": hashlib.sha256(name.encode()).hexdigest()}
        for name in envelope["toolchains"]
    }
    context = {
        "schema": PLAN.receipt_schema,
        "authority_sha256": "a" * 64,
        "source_commit": "b" * 40,
        "source_tree": "d" * 40,
        "source_tree_state": "clean",
        "environment": {"os": "linux", "arch": "x86_64", "python": "3.12"},
        "toolchains": toolchains,
        "command_envelope": envelope,
        "command_envelope_sha256": hashlib.sha256(
            json.dumps(envelope, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "source_custody": {
            "evidence_eligible": True,
            "ineligible_reasons": [],
        },
        "guard_receipt": {"sha256": "e" * 64},
    }
    row = _sealed_terminal_row(
        {
            "run_id": "heavy-native-run",
            "logical_id": "heavy-native",
            "status": "passed",
            "returncode": 0,
            "command_json": json.dumps(command),
            "command_envelope_json": json.dumps(envelope),
            "cwd": ".",
            "resource_family": "native-build",
            "started_at": "2026-07-18T00:00:00+00:00",
            "elapsed_s": 1.25,
            "summary_json": None,
            "receipt_context_json": json.dumps(context),
        }
    )
    first = proof_queue_evidence._queue_proof_receipt(row)
    second = proof_queue_evidence._queue_proof_receipt(row)
    assert first["toolchains"] == context["toolchains"]
    assert second["environment"]["python"] == "3.12"


def test_formal_is_required_now_that_cross_workflow_receipts_are_aggregated() -> None:
    formal = next(family for family in PLAN.families if family.name == "formal")
    assert formal.data["required"] is True
    assert {command.id for command in PLAN.commands if command.family == "formal"} == {
        "formal.lean.build",
        "formal.lean.sorry-baseline",
        "formal.quint.models",
        "formal.correspondence",
    }


def test_terminal_command_execution_closes_transitive_dependencies() -> None:
    commands = proof_plan._topological_commands(
        PLAN,
        command_id="formal.lean.sorry-baseline",
    )
    assert [command.id for command in commands] == [
        "formal.lean.build",
        "formal.lean.sorry-baseline",
    ]


def test_nightly_workflow_is_a_typed_scheduled_family_consumer() -> None:
    scheduled = {
        family.name: family
        for family in PLAN.scheduled_families
        if family.data["workflow"] == ".github/workflows/nightly.yml"
    }
    assert set(scheduled) == {
        "nightly_conformance",
        "nightly_determinism",
        "nightly_differential",
        "nightly_regrtest",
        "nightly_shard_profile_feedback",
        "nightly_shard_prepare",
        "nightly_verification_t3",
    }
    workflow = (proof_plan.ROOT / ".github/workflows/nightly.yml").read_text(
        encoding="utf-8"
    )
    for name in scheduled:
        assert f"--run-family {name} --receipt" in workflow
    for program, count in (("conformance", 8), ("differential", 16), ("regrtest", 4)):
        assert f"max-parallel: {count}" in workflow
        assert f"outputs.{program}_matrix" in workflow
        assert f"--program {program}" in workflow
    assert workflow.count("tools/nightly_runtime_bundle.py verify-extract") == 3
    for forbidden in (
        "tests/harness/run_molt_conformance.py",
        "tests/molt_diff.py",
        "tools/cpython_regrtest.py",
        "tools/check_deterministic_runtime.py",
        "tools/verify_ir_suite.py",
        "tools/ci_gate.py --tier",
    ):
        assert forbidden not in workflow


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
    assert replay["families"]["python_static"]["selected"] == 1
    assert replay["families"]["rust"]["selected"] == 1
    assert replay["families"]["rust_security"]["selected"] == 0
    assert replay["families"]["rust_security"]["avoidable_percent"] == 100.0


@pytest.mark.parametrize(
    ("path", "families"),
    [
        (
            "tests/proof_queue_owned_roots.py",
            {"python_unit", "platform_portability", "native_integration"},
        ),
        (
            "tools/proof_queue_pkg/custody_cas.py",
            {"python_unit", "platform_portability", "native_integration"},
        ),
        (
            "tests/tools/test_proof_queue_output_layout.py",
            {"python_unit", "platform_portability"},
        ),
    ],
)
def test_cas_executable_placement_dependencies_select_all_owning_families(
    path, families
):
    selected = {family.name for family in PLAN.select([path]).selected}
    assert families <= selected


def test_cas_placement_models_remain_mandatory_on_unit_and_all_portability_cells():
    commands = {command.id: command for command in PLAN.commands}
    for cid in (
        "python.unit.harness",
        "portability.queue.linux",
        "portability.queue.macos",
        "portability.queue.windows",
    ):
        command = commands[cid]
        assert "tests/tools/test_proof_queue_output_layout.py" in command.argv
        assert command.data["timeout_seconds"] == (
            900 if cid == "python.unit.harness" else 1800
        )


@pytest.mark.parametrize(
    "metrics, returncode, expected",
    [
        ({}, 7, "partition"),
        (
            {
                "timed_out": True,
                "termination_reports": [{"remaining_pids": [], "remaining_pgids": []}],
            },
            124,
            "partition",
        ),
        (
            {
                "timed_out": True,
                "windows_job_cleanup": {"completed": True, "remaining_processes": []},
            },
            124,
            "partition",
        ),
        (
            {
                "timed_out": True,
                "termination_reports": [{"remaining_pids": [], "remaining_pgids": []}],
            },
            0,
            "global",
        ),
        ({}, 124, "global"),
        ({"timed_out": True}, 124, "global"),
        ({"timed_out": True, "termination_reports": [{}]}, 124, "global"),
        (
            {
                "timed_out": True,
                "termination_reports": [
                    {"remaining_pids": [123], "remaining_pgids": []}
                ],
            },
            124,
            "global",
        ),
        ({"memory_violation": {"rss_kb": 123}}, 124, "global"),
        ({"guard_signal": 15}, 143, "global"),
        ({"infrastructure_failure": {"phase": "process_custody"}}, 2, "global"),
        (
            {
                "cargo_incremental_quarantine": {
                    "ownership_status": "deferred",
                    "errors": [],
                }
            },
            124,
            "global",
        ),
        (
            {"cargo_incremental_quarantine": {"ownership_status": "quarantined"}},
            7,
            "global",
        ),
        (
            {
                "cargo_incremental_quarantine": {
                    "ownership_status": "partial",
                    "errors": ["owned compiler still live"],
                }
            },
            124,
            "global",
        ),
        (
            {
                "timed_out": True,
                "termination_reports": [{"remaining_pids": [], "remaining_pgids": []}],
                "cargo_incremental_quarantine": {
                    "ownership_status": "quarantined",
                    "errors": [],
                    "interruption_inventory_complete": True,
                },
            },
            124,
            "partition",
        ),
        (
            {
                "timed_out": True,
                "termination_reports": [{"remaining_pids": [], "remaining_pgids": []}],
                "cargo_incremental_quarantine": {
                    "ownership_status": "quarantined",
                    "errors": [],
                    "interruption_inventory_complete": False,
                },
            },
            124,
            "global",
        ),
    ],
)
def test_executor_failure_scope_uses_guard_and_quarantine_authority(
    metrics, returncode, expected
) -> None:
    scope, reason = proof_plan._guarded_failure_scope(
        {"descendants_closed": True, **metrics},
        metrics_valid=True,
        returncode=returncode,
        cancelled=False,
    )
    assert scope == expected
    assert reason


def _job_cleanup_with_survivor(image: Path) -> dict[str, object]:
    return {
        "descendants_closed": True,
        "windows_job_cleanup": {
            "completed": True,
            "remaining_processes": [{"pid": 720, "image": str(image)}],
        },
    }


def test_job_closure_admits_only_declared_helpers_beside_their_linker(
    tmp_path: Path,
) -> None:
    admitted = proof_plan._admitted_linker_helpers(PLAN)
    assert admitted["vctip.exe"] == frozenset({"link.exe"})
    msvc = tmp_path / "MSVC" / "bin"
    msvc.mkdir(parents=True)
    (msvc / "link.exe").write_bytes(b"")
    (msvc / "vctip.exe").write_bytes(b"")
    lone = tmp_path / "elsewhere"
    lone.mkdir()
    (lone / "vctip.exe").write_bytes(b"")
    uncertain = ("global", "guard job closure is uncertain")

    def scope(image: Path, helpers=admitted) -> tuple[str, str | None]:
        return proof_plan._guarded_failure_scope(
            _job_cleanup_with_survivor(image),
            metrics_valid=True,
            returncode=0,
            cancelled=False,
            admitted_linker_helpers=helpers,
        )

    # MSVC's linker leaves its telemetry helper running after it exits; the
    # plan declares it, so terminating it through the Job is ordinary closure.
    assert scope(msvc / "vctip.exe") != uncertain
    assert scope(msvc / "VCTIP.EXE") != uncertain
    # A helper name without its declared linker beside it, an undeclared
    # survivor, or no plan policy at all remains uncertain closure.
    assert scope(lone / "vctip.exe") == uncertain
    assert scope(msvc / "link.exe") == uncertain
    assert scope(msvc / "vctip.exe", helpers={}) == uncertain


def test_executor_missing_guard_outcome_is_a_global_stop() -> None:
    assert (
        proof_plan._guarded_failure_scope(
            {}, metrics_valid=False, returncode=7, cancelled=False
        )[0]
        == "global"
    )


def test_proof_source_commit_rejects_mismatched_ci_declaration(monkeypatch) -> None:
    monkeypatch.setenv("GITHUB_SHA", "b" * 40)
    monkeypatch.setattr(proof_plan, "_run_git", lambda _args: "a" * 40)
    with pytest.raises(ValueError, match="actual checkout HEAD"):
        proof_plan._source_commit()


def test_proof_source_identity_binds_commit_and_immutable_tree(monkeypatch) -> None:
    monkeypatch.setenv("GITHUB_SHA", "a" * 40)

    def git(args):
        return "c" * 40 if args[-1] == ("a" * 40) + "^{tree}" else "a" * 40

    monkeypatch.setattr(proof_plan, "_run_git", git)
    assert proof_plan._source_identity() == {"commit": "a" * 40, "tree": "c" * 40}


@pytest.mark.parametrize("field", ["commit", "tree"])
def test_executor_stops_on_candidate_change_even_when_checkout_is_clean(
    tmp_path, monkeypatch, field
) -> None:
    original = {"commit": "a" * 40, "tree": "b" * 40}
    current = dict(original)
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(proof_plan, "_source_identity", lambda: dict(current))
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )
    commands = (
        _synthetic_executor_command("change"),
        _synthetic_executor_command("after", dependencies=["change"]),
    )
    plan = _synthetic_executor_plan(commands, limits={"resource-a": 1})

    def run(_plan, command, _cancel):
        current[field] = "c" * 40
        return _successful_synthetic_record(command)

    monkeypatch.setattr(proof_plan, "_run_command", run)
    output = tmp_path / "receipt.json"
    assert proof_plan.execute_commands(plan, commands, output) == 2
    receipt = json.loads(output.read_text(encoding="utf-8"))
    assert receipt["source_commit"] == original["commit"]
    assert receipt["source_tree"] == original["tree"]
    assert receipt["execution"]["global_stop_triggered"] is True
    assert (
        receipt["commands"][0]["failure_reason"]
        == "candidate HEAD or tree identity changed"
    )
    assert receipt["commands"][1]["status"] == "skipped"


def test_executor_control_plane_interrupt_cancels_siblings_before_join(
    tmp_path, monkeypatch
) -> None:
    commands = (
        _synthetic_executor_command("interrupt"),
        _synthetic_executor_command("live", resource_class="resource-b"),
    )
    plan = _synthetic_executor_plan(commands, limits={"resource-a": 1, "resource-b": 1})
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda _plan, _names: {"python": {"identity_sha256": "0" * 64}},
    )
    live_started = threading.Event()
    closed = threading.Event()

    def run(_plan, command, cancel):
        if command.id == "interrupt":
            assert live_started.wait(1)
            raise KeyboardInterrupt
        live_started.set()
        assert cancel.wait(1)
        closed.set()
        return {
            **_successful_synthetic_record(command),
            "status": "cancelled",
            "returncode": 130,
            "failure_scope": "global",
        }

    monkeypatch.setattr(proof_plan, "_run_command", run)
    with pytest.raises(KeyboardInterrupt):
        proof_plan.execute_commands(plan, commands, tmp_path / "receipt.json")
    assert closed.is_set()
    receipt = json.loads((tmp_path / "receipt.json").read_text(encoding="utf-8"))
    assert receipt["status"] == "failure"
    assert [record["status"] for record in receipt["commands"]] == [
        "failure",
        "cancelled",
    ]
    assert receipt["execution"]["completed_commands"] == 2
    assert receipt["execution"]["cancelled_commands"] == 1


def test_executor_real_cargo_test_timeout_retains_completed_incremental_cache(
    tmp_path: Path,
) -> None:
    project = tmp_path / "cargo-timeout"
    project.mkdir()
    (project / "Cargo.toml").write_text(
        '[package]\nname = "molt-guard-timeout-proof"\nversion = "0.0.0"\n'
        'edition = "2024"\n[workspace]\n[[test]]\nname = "hang"\npath = "hang.rs"\n',
        encoding="utf-8",
    )
    (project / "hang.rs").write_text(
        "#[test]\nfn hanging_test() {\n"
        'std::fs::write(std::env::var("MOLT_TEST_STARTED").unwrap(), b"started").unwrap();\n'
        "loop { std::thread::sleep(std::time::Duration::from_secs(1)); }\n}\n",
        encoding="utf-8",
    )
    started = project / "test-started"
    target = project / "target"
    cargo_env = {"CARGO_TARGET_DIR": str(target), "CARGO_INCREMENTAL": "1"}
    manifest = ["--manifest-path", str(project / "Cargo.toml"), "--test", "hang"]
    # Compile outside the timed partition: the 12-second budget then covers only
    # the hanging test, on any host speed (a cold compile under Rosetta alone
    # exceeds it).
    build = run_guarded_test_process(
        ["cargo", "test", "--offline", "--no-run", *manifest],
        env={**os.environ, **cargo_env},
        timeout=600,
    )
    assert build.returncode == 0, build.stderr
    incremental = target / "debug" / "incremental"
    compiled = sorted(path.name for path in incremental.iterdir())
    assert compiled
    command = proof_plan.ProofCommand(
        "synthetic.cargo-timeout",
        {
            **_synthetic_executor_command("synthetic.cargo-timeout").data,
            "timeout_seconds": 12,
            "toolchains": ["cargo", "python"],
            "argv": ["cargo", "test", "--offline", *manifest],
            "env": {**cargo_env, "MOLT_TEST_STARTED": str(started)},
        },
    )
    plan = _synthetic_executor_plan((command,), limits={"resource-a": 1})
    record = proof_plan._run_command(plan, command)
    assert started.read_bytes() == b"started", record
    assert record["status"] == "timeout"
    assert record["returncode"] == 124
    # Windows Job lifetime accounting fences unseen births through termination.
    # Ordinary POSIX snapshots cannot establish that stronger negative fact.
    assert record["failure_scope"] == (
        "partition" if sys.platform == "win32" else "global"
    ), record
    # The kill leaves the completed incremental cache exactly as it was.
    assert sorted(path.name for path in incremental.iterdir()) == compiled


@pytest.mark.parametrize(
    ("environ", "expected"),
    [
        ({"GITHUB_EVENT_NAME": "pull_request"}, "pr"),
        ({"GITHUB_EVENT_NAME": "push"}, "main"),
        ({"GITHUB_EVENT_NAME": "merge_group"}, "main"),
        ({"GITHUB_EVENT_NAME": "schedule"}, "scheduled"),
        ({"GITHUB_EVENT_NAME": "pull_request", "MOLT_PROOF_TIER": "main"}, "main"),
        ({}, None),
    ],
)
def test_active_tier_follows_the_ci_event(environ, expected) -> None:
    assert proof_plan.active_tier(environ) == expected


def test_classifier_text_output_names_every_family_and_matrix(capsys) -> None:
    # CI's classifier runs the text mode; it must print exactly the family and
    # matrix outputs the family jobs consume, with the computed values.
    assert proof_plan.main(["--path", "tools/proof_queue.py", "--tier", "pr"]) == 0
    printed = dict(line.split("=", 1) for line in capsys.readouterr().out.splitlines())
    expected = proof_plan.family_outputs(
        PLAN, PLAN.select(["tools/proof_queue.py"]), tier="pr"
    )
    names = {family.name for family in PLAN.families} | {
        proof_plan.family_matrix_output(family.name)
        for family in PLAN.families
        if family.data["executor"] == "github-matrix"
    }
    assert set(printed) == names
    assert printed == {name: expected[name] for name in names}


def test_empty_verified_selection_fails_with_its_cause(tmp_path, capsys) -> None:
    status = proof_plan.main(["--verify-selected", "", "--receipt-dir", str(tmp_path)])
    assert status == 2
    assert "classifier produced no selection" in capsys.readouterr().err


def test_active_tier_rejects_unknown_explicit_tiers() -> None:
    with pytest.raises(ValueError, match="MOLT_PROOF_TIER='weekly'"):
        proof_plan.active_tier({"MOLT_PROOF_TIER": "weekly"})


def test_every_event_maps_into_the_tier_vocabulary() -> None:
    assert set(proof_plan._EVENT_TIERS.values()) <= set(proof_plan.PROOF_TIERS)


def test_plan_rejects_tiers_outside_the_vocabulary() -> None:
    families = tuple(
        replace(family, data={**family.data, "tiers": ["pr", "main", "weekly"]})
        if family.name == "python_security"
        else family
        for family in PLAN.families
    )
    errors = replace(PLAN, families=families).validate()
    assert (
        "python_security: tiers ['pr', 'main', 'weekly'] must come from "
        "['pre-push', 'pr', 'main', 'scheduled']"
    ) in errors


def test_exact_pins_and_their_identity_patterns_move_together() -> None:
    policies = tuple(
        replace(policy, data={**policy.data, "setup_value": "99.0.0"})
        if policy.name == "node"
        else policy
        for policy in PLAN.toolchain_policies
    )
    errors = replace(PLAN, toolchain_policies=policies).validate()
    assert any(
        error.startswith("node: version_pattern") and "'99.0.0'" in error
        for error in errors
    ), errors


def test_plan_rejects_a_family_tier_that_gates_no_command() -> None:
    families = tuple(
        replace(family, data={**family.data, "tiers": ["pre-push", "pr", "main"]})
        if family.name == "rust_security"
        else family
        for family in PLAN.families
    )
    errors = replace(PLAN, families=families).validate()
    assert "rust_security: tier 'pre-push' gates no command" in errors


def test_scheduled_families_run_their_scheduled_tier_on_any_event() -> None:
    # A manual dispatch maps to `main`; a scheduled family must still prove
    # exactly what its schedule proves instead of running nothing.
    for family in PLAN.scheduled_families:
        dispatched = proof_plan._topological_commands(
            PLAN, family=family.name, tier="main"
        )
        scheduled = proof_plan._topological_commands(
            PLAN, family=family.name, tier="scheduled"
        )
        assert dispatched == scheduled and dispatched, family.name


def test_family_run_with_no_tier_commands_fails_loud() -> None:
    with pytest.raises(ValueError, match="has no commands in tier 'pre-push'"):
        proof_plan._topological_commands(PLAN, family="rust_security", tier="pre-push")


def test_pin_freshness_runs_only_on_schedule() -> None:
    pr = {
        command.id
        for command in proof_plan._topological_commands(
            PLAN, family="python_security", tier="pr"
        )
    }
    scheduled = {
        command.id
        for command in proof_plan._topological_commands(
            PLAN, family="python_security", tier="scheduled"
        )
    }
    assert "security.pin-freshness" not in pr
    assert "security.pin-freshness" in scheduled


def test_pull_requests_skip_main_only_commands_but_keep_their_dependencies() -> None:
    main_only = [
        command
        for command in PLAN.commands
        if "pr" not in command.data["tiers"] and "main" in command.data["tiers"]
    ]
    assert main_only, "the plan keeps whole-suite commands off pull requests"
    selection = PLAN.select(["tools/proof_queue.py"])
    pr = proof_plan.family_outputs(PLAN, selection, tier="pr")
    main = proof_plan.family_outputs(PLAN, selection, tier="main")
    pr_ids = {
        command_id
        for entry in json.loads(pr["platform_portability_matrix"])["include"]
        for command_id in entry["command_ids"]
    }
    main_ids = {
        command_id
        for entry in json.loads(main["platform_portability_matrix"])["include"]
        for command_id in entry["command_ids"]
    }
    assert "portability.queue.linux" in main_ids - pr_ids
    # The complete main queue owns the fixture once; PRs exercise the real
    # compiler/toolchain custody boundary directly before any main landing.
    selector = (
        "tests/tools/test_proof_queue.py::"
        "test_real_minimal_cargo_link_has_one_selection_per_unit_and_compact_custody"
    )
    cells = {
        "linux": "linux-x86_64-py312-queue-portability",
        "macos": "macos-arm64-py312-queue-portability",
        "windows": "windows-x86_64-py312-queue-portability",
    }
    for host, cell in cells.items():
        command_id = f"portability.cargo-link.{host}"
        assert command_id in pr_ids - main_ids
        command = next(c for c in PLAN.commands if c.id == command_id)
        assert command.data["cell"] == cell
        assert selector in command.data["argv"]
        assert (
            "tests/tools/test_proof_queue.py::"
            "test_python_selection_location_join_preserves_coordinate_and_content"
        ) in command.data["argv"]
        assert command.data["timeout_seconds"] == (180 if host == "windows" else 120)
        assert {"python", "uv", "rustc", "cargo"} <= set(command.toolchains)
        main_command = next(
            c for c in PLAN.commands if c.id == f"portability.queue.{host}"
        )
        assert main_command.data["argv"].count("tests/tools/test_proof_queue.py") == 1
    for family in {command.family for command in PLAN.commands}:
        ran = proof_plan._topological_commands(PLAN, family=family, tier="pr")
        ran_ids = {command.id for command in ran}
        for command in ran:
            # Tiers select roots; every dependency of a root still runs.
            assert set(command.dependencies) <= ran_ids


def test_family_without_tier_commands_starts_no_runner(tmp_path) -> None:
    selection = PLAN.select(["tools/proof_queue.py"])
    outputs = proof_plan.family_outputs(PLAN, selection, tier="no-such-tier")
    assert json.loads(outputs["selected"]) == []
    for family in PLAN.families:
        if family.data["executor"] == "github-matrix":
            output = proof_plan.family_matrix_output(family.name)
            assert json.loads(outputs[output]) == {"include": []}
    assert all(outputs[family.name] == "false" for family in PLAN.families)


@pytest.mark.parametrize("suffix", ["", ".macos"])
def test_python_rust_consumers_admit_tools_before_any_partition(
    tmp_path: Path, monkeypatch, suffix: str
) -> None:
    # The frontend frame and CLI cache-identity suites really invoke Cargo.
    # Removing either typed prerequisite must fail before the executor can
    # schedule a command, rather than be masked by a warm runner installation.
    commands = tuple(
        next(command for command in PLAN.commands if command.id == name + suffix)
        for name in ("python.unit.binding-authority", "python.unit.runtime-artifacts")
    )
    assert "tests/test_python_execution_frame.py" in commands[0].argv
    assert "tests/cli/test_cli_shared_stdlib_cache.py" in commands[1].argv
    for command in commands:
        assert {"rustc", "cargo"} <= set(PLAN.required_toolchains(command))
    observed = []

    def reject_incomplete_installation(_plan, names):
        observed.append(names)
        assert {"python", "uv", "rustc", "cargo"} <= set(names)
        raise ValueError("independent fixture: partial Rust installation")

    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(
        proof_plan, "toolchain_fingerprints", reject_incomplete_installation
    )
    monkeypatch.setattr(
        proof_plan,
        "_run_command",
        lambda *_args: pytest.fail("payload scheduled before toolchain admission"),
    )
    receipt_path = tmp_path / "unprovisioned.json"
    assert proof_plan.execute_commands(PLAN, commands, receipt_path) == 2
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert len(observed) == 1
    assert receipt["commands"] == []
    assert receipt["execution"]["scheduled_commands"] == 0
    assert receipt["status"] == "failure"


def test_setup_contract_inputs_and_native_portability_routes_are_complete() -> None:
    for path in (
        ".github/actions/setup-project/normalize-inputs.sh",
        ".github/actions/setup-project/provision-python.sh",
        ".github/actions/setup-project/provision-rust.py",
        "tools/check_rust_toolchain.py",
        "tests/tools/test_setup_project_inputs.py",
        "tests/tools/test_rust_toolchain_contract.py",
    ):
        assert path in PLAN.authority_inputs
        assert _classes(path)["platform_portability"] is True
        assert _classes(path)["python_unit"] is True
    commands = {command.id: command for command in PLAN.commands}
    for name in (
        "repository.docs-tests",
        "portability.queue.linux",
        "portability.queue.macos",
        "portability.queue.windows",
    ):
        for path in (
            "tests/tools/test_setup_project_inputs.py",
            "tests/tools/test_rust_toolchain_contract.py",
        ):
            assert commands[name].argv.count(path) == 1


@pytest.mark.parametrize(
    "defect",
    [
        None,
        "missing",
        "unclosed",
        "startup",
        "infrastructure",
        "refusal",
        "wrong-returncode",
        "noncanonical-code",
        "not-cancelled",
    ],
)
def test_executor_cancellation_requires_exact_terminal_closure(
    tmp_path, monkeypatch, defect
):
    from tools.command_execution import GuardedCommand

    command = _synthetic_executor_command("synthetic.cancel")
    plan = _synthetic_executor_plan((command,), limits={"resource-a": 1})
    event = threading.Event()
    startup = {
        "launch_id": "a" * 32,
        "guard_pid": 410,
        "command": list(command.argv),
        "child_process": {"pid": 411, "started_at": "fixture-birth"},
    }
    terminal = {
        **startup,
        "schema": "molt.guarded-command-metrics.v1",
        "returncode": 137,
        "child_returncode": -15,
        "duration_seconds": 0.2,
        "peak_tree_rss_bytes": 1024,
        "cancelled": True,
        "descendants_closed": True,
    }
    startup_path = tmp_path / "startup.json"
    summary_path = tmp_path / "summary.json"
    if defect == "unclosed":
        terminal["descendants_closed"] = False
    elif defect == "startup":
        startup["launch_id"] = "b" * 32
    elif defect == "infrastructure":
        terminal["infrastructure_failure"] = {"phase": "temporary_artifact_custody"}
    elif defect == "refusal":
        terminal["termination_reports"] = [
            {"remaining_pids": [411], "remaining_pgids": []}
        ]
    elif defect == "noncanonical-code":
        terminal["returncode"] = 130
    elif defect == "not-cancelled":
        terminal["cancelled"] = False
    elif defect == "wrong-returncode":
        terminal["returncode"] = 0
    startup_path.write_text(json.dumps(startup), encoding="utf-8")
    if defect != "missing":
        summary_path.write_text(json.dumps(terminal), encoding="utf-8")

    class Process:
        pid = 409  # Launcher identity deliberately differs from the guard.
        returncode = None

        def poll(self):
            return self.returncode

        def wait(self, timeout=None):
            self.returncode = 130 if defect == "noncanonical-code" else 137
            return self.returncode

        def terminate(self):
            pytest.fail("guard owner must remain alive until its own closure")

        kill = terminate

    owned = GuardedCommand(
        Process(),
        tmp_path / "cancel",
        summary_path,
        tmp_path / "custody.json",
        "a" * 32,
        startup_path,
        command.argv,
    )

    def start(*_args, **kwargs):
        assert kwargs["harness"] is True
        assert "summary_json" not in kwargs  # A unique owner directory survives retry.
        event.set()
        return owned

    monkeypatch.setattr(proof_plan, "_COMMANDS", SimpleNamespace(start_guarded=start))
    if defect in {"missing", "unclosed", "startup"}:
        with pytest.raises(RuntimeError) as caught:
            proof_plan._run_command(plan, command, event)
        assert caught.value.guard_command is owned
        record = caught.value.proof_record
    else:
        record = proof_plan._run_command(plan, command, event)
    assert record["guard_returncode"] == (130 if defect == "noncanonical-code" else 137)
    assert record["failure_scope"] == "global"
    assert record["status"] == ("cancelled" if defect is None else "failure")
    assert record["returncode"] == (130 if defect is None else 2)
    assert record["guard_custody"]["launch_id"] == owned.launch_id
    assert record["guard_custody"]["summary_path"] == str(summary_path)
    assert owned.cancellation_path.exists()
    assert summary_path.exists() is (defect != "missing")


def test_executor_retains_owner_in_failure_receipt_and_library_exception(
    tmp_path, monkeypatch
):
    from tools.command_execution import GuardedCommand

    command = _synthetic_executor_command("synthetic.unresolved")
    plan = _synthetic_executor_plan((command,), limits={"resource-a": 1})
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(
        proof_plan,
        "toolchain_fingerprints",
        lambda *_: {"python": {"identity_sha256": "0" * 64}},
    )
    process = SimpleNamespace(pid=31, returncode=None)
    owned = GuardedCommand(
        process,
        tmp_path / "cancel",
        tmp_path / "summary.json",
        tmp_path / "custody.json",
        "c" * 32,
        tmp_path / "startup.json",
        command.argv,
    )
    error = subprocess.TimeoutExpired("guard", 5)
    error.guard_command = owned
    record = {
        **_successful_synthetic_record(command),
        "status": "failure",
        "returncode": 2,
        "failure_scope": "global",
        "failure_reason": "guard outcome unavailable or inconsistent",
        "guard_custody": {
            "launch_id": owned.launch_id,
            "summary_path": str(owned.summary_path),
            "startup_path": str(owned.startup_path),
            "evidence_path": str(owned.evidence_path),
            "cancellation_path": str(owned.cancellation_path),
            "terminal": False,
        },
    }
    error.proof_record = record

    def run(*_args):
        raise error

    monkeypatch.setattr(proof_plan, "_run_command", run)
    receipt_path = tmp_path / "receipt.json"
    with pytest.raises(ExceptionGroup) as caught:
        proof_plan.execute_commands(plan, (command,), receipt_path)
    assert caught.value.exceptions == (error,)
    assert caught.value.exceptions[0].guard_command is owned
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["status"] == "failure"
    assert receipt["executed_partitions"] == []
    assert receipt["commands"][0]["guard_custody"] == record["guard_custody"]
    # The CLI may release its Python objects: ordinary durable custody references
    # remain in the failure receipt and the autonomous guard owns eventual close.
    monkeypatch.setattr(proof_plan.ProofPlan, "load", lambda _: plan)
    monkeypatch.setattr(
        proof_plan, "_topological_commands", lambda *_a, **_k: (command,)
    )
    assert (
        proof_plan.main(["--run-command", command.id, "--receipt", str(receipt_path)])
        == 2
    )
    assert (
        json.loads(receipt_path.read_text(encoding="utf-8"))["commands"][0][
            "guard_custody"
        ]["launch_id"]
        == owned.launch_id
    )


def test_cli_process_exit_preserves_autonomous_guard_and_eventual_closure(tmp_path):
    """The CLI's interpreter exits while its existing guard still owns closure."""
    from tools.command_execution import CommandExecutor
    from tools import memory_guard

    root = Path(proof_plan.__file__).resolve().parents[1]
    release = tmp_path / "release"
    entered = tmp_path / "entered"
    ready = tmp_path / "ready"
    receipt = tmp_path / "receipt.json"
    worker = tmp_path / "held_worker.py"
    worker.write_text(
        "import os,pathlib,sys,time\n"
        f"sys.path.insert(0, {str(root)!r})\n"
        "from tools import guarded_exec\n"
        "if os.environ.get('MOLT_TEST_HOLD_CLOSURE') == '1':\n"
        " guard=guarded_exec.harness_memory_guard.memory_guard\n"
        " original=guard._temporary_artifact_descendant_closure\n"
        " def held(**kwargs):\n"
        f"  pathlib.Path({str(entered)!r}).write_text('entered', encoding='utf-8')\n"
        "  deadline=time.monotonic()+30\n"
        f"  while not pathlib.Path({str(release)!r}).exists() and time.monotonic()<deadline: time.sleep(.02)\n"
        "  return original(**kwargs)\n"
        " guard._temporary_artifact_descendant_closure=held\n"
        "raise SystemExit(guarded_exec.main())\n",
        encoding="utf-8",
    )
    live_code = f"import pathlib,time; pathlib.Path({str(ready)!r}).write_text('ready', encoding='utf-8'); time.sleep(30)"
    fail_code = (
        "import pathlib,time; deadline=time.monotonic()+15\n"
        f"while not pathlib.Path({str(ready)!r}).exists() and time.monotonic()<deadline: time.sleep(.02)\n"
        "raise SystemExit(130)"
    )
    commands = (
        replace(
            _synthetic_executor_command("synthetic.fail"),
            data={
                **_synthetic_executor_command("synthetic.fail").data,
                "argv": [sys.executable, "-c", fail_code],
                "timeout_seconds": 20,
            },
        ),
        replace(
            _synthetic_executor_command("synthetic.live", resource_class="resource-b"),
            data={
                **_synthetic_executor_command(
                    "synthetic.live", resource_class="resource-b"
                ).data,
                "argv": [sys.executable, "-c", live_code],
                "timeout_seconds": 20,
                "env": {"MOLT_TEST_HOLD_CLOSURE": "1"},
            },
        ),
    )
    cli = tmp_path / "proof_cli.py"
    cli.write_text(
        "import sys\nfrom dataclasses import replace\nfrom pathlib import Path\n"
        f"sys.path.insert(0, {str(root)!r})\n"
        "from tools import proof_plan as p\nfrom tools.command_execution import CommandExecutor\n"
        "base=p.ProofPlan.load()\n"
        f"commands=tuple(p.ProofCommand(identity,data) for identity,data in {[(c.id, c.data) for c in commands]!r})\n"
        "plan=replace(base,commands=commands,executor_max_workers=2,resource_policies=(p.ResourcePolicy('resource-a',1),p.ResourcePolicy('resource-b',1)))\n"
        "p.ProofPlan.load=lambda _:plan\np._topological_commands=lambda *a,**k:commands\n"
        "p._source_tree_changes=lambda:()\np.toolchain_fingerprints=lambda *a:{}\n"
        "p._source_identity=lambda:{'commit':'a'*40,'tree':'b'*40}\n"
        "start=CommandExecutor.start_owned\n"
        "def held_start(self,args,**kwargs):\n"
        f" return start(self,[args[0],{str(worker)!r},*args[2:]],**kwargs)\n"
        "CommandExecutor.start_owned=held_start\n"
        f"raise SystemExit(p.main(['--run-family','synthetic','--receipt',{str(receipt)!r}]))\n",
        encoding="utf-8",
    )
    executor = CommandExecutor(prefix="MOLT_TEST_CLI_EXIT", repo_root=root)
    with (tmp_path / "cli.log").open("wb") as output:
        process = executor.start_owned(
            [sys.executable, str(cli)],
            cwd=root,
            env={**os.environ, "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "state")},
            stdout=output,
            stderr=output,
        )
        try:
            assert process.wait(timeout=20) == 2
            payload = json.loads(receipt.read_text(encoding="utf-8"))
            assert payload["status"] == "failure"
            live = next(r for r in payload["commands"] if r["id"] == "synthetic.live")
            custody = live["guard_custody"]
            assert live["status"] == "failure"
            assert custody["terminal"] is False
            assert custody["observation_error"]["type"] == "TimeoutExpired"
            assert entered.is_file()
            startup = json.loads(
                Path(custody["startup_path"]).read_text(encoding="utf-8")
            )
            assert startup["launch_id"] == custody["launch_id"]
            assert startup["guard_pid"] in memory_guard.sample_processes()
            release.write_text("release", encoding="utf-8")
            deadline = time.monotonic() + 10
            terminal = {}
            while time.monotonic() < deadline:
                terminal = json.loads(
                    Path(custody["summary_path"]).read_text(encoding="utf-8")
                )
                if terminal.get("descendants_closed") is True:
                    break
                time.sleep(0.02)
            assert terminal["descendants_closed"] is True
            assert terminal["cancelled"] is True
            assert terminal["launch_id"] == custody["launch_id"]
            assert terminal["child_process"] == startup["child_process"]
            assert (
                startup["child_process"]["pid"] not in memory_guard.sample_processes()
            )
            assert (
                json.loads(receipt.read_text(encoding="utf-8"))["status"] == "failure"
            )
        finally:
            release.write_text("release", encoding="utf-8")
            process.wait(timeout=30)


def test_wasi_c_abi_witness_is_required_and_selects_sdk_compiler():
    policies = {policy.name: policy for policy in PLAN.toolchain_policies}
    command = next(
        command for command in PLAN.commands if command.id == "wasm.test.control-flow"
    )
    assert "wasm.build.host" in command.dependencies
    assert "wasi-clang" in command.toolchains
    assert "tests/test_wasm_longdouble_printf_link.py" in command.argv
    assert command.argv[:3] == ("python3", "tools/venv_exec.py", "python3")
    assert "wasm.build.shared-runtime" in command.dependencies
    assert command.data["timeout_budget"] == "integration"
    assert "main" in command.data["tiers"]
    assert all(item.id != "wasm.execute.c-abi" for item in PLAN.commands)
    workflow = (
        Path(__file__).resolve().parents[1] / ".github/workflows/molt-wasm-ci.yml"
    ).read_text(encoding="utf-8")
    steps = workflow.split("      - ")
    setup_index = next(
        index
        for index, step in enumerate(steps)
        if "uses: ./.github/actions/setup-project\n" in step
    )
    run_index = next(
        index
        for index, step in enumerate(steps)
        if "tools/proof_plan.py --run-family wasm" in step
    )
    assert setup_index < run_index
    assert 'sync-frozen: "true"' in steps[setup_index]
    assert "sync-groups: source-build-numpy" in steps[setup_index]
    assert policies["wasi-clang"].data["wasi_sdk_tool"] == "clang"
    assert policies["wasi-clang"].identity_kind == "executable"
    assert policies["clang"].data["setup_value"] == "22.1.8"


def test_wasm_execution_uses_selected_host_from_its_completed_build():
    commands = {command.id: command for command in PLAN.commands}
    for name in ("hello", "comprehension", "sieve"):
        execute = commands[f"wasm.run.{name}"]
        compile = commands[f"wasm.compile.{name}"]
        assert compile.id in execute.dependencies
        assert "wasm.build.host" in execute.dependencies
        assert "wasm.build.host" not in compile.dependencies
        order = [
            command.id
            for command in proof_plan._topological_commands(PLAN, command_id=execute.id)
        ]
        assert order.index("wasm.build.host") < order.index(execute.id)
        assert order.index(compile.id) < order.index(execute.id)
        assert execute.argv == (
            "python3",
            "tools/venv_exec.py",
            "python3",
            "tools/run_wasm_host.py",
            "--cargo-profile",
            "dev-fast",
            "--",
            f"/tmp/molt-wasm-ci/{name}/manifest.json",
        )
        assert execute.toolchains == ("python",)


def test_wasi_compiler_fingerprint_bypasses_native_path_and_binds_helpers(
    tmp_path, monkeypatch
):
    from molt import llvm_toolchain, wasi_sdk_identity

    compiler = tmp_path / "clang"
    compiler.write_bytes(b"selected SDK compiler")
    policy = next(
        policy for policy in PLAN.toolchain_policies if policy.name == "wasi-clang"
    )
    selected = []

    def resolve(root, role, *, environ):
        selected.append(role)
        return compiler

    monkeypatch.setattr(llvm_toolchain, "resolve_wasi_sdk_tool", resolve)
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        proof_plan,
        which=lambda name: pytest.fail("native PATH consulted"),
    )
    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        proof_plan,
        run=lambda argv, **kwargs: proof_plan.subprocess.CompletedProcess(
            argv, 0, "clang version 23.1.0", ""
        ),
    )
    closure = {
        "resources": "first",
        "process_images": [
            {
                "path": str(compiler),
                "sha256": hashlib.sha256(compiler.read_bytes()).hexdigest(),
            },
        ],
    }
    monkeypatch.setattr(
        llvm_toolchain, "capture_wasi_sdk_selection", lambda **kwargs: closure
    )
    monkeypatch.setattr(
        wasi_sdk_identity,
        "capture_wasi_sdk_tool_files",
        lambda selected: selected["process_images"],
    )
    open_file = Path.open

    def no_duplicate_image_read(path, *args, **kwargs):
        assert path != compiler, "SDK capture's compiler identity was hashed again"
        return open_file(path, *args, **kwargs)

    with monkeypatch.context() as io_scope:
        io_scope.setattr(Path, "open", no_duplicate_image_read)
        first = proof_plan._version_fingerprint(policy)
        closure["resources"] = "changed helper"
        second = proof_plan._version_fingerprint(policy)
        assert first["path"] == str(compiler) == second["path"]
        assert first["identity_sha256"] != second["identity_sha256"]
        assert selected == ["clang", "clang"]


def test_ninja_identity_binds_locked_release_and_observed_distribution_banner(
    tmp_path, monkeypatch
):
    import tomllib

    policy = next(
        policy for policy in PLAN.toolchain_policies if policy.name == "ninja"
    )
    root = Path(__file__).resolve().parents[1]
    package = next(
        item
        for item in tomllib.loads((root / "uv.lock").read_text(encoding="utf-8"))[
            "package"
        ]
        if item["name"] == "ninja"
    )
    assert package["version"] == policy.data["setup_value"] == "1.13.2"
    assert any(
        wheel["hash"]
        == "sha256:fd82e26c0706ad4ab88e5fdd26f3fab0a987a90f810160f6c322e752c6af298b"
        for wheel in package["wheels"]
    )
    pattern = str(policy.data["version_pattern"])
    observed = "1.13.2"
    assert re.fullmatch(pattern, observed)
    for rejected in (
        "1.13.1.git.kitware.jobserver-pipe-1",
        "1.13.0",
        "1.13.21",
        "1.13.2.git.unowned",
        "1.13.2.git.kitware.jobserver-pipe-2",
    ):
        assert not re.fullmatch(pattern, rejected)
    executable = tmp_path / "ninja"
    executable.write_bytes(b"pinned ninja distribution image")
    install_module_view(
        monkeypatch, "shutil", shutil, proof_plan, which=lambda name: str(executable)
    )
    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        proof_plan,
        run=lambda argv, **kwargs: proof_plan.subprocess.CompletedProcess(
            argv, 0, observed + "\n", ""
        ),
    )
    fingerprint = proof_plan._version_fingerprint(policy)
    assert fingerprint["version"] == observed
    assert (
        fingerprint["executable_sha256"]
        == hashlib.sha256(executable.read_bytes()).hexdigest()
    )


def test_receipt_verdict_binds_sdk_closure_without_changing_native_hashes(tmp_path):
    command = next(item for item in PLAN.commands if item.id == "python.static.ty")
    for sibling in PLAN.commands:
        if sibling.family == command.family and sibling.id != command.id:
            (tmp_path / f"{sibling.id}.json").write_text(
                json.dumps(_receipt_for(sibling, tmp_path)), encoding="utf-8"
            )
    receipt = _receipt_for(command, tmp_path)
    policy = next(item for item in PLAN.toolchain_policies if item.name == "wasi-clang")
    sdk = {
        "path": "/sdk/bin/clang",
        "launcher_path": "/sdk/bin/clang-23",
        "launcher_sha256": "1" * 64,
        "content_path": "/sdk/bin/clang-23",
        "executable_sha256": "1" * 64,
        "version": "clang version 23.1.0",
        "probe_cwd": ".",
        "version_pattern": policy.data["version_pattern"],
        "wasi_sdk_sha256": "2" * 64,
    }
    # Independent wire oracle: native identities retain their old seven fields;
    # this selected SDK role binds its complete captured closure as field eight.
    sdk["identity_sha256"] = hashlib.sha256(
        (
            "/sdk/bin/clang\0/sdk/bin/clang-23\0"
            + "1" * 64
            + "\0/sdk/bin/clang-23\0"
            + "1" * 64
            + "\0clang version 23.1.0\0.\0"
            + "2" * 64
        ).encode()
    ).hexdigest()
    receipt["toolchains"]["wasi-clang"] = sdk
    path = tmp_path / "sdk.json"
    path.write_text(json.dumps(receipt), encoding="utf-8")
    assert proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path) == []
    sdk["wasi_sdk_sha256"] = "3" * 64
    path.write_text(json.dumps(receipt), encoding="utf-8")
    assert any(
        "wasi-clang toolchain identity hash is invalid" in error
        for error in proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    )
    del sdk["wasi_sdk_sha256"]
    path.write_text(json.dumps(receipt), encoding="utf-8")
    assert any(
        "invalid wasi-clang toolchain identity" in error
        for error in proof_plan.verify_receipts(PLAN, ["python_static"], tmp_path)
    )


def test_selected_sdk_fingerprint_roundtrips_through_actual_receipt_receiver(
    tmp_path, monkeypatch
):
    from tests.runtime_build_identity_helper import (
        RuntimeFixtureRoot,
        provisioned_wasi_sdk_fixture,
    )

    installation = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    with monkeypatch.context() as selected:
        selected.setenv("WASI_SDK_PATH", str(installation.sdk))
        selected.setattr(
            proof_plan.subprocess,
            "run",
            lambda argv, **kwargs: proof_plan.subprocess.CompletedProcess(
                argv, 0, "clang version 23.1.0", ""
            ),
        )
        sdk = proof_plan.toolchain_fingerprints(PLAN, ("wasi-clang",))["wasi-clang"]
    command = next(item for item in PLAN.commands if item.id == "python.static.ty")
    receipt_root = tmp_path / "receipts"
    receipt_root.mkdir()
    for sibling in PLAN.commands:
        if sibling.family == command.family and sibling.id != command.id:
            (receipt_root / f"{sibling.id}.json").write_text(
                json.dumps(_receipt_for(sibling, receipt_root)), encoding="utf-8"
            )
    receipt = _receipt_for(command, receipt_root)
    receipt["toolchains"]["wasi-clang"] = sdk
    path = receipt_root / "sdk.json"
    path.write_text(json.dumps(receipt), encoding="utf-8")
    assert proof_plan.verify_receipts(PLAN, ["python_static"], receipt_root) == []
    native_name = next(name for name in receipt["toolchains"] if name != "wasi-clang")
    native = receipt["toolchains"][native_name]
    native["wasi_sdk_sha256"] = sdk["wasi_sdk_sha256"]
    # Even a correctly sealed extended digest must not change the native contract.
    native["identity_sha256"] = hashlib.sha256(
        "\0".join(
            str(native[key])
            for key in (
                "path",
                "launcher_path",
                "launcher_sha256",
                "content_path",
                "executable_sha256",
                "version",
                "probe_cwd",
                "wasi_sdk_sha256",
            )
        ).encode()
    ).hexdigest()
    path.write_text(json.dumps(receipt), encoding="utf-8")
    assert any(
        f"invalid {native_name} toolchain identity" in error
        for error in proof_plan.verify_receipts(PLAN, ["python_static"], receipt_root)
    )


def test_wasm_runtime_and_host_prerequisites_follow_actual_consumers():
    commands = {command.id: command for command in PLAN.commands}
    for name in ("wasm.build.shared-runtime", "wasm.build.split-runtime-release"):
        command = commands[name]
        assert command.dependencies == ()
        assert set(command.toolchains) == {
            "python",
            "uv",
            "rustc",
            "cargo",
            "wasm-ld",
            "wasm-tools",
            "wasi-clang",
        }
    assert commands["wasm.build.host"].dependencies == ()
    assert set(commands["wasm.build.host"].toolchains) == {"rustc", "cargo"}
    for name in (
        "wasm.compile.hello",
        "wasm.compile.comprehension",
        "wasm.compile.sieve",
        "wasm.test.freestanding-e2e",
    ):
        selected = {
            command.id
            for command in proof_plan._topological_commands(PLAN, command_id=name)
        }
        assert "wasm.build.host" not in selected
        assert {name, "wasm.build.backend", "wasm.build.shared-runtime"} <= selected
    split = "wasm.integration.split-runtime"
    assert {
        command.id
        for command in proof_plan._topological_commands(PLAN, command_id=split)
    } == {split, "wasm.build.backend", "wasm.build.split-runtime-release"}
    # The split artifact and browser VFS consumers execute Node, with no native
    # precompile request. The native consumers below retain their host producer.
    for name in (
        "wasm.run.hello",
        "wasm.run.comprehension",
        "wasm.run.sieve",
        "wasm.test.control-flow",
        "wasm.test.finally-pending-observer-parity",
    ):
        assert "wasm.build.host" in commands[name].dependencies
    for name in (
        "wasm.compile.hello",
        "wasm.compile.comprehension",
        "wasm.compile.sieve",
        "wasm.test.control-flow",
        "wasm.test.freestanding-e2e",
        "wasm.test.finally-pending-observer-parity",
        "wasm.integration.split-runtime",
    ):
        assert "wasm.build.backend" in commands[name].dependencies


def test_native_c_obligation_is_declared_only_for_confirmed_c_builders():
    plan = proof_plan.ProofPlan.load()
    declared = {
        row.id for row in plan.commands if proof_plan.cargo_native_c_units(row.data)
    }
    assert declared == {
        "wasm.build.host",
        "rust.clippy.workspace-default",
        "portability.rust.linux-aarch64.clippy-workspace",
        "portability.rust.macos.clippy-workspace",
        "rust.test.default-truth",
        "rust.test.runtime-extension-admission",
        "runtime.cost.candidate",
    }
    for name in ("wasm.build.shared-runtime", "wasm.build.split-runtime-release"):
        row = next(row for row in plan.commands if row.id == name)
        assert proof_plan.cargo_native_c_units(row.data) == ()


@pytest.mark.parametrize(
    "units,tools",
    [
        (["target", "target"], ["cargo", "rustc"]),
        (["everything"], ["cargo", "rustc"]),
        (["target"], ["rustc"]),
        (["host"], ["cargo"]),
    ],
)
def test_native_c_declaration_rejects_unbound_or_duplicate_units(units, tools):
    with pytest.raises(ValueError, match="native C|cargo_native_c_units"):
        proof_plan.cargo_native_c_units(
            {"cargo_native_c_units": units, "toolchains": tools}
        )


@pytest.mark.parametrize("name", ["rustc", "cargo"])
@pytest.mark.parametrize("explicit", [False, True])
def test_rust_fingerprint_keeps_selected_physical_tool_without_rustup(
    tmp_path, monkeypatch, name, explicit
):
    from molt import process_guard

    selected = tmp_path / name
    selected.write_bytes(b"independent physical Rust component")
    selected.chmod(0o755)
    (tmp_path / ("rustup.exe" if os.name == "nt" else "rustup")).write_bytes(
        b"different rustup image"
    )
    for key in ("RUSTC", "CARGO", "CARGO_BUILD_RUSTC"):
        monkeypatch.delenv(key, raising=False)
    if explicit:
        monkeypatch.setenv(
            "CARGO_BUILD_RUSTC" if name == "rustc" else "CARGO", str(selected)
        )
    requests, commands = [], []

    def which(requested):
        requests.append(requested)
        assert requested == (str(selected) if explicit else name)
        return str(selected)

    def run(command, **kwargs):
        commands.append(command)
        assert command[0] == str(selected)
        return proof_plan.subprocess.CompletedProcess(command, 0, name + " 1.99.0", "")

    install_module_view(monkeypatch, "shutil", shutil, proof_plan, which=which)
    install_module_view(monkeypatch, "subprocess", subprocess, proof_plan, run=run)
    monkeypatch.setattr(
        process_guard,
        "run_completed_command",
        lambda *args, **kwargs: pytest.fail("physical component invoked rustup"),
    )
    policy = next(row for row in PLAN.toolchain_policies if row.name == name)
    result = proof_plan._version_fingerprint(policy)
    assert result["content_path"] == str(selected)
    assert (
        result["executable_sha256"] == hashlib.sha256(selected.read_bytes()).hexdigest()
    )
    assert len(commands) == len(requests) == 1


@pytest.mark.parametrize("role", ["clang", "llvm-ar", "wasm-ld"])
def test_sdk_fingerprint_refuses_changed_helper_before_version_probe(
    tmp_path, monkeypatch, role
):
    from tests.runtime_build_identity_helper import (
        RuntimeFixtureRoot,
        provisioned_wasi_sdk_fixture,
    )

    installation = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    fact = installation.tool_fact(role)
    content = installation.sdk / str(fact["content_path"])
    content.write_bytes(content.read_bytes() + b"changed after provisioning")
    monkeypatch.setenv("WASI_SDK_PATH", str(installation.sdk))
    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        proof_plan,
        run=lambda *args, **kwargs: pytest.fail(
            "changed SDK helper reached a version probe"
        ),
    )
    with pytest.raises(
        ValueError, match="helper differs from its provisioned generation"
    ):
        proof_plan.toolchain_fingerprints(PLAN, ("wasi-clang",))


@pytest.mark.parametrize(
    "cli,environment,configured,expected",
    [
        ({"build": {"rustc": "cli"}}, {"RUSTC": "env"}, "config", "cli"),
        ({}, {"RUSTC": "env", "CARGO_BUILD_RUSTC": "cargo-env"}, "config", "env"),
        ({}, {"CARGO_BUILD_RUSTC": "cargo-env"}, "config", "cargo-env"),
        ({}, {"RUSTC": ""}, "config", ""),
        ({}, {}, "config", "config"),
        ({}, {}, None, "default"),
    ],
)
def test_core_cargo_selection_preserves_declared_precedence(
    cli, environment, configured, expected
):
    from molt.rust_toolchain import cargo_selected_value

    assert (
        cargo_selected_value(
            {"build": {"rustc": configured}},
            cli,
            environment,
            ("build", "rustc"),
            ("RUSTC", "CARGO_BUILD_RUSTC"),
            "default",
        )
        == expected
    )
    with pytest.raises(ValueError, match="string-keyed table"):
        cargo_selected_value(
            {"build": {"rustc": "value", 1: "invalid"}},
            {},
            {},
            ("build", "rustc"),
            (),
            "default",
        )


def test_fingerprint_mock_preserves_unrelated_process_sampler_boundary(monkeypatch):
    from tests.process_guard_common import run_custody_subject_process

    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        proof_plan,
        run=lambda *args, **kwargs: pytest.fail("fingerprint probe must remain unused"),
    )
    result = run_custody_subject_process(
        [sys.executable, "-c", "print('independent-process-boundary')"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "independent-process-boundary"
