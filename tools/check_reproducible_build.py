#!/usr/bin/env python3
"""Verify that two independent builds produce bit-identical artifacts.

Usage:
    python tools/check_reproducible_build.py [--object] build1.json build2.json
    python tools/check_reproducible_build.py --build source.py
    python tools/check_reproducible_build.py --batch examples/*.py

Each JSON file should be the output of `molt.cli build --json`, containing
an "output" or "artifact" key with the path to the compiled binary.

Modes:
    build1.json build2.json   Compare two pre-built JSON artifacts.
    --build source.py         Self-contained: build source.py twice in
                              isolated caches and compare the artifacts.
    --batch sources...        Build each source file twice and report results.

Flags:
    --object  Compare .o object files instead of linked binaries.
              Use this to avoid linker-injected nondeterminism (macOS LC_UUID).
    --json-out FILE  Write JSON results (for CI integration).

Exit codes:
    0 — builds are reproducible (SHA256 match)
    1 — builds differ (SHA256 mismatch)
    2 — usage error
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import harness_memory_guard  # noqa: E402
from tools.proof_counts import fail_closed_proof_exit_code  # noqa: E402

CORPUS_MANIFEST = ROOT / "config" / "reproducibility_corpus.toml"


def _write_proof_receipt(
    path: str | None,
    *,
    mode: str,
    selected: int,
    executed: int,
    passed: int,
    failed: int,
    errors: int,
    **evidence: object,
) -> None:
    """Write the same fail-closed proof contract for every invocation mode."""
    if path is None:
        return
    payload = {
        "schema": "molt.reproducibility-proof.v2",
        "status": (
            "success"
            if executed > 0 and failed == 0 and errors == 0
            else "failure"
        ),
        "mode": mode,
        "selected": selected,
        "executed": executed,
        "passed": passed,
        "failed": failed,
        "errors": errors,
        **evidence,
    }
    output = Path(path)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def sha256_file(path: str) -> str:
    """Compute SHA256 hex digest of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def find_first_diff(path1: str, path2: str) -> tuple[int, int, int] | None:
    """Find the byte offset of the first difference between two files.

    Returns (offset, byte1, byte2) or None if files are identical.
    """
    with open(path1, "rb") as f1, open(path2, "rb") as f2:
        offset = 0
        while True:
            chunk1 = f1.read(4096)
            chunk2 = f2.read(4096)
            if not chunk1 and not chunk2:
                return None
            if not chunk1 or not chunk2:
                return (
                    offset,
                    -1 if not chunk1 else chunk1[0],
                    -1 if not chunk2 else chunk2[0],
                )
            for i, (b1, b2) in enumerate(zip(chunk1, chunk2)):
                if b1 != b2:
                    return (offset + i, b1, b2)
            if len(chunk1) != len(chunk2):
                return (offset + min(len(chunk1), len(chunk2)), -1, -1)
            offset += len(chunk1)
    return None


def extract_artifact_path(build_json: dict, prefer_object: bool = False) -> str:
    """Extract the artifact path from build JSON output.

    When *prefer_object* is True, prefer the ``.o`` file over the linked binary
    because the linker (especially on macOS) injects nondeterministic UUIDs.
    """
    data = build_json
    # Unwrap "data" envelope (molt.cli build --json wraps output in data)
    if "data" in build_json and isinstance(build_json["data"], dict):
        data = build_json["data"]

    # Check status field — bail early if build failed
    status = build_json.get("status") or data.get("status")
    if status and status != "ok":
        raise KeyError(f"Build reported non-ok status: {status}")

    # If prefer_object, try to find the object file first
    if prefer_object:
        artifacts = data.get("artifacts", {})
        if isinstance(artifacts, dict) and "object" in artifacts:
            return artifacts["object"]

    # Try standard keys
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return data[key]
    # Try nested under "build"
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return data["build"][key]
    raise KeyError(
        f"Cannot find artifact path in build JSON. Available keys: {list(data.keys())}"
    )


