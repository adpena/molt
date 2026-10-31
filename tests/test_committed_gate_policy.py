"""The committed proof plan's gate selection.

These tests select through the repository's own tools/proof_plan.toml, so the
committed-gate-policy rule runs them whenever a plan section that GateConfig
reads changes (HF-457); the driver's own tests run only when the driver does.
"""

from __future__ import annotations

import shlex
from pathlib import Path, PurePosixPath
import tomllib

import pytest

from molt.process_environment import ProcessEnvironment
from molt import host_capacity
from tools.molt_dev_gates import display
from tools.molt_dev_policy import GateConfig, LandingMode
from tools.proof_plan import CargoTargetAccess

REPO_ROOT = Path(__file__).resolve().parents[1]
COMMITTED_GATES = REPO_ROOT / "tools" / "proof_plan.toml"


def test_committed_gate_manifest_is_valid():
    # The shipped manifest must parse and carry rules + always-gates.
    cfg = GateConfig.load(COMMITTED_GATES)
    assert cfg.always, "the committed manifest must define `always` gates"
    assert cfg.rules, "the committed manifest must define rules"
    names = {r.name for r in cfg.rules}
    # The dev-tooling self-gate must exist (the tool gates its own changes).
    assert "dev-tooling" in names
    assert "backend-native" in names
    assert "backend-llvm" in names
    assert "tir-midend" in names
    assert "wasm-host" in names


# The deps landing of 2026-10-22 changed one CI command and nothing else in
# the plan (security.python.audit -> security.lock-audit).
CI_COMMAND_EDIT = frozenset({("command", "security.lock-audit")})


# This oracle names the producers below as selection inputs. The dependency
# graph derives their selection from their own imports, so an edit to one of
# them selects this file as well (tools/test_dependency_graph.py).
POLICY_ORACLE = "tests/test_committed_gate_policy.py"


def _plan_changes(paths, changes):
    return changes if "tools/proof_plan.toml" in paths else None


def _selected_pytest_targets(
    cfg, paths, changes=CI_COMMAND_EDIT, mode=LandingMode.PRESUBMIT
):
    gates, _ = cfg.select(paths, plan_changes=_plan_changes(paths, changes), mode=mode)
    return {
        argument
        for gate in gates
        if isinstance(gate.command, tuple) and "pytest" in gate.command
        for argument in gate.command
        if argument.startswith("tests/")
    }


@pytest.mark.parametrize(
    "path",
    ["tools/check_changed_clippy.py", "tests/tools/test_check_changed_clippy.py"],
)
def test_clippy_selector_edits_do_not_run_landing_simulations(path):
    cfg = GateConfig.load(COMMITTED_GATES)
    assert _selected_pytest_targets(cfg, [path]) == {
        "tests/tools/test_import_file.py",
        "tests/tools/test_check_changed_clippy.py",
        *([POLICY_ORACLE] if path.startswith("tools/") else []),
    }
    gates, _ = cfg.select([path])
    assert {gate.command for gate in gates} >= set(cfg.always)


@pytest.mark.parametrize(
    "path",
    [
        "src/molt/cargo_workspace.py",
        "src/molt/cargo_execution_policy.py",
        "src/molt/cli/cargo_source_closure.py",
        "config/ci_resource_policy.toml",
        "tools/git_identity.py",
        "tools/command_execution.py",
        "tools/proof_plan.py",
        "tools/toolchain_env.py",
        "tools/import_file.py",
    ],
)
def test_clippy_shared_authorities_retain_selector_consumer(path):
    # A transitive consumer: presubmit runs direct readers only, and postsubmit
    # owns the closure (docs/design/observed_inputs.md).
    cfg = GateConfig.load(COMMITTED_GATES)
    assert "tests/tools/test_check_changed_clippy.py" in _selected_pytest_targets(
        cfg, [path], mode=LandingMode.POSTSUBMIT
    )


def test_cargo_workspace_edits_keep_driver_and_selector_consumers():
    cfg = GateConfig.load(COMMITTED_GATES)
    assert _selected_pytest_targets(cfg, ["src/molt/cargo_workspace.py"]) >= {
        "tests/test_cargo_workspace.py",
        "tests/test_molt_dev.py",
        "tests/tools/test_check_changed_clippy.py",
    }


def test_cargo_source_projection_selects_both_consumer_oracles():
    cfg = GateConfig.load(COMMITTED_GATES)
    selected = _selected_pytest_targets(
        cfg, ["src/molt/cli/cargo_source_closure.py"], mode=LandingMode.POSTSUBMIT
    )
    assert selected >= {
        "tests/tools/test_import_file.py",
        "tests/tools/test_check_changed_clippy.py",
        "tests/cli/test_runtime_source_closure.py",
        "tests/cli/test_cli_cache_fingerprint.py::test_backend_lock_identity_tracks_only_reachable_dependency_content",
        "tests/cli/test_cli_cache_fingerprint.py::test_compiler_lock_dependency_identity_fails_closed",
        "tests/cli/test_compiler_identity.py::test_lock_projection_is_mandatory_without_raw_lock_in_source_paths",
    }
    # The driver reaches the projection through its imports, so the graph
    # selects its suite; the old exclusion was a selection gap.
    assert "tests/test_molt_dev.py" in selected


def test_source_closure_test_edits_do_not_compile_compression_providers():
    cfg = GateConfig.load(COMMITTED_GATES)
    path = "tests/cli/test_runtime_source_closure.py"
    selected = _selected_pytest_targets(cfg, [path])
    assert "tests/tools/test_import_file.py" in selected
    assert "tests/cli/test_runtime_source_closure.py" in selected
    assert "tests/test_molt_dev.py" not in selected
    gates, matched = cfg.select([path])
    assert "maintained-zlib-counters" not in {rule.name for rule in matched}
    assert {gate.command for gate in gates} >= set(cfg.always)


