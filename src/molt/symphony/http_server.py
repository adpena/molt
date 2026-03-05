from __future__ import annotations

import base64
import hashlib
import hmac
import ipaddress
import json
import os
import select
import shlex
import subprocess
import sys
import time
from dataclasses import dataclass, field
from datetime import UTC, datetime
from http import HTTPStatus
from http.cookies import SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import BoundedSemaphore, Lock, Thread
from typing import Any, Callable, Protocol
from urllib.parse import parse_qs, unquote, urlparse, urlsplit

from .dashboard_assets import (
    DASHBOARD_HTML,
    fetch_dashboard_asset,
    fetch_dashboard_kernel_wasm_asset,
)
from .observability_presenter import (
    load_security_events_summary,
    project_state_payload,
)


class StateProvider(Protocol):
    def snapshot_state(self) -> dict[str, Any]: ...

    def snapshot_durable_memory(self, limit: int = 120) -> dict[str, Any]: ...

    def snapshot_issue(self, issue_identifier: str) -> dict[str, Any] | None: ...

    def request_refresh(self) -> bool: ...

    def request_retry_now(self, issue_identifier: str) -> dict[str, Any]: ...

    def run_dashboard_tool(
        self, tool_name: str, payload: dict[str, Any]
    ) -> dict[str, Any]: ...


class _QuietThreadingHTTPServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(
        self,
        server_address: tuple[str, int],
        request_handler_class: type[BaseHTTPRequestHandler],
        *,
        max_active_requests: int = 96,
        on_overload: Callable[[], None] | None = None,
    ) -> None:
        self._request_slots = BoundedSemaphore(max(max_active_requests, 8))
        self._on_overload = on_overload
        super().__init__(server_address, request_handler_class)

    def process_request(self, request: Any, client_address: Any) -> None:
        if not self._request_slots.acquire(blocking=False):
            if self._on_overload is not None:
                try:
                    self._on_overload()
                except Exception:
                    pass
            _reject_connection_overloaded(request)
            return
        try:
            super().process_request(request, client_address)
        except Exception:
            self._request_slots.release()
            raise

    def process_request_thread(self, request: Any, client_address: Any) -> None:
        try:
            super().process_request_thread(request, client_address)
        finally:
            self._request_slots.release()

    def handle_error(self, request: Any, client_address: Any) -> None:
        exc_type, _, _ = sys.exc_info()
        if exc_type in {BrokenPipeError, ConnectionResetError, TimeoutError}:
            return
        super().handle_error(request, client_address)


@dataclass(slots=True)
class _ExternalStateHasher:
    command: list[str]
    timeout_seconds: float
    frame_mode: bool = False
    fallback_command: list[str] | None = None
    _proc: subprocess.Popen[Any] | None = field(default=None, init=False)
    _lock: Lock = field(default_factory=Lock, init=False)
    _enabled: bool = field(default=True, init=False)

    def _ensure_started(self) -> bool:
        if not self._enabled:
            return False
        if self._proc is not None and self._proc.poll() is None:
            return True
        try:
            self._proc = subprocess.Popen(
                self.command,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=not self.frame_mode,
                encoding="utf-8" if not self.frame_mode else None,
                bufsize=1 if not self.frame_mode else 0,
            )
        except OSError:
            self._enabled = False
            self._proc = None
            return False
        return True

    def _read_exact_with_timeout(self, stream: Any, count: int) -> bytes | None:
        if count <= 0:
            return b""
        out = bytearray()
        while len(out) < count:
            if os.name != "nt":
                ready, _, _ = select.select([stream], [], [], self.timeout_seconds)
                if not ready:
                    return None
            chunk = stream.read(count - len(out))
            if not isinstance(chunk, (bytes, bytearray)) or not chunk:
                return None
            out.extend(chunk)
        return bytes(out)

    def _switch_to_fallback_locked(self) -> bool:
        fallback = self.fallback_command
        if not fallback:
            return False
        self._close_locked(disable=False)
        self.command = list(fallback)
        self.frame_mode = False
        self.fallback_command = None
        self._enabled = True
        return self._ensure_started()

    def hash(self, payload_bytes: bytes) -> str | None:
        with self._lock:
            if not self._ensure_started():
                return None
            proc = self._proc
            if proc is None or proc.stdin is None or proc.stdout is None:
                self._enabled = False
                return None
            if self.frame_mode:
                try:
                    proc.stdin.write(
                        len(payload_bytes).to_bytes(4, "big", signed=False)
                    )
                    proc.stdin.write(payload_bytes)
                    proc.stdin.flush()
                    digest = self._read_exact_with_timeout(proc.stdout, 8)
                    if digest is not None:
                        return f'W/"{digest.hex()}"'
                except OSError:
                    pass
                if not self._switch_to_fallback_locked():
                    self._close_locked(disable=True)
                    return None
                proc = self._proc
                if proc is None or proc.stdin is None or proc.stdout is None:
                    self._enabled = False
                    return None
            payload_b64 = base64.b64encode(payload_bytes).decode("ascii")
            try:
                proc.stdin.write(payload_b64 + "\n")
                proc.stdin.flush()
                if os.name != "nt":
                    ready, _, _ = select.select(
                        [proc.stdout], [], [], self.timeout_seconds
                    )
                    if not ready:
                        self._close_locked(disable=True)
                        return None
                line = proc.stdout.readline()
            except OSError:
                self._close_locked(disable=True)
                return None
        etag = line.strip()
        if etag.startswith('W/"') and etag.endswith('"') and len(etag) >= 6:
            return etag
        return None

    def close(self, *, disable: bool = False) -> None:
        with self._lock:
            self._close_locked(disable=disable)

    def _close_locked(self, *, disable: bool = False) -> None:
        if disable:
            self._enabled = False
        proc = self._proc
        self._proc = None
        if proc is None:
            return
        try:
            proc.terminate()
        except OSError:
            return
        try:
            proc.wait(timeout=0.5)
        except (subprocess.TimeoutExpired, OSError):
            try:
                proc.kill()
            except OSError:
                return


