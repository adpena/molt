#!/usr/bin/env python3
"""Verify a Cloudflare demo worker's endpoints, from source or over HTTP.

The one endpoint verifier for both demo workers: the Molt demo
(``examples/cloudflare-demo``; case sets ``matrix``, ``fuzz`` and ``all``) and
the Falcon-OCR worker (``deploy/cloudflare``; case set ``falcon-ocr``, HTTP
only). After a deploy, pass ``--wrangler-log`` with wrangler's output to verify
the URL wrangler printed. Summaries go under canonical roots; the exit status is
1 when a case fails.
"""

from __future__ import annotations

import argparse
import sys
from datetime import datetime, timezone
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

REPO_ROOT = Path(__file__).resolve().parents[1]


from tools.cloudflare_demo_verify import (  # noqa: E402 - repo root must be added first.
    MatrixFailed,
    VerificationReport,
    build_demo_fuzz_matrix,
    build_demo_matrix,
    build_falcon_ocr_matrix,
    extract_live_url,
    verify_http_matrix,
    verify_source_matrix,
)

# Case sets that have no app source to run.
_HTTP_ONLY_CASE_SETS = frozenset({"falcon-ocr"})


def _stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")


def _default_artifact_root() -> Path:
    return Path("logs") / "cloudflare_demo_verify" / _stamp()


def _default_tmp_root() -> Path:
    from molt.dx import scratch_dir

    return scratch_dir(REPO_ROOT, "cloudflare_demo_verify") / _stamp()


def _select_cases(case_set: str):
    if case_set == "matrix":
        return build_demo_matrix()
    if case_set == "fuzz":
        return build_demo_fuzz_matrix()
    if case_set == "falcon-ocr":
        return build_falcon_ocr_matrix()
    return build_demo_matrix() + build_demo_fuzz_matrix()


def _base_url(args: argparse.Namespace) -> str | None:
    if args.wrangler_log is None:
        return args.base_url
    url = extract_live_url(args.wrangler_log.read_text(encoding="utf-8"))
    if url is None:
        raise SystemExit(f"no workers.dev URL in {args.wrangler_log}")
    return url


def _print_report(report: VerificationReport) -> None:
    passed = sum(case.ok for case in report.cases)
    print(
        f"{report.transport} {report.case_set}: {passed}/{len(report.cases)} cases "
        f"passed; summary in {report.artifact_root}"
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--entry",
        type=Path,
        default=Path("examples/cloudflare-demo/src/app.py"),
        help="Demo entry script for source verification.",
    )
    url_source = parser.add_mutually_exclusive_group()
    url_source.add_argument(
        "--base-url",
        help="Local or live worker base URL for HTTP verification.",
    )
    url_source.add_argument(
        "--wrangler-log",
        type=Path,
        help="wrangler deploy output; verifies the last workers.dev URL it names.",
    )
    parser.add_argument(
        "--case-set",
        choices=("matrix", "fuzz", "all", "falcon-ocr"),
        default="all",
        help="Which endpoint set to verify.",
    )
    parser.add_argument(
        "--transport",
        choices=("source", "http", "both"),
        default=None,
        help="Verification transport. Defaults to source when no URL is supplied.",
    )
    parser.add_argument(
        "--attempts",
        type=int,
        default=1,
        help="HTTP attempts per case, so a fresh deployment can converge.",
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

    base_url = _base_url(args)
    artifact_root = args.artifact_root or _default_artifact_root()
    tmp_root = args.tmp_root or _default_tmp_root()
    cases = _select_cases(args.case_set)
    transport = args.transport or ("http" if base_url else "source")

    if transport in {"http", "both"} and not base_url:
        parser.error("HTTP verification needs --base-url or --wrangler-log")
    if transport in {"source", "both"} and args.case_set in _HTTP_ONLY_CASE_SETS:
        parser.error(f"case set {args.case_set} has no source transport")

    try:
        if transport in {"source", "both"}:
            _print_report(
                verify_source_matrix(
                    args.entry,
                    cases,
                    artifact_root=artifact_root / "source",
                    tmp_root=tmp_root,
                    case_set=args.case_set,
                )
            )
        if transport in {"http", "both"}:
            _print_report(
                verify_http_matrix(
                    base_url,
                    cases,
                    artifact_root=artifact_root / "http",
                    tmp_root=tmp_root,
                    case_set=args.case_set,
                    attempts=args.attempts,
                )
            )
    except MatrixFailed as exc:
        for failure in exc.report.failures:
            print(f"FAIL {failure}", file=sys.stderr)
        _print_report(exc.report)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