def test_compression_provider_edits_keep_compiled_and_source_oracles():
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, _ = cfg.select(["vendor/rust/zlib-rs/src/inflate.rs"])
    commands = {gate.command for gate in gates}
    assert (
        "cargo test --locked --profile dev-fast -p zlib-rs@0.6.8 --lib "
        "--no-default-features --features rust-allocator,std,ZLIB_DEBUG -- "
        "counter_wrap --nocapture --test-threads=1"
    ) in commands
    assert any(
        "--policy zlib-wasi" in command
        for command in commands
        if isinstance(command, str)
    )
    assert "tests/cli/test_runtime_source_closure.py" in _selected_pytest_targets(
        cfg, ["vendor/rust/zlib-rs/src/inflate.rs"]
    )


def test_clippy_command_policy_edits_retain_selector_tests():
    cfg = GateConfig.load(COMMITTED_GATES)
    assert "tests/tools/test_check_changed_clippy.py" in _selected_pytest_targets(
        cfg,
        ["tools/proof_plan.toml"],
        frozenset({("command", "rust.clippy.workspace-default")}),
    )


def test_scanner_gate_edits_select_direct_owners_without_ci_siblings():
    cfg = GateConfig.load(COMMITTED_GATES)
    paths = [
        "docs/design/foundation/56_dx_buildspeed_tooling.md",
        "tools/check_generator_manifest.py",
        "tools/structural_audit.py",
        "tests/test_generator_manifest.py",
        "tests/test_structural_audit.py",
    ]
    assert _selected_pytest_targets(cfg, paths) == {
        "tests/tools/test_import_file.py",
        "tests/test_generator_manifest.py",
        "tests/test_structural_audit.py",
        "tests/tools/test_structural_audit_records.py",
        # tools/structural_audit.py is a parser producer for these consumers.
        "tests/test_rust_source_scan.py",
        "tests/tools/test_gen_luau_support_matrix.py",
        # Tests that import the audits or read the design document.
        POLICY_ORACLE,
        "tests/test_dx_run_context.py",
        "tests/test_agent_coordination.py",
        "tests/test_check_docs_architecture.py",
        "tests/test_executable_test_support.py",
        "tests/test_memory_guard_wiring.py",
        "tests/test_proof_plan.py",
        "tests/tools/test_check_memory_guard_wiring.py",
        "tests/tools/test_dev_py.py",
        "tests/tools/test_phase_exit_manifest.py",
        "tests/tools/test_release_criterion_receipt.py",
        "tests/tools/test_release_exit_gate.py",
        "tests/tools/test_subprocess_guard_coverage.py",
        "tests/tools/test_verified_subset.py",
    }
    gates, _ = cfg.select(paths)
    # Narrowing the consumer run must retain every independent always gate.
    assert {gate.command for gate in gates} >= set(cfg.always)
    assert "python tools/check_docs_architecture.py" in {gate.command for gate in gates}


# The tests besides the owner that import each scanner (or a tool that does).
_GUARD_AUDIT_READERS = {
    "tests/cli/test_cli_package.py",
    "tests/cli/test_cli_setup_validate.py",
    "tests/cli/test_compiler_source_root.py",
    "tests/cli/test_installed_toolchain_rules.py",
    "tests/test_memory_guard_wiring.py",
    "tests/tools/test_check_memory_guard_wiring.py",
    "tests/tools/test_dev_py.py",
    # The CLI completion smoke test names both guard audits as files.
    "tests/cli/test_cli_smoke.py::test_cli_completion_uses_parser_commands_flags_and_positional_semantics",
}


@pytest.mark.parametrize(
    "source,owner,readers",
    [
        # tools/check_gate_liveness.py imports the encoding gate, so its test
        # reads it at the second hop: postsubmit runs it, presubmit does not.
        ("tools/encoding_gate.py", "tests/tools/test_encoding_gate.py", set()),
        ("tools/check_ratio_direction.py", "tests/tools/test_signed_ratio.py", set()),
        (
            "tools/check_subprocess_guard_coverage.py",
            "tests/tools/test_subprocess_guard_coverage.py",
            _GUARD_AUDIT_READERS
            | {
                "tests/test_memory_guard_wiring.py::test_minimal_guard_environment_audit_has_teeth",
                "tests/test_agent_coordination.py",
                "tests/test_molt_dev_lanes.py",
                "tests/test_proof_plan.py",
                "tests/tools/test_install_git_hooks.py",
                "tests/tools/test_new_agent_task.py",
            },
        ),
        (
            "tools/check_environment_registry.py",
            "tests/tools/test_check_environment_registry.py",
            set(),
        ),
        (
            "tools/gen_cpython_coverage.py",
            "tests/tools/test_gen_cpython_coverage.py",
            set(),
        ),
        (
            "tools/gen_stringprep_tables.py",
            "tests/test_gen_stringprep_tables.py",
            set(),
        ),
        (
            "tools/check_generator_manifest.py",
            "tests/test_generator_manifest.py",
            {
                "tests/test_agent_coordination.py",
                "tests/test_memory_guard_wiring.py",
                "tests/test_proof_plan.py",
                "tests/tools/test_check_memory_guard_wiring.py",
                "tests/tools/test_dev_py.py",
                "tests/tools/test_subprocess_guard_coverage.py",
            },
        ),
    ],
)
def test_scanner_leaf_source_and_test_select_their_owner(source, owner, readers):
    cfg = GateConfig.load(COMMITTED_GATES)
    # A source edit selects its owner and every test that reads it; a test
    # edit selects only itself (and the host-entrypoint import check).
    assert _selected_pytest_targets(cfg, [source]) == {
        "tests/tools/test_import_file.py",
        owner,
        POLICY_ORACLE,
        *readers,
    }
    assert _selected_pytest_targets(cfg, [owner]) == {
        "tests/tools/test_import_file.py",
        owner,
    }


