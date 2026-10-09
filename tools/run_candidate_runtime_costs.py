#!/usr/bin/env python3
"""Fixed candidate cost operations; evidence-only, never release acceptance.

Measurement, allocation, guards and retained binary receipts stay with their
existing owners. This entry point supplies only fixed operation selection and
current-job source/output joins; it accepts no arbitrary command or baseline.
"""

from __future__ import annotations

import argparse
from io import StringIO
import json
import os
from pathlib import Path
import secrets

if __package__ in (None, ""):
    from import_file import bind_repository_imports, load_module_from_path
else:
    from tools.import_file import bind_repository_imports, load_module_from_path

ROOT = bind_repository_imports(__file__)

from molt.cargo_execution_policy import load_ci_cargo_policy  # noqa: E402
from molt.exact_json import loads_exact  # noqa: E402
from tools import cargo_test_binary_runner as binaries  # noqa: E402
from tools.libtest_results import accounting_problem, parse_libtest  # noqa: E402
from tools.run_cargo_test_truth import (  # noqa: E402
    _receipt_infrastructure_problem,
    git_source_identity,
    load_binary_receipts,
)
from tools.runtime_descendant_receipts import _parent_capture  # noqa: E402

# The benchmark directory shares its name with tools/bench.py. Bind both files
# explicitly so the list owner's import reuses this exact numeric module.
l7 = load_module_from_path(
    "run_l7_numeric_attestation", ROOT / "tools/bench/run_l7_numeric_attestation.py"
)
lists = load_module_from_path(
    "run_list_delta_attestation", ROOT / "tools/bench/run_list_delta_attestation.py"
)
OUTPUT = ROOT / "proof-receipts/evidence/candidate-runtime-costs"
STORAGE_TEST = "shared_hash_storage_performance_attestation"
STORAGE_PREFIX = "SHARED_HASH_STORAGE_ATTESTATION="


def storage_payload(
    stdout: str, argv: list[str], source: dict, affinity_mask: str
) -> dict:
    report = parse_libtest(StringIO(stdout), tuple(argv))
    if not report.complete or report.rows() != [
        {"identity": STORAGE_TEST, "status": "pass"}
    ]:
        raise RuntimeError("storage cost capture lacks its exact completed test")
    records = [
        line.split(STORAGE_PREFIX, 1)[1]
        for line in stdout.splitlines()
        if STORAGE_PREFIX in line
    ]
    if len(records) != 1:
        raise RuntimeError("storage cost capture must contain one attestation")
    payload = loads_exact(records[0])
    if (
        not isinstance(payload, dict)
        or type(payload.get("schema_version")) is not int
        or payload["schema_version"] != 1
        or payload.get("kind") != "shared_hash_storage_performance_attestation"
        or payload.get("profile") != "release"
        or payload.get("source") != source
        or payload.get("affinity_mask") != affinity_mask
        or not isinstance(payload.get("cases"), list)
        or not payload["cases"]
    ):
        raise RuntimeError("storage cost capture does not bind its source or workload")
    return payload


def storage_capture(
    directory: Path, nonce: str, identity: dict, argv: list[str]
) -> str:
    receipts = load_binary_receipts(
        directory, expected_run_id=nonce, expected_source_identity=identity
    )
    if len(receipts) != 1:
        raise RuntimeError("storage cost requires exactly one binary receipt")
    receipt = receipts[0]
    executions = receipt.get("executions")
    if (
        receipt.get("status") != "success"
        or receipt.get("returncode") != 0
        or receipt.get("test_results") != [{"identity": STORAGE_TEST, "status": "pass"}]
        or accounting_problem(receipt) is not None
        or _receipt_infrastructure_problem(receipt) is not None
        or receipt.get("executable_resolved") != str(Path(argv[0]).resolve())
        or not isinstance(executions, list)
        or len(executions) != 1
        or not isinstance(executions[0], dict)
        or executions[0].get("argv") != argv
        or type(executions[0].get("returncode")) is not int
        or executions[0]["returncode"] != 0
        or executions[0].get("timed_out") is not False
        or executions[0].get("termination")
        != binaries.termination_payload(0, timed_out=False)
    ):
        raise RuntimeError("storage cost lacks its exact successful binary execution")
    # Console output is a diagnostic tail. The existing receipt owner binds
    # the complete retained capture to this invocation and its byte identity.
    stream = _parent_capture(receipt, executions[0], "stdout", directory)
    return stream.read_text(encoding="utf-8")


