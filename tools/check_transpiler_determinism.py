#!/usr/bin/env python3
"""Verify deterministic transpiler source output for Rust/Luau targets.

Builds one or more Python sources repeatedly with ``molt.cli build --target``
and checks whether emitted backend source artifacts are byte-identical across
runs. This catches nondeterminism in transpiler emission before downstream
compile/runtime phases.

Usage:
    python tools/check_transpiler_determinism.py examples/hello.py
    python tools/check_transpiler_determinism.py --batch tests/differential/basic
    python tools/check_transpiler_determinism.py --targets rust --runs 5 --json-out out.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any


TARGET_CHOICES = ("rust", "luau")


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _artifact_root() -> Path:
    configured = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if configured:
        root = Path(configured).expanduser().resolve()
        if root.is_dir():
            return root
        raise SystemExit(f"MOLT_EXT_ROOT is not a directory: {root}")
    return _repo_root()


def _tmp_root() -> Path:
    for env_var in ("MOLT_DIFF_TMPDIR", "TMPDIR"):
        raw = os.environ.get(env_var, "").strip()
        if raw:
            path = Path(raw).expanduser().resolve()
            path.mkdir(parents=True, exist_ok=True)
            return path
    root = _artifact_root() / "tmp"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _build_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = "0"
    env.setdefault("MOLT_EXT_ROOT", str(_artifact_root()))
    env.setdefault("CARGO_TARGET_DIR", str(_artifact_root() / "target"))
    env.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", env["CARGO_TARGET_DIR"])
    env.setdefault("MOLT_CACHE", str(_artifact_root() / ".molt_cache"))
    env.setdefault("MOLT_DIFF_ROOT", str(_artifact_root() / "tmp" / "diff"))
    env.setdefault("MOLT_DIFF_TMPDIR", str(_tmp_root()))
    env.setdefault("TMPDIR", str(_tmp_root()))
    env.setdefault("UV_CACHE_DIR", str(_artifact_root() / ".uv-cache"))
    env.setdefault("MOLT_DEV_CARGO_PROFILE", "release-fast")
    env.setdefault("UV_NO_SYNC", "1")
    env.setdefault("UV_LINK_MODE", "copy")
    src = str(_repo_root() / "src")
    existing_pythonpath = env.get("PYTHONPATH", "")
    if existing_pythonpath:
        parts = existing_pythonpath.split(os.pathsep)
        if src not in parts:
            env["PYTHONPATH"] = src + os.pathsep + existing_pythonpath
    else:
        env["PYTHONPATH"] = src
    return env


def _collect_sources(source: str | None, batch_dir: str | None) -> list[Path]:
    if source and batch_dir:
        raise SystemExit("Provide either SOURCE or --batch, not both.")
    if not source and not batch_dir:
        raise SystemExit("Provide SOURCE or --batch.")
    if source:
        path = Path(source).expanduser().resolve()
        if not path.is_file():
            raise SystemExit(f"Source not found: {path}")
        return [path]
    root = Path(batch_dir or "").expanduser().resolve()
    if not root.is_dir():
        raise SystemExit(f"Batch directory not found: {root}")
    files = sorted(p for p in root.rglob("*.py") if p.is_file())
    if not files:
        raise SystemExit(f"No .py files found in {root}")
    return files


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        while True:
            chunk = f.read(1024 * 1024)
            if not chunk:
                break
            h.update(chunk)
    return h.hexdigest()


def _build_once(
    *,
    source: Path,
    target: str,
    profile: str,
    timeout: float,
    env: dict[str, str],
) -> tuple[Path | None, str | None, float]:
    ext = ".rs" if target == "rust" else ".luau"
    with tempfile.NamedTemporaryFile(
        suffix=ext,
        dir=str(_tmp_root()),
        delete=False,
    ) as out_f:
        out_path = Path(out_f.name)

    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(source),
        "--target",
        target,
        "--profile",
        profile,
        "--output",
        str(out_path),
    ]
    started = time.perf_counter()
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(_repo_root()),
            env=env,
        )
    except subprocess.TimeoutExpired:
        elapsed = time.perf_counter() - started
        try:
            out_path.unlink(missing_ok=True)
        except OSError:
            pass
        return None, f"build timed out after {timeout}s", elapsed

    elapsed = time.perf_counter() - started
    if result.returncode != 0:
        detail = (result.stderr.strip() or result.stdout.strip())[:600]
        try:
            out_path.unlink(missing_ok=True)
        except OSError:
            pass
        return None, f"build failed (rc={result.returncode}): {detail}", elapsed
    if not out_path.exists():
        return None, "build completed but output artifact missing", elapsed
    return out_path, None, elapsed


def _check_source_target(
    *,
    source: Path,
    target: str,
    runs: int,
    profile: str,
    timeout: float,
    env: dict[str, str],
) -> dict[str, Any]:
    run_rows: list[dict[str, Any]] = []
    hashes: list[str] = []

    for run_idx in range(1, runs + 1):
        artifact, err, elapsed = _build_once(
            source=source,
            target=target,
            profile=profile,
            timeout=timeout,
            env=env,
        )
        if artifact is None:
            run_rows.append(
                {
                    "run": run_idx,
                    "ok": False,
                    "elapsed_sec": round(elapsed, 3),
                    "error": err or "unknown build error",
                }
            )
            return {
                "source": str(source),
                "target": target,
                "status": "error",
                "deterministic": False,
                "runs": run_rows,
            }
        digest = _sha256(artifact)
        hashes.append(digest)
        run_rows.append(
            {
                "run": run_idx,
                "ok": True,
                "elapsed_sec": round(elapsed, 3),
                "artifact": str(artifact),
                "sha256": digest,
            }
        )
        try:
            artifact.unlink(missing_ok=True)
        except OSError:
            pass

    first = hashes[0] if hashes else ""
    deterministic = all(h == first for h in hashes)
    return {
        "source": str(source),
        "target": target,
        "status": "pass" if deterministic else "fail",
        "deterministic": deterministic,
        "reference_sha256": first,
        "runs": run_rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", nargs="?", help="Single Python source file.")
    parser.add_argument(
        "--batch",
        metavar="DIR",
        help="Validate all .py files under DIR recursively.",
    )
    parser.add_argument(
        "--targets",
        default="rust,luau",
        help="Comma-separated targets from {rust,luau}. Default: rust,luau",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=3,
        help="Number of repeated transpiler builds per source/target. Default: 3",
    )
    parser.add_argument(
        "--profile",
        default="dev",
        help="Molt build profile. Default: dev",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=300.0,
        help="Per-build timeout in seconds. Default: 300",
    )
    parser.add_argument(
        "--json-out",
        metavar="FILE",
        help="Write full JSON report to FILE.",
    )
    args = parser.parse_args()

    sources = _collect_sources(args.source, args.batch)
    targets = [t.strip() for t in args.targets.split(",") if t.strip()]
    if not targets:
        raise SystemExit("No targets provided.")
    bad_targets = [t for t in targets if t not in TARGET_CHOICES]
    if bad_targets:
        raise SystemExit(
            f"Invalid targets: {', '.join(bad_targets)}. Allowed: {', '.join(TARGET_CHOICES)}"
        )
    if args.runs < 2:
        raise SystemExit("--runs must be >= 2")

    env = _build_env()
    total = len(sources) * len(targets)
    print(
        f"Transpiler determinism check: {len(sources)} source(s), "
        f"{len(targets)} target(s), runs={args.runs}, profile={args.profile}"
    )

    rows: list[dict[str, Any]] = []
    done = 0
    for source in sources:
        for target in targets:
            done += 1
            print(f"[{done}/{total}] {source.name} [{target}] ... ", end="", flush=True)
            row = _check_source_target(
                source=source,
                target=target,
                runs=args.runs,
                profile=args.profile,
                timeout=args.timeout,
                env=env,
            )
            rows.append(row)
            status = row["status"]
            if status == "pass":
                print("PASS")
            elif status == "fail":
                print("FAIL")
                hashes = [r.get("sha256", "")[:16] for r in row.get("runs", [])]
                if hashes:
                    print(f"    hashes: {hashes}")
            else:
                print("ERROR")
                runs = row.get("runs", [])
                if runs:
                    err = runs[-1].get("error", "unknown error")
                    print(f"    error: {err}")

    n_pass = sum(1 for r in rows if r["status"] == "pass")
    n_fail = sum(1 for r in rows if r["status"] == "fail")
    n_error = sum(1 for r in rows if r["status"] == "error")
    payload = {
        "summary": {
            "sources": len(sources),
            "targets": targets,
            "runs": args.runs,
            "pass": n_pass,
            "fail": n_fail,
            "error": n_error,
        },
        "results": rows,
    }
    if args.json_out:
        out = Path(args.json_out).expanduser().resolve()
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(payload, indent=2) + "\n")
        print(f"JSON report: {out}")

    print(f"Results: {n_pass} pass, {n_fail} fail, {n_error} error")
    if n_error > 0:
        return 2
    if n_fail > 0:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