def test_second_hop_readers_run_in_postsubmit_only():
    cfg = GateConfig.load(COMMITTED_GATES)
    reader = "tests/tools/test_check_gate_liveness.py"
    assert reader not in _selected_pytest_targets(cfg, ["tools/encoding_gate.py"])
    assert reader in _selected_pytest_targets(
        cfg, ["tools/encoding_gate.py"], mode=LandingMode.POSTSUBMIT
    )


def test_guard_wiring_audit_changes_select_their_direct_consumers():
    cfg = GateConfig.load(COMMITTED_GATES)
    direct = {
        "tests/tools/test_import_file.py",
        "tests/tools/test_check_memory_guard_wiring.py",
        "tests/test_memory_guard_wiring.py::test_minimal_guard_environment_audit_has_teeth",
    }
    assert (
        _selected_pytest_targets(cfg, ["tests/tools/test_check_memory_guard_wiring.py"])
        == direct
    )
    # The audit's importers read it as well.
    assert _selected_pytest_targets(cfg, ["tools/check_memory_guard_wiring.py"]) == (
        direct | _GUARD_AUDIT_READERS | {POLICY_ORACLE}
    )


@pytest.mark.parametrize(
    "source",
    [
        "tools/structural_audit.py",
        "tools/structural_audit_rust.py",
        "tools/structural_audit_rust_admission.py",
        "tools/structural_audit_rust_domains.py",
        "src/molt/rust_source_scan.py",
    ],
)
def test_structural_audit_producers_select_all_parser_consumers(source):
    cfg = GateConfig.load(COMMITTED_GATES)
    selected = _selected_pytest_targets(cfg, [source], mode=LandingMode.POSTSUBMIT)
    assert selected >= {
        "tests/test_structural_audit.py",
        "tests/tools/test_structural_audit_records.py",
        "tests/test_generator_manifest.py",
        "tests/test_rust_source_scan.py",
        "tests/tools/test_gen_luau_support_matrix.py",
    }


@pytest.mark.parametrize(
    "source",
    [
        "tools/build_control_path.py",
        "tools/harness_memory_guard.py",
        "src/molt/build_state_layout.py",
        "src/molt/memory_guard_paths.py",
        "src/molt/dx.py",
        "tests/test_build_state_layout.py",
    ],
)
def test_build_control_path_producers_select_their_consumer_contract(source):
    cfg = GateConfig.load(COMMITTED_GATES)
    assert "tests/test_build_state_layout.py" in _selected_pytest_targets(cfg, [source])


def test_unicode_source_facts_select_their_complete_pinned_source_owner():
    cfg = GateConfig.load(COMMITTED_GATES)
    assert _selected_pytest_targets(cfg, ["tools/gen_unicode_width.py"]) == {
        "tests/tools/test_import_file.py",
        "tests/test_runtime_build_python.py",
        # The manifest's meta-gate test runs every registered generator.
        "tests/test_generator_manifest.py",
        POLICY_ORACLE,
    }


@pytest.mark.parametrize(
    "owner",
    ["tests/test_structural_audit.py", "tests/tools/test_structural_audit_records.py"],
)
def test_structural_audit_test_edits_do_not_select_sibling_consumers(owner):
    cfg = GateConfig.load(COMMITTED_GATES)
    assert _selected_pytest_targets(cfg, [owner]) == {
        "tests/tools/test_import_file.py",
        owner,
    }


@pytest.mark.parametrize(
    "source",
    [
        "tools/scan_records.py",
        "tools/generators.py",
        "tools/molt_dev_gates.py",
        "tools/molt_dev_common.py",
    ],
)
def test_shared_scan_record_producers_select_all_consumer_owners(source):
    cfg = GateConfig.load(COMMITTED_GATES)
    assert _selected_pytest_targets(cfg, [source]) >= {
        "tests/tools/test_encoding_gate.py",
        "tests/tools/test_signed_ratio.py",
        "tests/tools/test_subprocess_guard_coverage.py",
        "tests/tools/test_check_environment_registry.py",
        "tests/tools/test_gen_cpython_coverage.py",
        "tests/tools/test_structural_audit_records.py",
        "tests/test_structural_audit.py",
        "tests/test_generator_manifest.py",
        "tests/test_gen_stringprep_tables.py",
    }


@pytest.mark.parametrize(
    "source",
    ["tools/generator_io.py", "tools/proof_plan.toml", ".github/workflows/ci.yml"],
)
def test_shared_generator_and_ci_inputs_retain_ci_consumer_closure(source):
    cfg = GateConfig.load(COMMITTED_GATES)
    assert _selected_pytest_targets(cfg, [source]) >= {
        "tests/test_proof_plan.py",
        "tests/test_ci_workflow_topology.py",
        "tests/test_dependabot_config.py",
        "tests/test_ci_cargo_cache.py",
        "tests/test_generator_manifest.py",
        "tests/test_resource_pressure.py",
        "tests/test_translation_validate_roots.py",
        "tests/tools/test_ci_resource_env.py",
        "tests/tools/test_fuzz_compiler_cli.py",
        "tests/tools/test_formal_correspondence_truth.py",
        "tests/tools/test_nightly_prepare.py",
        "tests/tools/test_nightly_profile_feedback.py",
        "tests/tools/test_nightly_runtime_bundle.py",
        "tests/tools/test_nightly_shard_profile.py",
        "tests/tools/test_nightly_sharding.py",
        "tests/tools/test_reproducible_build_truth.py",
    }


