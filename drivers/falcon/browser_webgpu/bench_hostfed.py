from __future__ import annotations

import argparse
import datetime as dt
import json
from pathlib import Path


def _load_molt_bench_wasm():
    import tools.bench_wasm as bench_wasm

    return bench_wasm


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Falcon split-runtime host-fed benchmark driver for browser WebGPU"
    )
    parser.add_argument(
        "--target-root",
        type=Path,
        required=True,
        help="Falcon application root containing dist/browser_split and tmp host-fed bundles.",
    )
    parser.add_argument(
        "--phase",
        action="append",
        choices=("init_only", "init_plus_1_token"),
        dest="phases",
        help="Phase(s) to run. Defaults to both in order.",
    )
    parser.add_argument(
        "--phase-timeout-s",
        type=float,
        default=None,
        help="Optional per-phase timeout in seconds.",
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        default=Path("bench/results/falcon/browser_webgpu/falcon_split_runtime.json"),
        help="Output JSON artifact path.",
    )
    args = parser.parse_args()

    if args.phase_timeout_s is not None and args.phase_timeout_s <= 0:
        raise SystemExit("--phase-timeout-s must be > 0")

    target_root = args.target_root.resolve()
    artifact_dir = target_root / "dist" / "browser_split"
    init_calls = (
        target_root / "tmp" / "falcon_realdata_hostfed" / "calls_init_only.json"
    )
    token_calls = target_root / "tmp" / "falcon_realdata_hostfed" / "calls.json"
    phases = tuple(args.phases or ("init_only", "init_plus_1_token"))
    bench_wasm = _load_molt_bench_wasm()
    runner_cmd = [
        "node",
        str(Path(__file__).resolve().parents[3] / "wasm" / "run_wasm.js"),
    ]

    phase_results = []
    for phase, calls_path in (
        ("init_only", init_calls),
        ("init_plus_1_token", token_calls),
    ):
        if phase not in phases:
            continue
        phase_results.append(
            bench_wasm._run_hostfed_call_bundle(
                label=phase,
                app_wasm=artifact_dir / "app.wasm",
                runtime_wasm=artifact_dir / "molt_runtime.wasm",
                calls_path=calls_path,
                runner_cmd=runner_cmd,
                runner_name="node",
                log=None,
                timeout_s=args.phase_timeout_s,
            )
        )

    payload = {
        "schema_version": 1,
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "benchmark": "falcon_split_hostfed",
        "driver": "molt.falcon.browser_webgpu",
        "target_root": str(target_root),
        "artifact_dir": str(artifact_dir),
        "runner": "node",
        "runner_cmd": runner_cmd,
        "phase_results": phase_results,
    }
    args.json_out.parent.mkdir(parents=True, exist_ok=True)
    args.json_out.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(args.json_out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
