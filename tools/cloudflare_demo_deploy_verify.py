#!/usr/bin/env python3
"""Post-deploy live endpoint sweep for the Cloudflare Demo Worker.

Probes each canonical endpoint against a live Cloudflare Worker URL, checks
status codes and body sentinels, retries on failure, and writes a JSON report.

Usage::

    python3 tools/cloudflare_demo_deploy_verify.py --live-base-url https://your-worker.workers.dev
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional


# ---------------------------------------------------------------------------
# Probe matrix
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class ProbeSpec:
    path: str
    expected_status: int
    expected_content_type_prefix: str  # checked as prefix, e.g. "text/html"
    sentinel: Optional[str] = None     # substring that must appear in body


PROBE_PATHS: list[ProbeSpec] = [
    ProbeSpec("/",            200, "text/html",  None),
    ProbeSpec("/fib/10",      200, "text/plain", "55"),
    ProbeSpec("/primes/100",  200, "text/plain", None),
    ProbeSpec("/diamond/5",   200, "text/plain", None),
    ProbeSpec("/fizzbuzz/15", 200, "text/plain", "FizzBuzz"),
    ProbeSpec("/pi/1000",     200, "text/plain", "3.14"),
    ProbeSpec("/generate/1",  200, "text/plain", None),
    ProbeSpec("/bench",       200, "text/plain", None),
    ProbeSpec("/demo",        200, "text/html",  None),
]


# ---------------------------------------------------------------------------
# Result dataclasses
# ---------------------------------------------------------------------------

@dataclass
class ProbeResult:
    path: str
    passed: bool
    status_code: Optional[int] = None
    content_type: Optional[str] = None
    body_snippet: Optional[str] = None   # first 256 chars of body for diagnostics
    failure_reason: Optional[str] = None
    attempts: int = 0


@dataclass
class DeployVerifyReport:
    live_base_url: str
    timestamp_utc: str
    retries_allowed: int
    total: int = 0
    passed: int = 0
    failed: int = 0
    results: list[ProbeResult] = field(default_factory=list)

    @property
    def all_passed(self) -> bool:
        return self.failed == 0


# ---------------------------------------------------------------------------
# HTTP probe logic
# ---------------------------------------------------------------------------

_TIMEOUT_SECONDS = 15
_RETRY_DELAY_SECONDS = 2


def _probe_once(url: str, spec: ProbeSpec) -> ProbeResult:
    """Perform a single HTTP GET and validate the response against *spec*."""
    full_url = url.rstrip("/") + spec.path
    result = ProbeResult(path=spec.path, passed=False)

    try:
        req = urllib.request.Request(full_url, headers={"User-Agent": "molt-deploy-verify/1.0"})
        with urllib.request.urlopen(req, timeout=_TIMEOUT_SECONDS) as resp:
            result.status_code = resp.status
            result.content_type = resp.headers.get("Content-Type", "")
            raw_body = resp.read()

            # Reject NUL-prefixed output (corrupted WASM response)
            if raw_body and raw_body[0] == 0:
                result.failure_reason = "NUL-prefixed response body (WASM corruption)"
                return result

            body = raw_body.decode("utf-8", errors="replace")
            result.body_snippet = body[:256]

            # Reject Cloudflare error pages
            if "Error 1102" in body:
                result.failure_reason = "Cloudflare Error 1102 in response body"
                return result

            # Check status code
            if result.status_code != spec.expected_status:
                result.failure_reason = (
                    f"Expected status {spec.expected_status}, got {result.status_code}"
                )
                return result

            # Check content-type prefix
            ct = result.content_type or ""
            if not ct.startswith(spec.expected_content_type_prefix):
                result.failure_reason = (
                    f"Expected Content-Type prefix '{spec.expected_content_type_prefix}', "
                    f"got '{ct}'"
                )
                return result

            # Check sentinel substring
            if spec.sentinel is not None and spec.sentinel not in body:
                result.failure_reason = (
                    f"Sentinel '{spec.sentinel}' not found in body"
                )
                return result

            result.passed = True
            return result

    except urllib.error.HTTPError as exc:
        result.status_code = exc.code
        result.failure_reason = f"HTTP error {exc.code}: {exc.reason}"
        return result
    except urllib.error.URLError as exc:
        result.failure_reason = f"URL error: {exc.reason}"
        return result
    except TimeoutError:
        result.failure_reason = f"Request timed out after {_TIMEOUT_SECONDS}s"
        return result
    except Exception as exc:  # noqa: BLE001 - capture all network errors for reporting
        result.failure_reason = f"Unexpected error: {exc}"
        return result


def probe_with_retries(base_url: str, spec: ProbeSpec, retries: int) -> ProbeResult:
    """Probe *spec* against *base_url*, retrying up to *retries* times on failure."""
    last_result: Optional[ProbeResult] = None
    for attempt in range(1, retries + 2):  # attempts = retries + 1 initial try
        result = _probe_once(base_url, spec)
        result.attempts = attempt
        if result.passed:
            return result
        last_result = result
        if attempt <= retries:
            time.sleep(_RETRY_DELAY_SECONDS)
    return last_result  # type: ignore[return-value]  # loop runs at least once


# ---------------------------------------------------------------------------
# Report + console output
# ---------------------------------------------------------------------------

def _write_report(report: DeployVerifyReport, artifact_root: Path) -> Path:
    artifact_root.mkdir(parents=True, exist_ok=True)
    report_path = artifact_root / "deploy_verify_report.json"
    report_path.write_text(
        json.dumps(asdict(report), indent=2),
        encoding="utf-8",
    )
    return report_path


def _print_summary(report: DeployVerifyReport) -> None:
    width = 72
    print("=" * width)
    print(f"  Cloudflare Deploy Verify — {report.live_base_url}")
    print(f"  Timestamp : {report.timestamp_utc}")
    print(f"  Retries   : {report.retries_allowed}")
    print("-" * width)
    for r in report.results:
        status_tag = "PASS" if r.passed else "FAIL"
        attempts_tag = f"(attempt {r.attempts})" if r.attempts > 1 else ""
        line = f"  [{status_tag}] {r.path:<22} {attempts_tag}"
        if not r.passed and r.failure_reason:
            line += f"\n         reason: {r.failure_reason}"
            if r.body_snippet:
                snippet = r.body_snippet.replace("\n", " ")[:120]
                line += f"\n         body  : {snippet}"
        print(line)
    print("-" * width)
    print(
        f"  Result: {report.passed}/{report.total} passed"
        + ("  ✓ ALL PASSED" if report.all_passed else "  ✗ FAILURES DETECTED")
    )
    print("=" * width)


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--live-base-url",
        required=True,
        metavar="URL",
        help="Live Cloudflare Worker base URL, e.g. https://demo.workers.dev",
    )
    parser.add_argument(
        "--artifact-root",
        type=Path,
        default=Path("logs/cloudflare_deploy"),
        metavar="DIR",
        help="Directory for the JSON report (default: logs/cloudflare_deploy)",
    )
    parser.add_argument(
        "--retries",
        type=int,
        default=2,
        metavar="N",
        help="Number of retries per endpoint on failure (default: 2)",
    )
    args = parser.parse_args(argv)

    timestamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    report = DeployVerifyReport(
        live_base_url=args.live_base_url,
        timestamp_utc=timestamp,
        retries_allowed=args.retries,
    )

    for spec in PROBE_PATHS:
        result = probe_with_retries(args.live_base_url, spec, args.retries)
        report.results.append(result)
        report.total += 1
        if result.passed:
            report.passed += 1
        else:
            report.failed += 1

    report_path = _write_report(report, args.artifact_root)
    _print_summary(report)
    print(f"\n  Report written to: {report_path}")

    return 0 if report.all_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