def _build_once(
    source: str,
    cache_dir: str,
    profile: str,
    prefer_object: bool,
) -> tuple[str | None, str]:
    """Build a source file once, returning (artifact_path, error_msg)."""
    env = os.environ.copy()
    env.setdefault("PYTHONPATH", "src")
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"
    env["MOLT_CACHE"] = cache_dir
    # Clear any cached state
    if "MOLT_BUILD_CACHE" in env:
        del env["MOLT_BUILD_CACHE"]
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)

    emit_args = ["--emit", "obj"] if prefer_object else []
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "--deterministic",
        "--json",
        *emit_args,
        source,
    ]
    try:
        result = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            env=env,
            timeout=120,
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        return None, "build timed out"

    if result.returncode != 0:
        return None, f"build failed (exit {result.returncode}): {result.stderr[:500]}"

    stdout = result.stdout.strip()
    json_str = None
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            json_str = line
            break

    if json_str is None:
        return None, f"no JSON in build output: {stdout[:300]}"

    try:
        build_info = json.loads(json_str)
    except json.JSONDecodeError as e:
        return None, f"invalid build JSON: {e}"

    try:
        artifact = extract_artifact_path(build_info, prefer_object=prefer_object)
    except KeyError as e:
        return None, str(e)

    if not Path(artifact).exists():
        return None, f"artifact not found: {artifact}"

    return artifact, ""


def compare_artifacts(
    artifact1: str,
    artifact2: str,
    label: str = "",
) -> tuple[bool, dict]:
    """Compare two artifact files. Returns (match, details_dict)."""
    hash1 = sha256_file(artifact1)
    hash2 = sha256_file(artifact2)
    size1 = Path(artifact1).stat().st_size
    size2 = Path(artifact2).stat().st_size

    details = {
        "artifact1": artifact1,
        "artifact2": artifact2,
        "sha256_1": hash1,
        "sha256_2": hash2,
        "size_1": size1,
        "size_2": size2,
        "match": hash1 == hash2,
    }

    if label:
        details["source"] = label

    if hash1 != hash2:
        diff = find_first_diff(artifact1, artifact2)
        if diff is not None:
            offset, b1, b2 = diff
            details["first_diff_offset"] = offset
            details["first_diff_byte1"] = b1
            details["first_diff_byte2"] = b2

    return hash1 == hash2, details


def _build_repeated_and_compare(
    source: str,
    profile: str,
    prefer_object: bool,
    verbose: bool,
    runs: int,
) -> tuple[bool, dict]:
    """Build a source repeatedly in isolated caches and compare all outputs."""
    if runs < 2:
        return False, {"source": source, "error": "runs must be at least 2"}
    hashes: list[str] = []
    sizes: list[int] = []
    artifacts: list[str] = []
    for run in range(runs):
        with tempfile.TemporaryDirectory(prefix=f"repro_{run}_") as cache:
            artifact, error = _build_once(source, cache, profile, prefer_object)
            if artifact is None:
                return False, {
                    "source": source,
                    "error": f"build {run + 1}: {error}",
                }
            digest = sha256_file(artifact)
            size = Path(artifact).stat().st_size
            hashes.append(digest)
            sizes.append(size)
            artifacts.append(artifact)
            if verbose:
                print(
                    f"  Build {run + 1}: {artifact}\n"
                    f"    SHA256: {digest}  ({size} bytes)"
                )
    return len(set(hashes)) == 1, {
        "source": source,
        "runs": runs,
        "hashes": hashes,
        "sizes": sizes,
        "artifacts": artifacts,
        "unique_hashes": len(set(hashes)),
        "match": len(set(hashes)) == 1,
        "command": [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--profile",
            profile,
            "--deterministic",
            "--json",
            *(["--emit", "obj"] if prefer_object else []),
            source,
        ],
        "environment": {
            "PYTHONHASHSEED": "0",
            "MOLT_DETERMINISTIC": "1",
            "isolated_cache_per_run": True,
        },
        "toolchain": {"python": sys.version},
    }


def _compile_to_ir_json(source_text: str, run_index: int) -> str:
    """Compile source to canonical IR JSON in a fresh Python process."""
    script = (
        "import json, sys; "
        "sys.path.insert(0, {src!r}); "
        "from molt.frontend import compile_to_tir; "
        "print(json.dumps(compile_to_tir(sys.stdin.read()), sort_keys=True, indent=2))"
    ).format(src=str(ROOT / "src"))
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = str(run_index)
    with tempfile.TemporaryDirectory(prefix=f"repro_ir_{run_index}_") as cwd:
        env["TMP"] = cwd
        env["TEMP"] = cwd
        result = harness_memory_guard.guarded_completed_process(
            [sys.executable, "-c", script],
            prefix="MOLT_TEST_SUITE",
            input=source_text,
            capture_output=True,
            text=True,
            env=env,
            cwd=cwd,
            timeout=60,
            limits=harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env),
        )
    if result.returncode != 0:
        raise RuntimeError(
            f"IR compilation failed (rc={result.returncode}): {result.stderr[:1000]}"
        )
    return result.stdout


