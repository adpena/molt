#!/usr/bin/env python3
from __future__ import annotations

import shutil
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
TOOL_ROOT = ROOT / "tools" / "browser_asset_graph"


def main() -> int:
    npm = shutil.which("npm")
    if npm is None:
        raise SystemExit("browser-asset-parser: npm is required (Node >=18)")
    result = subprocess.run(
        [npm, "ci", "--ignore-scripts", "--prefix", str(TOOL_ROOT)],
        cwd=ROOT,
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(result.returncode)
    print("browser-asset-parser: ready (Acorn 8.15.0, lockfile exact)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