def run_storage(directory: Path, timeout: float) -> None:
    source = l7._source_snapshot()
    rustc = l7._parent_command(["rustc", "--version", "--verbose"]).decode().strip()
    lock = l7._sha256_file(ROOT / "Cargo.lock")
    # Same Cargo target as the numeric component, discovered by the existing
    # compiler-artifact selector; no glob, guessed executable or second parser.
    image, build = l7._build_test_executable(
        l7.COMPONENTS["runtime_bigint"],
        timeout=timeout,
        source=source,
        rustc=rustc,
        cargo_lock_sha256=lock,
    )
    control = l7._resolve_execution_control("auto")
    nonce = secrets.token_hex(16)
    expected = {
        "git_commit": source["git_commit"],
        "git_dirty": "true" if source["git_dirty"] else "false",
        "rustc": rustc,
        "build_fingerprint": build["artifact_fingerprint"],
        "run_nonce": nonce,
    }
    child_env = {
        "MOLT_L7_GIT_COMMIT": expected["git_commit"],
        "MOLT_L7_GIT_DIRTY": expected["git_dirty"],
        "MOLT_L7_RUSTC": rustc,
        "MOLT_L7_BUILD_FINGERPRINT": expected["build_fingerprint"],
        "MOLT_L7_RUN_NONCE": nonce,
        "MOLT_L7_AFFINITY_MASK": control["affinity_mask"],
    }
    argv = [
        str(image),
        STORAGE_TEST,
        "--exact",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    identity = git_source_identity()
    l7._write_json_atomic(
        directory / "selection.json",
        {
            "source": source,
            "source_identity": identity,
            "build": build,
            "execution_control": control,
            "argv": argv,
            "child_source": expected,
        },
    )
    command = [
        "--timeout-seconds",
        str(timeout),
        "--receipt-dir",
        str(directory / "binaries"),
        "--run-id",
        nonce,
        "--source-identity-json",
        json.dumps(identity),
        "--",
        *argv,
    ]
    # The binary runner owns its one lifetime budget, guard, closure and
    # immutable receipt. Do not wrap it in a competing equal-deadline child.
    previous = {name: os.environ.get(name) for name in child_env}
    try:
        os.environ.update(child_env)
        code = binaries.main(command)
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
    if code:
        raise RuntimeError(f"storage cost child receipt failed: {code}")
    if (
        l7._source_snapshot() != source
        or l7._sha256_file(image) != build["executable_sha256"]
    ):
        raise RuntimeError("storage cost source or image changed")
    stdout = storage_capture(directory / "binaries", nonce, identity, argv)
    payload = storage_payload(stdout, argv, expected, control["affinity_mask"])
    l7._write_json_atomic(directory / "attestation.json", payload)


def run_operation(operation: str) -> None:
    directory = OUTPUT / operation
    directory.mkdir(parents=True, exist_ok=False)
    timeout = float(load_ci_cargo_policy().execution_budgets.timeout_seconds("cold"))
    record = {
        "operation": operation,
        "performance_claim": False,
        "release_acceptance": False,
        "status": "running",
    }
    l7._write_json_atomic(directory / "result.json", record)
    try:
        if operation == "l7":
            output = directory / "attestation.json"
            code = l7.main(
                ["--runs", "7", "--timeout", str(timeout), "--output", str(output)]
            )
            payload = l7._load_json_strict(output)
            if (
                code
                or payload["comparison"]["status"] != "evidence_only"
                or payload["comparison"]["performance_claim"] is not False
            ):
                raise RuntimeError("candidate L7 must be valid evidence_only output")
        elif operation == "list":
            code = lists.main(
                [
                    "--runs",
                    "7",
                    "--timeout",
                    str(timeout),
                    "--output",
                    str(directory / "attestation.json"),
                ]
            )
            if code:
                raise RuntimeError(f"candidate list attestation failed: {code}")
        else:
            run_storage(directory, timeout)
        record["status"] = "evidence_only"
    except BaseException as error:
        record.update(status="failed", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        l7._write_json_atomic(directory / "result.json", record)
    print(f"{operation}: candidate evidence only; no comparative or release PASS")


def main(argv: list[str] | None = None) -> int:
    argparse.ArgumentParser(description=__doc__).parse_args(argv)
    for operation in ("l7", "list", "storage"):
        run_operation(operation)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
