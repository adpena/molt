#!/usr/bin/env python3
"""Paired first-load/warm observations of fresh executable bytes (HF-634).

This measures the complete direct-child start, not a claimed antivirus-only
interval. File publication, tree copying and image sealing are timed separately.
No cache flush, security exemption or host interpreter mutation is performed.
Link measurements remain owned by native_link_benchmark.py.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import statistics
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.process_environment import ProcessEnvironment  # noqa: E402
from tools import output_startup_size_audit as startup  # noqa: E402


def _byte_copy(source: str | Path, destination: str | Path) -> str:
    # copy2/copyfile may use a kernel clone on some Python/platform versions.
    with Path(source).open("rb") as reader, Path(destination).open("xb") as writer:
        shutil.copyfileobj(reader, writer, length=1024 * 1024)
    shutil.copystat(source, destination)
    return os.fspath(destination)


def _load() -> list[float] | None:
    return list(os.getloadavg()) if hasattr(os, "getloadavg") else None


def probe(
    artifact: Path,
    *,
    work: Path,
    arguments: list[str],
    samples: int = 3,
    warm_runs: int = 3,
    tree: Path | None = None,
    seal: bool = False,
    timeout: float = 30.0,
) -> dict[str, Any]:
    """Copy/start/warm one private inode per sample, retaining every result.

    ``tree`` models the owned-interpreter fixture, including its dependent
    libraries. ``seal`` additionally exercises the production detach operation
    on that private executable. Neither ever hardlinks the host installation.
    """
    if samples < 1 or warm_runs < 1:
        raise ValueError("samples and warm runs must be positive")
    artifact = artifact.resolve(strict=True)
    tree = tree.resolve(strict=True) if tree is not None else None
    if seal and tree is None:
        raise ValueError("sealing requires an owned installation tree")
    relative = artifact.relative_to(tree) if tree is not None else Path(artifact.name)
    work.mkdir(parents=True, exist_ok=True)
    records = []
    env = ProcessEnvironment(dict(os.environ))
    expected = None
    for index in range(samples):
        with tempfile.TemporaryDirectory(prefix="first-load-", dir=work) as private:
            root = Path(private) / "image"
            before_load = _load()
            started = time.perf_counter_ns()
            if tree is None:
                root.mkdir()
                fresh = root / relative
                _byte_copy(artifact, fresh)
            else:
                shutil.copytree(tree, root, symlinks=True, copy_function=_byte_copy)
                fresh = root / relative
            copy_ns = time.perf_counter_ns() - started
            seal_ns = None
            if seal:
                from molt.python_environment_location import (
                    _detach_shared_installation_file,
                )

                started = time.perf_counter_ns()
                _detach_shared_installation_file(root, [fresh])
                seal_ns = time.perf_counter_ns() - started
            identity = fresh.stat()
            if os.path.samestat(identity, artifact.stat()):
                raise ValueError("fresh probe must own an independent executable inode")
            runs = []
            for run in range(1 + warm_runs):
                command = [str(fresh), *arguments]
                result = startup._run_guarded(
                    command,
                    env=env,
                    timeout=timeout,
                    progress_label=f"first-load {index + 1}/{samples} start {run}",
                )
                observation = startup._sample_record(
                    index=run, command=command, result=result
                )
                runs.append(observation)
                if observation["elapsed_s"] is None:
                    raise ValueError(f"first-load child failed: {observation}")
                output = (result.returncode, result.stdout, result.stderr)
                if expected is None:
                    expected = output
                elif output != expected:
                    raise ValueError(
                        "fresh and warm executable output/exit/stderr differ"
                    )
                if not os.path.samestat(identity, fresh.stat()):
                    raise ValueError(
                        "executable inode changed between first and warm starts"
                    )
            warm = statistics.median(row["elapsed_s"] for row in runs[1:])
            after_load = _load()
            records.append(
                {
                    "index": index,
                    "load_before": before_load,
                    "load_after": after_load,
                    "unloaded": before_load is not None
                    and after_load is not None
                    and max(before_load[0], after_load[0]) < 2,
                    "copy_ns": copy_ns,
                    "seal_ns": seal_ns,
                    "size_bytes": identity.st_size,
                    "first_s": runs[0]["elapsed_s"],
                    "warm_median_s": warm,
                    "first_extra_s": runs[0]["elapsed_s"] - warm,
                    "runs": runs,
                }
            )
    return {
        "schema": "molt.executable-first-load.v1",
        "host": {
            "system": platform.system(),
            "machine": platform.machine(),
            "version": platform.version(),
            "python": sys.version,
        },
        "source": str(artifact),
        "sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
        "tree": str(tree) if tree else None,
        "seal": seal,
        "records": records,
        "unloaded": all(row["unloaded"] for row in records),
        "median_first_extra_s": statistics.median(
            row["first_extra_s"] for row in records
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact", required=True, type=Path)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--json-out", required=True, type=Path)
    parser.add_argument("--tree", type=Path)
    parser.add_argument("--seal", action="store_true")
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--warm-runs", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    report = probe(
        args.artifact,
        work=args.work,
        arguments=args.arguments[1:]
        if args.arguments[:1] == ["--"]
        else args.arguments,
        samples=args.samples,
        warm_runs=args.warm_runs,
        tree=args.tree,
        seal=args.seal,
        timeout=args.timeout,
    )
    args.json_out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(
        json.dumps({key: report[key] for key in ("unloaded", "median_first_extra_s")})
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
