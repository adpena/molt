"""Prepare the real native resource witnesses and existing WASM host recipe.

One proof-plan cold window owns every child. The following end-to-end tests
still call ``molt run`` and independently admit the source-selected runtime
products. This prepares no WASM application runtime and runs no test oracle.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from time import monotonic

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402
from tools import proof_plan  # noqa: E402
from molt.cargo_execution_policy import proof_command_timeout_seconds  # noqa: E402
from molt.cli.build_inputs import _collect_env_overrides  # noqa: E402
from molt.cli.env_paths import _base_env  # noqa: E402
from molt.cli.project_roots import _find_project_root  # noqa: E402
from molt.cli.script_commands import _apply_run_capability_policy  # noqa: E402
from molt.cli.wrapper_build import _parse_wrapper_build_contract_payload  # noqa: E402
from molt.process_environment import ProcessEnvironment  # noqa: E402
from tests.resource_enforcement_witnesses import (  # noqa: E402
    ResourceWitness,
    child_resource_witness,
    feedback_destination_witness,
)

_COMMANDS = CommandExecutor.for_file(__file__)


def native_build_environment(
    source: Path, witness: ResourceWitness, *, compiler_root: Path
) -> tuple[Path, ProcessEnvironment]:
    """Use the original run path's project, environment and policy authorities."""
    project_root = _find_project_root(source.resolve())
    environment = _base_env(project_root, source.resolve(), molt_root=compiler_root)
    environment.update(witness.environment)
    environment.update(_collect_env_overrides(str(source)))
    error = _apply_run_capability_policy(
        environment,
        capabilities=None,
        capability_manifest=None,
        require_signed_manifest=False,
    )
    if error is not None:
        raise ValueError(error)
    return project_root, ProcessEnvironment(environment)


def prepare(plan: proof_plan.ProofPlan, output: Path) -> None:
    rows = {command.id: command for command in plan.commands}
    owner = rows["python.unit.compiled-contracts.prepare.macos"]
    host = rows["wasm.build.host"]
    if host.dependencies:
        raise ValueError(
            "compiled preparation requires the primitive host build recipe"
        )
    if not set(plan.required_toolchains(host)) <= set(plan.required_toolchains(owner)):
        raise ValueError(
            "compiled preparation does not declare the host toolchain closure"
        )
    budget = proof_command_timeout_seconds(os.environ)
    if budget != owner.data["timeout_seconds"]:
        raise ValueError("compiled preparation requires its exact owning proof budget")
    assert budget is not None
    deadline = monotonic() + budget
    root = plan.path.parent.parent
    output.mkdir(parents=True, exist_ok=True)

    def remaining(argv: tuple[str, ...]) -> float:
        seconds = deadline - monotonic()
        if seconds <= 0:
            raise subprocess.TimeoutExpired(argv, budget)
        return seconds

    # Project the existing host command, including its environment and cwd.
    # Do not recursively execute another proof plan or copy its Cargo recipe.
    host_argv = proof_plan.resolved_command_argv(host)
    host_environment, _ = proof_plan._command_environment(
        plan, host, int(budget), environment=os.environ, memory_share=None
    )
    _COMMANDS.run(
        host_argv,
        cwd=root / str(host.data.get("cwd", ".")),
        env=host_environment,
        timeout=remaining(host_argv),
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        stdout_capture_path=output / "host.stdout.log",
        stderr_capture_path=output / "host.stderr.log",
    )

    # Match the witnesses' temporary-file project context. Keeping these inputs
    # under the checkout would change project/configuration discovery.
    with tempfile.TemporaryDirectory(prefix="molt-resource-prepare-") as temporary:
        scratch = Path(temporary)
        witnesses = (
            ("child-policy", child_resource_witness(sys.executable)),
            (
                "feedback",
                feedback_destination_witness(scratch / "unauthorized-feedback.json"),
            ),
        )
        for name, witness in witnesses:
            source = scratch / f"{name}.py"
            source.write_text(witness.source, encoding="utf-8")
            (output / f"{name}.py").write_text(witness.source, encoding="utf-8")
            cwd, environment = native_build_environment(
                source, witness, compiler_root=root
            )
            # These are the native run wrapper's default build arguments.
            # In particular, do not replace its guest default with --profile dev.
            argv = (sys.executable, "-m", "molt.cli", "build", "--json", str(source))
            result = _COMMANDS.run(
                argv,
                cwd=cwd,
                env=environment,
                timeout=remaining(argv),
                check=True,
                capture_output=True,
                text=True,
                encoding="utf-8",
                stdout_capture_path=output / f"{name}.build.json",
                stderr_capture_path=output / f"{name}.stderr.log",
            )
            contract, error = _parse_wrapper_build_contract_payload(
                json.loads(result.stdout), json_output=False, command="build"
            )
            if error is not None or contract is None:
                raise ValueError(
                    f"{name}: native preparation produced no build contract"
                )
            if not contract.consumer_output.is_file():
                raise ValueError(f"{name}: native preparation output is missing")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    output = args.out if args.out.is_absolute() else ROOT / args.out
    prepare(proof_plan.ProofPlan.load(), output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
