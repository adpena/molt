from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import enum
import importlib.util
import sys
import types


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


builtins._molt_intrinsics = {{
    "molt_stdlib_probe": lambda: True,
    "molt_http_parse_header_pairs": lambda data: [("Host", "example.test")],
    "molt_http_server_read_request": lambda handler: True,
    "molt_http_server_compute_close_connection": lambda *args, **kwargs: False,
    "molt_http_server_handle_one_request": lambda handler: False,
    "molt_http_server_send_response": lambda handler, code, message=None: None,
    "molt_http_server_send_response_only": lambda handler, code, message=None: None,
    "molt_http_server_send_header": lambda handler, keyword, value: None,
    "molt_http_server_end_headers": lambda handler: None,
    "molt_http_server_send_error": lambda handler, code, message=None: None,
    "molt_http_server_version_string": lambda server_version, sys_version: f"{{server_version}} {{sys_version}}".strip(),
    "molt_http_server_date_time_string": lambda timestamp=None: "Thu, 01 Jan 1970 00:00:00 GMT",
    "molt_gc_collect": lambda generation=2: generation,
    "molt_gc_enable": lambda: None,
    "molt_gc_disable": lambda: None,
    "molt_gc_isenabled": lambda: True,
    "molt_gc_set_threshold": lambda a, b, c: None,
    "molt_gc_get_threshold": lambda: (700, 10, 10),
    "molt_gc_set_debug": lambda flags: None,
    "molt_gc_get_debug": lambda: 0,
    "molt_gc_get_count": lambda: (1, 2, 3),
    "molt_repr_from_obj": lambda value: repr(value),
    "molt_pprint_safe_repr": lambda obj, context, maxlevels: repr(obj),
    "molt_pprint_pformat": lambda obj, indent, width, depth, compact, sort_dicts, underscore_numbers: repr(obj),
    "molt_pprint_isreadable": lambda obj: True,
    "molt_pprint_isrecursive": lambda obj: False,
    "molt_pprint_format_object": lambda *args, **kwargs: repr(args[0]) if args else "",
}}

_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    intrinsics = getattr(builtins, "_molt_intrinsics", {{}})
    if name in intrinsics:
        value = intrinsics[name]
        if namespace is not None:
            namespace[name] = value
        return value
    raise RuntimeError(f"intrinsic unavailable: {{name}}")


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod

http_pkg = types.ModuleType("http")


class HTTPStatus(enum.IntEnum):
    NOT_IMPLEMENTED = 501


http_pkg.HTTPStatus = HTTPStatus
sys.modules["http"] = http_pkg

server_mod = _load_module("molt_test_http_server", {str(STDLIB_ROOT / "http" / "server.py")!r})
gc_mod = _load_module("molt_test_gc", {str(STDLIB_ROOT / "gc.py")!r})
pprint_mod = _load_module("molt_test_pprint", {str(STDLIB_ROOT / "pprint.py")!r})
handler = types.SimpleNamespace(server_version="BaseHTTP/0.6", sys_version="")

checks = {{
    "http_server": (
        server_mod.parse_headers(b"Host: example.test\\r\\n\\r\\n").get("host") == "example.test"
        and server_mod.BaseHTTPRequestHandler.version_string(handler) == "BaseHTTP/0.6"
        and "molt_stdlib_probe" not in server_mod.__dict__
        and "molt_http_server_handle_one_request" not in server_mod.__dict__
    ),
    "gc": (
        gc_mod.collect(1) == 1
        and gc_mod.get_threshold() == (700, 10, 10)
        and gc_mod.get_count() == (1, 2, 3)
        and "molt_stdlib_probe" not in gc_mod.__dict__
        and "molt_gc_collect" not in gc_mod.__dict__
    ),
    "pprint": (
        pprint_mod.pformat({{"a": 1}}) == "{{'a': 1}}"
        and pprint_mod.isreadable(object()) is True
        and "molt_repr_from_obj" not in pprint_mod.__dict__
        and "molt_pprint_pformat" not in pprint_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_v() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        if line.startswith("CHECK|"):
            _, key, value = line.split("|", 2)
            checks[key] = value
    assert checks == {
        "gc": "True",
        "http_server": "True",
        "pprint": "True",
    }