def test_proof_plan_test_leaf_runs_complete_file_without_production_consumers():
    cfg = GateConfig.load(COMMITTED_GATES)
    target = "tests/test_proof_plan.py"
    gates, matched = cfg.select([target])
    names = {rule.name for rule in matched}
    assert "proof-plan-tests" in names
    assert (
        not {
            "ci-wiring",
            "source-extension-proof-policy",
            "source-extension-package-sets",
        }
        & names
    )
    assert _selected_pytest_targets(cfg, [target]) == {
        target,
        "tests/tools/test_import_file.py",
    }
    (cohort,) = [
        gate
        for gate in gates
        if isinstance(gate.command, tuple) and target in gate.command
    ]
    assert cohort.command[-1:] == (target,)
    assert "-k" not in cohort.command and "-m" not in cohort.command[3:]
    assert cohort.toolchains == ("python", "uv", "cargo")
    assert {gate.command for gate in gates} >= set(cfg.always)


def test_combined_proof_plan_leaf_and_ci_share_one_complete_cargo_cohort():
    cfg = GateConfig.load(COMMITTED_GATES)
    target = "tests/test_proof_plan.py"
    gates, matched = cfg.select([target, ".github/workflows/ci.yml"])
    assert {"proof-plan-tests", "ci-wiring"} <= {rule.name for rule in matched}
    (cohort,) = [
        gate
        for gate in gates
        if isinstance(gate.command, tuple) and target in gate.command
    ]
    assert cohort.command[-1:] == (target,)
    assert "-k" not in cohort.command and "-m" not in cohort.command[3:]
    assert cohort.toolchains == ("python", "uv", "cargo")
    assert _selected_pytest_targets(cfg, [target, ".github/workflows/ci.yml"]) >= {
        target,
        "tests/test_ci_workflow_topology.py",
        "tests/cli/test_native_toolchain_transfer.py",
        "tests/tools/test_nightly_runtime_bundle.py",
        "tests/tools/test_nightly_prepare.py",
    }


@pytest.mark.parametrize("producer", ["tools/proof_plan.py", "tools/proof_receipts.py"])
def test_proof_plan_producers_keep_complete_cargo_and_extension_consumer_union(
    producer,
):
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([producer])
    assert {"ci-wiring", "source-extension-proof-policy"} <= {
        rule.name for rule in matched
    }
    (cohort,) = [
        gate
        for gate in gates
        if isinstance(gate.command, tuple)
        and "tests/test_proof_plan.py" in gate.command
    ]
    assert cohort.command[-1:] == ("tests/test_proof_plan.py",)
    assert "-k" not in cohort.command and "-m" not in cohort.command[3:]
    assert cohort.toolchains == ("python", "uv", "cargo")
    assert _selected_pytest_targets(cfg, [producer]) >= {
        "tests/test_ci_workflow_topology.py",
        "tests/cli/test_source_extension_link_inputs.py",
        "tests/tools/test_source_extension_proof_family.py",
        "tests/tools/test_target_derived_toolchains.py",
        "tests/tools/test_proof_queue.py",
    }


def test_landing_pytest_preserves_each_actual_owning_envelope():
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, _rules = cfg.select(
        [
            "src/molt/cli/backend_compile.py",
            "runtime/molt-runtime/src/builtins/exceptions.rs",
        ]
    )
    gates = [gate.command for gate in gates]
    # The rule's scoped cohort keeps its envelope; a direct reader may also run
    # whole in presubmit, since a narrower rule scope does not absorb it.
    import_run = next(
        gate
        for gate in gates
        if isinstance(gate, tuple)
        and "tests/cli/test_cli_import_collection.py" in gate
        and "-k" in gate
    )
    assert "--molt-worker-memory-gb=6.25" in import_run
    assert import_run[import_run.index("-k") + 1].startswith("backend_codegen or ")
    light = next(
        gate
        for gate in gates
        if isinstance(gate, tuple) and "tests/test_table_drift_gate.py" in gate
    )
    # Equal-tool targets conservatively share their largest requirement.
    # Read declared units independently; do not ask the selection's own index.
    declarations = tomllib.loads(COMMITTED_GATES.read_text(encoding="utf-8"))
    covering: dict[str, float] = {}
    descendants: dict[str, float] = {}
    for row in declarations["command"]:
        if "pytest_worker_memory_gb" not in row:
            continue
        unit = float(row["pytest_worker_memory_gb"])
        for target in row.get("pytest", ()):
            path, node, _ = target.partition("::")
            descendants[path] = max(descendants.get(path, 0.0), unit)
            if not node:
                covering[path] = max(covering.get(path, 0.0), unit)
    envelopes = []
    for target in (arg for arg in light if arg.startswith("tests/")):
        # This committed consumer cohort requests whole files. A named-node
        # migration needs a new independent envelope oracle at that boundary.
        assert "::" not in target and target.endswith(".py")
        path = PurePosixPath(target)
        inherited = [
            covering[str(parent)]
            for parent in (path, *path.parents)
            if str(parent) in covering
        ]
        envelopes.append(
            max(
                max(inherited)
                if inherited
                else host_capacity.DEFAULT_PYTEST_WORKER_MEMORY_GB,
                descendants.get(target, 0.0),
            )
        )
    assert f"--molt-worker-memory-gb={max(envelopes)}" in light
    assert "tests/test_resource_enforcement_e2e.py" not in light
    assert "tests/test_wasm_runtime_heavy_regressions.py" not in light
    assert "-k" not in light and "-m" not in light[3:]


