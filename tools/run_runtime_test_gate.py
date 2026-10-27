#!/usr/bin/env python3
"""Source-bound runtime semantic and allocation-observer proof policies.

Lifecycle tests intentionally mutate process-global publication/shutdown state.
The isolated GIL custody case still launches real concurrent worker threads; its
libtest harness has one test, not a single-threaded runtime. Performance probes
are declared unrun and never credited as semantic passes. Owning tests that
re-execute the runtime image are credited only with their source/image-bound
descendant receipts (tools/runtime_descendant_receipts.py); this gate has no
CPython target-minor coordinate authority. The allocation observer policy builds
both feature images once and runs only its declared controls in fresh processes;
process-global allocator counters and refusal must never share a libtest run.
The WASI lifecycle policy builds separate managed-SDK runtime and Rust-only
platform libtests and hands each exact image to the existing native production-
host integration; Rust owns both semantic rosters.
The WASI Clippy policy checks the declared library packages through the same
managed Cargo operation and Cargo-owned warning denial with human diagnostics;
its guarded exit status is authoritative. It executes no test image.
The native semantic policy also owns the declared core/math numeric libtests in
that same Cargo build; primitive timings are observations, not performance gates.
Cargo build streams go directly to the shared guard's exclusive capture files,
so partial output survives a timeout without terminal rewrites. The artifact
parser consumes complete compiler JSON for artifact-producing policies. Lint
retains human output without interpreting it; guard diagnostics remain separate
from the raw build stderr file.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from collections.abc import Mapping
from pathlib import Path
import sys
import uuid

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from tools import cargo_test_binary_runner as runner  # noqa: E402
from tools import run_cargo_test_truth as truth  # noqa: E402
from tools import runtime_descendant_receipts as descendants  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402
from molt.process_environment import ProcessEnvironment  # noqa: E402
from molt.cargo_execution_policy import admit_cargo_build  # noqa: E402
from molt.dx import proof_scratch_root  # noqa: E402
from molt.exact_json import loads_exact  # noqa: E402
from molt.flag_strings import FlagStringRule  # noqa: E402
from molt.node_runtime import resolve_node_runtime  # noqa: E402
from molt.rust_toolchain import rust_flag_spans  # noqa: E402
from tools.libtest_results import accounting_problem  # noqa: E402
from tools.runtime_test_gate_plan import (  # noqa: E402
    OBSERVER_FEATURE,
    SEMANTIC_IMAGES,
    WASI_TARGET,
    cargo_operations,
    parse_arguments,
)

COMMANDS = CommandExecutor.for_file(__file__)
ISOLATED = (
    "async_rt::sockets::state::socket_runtime_state_tests::cold_socket_and_websocket_retirement_never_starts_services",
    # Runtime restart rebuilds previously exposed static C types. Only a new
    # process can observe their initial rootless READY state and lazy namespace.
    "cpython_abi_hooks::buffer_semantics_tests::builtin_buffer_names_share_cold_runtime_lookup_and_native_readiness",
    "cpython_abi_hooks::pending_cleanup_tests::canonical_cache_allocation_failure_reentry_and_ascii_no_retry",
    "cpython_abi_hooks::immortal_ownership_tests::immortal_c_views_preserve_runtime_roots_and_gc",
    "state::lifecycle::shutdown_tests::embedding_shutdown_collects_native_type_cycles_across_module_retirement",
    "builtins::exceptions::tests::tls_exception_destructor_excludes_concurrent_shutdown",
    "cpython_abi_hooks::tests::gil_custody_recursive_ensure_and_allow_threads_make_progress",
    "state::lifecycle::shutdown_tests::embedding_shutdown_covers_first_pending_callback_atexit_and_live_stdio",
    "state::lifecycle::shutdown_tests::process_exit_covers_collection_before_pending_callbacks_with_or_without_lease",
    "state::runtime_state::tests::concurrent_init_waits_for_ready_publication",
    "state::runtime_state::tests::finalizing_unpublishes_before_atexit_reentry_and_racing_entrants",
    "state::runtime_state::tests::shutdown_refuses_attached_and_detached_foreign_thread_state_owners",
    "concurrency::isolates::bootstrap_failure_tests::completed_isolate_preserves_primary_resource_factory_for_new_workers",
    "state::runtime_state::tests::runtime_policy_freezes_before_first_consumers_and_keeps_absent_vfs_absent",
    "state::runtime_state::tests::runtime_vfs_freezes_route_and_quota_without_loading_payload",
    "state::runtime_state::tests::runtime_relative_vfs_route_keeps_startup_cwd_until_first_use",
    "state::runtime_state::tests::runtime_output_sinks_keep_startup_cwd_until_first_write",
)
# The lost-CWD oracle removes its current directory, which Rust compiles only
# under cfg(unix). This policy executes native host images; WASI has its own
# lifecycle policy and does not use this host partition.
ISOLATED_UNIX = (
    "state::runtime_state::tests::relative_output_sink_capture_failure_disables_without_fallback",
)
PROBES = (
    "builtins::functools::tests::lru_cache_cost_probe",
    "c_api::tests::dictionary_snapshot_materialization_components",
    "cpython_abi_hooks::generic_attributes_tests::mutation_consumers::managed_mutation_projection_profile",
    "builtins::concurrent::tests::lifecycle_cost_probe",
    "cpython_abi_hooks::tests::single_thread_extension_call_preemption_bench",
    "object::gc::tests::generational_scan_reduction_bench",
    "object::gc::tests::repeated_reachable_collection_workspace_bench",
    "test_support::runtime_test_transaction_overhead_probe",
)
# A probe exists only where the runtime source compiles it. The extension
# preemption bench pins the process to one CPU, which the source limits with
# `cfg(any(windows, target_os = "linux"))`; its row is keyed by `sys.platform`.
# A probe absent from this table exists on every host.
PROBE_PLATFORMS: dict[str, frozenset[str]] = {
    "builtins::concurrent::tests::lifecycle_cost_probe": frozenset({"linux", "win32"}),
    "cpython_abi_hooks::tests::single_thread_extension_call_preemption_bench": (
        frozenset({"linux", "win32"})
    ),
}


# These are semantic obligations and isolation policy, not inventory snapshots.
# Cargo owns image paths/features; libtest owns the discovered test identities.
OBSERVER_CASES = {
    "molt_runtime": (
        "builtins::frames::namespace_tests::task_growth_refusal::both_task_producers_refuse_actual_late_map_growth_and_restore_owners",
        "call::bind::inline_cache::binding_tests::direct_call_ic_hot_path_is_allocation_free_with_slow_path_control",
        "cpython_abi_hooks::pending_cleanup_tests::writable_buffer_lease_preparation_failure_precedes_exporter_dispatch",
        "cpython_abi_hooks::tests::cpython_abi_gil_custody_is_allocation_free_after_warmup",
        "object::gc::tests::repeated_reachable_collection_workspace_allocations",
        "cpython_abi_hooks::exception_rendering_tests::decimal_output_limits::rejected_decimal_magnitude_does_not_allocate_decimal_storage",
        "object::ops::unpack_transaction_tests::exact_builtin_unpack_hot_path_performs_zero_allocator_calls",
        "object::ops_slice::tests::owned_range_parts_constructor_and_subscription_transfer_and_retire",
        "regex_bridge::tests::regex_aggregates_retain_each_created_child_once",
        "regex_bridge::tests::regex_capture_aggregate_owns_children_and_rolls_back_every_failure",
        "state::runtime_state::tests::encoded_core_gil_entry_is_allocation_free_after_warmup",
    ),
    "l7_numeric_runtime_perf_attestation": (
        "integer_byte_encoder_runs_with_allocation_denied_after_preparation",
        "prepared_memory_error_transport_and_reuse_attempt_no_rust_or_platform_c_allocation",
    ),
}


OBSERVER_PERFORMANCE_PROBES = {
    "molt_runtime": (
        "builtins::module_table::tests::import_catalog_perf_attestation",
        "object::gc::tests::cycle_capable_registry_hot_path_bench",
    ),
    "l7_numeric_runtime_perf_attestation": (
        "l7_numeric_runtime_performance_attestation",
        "shared_hash_storage_performance_attestation",
    ),
}


def unrun_reason(policy: str, target: str, name: str, platform: str) -> str:
    if policy == "semantic" and target != "molt_runtime":
        return "outside declared satellite numeric controls"
    if (
        policy == "semantic"
        or name in OBSERVER_PERFORMANCE_PROBES[target]
        or (target == "molt_runtime" and name in probes_for(platform))
    ):
        return "performance probe"
    return "outside declared allocation-observer controls"


def image_targets(policy: str) -> dict[str, str]:
    if policy == "semantic":
        return {image.target: image.kind for image in SEMANTIC_IMAGES}
    targets = {"molt_runtime": "rlib"}
    if policy == "allocation-observer":
        targets["l7_numeric_runtime_perf_attestation"] = "test"
    return targets


def observer_selections(
    inventory: list[str], target: str
) -> dict[str, tuple[list[str], set[str]]]:
    if len(inventory) != len(set(inventory)):
        raise RuntimeError("duplicate test discovery identity")
    required = OBSERVER_CASES[target]
    missing = set(required) - set(inventory)
    if missing:
        raise RuntimeError(
            f"{target}: missing required observer cases: {sorted(missing)}"
        )
    return {
        name: (
            ["--exact", name, "--include-ignored", "--test-threads=1", "--nocapture"],
            {name},
        )
        for name in required
    }


def isolated_for(platform: str) -> tuple[str, ...]:
    """Return semantic fresh-process obligations for the native host image."""
    return ISOLATED if platform == "win32" else ISOLATED + ISOLATED_UNIX


def probes_for(platform: str) -> tuple[str, ...]:
    """Return the probes the runtime inventory must carry on ``platform``."""
    return tuple(
        name
        for name in PROBES
        if platform in PROBE_PLATFORMS.get(name, frozenset({platform}))
    )


def selections(
    inventory: list[str], threads: int, platform: str, *, target: str = "molt_runtime"
) -> dict[str, tuple[list[str], set[str]]]:
    if threads < 2:
        raise RuntimeError(
            "parallel runtime remainder requires at least two harness threads"
        )
    if len(inventory) != len(set(inventory)):
        raise RuntimeError("duplicate test discovery identity")
    if target != "molt_runtime":
        image = next((row for row in SEMANTIC_IMAGES if row.target == target), None)
        if image is None or not image.cases:
            raise RuntimeError(f"unknown semantic satellite image: {target}")
        required = set(image.cases)
        if not required <= set(inventory):
            raise RuntimeError(
                f"{target}: missing required satellite cases: {sorted(required - set(inventory))}"
            )
        return {
            "declared": (
                ["--exact", *image.cases, "--test-threads=1", "--nocapture"],
                required,
            )
        }
    isolated = isolated_for(platform)
    probes = probes_for(platform)
    required = set(isolated + probes)
    if not required <= set(inventory):
        raise RuntimeError(
            f"missing required runtime cases: {sorted(required - set(inventory))}"
        )
    remainder = set(inventory) - required
    if not remainder:
        raise RuntimeError("empty parallel runtime remainder")
    parallel = [f"--test-threads={threads}", "--nocapture"]
    for name in isolated + probes:
        # libtest skip filters match substrings: final row-set validation ensures
        # no lateral test accidentally disappears through a matching name.
        parallel.extend(["--skip", name])
    result = {"parallel": (parallel, remainder)}
    result.update(
        {
            name: (
                ["--exact", name, "--ignored", "--test-threads=1", "--nocapture"],
                {name},
            )
            for name in isolated
        }
    )
    return result


def validate_children(
    children: dict[str, list[dict]],
    selection: dict[str, tuple[list[str], set[str]]],
    *,
    source: dict,
    run_id: str,
    binary: Path,
    identity: tuple[int, str],
    require_passed: bool = False,
    invocations: set[str] | None = None,
) -> dict[str, dict[str, object] | None]:
    if set(children) != set(selection):
        raise RuntimeError("aggregate lacks exact complete child ledger")
    if invocations is None:
        invocations = set()
    verified: dict[str, dict[str, object] | None] = {}
    for name, (argv, expected) in selection.items():
        receipts = children[name]
        if len(receipts) != 1:
            raise RuntimeError(f"{name}: expected exactly one child receipt")
        receipt = receipts[0]
        if receipt.get("source_identity") != source or receipt.get("run_id") != run_id:
            raise RuntimeError(f"{name}: source/run custody mismatch")
        invocation = receipt.get("invocation_id")
        if (
            not isinstance(invocation, str)
            or not invocation
            or invocation in invocations
        ):
            raise RuntimeError(f"{name}: duplicate or missing process invocation")
        invocations.add(invocation)
        if (
            receipt.get("executable_size"),
            receipt.get("executable_sha256"),
        ) != identity or receipt.get("executable_resolved") != str(binary.resolve()):
            raise RuntimeError(f"{name}: executable custody mismatch")
        if receipt.get("inherited_args") != argv:
            raise RuntimeError(f"{name}: wrong test selection")
        if require_passed and receipt.get("required_passed_tests") != sorted(expected):
            raise RuntimeError(f"{name}: wrong required-pass contract")
        problem = accounting_problem(receipt)
        if (
            problem
            or receipt.get("status") != "success"
            or receipt.get("returncode") != 0
        ):
            raise RuntimeError(f"{name}: unsuccessful/incomplete child: {problem}")
        rows = receipt["test_results"]
        if (
            len(rows) != len(expected)
            or {row["identity"] for row in rows} != expected
            or any(row["status"] != "pass" for row in rows)
        ):
            raise RuntimeError(f"{name}: exact all-pass test accounting mismatch")
        executions = receipt.get("executions")
        if (
            not isinstance(executions, list)
            or len(executions) != 1
            or executions[0].get("argv") != [str(binary), *argv]
        ):
            raise RuntimeError(
                f"{name}: ambiguous baseline/diagnostic execution custody"
            )
        # Promotion re-opens the full parent and descendant captures itself;
        # the loader's or runner's saved descendant summary is not evidence.
        try:
            verified[name] = descendants.verify_receipt(receipt)
        except descendants.DescendantEvidenceError as exc:
            raise RuntimeError(f"{name}: {exc}") from exc
    return verified


def validate_artifact(
    artifact: dict[str, str],
    target_name: str = "molt_runtime",
    target_kind: str = "rlib",
    *,
    package: str = "molt-runtime",
) -> None:
    # Cargo target.kind follows crate-type: this runtime declares rlib, not lib.
    # Do not normalize arbitrary binary artifacts into the runtime authority.
    if (
        artifact["target_name"] != target_name
        or artifact["target_kind"] != target_kind
        or not artifact["package"].startswith(f"{package}@")
    ):
        raise RuntimeError(
            f"Cargo artifact is not the {target_name}/{target_kind} test authority"
        )


def select_artifacts(
    artifacts: dict[str, dict[str, str]], policy: str
) -> dict[str, dict[str, str]]:
    targets = image_targets(policy)
    packages = (
        {image.target: image.package for image in SEMANTIC_IMAGES}
        if policy == "semantic"
        else dict.fromkeys(targets, "molt-runtime")
    )
    selected = {}
    for artifact in artifacts.values():
        name = artifact["target_name"]
        if name not in targets or name in selected:
            raise RuntimeError(f"unexpected or duplicate Cargo test target: {name}")
        validate_artifact(artifact, name, targets[name], package=packages[name])
        selected[name] = artifact
    if set(selected) != set(targets):
        raise RuntimeError(
            f"missing Cargo test targets: {sorted(set(targets) - set(selected))}"
        )
    return selected


def observed_abi_dependency(
    cargo_output: str,
    metadata_output: str,
    *,
    policy: str,
    source_root: Path,
) -> dict:
    """Bind allocation policy to the actual workspace ABI library compilation.

    Calibration core is intentionally legal in timing/semantic images. Only
    the dedicated allocation-observer feature enables platform C counters.
    Package identity and target shape come from Cargo's manifest metadata,
    never a package-id spelling or an executable's top-level features. This
    proves Cargo feature resolution; effective rustc cfg/config custody remains
    with the existing build/toolchain capture authority. Manually injected cfgs
    are not represented in Cargo compiler-artifact feature lists.
    """
    if policy not in {"semantic", "allocation-observer"}:
        raise RuntimeError(f"unknown ABI allocation policy: {policy}")
    identities = truth.package_identities_from_metadata(metadata_output)
    try:
        metadata = loads_exact(metadata_output)
    except ValueError as exc:
        raise RuntimeError("malformed Cargo package metadata") from exc
    if not isinstance(metadata, dict):
        raise RuntimeError("Cargo package metadata must be an object")
    manifest = (source_root / "runtime/molt-cpython-abi/Cargo.toml").resolve()
    packages = [
        package
        for package in metadata["packages"]
        if isinstance(package.get("manifest_path"), str)
        and Path(package["manifest_path"]).resolve() == manifest
    ]
    if len(packages) != 1:
        raise RuntimeError("requires one canonical ABI package manifest in metadata")
    package = packages[0]
    package_id = package["id"]
    if (
        package.get("source") is not None
        or not Path(package["manifest_path"]).is_absolute()
        or not isinstance(metadata.get("workspace_root"), str)
        or Path(metadata["workspace_root"]).resolve() != source_root.resolve()
        or not isinstance(metadata.get("workspace_members"), list)
        or metadata["workspace_members"].count(package_id) != 1
    ):
        raise RuntimeError("ABI package is not the canonical workspace member")
    targets = package.get("targets")
    if not isinstance(targets, list) or not all(
        isinstance(t, dict) and isinstance(t.get("crate_types"), list) for t in targets
    ):
        raise RuntimeError("ABI package lacks target metadata")
    libraries = [t for t in targets if "rlib" in t.get("crate_types", [])]
    if len(libraries) != 1:
        raise RuntimeError("requires one ABI library target in metadata")
    target = libraries[0]
    if (
        not isinstance(target.get("name"), str)
        or not isinstance(target.get("src_path"), str)
        or Path(target["src_path"]).resolve() != manifest.parent / "src/lib.rs"
        or not isinstance(target.get("kind"), list)
        or not isinstance(target.get("crate_types"), list)
        or not all(
            isinstance(k, str) and k for k in target["kind"] + target["crate_types"]
        )
        or set(target["kind"]) != set(target["crate_types"])
        or set(target["crate_types"]) != {"rlib", "cdylib"}
    ):
        raise RuntimeError("ABI metadata does not describe its canonical library")
    declared = package.get("features")
    if (
        not isinstance(declared, dict)
        or not all(
            isinstance(k, str)
            and isinstance(v, list)
            and all(isinstance(f, str) for f in v)
            for k, v in declared.items()
        )
        or "l7-attestation-core" not in declared
        or "l7-attestation-core" not in declared.get("allocation-observer", [])
    ):
        raise RuntimeError(
            "ABI manifest lacks the separate allocation feature contract"
        )
    matches = []
    for item in truth.compiler_artifacts(cargo_output, source_context="compiler-only"):
        emitted_target = item.get("target")
        if not isinstance(emitted_target, dict):
            raise RuntimeError("Cargo compiler artifact lacks target metadata")
        if item.get("package_id") == package_id:
            if emitted_target.get("kind") == ["custom-build"]:
                continue
            matches.append(item)
        elif emitted_target.get("name") == target["name"]:
            raise RuntimeError("foreign ABI compiler artifact package identity")
    if len(matches) != 1:
        raise RuntimeError("requires one unambiguous emitted ABI library artifact")
    item = matches[0]
    emitted_target = item["target"]
    features, profile, filenames = (
        item.get("features"),
        item.get("profile"),
        item.get("filenames"),
    )
    if (
        item.get("manifest_path") != package["manifest_path"]
        or any(
            emitted_target.get(k) != target[k]
            for k in ("name", "src_path", "kind", "crate_types")
        )
        or item.get("executable") is not None
        or not isinstance(profile, dict)
        or profile.get("test") is not False
        or type(profile.get("debug_assertions")) is not bool
        or not isinstance(profile.get("opt_level"), str)
        or not isinstance(features, list)
        or not all(isinstance(f, str) and f for f in features)
        or len(features) != len(set(features))
        or not set(features) <= declared.keys()
        or not isinstance(filenames, list)
        or not filenames
        or not all(isinstance(f, str) and Path(f).is_absolute() for f in filenames)
        or len(filenames) != len(set(filenames))
        or sum(Path(f).suffix == ".rlib" for f in filenames) != 1
    ):
        raise RuntimeError(
            "ABI compiler artifact lacks exact library/features evidence"
        )
    observed = set(features)
    observer = "allocation-observer" in observed
    if observer != (policy == "allocation-observer") or (
        observer and "l7-attestation-core" not in observed
    ):
        raise RuntimeError(
            f"ABI allocation features contradict {policy} policy: {features}"
        )
    return {
        "package_id": package_id,
        "package": identities[package_id],
        "manifest_path": package["manifest_path"],
        "target": emitted_target,
        "features": features,
        "allocation_feature_contract": {
            name: declared[name]
            for name in ("allocation-observer", "l7-attestation-core")
        },
        "observed_profile": profile,
        "filenames": filenames,
        "metadata_sha256": hashlib.sha256(metadata_output.encode("utf-8")).hexdigest(),
        "compiler_stdout_sha256": hashlib.sha256(
            cargo_output.encode("utf-8")
        ).hexdigest(),
    }


_PROFILE_CHECKS = {
    "dev-fast": ("DEV", True, "correctness"),
    "release-fast": ("RELEASE", False, "iteration"),
    "release-output": ("RELEASE", False, "shipping"),
}
_CHECK_FIELDS = ("debug_assertions", "overflow_checks")


def build_environment(environment: Mapping[str, str]) -> dict[str, str | None]:
    """Retain every selector used to admit the resolved runtime profile."""
    names = {
        "RUSTUP_TOOLCHAIN",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_TARGET_DIR",
    }
    for profile, (parent, _expected, _role) in _PROFILE_CHECKS.items():
        for selector in (parent, profile.upper().replace("-", "_")):
            names.update(
                f"CARGO_PROFILE_{selector}_{field.upper()}" for field in _CHECK_FIELDS
            )
    names.update(
        key
        for key in environment
        if key.startswith("CARGO_TARGET_") and key.endswith("_RUSTFLAGS")
    )
    return {key: environment.get(key) for key in sorted(names)}


def observed_build(
    cargo_output: str,
    artifact: dict[str, str],
    profile_name: str,
    environment: Mapping[str, str | None],
) -> dict:
    matches = []
    for item in truth.compiler_artifacts(cargo_output, source_context="compiler-only"):
        if (
            item.get("executable")
            and str(Path(item["executable"]).resolve()) == artifact["executable"]
        ):
            matches.append(item)
    if len(matches) != 1:
        raise RuntimeError("runtime requires one unambiguous raw compiler artifact")
    item = matches[0]
    profile = item.get("profile")
    features = item.get("features")
    if (
        not isinstance(profile, dict)
        or profile.get("test") is not True
        or type(profile.get("debug_assertions")) is not bool
        or not isinstance(profile.get("opt_level"), str)
        or not isinstance(features, list)
        or not all(isinstance(feature, str) for feature in features)
    ):
        raise RuntimeError("compiler artifact lacks resolved profile/features evidence")
    policy = _PROFILE_CHECKS.get(profile_name)
    if policy is not None:
        parent, expected, _role = policy
        for field in _CHECK_FIELDS:
            if profile.get(field) is not expected:
                raise RuntimeError(
                    f"{profile_name} compiler artifact requires {field}={expected}"
                )
        # rustc -C flags can override Cargo's profile metadata. Fail closed for
        # contradictory or unfamiliar explicit settings in every named tier.
        flag_variables = {
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
        } | {
            key
            for key in environment
            if key.startswith("CARGO_TARGET_") and key.endswith("_RUSTFLAGS")
        }
        for variable in sorted(flag_variables):
            rule = (
                FlagStringRule.CARGO_ENCODED
                if variable == "CARGO_ENCODED_RUSTFLAGS"
                else FlagStringRule.CARGO_RUSTFLAGS
                if variable == "RUSTFLAGS"
                else FlagStringRule.CARGO_LIST
            )
            try:
                spans = tuple(
                    rust_flag_spans(rule.decode(environment.get(variable) or ""))
                )
            except ValueError as exc:
                raise RuntimeError(f"{variable}: {exc}") from exc
            for span in spans:
                if (
                    not expected
                    and span.option == "--cfg"
                    and (span.value or "").partition("=")[0].strip("\"'")
                    == "debug_assertions"
                ):
                    raise RuntimeError(
                        f"{variable} explicitly enables debug_assertions cfg"
                    )
                if span.codegen is not None:
                    key, _separator, value = span.codegen.partition("=")
                    accepted = (
                        {"yes", "on", "true", "1"}
                        if expected
                        else {"no", "off", "false", "0"}
                    )
                    if (
                        key.replace("-", "_") in _CHECK_FIELDS
                        and value.lower() not in accepted
                    ):
                        raise RuntimeError(f"{variable} contradicts {key}={expected}")
        for selector in (parent, profile_name.upper().replace("-", "_")):
            for field in _CHECK_FIELDS:
                variable = f"CARGO_PROFILE_{selector}_{field.upper()}"
                value = environment.get(variable)
                if value is not None and value.lower() not in (
                    {"true", "1"} if expected else {"false", "0"}
                ):
                    raise RuntimeError(f"{variable} contradicts {field}={expected}")
    return {
        "requested_profile": profile_name,
        "profile_role": policy[2] if policy is not None else "custom",
        "observed_profile": profile,
        "features": features,
        "target": item["target"],
    }


FAILURE_NAME_LIMIT = 40
STDERR_TAIL_LINES = 40


def child_failure_summary(
    name: str, returncode: int, receipts: list[dict], stderr: str
) -> str:
    """Name what failed in one child so a red gate is actionable from its log.

    The receipts keep the full record; this bounded summary is what CI prints.
    """
    failing = sorted(
        {
            identity
            for receipt in receipts
            for identity in (
                receipt.get("failure_identities")
                or receipt.get("reported_failures")
                or []
            )
            if isinstance(identity, str)
        }
    )
    terminations = [
        receipt["baseline_termination"]
        for receipt in receipts
        if isinstance(receipt.get("baseline_termination"), dict)
        and receipt["baseline_termination"].get("kind") != "exit"
    ]
    lines = [f"runtime-gate: child {name} failed with exit code {returncode}"]
    if failing:
        shown = failing[:FAILURE_NAME_LIMIT]
        lines.append(f"  {len(failing)} failing test(s):")
        lines.extend(f"    {identity}" for identity in shown)
        if len(failing) > len(shown):
            lines.append(f"    ... {len(failing) - len(shown)} more in the receipts")
    for termination in terminations:
        lines.append(
            f"  abnormal termination: {json.dumps(termination, sort_keys=True)}"
        )
    tail = stderr.splitlines()[-STDERR_TAIL_LINES:]
    if tail:
        lines.append(f"  driver stderr (last {len(tail)} lines):")
        lines.extend(f"    {line}" for line in tail)
    return "\n".join(lines)


def execute_child(
    binary: Path,
    child_args: list[str],
    expected: set[str],
    *,
    directory: Path,
    run_id: str,
    source: dict,
    timeout: float,
    require_passed: bool,
    retained_root: Path,
    environment: Mapping[str, str] | None = None,
) -> tuple[subprocess.CompletedProcess[str], list[dict]]:
    child_command = [
        sys.executable,
        str(ROOT / "tools/cargo_test_binary_runner.py"),
        "--timeout-seconds",
        str(timeout),
        "--receipt-dir",
        str(directory),
        "--retained-root",
        str(retained_root),
        "--run-id",
        run_id,
        "--source-identity-json",
        json.dumps(source),
    ]
    if require_passed:
        for test in sorted(expected):
            child_command.extend(["--require-passed-test", test])
    child_command.extend(["--", str(binary), *child_args])
    child = COMMANDS.run(
        child_command,
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=timeout + 60,
        encoding="utf-8",
        env=environment,
    )
    (directory / "driver.stdout.log").write_text(child.stdout, encoding="utf-8")
    (directory / "driver.stderr.log").write_text(child.stderr, encoding="utf-8")
    receipts = truth.load_binary_receipts(
        directory,
        expected_run_id=run_id,
        expected_source_identity=source,
        retained_root=retained_root,
    )
    return child, receipts


WASI_CONSUMER = "actual_wasi_runtime_lifecycle_cases_through_production_host"
WASI_PLATFORM_CONSUMER = "actual_wasi_platform_errno_cases_through_production_host"


def lifecycle_artifact(
    output: str,
    metadata: str,
    *,
    package: str,
    target: str,
    kind: str,
    test: bool = True,
    manifest: Path | None = None,
) -> dict[str, str]:
    """Select one compiler-produced lifecycle image before any execution."""
    emitted = [
        item
        for item in truth.compiler_artifacts(output, source_context="compiler-only")
        if isinstance(item.get("profile"), dict)
        and item["profile"].get("test") is test
        and item.get("executable") is not None
        and (
            test
            or (
                isinstance(item.get("target"), dict)
                and item["target"].get("name") == target
            )
        )
    ]
    if len(emitted) != 1:
        raise RuntimeError(f"lifecycle build requires exactly one {target} artifact")
    identities = truth.package_identities_from_metadata(metadata)
    records = truth.expected_test_binaries_from_artifacts(emitted, identities)
    artifact = next(iter(records.values()))
    packages = loads_exact(metadata)["packages"]
    canonical = [
        p
        for p in packages
        if p.get("name") == package
        and p.get("manifest_path")
        == str(manifest or ROOT / "runtime" / package / "Cargo.toml")
        and p.get("source") is None
    ]
    if len(canonical) != 1 or emitted[0].get("package_id") != canonical[0]["id"]:
        raise RuntimeError("lifecycle artifact has foreign package authority")
    if artifact["target_name"] != target or artifact["target_kind"] != kind:
        raise RuntimeError("lifecycle artifact has wrong target authority")
    return artifact


def cargo_build(
    command: list[str],
    directory: Path,
    timeout: float,
    environment: Mapping[str, str] | None = None,
    *,
    stdout_capture_path: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    admit_cargo_build(command, cwd=ROOT, env=environment)
    return COMMANDS.run(
        command,
        cwd=ROOT,
        env=environment,
        capture_output=True,
        stdout_capture_path=(
            directory / "build.stdout.jsonl"
            if stdout_capture_path is None
            else stdout_capture_path
        ),
        stderr_capture_path=directory / "build.stderr.log",
        capture_tail_bytes=None,  # Keep full raw output for its owning consumer.
        text=True,
        timeout=timeout,
        check=False,
        encoding="utf-8",
    )


def run_compile_only_lint(args: argparse.Namespace, run: Path, aggregate: dict) -> None:
    from tools.proof_queue_pkg.toolchain_capture import (
        resolve_runtime_gate_cargo_operation,
    )

    source = truth.git_source_identity()
    aggregate["source_identity"] = source
    (operation,) = cargo_operations(
        args.policy, args.profile, cargo=os.environ.get("CARGO", "cargo")
    )
    command, environment, plan = resolve_runtime_gate_cargo_operation(
        operation, cwd=ROOT, env=dict(os.environ), compiler_host=None
    )
    if plan is None:
        raise RuntimeError("WASI lint operation lacks its managed Cargo plan")
    aggregate["source_command"] = list(operation.command)
    aggregate["build_command"] = command
    aggregate["resolved_rustflags"] = list(plan.rustflags)
    build = cargo_build(
        command,
        run,
        args.build_timeout_seconds,
        environment,
        stdout_capture_path=run / "build.stdout.log",
    )
    aggregate["build_returncode"] = build.returncode
    plan.verify()
    if build.returncode:
        raise RuntimeError(f"Cargo lint compilation failed with {build.returncode}")
    if truth.git_source_identity() != source:
        raise RuntimeError("source changed during lint compilation")
    aggregate["status"] = "success"


def run_wasi_lifecycle(args: argparse.Namespace, run: Path, aggregate: dict) -> None:
    # Keep Rust's CASES as the sole semantic roster. The Python policy owns only
    # four compiler images and two native integration consumers. Rust owns the
    # runtime nine and platform three semantic laws, never Python case copies.
    from tools.proof_queue_pkg.toolchain_capture import (
        resolve_runtime_gate_cargo_operation,
    )

    source = truth.git_source_identity()
    aggregate["source_identity"] = source
    host_target = truth.host_target()
    aggregate["host_target"] = host_target
    cargo = os.environ.get("CARGO", "cargo")
    aggregate["unrun_scope"] = (
        "WASI cases outside the Rust lifecycle roster and all native runtime semantics"
    )
    metadata = COMMANDS.run(
        [cargo, "metadata", "--locked", "--format-version=1", "--no-deps"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=300,
        check=True,
        encoding="utf-8",
    ).stdout
    (run / "cargo.metadata.json").write_text(metadata, encoding="utf-8")
    aggregate["metadata_sha256"] = hashlib.sha256(metadata.encode("utf-8")).hexdigest()
    images = {}
    for role, package, target, kind, triple in (
        ("runtime", "molt-runtime", "molt_runtime", "rlib", WASI_TARGET),
        ("host", "molt-wasm-host", "wasi_runtime_lifecycle", "test", host_target),
        (
            "provider",
            "molt-runtime-platform",
            "molt_runtime_platform",
            "lib",
            WASI_TARGET,
        ),
    ):
        operation = next(
            operation
            for operation in cargo_operations(
                args.policy, args.profile, cargo=cargo, compiler_host=host_target
            )
            if operation.role == role
        )
        command, environment, plan = resolve_runtime_gate_cargo_operation(
            operation, cwd=ROOT, env=dict(os.environ), compiler_host=host_target
        )
        directory = run / target
        directory.mkdir()
        image = aggregate["images"][target] = {
            "build_command": command,
            "target_triple": triple,
        }
        if plan is not None:
            image["resolved_rustflags"] = list(plan.rustflags)
        build = cargo_build(command, directory, args.build_timeout_seconds, environment)
        image["build_returncode"] = build.returncode
        if build.returncode:
            raise RuntimeError(
                f"{target}: Cargo compilation failed with {build.returncode}"
            )
        if plan is not None:
            plan.verify()
        artifact = lifecycle_artifact(
            build.stdout, metadata, package=package, target=target, kind=kind
        )
        binary = Path(artifact["executable"])
        identity = truth._file_identity(binary)
        if triple == WASI_TARGET:
            with binary.open("rb") as stream:
                if stream.read(8) != b"\x00asm\x01\x00\x00\x00":
                    raise RuntimeError(
                        "WASI compiler artifact is not a WebAssembly module"
                    )
        if role == "runtime":
            aggregate["abi_dependency"] = observed_abi_dependency(
                build.stdout, metadata, policy="semantic", source_root=ROOT
            )
        if role == "runtime":
            aggregate["abi_dependency"]["retained_files"] = {
                name: descendants.retain_file(Path(name), run)
                for name in aggregate["abi_dependency"]["filenames"]
                if Path(name).suffix != ".rlib"
            }
        image["build"] = {
            "retained_artifact": descendants.retain_file(binary, run),
            "artifact": artifact,
            "executable_size": identity[0],
            "executable_sha256": identity[1],
            "observed": observed_build(
                build.stdout, artifact, args.profile, environment
            ),
            "stdout_sha256": truth._file_identity(directory / "build.stdout.jsonl")[1],
        }
        images[target] = (binary, identity)
        if package == "molt-wasm-host":
            # CARGO_BIN_EXE binds the integration consumer to this Cargo-built
            # production executable. Its bytes are a required input too, not
            # merely an arbitrary descendant admitted by the outer supervisor.
            host_artifact = lifecycle_artifact(
                build.stdout,
                metadata,
                package=package,
                target="molt-wasm-host",
                kind="bin",
                test=False,
            )
            host_binary = Path(host_artifact["executable"])
            host_identity = truth._file_identity(host_binary)
            image["production_host"] = {
                "retained_artifact": descendants.retain_file(host_binary, run),
                "artifact": host_artifact,
                "executable_size": host_identity[0],
                "executable_sha256": host_identity[1],
            }
            images["molt-wasm-host"] = (host_binary, host_identity)
    if truth.git_source_identity() != source or any(
        truth._file_identity(binary) != identity for binary, identity in images.values()
    ):
        raise RuntimeError("lifecycle source/image changed before handoff")
    binary, identity = images["wasi_runtime_lifecycle"]
    consumers = (
        ("molt_runtime", WASI_CONSUMER),
        ("molt_runtime_platform", WASI_PLATFORM_CONSUMER),
    )
    selection = {
        consumer: (
            ["--exact", consumer, "--ignored", "--test-threads=1", "--nocapture"],
            {consumer},
        )
        for _, consumer in consumers
    }
    aggregate["wasi_handoffs"] = []
    image = aggregate["images"]["wasi_runtime_lifecycle"]
    image["children"] = {}
    children = {}
    for index, (target, consumer) in enumerate(consumers):
        wasm, wasm_identity = images[target]
        environment = dict(os.environ)
        environment.update(
            MOLT_WASI_LIBTEST_ARTIFACT=str(wasm),
            MOLT_WASI_LIBTEST_SHA256=wasm_identity[1],
        )
        aggregate["wasi_handoffs"].append(
            {
                "consumer": consumer,
                "target": target,
                "environment": {
                    key: environment[key]
                    for key in (
                        "MOLT_WASI_LIBTEST_ARTIFACT",
                        "MOLT_WASI_LIBTEST_SHA256",
                    )
                },
            }
        )
        child_args, required = selection[consumer]
        directory = run / "wasi_runtime_lifecycle" / f"child-{index}"
        directory.mkdir()
        child, receipts = execute_child(
            binary,
            child_args,
            required,
            directory=directory,
            run_id=aggregate["run_id"],
            source=source,
            timeout=args.child_timeout_seconds,
            require_passed=True,
            retained_root=run,
            environment=environment,
        )
        children[consumer] = receipts
        image["children"][consumer] = {
            "returncode": child.returncode,
            "receipts": [str(p) for p in directory.glob("*.json")],
        }
        if child.returncode:
            raise RuntimeError(
                child_failure_summary(
                    consumer, child.returncode, receipts, child.stderr
                )
            )
    # One validator sees both ledgers and shares the invocation-id set, so an
    # otherwise valid first-child receipt cannot be replayed for the second.
    validate_children(
        children,
        selection,
        source=source,
        run_id=aggregate["run_id"],
        binary=binary,
        identity=identity,
        require_passed=True,
    )
    if truth.git_source_identity() != source or any(
        truth._file_identity(binary) != identity for binary, identity in images.values()
    ):
        raise RuntimeError("lifecycle source/image changed during execution")
    # Two integration passes: Rust enforces runtime nine/platform three laws.
    # Python accounts for those consumers, never inventing a WASM inventory.
    for path, row in aggregate["abi_dependency"]["retained_files"].items():
        descendants.retained_file(row, root=run)
        if truth._file_identity(Path(path)) != (row["size_bytes"], row["sha256"]):
            raise RuntimeError("ABI dependency changed during execution")
    image["passed_test_count"] = aggregate["passed_test_count"] = 2
    aggregate["status"] = "success"


# Fixed semantic obligations, checked against actual libtest discovery. This
# policy does not claim Molt WASM runtime or native ZLIB_DEBUG coverage.
ZLIB_WASI_CASES = frozenset(
    {
        "inflate::tests::counter_wrap_checksum_completion_and_gzip_length",
        "inflate::tests::counter_wrap_sync_preserves_recovered_totals",
        "deflate::test::counter_wrap_stored_and_compressed_streams",
        "stable::tests::counter_wrap_stable_totals_preserve_u64_width",
    }
)


def zlib_wasi_flags(
    command: list[str], environment: Mapping[str, str]
) -> tuple[dict, ProcessEnvironment]:
    # Reuse the queue's captured Cargo cfg/flag precedence (including wrappers),
    # without the runtime planner's unrelated external WASI C SDK requirement.
    from molt.cli.runtime_cargo_plan import (
        capture_cargo_configuration_inputs,
        cargo_selected_value,
    )
    from molt.cli.runtime_tool_selection import runtime_tool_path
    from molt.rust_toolchain import resolve_rustup_proxy, rust_flag_spans
    from tools.proof_queue_pkg.toolchain_capture import _cargo_flag_context

    captured = capture_cargo_configuration_inputs(
        ROOT, env=environment, cargo_command=command
    )
    configured_env, configuration, cli = captured[1], captured[4], captured[7]
    selector = cargo_selected_value(
        configuration,
        cli,
        configured_env,
        ("build", "rustc"),
        ("RUSTC", "CARGO_BUILD_RUSTC"),
        "rustc",
    )
    rustc = resolve_rustup_proxy(
        runtime_tool_path(selector, root=ROOT, env=configured_env, role="rustc"),
        role="rustc",
        root=ROOT,
        env=configured_env,
    )
    probes = []
    context, configured_env = _cargo_flag_context(
        command=command,
        cwd=ROOT,
        env=environment,
        target=WASI_TARGET,
        host_unit=False,
        rustc=rustc,
        probes=probes,
    )
    allowed = {
        "opt-level": {"0"},
        "overflow-checks": {"yes", "on", "true", "1"},
        "debug-assertions": {"yes", "on", "true", "1"},
    }
    for span in rust_flag_spans(context["selected_flags"]):
        short = (
            span.leading
            if span.option is not None
            else context["selected_flags"][span.start : span.stop]
        )
        if any(
            word.startswith("-") and not word.startswith("--") and "O" in word[1:]
            for word in short
        ):
            raise RuntimeError("provider flags contradict opt0: -O")
        if span.codegen is not None:
            key, _, value = span.codegen.partition("=")
            if key in allowed and value not in allowed[key]:
                raise RuntimeError(
                    f"provider flags contradict checked opt0: {span.codegen}"
                )
    return {"inputs": context, "probes": probes}, configured_env


def zlib_wasi_artifact(
    output: str, metadata: str, profile: str, environment: Mapping[str, str]
) -> tuple[dict, dict]:
    finished = [
        loads_exact(line) for line in output.splitlines() if line.startswith("{")
    ]
    finished = [
        row
        for row in finished
        if isinstance(row, dict) and row.get("reason") == "build-finished"
    ]
    if len(finished) != 1 or finished[0].get("success") is not True:
        raise RuntimeError("provider Cargo build did not finish successfully")
    artifact = lifecycle_artifact(
        output,
        metadata,
        package="zlib-rs",
        target="zlib_rs",
        kind="lib",
        manifest=ROOT / "vendor/rust/zlib-rs/Cargo.toml",
    )
    package = next(
        row
        for row in loads_exact(metadata)["packages"]
        if row["manifest_path"] == str(ROOT / "vendor/rust/zlib-rs/Cargo.toml")
    )
    if package["version"] != "0.6.8":
        raise RuntimeError("provider version differs from maintained package selector")
    observed = observed_build(output, artifact, profile, environment)
    if observed["target"].get("src_path") != str(
        ROOT / "vendor/rust/zlib-rs/src/lib.rs"
    ):
        raise RuntimeError("provider target has foreign source")
    actual = observed["observed_profile"]
    if (
        actual["opt_level"] != "0"
        or actual["debug_assertions"] is not True
        or actual.get("overflow_checks") is not True
        or observed["features"] != ["rust-allocator"]
    ):
        raise RuntimeError(
            "provider requires opt0, checked arithmetic and rust-allocator only"
        )
    binary = Path(artifact["executable"])
    target_root = Path(loads_exact(metadata)["target_directory"]).resolve()
    if not binary.is_relative_to(target_root / WASI_TARGET) or binary.suffix != ".wasm":
        raise RuntimeError("provider image is not in the selected WASI target")
    with binary.open("rb") as stream:
        if stream.read(8) != b"\0asm\x01\0\0\0":
            raise RuntimeError("provider image is not a WebAssembly module")
    return artifact, observed


def run_zlib_wasi(args: argparse.Namespace, run: Path, aggregate: dict) -> None:
    from tools.proof_queue_pkg.execution_custody import node_bootstrap_options

    if args.profile != "dev-fast":
        raise RuntimeError("zlib-wasi requires the checked dev-fast profile")
    source = truth.git_source_identity()
    aggregate["source_identity"] = source
    environment = dict(os.environ)
    node_options = environment.get("NODE_OPTIONS", "")
    if node_options and node_options != node_bootstrap_options():
        raise RuntimeError("zlib-wasi permits only queue-owned Node bootstrap options")
    node = resolve_node_runtime(source_root=ROOT, environment=environment)
    node_identity = truth._file_identity(node.path)
    aggregate["node"] = {
        "path": str(node.path),
        "version": node.version,
        "retained": descendants.retain_file(node.path, run),
    }
    cargo = os.environ.get("CARGO", "cargo")
    metadata = COMMANDS.run(
        [cargo, "metadata", "--locked", "--format-version=1", "--no-deps"],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        timeout=300,
        check=True,
        encoding="utf-8",
    ).stdout
    (run / "cargo.metadata.json").write_text(metadata, encoding="utf-8")
    aggregate["metadata_sha256"] = hashlib.sha256(metadata.encode()).hexdigest()
    (operation,) = cargo_operations(args.policy, args.profile, cargo=cargo)
    command = list(operation.command)
    context, environment = zlib_wasi_flags(command, environment)
    aggregate["cargo_flags"] = context
    if environment.get("NODE_OPTIONS", "") != node_options:
        raise RuntimeError(
            "zlib-wasi Cargo configuration changed Node bootstrap options"
        )
    aggregate["build_command"] = command
    build = cargo_build(command, run, args.build_timeout_seconds, environment)
    aggregate["build_returncode"] = build.returncode
    if build.returncode:
        raise RuntimeError(f"provider Cargo build failed: {build.returncode}")
    artifact, observed = zlib_wasi_artifact(
        build.stdout, metadata, args.profile, environment
    )
    wasm = Path(artifact["executable"])
    wasm_identity = truth._file_identity(wasm)
    image = aggregate["images"]["zlib_rs"] = {
        "artifact": artifact,
        "observed": observed,
        "target_triple": WASI_TARGET,
        "retained": descendants.retain_file(wasm, run),
    }
    bootstrap = "\n".join(
        [
            'const fs = require("node:fs");',
            'const {WASI} = require("node:wasi");',
            f"const image = {json.dumps(str(wasm))};",
            'const wasi = new WASI({version:"preview1",args:[image,...process.argv.slice(1)],env:{},returnOnExit:true});',
            "const wasmModule = new WebAssembly.Module(fs.readFileSync(image));",
            "const instance = new WebAssembly.Instance(wasmModule,{wasi_snapshot_preview1:wasi.wasiImport});",
            "process.exitCode = wasi.start(instance);",
        ]
    )
    # --eval is a flag, so the existing libtest runner preserves the bootstrap
    # while independently listing or narrowing libtest arguments after --.
    child_args = [
        f"--redirect-warnings={run / 'node-warnings.log'}",
        "--eval=" + bootstrap,
        "--",
        "counter_wrap",
        "--test-threads=1",
    ]
    aggregate["node_invocation"] = [str(node.path), *child_args]
    if (
        truth.git_source_identity() != source
        or truth._file_identity(node.path) != node_identity
    ):
        raise RuntimeError("provider source/Node changed before discovery")
    runner._ACTIVE_EVIDENCE_DIR = run / "discovery-evidence"
    runner._ACTIVE_EVIDENCE_DIR.mkdir()
    inventory, discovery = runner.listed_tests(
        str(node.path), child_args, timeout_seconds=args.child_timeout_seconds
    )
    image["discovery"] = discovery.receipt()
    if (
        not discovery.succeeded
        or len(inventory) != len(ZLIB_WASI_CASES)
        or set(inventory) != ZLIB_WASI_CASES
    ):
        raise RuntimeError(f"provider counter inventory mismatch: {inventory}")
    directory = run / "counter-controls"
    directory.mkdir()
    child, receipts = execute_child(
        node.path,
        child_args,
        ZLIB_WASI_CASES,
        directory=directory,
        run_id=aggregate["run_id"],
        source=source,
        timeout=args.child_timeout_seconds,
        require_passed=True,
        retained_root=run,
        environment=environment,
    )
    image["child_returncode"] = child.returncode
    if child.returncode:
        raise RuntimeError(
            child_failure_summary(
                "counter_wrap", child.returncode, receipts, child.stderr
            )
        )
    image["verified"] = validate_children(
        {"counter_wrap": receipts},
        {"counter_wrap": (child_args, ZLIB_WASI_CASES)},
        source=source,
        run_id=aggregate["run_id"],
        binary=node.path,
        identity=node_identity,
        require_passed=True,
    )
    if (
        truth.git_source_identity() != source
        or truth._file_identity(node.path) != node_identity
        or truth._file_identity(wasm) != wasm_identity
    ):
        raise RuntimeError("provider source/Node/WASM changed during execution")
    for row in context["inputs"]["sources"]:
        if truth._file_identity(Path(row["path"])) != (row["size"], row["sha256"]):
            raise RuntimeError("provider Cargo configuration changed during execution")
    for retained in (image["retained"], aggregate["node"]["retained"]):
        descendants.retained_file(retained, root=run)
    aggregate["passed_test_count"] = len(ZLIB_WASI_CASES)
    aggregate["status"] = "success"


def finish_aggregate(run: Path, aggregate: dict) -> int:
    runner.write_receipt(run / "aggregate.json", aggregate)
    print(
        json.dumps(
            {
                "status": aggregate["status"],
                "receipt": str(run / "aggregate.json"),
                "error": aggregate.get("error"),
            }
        )
    )
    return 0 if aggregate["status"] == "success" else 1


def main(argv: list[str] | None = None) -> int:
    args = parse_arguments(argv)
    if args.receipt_root is None:
        args.receipt_root = proof_scratch_root(ROOT) / "runtime-gate-runs"
    run_id = "runtime-" + uuid.uuid4().hex
    run = args.receipt_root / run_id
    run.mkdir(parents=True, exist_ok=False)
    aggregate: dict = {
        "schema": "molt.runtime-test-gate.v2",
        "policy": args.policy,
        "run_id": run_id,
        "status": "failed",
        "scope": (
            "full default-feature runtime semantics and declared core/math numeric controls; runtime performance probes unrun"
            if args.policy == "semantic"
            else "declared WASI micro-runtime lifecycle and Rust-only platform errno cases through the production host only"
            if args.policy == "wasi-lifecycle"
            else "maintained provider four wasm32 counter controls only; no Molt WASM runtime claim"
            if args.policy == "zlib-wasi"
            else "compile-only WASI Clippy checks for declared library packages; no test execution"
            if args.policy == "wasi-clippy"
            else "declared untimed allocation/refusal controls only; no timed performance acceptance"
        ),
        "images": {},
    }
    try:
        if args.policy == "wasi-clippy":
            run_compile_only_lint(args, run, aggregate)
            return finish_aggregate(run, aggregate)
        if args.policy == "zlib-wasi":
            run_zlib_wasi(args, run, aggregate)
            return finish_aggregate(run, aggregate)
        if args.policy == "wasi-lifecycle":
            run_wasi_lifecycle(args, run, aggregate)
            return finish_aggregate(run, aggregate)
        source = truth.git_source_identity()
        aggregate["source_identity"] = source
        cargo = os.environ.get("CARGO", "cargo")
        (operation,) = cargo_operations(args.policy, args.profile, cargo=cargo)
        command = list(operation.command)
        metadata = COMMANDS.run(
            [cargo, "metadata", "--locked", "--format-version=1", "--no-deps"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=300,
            check=True,
            encoding="utf-8",
        )
        (run / "cargo.metadata.json").write_bytes(metadata.stdout.encode("utf-8"))
        build = cargo_build(command, run, args.build_timeout_seconds)
        aggregate["build_command"] = command
        aggregate["build_returncode"] = build.returncode
        if build.returncode:
            raise RuntimeError(f"Cargo compilation failed with {build.returncode}")
        toolchain = {}
        for tool, flags in (("rustc", ["-vV"]), ("cargo", ["--version"])):
            version = COMMANDS.run(
                [cargo if tool == "cargo" else tool, *flags],
                cwd=ROOT,
                capture_output=True,
                text=True,
                timeout=30,
                check=True,
                encoding="utf-8",
            )
            toolchain[tool] = version.stdout.strip()
        aggregate["toolchain"] = toolchain
        aggregate["build_environment"] = build_environment(os.environ)
        artifacts = truth.expected_test_binaries(
            build.stdout, truth.package_identities_from_metadata(metadata.stdout)
        )
        selected_artifacts = select_artifacts(artifacts, args.policy)
        aggregate["abi_dependency"] = observed_abi_dependency(
            build.stdout, metadata.stdout, policy=args.policy, source_root=ROOT
        )
        aggregate["abi_dependency"]["retained_files"] = {
            name: descendants.retain_file(Path(name), run)
            for name in aggregate["abi_dependency"]["filenames"]
            if Path(name).suffix != ".rlib"
        }
        observed_images = {
            target: observed_build(
                build.stdout, artifact, args.profile, aggregate["build_environment"]
            )
            for target, artifact in selected_artifacts.items()
        }
        for target, observed in observed_images.items():
            if (OBSERVER_FEATURE in observed["features"]) != (
                args.policy == "allocation-observer"
            ):
                raise RuntimeError(
                    f"{target}: compiled observer feature contradicts {args.policy} policy"
                )
        if truth.git_source_identity() != source:
            raise RuntimeError("source changed during Cargo compilation")
        # Admit every image's features before invoking even libtest inventory.
        images = {}
        for target, artifact in selected_artifacts.items():
            observed = observed_images[target]
            binary = Path(artifact["executable"])
            identity = truth._file_identity(binary)
            runner._ACTIVE_EVIDENCE_DIR = run / target / "discovery-evidence"
            runner._ACTIVE_EVIDENCE_DIR.mkdir(parents=True)
            inventory, discovery = runner.listed_tests(
                str(binary), [], timeout_seconds=args.child_timeout_seconds
            )
            if not discovery.succeeded:
                raise RuntimeError(f"{target}: runtime discovery failed")
            selection = (
                observer_selections(inventory, target)
                if args.policy == "allocation-observer"
                else selections(
                    inventory, args.parallel_threads, sys.platform, target=target
                )
            )
            selected_tests = set().union(
                *(expected for _, expected in selection.values())
            )
            images[target] = (binary, identity, selection)
            aggregate["images"][target] = {
                "build": {
                    "retained_artifact": descendants.retain_file(binary, run),
                    "observed": observed,
                    "artifact": artifact,
                    "executable_size": identity[0],
                    "executable_sha256": identity[1],
                    "stdout_sha256": truth._file_identity(run / "build.stdout.jsonl")[
                        1
                    ],
                },
                "discovery": discovery.receipt(),
                "inventory": inventory,
                "unrun": [
                    {
                        "identity": name,
                        "status": "unrun",
                        "reason": unrun_reason(args.policy, target, name, sys.platform),
                    }
                    for name in sorted(set(inventory) - selected_tests)
                ],
                "children": {},
            }
        failures = []
        verification_errors = {}
        invocations: set[str] = set()
        for target, (binary, identity, selection) in images.items():
            image = aggregate["images"][target]
            require_passed = (
                args.policy == "allocation-observer" or target != "molt_runtime"
            )
            children: dict[str, list[dict]] = {}
            for index, (name, (child_args, expected)) in enumerate(selection.items()):
                directory = run / target / f"child-{index}"
                directory.mkdir()
                child, children[name] = execute_child(
                    binary,
                    child_args,
                    expected,
                    directory=directory,
                    run_id=run_id,
                    source=source,
                    timeout=args.child_timeout_seconds,
                    require_passed=require_passed,
                    retained_root=run,
                )
                image["children"][name] = {
                    "returncode": child.returncode,
                    "receipts": [str(path) for path in directory.glob("*.json")],
                }
                if child.returncode:
                    failures.append(f"{target}:{name}")
                    print(
                        child_failure_summary(
                            name, child.returncode, children[name], child.stderr
                        ),
                        file=sys.stderr,
                    )
            try:
                image["runtime_descendants"] = validate_children(
                    children,
                    selection,
                    source=source,
                    run_id=run_id,
                    binary=binary,
                    identity=identity,
                    require_passed=require_passed,
                    invocations=invocations,
                )
            except RuntimeError as exc:
                # A red image cannot claim a pass, but the other image's
                # independent controls still produce useful first-run evidence.
                image["verification_error"] = str(exc)
                verification_errors[target] = str(exc)
            else:
                image["passed_test_count"] = sum(
                    len(expected) for _, expected in selection.values()
                )
        if failures or verification_errors:
            raise RuntimeError(
                f"failed child processes: {failures}; image verification errors: {verification_errors}"
            )
        if truth.git_source_identity() != source or any(
            truth._file_identity(binary) != identity
            for binary, identity, _ in images.values()
        ):
            raise RuntimeError("source/binary changed during aggregate verification")
        for path, row in aggregate["abi_dependency"]["retained_files"].items():
            descendants.retained_file(row, root=run)
            if truth._file_identity(Path(path)) != (row["size_bytes"], row["sha256"]):
                raise RuntimeError("ABI dependency changed during execution")
        aggregate["passed_test_count"] = sum(
            image["passed_test_count"] for image in aggregate["images"].values()
        )
        aggregate["status"] = "success"
    except Exception as exc:
        aggregate["error"] = f"{type(exc).__name__}: {exc}"
    return finish_aggregate(run, aggregate)


if __name__ == "__main__":
    raise SystemExit(main())
