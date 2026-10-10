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
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from molt.cargo_execution_policy import default_nested_process_timeout_seconds  # noqa: E402
from tools import harness_memory_guard  # noqa: E402
from molt.temporary_artifacts import OwnedTemporaryDirectory  # noqa: E402
from tools.check_reproducible_build import (  # noqa: E402
    _launch_evidence,
    extract_artifact_path,
    resolve_corpus,
)
from tools.proof_counts import fail_closed_proof_exit_code  # noqa: E402


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
    build_timeout: float | None = None,
) -> tuple[str | None, str, dict[str, object] | None, dict[str, object]]:
    """Return binary, error, build JSON and evidence from the actual launch.

    Every observation shares the compiler build (the Cargo target and
    toolchain roots in the environment). It gets its own working directory,
    program cache and output path, and no backend daemon, so no compile state
    passes between observations. ``build_timeout`` defaults to the plan's
    nested build budget.
    """
    if build_timeout is None:
        build_timeout = default_nested_process_timeout_seconds("build")
    launch_cwd = Path.cwd() if cwd is None else Path(cwd).absolute()
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env["PYTHONHASHSEED"] = str(hash_seed)
    if deterministic:
        env["MOLT_DETERMINISTIC"] = "1"
    else:
        env.pop("MOLT_DETERMINISTIC", None)
    if cache_dir is not None:
        env["MOLT_CACHE"] = cache_dir
    env["MOLT_BACKEND_DAEMON"] = "0"
    output_args = (
        ["--output", str(Path(cwd) / Path(source).stem)] if cwd is not None else []
    )
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
        *output_args,
        source,
    ]
    try:
        result = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            env=env,
            cwd=launch_cwd,
            timeout=build_timeout,
            limits=limits,
        )
    except (subprocess.TimeoutExpired, OSError) as exc:
        launch = _launch_evidence(cmd, env, cwd=launch_cwd, outcome=exc)
        return (
            None,
            f"build timed out after {build_timeout:g} s"
            if isinstance(exc, subprocess.TimeoutExpired)
            else str(exc),
            None,
            launch,
        )

    launch = _launch_evidence(cmd, env, cwd=launch_cwd, outcome=result)
    if launch["status"] != "completed":
        return None, f"build {launch['status']}", None, launch

    if result.returncode != 0:
        return (
            None,
            f"build failed (exit {result.returncode}): {result.stderr[:1000]}",
            None,
            launch,
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
            return None, f"invalid build JSON: {e}", None, launch
    else:
        try:
            build_info = json.loads(json_str)
        except json.JSONDecodeError as e:
            return None, f"invalid build JSON: {e}", None, launch

    try:
        binary = extract_artifact_path(build_info)
        binary_path = Path(binary)
        if not binary_path.is_absolute():
            binary_path = launch_cwd / binary_path
        if not binary_path.exists():
            return None, f"binary not found: {binary}", build_info, launch
    except (KeyError, ValueError, OSError) as exc:
        return (
            None,
            f"build artifact error: {str(exc) or type(exc).__name__}",
            build_info if isinstance(build_info, dict) else None,
            launch,
        )

    return str(binary_path), "", build_info, launch


def run_binary(
    binary: str,
    run_index: int,
    timeout: int = 60,
    *,
    deterministic: bool = True,
    cwd: str | Path | None = None,
) -> tuple[bytes, bytes, int | None, dict[str, object]]:
    """Return stdout, stderr, returncode and actual launch evidence."""
    launch_cwd = Path.cwd() if cwd is None else Path(cwd).absolute()
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = str(run_index)
    if deterministic:
        env["MOLT_DETERMINISTIC"] = "1"
    else:
        env.pop("MOLT_DETERMINISTIC", None)
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)

    command = [binary]
    try:
        result = harness_memory_guard.guarded_completed_process(
            command,
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=False,
            env=env,
            cwd=launch_cwd,
            timeout=timeout,
            limits=limits,
        )
    except (subprocess.TimeoutExpired, OSError) as exc:
        return (
            b"",
            b"",
            None,
            _launch_evidence(command, env, cwd=launch_cwd, outcome=exc),
        )

    launch = _launch_evidence(command, env, cwd=launch_cwd, outcome=result)
    rc = result.returncode if launch["status"] == "completed" else None
    return result.stdout, result.stderr, rc, launch


