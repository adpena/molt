from __future__ import annotations

import argparse
import importlib
import json
import sys
from pathlib import Path
from typing import Sequence

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT / "src") not in sys.path:
    sys.path.insert(0, str(ROOT / "src"))

source_extensions = importlib.import_module("molt.cli.source_extensions")

REQUIRED_FP_FLAGS = ("-fno-fast-math", "-ffp-contract=off")
FORBIDDEN_FP_FLAGS = (
    "-ffast-math",
    "-funsafe-math-optimizations",
    "-fassociative-math",
    "-freciprocal-math",
    "-fno-signed-zeros",
    "-ffp-contract=fast",
    "-ffp-contract=on",
)


def audit_compile_args(args: Sequence[str]) -> list[str]:
    errors = []
    missing = [flag for flag in REQUIRED_FP_FLAGS if flag not in args]
    if missing:
        errors.append("missing deterministic FP flags: " + ", ".join(missing))
    unsafe = [flag for flag in FORBIDDEN_FP_FLAGS if flag in args]
    if unsafe:
        errors.append("unsafe FP flags present: " + ", ".join(unsafe))
    return errors


def audit_target_manifest(path: Path) -> list[str]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    features = {row["id"]: row for row in payload["features"]}
    errors = []
    simd = features.get("wasm.simd128")
    relaxed = features.get("wasm.relaxed_simd")
    if simd is None or simd.get("determinism") != "deterministic":
        errors.append("wasm.simd128 must remain classified deterministic")
    if relaxed is None or relaxed.get("determinism") != "non_bit_exact":
        errors.append("wasm.relaxed_simd must remain classified non_bit_exact")
    for profile in payload["targets"]:
        rows = {row["id"]: row for row in profile["features"]}
        relaxed_row = rows.get("wasm.relaxed_simd")
        if relaxed_row is None:
            continue
        if relaxed_row.get("gate") != "explicit_non_bit_exact_profile":
            errors.append(
                f"{profile['id']} relaxed SIMD gate must be explicit_non_bit_exact_profile"
            )
    return errors


def run(*, probe_unsafe_flag: str | None = None) -> int:
    compile_args = source_extensions._source_extension_wasm_compile_args(
        target_triple="wasm32-wasip1",
        cc_cmd=("clang",),
    )
    if probe_unsafe_flag:
        compile_args.append(probe_unsafe_flag)
    errors = audit_compile_args(compile_args)
    errors.extend(audit_target_manifest(ROOT / "wasm" / "target_feature_manifest.json"))
    if errors:
        for error in errors:
            print(f"determinism-perf-gate: ERROR: {error}", file=sys.stderr)
        return 1
    print(
        "determinism-perf-gate: OK "
        f"compile_args={' '.join(compile_args)} relaxed_simd=explicit-only"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe-unsafe-flag")
    args = parser.parse_args()
    return run(probe_unsafe_flag=args.probe_unsafe_flag)


if __name__ == "__main__":
    raise SystemExit(main())
