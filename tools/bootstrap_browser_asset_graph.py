#!/usr/bin/env python3
from __future__ import annotations

import json
import shutil
from pathlib import Path

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)


ROOT = Path(__file__).resolve().parents[1]
TOOL_ROOT = ROOT / "tools" / "browser_asset_graph"


def main() -> int:
    npm = shutil.which("npm")
    if npm is None:
        raise SystemExit("browser-asset-parser: npm is required (Node >=18)")
    result = _COMMANDS.run(
        [npm, "ci", "--ignore-scripts", "--prefix", str(TOOL_ROOT)],
        cwd=ROOT,
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(result.returncode)
    installed = json.loads(
        (TOOL_ROOT / "node_modules" / "acorn" / "package.json").read_text(
            encoding="utf-8"
        )
    )
    print(f"browser-asset-parser: ready (Acorn {installed['version']}, lockfile exact)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