def _gate_args(gate):
    return shlex.split(gate) if isinstance(gate, str) else gate


@pytest.mark.parametrize(
    "path",
    [
        "tools/proof_plan.toml",
        "tools/proof_plan.py",
        "tools/proof_receipts.py",
        "tools/proof_queue_pkg/command_admission.py",
        "tools/proof_queue_pkg/command_identity.py",
        "tools/proof_queue_pkg/execution_environment.py",
        "tools/proof_queue_pkg/target_derived_toolchains.py",
        "tools/proof_queue_pkg/toolchain_capture.py",
        "tools/proof_queue_pkg/process_image_capture.py",
        "tools/proof_queue_pkg/python_custody_bootstrap.py",
        "tools/proof_queue_pkg/cli.py",
        "tools/proof_queue_pkg/pact.py",
        "tests/tools/test_source_extension_proof_family.py",
        "tests/tools/test_target_derived_toolchains.py",
        "tests/tools/test_python_custody_bootstrap.py",
    ],
)
def test_proof_policy_edits_select_receivers_without_package_production(path):
    cfg = GateConfig.load(COMMITTED_GATES)
    # A plan edit reaches these receivers through the toolchain policy they read.
    gates, matched = cfg.select(
        [path],
        plan_changes=_plan_changes(
            [path], frozenset({("toolchain_policy", "source-extension")})
        ),
    )
    gates = [gate.command for gate in gates]
    names = {rule.name for rule in matched}
    assert "source-extension-proof-policy" in names
    assert "source-extension-package-sets" not in names
    arguments = [_gate_args(gate) for gate in gates]
    selected = {argument for argv in arguments for argument in argv}
    # These mixed consumers include real proof-environment and Pact-oracle
    # reads; dropping all source-extension tests would miss those boundaries.
    assert {
        "tests/cli/test_source_extension_link_inputs.py",
        "tests/tools/test_source_extension_proof_family.py",
        "tests/tools/test_target_derived_toolchains.py",
        "tests/tools/test_python_custody_bootstrap.py",
        "tests/tools/test_scientific_stack_versions.py",
    } <= selected
    assert "tests/cli/test_source_extension_producer.py" not in selected
    assert "tests/cli/test_source_extension_compiler_inputs.py" not in selected
    # The rule keeps its scoped queue cohort. A direct reader of the edited path
    # also runs whole in presubmit: a narrower rule scope does not absorb it.
    queue = next(
        argv
        for argv in arguments
        if "tests/tools/test_proof_queue.py" in argv and "-k" in argv
    )
    assert set(queue[queue.index("-k") + 1].split(" or ")) == {
        "proof_queue_pact_witness",
        "proof_queue_rejects_scipy",
        "proof_queue_requires_explicit_scipy",
        "rejects_expected_extension_identity_drift",
    }


@pytest.mark.parametrize(
    "path",
    [
        "config/source_extension_package_sets.toml",
        "config/scientific_stack_versions.toml",
        "src/molt/cli/source_extension_set_registry.py",
        "src/molt/cli/source_extension_invocation.py",
        "src/molt/cli/source_extension_producer.py",
        "src/molt/source_build_environment.py",
        "src/molt/source_build_environment_schema.py",
        "src/molt/source_extension_link_inputs.py",
        "tests/cli/test_source_build_environment_probe.py",
        "tests/tools/test_scientific_stack_versions.py",
    ],
)
def test_producer_edits_retain_package_and_proof_boundary_checks(path):
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    names = {rule.name for rule in matched}
    assert {"source-extension-package-sets", "source-extension-proof-policy"} <= names
    selected = {argument for gate in gates for argument in _gate_args(gate)}
    assert {
        "tests/cli/test_source_extension_producer.py",
        "tests/cli/test_source_extension_invocation.py",
        "tests/cli/test_source_extension_compiler_inputs.py",
        "tests/cli/test_source_extension_link_inputs.py",
        "tests/tools/test_source_extension_proof_family.py",
        "tests/tools/test_proof_queue.py",
    } <= selected
    assert len(gates) == len(set(gates))


def test_mixed_rust_python_change_checks_rust_before_consumer_cohorts():
    cfg = GateConfig.load(COMMITTED_GATES)
    selected, _ = cfg.select(
        [
            "runtime/molt-backend/src/backend_process/input/mod.rs",
            "src/molt/cli/backend_execution.py",
        ]
    )
    commands = [gate.command for gate in selected]
    rust = next(
        i
        for i, command in enumerate(commands)
        if "tools/check_changed_clippy.py" in command
    )
    consumers = [
        i
        for i, command in enumerate(commands)
        if isinstance(command, tuple) and "pytest" in _gate_args(command)
    ]
    assert consumers and all(rust < index for index in consumers)


def test_every_producer_trigger_also_selects_its_proof_boundary():
    cfg = GateConfig.load(COMMITTED_GATES)
    rules = {rule.name: rule for rule in cfg.rules}
    # Both actions depend on these inputs. A new producer trigger must not
    # silently leave its proof-admission consumer behind.
    assert set(rules["source-extension-package-sets"].globs) <= set(
        rules["source-extension-proof-policy"].globs
    )


