#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
from pathlib import Path
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


def _prepare_clean_dir(path: Path) -> None:
    owned = _assert_owned_tmp(path)
    if owned.exists():
        shutil.rmtree(owned)
    owned.mkdir(parents=True, exist_ok=True)


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

    out_dir = _assert_owned_tmp(args.out_dir)
    build_dir = out_dir / "build"
    run_dir = out_dir / "run"
    _prepare_clean_dir(build_dir)
    _prepare_clean_dir(run_dir)

    output_wasm = _build_wasm(build_dir)
    candidate = _run_candidate(output_wasm, run_dir)
    _check_parity(candidate)
    print("pact witness acceptance PASS", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
