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
The WASI lifecycle policy builds a target libtest and hands its exact image to
the existing native production-host integration; Rust owns its semantic roster.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import subprocess
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
from molt.cargo_execution_policy import admit_cargo_build  # noqa: E402
from molt.dx import PROOF_SCRATCH_ROOT_ENV, proof_scratch_root  # noqa: E402
from molt.exact_json import loads_exact  # noqa: E402
from tools.libtest_results import accounting_problem  # noqa: E402

COMMANDS = CommandExecutor.for_file(__file__)
ISOLATED = (
    "async_rt::sockets::state::socket_runtime_state_tests::cold_socket_and_websocket_retirement_never_starts_services",
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
OBSERVER_FEATURE = "l7-attestation-probe"
OBSERVER_CASES = {
    "molt_runtime": (
        "builtins::frames::namespace_tests::task_growth_refusal::both_task_producers_refuse_actual_late_map_growth_and_restore_owners",
        "call::bind::inline_cache::binding_tests::direct_call_ic_hot_path_is_allocation_free_with_slow_path_control",
        "cpython_abi_hooks::tests::cpython_abi_gil_custody_is_allocation_free_after_warmup",
        "object::gc::tests::repeated_reachable_collection_workspace_allocations",
        "cpython_abi_hooks::exception_rendering_tests::decimal_output_limits::rejected_decimal_magnitude_does_not_allocate_decimal_storage",
        "object::ops::unpack_transaction_tests::exact_builtin_unpack_hot_path_performs_zero_allocator_calls",
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
    if (
        policy == "semantic"
        or name in OBSERVER_PERFORMANCE_PROBES[target]
        or (target == "molt_runtime" and name in probes_for(platform))
    ):
        return "performance probe"
    return "outside declared allocation-observer controls"


def image_targets(policy: str) -> dict[str, str]:
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


def probes_for(platform: str) -> tuple[str, ...]:
    """Return the probes the runtime inventory must carry on ``platform``."""
    return tuple(
        name
        for name in PROBES
        if platform in PROBE_PLATFORMS.get(name, frozenset({platform}))
    )


def selections(
    inventory: list[str], threads: int, platform: str
) -> dict[str, tuple[list[str], set[str]]]:
    if threads < 2:
        raise RuntimeError(
            "parallel runtime remainder requires at least two harness threads"
        )
    if len(inventory) != len(set(inventory)):
        raise RuntimeError("duplicate test discovery identity")
    probes = probes_for(platform)
    required = set(ISOLATED + probes)
    if not required <= set(inventory):
        raise RuntimeError(
            f"missing required runtime cases: {sorted(required - set(inventory))}"
        )
    remainder = set(inventory) - required
    if not remainder:
        raise RuntimeError("empty parallel runtime remainder")
    parallel = [f"--test-threads={threads}", "--nocapture"]
    for name in ISOLATED + probes:
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
            for name in ISOLATED
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
) -> None:
    # Cargo target.kind follows crate-type: this runtime declares rlib, not lib.
    # Do not normalize arbitrary binary artifacts into the runtime authority.
    if (
        artifact["target_name"] != target_name
        or artifact["target_kind"] != target_kind
        or not artifact["package"].startswith("molt-runtime@")
    ):
        raise RuntimeError(
            f"Cargo artifact is not the {target_name}/{target_kind} test authority"
        )


def select_artifacts(
    artifacts: dict[str, dict[str, str]], policy: str
) -> dict[str, dict[str, str]]:
    targets = image_targets(policy)
    selected = {}
    for artifact in artifacts.values():
        name = artifact["target_name"]
        if name not in targets or name in selected:
            raise RuntimeError(f"unexpected or duplicate Cargo test target: {name}")
        validate_artifact(artifact, name, targets[name])
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


def observed_build(
    cargo_output: str,
    artifact: dict[str, str],
    profile_name: str,
    environment: dict[str, str | None],
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
    if profile_name in {"release-fast", "release-output"}:
        if profile["debug_assertions"] is not False:
            raise RuntimeError(
                f"{profile_name} compiler artifact enables debug assertions"
            )
        # rustc -C flags can override Cargo's profile metadata. Fail closed for
        # contradictory or unfamiliar explicit assertion settings in either lane.
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
            flags = (environment.get(variable) or "").replace("\x1f", " ")
            if re.search(r"--cfg(?:\s+|=)[\"']?debug_assertions(?:\b|$)", flags):
                raise RuntimeError(
                    f"{variable} explicitly enables debug_assertions cfg"
                )
            for match in re.finditer(
                r"debug-assertions(?:\s*=\s*|\s+)?([^\s]*)", flags
            ):
                if match.group(1).lower() not in {"no", "off", "false", "0"}:
                    raise RuntimeError(
                        f"{variable} contradicts disabled debug assertions"
                    )
        for variable in (
            f"CARGO_PROFILE_{profile_name.upper().replace('-', '_')}_DEBUG_ASSERTIONS",
            "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS",
        ):
            value = environment.get(variable)
            if value is not None and value.lower() not in {"false", "0"}:
                raise RuntimeError(f"{variable} contradicts disabled debug assertions")
    return {
        "requested_profile": profile_name,
        "profile_role": "shipping"
        if profile_name == "release-output"
        else "iteration"
        if profile_name == "release-fast"
        else "custom",
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
    environment: dict[str, str] | None = None,
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


WASI_TARGET = "wasm32-wasip1"
WASI_CONSUMER = "actual_wasi_runtime_lifecycle_cases_through_production_host"


def lifecycle_artifact(
    output: str,
    metadata: str,
    *,
    package: str,
    target: str,
    kind: str,
    test: bool = True,
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
        and p.get("manifest_path") == str(ROOT / "runtime" / package / "Cargo.toml")
        and p.get("source") is None
    ]
    if len(canonical) != 1 or emitted[0].get("package_id") != canonical[0]["id"]:
        raise RuntimeError("lifecycle artifact has foreign package authority")
    if artifact["target_name"] != target or artifact["target_kind"] != kind:
        raise RuntimeError("lifecycle artifact has wrong target authority")
    return artifact


def run_wasi_lifecycle(args: argparse.Namespace, run: Path, aggregate: dict) -> None:
    # Keep Rust's CASES as the sole semantic roster. The Python policy owns only
    # its three-image production/handoff and the exact native integration consumer.
    from molt.cli.runtime_cargo_plan import resolve_runtime_cargo_plan

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
    for package, target, kind, triple in (
        ("molt-runtime", "molt_runtime", "rlib", WASI_TARGET),
        ("molt-wasm-host", "wasi_runtime_lifecycle", "test", host_target),
    ):
        command = [
            cargo,
            "test",
            "--locked",
            "--profile",
            args.profile,
            "-p",
            package,
            "--target",
            triple,
            "--no-run",
            "--message-format=json",
        ]
        environment = dict(os.environ)
        plan = None
        if triple == WASI_TARGET:
            command.extend(
                ["--lib", "--no-default-features", "--features", "stdlib_micro"]
            )
            plan = resolve_runtime_cargo_plan(
                ROOT,
                env=environment,
                cargo_command=command,
                requested_target=WASI_TARGET,
                host_target=host_target,
                rustflags_transform=lambda flags: (
                    *flags,
                    "-C",
                    "link-arg=--export-table",
                ),
            )
            plan.verify()
            command, environment = list(plan.command), dict(plan.environment)
        else:
            command.extend(["--test", target])
        directory = run / target
        directory.mkdir()
        image = aggregate["images"][target] = {
            "build_command": command,
            "target_triple": triple,
        }
        if plan is not None:
            image["resolved_rustflags"] = list(plan.rustflags)
        admit_cargo_build(command, cwd=ROOT, env=environment)
        build = COMMANDS.run(
            command,
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            timeout=args.build_timeout_seconds,
            check=False,
            encoding="utf-8",
        )
        (directory / "build.stdout.jsonl").write_text(build.stdout, encoding="utf-8")
        (directory / "build.stderr.log").write_text(build.stderr, encoding="utf-8")
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
            aggregate["abi_dependency"] = observed_abi_dependency(
                build.stdout, metadata, policy="semantic", source_root=ROOT
            )
        if triple == WASI_TARGET:
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
    wasm, wasm_identity = images["molt_runtime"]
    binary, identity = images["wasi_runtime_lifecycle"]
    environment = dict(os.environ)
    environment.update(
        MOLT_WASI_LIBTEST_ARTIFACT=str(wasm), MOLT_WASI_LIBTEST_SHA256=wasm_identity[1]
    )
    aggregate["wasi_handoff"] = {
        key: environment[key]
        for key in ("MOLT_WASI_LIBTEST_ARTIFACT", "MOLT_WASI_LIBTEST_SHA256")
    }
    selection = {
        WASI_CONSUMER: (
            ["--exact", WASI_CONSUMER, "--ignored", "--test-threads=1", "--nocapture"],
            {WASI_CONSUMER},
        )
    }
    child_args, required = selection[WASI_CONSUMER]
    directory = run / "wasi_runtime_lifecycle" / "child-0"
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
    image = aggregate["images"]["wasi_runtime_lifecycle"]
    image["children"] = {
        WASI_CONSUMER: {
            "returncode": child.returncode,
            "receipts": [str(p) for p in directory.glob("*.json")],
        }
    }
    if child.returncode:
        raise RuntimeError(
            child_failure_summary(
                WASI_CONSUMER, child.returncode, receipts, child.stderr
            )
        )
    validate_children(
        {WASI_CONSUMER: receipts},
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
    # This is one integration pass. Its five semantic cases are enforced and
    # reported by Rust; no invented Python test count or native WASM inventory.
    for path, row in aggregate["abi_dependency"]["retained_files"].items():
        descendants.retained_file(row, root=run)
        if truth._file_identity(Path(path)) != (row["size_bytes"], row["sha256"]):
            raise RuntimeError("ABI dependency changed during execution")
    image["passed_test_count"] = aggregate["passed_test_count"] = 1
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="release-output")
    parser.add_argument(
        "--policy",
        choices=("semantic", "allocation-observer", "wasi-lifecycle"),
        default="semantic",
    )
    parser.add_argument(
        "--parallel-threads", type=int, default=max(2, min(8, os.cpu_count() or 2))
    )
    parser.add_argument("--build-timeout-seconds", type=float, default=6300)
    parser.add_argument("--child-timeout-seconds", type=float, default=120)
    parser.add_argument(
        "--receipt-root",
        type=Path,
        default=(
            proof_scratch_root(ROOT)
            if os.environ.get(PROOF_SCRATCH_ROOT_ENV, "").strip()
            else ROOT / "proof-receipts/evidence"
        )
        / "runtime-gate-runs",
    )
    args = parser.parse_args(argv)
    if (args.policy == "semantic" and args.parallel_threads < 2) or any(
        not math.isfinite(value) or value <= 0
        for value in (args.build_timeout_seconds, args.child_timeout_seconds)
    ):
        parser.error("positive timeouts and at least two parallel threads are required")
    run_id = "runtime-" + uuid.uuid4().hex
    run = args.receipt_root / run_id
    run.mkdir(parents=True, exist_ok=False)
    aggregate: dict = {
        "schema": "molt.runtime-test-gate.v2",
        "policy": args.policy,
        "run_id": run_id,
        "status": "failed",
        "scope": (
            "full default-feature runtime semantics; performance probes unrun"
            if args.policy == "semantic"
            else "declared WASI micro-runtime lifecycle cases through the production host only"
            if args.policy == "wasi-lifecycle"
            else "declared untimed allocation/refusal controls only; no timed performance acceptance"
        ),
        "images": {},
    }
    try:
        if args.policy == "wasi-lifecycle":
            run_wasi_lifecycle(args, run, aggregate)
            return finish_aggregate(run, aggregate)
        source = truth.git_source_identity()
        aggregate["source_identity"] = source
        cargo = os.environ.get("CARGO", "cargo")
        command = [
            cargo,
            "test",
            "--locked",
            "--profile",
            args.profile,
            "-p",
            "molt-runtime",
            "--lib",
            "--no-run",
            "--message-format=json",
        ]
        if args.policy == "allocation-observer":
            command.extend(
                [
                    "--test",
                    "l7_numeric_runtime_perf_attestation",
                    "--features",
                    OBSERVER_FEATURE,
                ]
            )
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
        admit_cargo_build(command, cwd=ROOT, env=None)
        build = COMMANDS.run(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=args.build_timeout_seconds,
            check=False,
            encoding="utf-8",
        )
        (run / "build.stdout.jsonl").write_bytes(build.stdout.encode("utf-8"))
        (run / "build.stderr.log").write_text(build.stderr, encoding="utf-8")
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
        aggregate["build_environment"] = {
            key: os.environ.get(key)
            for key in (
                "RUSTUP_TOOLCHAIN",
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "CARGO_BUILD_RUSTFLAGS",
                "CARGO_BUILD_TARGET",
                "CARGO_TARGET_DIR",
                "CARGO_PROFILE_RELEASE_FAST_DEBUG_ASSERTIONS",
                "CARGO_PROFILE_RELEASE_OUTPUT_DEBUG_ASSERTIONS",
                "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS",
            )
        }
        aggregate["build_environment"].update(
            {
                key: value
                for key, value in os.environ.items()
                if key.startswith("CARGO_TARGET_") and key.endswith("_RUSTFLAGS")
            }
        )
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
                else selections(inventory, args.parallel_threads, sys.platform)
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
                    require_passed=args.policy == "allocation-observer",
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
                    require_passed=args.policy == "allocation-observer",
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