def check_determinism(
    source: str,
    runs: int,
    profile: str,
    timeout: int = 60,
    verbose: bool = False,
    deterministic_mode: bool = True,
    build_timeout: float | None = None,
) -> dict:
    """Check determinism for a single source file. Returns result dict."""
    result = {
        "source": source,
        "runs": runs,
        "deterministic": False,
        "status": "unknown",
        "mode": "deterministic" if deterministic_mode else "default",
        "profile": profile,
        "toolchain": {"python": sys.version},
    }

    if runs < 2:
        result["status"] = "error"
        result["error"] = "runs must be at least 2"
        return result

    try:
        if not Path(source).exists():
            result["status"] = "error"
            result["error"] = "source file not found"
            return result
        source_path = Path(source).resolve()
    except OSError as exc:
        result.update(status="error", error=str(exc) or type(exc).__name__)
        return result
    outputs: list[tuple[bytes, bytes, int | None]] = []
    observations: list[dict[str, object]] = []
    result["observations"] = observations
    result["completed_runs"] = 0
    for i in range(runs):
        observation = {
            "index": i + 1,
            "logical_cwd": f"isolated-{i + 1}",
            "source": source_path.name,
            "build": None,
            "build_receipt": None,
            "runtime": None,
        }
        observations.append(observation)
        phase = "prepare"
        try:
            with OwnedTemporaryDirectory(prefix=f"runtime_repeat_{i}_") as run_root:
                relocated_source = Path(run_root) / source_path.name
                shutil.copyfile(source_path, relocated_source)
                cache = Path(run_root) / "cache"
                phase = "build"
                binary, error, build_receipt, build_launch = build_program(
                    str(relocated_source),
                    profile,
                    deterministic=deterministic_mode,
                    cache_dir=str(cache),
                    cwd=run_root,
                    hash_seed=0,
                    build_timeout=build_timeout,
                )
                observation.update(build=build_launch, build_receipt=build_receipt)
                if binary is None:
                    result["status"] = "build_error"
                    result["error"] = f"observation {i + 1}: {error}"
                    phase = "cleanup"
                    return result
                phase = "artifact"
                binary_path = Path(binary)
                if not binary_path.is_absolute():
                    binary_path = Path(run_root) / binary_path
                binary_hash = _sha256_file(binary_path)
                observation["binary_sha256"] = binary_hash
                phase = "runtime"
                stdout, stderr, rc, runtime_launch = run_binary(
                    str(binary_path),
                    i + 1,
                    timeout,
                    deterministic=deterministic_mode,
                    cwd=run_root,
                )
                observation["runtime"] = runtime_launch
                outputs.append((stdout, stderr, rc))
                digest = hashlib.sha256()
                digest.update(len(stdout).to_bytes(8, "big"))
                digest.update(stdout)
                digest.update(len(stderr).to_bytes(8, "big"))
                digest.update(stderr)
                digest.update(
                    (-1 if rc is None else rc).to_bytes(8, "big", signed=True)
                )
                observation.update(
                    stdout_sha256=hashlib.sha256(stdout).hexdigest(),
                    stderr_sha256=hashlib.sha256(stderr).hexdigest(),
                    returncode=rc,
                    observable_sha256=digest.hexdigest(),
                )
                if verbose:
                    print(
                        f"  Observation {i + 1}: {digest.hexdigest()[:16]} "
                        f"(stdout={len(stdout)}, stderr={len(stderr)}) rc={rc}"
                    )
                phase = "cleanup"
            if rc is not None:
                result["completed_runs"] += 1
        except (OSError, ValueError) as exc:
            # A completed child remains completed. Artifact/cleanup failures
            # invalidate this cell without discarding already captured launches.
            diagnostic = str(exc) or type(exc).__name__
            observation.update(error_phase=phase, error=diagnostic)
            result.update(
                status="error", error=f"observation {i + 1} {phase}: {diagnostic}"
            )
            return result

    if any(rc is None for _, _, rc in outputs):
        statuses = {
            row["runtime"]["status"]
            for row in observations
            if row["runtime"] is not None
        }
        result["status"] = (
            "timeout" if statuses <= {"completed", "timeout"} else "run_error"
        )
        result["error"] = (
            "one or more runtime observations did not complete successfully"
        )
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
        "--build-timeout",
        type=float,
        default=None,
        help="Timeout in seconds per program build (default: the proof plan's "
        "nested build budget)",
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

    selected_modes = sum(
        value is not None for value in (args.batch, args.corpus, args.source)
    )
    if selected_modes > 1:
        parser.error("choose exactly one source, --batch, or --corpus")
    sources = (
        args.batch
        or (resolve_corpus(args.corpus) if args.corpus else None)
        or ([args.source] if args.source else [])
    )
    if not sources:
        parser.error("Either provide a source file or use --batch")
    modes = [False, True] if args.mode == "both" else [args.mode == "deterministic"]
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
            args.build_timeout,
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
        "schema": "molt.deterministic-runtime-proof.v3",
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
