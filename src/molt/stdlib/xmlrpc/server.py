"""Minimal `xmlrpc.server` subset for Molt."""

from __future__ import annotations

import re
from typing import Any, Callable

from wsgiref.simple_server import make_server

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_XMLRPC_RUNTIME_READY = _require_intrinsic("molt_xmlrpc_runtime_ready", globals())

# TODO(stdlib, owner:runtime, milestone:TL3, priority:P2, status:planned):
# Extend XML-RPC coverage to support full marshalling/fault handling and
# introspection APIs with Rust-backed parsing/serialization.


def _decode_value(payload: str) -> Any:
    int_match = re.search(r"<(?:int|i4)>(-?\d+)</(?:int|i4)>", payload)
    if int_match is not None:
        return int(int_match.group(1))
    bool_match = re.search(r"<boolean>([01])</boolean>", payload)
    if bool_match is not None:
        return bool(int(bool_match.group(1)))
    string_match = re.search(r"<string>(.*?)</string>", payload, re.DOTALL)
    if string_match is not None:
        return string_match.group(1)
    raise ValueError("unsupported XML-RPC request value")


def _encode_value(value: Any) -> str:
    if isinstance(value, bool):
        return f"<boolean>{1 if value else 0}</boolean>"
    if isinstance(value, int):
        return f"<int>{value}</int>"
    return f"<string>{value}</string>"


class SimpleXMLRPCServer:
    def __init__(
        self,
        addr: tuple[str, int],
        requestHandler: Any | None = None,
        logRequests: bool = True,
        allow_none: bool = False,
        encoding: str | None = None,
        bind_and_activate: bool = True,
    ) -> None:
        del requestHandler, logRequests, allow_none, encoding, bind_and_activate
        self._functions: dict[str, Callable[..., Any]] = {}
        self._httpd = make_server(addr[0], int(addr[1]), self._wsgi_app)
        self.server_address = self._httpd.server_address

    def register_function(
        self, function: Callable[..., Any], name: str | None = None
    ) -> str:
        method_name = str(name) if name is not None else str(function.__name__)
        self._functions[method_name] = function
        return method_name

    def _dispatch(self, method_name: str, params: list[Any]) -> Any:
        func = self._functions.get(method_name)
        if func is None:
            raise ValueError(f"unknown method: {method_name}")
        return func(*params)

    def _wsgi_app(self, environ: dict[str, Any], start_response: Callable[..., Any]):
        body_stream = environ.get("wsgi.input")
        payload = body_stream.read() if body_stream is not None else b""
        xml = payload.decode("utf-8", errors="replace")
        method_match = re.search(r"<methodName>(.*?)</methodName>", xml, re.DOTALL)
        if method_match is None:
            response_body = b""
            start_response("400 Bad Request", [("Content-Length", "0")])
            return [response_body]
        method_name = method_match.group(1).strip()
        params: list[Any] = []
        for value_xml in re.findall(
            r"<param>\s*<value>(.*?)</value>\s*</param>", xml, re.DOTALL
        ):
            params.append(_decode_value(value_xml))
        result = self._dispatch(method_name, params)
        response_xml = (
            "<?xml version='1.0'?>"
            "<methodResponse><params><param><value>"
            f"{_encode_value(result)}"
            "</value></param></params></methodResponse>"
        )
        response_body = response_xml.encode("utf-8")
        start_response(
            "200 OK",
            [("Content-Type", "text/xml"), ("Content-Length", str(len(response_body)))],
        )
        return [response_body]

    def serve_forever(self) -> None:
        self._httpd.serve_forever()

    def shutdown(self) -> None:
        self._httpd.shutdown()

    def server_close(self) -> None:
        self._httpd.server_close()


__all__ = ["SimpleXMLRPCServer"]
