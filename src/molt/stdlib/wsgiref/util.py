"""Minimal `wsgiref.util` subset for Molt."""

from __future__ import annotations

import io
import sys
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_WSGIREF_RUNTIME_READY = _require_intrinsic(
    "molt_wsgiref_runtime_ready", globals()
)


def setup_testing_defaults(environ: dict[str, Any]) -> None:
    environ.setdefault("SERVER_NAME", "127.0.0.1")
    environ.setdefault("SERVER_PROTOCOL", "HTTP/1.0")
    environ.setdefault("HTTP_HOST", environ["SERVER_NAME"])
    environ.setdefault("REQUEST_METHOD", "GET")
    environ.setdefault("SCRIPT_NAME", "")
    environ.setdefault("PATH_INFO", "/")
    environ.setdefault("QUERY_STRING", "")
    environ.setdefault("CONTENT_TYPE", "text/plain")
    environ.setdefault("CONTENT_LENGTH", "0")
    environ.setdefault("SERVER_PORT", "80")
    environ.setdefault("wsgi.version", (1, 0))
    environ.setdefault("wsgi.url_scheme", "http")
    environ.setdefault("wsgi.input", io.BytesIO(b""))
    environ.setdefault("wsgi.errors", sys.stderr)
    environ.setdefault("wsgi.multithread", False)
    environ.setdefault("wsgi.multiprocess", False)
    environ.setdefault("wsgi.run_once", False)


__all__ = ["setup_testing_defaults"]
