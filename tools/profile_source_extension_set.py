"""Profile one canonical source-extension set production run.

The caller provisions the exact upstream build requirements in the active
interpreter.  This wrapper measures the complete producer process tree and
records the verified content-addressed output, so cold/warm reproductions are
directly comparable without weakening producer custody.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import sys
from pathlib import Path

from molt.cli.extension_manifest import _host_target_triple
from molt.cli.source_package_seal import verify_source_package_seal
from molt.cli.source_extension_target import resolve_source_extension_target_plan
from molt.scientific_stack_versions import (
    ScientificExtensionVariant,
    resolve_scientific_stack,
    scientific_extension_set,
    scientific_extension_set_root,
)
from tools.perf_calibration import run_and_measure

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)


def _git_head(source: Path) -> str:
    result = _COMMANDS.run(
        ("git", "-C", str(source), "rev-parse", "HEAD"),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.returncode != 0 or not result.stdout.strip():
        raise ValueError(f"cannot attest source HEAD for {source}")
    return result.stdout.strip()


def _tree_metrics(root: Path) -> dict[str, int]:
    files = tuple(path for path in root.rglob("*") if path.is_file())
    return {
        "file_count": len(files),
        "total_bytes": sum(path.stat().st_size for path in files),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", required=True)
    parser.add_argument("--module-set", required=True)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--build-root", required=True, type=Path)
    parser.add_argument("--target", default="wasm")
    parser.add_argument("--abi-tier", default="cpython-abi")
    parser.add_argument("--timeout", type=float, default=7200.0)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)

    source = args.source.expanduser().resolve()
    build_root = args.build_root.expanduser().resolve()
    stack = resolve_scientific_stack()
    extension_set = scientific_extension_set(
        args.package,
        args.module_set,
        stack=stack,
    )
    target_plan = resolve_source_extension_target_plan(
        args.target,
        host_target_triple=_host_target_triple(),
        host_platform=sys.platform,
        host_arch=platform.machine(),
    )
    destination = scientific_extension_set_root(
        extension_set,
        variant=ScientificExtensionVariant(
            cpython=stack.cpython,
            abi_tier=args.abi_tier,
            target_triple=target_plan.target_triple,
        ),
        stack=stack,
    )
    command = [
        sys.executable,
        "-m",
        "molt",
        "extension",
        "produce-set",
        "--package",
        args.package,
        "--module-set",
        args.module_set,
        "--source",
        str(source),
        "--build-root",
        str(build_root),
        "--target",
        args.target,
        "--abi-tier",
        args.abi_tier,
        "--json",
    ]
    measured = run_and_measure(
        command,
        timeout=args.timeout,
        cwd=str(Path(__file__).resolve().parents[1]),
        env={"MOLT_EXT_ROOT": os.environ.get("MOLT_EXT_ROOT", "")},
    )
    record: dict[str, object] = {
        "schema_version": 1,
        "kind": "source-extension-set-profile",
        "package": args.package,
        "module_set": args.module_set,
        "source": str(source),
        "source_head": _git_head(source),
        "build_root": str(build_root),
        "destination": str(destination),
        "command": command,
        "returncode": measured.returncode,
        "timed_out": measured.timed_out,
        "elapsed_s": measured.elapsed_s,
        "peak_process_rss_bytes": measured.peak_rss_bytes,
        "peak_process_tree_commit_bytes": measured.peak_job_commit_bytes,
        "stdout": measured.stdout,
        "stderr": measured.stderr,
    }
    if measured.returncode == 0:
        seal = verify_source_package_seal(destination)
        record["seal_sha256"] = seal.seal_sha256
        record["artifact"] = _tree_metrics(destination)
    encoded = json.dumps(record, indent=2, sort_keys=True) + "\n"
    if args.output is not None:
        output = args.output.expanduser().resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(encoded, encoding="utf-8")
    sys.stdout.write(encoded)
    return measured.returncode


if __name__ == "__main__":
    raise SystemExit(main())
