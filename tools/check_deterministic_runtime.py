#!/usr/bin/env python3
"""Verify that a Molt-compiled binary produces deterministic output.

Builds a test program, runs it N times, and asserts all outputs are identical.

Usage:
    python tools/check_deterministic_runtime.py [--runs N] [--build-profile PROFILE] <source.py>
    python tools/check_deterministic_runtime.py --batch examples/*.py --runs 5

Exit codes:
    0 — all runs produced identical output
    1 — outputs differ across runs
    2 — build or execution error
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import harness_memory_guard  # noqa: E402
from tools.check_reproducible_build import resolve_corpus  # noqa: E402
from tools.proof_counts import fail_closed_proof_exit_code  # noqa: E402


def _extract_binary(build_json: dict) -> str | None:
    """Extract the binary path from build JSON, unwrapping data envelope."""
    data = build_json
    if "data" in build_json and isinstance(build_json["data"], dict):
        data = build_json["data"]
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return data[key]
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return data["build"][key]
    return None


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def build_program(
    source: str,
    profile: str = "dev",
    *,
    deterministic: bool = True,
    cache_dir: str | None = None,
    cwd: str | Path | None = None,
    hash_seed: int = 0,
) -> tuple[str | None, str, dict[str, object] | None]:
    """Build a Molt program. Returns (binary_path, error_msg).

    Returns (None, error) on failure instead of sys.exit().
    """
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env["PYTHONHASHSEED"] = str(hash_seed)
    if deterministic:
        env["MOLT_DETERMINISTIC"] = "1"
    else:
        env.pop("MOLT_DETERMINISTIC", None)
    if cache_dir is not None:
        env["MOLT_CACHE"] = cache_dir
    if cwd is not None:
        root = Path(cwd)
        env["MOLT_EXT_ROOT"] = str(root / "artifacts")
        env["MOLT_TARGET_ROOT"] = str(root / "target-root")
        env["CARGO_TARGET_DIR"] = str(root / "cargo-target")
        env["MOLT_BACKEND_DAEMON"] = "0"
        env["MOLT_BACKEND_DAEMON_SOCKET_DIR"] = str(root / "daemon-sockets")
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)

    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "--json",
        *(["--deterministic"] if deterministic else []),
        source,
    ]
    try:
        result = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            env=env,
            cwd=cwd,
            timeout=120,
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        return None, "build timed out", None

    if result.returncode != 0:
        return (
            None,
            f"build failed (exit {result.returncode}): {result.stderr[:1000]}",
            None,
        )

    stdout = result.stdout.strip()
    json_str = None
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            json_str = line
            break

    if json_str is None:
        try:
            build_info = json.loads(stdout)
        except json.JSONDecodeError as e:
            return None, f"invalid build JSON: {e}", None
    else:
        try:
            build_info = json.loads(json_str)
        except json.JSONDecodeError as e:
            return None, f"invalid build JSON: {e}", None

    binary = _extract_binary(build_info)
    if binary is None:
        return (
            None,
            f"no binary in build output (keys: {list(build_info.keys())})",
            build_info,
        )
    binary_path = Path(binary)
    if not binary_path.is_absolute() and cwd is not None:
        binary_path = Path(cwd) / binary_path
    if not binary_path.exists():
        return None, f"binary not found: {binary}", build_info

    return str(binary_path), "", build_info


def run_binary(
    binary: str,
    run_index: int,
    timeout: int = 60,
    *,
    deterministic: bool = True,
    cwd: str | Path | None = None,
) -> tuple[bytes, bytes, int | None]:
    """Run a binary. Returns (stdout, stderr, returncode). returncode=None on timeout."""
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = str(run_index)
    if deterministic:
        env["MOLT_DETERMINISTIC"] = "1"
    else:
        env.pop("MOLT_DETERMINISTIC", None)
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)

    try:
        result = harness_memory_guard.guarded_completed_process(
            [binary],
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=False,
            env=env,
            cwd=cwd,
            timeout=timeout,
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        return b"", b"", None

    return result.stdout, result.stderr, result.returncode


def check_determinism(
    source: str,
    runs: int,
    profile: str,
    timeout: int = 60,
    verbose: bool = False,
    deterministic_mode: bool = True,
) -> dict:
    """Check determinism for a single source file. Returns result dict."""
    result = {
        "source": source,
        "runs": runs,
        "deterministic": False,
        "status": "unknown",
        "mode": "deterministic" if deterministic_mode else "default",
        "profile": profile,
        "command": [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--profile",
            profile,
            *(["--deterministic"] if deterministic_mode else []),
            "--json",
            "<relocated-source>",
        ],
        "toolchain": {"python": sys.version},
    }

    if runs < 2:
        result["status"] = "error"
        result["error"] = "runs must be at least 2"
        return result

    if not Path(source).exists():
        result["status"] = "error"
        result["error"] = "source file not found"
        return result

    source_path = Path(source).resolve()
    outputs: list[tuple[bytes, bytes, int | None]] = []
    observations: list[dict[str, object]] = []
    for i in range(runs):
        with tempfile.TemporaryDirectory(prefix=f"runtime_repeat_{i}_") as run_root:
            relocated_source = Path(run_root) / source_path.name
            shutil.copyfile(source_path, relocated_source)
            cache = Path(run_root) / "cache"
            binary, error, build_receipt = build_program(
                str(relocated_source),
                profile,
                deterministic=deterministic_mode,
                cache_dir=str(cache),
                cwd=run_root,
                hash_seed=0,
            )
            if binary is None:
                result["status"] = "build_error"
                result["error"] = f"observation {i + 1}: {error}"
                return result
            binary_path = Path(binary)
            if not binary_path.is_absolute():
                binary_path = Path(run_root) / binary_path
            binary_hash = _sha256_file(binary_path)
            stdout, stderr, rc = run_binary(
                str(binary_path),
                i + 1,
                timeout,
                deterministic=deterministic_mode,
                cwd=run_root,
            )
            outputs.append((stdout, stderr, rc))
            digest = hashlib.sha256()
            digest.update(len(stdout).to_bytes(8, "big"))
            digest.update(stdout)
            digest.update(len(stderr).to_bytes(8, "big"))
            digest.update(stderr)
            digest.update((-1 if rc is None else rc).to_bytes(8, "big", signed=True))
            observations.append(
                {
                    "index": i + 1,
                    "logical_cwd": f"isolated-{i + 1}",
                    "source": source_path.name,
                    "binary_sha256": binary_hash,
                    "stdout_sha256": hashlib.sha256(stdout).hexdigest(),
                    "stderr_sha256": hashlib.sha256(stderr).hexdigest(),
                    "returncode": rc,
                    "observable_sha256": digest.hexdigest(),
                    "environment": {
                        "PYTHONHASHSEED": "0",
                        "MOLT_DETERMINISTIC": "1" if deterministic_mode else None,
                        "isolated_cache": True,
                        "isolated_artifact_root": True,
                        "isolated_target_root": True,
                        "backend_daemon": "disabled",
                    },
                    "build_receipt": build_receipt,
                }
            )
            if verbose:
                print(
                    f"  Observation {i + 1}: {digest.hexdigest()[:16]} "
                    f"(stdout={len(stdout)}, stderr={len(stderr)}) rc={rc}"
                )

    result["observations"] = observations
    if any(rc is None for _, _, rc in outputs):
        result["status"] = "timeout"
        result["error"] = "one or more runtime observations timed out"
        return result
    if any(rc != 0 for _, _, rc in outputs):
        result["status"] = "run_error"
        result["error"] = "one or more runtime observations returned non-zero"
        return result

    reference = outputs[0]
    all_match = True
    diff_details = []

    for i, observable in enumerate(outputs[1:], 2):
        stdout, stderr, rc = observable
        ref_stdout, ref_stderr, ref_rc = reference
        if observable != reference:
            all_match = False
            lines_ref = ref_stdout.splitlines()
            lines_cur = stdout.splitlines()
            first_diff_line = None
            for j, (lr, lc) in enumerate(zip(lines_ref, lines_cur)):
                if lr != lc:
                    first_diff_line = j + 1
                    break
            if first_diff_line is None and len(lines_ref) != len(lines_cur):
                first_diff_line = min(len(lines_ref), len(lines_cur)) + 1
            diff_details.append(
                {
                    "run": i,
                    "first_diff_line": first_diff_line,
                    "stdout_changed": stdout != ref_stdout,
                    "stderr_changed": stderr != ref_stderr,
                    "returncode": rc,
                    "reference_returncode": ref_rc,
                }
            )

    result["deterministic"] = all_match
    result["status"] = "pass" if all_match else "fail"
    result["observable_hash"] = observations[0]["observable_sha256"]
    result["stdout_hash"] = hashlib.sha256(reference[0]).hexdigest()
    result["stderr_hash"] = hashlib.sha256(reference[1]).hexdigest()
    result["returncode"] = reference[2]
    if diff_details:
        result["diffs"] = diff_details

    return result


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "source",
        nargs="?",
        help="Python source file to test (use --batch for multiple files)",
    )
    parser.add_argument(
        "--batch",
        nargs="+",
        metavar="SOURCE",
        help="Test multiple source files for determinism",
    )
    parser.add_argument(
        "--corpus",
        choices=("smoke", "full"),
        help="Repository-owned corpus from config/reproducibility_corpus.toml",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=3,
        help="Number of runs to compare (default: 3)",
    )
    parser.add_argument(
        "--build-profile",
        default="dev",
        help="Molt build profile (default: dev)",
    )
    parser.add_argument(
        "--mode",
        choices=("both", "default", "deterministic"),
        default="both",
        help="Runtime contract cells to prove (default: both)",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=60,
        help="Timeout in seconds per run (default: 60)",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
    )
    parser.add_argument(
        "--json-out",
        metavar="FILE",
        help="Write JSON results to FILE (for CI integration)",
    )
    args = parser.parse_args()
    if args.runs < 2:
        parser.error("--runs must be at least 2")

    selected_modes = sum(value is not None for value in (args.batch, args.corpus, args.source))
    if selected_modes > 1:
        parser.error("choose exactly one source, --batch, or --corpus")
    sources = (
        args.batch
        or (resolve_corpus(args.corpus) if args.corpus else None)
        or ([args.source] if args.source else [])
    )
    if not sources:
        parser.error("Either provide a source file or use --batch")
    modes = (
        [False, True]
        if args.mode == "both"
        else [args.mode == "deterministic"]
    )
    tasks = [(source, mode) for source in sources for mode in modes]
    started = time.monotonic()

    def run_cell(task: tuple[str, bool]) -> dict:
        source, deterministic_mode = task
        return check_determinism(
            source,
            args.runs,
            args.build_profile,
            args.timeout,
            args.verbose,
            deterministic_mode,
        )

    with ThreadPoolExecutor(max_workers=min(2, len(tasks))) as executor:
        results = list(executor.map(run_cell, tasks))
    passed = sum(result["status"] == "pass" for result in results)
    failed = sum(result["status"] == "fail" for result in results)
    errors = len(results) - passed - failed
    for result in results:
        label = f"{result['source']} [{result['mode']}]"
        if result["status"] == "pass":
            print(f"  PASS  {label} ({str(result['observable_hash'])[:16]})")
        elif result["status"] == "fail":
            print(f"  FAIL  {label}")
        else:
            print(f"  ERROR {label}: {result.get('error', 'unknown')}")
    payload = {
        "schema": "molt.deterministic-runtime-proof.v2",
        "status": (
            "success" if passed > 0 and failed == 0 and errors == 0 else "failure"
        ),
        "selected": len(tasks),
        "executed": passed + failed,
        "passed": passed,
        "failed": failed,
        "errors": errors,
        "runs_per_cell": args.runs,
        "profile": args.build_profile,
        "toolchain": {"python": sys.version},
        "elapsed_s": round(time.monotonic() - started, 3),
        "results": results,
    }
    if args.json_out:
        receipt = Path(args.json_out)
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    return fail_closed_proof_exit_code(
        executed=passed + failed,
        failed=failed,
        errors=errors,
    )


if __name__ == "__main__":
    sys.exit(main())
