"""Integration tests for the backend daemon sequential request handling.

These tests verify that the daemon can process multiple consecutive requests
without stalling.  The original bug report described the daemon hanging when
benchmarks sent several compile requests in sequence.

The tests start a real daemon subprocess, exercise the socket protocol with
sequential requests, and assert that every request completes within a tight
per-request deadline.  A single stall would cause the test to timeout and
surface as a clear regression.
"""
from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest
import molt.cli as cli


ROOT = Path(__file__).resolve().parents[2]

# Re-use the same protocol version constant that cli.py uses.
_BACKEND_DAEMON_PROTOCOL_VERSION = 1

# How long to wait for a single request round-trip before declaring a stall.
_REQUEST_TIMEOUT_S = 15.0

# How long to wait for the daemon to become ready after spawn.
_STARTUP_TIMEOUT_S = 10.0


def _candidate_daemon_binaries() -> list[Path]:
    """Return the canonical native daemon binary in priority order."""
    profile = "dev-fast"
    path = cli._backend_bin_path(ROOT, profile, ("native-backend",))
    try:
        if cli._ensure_backend_binary(
            path,
            cargo_timeout=180.0,
            json_output=True,
            cargo_profile=profile,
            project_root=ROOT,
            backend_features=("native-backend",),
        ):
            return [path]
    except Exception:
        return []
    return []


def _daemon_binary() -> Path | None:
    bins = _candidate_daemon_binaries()
    return bins[0] if bins else None


def _send_request(
    sock: socket.socket,
    payload: dict,
) -> dict | None:
    """Send a newline-framed JSON request and read the response."""
    data = json.dumps(payload, separators=(",", ":")).encode() + b"\n"
    sock.sendall(data)
    raw = bytearray()
    view = bytearray(65536)
    mv = memoryview(view)
    while True:
        n = sock.recv_into(mv)
        if n == 0:
            break
        raw.extend(mv[:n])
        if b"\n" in raw:
            raw = raw.partition(b"\n")[0]
            break
    if not raw:
        return None
    return json.loads(raw)


def _ping_daemon(socket_path: Path, *, timeout: float) -> bool:
    """Return True if the daemon replies to a ping within *timeout* seconds."""
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(timeout)
            sock.connect(str(socket_path))
            resp = _send_request(
                sock,
                {"version": _BACKEND_DAEMON_PROTOCOL_VERSION, "ping": True},
            )
            return resp is not None and bool(resp.get("ok")) and bool(resp.get("pong"))
    except OSError:
        return False


def _wait_until_ready(socket_path: Path, deadline: float) -> bool:
    while time.monotonic() < deadline:
        if _ping_daemon(socket_path, timeout=1.0):
            return True
        time.sleep(0.05)
    return False


@pytest.fixture()
def daemon_socket(tmp_path: Path):
    """Start a daemon subprocess and yield the path to its socket.

    The daemon is killed when the fixture tears down.

    Note: Unix domain socket paths on macOS/Linux are limited to ~104 chars
    (SUN_LEN).  We use a short path under /tmp rather than the pytest tmp_path
    which can be much longer.
    """
    binary = _daemon_binary()
    if binary is None:
        pytest.skip("molt-backend binary not found; run 'cargo build -p molt-backend'")

    import hashlib
    # Keep the socket path short: /tmp/mbd-<8hex>.sock stays well under SUN_LEN.
    path_hash = hashlib.sha1(str(tmp_path).encode()).hexdigest()[:8]
    socket_path = Path(tempfile.gettempdir()) / f"mbd-{path_hash}.sock"
    log_path = tmp_path / "daemon.log"

    # Remove any stale socket from a previous interrupted run.
    try:
        socket_path.unlink()
    except FileNotFoundError:
        pass

    with log_path.open("wb") as log_file:
        proc = subprocess.Popen(
            [str(binary), "--daemon", "--socket", str(socket_path)],
            cwd=ROOT,
            stdout=log_file,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )

    deadline = time.monotonic() + _STARTUP_TIMEOUT_S
    ready = _wait_until_ready(socket_path, deadline)
    if not ready:
        proc.terminate()
        proc.wait(timeout=5)
        log_text = log_path.read_text(errors="replace") if log_path.exists() else "(no log)"
        pytest.skip(
            f"Daemon did not become ready within {_STARTUP_TIMEOUT_S}s. "
            f"Log tail:\n{log_text[-1000:]}"
        )

    yield socket_path

    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=2)
    # Clean up the short-lived socket file.
    try:
        socket_path.unlink()
    except FileNotFoundError:
        pass