@pytest.mark.parametrize(
    "path",
    [
        "runtime/molt-ir/src/tir/ops.rs",
        "runtime/molt-passes/src/tir/passes/effects.rs",
        "runtime/molt-passes/src/tir/lower_to_simple.rs",
        "runtime/molt-tir/src/representation_plan.rs",
        "runtime/molt-backend-wasm/src/wasm/final_functions.rs",
        "runtime/molt-backend-wasm/Cargo.toml",
    ],
)
def test_committed_gate_manifest_selects_tir_midend_ratchet(path):
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    if not path.startswith("runtime/molt-backend-wasm/"):
        # tests/test_gen_op_kinds.py reads TIR pass and verifier sources.
        reads_op_kind_sources = path.startswith(
            ("runtime/molt-passes/src/tir/", "runtime/molt-tir/src/tir/")
        )
        assert [r.name for r in matched] == [
            *(["op-kind-registry"] if reads_op_kind_sources else []),
            "rust-clippy-changed",
            "tir-midend",
        ]
    rule = next(rule for rule in matched if rule.name == "tir-midend")
    assert set(rule.gates) <= set(gates)
    cargo = [shlex.split(gate) for gate in rule.gates]
    assert len(cargo) == 2
    assert [command[1] for command in cargo] == ["clippy", "test"]
    for command in cargo:
        packages = {command[i + 1] for i, token in enumerate(command) if token == "-p"}
        assert packages == {"molt-ir", "molt-passes", "molt-tir", "molt-backend-wasm"}
        assert "--locked" in command and "--all-features" in command
        assert command[command.index("--profile") + 1] == "dev-fast"
    assert "--all-targets" in cargo[0]
    assert cargo[0][2:4] == ["--config", 'build.warnings="deny"']
    assert "--" not in cargo[0]
    assert "--no-fail-fast" in cargo[1]


@pytest.mark.parametrize(
    "path",
    [
        "runtime/molt-backend-native/src/lib.rs",
        "runtime/molt-backend-native/src/native_backend/function_compiler.rs",
        "runtime/molt-backend-native/src/simple_backend/module_metadata.rs",
        "runtime/molt-backend-native/Cargo.toml",
        "runtime/molt-backend/src/lib.rs",
    ],
)
def test_committed_native_policy_includes_owning_crate(path):
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    rule = next(rule for rule in matched if rule.name == "backend-native")
    (tests,) = [shlex.split(gate) for gate in rule.gates]
    packages = {tests[i + 1] for i, token in enumerate(tests) if token == "-p"}
    assert packages == {
        "molt-backend",
        "molt-backend-native",
        "molt-artifact-publish",
    }
    assert "--locked" in tests
    assert set(tests[tests.index("--features") + 1].split(",")) == {
        "molt-backend/native-backend",
        "molt-backend/wasm-backend",
    }
    assert "--test" not in tests
    assert (
        "python tools/check_changed_clippy.py --base {integration_upstream_commit}"
        in gates
    )
    assert tests[1] == "test" and "--lib" in tests and "--no-fail-fast" in tests
    assert tests[tests.index("--profile") + 1] == "release-fast"


@pytest.mark.parametrize(
    "path",
    [
        "runtime/molt-wasm-facts/src/publication.rs",
        "runtime/molt-wasm-facts/Cargo.toml",
        "tools/wasm_link_fact_provider.py",
    ],
)
def test_committed_facts_policy_excludes_compiler_codegen(path):
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    assert "backend-native" not in {rule.name for rule in matched}
    rule = next(rule for rule in matched if rule.name == "wasm-facts-host-tool")
    tests, provider = [shlex.split(gate) for gate in rule.gates]
    assert {tests[i + 1] for i, token in enumerate(tests) if token == "-p"} == {
        "molt-wasm-facts",
        "molt-artifact-publish",
        "molt-host-process",
    }
    assert "--locked" in tests
    assert tests[tests.index("--features") + 1] == "molt-wasm-facts/host-tool"
    assert tests[tests.index("--test") + 1] == "wasm_link_facts_cli"
    assert provider == [
        "python",
        "-m",
        "pytest",
        "-q",
        "tests/test_wasm_link_validation.py",
        "-k",
        "rust_facts or wasm_artifact_state",
    ]
    # Rust edits select changed-crate lint cells; the Python bridge reuses the
    # Rust image and exercises its consumers without another lint campaign.
    assert (
        "python tools/check_changed_clippy.py --base {integration_upstream_commit}"
        in gates
    ) == path.startswith("runtime/")
    assert tests[1] == "test" and "--lib" in tests and "--no-fail-fast" in tests
    assert tests[tests.index("--profile") + 1] == "release-fast"


def test_committed_source_checks_precede_complete_native_build_selection():
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(
        [
            "runtime/molt-backend-native/src/native_backend/simple_backend/config.rs",
            "runtime/molt-ir/src/tir/op_kinds.toml",
            "tools/proof_plan.toml",
            "config/source_extension_package_sets.toml",
        ],
        plan_changes=frozenset(
            {("toolchain_policy", "source-extension")} | CI_COMMAND_EDIT
        ),
    )
    gates = [gate.command for gate in gates]
    # These independent families must survive reordering, including every
    # affected backend owner and its complete Cargo test/build requirements.
    source_names = {
        "op-kind-registry",
        "source-extension-package-sets",
        "source-extension-proof-policy",
        "committed-gate-policy",
        "ci-wiring",
    }
    build_names = [
        "backend-native",
        "native-object-emission",
        "backend-luau",
        "tir-midend",
    ]
    lint_gate = (
        "python tools/check_changed_clippy.py --base {integration_upstream_commit}"
    )
    assert {rule.name for rule in matched} == source_names | set(build_names) | {
        "rust-clippy-changed"
    }
    source_rules = [rule for rule in matched if rule.name in source_names]
    build_rules = [rule for rule in matched if rule.name in build_names]
    assert [rule.name for rule in build_rules] == build_names
    source_gates = {gate for rule in source_rules for gate in rule.gates}
    build_gates = [gate for rule in build_rules for gate in rule.gates]
    assert source_gates <= set(gates)
    assert set(build_gates) <= set(gates)
    assert gates[: len(cfg.always)] == cfg.always
    # Clippy fails a lint after the source checks and before any build.
    assert max(gates.index(gate) for gate in source_gates) < gates.index(lint_gate)
    assert gates.index(lint_gate) < min(gates.index(gate) for gate in build_gates)
    assert [gate for gate in gates if gate in build_gates] == build_gates
    consumers = [gate for gate in gates if isinstance(gate, tuple)]
    assert consumers
    assert max(gates.index(gate) for gate in build_gates) < min(
        gates.index(gate) for gate in consumers
    )
    assert len(gates) == len(set(gates))


