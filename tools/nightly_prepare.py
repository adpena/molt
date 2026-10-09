#!/usr/bin/env python3
"""Prepare one immutable runtime, CPython source, and shard plan for Nightly.

The workflow transports these outputs; this module owns their construction so
GitHub YAML never duplicates compiler, source-custody, or partition policy.
"""

from __future__ import annotations

import argparse
from collections.abc import Mapping
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
for import_root in (ROOT, SRC):
    if str(import_root) not in sys.path:
        sys.path.insert(0, str(import_root))

from molt.artifact_publication import atomic_write_json  # noqa: E402
from molt.dx import development_artifact_env  # noqa: E402
from molt.exact_json import loads_exact  # noqa: E402
from tools.command_execution import CommandExecutor, bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools import cpython_regrtest, nightly_sharding  # noqa: E402


COMMANDS = CommandExecutor.for_file(__file__)


def _write_github_outputs(path: Path, plan: dict[str, object]) -> None:
    programs = plan["programs"]
    assert isinstance(programs, dict)
    lines = []
    for program in nightly_sharding.SHARD_COUNTS:
        entry = programs[program]
        assert isinstance(entry, dict)
        shards = entry["shards"]
        assert isinstance(shards, list)
        matrix = {"shard": [int(shard["id"]) for shard in shards]}
        lines.append(f"{program}_matrix={json.dumps(matrix, separators=(',', ':'))}")
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8", newline="\n") as stream:
        stream.write("\n".join(lines) + "\n")


def prepare(
    *,
    output_root: Path,
    cpython_dir: Path,
    build_env: Mapping[str, str],
    github_output: Path | None,
) -> dict[str, object]:
    """Build the runtime and backend once and bundle what that build admitted.

    ``build_env`` is the DX-resolved environment of the smoke build. The
    bundle exports under the same environment, so it reads the runtime
    generation and backend the build selected by construction.
    """
    output_root.mkdir(parents=True, exist_ok=True)
    log_path = output_root / "cpython-provision.log"
    sources = cpython_regrtest.load_cpython_sources()
    source = sources.get("3.12")
    if source is None:
        raise ValueError("Nightly requires a pinned CPython 3.12 source")
    with log_path.open("w", encoding="utf-8") as log:
        cpython_regrtest.ensure_cpython_checkout(
            cpython_dir,
            source,
            allow_clone=True,
            log_handle=log,
            dry_run=False,
        )

    smoke_output = output_root / "runtime-smoke"
    completed = COMMANDS.run(
        [
            "uv",
            "run",
            "python",
            "-m",
            "molt.cli",
            "build",
            "examples/hello.py",
            "--target",
            "native",
            "--build-profile",
            "dev",
            "--stdlib-profile",
            "full",
            "--output",
            str(smoke_output),
        ],
        cwd=ROOT,
        env=build_env,
        timeout=3600,
    )
    if completed.returncode != 0 or not smoke_output.is_file():
        raise RuntimeError("Nightly runtime preparation failed")

    archive = output_root / "runtime-bundle.tar"
    manifest_path = output_root / "runtime-bundle-manifest.json"
    # The CLI selects the runtime and backend from the environment, so the
    # export runs in the build's own environment and finds what it admitted.
    packed = COMMANDS.run(
        [
            sys.executable,
            "tools/nightly_runtime_bundle.py",
            "pack",
            "--output",
            str(archive),
            "--manifest-out",
            str(manifest_path),
        ],
        cwd=ROOT,
        env=build_env,
        stdout=subprocess.DEVNULL,
        timeout=1800,
    )
    if packed.returncode != 0 or not manifest_path.is_file():
        raise RuntimeError("Nightly runtime bundle packing failed")
    manifest = loads_exact(manifest_path.read_text(encoding="utf-8"))
    smoke_output.unlink()
    plan = nightly_sharding.build_plan(
        ROOT,
        runtime_artifact_manifest=manifest_path,
    )
    plan_path = output_root / "shard-plan.json"
    atomic_write_json(plan_path, plan, sort_keys=True)
    if github_output is not None:
        _write_github_outputs(github_output, plan)
    return {
        "schema": "molt.nightly-prepare.v1",
        "source_commit": manifest["identity"]["source_commit"],
        "cpython_commit": plan["cpython_commit"],
        "plan_sha256": plan["plan_sha256"],
        "weight_profile_sha256": plan["authority"]["weight_profile"]["profile_sha256"],
        "runtime_bundle": archive.name,
        "runtime_manifest": manifest,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument(
        "--cpython-dir", type=Path, default=ROOT / "third_party" / "cpython"
    )
    parser.add_argument(
        "--target-root",
        type=Path,
        default=None,
        help="Cargo target root; the DX authority chooses it by default",
    )
    parser.add_argument(
        "--github-output",
        type=Path,
        default=Path(os.environ["GITHUB_OUTPUT"])
        if os.environ.get("GITHUB_OUTPUT")
        else None,
    )
    args = parser.parse_args(argv)
    build_env = development_artifact_env(ROOT, session_prefix="nightly")
    if args.target_root is not None:
        build_env["CARGO_TARGET_DIR"] = str(args.target_root.resolve())
    try:
        summary = prepare(
            output_root=args.output_root.resolve(),
            cpython_dir=args.cpython_dir.resolve(),
            build_env=build_env,
            github_output=args.github_output,
        )
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"nightly-prepare: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
