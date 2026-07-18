#!/usr/bin/env python3
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import time


TOOLS_ROOT = Path(__file__).resolve().parent
REPO_ROOT = TOOLS_ROOT.parent
SRC_ROOT = REPO_ROOT / "src"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

import harness_memory_guard  # noqa: E402
from molt.cli.atomic_io import _atomic_write_json  # noqa: E402
from wasm_metrics import wasm_metrics  # noqa: E402


def _load_wasm_link():
    path = TOOLS_ROOT / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link_cache_bench", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load wasm linker authority from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _string_sequence_sha256(values: list[str]) -> str:
    hasher = hashlib.sha256()
    for value in values:
        hasher.update(value.encode("utf-8"))
        hasher.update(b"\0")
    return hasher.hexdigest()


def _rss_payload(value: object | None) -> dict[str, object] | None:
    if value is None:
        return None
    rss_kb = getattr(value, "rss_kb", None)
    if not isinstance(rss_kb, int):
        return None
    payload: dict[str, object] = {"rss_kb": rss_kb}
    scope = getattr(value, "scope", None)
    if isinstance(scope, str):
        payload["scope"] = scope
    return payload


def _worker_main(args: argparse.Namespace) -> int:
    runtime = args.runtime.expanduser().resolve()
    scanner = args.scanner.expanduser().resolve()
    cache_dir = args.cache_dir.expanduser().resolve()
    scratch = args.scratch_dir.expanduser().resolve()
    session_target = args.session_target.expanduser().resolve()
    output = args.output.expanduser().resolve()
    if not runtime.is_file():
        raise SystemExit(f"runtime artifact not found: {runtime}")
    if not scanner.is_file():
        raise SystemExit(f"WASM facts scanner not found: {scanner}")
    if scratch.exists() or session_target.exists():
        raise SystemExit("worker scratch and target roots must not already exist")

    os.environ["MOLT_CACHE"] = os.fspath(cache_dir)
    os.environ["CARGO_TARGET_DIR"] = os.fspath(session_target)
    scratch.mkdir(parents=True)
    session_target.mkdir(parents=True)
    started_at = datetime.now(timezone.utc).isoformat()
    wasm_link = _load_wasm_link()
    runtime_data = runtime.read_bytes()
    required_exports = wasm_link._canonical_split_runtime_required_exports(runtime_data)
    facts_metrics: dict[str, float] = {}
    provider = wasm_link._make_rust_wasm_facts_provider(scanner, scratch, facts_metrics)
    cache_metrics: dict[str, int | float] = {}
    started = time.perf_counter()
    result = wasm_link._tree_shake_runtime(
        runtime_data,
        required_exports,
        facts_provider=provider,
        operation_counts=cache_metrics,
    )
    wall_s = max(0.0, time.perf_counter() - started)
    cache_metrics.update(facts_metrics)
    exports = sorted(wasm_link._collect_exports(result))
    payload = {
        "schema_version": 1,
        "session": args.worker_session,
        "pid": os.getpid(),
        "started_at": started_at,
        "cargo_target_dir": os.fspath(session_target),
        "scratch_dir": os.fspath(scratch),
        "wall_s": round(wall_s, 6),
        "bytes": len(result),
        "sha256": _sha256(result),
        "sections": wasm_metrics(result).get("sections", {}),
        "export_count": len(exports),
        "exports_sha256": _string_sequence_sha256(exports),
        "telemetry": cache_metrics,
    }
    _atomic_write_json(output, payload, indent=2, sort_keys=True)
    print(
        json.dumps(
            {
                "session": args.worker_session,
                "pid": os.getpid(),
                "wall_s": payload["wall_s"],
                "cache_hits": cache_metrics.get("runtime_tree_shake_cache_hits", 0),
                "cache_misses": cache_metrics.get("runtime_tree_shake_cache_misses", 0),
            },
            sort_keys=True,
        )
    )
    return 0


def _worker_command(
    args: argparse.Namespace,
    *,
    session: str,
    scratch: Path,
    target: Path,
    output: Path,
) -> list[str]:
    return [
        sys.executable,
        os.fspath(Path(__file__).resolve()),
        "--runtime",
        os.fspath(args.runtime.expanduser().resolve()),
        "--scanner",
        os.fspath(args.scanner.expanduser().resolve()),
        "--cache-dir",
        os.fspath(args.cache_dir.expanduser().resolve()),
        "--scratch-dir",
        os.fspath(scratch),
        "--session-target",
        os.fspath(target),
        "--output",
        os.fspath(output),
        "--worker-session",
        session,
    ]


