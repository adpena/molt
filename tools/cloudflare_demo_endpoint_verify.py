#!/usr/bin/env python3
"""Run the Cloudflare demo endpoint matrix and fuzz sweeps.

This wrapper uses the shared verifier helpers from
``tools.cloudflare_demo_verify`` and writes summaries under canonical roots.
"""

from __future__ import annotations

import argparse
import sys
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools.cloudflare_demo_verify import (  # noqa: E402 - repo root must be added first.
    build_demo_fuzz_matrix,
    build_demo_matrix,
    verify_http_matrix,
    verify_source_matrix,
)


def _stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")


def _default_artifact_root() -> Path:
    return Path("logs") / "cloudflare_demo_verify" / _stamp()


def _default_tmp_root() -> Path:
    return Path("tmp") / "cloudflare_demo_verify" / _stamp()


def _select_cases(case_set: str):
    if case_set == "matrix":
        return build_demo_matrix()
    if case_set == "fuzz":
        return build_demo_fuzz_matrix()
    return build_demo_matrix() + build_demo_fuzz_matrix()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--entry",
        type=Path,
        default=Path("examples/cloudflare-demo/src/app.py"),
        help="Demo entry script for source verification.",
    )
    parser.add_argument(
        "--base-url",
        help="Local or live worker base URL for HTTP verification.",
    )
    parser.add_argument(
        "--case-set",
        choices=("matrix", "fuzz", "all"),
        default="all",
        help="Which endpoint set to verify.",
    )
    parser.add_argument(
        "--transport",
        choices=("source", "http", "both"),
        default=None,
        help="Verification transport. Defaults to source when no base URL is supplied.",
    )
    parser.add_argument(
        "--artifact-root",
        type=Path,
        default=None,
        help="Canonical logs root for verification summaries.",
    )
    parser.add_argument(
        "--tmp-root",
        type=Path,
        default=None,
        help="Canonical scratch root for transient verification files.",
    )
    args = parser.parse_args(argv)

    artifact_root = args.artifact_root or _default_artifact_root()
    tmp_root = args.tmp_root or _default_tmp_root()
    cases = _select_cases(args.case_set)
    transport = args.transport or ("http" if args.base_url else "source")

    if transport in {"http", "both"} and not args.base_url:
        raise RuntimeError("--base-url is required for HTTP verification")

    if transport in {"source", "both"}:
        verify_source_matrix(
            args.entry,
            cases,
            artifact_root=artifact_root / "source",
            tmp_root=tmp_root,
            case_set=args.case_set,
        )
    if transport in {"http", "both"}:
        verify_http_matrix(
            args.base_url,
            cases,
            artifact_root=artifact_root / "http",
            tmp_root=tmp_root,
            case_set=args.case_set,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
