#!/usr/bin/env python3
from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path


def _find_tool(names: list[str]) -> str | None:
    for name in names:
        path = shutil.which(name)
        if path:
            return path
    return None


def _run_wasm_ld(wasm_ld: str, runtime: Path, output: Path, linked: Path) -> int:
    cmd = [
        wasm_ld,
        "--no-entry",
        "--allow-undefined",
        "--export=molt_main",
        "--export=molt_memory",
        "--export=molt_table",
        "--export=molt_call_indirect1",
        "-o",
        str(linked),
        str(runtime),
        str(output),
    ]
    res = subprocess.run(cmd, capture_output=True, text=True)
    if res.returncode != 0:
        err = res.stderr.strip() or res.stdout.strip()
        if err:
            print(err, file=sys.stderr)
    return res.returncode


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Attempt to link Molt output/runtime into a single WASM module.",
    )
    parser.add_argument("--runtime", type=Path, default=Path("wasm/molt_runtime.wasm"))
    parser.add_argument("--input", type=Path, default=Path("output.wasm"))
    parser.add_argument("--output", type=Path, default=Path("output_linked.wasm"))
    args = parser.parse_args()

    runtime = args.runtime
    output = args.input
    linked = args.output

    if not runtime.exists():
        print(f"Runtime wasm not found: {runtime}", file=sys.stderr)
        return 1
    if not output.exists():
        print(f"Output wasm not found: {output}", file=sys.stderr)
        return 1
    linked.parent.mkdir(parents=True, exist_ok=True)
    if linked.exists():
        linked.unlink()

    wasm_ld = _find_tool(["wasm-ld"])
    if not wasm_ld:
        print(
            "wasm-ld not found; install LLVM to enable single-module linking.",
            file=sys.stderr,
        )
        return 1

    return _run_wasm_ld(wasm_ld, runtime, output, linked)


if __name__ == "__main__":
    raise SystemExit(main())