class TestDaemonSequentialRequests:
    """Verify that the daemon handles consecutive requests without stalling."""

    def test_ten_sequential_pings(self, daemon_socket: Path) -> None:
        """Ten pings in series must all succeed within the per-request deadline."""
        for i in range(10):
            ok = _ping_daemon(daemon_socket, timeout=_REQUEST_TIMEOUT_S)
            assert ok, f"Ping {i + 1}/10 timed out or failed — daemon stalled"

    def test_sequential_pings_on_same_connection(self, daemon_socket: Path) -> None:
        """The daemon must handle multiple requests on a single persistent connection."""
        payload = {"version": _BACKEND_DAEMON_PROTOCOL_VERSION, "ping": True}
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(_REQUEST_TIMEOUT_S)
            sock.connect(str(daemon_socket))
            for i in range(5):
                resp = _send_request(sock, payload)
                assert resp is not None, f"Request {i + 1}/5 got no response"
                assert resp.get("ok"), f"Request {i + 1}/5 reported not ok: {resp}"
                assert resp.get("pong"), f"Request {i + 1}/5 not a pong: {resp}"

    def test_probe_cache_miss_returns_needs_ir(self, daemon_socket: Path) -> None:
        """A probe-cache-only request with no IR must return needs_ir=True, not stall."""
        payload = {
            "version": _BACKEND_DAEMON_PROTOCOL_VERSION,
            "jobs": [
                {
                    "id": "probe0",
                    "is_wasm": False,
                    "output": "/tmp/molt-test-probe.o",
                    "cache_key": "non-existent-key-" + str(time.time()),
                    "probe_cache_only": True,
                }
            ],
        }
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(_REQUEST_TIMEOUT_S)
            sock.connect(str(daemon_socket))
            resp = _send_request(sock, payload)

        assert resp is not None, "Daemon returned no response to probe request"
        assert resp.get("ok"), f"Probe request failed: {resp}"
        jobs = resp.get("jobs", [])
        assert jobs, "Response has no jobs"
        assert jobs[0].get("needs_ir"), "Cache miss probe must set needs_ir=True"
        assert not jobs[0].get("cached"), "Cache miss probe must not set cached=True"

    def test_five_sequential_probe_requests_no_stall(self, daemon_socket: Path) -> None:
        """Five back-to-back probe requests on separate connections must not stall.

        This is the regression test for the original bug: the daemon stalled
        when benchmarks sent several compile requests sequentially.
        """
        for i in range(5):
            payload = {
                "version": _BACKEND_DAEMON_PROTOCOL_VERSION,
                "jobs": [
                    {
                        "id": f"probe{i}",
                        "is_wasm": False,
                        "output": f"/tmp/molt-test-probe-{i}.o",
                        "cache_key": f"non-existent-key-{i}-{time.time()}",
                        "probe_cache_only": True,
                    }
                ],
            }
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                    sock.settimeout(_REQUEST_TIMEOUT_S)
                    sock.connect(str(daemon_socket))
                    resp = _send_request(sock, payload)
            except OSError as exc:
                pytest.fail(f"Request {i + 1}/5 raised OSError (daemon stalled?): {exc}")

            assert resp is not None, f"Request {i + 1}/5 got no response (daemon stalled?)"
            assert resp.get("ok"), f"Request {i + 1}/5 failed: {resp}"

    def test_health_reported_in_ping_response(self, daemon_socket: Path) -> None:
        """Daemon health stats must be present in ping responses."""
        payload = {
            "version": _BACKEND_DAEMON_PROTOCOL_VERSION,
            "ping": True,
            "include_health": True,
        }
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                sock.settimeout(_REQUEST_TIMEOUT_S)
                sock.connect(str(daemon_socket))
                resp = _send_request(sock, payload)
        except OSError as exc:
            pytest.fail(f"Health ping failed: {exc}")

        assert resp is not None
        assert resp.get("ok")
        assert resp.get("pong")
        health = resp.get("health")
        assert isinstance(health, dict), f"Expected health dict, got: {health!r}"
        assert health.get("protocol_version") == _BACKEND_DAEMON_PROTOCOL_VERSION
        assert isinstance(health.get("requests_total"), int)

    def test_invalid_version_returns_error(self, daemon_socket: Path) -> None:
        """A request with an invalid protocol version must get an error, not hang."""
        payload = {"version": 999, "ping": True}
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                sock.settimeout(_REQUEST_TIMEOUT_S)
                sock.connect(str(daemon_socket))
                resp = _send_request(sock, payload)
        except OSError as exc:
            pytest.fail(f"Version-mismatch request raised OSError: {exc}")

        assert resp is not None
        assert not resp.get("ok")
        assert resp.get("error"), "Expected an error message for version mismatch"

    def test_daemon_processes_after_closed_probe_connection(self, daemon_socket: Path) -> None:
        """Daemon must be responsive after a client closes a probe connection mid-protocol.

        In the probe-then-compile pattern the client opens a connection for the
        probe, and closes it (without sending the full request) when the cache
        hits.  The daemon must not get stuck waiting for the next read after
        that EOF and must serve the next connection normally.
        """
        # Open a connection, send a probe request, read the response, then
        # close WITHOUT sending a follow-up — simulates a cache-hit path.
        probe_payload = {
            "version": _BACKEND_DAEMON_PROTOCOL_VERSION,
            "jobs": [
                {
                    "id": "probe-close",
                    "is_wasm": False,
                    "output": "/tmp/molt-test-probe-close.o",
                    "cache_key": f"non-existent-{time.time()}",
                    "probe_cache_only": True,
                }
            ],
        }
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(_REQUEST_TIMEOUT_S)
            sock.connect(str(daemon_socket))
            _send_request(sock, probe_payload)
            # Deliberately close without a follow-up — simulates probe cache-hit.

        # The daemon must now answer a normal ping promptly.
        ok = _ping_daemon(daemon_socket, timeout=_REQUEST_TIMEOUT_S)
        assert ok, "Daemon became unresponsive after a probe connection was closed early"
