#!/usr/bin/env python3
"""Source-bound runtime gate: parallel ordinary tests and mandatory fresh processes.

Lifecycle tests intentionally mutate process-global publication/shutdown state.
The isolated GIL custody case still launches real concurrent worker threads; its
libtest harness has one test, not a single-threaded runtime. Performance probes
are declared unrun and never credited as semantic passes. Owning tests that
re-execute the runtime image are credited only with their source/image-bound
descendant receipts (tools/runtime_descendant_receipts.py); this gate has no
CPython target-minor coordinate authority.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
from pathlib import Path
import sys
import uuid

if __package__ in (None, ""):
    from import_file import bind_repository_imports
else:
    from tools.import_file import bind_repository_imports
ROOT = bind_repository_imports(__file__)

from tools import cargo_test_binary_runner as runner  # noqa: E402
from tools import run_cargo_test_truth as truth  # noqa: E402
from tools import runtime_descendant_receipts as descendants  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402
from tools.libtest_results import accounting_problem  # noqa: E402

COMMANDS = CommandExecutor.for_file(__file__)
ISOLATED = (
    "builtins::exceptions::tests::tls_exception_destructor_excludes_concurrent_shutdown",
    "cpython_abi_hooks::tests::gil_custody_recursive_ensure_and_allow_threads_make_progress",
    "state::lifecycle::shutdown_tests::embedding_shutdown_covers_first_pending_callback_atexit_and_live_stdio",
    "state::lifecycle::shutdown_tests::process_exit_covers_collection_before_pending_callbacks_with_or_without_lease",
    "state::runtime_state::tests::concurrent_init_waits_for_ready_publication",
    "state::runtime_state::tests::finalizing_unpublishes_before_atexit_reentry_and_racing_entrants",
    "state::runtime_state::tests::shutdown_refuses_attached_and_detached_foreign_thread_state_owners",
)
PROBES = (
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
    "cpython_abi_hooks::tests::single_thread_extension_call_preemption_bench": (
        frozenset({"linux", "win32"})
    ),
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
) -> dict[str, dict[str, object] | None]:
    if set(children) != set(selection):
        raise RuntimeError("aggregate lacks exact complete child ledger")
    invocations: set[str] = set()
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


def validate_artifact(artifact: dict[str, str]) -> None:
    # Cargo target.kind follows crate-type: this runtime declares rlib, not lib.
    # Do not normalize arbitrary binary artifacts into the runtime authority.
    if (
        artifact["target_name"] != "molt_runtime"
        or artifact["target_kind"] != "rlib"
        or not artifact["package"].startswith("molt-runtime@")
    ):
        raise RuntimeError("Cargo artifact is not the runtime rlib test authority")


def observed_build(
    cargo_output: str,
    artifact: dict[str, str],
    profile_name: str,
    environment: dict[str, str | None],
) -> dict:
    matches = []
    for line in cargo_output.splitlines():
        if not line.startswith("{"):
            continue
        item = json.loads(line)
        if (
            item.get("reason") == "compiler-artifact"
            and item.get("executable")
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="release-output")
    parser.add_argument(
        "--parallel-threads", type=int, default=max(2, min(8, os.cpu_count() or 2))
    )
    parser.add_argument("--build-timeout-seconds", type=float, default=6300)
    parser.add_argument("--child-timeout-seconds", type=float, default=120)
    parser.add_argument(
        "--receipt-root",
        type=Path,
        default=ROOT / "proof-receipts/evidence/runtime-gate-runs",
    )
    args = parser.parse_args(argv)
    if args.parallel_threads < 2 or any(
        not math.isfinite(value) or value <= 0
        for value in (args.build_timeout_seconds, args.child_timeout_seconds)
    ):
        parser.error("positive timeouts and at least two parallel threads are required")
    run_id = "runtime-" + uuid.uuid4().hex
    run = args.receipt_root / run_id
    run.mkdir(parents=True, exist_ok=False)
    aggregate: dict = {
        "schema": "molt.runtime-test-gate.v1",
        "run_id": run_id,
        "status": "failed",
        "performance_probes": [
            {"identity": name, "status": "unrun"} for name in probes_for(sys.platform)
        ],
        "children": {},
    }
    try:
        source = truth.git_source_identity()
        aggregate["source_identity"] = source
        command = [
            "cargo",
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
        metadata = COMMANDS.run(
            ["cargo", "metadata", "--locked", "--format-version=1", "--no-deps"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=300,
            check=True,
            encoding="utf-8",
        )
        build = COMMANDS.run(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=args.build_timeout_seconds,
            check=False,
            encoding="utf-8",
        )
        (run / "build.stdout.jsonl").write_text(build.stdout, encoding="utf-8")
        (run / "build.stderr.log").write_text(build.stderr, encoding="utf-8")
        aggregate["build_command"] = command
        aggregate["build_returncode"] = build.returncode
        if build.returncode:
            raise RuntimeError(f"Cargo compilation failed with {build.returncode}")
        toolchain = {}
        for tool, flags in (("rustc", ["-vV"]), ("cargo", ["--version"])):
            version = COMMANDS.run(
                [tool, *flags],
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
        if len(artifacts) != 1:
            raise RuntimeError("Cargo must publish exactly one runtime lib test binary")
        artifact = next(iter(artifacts.values()))
        validate_artifact(artifact)
        observed = observed_build(
            build.stdout, artifact, args.profile, aggregate["build_environment"]
        )
        binary = Path(artifact["executable"])
        identity = truth._file_identity(binary)
        aggregate["build"] = {
            "argv": command,
            "profile": args.profile,
            "observed": observed,
            "artifact": artifact,
            "executable_size": identity[0],
            "executable_sha256": identity[1],
            "stdout_sha256": truth._file_identity(run / "build.stdout.jsonl")[1],
        }
        if truth.git_source_identity() != source:
            raise RuntimeError("source changed during Cargo compilation")
        runner._ACTIVE_EVIDENCE_DIR = run / "discovery-evidence"
        runner._ACTIVE_EVIDENCE_DIR.mkdir()
        inventory, discovery = runner.listed_tests(
            str(binary), [], timeout_seconds=args.child_timeout_seconds
        )
        if not discovery.succeeded:
            raise RuntimeError("runtime discovery failed")
        aggregate["discovery"] = discovery.receipt()
        aggregate["inventory"] = inventory
        selection = selections(inventory, args.parallel_threads, sys.platform)
        children: dict[str, list[dict]] = {}
        failures = []
        for index, (name, (child_args, _)) in enumerate(selection.items()):
            directory = run / f"child-{index}"
            directory.mkdir()
            child_command = [
                sys.executable,
                str(ROOT / "tools/cargo_test_binary_runner.py"),
                "--timeout-seconds",
                str(args.child_timeout_seconds),
                "--receipt-dir",
                str(directory),
                "--run-id",
                run_id,
                "--source-identity-json",
                json.dumps(source),
                "--",
                str(binary),
                *child_args,
            ]
            child = COMMANDS.run(
                child_command,
                cwd=ROOT,
                capture_output=True,
                text=True,
                timeout=args.child_timeout_seconds + 60,
                encoding="utf-8",
            )
            (directory / "driver.stdout.log").write_text(child.stdout, encoding="utf-8")
            (directory / "driver.stderr.log").write_text(child.stderr, encoding="utf-8")
            children[name] = truth.load_binary_receipts(
                directory, expected_run_id=run_id, expected_source_identity=source
            )
            aggregate["children"][name] = {
                "returncode": child.returncode,
                "receipts": [str(path) for path in directory.glob("*.json")],
            }
            if child.returncode:
                failures.append(name)
                print(
                    child_failure_summary(
                        name, child.returncode, children[name], child.stderr
                    ),
                    file=sys.stderr,
                )
        if failures:
            raise RuntimeError(f"failed child processes: {failures}")
        aggregate["runtime_descendants"] = validate_children(
            children,
            selection,
            source=source,
            run_id=run_id,
            binary=binary,
            identity=identity,
        )
        if (
            truth.git_source_identity() != source
            or truth._file_identity(binary) != identity
        ):
            raise RuntimeError("source/binary changed during aggregate verification")
        aggregate["semantic_pass_count"] = len(inventory) - len(
            probes_for(sys.platform)
        )
        aggregate["status"] = "success"
    except Exception as exc:
        aggregate["error"] = f"{type(exc).__name__}: {exc}"
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


if __name__ == "__main__":
    raise SystemExit(main())