def _run_worker(
    args: argparse.Namespace,
    *,
    session: str,
    scratch: Path,
    target: Path,
    output: Path,
) -> dict[str, object]:
    command = _worker_command(
        args,
        session=session,
        scratch=scratch,
        target=target,
        output=output,
    )
    env = os.environ.copy()
    env["MOLT_CACHE"] = os.fspath(args.cache_dir.expanduser().resolve())
    env["CARGO_TARGET_DIR"] = os.fspath(target)
    guarded = harness_memory_guard.guarded_completed_process(
        command,
        prefix="MOLT_BENCH",
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=420,
        progress_label=f"wasm-link-cache-{session}",
    )
    if guarded.returncode != 0:
        raise RuntimeError(
            f"WASM linker cache worker {session} failed rc={guarded.returncode}:\n"
            f"{guarded.stderr}"
        )
    payload = json.loads(output.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise RuntimeError(f"worker {session} emitted invalid evidence")
    child = guarded.child_process
    if child is None:
        raise RuntimeError(f"worker {session} guard omitted child-process custody")
    payload["guard"] = {
        "guarded_root_pid": child.pid,
        "guarded_root_started_at": child.started_at,
        "worker_pid": payload.get("pid"),
        "worker_started_at": payload.get("started_at"),
        "elapsed_s": guarded.elapsed_s,
        "peak_process_rss": _rss_payload(guarded.peak),
        "peak_process_tree_rss": _rss_payload(guarded.peak_total),
    }
    return payload


def _controller_main(args: argparse.Namespace) -> int:
    runtime = args.runtime.expanduser().resolve()
    scanner = args.scanner.expanduser().resolve()
    cache_dir = args.cache_dir.expanduser().resolve()
    scratch_dir = args.scratch_dir.expanduser().resolve()
    if not runtime.is_file():
        raise SystemExit(f"runtime artifact not found: {runtime}")
    if not scanner.is_file():
        raise SystemExit(f"WASM facts scanner not found: {scanner}")
    if cache_dir.exists() or scratch_dir.exists():
        raise SystemExit("cache and scratch benchmark roots must not already exist")
    cache_dir.mkdir(parents=True)
    scratch_dir.mkdir(parents=True)

    cold = _run_worker(
        args,
        session="cold",
        scratch=scratch_dir / "session-a-scratch",
        target=scratch_dir / "session-a-target",
        output=scratch_dir / "session-a.json",
    )
    warm = _run_worker(
        args,
        session="cross-session-hit",
        scratch=scratch_dir / "session-b-scratch",
        target=scratch_dir / "session-b-target",
        output=scratch_dir / "session-b.json",
    )
    cold_telemetry = cold.get("telemetry", {})
    warm_telemetry = warm.get("telemetry", {})
    if not isinstance(cold_telemetry, dict) or not isinstance(warm_telemetry, dict):
        raise SystemExit("worker telemetry is not an object")
    parity = {
        field: cold.get(field) == warm.get(field)
        for field in ("bytes", "sha256", "sections", "export_count", "exports_sha256")
    }
    cold_wall_s = float(cold["wall_s"])
    warm_wall_s = float(warm["wall_s"])
    process_isolation = {
        "distinct_pids": cold.get("pid") != warm.get("pid"),
        "distinct_process_starts": cold.get("started_at") != warm.get("started_at"),
        "distinct_cargo_targets": cold.get("cargo_target_dir")
        != warm.get("cargo_target_dir"),
        "distinct_scratch_roots": cold.get("scratch_dir") != warm.get("scratch_dir"),
    }
    evidence = {
        "schema_version": 2,
        "runtime": os.fspath(runtime),
        "runtime_bytes": runtime.stat().st_size,
        "scanner": os.fspath(scanner),
        "cache_dir": os.fspath(cache_dir),
        "cold": cold,
        "cross_session_hit": warm,
        "wall_speedup": round(cold_wall_s / warm_wall_s, 6)
        if warm_wall_s > 0.0
        else None,
        "wall_saved_s": round(cold_wall_s - warm_wall_s, 6),
        "parity": parity,
        "process_isolation": process_isolation,
    }
    if not all(parity.values()):
        raise SystemExit("cross-session WASM linker cache artifact parity failed")
    if not all(process_isolation.values()):
        raise SystemExit("WASM linker cache benchmark did not isolate worker processes")
    if cold_telemetry.get("runtime_tree_shake_cache_misses") != 1:
        raise SystemExit("cold worker did not publish exactly one cache miss")
    if cold_telemetry.get("runtime_tree_shake_cache_hits", 0) != 0:
        raise SystemExit("cold worker unexpectedly hit the shared cache")
    if warm_telemetry.get("runtime_tree_shake_cache_hits") != 1:
        raise SystemExit("fresh worker B did not record exactly one disk cache hit")
    if warm_telemetry.get("runtime_tree_shake_cache_misses", 0) != 0:
        raise SystemExit("fresh worker B unexpectedly missed the shared cache")
    if warm_telemetry.get("wasm_facts_scan_calls", 0) != 0:
        raise SystemExit("fresh worker B repeated WASM facts scanning before cache hit")
    output = args.output.expanduser().resolve()
    _atomic_write_json(output, evidence, indent=2, sort_keys=True)
    print(
        json.dumps(
            {
                "evidence": os.fspath(output),
                "cold_wall_s": cold["wall_s"],
                "cross_session_hit_wall_s": warm["wall_s"],
                "wall_speedup": evidence["wall_speedup"],
                "wall_saved_s": evidence["wall_saved_s"],
                "parity": parity,
                "process_isolation": process_isolation,
                "cold_peak_tree_rss": cold["guard"]["peak_process_tree_rss"],
                "warm_peak_tree_rss": warm["guard"]["peak_process_tree_rss"],
            },
            sort_keys=True,
        )
    )
    return 0


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Benchmark cold versus process-isolated shared WASM linker cache reuse "
            "and attest byte, section, export, wall-time, and RSS parity."
        )
    )
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--scanner", type=Path, required=True)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--scratch-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--worker-session", default="", help=argparse.SUPPRESS)
    parser.add_argument("--session-target", type=Path, help=argparse.SUPPRESS)
    return parser.parse_args()


def main() -> int:
    args = _parse_args()
    if args.worker_session:
        if args.session_target is None:
            raise SystemExit("worker mode requires --session-target")
        return _worker_main(args)
    return _controller_main(args)


if __name__ == "__main__":
    raise SystemExit(main())
