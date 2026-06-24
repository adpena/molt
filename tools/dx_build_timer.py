#!/usr/bin/env python3
"""DX build-timing harness for the build-throughput arc (foundation/08_DX-buildspeed.md).

Measures wall-clock for the canonical backend-daemon build in well-defined
scenarios:
  - cold      : clean target dir, full build
  - inc-<file>: touch ONE file then rebuild (incremental)
  - test-lib  : `cargo test --lib --no-run` compile time after a touch

It drives `cargo` directly (NOT `molt build`) because the thing being optimised
is the cargo build of the backend crate(s) themselves. Each scenario is run N
times; we report min/median/max so noise from other agents is visible.

This tool never runs a compiled Molt binary, but every Cargo child still routes
through the shared memory guard because build throughput work must not bypass
process-tree custody.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools import harness_memory_guard  # noqa: E402


def _now() -> float:
    return time.perf_counter()


def _touch(path: Path) -> None:
    """Append + remove a trailing no-op comment line to force a content change.

    Pure `os.utime` (mtime bump) is enough for cargo's fingerprint, but a real
    content edit is the honest model of what an agent edit does (it invalidates
    the incremental-compilation cache for that file's codegen unit). We add then
    immediately remove a blank comment so the file content returns to identical
    bytes across runs (deterministic), while still changing mtime."""
    text = path.read_text()
    marker = "\n// dx_build_timer touch\n"
    if text.endswith(marker):
        path.write_text(text[: -len(marker)])
    else:
        path.write_text(text + marker)


def _output_text(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return str(value)


def _run_completed(
    cmd: list[str],
    env: dict[str, str],
    cwd: Path,
) -> tuple[harness_memory_guard.GuardedCompletedProcess, float]:
    start = _now()
    proc = harness_memory_guard.guarded_completed_process(
        cmd,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        prefix="MOLT_DX_BUILD",
    )
    elapsed = proc.elapsed_s if proc.elapsed_s is not None else _now() - start
    return proc, elapsed


def _run(cmd: list[str], env: dict[str, str], cwd: Path) -> tuple[int, float, str]:
    proc, elapsed = _run_completed(cmd, env, cwd)
    tail = "\n".join(_output_text(proc.stderr).splitlines()[-8:])
    return proc.returncode, elapsed, tail


def _build_cmd(args: argparse.Namespace, extra: list[str] | None = None) -> list[str]:
    cmd = [
        "cargo",
        "build",
        "--profile",
        args.profile,
        "-p",
        args.package,
        "--features",
        args.features,
    ]
    if extra:
        cmd.extend(extra)
    return cmd


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", default="release-fast")
    ap.add_argument("--package", default="molt-backend")
    ap.add_argument("--features", default="native-backend")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--target-dir", required=True, help="CARGO_TARGET_DIR to use")
    ap.add_argument(
        "--scenarios",
        nargs="+",
        default=[
            "cold",
            "inc-value_range",
            "inc-function_compiler",
            "inc-modules",
            "test-lib",
        ],
    )
    ap.add_argument("--json-out", default=None)
    ap.add_argument(
        "--cold-clean",
        action="store_true",
        help="rm -rf target dir before the cold scenario (true cold)",
    )
    args = ap.parse_args()

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = args.target_dir

    touch_files = {
        "value_range": REPO_ROOT / "runtime/molt-tir/src/tir/passes/value_range.rs",
        "function_compiler": REPO_ROOT
        / "runtime/molt-backend/src/native_backend/function_compiler.rs",
        "modules": REPO_ROOT / "runtime/molt-runtime/src/builtins/modules.rs",
        "gvn": REPO_ROOT / "runtime/molt-tir/src/tir/passes/gvn.rs",
    }

    results: dict[str, dict] = {}

    def measure(label: str, prep, cmd: list[str]) -> None:
        samples = []
        rc_last = 0
        tail_last = ""
        for i in range(args.runs):
            if prep:
                prep()
            rc, elapsed, tail = _run(cmd, env, REPO_ROOT)
            rc_last, tail_last = rc, tail
            samples.append(round(elapsed, 2))
            print(
                f"  [{label}] run {i + 1}/{args.runs}: {elapsed:.2f}s rc={rc}",
                flush=True,
            )
            if rc != 0:
                print(f"    FAILED:\n{tail}", flush=True)
                break
        results[label] = {
            "samples_sec": samples,
            "min": min(samples) if samples else None,
            "median": round(statistics.median(samples), 2) if samples else None,
            "max": max(samples) if samples else None,
            "rc": rc_last,
            "cmd": cmd,
            "stderr_tail": tail_last if rc_last != 0 else "",
        }

    # Ensure a warm baseline build exists first (so incremental scenarios are real).
    print("[dx] priming warm build ...", flush=True)
    rc, elapsed, tail = _run(_build_cmd(args), env, REPO_ROOT)
    print(f"[dx] prime build: {elapsed:.2f}s rc={rc}", flush=True)
    if rc != 0:
        print(f"[dx] prime FAILED:\n{tail}", file=sys.stderr)
        return 1

    for scen in args.scenarios:
        if scen == "cold":

            def cold_prep():
                if args.cold_clean:
                    td = Path(args.target_dir)
                    if td.exists():
                        shutil.rmtree(td)

            measure("cold", cold_prep, _build_cmd(args))
        elif scen.startswith("inc-"):
            key = scen[len("inc-") :]
            f = touch_files[key]
            measure(scen, (lambda f=f: _touch(f)), _build_cmd(args))
        elif scen == "test-lib":
            measure(
                "test-lib",
                (lambda: _touch(touch_files["value_range"])),
                _build_cmd(args, ["--tests", "--no-run"]),
            )
        else:
            print(f"unknown scenario: {scen}", file=sys.stderr)

    payload = {
        "meta": {
            "profile": args.profile,
            "package": args.package,
            "features": args.features,
            "runs": args.runs,
            "target_dir": args.target_dir,
            "cargo": _output_text(
                _run_completed(["cargo", "--version"], env, REPO_ROOT)[0].stdout
            ).strip(),
        },
        "results": results,
    }
    out = json.dumps(payload, indent=2)
    if args.json_out:
        Path(args.json_out).write_text(out + "\n")
        print(f"wrote {args.json_out}")
    print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