@dataclass(slots=True)
class DashboardServer:
    provider: StateProvider
    port: int
    _server: _QuietThreadingHTTPServer | None = field(default=None, init=False)
    _thread: Thread | None = field(default=None, init=False)
    _state_hasher: _ExternalStateHasher | None = field(default=None, init=False)

    def start(self) -> int:
        provider = self.provider
        state_hasher = _state_hasher_from_env()
        security = _dashboard_security_from_env(port=self.port)
        if security.startup_error:
            raise RuntimeError(security.startup_error)
        bind_host, nonlocal_bind = _bind_host_from_env()
        allow_unauthenticated_nonlocal = _coerce_bool_env(
            "MOLT_SYMPHONY_ALLOW_UNAUTHENTICATED_NONLOCAL",
            default=False,
        )
        if (
            nonlocal_bind
            and not security.api_token
            and not allow_unauthenticated_nonlocal
        ):
            raise RuntimeError(
                "Refusing non-loopback dashboard bind without API token. Set "
                "MOLT_SYMPHONY_API_TOKEN (or MOLT_SYMPHONY_DASHBOARD_TOKEN) or "
                "explicitly set MOLT_SYMPHONY_ALLOW_UNAUTHENTICATED_NONLOCAL=1."
            )
        ext_root = Path(
            os.environ.get("MOLT_EXT_ROOT", "/Volumes/APDataStore/Molt")
        ).expanduser()
        security_events_file = Path(
            os.environ.get(
                "MOLT_SYMPHONY_SECURITY_EVENTS_FILE",
                str(ext_root / "logs" / "symphony" / "security" / "events.jsonl"),
            )
        ).expanduser()
        self._state_hasher = state_hasher
        max_stream_clients = _coerce_non_negative_int_env(
            "MOLT_SYMPHONY_MAX_STREAM_CLIENTS", default=16, minimum=1
        )
        stream_max_age_seconds = max(
            _coerce_non_negative_float_env(
                "MOLT_SYMPHONY_STREAM_MAX_AGE_SECONDS",
                default=300.0,
                minimum=30.0,
            ),
            30.0,
        )
        rate_limit_max_requests = _coerce_non_negative_int_env(
            "MOLT_SYMPHONY_HTTP_RATE_LIMIT_MAX_REQUESTS",
            default=240,
            minimum=0,
        )
        rate_limit_window_seconds = max(
            _coerce_non_negative_float_env(
                "MOLT_SYMPHONY_HTTP_RATE_LIMIT_WINDOW_SECONDS",
                default=60.0,
                minimum=1.0,
            ),
            1.0,
        )
        active_stream_clients = 0
        stream_clients_lock = Lock()
        rate_limit_lock = Lock()
        rate_limit_state: dict[str, tuple[int, int]] = {}
        security_counters_lock = Lock()
        security_counters: dict[str, int] = {
            "unauthorized": 0,
            "origin_denied": 0,
            "csrf_denied": 0,
            "rate_limited": 0,
            "overload_rejected": 0,
            "stream_capacity_rejected": 0,
            "method_not_allowed": 0,
            "not_found": 0,
        }
        security_counter_generation = 0
        security_events_cache: dict[str, Any] = {
            "signature": None,
            "payload": {"secret_guard_blocked": {"total": 0, "last_at": None}},
        }
        state_cache_lock = Lock()
        state_cache_ttl_seconds = 0.25
        state_cache: dict[str, Any] = {
            "captured_at_monotonic": 0.0,
            "payload": None,
            "encoded": b"",
            "etag": "",
            "security_counter_generation": -1,
        }

        def _incr_security_counter(name: str, *, inc: int = 1) -> None:
            nonlocal security_counter_generation
            with security_counters_lock:
                security_counters[name] = max(
                    int(security_counters.get(name, 0)) + int(inc), 0
                )
                security_counter_generation = max(
                    int(security_counter_generation) + 1, 0
                )

        def _security_events_snapshot() -> dict[str, Any]:
            try:
                stat = security_events_file.stat()
                signature = (int(stat.st_mtime_ns), int(stat.st_size))
            except OSError:
                signature = None
            with security_counters_lock:
                if signature == security_events_cache.get("signature"):
                    cached = security_events_cache.get("payload")
                    if isinstance(cached, dict):
                        return dict(cached)
            payload = load_security_events_summary(security_events_file, max_lines=2000)
            with security_counters_lock:
                security_events_cache["signature"] = signature
                security_events_cache["payload"] = payload
            return dict(payload)

        def _http_security_snapshot() -> dict[str, Any]:
            with security_counters_lock:
                counters = dict(security_counters)
            return {
                "profile": security.profile,
                "bind_host": bind_host,
                "nonlocal_bind": nonlocal_bind,
                "allow_query_token": security.allow_query_token,
                "dashboard_enabled": not security.disable_dashboard,
                "rate_limit": {
                    "max_requests": rate_limit_max_requests,
                    "window_seconds": rate_limit_window_seconds,
                },
                "counters": counters,
                "events": _security_events_snapshot(),
            }

        def _invalidate_state_cache() -> None:
            with state_cache_lock:
                state_cache["captured_at_monotonic"] = 0.0
                state_cache["payload"] = None
                state_cache["encoded"] = b""
                state_cache["etag"] = ""

        def _snapshot_state_cached() -> tuple[dict[str, Any], bytes, str]:
            now_mono = time.monotonic()
            with security_counters_lock:
                current_security_generation = int(security_counter_generation)
            with state_cache_lock:
                captured_at = float(state_cache.get("captured_at_monotonic") or 0.0)
                encoded_cached = state_cache.get("encoded")
                etag_cached = state_cache.get("etag")
                payload_cached = state_cache.get("payload")
                cached_security_generation = int(
                    state_cache.get("security_counter_generation") or 0
                )
                if (
                    isinstance(encoded_cached, bytes)
                    and encoded_cached
                    and isinstance(etag_cached, str)
                    and etag_cached
                    and isinstance(payload_cached, dict)
                    and cached_security_generation == current_security_generation
                    and (now_mono - captured_at) <= state_cache_ttl_seconds
                ):
                    return payload_cached, encoded_cached, etag_cached
            payload = project_state_payload(
                provider.snapshot_state(),
                http_security=_http_security_snapshot(),
            )
            encoded = _encode_json_bytes(payload)
            etag_payload = _normalize_state_payload_for_etag(payload)
            etag_encoded = _encode_json_bytes(etag_payload)
            etag = _state_etag_for_payload(etag_encoded, hasher=state_hasher)
            with state_cache_lock:
                state_cache["captured_at_monotonic"] = now_mono
                state_cache["payload"] = payload
                state_cache["encoded"] = encoded
                state_cache["etag"] = etag
                state_cache["security_counter_generation"] = current_security_generation
            return payload, encoded, etag

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: object) -> None:  # noqa: A003
                return

            def _auth_token_from_request(
                self, *, query: str, allow_query: bool
            ) -> str | None:
                auth_header = str(self.headers.get("Authorization") or "").strip()
                if auth_header.lower().startswith("bearer "):
                    token = auth_header[7:].strip()
                    if token:
                        return token
                token_header = str(self.headers.get("X-Symphony-Token") or "").strip()
                if token_header:
                    return token_header
                cookie_header = str(self.headers.get("Cookie") or "").strip()
                if cookie_header:
                    parsed = SimpleCookie()
                    try:
                        parsed.load(cookie_header)
                    except Exception:
                        parsed = SimpleCookie()
                    morsel = parsed.get("molt_symphony_token")
                    if morsel is not None:
                        cookie_token = str(morsel.value or "").strip()
                        if cookie_token:
                            return cookie_token
                if allow_query:
                    query_values = parse_qs(query).get("token", [])
                    if query_values:
                        return str(query_values[0] or "").strip() or None
                return None

            def _request_origin(self) -> str | None:
                origin = str(self.headers.get("Origin") or "").strip()
                if origin:
                    return origin
                referer = str(self.headers.get("Referer") or "").strip()
                if not referer:
                    return None
                try:
                    parsed = urlparse(referer)
                except Exception:
                    return None
                if not parsed.scheme or not parsed.netloc:
                    return None
                return f"{parsed.scheme}://{parsed.netloc}"

            def _is_origin_allowed(self, origin: str) -> bool:
                origin_norm = origin.strip().lower()
                if not origin_norm:
                    return False
                if origin_norm in security.allowed_origins:
                    return True
                if security.allow_localhost_any_port:
                    if origin_norm.startswith("http://127.0.0.1:"):
                        return True
                    if origin_norm.startswith("http://localhost:"):
                        return True
                return False

            def _deny_json(
                self,
                status: HTTPStatus,
                code: str,
                message: str,
                *,
                headers: dict[str, str] | None = None,
            ) -> None:
                self._write_json(
                    int(status),
                    {
                        "error": {
                            "code": code,
                            "message": message,
                        }
                    },
                    headers=headers,
                )

            def _rate_limit_principal(
                self,
                *,
                query: str,
                allow_query_token: bool,
                expected_api_token: str | None,
            ) -> str:
                supplied = self._auth_token_from_request(
                    query=query,
                    allow_query=allow_query_token,
                )
                if supplied and (
                    expected_api_token is None
                    or hmac.compare_digest(supplied, expected_api_token)
                ):
                    digest = hashlib.sha256(
                        supplied.encode("utf-8", errors="ignore")
                    ).hexdigest()[:16]
                    return f"token:{digest}"
                peer_host = "unknown"
                client_address = getattr(self, "client_address", None)
                if isinstance(client_address, tuple) and client_address:
                    peer_host = str(client_address[0])
                return f"ip:{peer_host}"

            def _consume_rate_limit(
                self,
                *,
                query: str,
                allow_query_token: bool,
                expected_api_token: str | None,
            ) -> int | None:
                if rate_limit_max_requests <= 0:
                    return None
                now = time.time()
                bucket = int(now // rate_limit_window_seconds)
                principal = self._rate_limit_principal(
                    query=query,
                    allow_query_token=allow_query_token,
                    expected_api_token=expected_api_token,
                )
                with rate_limit_lock:
                    previous = rate_limit_state.get(principal)
                    if previous is None or previous[0] != bucket:
                        count = 1
                    else:
                        count = int(previous[1]) + 1
                    rate_limit_state[principal] = (bucket, count)
                    if len(rate_limit_state) > 8192:
                        stale_keys = [
                            key
                            for key, (seen_bucket, _count) in rate_limit_state.items()
                            if seen_bucket != bucket
                        ]
                        for key in stale_keys:
                            rate_limit_state.pop(key, None)
                if count > rate_limit_max_requests:
                    retry_after = max(
                        int((bucket + 1) * rate_limit_window_seconds - now + 0.999),
                        1,
                    )
                    return retry_after
                return None

            def _authorize_request(
                self,
                *,
                method: str,
                query: str,
                mutating: bool,
                allow_query_token: bool = True,
            ) -> bool:
                _ = method
                auth_header_present = bool(
                    str(self.headers.get("Authorization") or "").strip()
                    or str(self.headers.get("X-Symphony-Token") or "").strip()
                )
                retry_after_seconds = self._consume_rate_limit(
                    query=query,
                    allow_query_token=allow_query_token,
                    expected_api_token=security.api_token,
                )
                if retry_after_seconds is not None:
                    _incr_security_counter("rate_limited")
                    _invalidate_state_cache()
                    self._deny_json(
                        HTTPStatus.TOO_MANY_REQUESTS,
                        "rate_limited",
                        "Request rate limit exceeded. Retry later.",
                        headers={"Retry-After": str(retry_after_seconds)},
                    )
                    return False
                if security.api_token:
                    supplied = self._auth_token_from_request(
                        query=query, allow_query=allow_query_token
                    )
                    if not hmac.compare_digest(str(supplied), security.api_token):
                        _incr_security_counter("unauthorized")
                        self._deny_json(
                            HTTPStatus.UNAUTHORIZED,
                            "unauthorized",
                            "Missing or invalid Symphony API token.",
                        )
                        return False
                if mutating and security.enforce_origin:
                    origin = self._request_origin()
                    if not origin:
                        if security.api_token and not auth_header_present:
                            _incr_security_counter("origin_denied")
                            self._deny_json(
                                HTTPStatus.FORBIDDEN,
                                "forbidden_origin",
                                "Origin is required for cookie/query-token mutating requests.",
                            )
                            return False
                    elif not self._is_origin_allowed(origin):
                        _incr_security_counter("origin_denied")
                        self._deny_json(
                            HTTPStatus.FORBIDDEN,
                            "forbidden_origin",
                            "Origin not allowed for mutating Symphony requests.",
                        )
                        return False
                if mutating and security.require_csrf_header:
                    origin = self._request_origin()
                    if origin:
                        csrf = str(self.headers.get("X-Symphony-CSRF") or "").strip()
                        if csrf != "1":
                            _incr_security_counter("csrf_denied")
                            self._deny_json(
                                HTTPStatus.FORBIDDEN,
                                "missing_csrf_header",
                                "Missing X-Symphony-CSRF header.",
                            )
                            return False
                return True

            def _write_json(
                self,
                status: int,
                payload: dict[str, Any],
                *,
                headers: dict[str, str] | None = None,
            ) -> None:
                data = _encode_json_bytes(payload)
                self._write_json_bytes(status, data, headers=headers)

            def _write_json_bytes(
                self,
                status: int,
                data: bytes,
                *,
                headers: dict[str, str] | None = None,
            ) -> None:
                merged_headers = {
                    "Cache-Control": "no-store, private",
                    "Pragma": "no-cache",
                    "Vary": "Authorization, X-Symphony-Token, Cookie",
                }
                if headers:
                    merged_headers.update(headers)
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self._write_common_security_headers(frame_options="DENY")
                for key, value in merged_headers.items():
                    self.send_header(key, value)
                self.end_headers()
                self.wfile.write(data)

            def _write_common_security_headers(
                self, *, frame_options: str | None
            ) -> None:
                self.send_header("X-Content-Type-Options", "nosniff")
                if frame_options:
                    self.send_header("X-Frame-Options", frame_options)
                self.send_header("Referrer-Policy", "no-referrer")
                self.send_header("Permissions-Policy", "interest-cohort=()")
                self.send_header("Cross-Origin-Opener-Policy", "same-origin")
                self.send_header("Cross-Origin-Resource-Policy", "same-origin")

            def _write_html(self, status: int, body: str) -> None:
                data = body.encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", str(len(data)))
                self._write_common_security_headers(frame_options="DENY")
                self.send_header(
                    "Content-Security-Policy",
                    (
                        "default-src 'self'; "
                        "script-src 'self'; "
                        "style-src 'self'; "
                        "img-src 'self' data:; "
                        "connect-src 'self'; "
                        "object-src 'none'; "
                        "base-uri 'none'; "
                        "frame-ancestors 'none'; "
                        "form-action 'none'"
                    ),
                )
                self.end_headers()
                self.wfile.write(data)

            def _write_static_asset(
                self,
                *,
                content_type: str,
                body: bytes,
                cache_control: str,
                etag: str | None = None,
            ) -> None:
                self.send_response(HTTPStatus.OK)
                self.send_header("Content-Type", content_type)
                self.send_header("Cache-Control", cache_control)
                self.send_header("Content-Length", str(len(body)))
                self._write_common_security_headers(frame_options=None)
                if isinstance(etag, str) and etag:
                    self.send_header("ETag", etag)
                self.end_headers()
                self.wfile.write(body)

            def _write_not_modified(
                self,
                *,
                cache_control: str,
                etag: str,
                include_frame_options: bool,
            ) -> None:
                self.send_response(HTTPStatus.NOT_MODIFIED)
                self.send_header("Cache-Control", cache_control)
                self.send_header("Content-Length", "0")
                self.send_header("ETag", etag)
                self._write_common_security_headers(
                    frame_options="DENY" if include_frame_options else None
                )
                self.end_headers()

            def _is_known_dashboard_route(self, path: str) -> bool:
                return (
                    path == "/"
                    or path == "/dashboard.css"
                    or path == "/dashboard-kernel-bridge.js"
                    or path == "/dashboard-kernel.wasm"
                    or path == "/dashboard.js"
                    or path == "/api/v1/state"
                    or path == "/api/v1/durable"
                    or path == "/api/v1/stream"
                    or path == "/api/v1/interventions/retry-now"
                    or path == "/api/v1/tools/run"
                    or path.startswith("/api/v1/")
                )

            def _write_not_found(self, path: str) -> None:
                self._write_json(
                    HTTPStatus.NOT_FOUND,
                    {
                        "error": {
                            "code": "not_found",
                            "message": f"Unknown route: {path}",
                        }
                    },
                )
                _incr_security_counter("not_found")

            def _write_method_not_allowed(self, method: str, path: str) -> None:
                _incr_security_counter("method_not_allowed")
                self._write_json(
                    HTTPStatus.METHOD_NOT_ALLOWED,
                    {
                        "error": {
                            "code": "method_not_allowed",
                            "message": f"Method {method} not allowed for {path}",
                        }
                    },
                )

            def _handle_unsupported_method(self, method: str) -> None:
                parsed = urlsplit(self.path)
                path = parsed.path
                if self._is_known_dashboard_route(path):
                    self._write_method_not_allowed(method, path)
                    return
                self._write_not_found(path)

            def do_GET(self) -> None:  # noqa: N802
                parsed = urlsplit(self.path)
                path = parsed.path

                static_asset = fetch_dashboard_asset(path)
                if static_asset is not None:
                    if self.headers.get("If-None-Match") == static_asset.etag:
                        self._write_not_modified(
                            cache_control="public, max-age=300, immutable",
                            etag=static_asset.etag,
                            include_frame_options=False,
                        )
                        return
                    self._write_static_asset(
                        content_type=static_asset.content_type,
                        body=static_asset.body,
                        cache_control="public, max-age=300, immutable",
                        etag=static_asset.etag,
                    )
                    return
                if path == "/dashboard-kernel.wasm":
                    wasm_asset = fetch_dashboard_kernel_wasm_asset()
                    if wasm_asset is None:
                        self._write_json(
                            HTTPStatus.NOT_FOUND,
                            {
                                "error": {
                                    "code": "wasm_kernel_unavailable",
                                    "message": (
                                        "Dashboard WASM kernel not found; set "
                                        "MOLT_SYMPHONY_DASHBOARD_KERNEL_WASM_PATH "
                                        "or build tools/symphony_dashboard_wasm.py output."
                                    ),
                                }
                            },
                        )
                        return
                    if self.headers.get("If-None-Match") == wasm_asset.etag:
                        self._write_not_modified(
                            cache_control="public, max-age=60",
                            etag=wasm_asset.etag,
                            include_frame_options=False,
                        )
                        return
                    self._write_static_asset(
                        content_type=wasm_asset.content_type,
                        body=wasm_asset.body,
                        cache_control="public, max-age=60",
                        etag=wasm_asset.etag,
                    )
                    return

                if path == "/":
                    if security.disable_dashboard:
                        _incr_security_counter("not_found")
                        self._write_json(
                            HTTPStatus.NOT_FOUND,
                            {
                                "error": {
                                    "code": "dashboard_disabled",
                                    "message": (
                                        "Dashboard UI is disabled in the active "
                                        "security profile."
                                    ),
                                }
                            },
                        )
                        return
                    if not self._authorize_request(
                        method="GET",
                        query=parsed.query,
                        mutating=False,
                        allow_query_token=security.allow_query_token,
                    ):
                        return
                    self._handle_dashboard()
                    return
                if path == "/api/v1/state":
                    if not self._authorize_request(
                        method="GET",
                        query=parsed.query,
                        mutating=False,
                        allow_query_token=security.allow_query_token,
                    ):
                        return
                    _, encoded, etag = _snapshot_state_cached()
                    if self.headers.get("If-None-Match") == etag:
                        self._write_not_modified(
                            cache_control="no-store",
                            etag=etag,
                            include_frame_options=True,
                        )
                        return
                    self._write_json_bytes(
                        HTTPStatus.OK,
                        encoded,
                        headers={"Cache-Control": "no-store", "ETag": etag},
                    )
                    return
                if path == "/api/v1/durable":
                    if not self._authorize_request(
                        method="GET",
                        query=parsed.query,
                        mutating=False,
                        allow_query_token=security.allow_query_token,
                    ):
                        return
                    limit = _coerce_limit_param(parsed.query, default=120)
                    self._write_json(
                        HTTPStatus.OK, provider.snapshot_durable_memory(limit=limit)
                    )
                    return
                if path == "/api/v1/stream":
                    if not self._authorize_request(
                        method="GET",
                        query=parsed.query,
                        mutating=False,
                        allow_query_token=security.allow_query_token,
                    ):
                        return
                    self._handle_state_stream(parsed.query)
                    return
                if path.startswith("/api/v1/"):
                    if not self._authorize_request(
                        method="GET",
                        query=parsed.query,
                        mutating=False,
                        allow_query_token=security.allow_query_token,
                    ):
                        return
                    issue_identifier = unquote(path.removeprefix("/api/v1/"))
                    payload = provider.snapshot_issue(issue_identifier)
                    if payload is None:
                        self._write_json(
                            HTTPStatus.NOT_FOUND,
                            {
                                "error": {
                                    "code": "issue_not_found",
                                    "message": (
                                        f"Unknown issue identifier: {issue_identifier}"
                                    ),
                                }
                            },
                        )
                        return
                    self._write_json(HTTPStatus.OK, payload)
                    return
                self._write_not_found(path)

            def do_POST(self) -> None:  # noqa: N802
                parsed = urlsplit(self.path)
                path = parsed.path

                if path == "/api/v1/refresh":
                    if not self._authorize_request(
                        method="POST",
                        query=parsed.query,
                        mutating=True,
                        allow_query_token=False,
                    ):
                        return
                    queued = provider.request_refresh()
                    _invalidate_state_cache()
                    self._write_json(
                        HTTPStatus.ACCEPTED,
                        {
                            "queued": queued,
                            "coalesced": not queued,
                            "requested_at": datetime.now(UTC)
                            .isoformat()
                            .replace("+00:00", "Z"),
                            "operations": ["poll", "reconcile"],
                        },
                    )
                    return
                if path == "/api/v1/interventions/retry-now":
                    if not self._authorize_request(
                        method="POST",
                        query=parsed.query,
                        mutating=True,
                        allow_query_token=False,
                    ):
                        return
                    payload = self._read_json_payload()
                    if payload is None:
                        return
                    issue_identifier = str(payload.get("issue_identifier", "")).strip()
                    result = provider.request_retry_now(issue_identifier)
                    status = (
                        HTTPStatus.ACCEPTED
                        if bool(result.get("ok"))
                        else HTTPStatus.NOT_FOUND
                    )
                    self._write_json(int(status), result)
                    return
                if path == "/api/v1/tools/run":
                    if not self._authorize_request(
                        method="POST",
                        query=parsed.query,
                        mutating=True,
                        allow_query_token=False,
                    ):
                        return
                    payload = self._read_json_payload()
                    if payload is None:
                        return
                    tool_name = str(payload.get("tool") or "").strip()
                    result = provider.run_dashboard_tool(tool_name, payload)
                    status = (
                        HTTPStatus.ACCEPTED
                        if bool(result.get("ok"))
                        else HTTPStatus.BAD_REQUEST
                    )
                    self._write_json(int(status), result)
                    return

                if self._is_known_dashboard_route(path):
                    self._write_method_not_allowed("POST", path)
                    return

                self._write_not_found(path)

            def do_PUT(self) -> None:  # noqa: N802
                self._handle_unsupported_method("PUT")

            def do_PATCH(self) -> None:  # noqa: N802
                self._handle_unsupported_method("PATCH")

            def do_DELETE(self) -> None:  # noqa: N802
                self._handle_unsupported_method("DELETE")

            def do_OPTIONS(self) -> None:  # noqa: N802
                self._handle_unsupported_method("OPTIONS")

            def _read_json_payload(self) -> dict[str, Any] | None:
                content_length_raw = self.headers.get("Content-Length") or "0"
                try:
                    content_length = int(content_length_raw)
                except ValueError:
                    content_length = 0
                if content_length <= 0:
                    return {}
                if content_length > 262_144:
                    self._drain_request_body(content_length)
                    self._deny_json(
                        HTTPStatus.REQUEST_ENTITY_TOO_LARGE,
                        "payload_too_large",
                        "Request body exceeds 262144 bytes.",
                        headers={"Connection": "close"},
                    )
                    self.close_connection = True
                    return None
                body = self.rfile.read(content_length)
                if not body:
                    return {}
                content_type = str(self.headers.get("Content-Type") or "").lower()
                try:
                    decoded_text = body.decode("utf-8")
                except UnicodeDecodeError:
                    decoded_text = ""
                if "application/x-www-form-urlencoded" in content_type and decoded_text:
                    form_payload = parse_qs(decoded_text, keep_blank_values=True)
                    normalized: dict[str, Any] = {}
                    for key, values in form_payload.items():
                        if not values:
                            normalized[key] = ""
                        elif len(values) == 1:
                            normalized[key] = values[0]
                        else:
                            normalized[key] = values
                    return normalized
                try:
                    decoded = json.loads(decoded_text)
                except (UnicodeDecodeError, json.JSONDecodeError):
                    return {}
                if isinstance(decoded, dict):
                    return decoded
                return {}

            def _drain_request_body(self, content_length: int) -> None:
                remaining = max(int(content_length), 0)
                while remaining > 0:
                    chunk = self.rfile.read(min(remaining, 65_536))
                    if not chunk:
                        return
                    remaining -= len(chunk)

            def _handle_dashboard(self) -> None:
                self._write_html(HTTPStatus.OK, DASHBOARD_HTML)

            def _handle_state_stream(self, query: str) -> None:
                nonlocal active_stream_clients
                with stream_clients_lock:
                    if active_stream_clients >= max_stream_clients:
                        _incr_security_counter("stream_capacity_rejected")
                        self._write_json(
                            int(HTTPStatus.TOO_MANY_REQUESTS),
                            {
                                "error": {
                                    "code": "stream_capacity_exhausted",
                                    "message": (
                                        "Maximum concurrent stream clients reached."
                                    ),
                                }
                            },
                        )
                        return
                    active_stream_clients += 1
                interval_ms = _coerce_stream_interval_ms(query)
                self.send_response(HTTPStatus.OK)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-store, private, no-transform")
                self.send_header("Pragma", "no-cache")
                self.send_header("X-Accel-Buffering", "no")
                self.send_header("Connection", "keep-alive")
                self.send_header("X-Content-Type-Options", "nosniff")
                self.send_header("X-Frame-Options", "DENY")
                self.send_header("Referrer-Policy", "no-referrer")
                self.send_header("Cross-Origin-Opener-Policy", "same-origin")
                self.send_header("Cross-Origin-Resource-Policy", "same-origin")
                self.end_headers()
                try:
                    self.wfile.write(b": stream-open\n\n")
                    self.wfile.flush()
                    next_heartbeat = time.monotonic() + 15.0
                    started_monotonic = time.monotonic()
                    previous_etag = ""
                    while True:
                        if (
                            time.monotonic() - started_monotonic
                        ) >= stream_max_age_seconds:
                            self.wfile.write(
                                b'event: stream_end\ndata: {"reason":"max_age"}\n\n'
                            )
                            self.wfile.flush()
                            return
                        _, encoded, etag = _snapshot_state_cached()
                        if etag != previous_etag:
                            event = (
                                f"id: {etag}\nevent: state\ndata: "
                                + encoded.decode("utf-8")
                                + "\n\n"
                            ).encode("utf-8")
                            self.wfile.write(event)
                            self.wfile.flush()
                            previous_etag = etag
                        deadline = time.monotonic() + (interval_ms / 1000.0)
                        while True:
                            remaining = deadline - time.monotonic()
                            if remaining <= 0:
                                break
                            sleep_window = min(remaining, 1.0)
                            time.sleep(sleep_window)
                            if time.monotonic() >= next_heartbeat:
                                self.wfile.write(b": heartbeat\n\n")
                                self.wfile.flush()
                                next_heartbeat = time.monotonic() + 15.0
                except (BrokenPipeError, ConnectionResetError, TimeoutError):
                    return
                finally:
                    with stream_clients_lock:
                        active_stream_clients = max(active_stream_clients - 1, 0)

        server = _QuietThreadingHTTPServer(
            (bind_host, self.port),
            Handler,
            max_active_requests=_coerce_non_negative_int_env(
                "MOLT_SYMPHONY_MAX_HTTP_CONNECTIONS",
                default=96,
                minimum=8,
            ),
            on_overload=lambda: _incr_security_counter("overload_rejected"),
        )
        self._server = server
        self._thread = Thread(
            target=server.serve_forever, name="symphony-http", daemon=True
        )
        self._thread.start()
        return int(server.server_port)

    def stop(self) -> None:
        if self._state_hasher is not None:
            self._state_hasher.close()
            self._state_hasher = None
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
        if self._thread is not None:
            self._thread.join(timeout=2.0)


def _coerce_stream_interval_ms(query: str) -> int:
    values = parse_qs(query).get("interval_ms", [])
    if not values:
        return 1000
    raw = values[0]
    try:
        parsed = int(raw)
    except (TypeError, ValueError):
        return 1000
    return max(250, min(parsed, 10000))


def _encode_json_bytes(payload: dict[str, Any]) -> bytes:
    return json.dumps(payload, ensure_ascii=True, separators=(",", ":")).encode("utf-8")


def _state_etag_for_payload(
    payload_bytes: bytes, *, hasher: _ExternalStateHasher | None = None
) -> str:
    if hasher is not None:
        external = hasher.hash(payload_bytes)
        if external is not None:
            return external
    digest = hashlib.blake2s(payload_bytes, digest_size=8).hexdigest()
    return f'W/"{digest}"'


def _normalize_state_payload_for_etag(payload: dict[str, Any]) -> dict[str, Any]:
    normalized = dict(payload)
    normalized.pop("generated_at", None)
    suspension_raw = normalized.get("suspension")
    if isinstance(suspension_raw, dict):
        suspension = dict(suspension_raw)
        suspension.pop("due_in_seconds", None)
        normalized["suspension"] = suspension
    return normalized


def _state_hasher_from_env() -> _ExternalStateHasher | None:
    raw = str(os.environ.get("MOLT_SYMPHONY_STATE_HASH_HELPER", "")).strip()
    if not raw:
        return None
    try:
        command = shlex.split(raw)
    except ValueError:
        return None
    if not command:
        return None
    has_mode = "--stdio" in command or "--stdio-frame" in command
    prefer_frame = _coerce_bool_env(
        "MOLT_SYMPHONY_STATE_HASH_HELPER_PREFER_FRAME", default=True
    )
    frame_mode = "--stdio-frame" in command
    fallback_command: list[str] | None = None
    if not has_mode:
        if prefer_frame:
            fallback_command = [*command, "--stdio"]
            command = [*command, "--stdio-frame"]
            frame_mode = True
        else:
            command = [*command, "--stdio"]
            frame_mode = False
    timeout_ms_raw = str(
        os.environ.get("MOLT_SYMPHONY_STATE_HASH_HELPER_TIMEOUT_MS", "150")
    ).strip()
    try:
        timeout_ms = int(timeout_ms_raw)
    except ValueError:
        timeout_ms = 150
    timeout_seconds = max(timeout_ms, 10) / 1000.0
    return _ExternalStateHasher(
        command=command,
        timeout_seconds=timeout_seconds,
        frame_mode=frame_mode,
        fallback_command=fallback_command,
    )


def _coerce_limit_param(query: str, *, default: int) -> int:
    values = parse_qs(query).get("limit", [])
    if not values:
        return default
    raw = values[0]
    try:
        parsed = int(raw)
    except (TypeError, ValueError):
        return default
    return max(10, min(parsed, 1000))


@dataclass(frozen=True, slots=True)
class _DashboardSecurityConfig:
    api_token: str | None
    enforce_origin: bool
    require_csrf_header: bool
    allowed_origins: frozenset[str]
    allow_localhost_any_port: bool
    profile: str
    allow_query_token: bool
    disable_dashboard: bool
    startup_error: str | None


def _dashboard_security_from_env(*, port: int) -> _DashboardSecurityConfig:
    profile_raw = str(os.environ.get("MOLT_SYMPHONY_SECURITY_PROFILE") or "").strip()
    profile = profile_raw.lower() if profile_raw else "local"
    if profile not in {"local", "production"}:
        profile = "local"
    api_token = str(
        os.environ.get("MOLT_SYMPHONY_API_TOKEN")
        or os.environ.get("MOLT_SYMPHONY_DASHBOARD_TOKEN")
        or ""
    ).strip()
    default_enable_strict = profile == "production"
    enforce_origin = _coerce_bool_env("MOLT_SYMPHONY_ENFORCE_ORIGIN", default=True)
    require_csrf = _coerce_bool_env("MOLT_SYMPHONY_REQUIRE_CSRF_HEADER", default=True)
    allow_query_token = _coerce_bool_env(
        "MOLT_SYMPHONY_ALLOW_QUERY_TOKEN",
        default=not default_enable_strict,
    )
    disable_dashboard = _coerce_bool_env(
        "MOLT_SYMPHONY_DISABLE_DASHBOARD_UI",
        default=default_enable_strict,
    )
    allowed_raw = str(os.environ.get("MOLT_SYMPHONY_ALLOWED_ORIGINS") or "").strip()
    allow_localhost_any_port = False
    if allowed_raw:
        allowed = frozenset(
            origin.strip().lower()
            for origin in allowed_raw.split(",")
            if origin.strip()
        )
    else:
        allow_localhost_any_port = port <= 0
        allowed = frozenset(
            {
                f"http://127.0.0.1:{port}".lower(),
                f"http://localhost:{port}".lower(),
            }
        )
    startup_error: str | None = None
    if profile == "production" and not api_token:
        startup_error = (
            "MOLT_SYMPHONY_SECURITY_PROFILE=production requires "
            "MOLT_SYMPHONY_API_TOKEN or MOLT_SYMPHONY_DASHBOARD_TOKEN."
        )
    return _DashboardSecurityConfig(
        api_token=api_token or None,
        enforce_origin=enforce_origin,
        require_csrf_header=require_csrf,
        allowed_origins=allowed,
        allow_localhost_any_port=allow_localhost_any_port,
        profile=profile,
        allow_query_token=allow_query_token,
        disable_dashboard=disable_dashboard,
        startup_error=startup_error,
    )


def _bind_host_from_env() -> tuple[str, bool]:
    host = str(os.environ.get("MOLT_SYMPHONY_BIND_HOST") or "").strip() or "127.0.0.1"
    allow_nonlocal = _coerce_bool_env(
        "MOLT_SYMPHONY_ALLOW_NONLOCAL_BIND", default=False
    )
    is_loopback = _is_loopback_host(host)
    if not is_loopback and not allow_nonlocal:
        raise RuntimeError(
            "Refusing non-loopback bind host without explicit opt-in. "
            "Set MOLT_SYMPHONY_ALLOW_NONLOCAL_BIND=1 to allow "
            f"MOLT_SYMPHONY_BIND_HOST={host!r}."
        )
    return host, (not is_loopback)


def _is_loopback_host(host: str) -> bool:
    normalized = host.strip().lower()
    if normalized in {"localhost"}:
        return True
    try:
        return bool(ipaddress.ip_address(normalized).is_loopback)
    except ValueError:
        return False


def _coerce_bool_env(name: str, *, default: bool) -> bool:
    raw = str(os.environ.get(name, "")).strip().lower()
    if not raw:
        return default
    if raw in {"1", "true", "yes", "on"}:
        return True
    if raw in {"0", "false", "no", "off"}:
        return False
    return default


def _coerce_non_negative_int_env(name: str, *, default: int, minimum: int) -> int:
    raw = str(os.environ.get(name, "")).strip()
    if not raw:
        return max(default, minimum)
    try:
        parsed = int(raw)
    except ValueError:
        parsed = default
    return max(parsed, minimum)


def _coerce_non_negative_float_env(
    name: str, *, default: float, minimum: float
) -> float:
    raw = str(os.environ.get(name, "")).strip()
    if not raw:
        return max(default, minimum)
    try:
        parsed = float(raw)
    except ValueError:
        parsed = default
    return max(parsed, minimum)


def _reject_connection_overloaded(request: Any) -> None:
    try:
        request.sendall(
            (
                b"HTTP/1.1 503 Service Unavailable\r\n"
                b"Connection: close\r\n"
                b"Content-Type: text/plain; charset=utf-8\r\n"
                b"Content-Length: 32\r\n\r\n"
                b"Service overloaded. Retry later."
            )
        )
    except OSError:
        pass
    try:
        request.shutdown(2)
    except OSError:
        pass
    try:
        request.close()
    except OSError:
        pass