def test_committed_gate_manifest_selects_molt_ir_op_kind_registry():
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(["tools/op_kinds/render_rust.py"])
    gates = [gate.command for gate in gates]

    # Every host script change also proves its import identity.
    assert [r.name for r in matched] == ["op-kind-registry", "host-entrypoint-imports"]
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_op_kinds.py --check" not in gates
    assert "python tools/audit_op_kinds.py --check" in gates

    gates, matched = cfg.select(["runtime/molt-ir/src/tir/op_kinds.toml"])
    gates = [gate.command for gate in gates]
    assert [r.name for r in matched] == [
        "op-kind-registry",
        "rust-clippy-changed",
        "backend-luau",
        "tir-midend",
    ]
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_op_kinds.py --check" not in gates
    assert "python tools/audit_op_kinds.py --check" in gates


def test_committed_gate_manifest_selects_backend_native_llvm():
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(
        [
            "runtime/molt-backend-native/src/llvm_backend/lowering/preserved_ops/direct_ops.rs"
        ]
    )
    gates = [gate.command for gate in gates]

    assert [r.name for r in matched] == [
        "op-kind-registry",
        "rust-clippy-changed",
        "backend-llvm",
    ]
    assert (
        "cargo test --locked --profile release-fast -p molt-backend-native"
        " -p molt-backend --features molt-backend/native-backend,molt-backend/llvm"
        " --lib --test ir_contract_validation --no-fail-fast" in gates
    )
    assert "python tools/audit_op_kinds.py --check" in gates


@pytest.mark.parametrize(
    "path",
    [
        "runtime/molt-stdlib-text/src/codec_registry.rs",
        "runtime/molt-stdlib-text/src/codec_aliases_generated.rs",
        "runtime/molt-stdlib-text/src/charmap_codecs_generated.rs",
        "runtime/molt-runtime/src/object/ops_encoding.rs",
        "src/molt/stdlib/encodings/cp1252.py",
        "config/codec_aliases.toml",
        "tools/gen_codecs.py",
    ],
)
def test_codec_table_inputs_select_their_generator_oracle(path):
    # tests/test_gen_codecs.py reads every one of these through the generator
    # it imports; the dependency graph selects it once, with no hand rule.
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    assert "codec-tables" not in {rule.name for rule in cfg.rules}
    assert ("rust-clippy-changed" in {r.name for r in matched}) == path.endswith(".rs")
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_codecs.py --check" not in gates
    assert (
        sum(
            isinstance(gate, tuple) and "tests/test_gen_codecs.py" in gate
            for gate in gates
        )
        == 1
    )


def test_committed_gate_manifest_selects_wasm_host_ratchet():
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(["runtime/molt-wasm-host/src/main.rs"])
    gates = [gate.command for gate in gates]

    assert [r.name for r in matched] == [
        "rust-clippy-changed",
        "wasm-host",
    ]
    # Clippy for every reached crate belongs to the changed-crate gate.
    assert (
        "python tools/check_changed_clippy.py --base {integration_upstream_commit}"
        in gates
    )
    assert "cargo build --locked --profile release-fast -p molt-wasm-host" in gates


def test_land_precheck_selects_only_gates_that_read_the_candidate():
    # The committed policy, for a frontend source and a runtime source.
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, _rules = cfg.select(
        [
            "src/molt/cli/backend_compile.py",
            "runtime/molt-runtime/src/builtins/exceptions.rs",
        ]
    )
    shown = {display(gate.command): gate for gate in gates}
    pre = {name for name, gate in shown.items() if gate.precheck}

    def runs_pytest(gate) -> bool:
        return "pytest" in _gate_args(gate.command)

    # Every always gate and the frontend type check read files only.
    assert {display(command) for command in cfg.always} <= pre
    assert "python -m ty check src" in pre
    # A pytest session, a Cargo build or a compiled-program gate waits for the
    # queue's batch, and every gate that the precheck skips is one of those.
    for name, gate in shown.items():
        builds = "cargo" in gate.toolchains
        if runs_pytest(gate) or builds:
            assert name not in pre, name
        else:
            assert name in pre or not set(gate.toolchains) <= {
                "python",
                "uv",
                "git",
                "generators",
                "rustfmt",
            }, name
    assert any(runs_pytest(gate) for gate in shown.values())
    assert any("cargo" in gate.toolchains for gate in shown.values())


def test_transitive_cargo_metadata_availability_adds_no_build_requirement():
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, _ = cfg.select(["docs/agent/V1_HANDOFF_FINDINGS.md"])
    # This is the real ledger-only policy, including its generator dependency.
    assert ("python", "git", "generators") in [gate.toolchains for gate in gates]
    from tools.molt_dev_policy import GateEnvironment

    environments = {
        gate.toolchains: GateEnvironment(ProcessEnvironment({}), None, False)
        for gate in gates
    }
    # The policy gates reach Cargo only through toolchain closure; none writes.
    policy_gates = [gate for gate in gates if not gate.pytest]
    assert cfg.cargo_outputs(environments, policy_gates) == {}
    # The ledger's readers run in their cells, and only a cell that declares
    # Cargo carries Cargo's target effect.
    for gate in gates:
        if gate.pytest and gate.cargo_target_access is not CargoTargetAccess.NONE:
            assert "cargo" in gate.toolchains


