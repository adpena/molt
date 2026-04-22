"""Cloudflare Worker latency baseline test.

Measures DNS resolution, TLS handshake, TTFB, and total response time
for the Falcon-OCR Cloudflare Worker. Reports p50/p95/p99 latencies.
"""

import json
import socket
import ssl
import statistics
import time
import urllib.request
import urllib.error
from dataclasses import dataclass, field


@dataclass
class LatencyMeasurement:
    """Individual request latency breakdown."""
    dns_ms: float = 0.0
    tls_ms: float = 0.0
    ttfb_ms: float = 0.0
    total_ms: float = 0.0
    status_code: int = 0
    error: str | None = None


@dataclass
class LatencyReport:
    """Aggregate latency report with percentiles."""
    endpoint: str
    measurements: list[LatencyMeasurement] = field(default_factory=list)

    def _percentile(self, values: list[float], p: float) -> float:
        if not values:
            return 0.0
        sorted_v = sorted(values)
        idx = (len(sorted_v) - 1) * p / 100.0
        lower = int(idx)
        upper = min(lower + 1, len(sorted_v) - 1)
        weight = idx - lower
        return sorted_v[lower] * (1 - weight) + sorted_v[upper] * weight

    def summary(self) -> dict:
        successful = [m for m in self.measurements if m.error is None]
        failed = [m for m in self.measurements if m.error is not None]

        if not successful:
            return {
                "endpoint": self.endpoint,
                "total_requests": len(self.measurements),
                "failures": len(failed),
                "error": "all requests failed",
            }

        dns_vals = [m.dns_ms for m in successful]
        tls_vals = [m.tls_ms for m in successful]
        ttfb_vals = [m.ttfb_ms for m in successful]
        total_vals = [m.total_ms for m in successful]

        return {
            "endpoint": self.endpoint,
            "total_requests": len(self.measurements),
            "successful": len(successful),
            "failures": len(failed),
            "dns_ms": {
                "p50": round(self._percentile(dns_vals, 50), 2),
                "p95": round(self._percentile(dns_vals, 95), 2),
                "p99": round(self._percentile(dns_vals, 99), 2),
            },
            "tls_ms": {
                "p50": round(self._percentile(tls_vals, 50), 2),
                "p95": round(self._percentile(tls_vals, 95), 2),
                "p99": round(self._percentile(tls_vals, 99), 2),
            },
            "ttfb_ms": {
                "p50": round(self._percentile(ttfb_vals, 50), 2),
                "p95": round(self._percentile(ttfb_vals, 95), 2),
                "p99": round(self._percentile(ttfb_vals, 99), 2),
            },
            "total_ms": {
                "p50": round(self._percentile(total_vals, 50), 2),
                "p95": round(self._percentile(total_vals, 95), 2),
                "p99": round(self._percentile(total_vals, 99), 2),
                "min": round(min(total_vals), 2),
                "max": round(max(total_vals), 2),
                "mean": round(statistics.mean(total_vals), 2),
                "stdev": round(statistics.stdev(total_vals), 2) if len(total_vals) > 1 else 0.0,
            },
        }


