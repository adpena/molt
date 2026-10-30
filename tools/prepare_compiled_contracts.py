"""Prepare the real compiled-contract inputs and existing WASM host recipe.

One proof-plan cold window owns every child. The following end-to-end tests
still call ``molt run`` and independently admit the source-selected runtime
products. WASM builds use the tests' source and command authorities, so final IR
selects the runtime products. No test oracle or guest executes here.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from time import monotonic
from typing import assert_never

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

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
from molt.dx import artifact_root  # noqa: E402
from tests.resource_enforcement_witnesses import (  # noqa: E402
    ResourceWitness,
    child_resource_witness,
    feedback_destination_witness,
)
from tests.wasm_linked_runner import (  # noqa: E402
    WasmTestBuildCommand,
    wasm_linked_build_command,
)
from tests.test_wasm_split_runtime import wasm_split_build_command  # noqa: E402
from tests.wasm_runtime_heavy_witnesses import (  # noqa: E402
    HEAVY_WASM_WITNESSES,
    HeavyWasmWitness,
    syntax_error_target_arguments,
)
from molt.cli.script_commands import _wasm_run_build_args  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_PROOF")


def native_build_environment(
    source: Path, witness: ResourceWitness, *, compiler_root: Path
) -> tuple[Path, ProcessEnvironment]:
    """Use the original run path's project, environment and policy authorities."""
    return run_build_environment(
        source, witness.environment, compiler_root=compiler_root
    )


def run_build_environment(
    source: Path, overrides: dict[str, str], *, compiler_root: Path
) -> tuple[Path, ProcessEnvironment]:
    project_root = _find_project_root(source.resolve())
    environment = _base_env(project_root, source.resolve(), molt_root=compiler_root)
    environment.update(overrides)
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


def wasm_build_command(
    root: Path, witness: HeavyWasmWitness, source: Path, output: Path
) -> WasmTestBuildCommand:
    """Project each test's real argument/environment authority without a guest."""
    match witness.layout:
        case "linked":
            return wasm_linked_build_command(
                root, source, output, extra_args=[*witness.extra_args, "--json"]
            )
        case "split":
            return wasm_split_build_command(
                source, output, extra_args=[*witness.extra_args, "--json"]
            )
        case "run":
            # Keep the CLI run wrapper's guest default and linked requirement.
            # The shared target/version arguments are also used by its oracle.
            arguments = _wasm_run_build_args(syntax_error_target_arguments("wasm"))
            cwd, environment = run_build_environment(source, {}, compiler_root=root)
            return WasmTestBuildCommand(
                (
                    sys.executable,
                    "-m",
                    "molt.cli",
                    "build",
                    "--json",
                    *arguments,
                    "--out-dir",
                    str(output),
                    str(source),
                ),
                cwd,
                environment,
                None,  # The CLI build contract supplies the default output name.
            )
    assert_never(witness.layout)


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
    body = rows["python.unit.compiled-contracts.macos"]
    if not set(plan.required_toolchains(body)) <= set(plan.required_toolchains(owner)):
        raise ValueError(
            "compiled preparation does not declare the WASM toolchain closure"
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

    # Build the actual selected source/layout coordinates. The provider derives
    # and deduplicates exact runtime generations from final IR, never this list.
    # Products live outside guard scratch, so diagnostics can use their exact
    # manifest/module after the temporary source project has been removed.
    products = artifact_root(root) / "build" / "compiled-contracts"
    products.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="molt-wasm-prepare-") as temporary:
        for witness in HEAVY_WASM_WITNESSES:
            if witness.slow:  # The owning body already selects `not slow`.
                continue
            name = f"wasm-{witness.name}-{witness.layout}"
            sources = Path(temporary) / name
            sources.mkdir()
            source = witness.materialize(root, sources)
            evidence = Path(tempfile.mkdtemp(prefix=f"{name}-", dir=output))
            shutil.copytree(sources, evidence / "sources")
            product = Path(tempfile.mkdtemp(prefix=f"{name}-", dir=products))
            command = wasm_build_command(root, witness, source, product)
            (evidence / "command.json").write_text(
                json.dumps(
                    {
                        "argv": command.argv,
                        "cwd": str(command.cwd),
                        "source": str(source),
                        "retained_sources": str(evidence / "sources"),
                        "products": str(product),
                    },
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
            )
            result = _COMMANDS.run(
                command.argv,
                cwd=command.cwd,
                env=command.environment,
                timeout=remaining(command.argv),
                check=True,
                capture_output=True,
                text=True,
                encoding="utf-8",
                stdout_capture_path=evidence / "build.json",
                stderr_capture_path=evidence / "stderr.log",
            )
            contract, error = _parse_wrapper_build_contract_payload(
                json.loads(result.stdout), json_output=False, command="build"
            )
            if error is not None or contract is None:
                raise ValueError(f"{name}: WASM preparation produced no build contract")
            if not contract.consumer_output.is_file():
                raise ValueError(f"{name}: WASM preparation output is missing")
            if command.output is not None and not command.output.is_file():
                raise ValueError(f"{name}: WASM preparation layout output is missing")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    output = args.out if args.out.is_absolute() else _REPO_ROOT / args.out
    prepare(proof_plan.ProofPlan.load(), output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
