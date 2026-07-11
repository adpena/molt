#!/usr/bin/env python3
from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
KERNEL_ROOT = ROOT / "collab" / "pact" / "pact_witness_kernel"
TMP_ROOT = ROOT / "tmp"


def _run(args: list[str], *, cwd: Path) -> None:
    print(f"+ {' '.join(args)}", flush=True)
    env = os.environ.copy()
    # Same oracle determinism pin as tools/pact_witness_acceptance.py
    # `_prepare_reference_oracle` (ONE oracle numerics authority): generate on
    # the numpy wheel's baseline dispatch tier. Mask-proof + rationale in
    # docs/agent/E1_PARITY_FEASIBILITY.md (measured bitwise no-op on the
    # acceptance host; removes oracle host-variance only).
    env.setdefault("NPY_DISABLE_CPU_FEATURES", "X86_V3")
    subprocess.run(args, cwd=cwd, check=True, env=env)


def main() -> int:
    TMP_ROOT.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="pact_witness_oracle_", dir=TMP_ROOT
    ) as raw:
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
