"""Execute Mac-selected landing gates on the farm; publication stays on the Mac.

The existing trunk runner owns transport, source staging, detachment and collection.
The ordinary landing executor owns gate guards, admission and failure classification.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict, replace
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.driver_errors import CandidateFailure, DriverError  # noqa: E402
from tools.molt_dev_policy import GateConfig, SelectedGate  # noqa: E402
from tools.proof_plan import CargoTargetAccess  # noqa: E402

SCHEMA = "molt.remote-landing.v1"
HOST_SETTING = "MOLT_LANDING_GATE_HOST"


def gate_records(gates) -> list[dict]:
    # JSON's closed wire form; tuples and enum values have one spelling.
    return json.loads(json.dumps([asdict(gate) for gate in gates]))


def decode_gates(records: list[dict]) -> tuple[SelectedGate, ...]:
    gates = []
    for record in records:
        data = dict(record)
        for key in ("toolchains", "pinned_assets", "cells", "collection_command"):
            if data[key] is not None:
                data[key] = tuple(data[key])
        if isinstance(data["command"], list):
            data["command"] = tuple(data["command"])
        data["cargo_target_access"] = CargoTargetAccess(data["cargo_target_access"])
        gates.append(SelectedGate(**data))
    return tuple(gates)


def request_digest(request: dict) -> str:
    return hashlib.sha256(
        json.dumps(request, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def admit_result(receipt: dict, request: dict) -> None:
    expected = {
        "schema": SCHEMA,
        "source_commit": request["commit"],
        "source_tree": request["tree"],
        "source_tree_state": "clean",
        "request_sha256": request_digest(request),
    }
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise DriverError(
            "remote landing receipt has different source, tree or gate selection"
        )
    status = receipt.get("status")
    if status == "failure":
        raise CandidateFailure(f"remote landing gate failed: {receipt.get('error')}")
    if status != "success":
        raise DriverError(
            f"remote landing infrastructure stopped: {receipt.get('error')}"
        )
    if receipt.get("passed") != list(range(len(request["gates"]))):
        raise DriverError("remote landing receipt omits selected gates")
    if not isinstance(receipt.get("toolchains"), dict) or set(
        receipt["toolchains"]
    ) != set(request["toolchains"]):
        raise DriverError("remote landing receipt omits admitted toolchains")
    if not isinstance(receipt.get("seconds"), list) or len(receipt["seconds"]) != len(
        request["gates"]
    ):
        raise DriverError("remote landing receipt omits gate outcomes")


def run_remote(
    repo: Path, commit: str, gates, *, host: str
) -> tuple[list[tuple[float, str]], Path]:
    """Run ``gates`` on ``host``; return their seconds and the collected output
    directory, which holds each gate's ``gate-NNN`` logs in request order."""
    try:
        return _run_remote(repo, commit, gates, host=host)
    except DriverError:
        raise
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        raise DriverError(
            f"remote landing infrastructure failed on {host}: {exc}"
        ) from exc


def _run_remote(
    repo: Path, commit: str, gates, *, host: str
) -> tuple[list[tuple[float, str]], Path]:
    from tools import trunk_loop

    tree = trunk_loop.checked(["git", "-C", str(repo), "rev-parse", commit + "^{tree}"])
    request = {
        "commit": commit,
        "tree": tree,
        "gates": gate_records(gates),
        "toolchains": sorted({name for gate in gates for name in gate.toolchains}),
    }
    args = SimpleNamespace(
        inventory=repo / "tools/cross_hosts.toml",
        host=host,
        commit=commit,
        cell=["landing"],
    )
    record_path = trunk_loop.start(args, landing=request, repo=repo)
    rc = trunk_loop.collect(record_path)
    record = json.loads(record_path.read_text(encoding="utf-8"))
    if record.get("state") != "collected":
        raise DriverError(f"remote landing completion unknown: {record_path}")
    result = record_path.parents[2] / "landing" / record["run_id"] / "receipt.json"
    if not result.is_file():
        raise DriverError(f"remote landing omitted its receipt: {record_path}")
    receipt = json.loads(result.read_text(encoding="utf-8"))
    admit_result(receipt, request)
    if rc:
        raise DriverError(f"remote terminal and gate receipt disagree: {record_path}")
    # Mac custody still protects the merge/push boundary after collection.
    from tools import molt_dev

    molt_dev._assert_integration_source(molt_dev.Git(repo), commit)
    return (
        [(float(seconds), str(command)) for seconds, command in receipt["seconds"]],
        result.parent,
    )


def execute(record: dict, output: Path) -> int:
    from tools import molt_dev as drv, trunk_loop
    from tools.molt_dev_gates import admit_selected_output_capacity, run_selected_gates
    from molt.dx import RunContext

    request = record["landing"]
    repo = Path(record["checkout"])
    git = drv.Git(repo)
    gates = decode_gates(request["gates"])
    seconds = []
    runtime = None
    result = {
        "schema": SCHEMA,
        "source_commit": request["commit"],
        "source_tree": request["tree"],
        "source_tree_state": "unknown",
        "request_sha256": request_digest(request),
        "status": "infrastructure",
        "passed": [],
        "seconds": seconds,
        "toolchains": {},
    }
    try:
        drv._assert_integration_source(git, request["commit"])
        if git.rev_parse("HEAD^{tree}") != request["tree"]:
            raise DriverError("remote landing source tree differs")
        config = GateConfig.load(repo / "tools/proof_plan.toml")
        env = dict(os.environ)
        env.pop(HOST_SETTING, None)
        prepared = config.prepare(
            gates,
            baseline=env,
            prepare_python=lambda baseline, needed: drv.prepare_gate_environment(
                repo, baseline, requires_python=needed
            ),
        )
        if prepared.deferred:
            raise DriverError(
                "remote host cannot admit all selected gates: "
                + "; ".join(item.diagnostic() for item in prepared.deferred)
            )
        outputs = config.cargo_outputs(prepared.environments, gates)
        capacity = admit_selected_output_capacity(outputs, prepared.environments)
        for environment in prepared.environments.values():
            RunContext(repo).materialize_environment(
                environment.environment, protected_python_root=environment.python_root
            )
        runtime = drv._gate_runtime(
            git,
            request["commit"],
            repo=repo,
            policy=(repo / "tools/proof_plan.toml").read_text(encoding="utf-8"),
            config=config,
            environments=prepared.environments,
            cargo_outputs=outputs,
            capacity_requirements=capacity,
            progress=drv._IntegrationProgress(),
            toolchains=prepared.fingerprints,
        )
        runtime = replace(
            runtime, receipt_path=output / "proof.json", output_dir=output
        )
        result["toolchains"] = prepared.fingerprints
        run_selected_gates(gates, runtime, seconds)
        if len(runtime.passed) != len(gates):
            raise DriverError("remote executor omitted selected gates")
        drv._assert_integration_source(git, request["commit"])
        result.update(
            status="success", source_tree_state="clean", passed=list(range(len(gates)))
        )
    except CandidateFailure as exc:
        result.update(status="failure", source_tree_state="clean", error=str(exc))
    except Exception as exc:
        result["error"] = f"{type(exc).__name__}: {exc}"
        # BaseException and unresolved owned children must never publish completion.
        if getattr(exc, "owned_process", None) is not None:
            raise
    trunk_loop.write_json(output / "receipt.json", result)
    return 0 if result["status"] == "success" else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    return execute(json.loads(args.record.read_text(encoding="utf-8")), args.output)


if __name__ == "__main__":
    raise SystemExit(main())
