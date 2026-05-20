import argparse
import datetime as dt
import json
import os
import sys
from pathlib import Path

import harness_memory_guard
import bench_wasm


def _resolve_bench(spec: str) -> str:
    path = Path(spec)
    if path.exists():
        return str(path)
    needle = spec if spec.endswith(".py") else f"{spec}.py"
    for bench in bench_wasm.BENCHMARKS:
        if Path(bench).name == needle:
            return bench
    raise SystemExit(f"Unknown benchmark: {spec}")


def _run_node_profile(
    *,
    env: dict[str, str],
    out_dir: Path,
    name: str,
    interval_us: int | None,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> bool:
    try:
        node_bin = bench_wasm.resolve_node_binary()
    except RuntimeError as exc:
        print(f"Node resolver error: {exc}", file=sys.stderr)
        return False
    cmd = [
        node_bin,
        "--no-warnings",
        "--no-wasm-tier-up",
        "--no-wasm-dynamic-tiering",
        "--wasm-num-compilation-tasks=1",
        "--cpu-prof",
        f"--cpu-prof-dir={out_dir}",
        f"--cpu-prof-name={name}",
    ]
    if interval_us is not None:
        cmd.append(f"--cpu-prof-interval={interval_us}")
    cmd.append("wasm/run_wasm.js")
    env = env.copy()
    env.setdefault("NODE_NO_WARNINGS", "1")
    try:
        res = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_BENCH",
            env=env,
            capture_output=True,
            text=True,
            limits=limits,
        )
    except OSError as exc:
        print(f"Failed to run node: {exc}", file=sys.stderr)
        return False
    if res.returncode != 0:
        err = res.stderr.strip() or res.stdout.strip()
        if err:
            print(f"WASM profile run failed: {err}", file=sys.stderr)
        return False
    return True


def main() -> None:
    parser = argparse.ArgumentParser(description="Profile a Molt WASM bench with Node.")
    parser.add_argument(
        "--bench",
        default="tests/benchmarks/bench_sum.py",
        help="Benchmark path or name (default: bench_sum).",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=None,
        help="Output directory for profiles (default: logs/wasm_profile/<timestamp>).",
    )
    parser.add_argument(
        "--linked",
        action="store_true",
        help="Attempt single-module wasm linking with wasm-ld when available.",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=1,
        help="Number of profile runs to capture (default: 1).",
    )
    parser.add_argument(
        "--cpu-prof-interval",
        type=int,
        default=None,
        help="Sampling interval in microseconds for node --cpu-prof.",
    )
    args = parser.parse_args()

    bench = _resolve_bench(args.bench)
    bench_name = Path(bench).stem
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%d_%H%M%S")
    out_dir = args.out_dir or (Path("logs") / "wasm_profile" / stamp)
    out_dir.mkdir(parents=True, exist_ok=True)

    if args.linked:
        os.environ["MOLT_WASM_LINK"] = "1"
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")

    if not bench_wasm.build_runtime_wasm(
        reloc=False,
        output=bench_wasm.RUNTIME_WASM,
        tty=False,
        log=None,
        limits=limits,
    ):
        sys.exit(1)
    if args.linked and not bench_wasm.build_runtime_wasm(
        reloc=True,
        output=bench_wasm.RUNTIME_WASM_RELOC,
        tty=False,
        log=None,
        limits=limits,
    ):
        print(
            "Relocatable runtime build failed; falling back to non-linked wasm runs.",
            file=sys.stderr,
        )

    wasm_binary = bench_wasm.prepare_wasm_binary(
        bench,
        require_linked=args.linked,
        tty=False,
        log=None,
        keep_temp=False,
        limits=limits,
    )
    if wasm_binary is None:
        sys.exit(1)

    profiles: list[str] = []
    ok = True
    try:
        with harness_memory_guard.repo_process_sentinel(
            repo_root=bench_wasm._repo_root(),
            artifact_root=out_dir,
            label="wasm_profile",
            limits=limits,
        ):
            for idx in range(args.runs):
                suffix = f"_run{idx + 1}" if args.runs > 1 else ""
                profile_name = f"{bench_name}{suffix}.cpuprofile"
                if not _run_node_profile(
                    env=wasm_binary.run_env,
                    out_dir=out_dir,
                    name=profile_name,
                    interval_us=args.cpu_prof_interval,
                    limits=limits,
                ):
                    ok = False
                    break
                profiles.append(str(out_dir / profile_name))
    finally:
        wasm_binary.temp_dir.cleanup()

    manifest = {
        "schema_version": 1,
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": bench_wasm._git_rev(),
        "bench": bench,
        "bench_name": bench_name,
        "linked_requested": args.linked,
        "linked_used": wasm_binary.linked_used,
        "runs": args.runs,
        "cpu_prof_interval_us": args.cpu_prof_interval,
        "profiles": profiles,
        "molt_wasm_size_kb": wasm_binary.size_kb,
        "molt_wasm_build_s": wasm_binary.build_s,
        "memory_guard": harness_memory_guard.limits_summary(limits),
    }
    manifest_path = out_dir / "profile_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")

    if not ok:
        sys.exit(1)
    print(f"WASM profiles written to {out_dir}")


if __name__ == "__main__":
    main()
