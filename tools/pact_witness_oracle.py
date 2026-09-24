#!/usr/bin/env python3
"""CPython-only Pact Kernel A oracle sanity lane: regenerate the fixture and
reference pair and prove check_parity.py against them, inside the attested
locked environment of the pact-witness dependency group."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import sys
import tempfile

from molt.cli.source_build_environment import source_build_environment
from molt.dx import proof_scratch_root
from molt.scientific_stack_versions import PACT_WITNESS_DEPENDENCY_GROUP

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

ROOT = Path(__file__).resolve().parents[1]
KERNEL_ROOT = ROOT / "collab" / "pact" / "pact_witness_kernel"


def _run(args: list[str], *, cwd: Path) -> None:
    print(f"+ {' '.join(args)}", flush=True)
    env = os.environ.copy()
    # Same oracle determinism pin as tools/pact_witness_acceptance.py
    # `_prepare_reference_oracle` (ONE oracle numerics authority): generate on
    # the numpy wheel's baseline dispatch tier. Mask-proof + rationale in
    # docs/agent/E1_PARITY_FEASIBILITY.md (measured bitwise no-op on the
    # acceptance host; removes oracle host-variance only).
    env.setdefault("NPY_DISABLE_CPU_FEATURES", "X86_V3")
    _COMMANDS.run(args, cwd=cwd, check=True, env=env)


def main() -> int:
    environment = source_build_environment(ROOT, PACT_WITNESS_DEPENDENCY_GROUP)
    if not environment.active:
        raise SystemExit(
            "pact witness oracle requires its prepared locked interpreter; "
            "use proof_queue.py pact-witness-oracle"
        )
    scratch = proof_scratch_root(ROOT)
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="pact_witness_oracle_", dir=scratch) as raw:
        work = Path(raw)
        for name in ("make_fixture.py", "field_solve.py", "check_parity.py"):
            shutil.copy2(KERNEL_ROOT / name, work / name)

        _run([sys.executable, "make_fixture.py"], cwd=work)
        _run([sys.executable, "field_solve.py", "lstar_sample.npz"], cwd=work)
        _run([sys.executable, "check_parity.py", "reference_outputs.npz"], cwd=work)

    print("pact witness oracle parity PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
