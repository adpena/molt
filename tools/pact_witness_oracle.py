#!/usr/bin/env python3
from __future__ import annotations

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
    subprocess.run(args, cwd=cwd, check=True)


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