def check_ir_determinism(programs: list[Path], runs: int) -> list[dict]:
    """Compare at least two isolated IR observations for every program."""
    results: list[dict] = []
    for program in programs:
        try:
            observations = [
                _compile_to_ir_json(program.read_text(encoding="utf-8"), run)
                for run in range(runs)
            ]
        except (OSError, RuntimeError) as exc:
            results.append(
                {"source": str(program), "status": "error", "error": str(exc)}
            )
            continue
        digests = [hashlib.sha256(item.encode()).hexdigest() for item in observations]
        results.append(
            {
                "source": str(program),
                "status": "pass" if len(set(digests)) == 1 else "fail",
                "runs": runs,
                "sha256": digests,
            }
        )
    return results


def resolve_corpus(name: str) -> list[str]:
    """Resolve a named, repository-owned reproducibility corpus."""
    payload = tomllib.loads(CORPUS_MANIFEST.read_text(encoding="utf-8"))
    if payload.get("schema") != "molt.reproducibility-corpus.v1":
        raise ValueError("unsupported reproducibility corpus schema")
    entries = payload.get("corpus", {}).get(name)
    if not isinstance(entries, list) or not entries:
        raise ValueError(f"missing non-empty reproducibility corpus: {name}")
    return [str(ROOT / entry) for entry in entries]


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "build_jsons",
        nargs="*",
        metavar="BUILD_JSON",
        help="Build JSON files to compare (exactly 2 required unless --build/--batch used)",
    )
    parser.add_argument(
        "--object",
        action="store_true",
        help="Compare .o files instead of linked binaries (avoids linker UUID nondeterminism)",
    )
    parser.add_argument(
        "--build",
        metavar="SOURCE",
        help="Self-contained mode: build SOURCE twice in isolated caches and compare",
    )
    parser.add_argument(
        "--batch",
        nargs="+",
        metavar="SOURCE",
        help="Batch mode: build each source twice and report reproducibility for all",
    )
    parser.add_argument(
        "--corpus",
        choices=("smoke", "full"),
        help="Repository-owned source corpus (alternative to --batch)",
    )
    parser.add_argument(
        "--build-profile",
        default="dev",
        help="Molt build profile for --build/--batch modes (default: dev)",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=2,
        help="Independent observations per source (minimum and default: 2)",
    )
    parser.add_argument(
        "--audit-ir",
        action="store_true",
        help="Also compare canonical frontend IR across fresh processes",
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
    if args.batch and args.corpus:
        parser.error("choose only one of --batch and --corpus")
    batch = args.batch or (resolve_corpus(args.corpus) if args.corpus else None)

    # Mode: --batch (multiple sources)
    if batch:
        results = []
        passed = 0
        failed = 0
        errors = 0

        for source in batch:
            if not Path(source).exists():
                print(f"  SKIP {source} (not found)")
                errors += 1
                results.append({"source": source, "error": "not found"})
                continue

            print(f"  Testing {source} ...")
            match, details = _build_repeated_and_compare(
                source,
                args.build_profile,
                args.object,
                args.verbose,
                args.runs,
            )
            results.append(details)

            if "error" in details:
                print(f"  ERROR {source}: {details['error']}")
                errors += 1
            elif match:
                print(f"  PASS  {source}")
                passed += 1
            else:
                print(
                    f"  FAIL  {source}  "
                    f"({details.get('unique_hashes', '?')} distinct hashes)"
                )
                failed += 1

        audits: list[dict] = []
        existing_sources = [Path(source) for source in batch if Path(source).is_file()]
        if args.audit_ir:
            audits.extend(check_ir_determinism(existing_sources, args.runs))
        for audit in audits:
            status = audit["status"]
            if status == "pass":
                passed += 1
            elif status == "fail":
                failed += 1
            else:
                errors += 1

        total = passed + failed + errors
        print(
            f"\nReproducible build sweep: {total} files | {passed} pass | {failed} fail | {errors} error"
        )

        _write_proof_receipt(
            args.json_out,
            mode="batch",
            selected=total,
            executed=passed + failed,
            passed=passed,
            failed=failed,
            errors=errors,
            runs_per_source=args.runs,
            results=results,
            audits=audits,
        )

        return fail_closed_proof_exit_code(
            executed=passed + failed,
            failed=failed,
            errors=errors,
        )

    # Mode: --build (single source, self-contained)
    if args.build:
        source = args.build
        if not Path(source).exists():
            print(f"ERROR: Source file not found: {source}", file=sys.stderr)
            _write_proof_receipt(
                args.json_out,
                mode="build",
                selected=1,
                executed=0,
                passed=0,
                failed=0,
                errors=1,
                runs_per_source=args.runs,
                results=[{"source": source, "error": "not found"}],
            )
            return 2

        print(f"Reproducible build test: {source}")
        match, details = _build_repeated_and_compare(
            source,
            args.build_profile,
            args.object,
            verbose=True,
            runs=args.runs,
        )

        if "error" in details:
            print(f"\nERROR: {details['error']}", file=sys.stderr)
            _write_proof_receipt(
                args.json_out,
                mode="build",
                selected=1,
                executed=0,
                passed=0,
                failed=0,
                errors=1,
                runs_per_source=args.runs,
                results=[details],
            )
            return 2

        _write_proof_receipt(
            args.json_out,
            mode="build",
            selected=1,
            executed=1,
            passed=int(match),
            failed=int(not match),
            errors=0,
            runs_per_source=args.runs,
            results=[details],
        )

        if match:
            print(f"\nREPRODUCIBLE: All {args.runs} builds are bit-identical.")
            return 0
        else:
            print(
                f"\nFAILED: Artifacts differ across {args.runs} independent builds."
            )
            return 1

    # Mode: compare two pre-built JSON files
    if len(args.build_jsons) != 2:
        parser.error("Exactly 2 build JSON files required (or use --build/--batch)")

    for label, path in [
        ("Build 1", args.build_jsons[0]),
        ("Build 2", args.build_jsons[1]),
    ]:
        if not Path(path).exists():
            print(f"ERROR: {label} JSON file not found: {path}", file=sys.stderr)
            _write_proof_receipt(
                args.json_out,
                mode="compare",
                selected=1,
                executed=0,
                passed=0,
                failed=0,
                errors=1,
                inputs=args.build_jsons,
                error=f"{label} JSON file not found",
            )
            return 2

    try:
        with open(args.build_jsons[0]) as f:
            build1 = json.load(f)
        with open(args.build_jsons[1]) as f:
            build2 = json.load(f)
    except json.JSONDecodeError as e:
        print(f"ERROR: Invalid JSON: {e}", file=sys.stderr)
        _write_proof_receipt(
            args.json_out,
            mode="compare",
            selected=1,
            executed=0,
            passed=0,
            failed=0,
            errors=1,
            inputs=args.build_jsons,
            error=f"invalid JSON: {e}",
        )
        return 2

    try:
        artifact1 = extract_artifact_path(build1, prefer_object=args.object)
        artifact2 = extract_artifact_path(build2, prefer_object=args.object)
    except KeyError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        _write_proof_receipt(
            args.json_out,
            mode="compare",
            selected=1,
            executed=0,
            passed=0,
            failed=0,
            errors=1,
            inputs=args.build_jsons,
            error=str(e),
        )
        return 2

    if not Path(artifact1).exists():
        print(f"ERROR: Artifact not found: {artifact1}", file=sys.stderr)
        _write_proof_receipt(
            args.json_out,
            mode="compare",
            selected=1,
            executed=0,
            passed=0,
            failed=0,
            errors=1,
            inputs=args.build_jsons,
            error=f"artifact not found: {artifact1}",
        )
        return 2
    if not Path(artifact2).exists():
        print(f"ERROR: Artifact not found: {artifact2}", file=sys.stderr)
        _write_proof_receipt(
            args.json_out,
            mode="compare",
            selected=1,
            executed=0,
            passed=0,
            failed=0,
            errors=1,
            inputs=args.build_jsons,
            error=f"artifact not found: {artifact2}",
        )
        return 2

    match, details = compare_artifacts(artifact1, artifact2)
    _write_proof_receipt(
        args.json_out,
        mode="compare",
        selected=1,
        executed=1,
        passed=int(match),
        failed=int(not match),
        errors=0,
        inputs=args.build_jsons,
        results=[details],
    )

    print(f"Build 1: {artifact1}")
    print(f"  SHA256: {details['sha256_1']}  ({details['size_1']} bytes)")
    print(f"Build 2: {artifact2}")
    print(f"  SHA256: {details['sha256_2']}  ({details['size_2']} bytes)")

    if match:
        print("\nREPRODUCIBLE: Artifacts are bit-identical.")
        return 0
    else:
        print("\nFAILED: Artifacts differ!")
        size1, size2 = details["size_1"], details["size_2"]
        if size1 != size2:
            print(
                f"  Size differs: {size1} vs {size2} bytes ({abs(size1 - size2)} byte delta)"
            )
        else:
            print(f"  Same size ({size1} bytes) but different content")
        if "first_diff_offset" in details:
            print(
                f"  First byte difference at offset {details['first_diff_offset']} "
                f"(0x{details['first_diff_offset']:x})"
            )
        return 1


if __name__ == "__main__":
    sys.exit(main())