def test_a_ci_command_edit_skips_the_driver_and_source_extension_suites():
    # HF-457: the 2026-10-22 deps landing changed one CI command, and the plan
    # glob ran the dev-tooling and source-extension suites, 278 s of its gates.
    cfg = GateConfig.load(COMMITTED_GATES)
    _gates, matched = cfg.select(
        ["tools/proof_plan.toml"], plan_changes=CI_COMMAND_EDIT
    )
    names = {rule.name for rule in matched}
    assert {"ci-wiring", "committed-gate-policy"} <= names
    assert not names & {"dev-tooling", "source-extension-proof-policy"}


def test_an_edited_rule_selects_itself():
    cfg = GateConfig.load(COMMITTED_GATES)
    _gates, matched = cfg.select(
        ["tools/proof_plan.toml"], plan_changes=frozenset({("rule", "dev-tooling")})
    )
    assert "dev-tooling" in {rule.name for rule in matched}


def test_a_plan_edit_without_its_changes_is_refused():
    from tools.driver_errors import DriverError  # noqa: E402

    cfg = GateConfig.load(COMMITTED_GATES)
    with pytest.raises(DriverError, match="changed entries are unknown"):
        cfg.select(["tools/proof_plan.toml"])


def test_an_unreadable_plan_change_selects_every_rule():
    from tools.molt_dev_policy import EVERY_PLAN_ENTITY

    cfg = GateConfig.load(COMMITTED_GATES)
    _gates, matched = cfg.select(
        ["tools/proof_plan.toml"], plan_changes=EVERY_PLAN_ENTITY
    )
    assert {rule.name for rule in matched} == {rule.name for rule in cfg.rules}


def test_every_plan_section_has_a_consumer_and_no_rule_globs_the_plan():
    import tomllib

    from tools.proof_plan import plan_policy_errors

    data = tomllib.loads(COMMITTED_GATES.read_text(encoding="utf-8"))
    assert plan_policy_errors(data) == []
    bare = {**data, "rule": [r for r in data["rule"] if r["name"] != "ci-wiring"]}
    assert any("no local rule claims" in e for e in plan_policy_errors(bare))
    globbed = {
        **data,
        "rule": [
            {**r, "globs": [*r["globs"], "tools/*.toml"]}
            if r["name"] == "dev-tooling"
            else r
            for r in data["rule"]
        ],
    }
    assert any(
        "dev-tooling: claim the plan entries" in e for e in plan_policy_errors(globbed)
    )


def test_a_rule_reorder_selects_the_order_sensitive_consumers():
    # Rule order is gate order: swapping two rules changes no entry, but it
    # must reach the tests that check the committed order.
    import copy
    import tomllib

    from tools.proof_plan import changed_plan_entities

    data = tomllib.loads(COMMITTED_GATES.read_text(encoding="utf-8"))
    swapped = copy.deepcopy(data)
    names = [rule["name"] for rule in swapped["rule"]]
    first, second = names.index("rust-clippy-changed"), names.index("tir-midend")
    swapped["rule"][first], swapped["rule"][second] = (
        swapped["rule"][second],
        swapped["rule"][first],
    )
    changes = changed_plan_entities(data, swapped)
    assert changes == {("rule", None)}
    cfg = GateConfig.load(COMMITTED_GATES)
    _gates, matched = cfg.select(["tools/proof_plan.toml"], plan_changes=changes)
    assert {"committed-gate-policy", "ci-wiring"} <= {rule.name for rule in matched}


@pytest.mark.parametrize(
    "lane",
    [
        "pact.witness.acceptance.native",
        "pact.witness.acceptance.wasm",
        "pact.witness.oracle",
    ],
)
def test_a_pact_lane_edit_reaches_the_source_extension_cohort(lane):
    # The source-extension rule's Pact cohort reads these named lanes.
    cfg = GateConfig.load(COMMITTED_GATES)
    _gates, matched = cfg.select(
        ["tools/proof_plan.toml"], plan_changes=frozenset({("named_lane", lane)})
    )
    assert "source-extension-proof-policy" in {rule.name for rule in matched}


def test_plan_changes_read_the_upstream_plan_through_git():
    import subprocess

    from tools.molt_dev_policy import EVERY_PLAN_ENTITY, _plan_changes

    policy = COMMITTED_GATES.read_text(encoding="utf-8")

    class Git:
        def __init__(self, returncode, stdout):
            self.returncode, self.stdout, self.calls = returncode, stdout, []

        def _run(self, arguments, *, check=True):
            self.calls.append((arguments, check))
            return subprocess.CompletedProcess(
                arguments, self.returncode, self.stdout, ""
            )

    edited = policy.replace(
        'name = "dev-tooling"\n', 'name = "dev-tooling"\n# edited\n', 1
    ).replace('description = "dev drivers,', 'description = "the dev drivers,', 1)
    readable = Git(0, policy)
    assert _plan_changes(readable, "a" * 40, edited) == {("rule", "dev-tooling")}
    assert readable.calls == [(["show", f"{'a' * 40}:tools/proof_plan.toml"], False)]
    # No plan upstream, or one that does not parse: every rule matches.
    assert _plan_changes(Git(128, ""), "a" * 40, policy) is EVERY_PLAN_ENTITY
    assert _plan_changes(Git(0, "not = [toml"), "a" * 40, policy) is EVERY_PLAN_ENTITY
