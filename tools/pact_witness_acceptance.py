#!/usr/bin/env python3
from __future__ import annotations

import argparse
from datetime import UTC, datetime
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
KERNEL_ROOT = ROOT / "collab" / "pact" / "pact_witness_kernel"
DEFAULT_OUT_DIR = ROOT / "tmp" / "pact_witness_acceptance_queue"


def _run(args: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> None:
    print(f"+ {' '.join(args)}", flush=True)
    subprocess.run(args, cwd=cwd, env=env, check=True)


def _node_bin() -> str:
    requested = os.environ.get("MOLT_NODE_BIN", "").strip()
    if requested:
        return requested
    found = shutil.which("node")
    if found:
        return found
    raise SystemExit("node is required to execute the Pact witness WASM artifact")


def _assert_owned_tmp(path: Path) -> Path:
    resolved = path.resolve()
    tmp_root = (ROOT / "tmp").resolve()
    try:
        resolved.relative_to(tmp_root)
    except ValueError as exc:
        raise SystemExit(
            f"Pact witness acceptance out-dir must stay under {tmp_root}: {resolved}"
        ) from exc
    return resolved


def _safe_attempt_slug(raw: str) -> str:
    cleaned = re.sub(r"[^0-9A-Za-z_.-]+", "_", raw.strip()).strip("._-")
    return cleaned or "manual"


def _attempt_slug() -> str:
    run_id = os.environ.get("MOLT_PROOF_QUEUE_RUN_ID", "").strip()
    if run_id:
        return _safe_attempt_slug(run_id)
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%S.%fZ")
    return _safe_attempt_slug(f"manual-{stamp}-{os.getpid()}")


def _prepare_attempt_dirs(out_dir: Path) -> tuple[Path, Path]:
    owned = _assert_owned_tmp(out_dir)
    owned.mkdir(parents=True, exist_ok=True)
    attempts_root = owned / "runs"
    attempts_root.mkdir(parents=True, exist_ok=True)
    base = _attempt_slug()
    attempt_dir = attempts_root / base
    counter = 2
    while attempt_dir.exists():
        attempt_dir = attempts_root / f"{base}-{counter}"
        counter += 1
    attempt_dir.mkdir(parents=True)
    build_dir = attempt_dir / "build"
    run_dir = attempt_dir / "run"
    build_dir.mkdir()
    run_dir.mkdir()
    (owned / "latest_attempt.txt").write_text(str(attempt_dir) + "\n", encoding="utf-8")
    return build_dir, run_dir


def _build_env() -> dict[str, str]:
    env = os.environ.copy()
    src_path = str(ROOT / "src")
    current = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = src_path if not current else src_path + os.pathsep + current
    return env


def _build_wasm(build_dir: Path) -> Path:
    env = _build_env()
    _run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "--target",
            "wasm",
            "--profile",
            "browser",
            "--wasm-profile",
            "auto",
            "--split-runtime",
            "--out-dir",
            str(build_dir),
        ],
        cwd=ROOT,
        env=env,
    )
    output_wasm = build_dir / "output.wasm"
    if not output_wasm.is_file():
        raise SystemExit(f"missing build artifact: {output_wasm}")
    return output_wasm


def _run_candidate(output_wasm: Path, run_dir: Path) -> Path:
    fixture = KERNEL_ROOT / "lstar_sample.npz"
    if not fixture.is_file():
        raise SystemExit(f"missing Pact fixture: {fixture}")
    shutil.copy2(fixture, run_dir / "lstar_sample.npz")
    raw_output = run_dir / "reference_outputs.npz"
    candidate = run_dir / "candidate_outputs.npz"
    raw_output.unlink(missing_ok=True)
    candidate.unlink(missing_ok=True)
    _run(
        [_node_bin(), str(ROOT / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=run_dir,
    )
    if not raw_output.is_file():
        raise SystemExit(
            "Pact witness WASM execution did not produce reference_outputs.npz"
        )
    raw_output.replace(candidate)
    print(f"candidate_outputs={candidate}", flush=True)
    return candidate


def _check_parity(candidate: Path) -> None:
    reference = KERNEL_ROOT / "reference_outputs.npz"
    if not reference.is_file():
        raise SystemExit(f"missing Pact reference oracle: {reference}")
    _run(
        [
            sys.executable,
            str(KERNEL_ROOT / "check_parity.py"),
            str(candidate),
            str(reference),
        ],
        cwd=candidate.parent,
        env=_build_env(),
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Build, execute, and parity-check the Pact Kernel A WASM witness."
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=DEFAULT_OUT_DIR,
        help="Owned tmp artifact root for build/, run/, and candidate_outputs.npz.",
    )
    args = parser.parse_args(argv)

    build_dir, run_dir = _prepare_attempt_dirs(args.out_dir)

    output_wasm = _build_wasm(build_dir)
    candidate = _run_candidate(output_wasm, run_dir)
    _check_parity(candidate)
    print("pact witness acceptance PASS", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
