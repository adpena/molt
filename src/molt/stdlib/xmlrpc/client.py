"""Minimal `xmlrpc.client` subset for Molt."""

from __future__ import annotations

import re
from typing import Any

import urllib.request

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_XMLRPC_RUNTIME_READY = _require_intrinsic("molt_xmlrpc_runtime_ready", globals())


def _encode_value(value: Any) -> str:
    if isinstance(value, bool):
        return f"<boolean>{1 if value else 0}</boolean>"
    if isinstance(value, int):
        return f"<int>{value}</int>"
    return f"<string>{value}</string>"


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
    raise ValueError("unsupported XML-RPC response payload")


class _MethodProxy:
    def __init__(self, server: "ServerProxy", name: str) -> None:
        self._server = server
        self._name = name

    def __call__(self, *params: Any) -> Any:
        return self._server._call(self._name, params)


class ServerProxy:
    def __init__(self, uri: str) -> None:
        self._uri = str(uri)

    def __getattr__(self, name: str) -> _MethodProxy:
        return _MethodProxy(self, str(name))

    def _call(self, method_name: str, params: tuple[Any, ...]) -> Any:
        params_xml = "".join(
            f"<param><value>{_encode_value(value)}</value></param>" for value in params
        )
        request_xml = (
            "<?xml version='1.0'?>"
            f"<methodCall><methodName>{method_name}</methodName>"
            f"<params>{params_xml}</params></methodCall>"
        )
        req = urllib.request.Request(
            self._uri,
            data=request_xml.encode("utf-8"),
            headers={"Content-Type": "text/xml"},
            method="POST",
        )
        with urllib.request.urlopen(req) as response:
            payload = response.read().decode("utf-8", errors="replace")
        return _decode_value(payload)


__all__ = ["ServerProxy"]