def measure_request(url: str) -> LatencyMeasurement:
    """Measure a single HTTP request with timing breakdown."""
    m = LatencyMeasurement()

    # Parse URL
    from urllib.parse import urlparse
    parsed = urlparse(url)
    host = parsed.hostname or ""
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    path = parsed.path or "/"

    try:
        # DNS resolution
        t0 = time.perf_counter()
        addr_info = socket.getaddrinfo(host, port, socket.AF_UNSPEC, socket.SOCK_STREAM)
        t1 = time.perf_counter()
        m.dns_ms = (t1 - t0) * 1000

        if not addr_info:
            m.error = "DNS resolution returned no results"
            return m

        # TCP + TLS connection
        raw_sock = socket.socket(addr_info[0][0], socket.SOCK_STREAM)
        raw_sock.settimeout(10)
        raw_sock.connect(addr_info[0][4])

        t2 = time.perf_counter()
        if parsed.scheme == "https":
            ctx = ssl.create_default_context()
            sock = ctx.wrap_socket(raw_sock, server_hostname=host)
        else:
            sock = raw_sock
        t3 = time.perf_counter()
        m.tls_ms = (t3 - t2) * 1000

        # Send HTTP request
        request_line = f"GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: molt-latency-test/1.0\r\n\r\n"
        sock.sendall(request_line.encode())

        # Read first byte (TTFB)
        t4 = time.perf_counter()
        first_byte = sock.recv(1)
        t5 = time.perf_counter()
        m.ttfb_ms = (t5 - t4) * 1000

        # Read rest of response
        response_data = first_byte
        while True:
            chunk = sock.recv(4096)
            if not chunk:
                break
            response_data += chunk
        t6 = time.perf_counter()
        m.total_ms = (t6 - t0) * 1000

        # Parse status code from raw HTTP response
        response_str = response_data.decode("utf-8", errors="replace")
        status_line = response_str.split("\r\n", 1)[0]
        parts = status_line.split(" ", 2)
        if len(parts) >= 2:
            try:
                m.status_code = int(parts[1])
            except ValueError:
                m.status_code = 0

        sock.close()

    except socket.timeout:
        m.error = "timeout"
        m.total_ms = 10000.0
    except ConnectionRefusedError:
        m.error = "connection_refused"
    except ssl.SSLError as e:
        m.error = f"ssl_error: {e}"
    except OSError as e:
        m.error = f"os_error: {e}"

    return m


def run_latency_test(url: str, num_requests: int = 10) -> LatencyReport:
    """Run multiple requests and collect latency measurements."""
    report = LatencyReport(endpoint=url)

    for i in range(num_requests):
        m = measure_request(url)
        report.measurements.append(m)
        status = f"HTTP {m.status_code}" if m.error is None else m.error
        print(f"  [{i+1:2d}/{num_requests}] {status} — "
              f"dns={m.dns_ms:.1f}ms tls={m.tls_ms:.1f}ms "
              f"ttfb={m.ttfb_ms:.1f}ms total={m.total_ms:.1f}ms")

    return report


def print_report(report: LatencyReport) -> None:
    """Print a formatted latency report."""
    s = report.summary()
    print(f"\n{'='*70}")
    print(f"Endpoint: {s['endpoint']}")
    print(f"Requests: {s['total_requests']} total, "
          f"{s.get('successful', 0)} successful, {s['failures']} failed")

    if "error" in s:
        print(f"Error: {s['error']}")
        return

    print(f"\n{'Metric':<12} {'p50':>10} {'p95':>10} {'p99':>10}")
    print(f"{'-'*42}")
    for metric in ("dns_ms", "tls_ms", "ttfb_ms", "total_ms"):
        vals = s[metric]
        if isinstance(vals, dict):
            print(f"{metric:<12} {vals['p50']:>10.2f} {vals['p95']:>10.2f} {vals['p99']:>10.2f}")

    total = s["total_ms"]
    print(f"\nTotal: min={total['min']:.2f}ms max={total['max']:.2f}ms "
          f"mean={total['mean']:.2f}ms stdev={total['stdev']:.2f}ms")


def main() -> None:
    base_url = "https://falcon-ocr.adpena.workers.dev"

    print("Falcon-OCR Worker Latency Test")
    print(f"Target: {base_url}\n")

    # Test /health endpoint
    print("--- /health endpoint (10 requests) ---")
    health_report = run_latency_test(f"{base_url}/health", num_requests=10)
    print_report(health_report)

    # Test /ocr endpoint (expect 503 or similar error)
    print("\n--- /ocr endpoint (5 requests, expect error) ---")
    ocr_report = run_latency_test(f"{base_url}/ocr", num_requests=5)
    print_report(ocr_report)

    # JSON output for programmatic consumption
    combined = {
        "health": health_report.summary(),
        "ocr": ocr_report.summary(),
    }
    json_path = Path(__file__).parent / "test_images" / "latency_results.json"
    json_path.write_text(json.dumps(combined, indent=2))
    print(f"\nJSON results written to {json_path}")


if __name__ == "__main__":
    from pathlib import Path
    main()
