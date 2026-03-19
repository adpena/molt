from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
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


class _SelectNS:
    error = OSError


sys.modules["select"] = _SelectNS()


builtins._molt_intrinsics = {{
    "molt_platform_system": lambda: "MoltOS",
    "molt_platform_node": lambda: "node-1",
    "molt_platform_release": lambda: "1.0",
    "molt_platform_version": lambda: "v1",
    "molt_platform_machine": lambda: "x86_64",
    "molt_platform_processor": lambda: "moltcpu",
    "molt_platform_architecture": lambda: ("64bit", ""),
    "molt_platform_python_version": lambda: "3.12.9",
    "molt_platform_python_version_tuple": lambda: ("3", "12", "9"),
    "molt_platform_python_implementation": lambda: "Molt",
    "molt_platform_python_compiler": lambda: "rustc",
    "molt_platform_platform": lambda aliased, terse: "MoltOS-1.0",
    "molt_platform_uname": lambda: ("MoltOS", "node-1", "1.0", "v1", "x86_64", "moltcpu"),
    "molt_ipaddress_v4_new": lambda addr: ("v4", str(addr)),
    "molt_ipaddress_v4_str": lambda handle: handle[1],
    "molt_ipaddress_v4_int": lambda handle: 1234,
    "molt_ipaddress_v4_packed": lambda handle: b"\\x7f\\x00\\x00\\x01",
    "molt_ipaddress_v4_version": lambda handle: 4,
    "molt_ipaddress_v4_is_private": lambda handle: False,
    "molt_ipaddress_v4_is_loopback": lambda handle: True,
    "molt_ipaddress_v4_is_multicast": lambda handle: False,
    "molt_ipaddress_v4_is_link_local": lambda handle: False,
    "molt_ipaddress_v4_is_reserved": lambda handle: False,
    "molt_ipaddress_v4_is_global": lambda handle: False,
    "molt_ipaddress_v4_max_prefixlen": lambda handle: 32,
    "molt_ipaddress_drop": lambda handle: None,
    "molt_ipaddress_v6_new": lambda addr: ("v6", str(addr)),
    "molt_ipaddress_v6_str": lambda handle: handle[1],
    "molt_ipaddress_v6_int": lambda handle: 5678,
    "molt_ipaddress_v6_packed": lambda handle: b"0" * 16,
    "molt_ipaddress_v6_version": lambda handle: 6,
    "molt_ipaddress_v6_is_private": lambda handle: False,
    "molt_ipaddress_v6_is_loopback": lambda handle: False,
    "molt_ipaddress_v6_is_multicast": lambda handle: False,
    "molt_ipaddress_v6_is_link_local": lambda handle: False,
    "molt_ipaddress_v6_is_global": lambda handle: True,
    "molt_ipaddress_v6_drop": lambda handle: None,
    "molt_ipaddress_v4_network_new": lambda addr, strict: ("net4", str(addr), strict),
    "molt_ipaddress_v4_network_str": lambda handle: "192.168.0.0/24",
    "molt_ipaddress_v4_network_prefixlen": lambda handle: 24,
    "molt_ipaddress_v4_network_broadcast": lambda handle: ("v4", "192.168.0.255"),
    "molt_ipaddress_v4_network_hosts": lambda handle: [("v4", "192.168.0.1"), ("v4", "192.168.0.2")],
    "molt_ipaddress_v4_network_contains": lambda handle, addr: True,
    "molt_ipaddress_v4_network_drop": lambda handle: None,
    "molt_select_selector_new": lambda kind: {{"kind": kind}},
    "molt_select_selector_fileno": lambda handle: 9,
    "molt_select_selector_register": lambda handle, fileobj, events: None,
    "molt_select_selector_unregister": lambda handle, fd: None,
    "molt_select_selector_modify": lambda handle, fd, events: None,
    "molt_select_selector_poll": lambda handle, timeout: [(3, 1)],
    "molt_select_selector_close": lambda handle: None,
    "molt_select_selector_drop": lambda handle: None,
    "molt_select_fileno": lambda fileobj: int(fileobj),
    "molt_select_default_selector_kind": lambda: 0,
    "molt_select_backend_available": lambda kind: True,
    "molt_html_parser_new": lambda convert_charrefs: {{"convert": convert_charrefs}},
    "molt_html_parser_feed": lambda handle, data: [("starttag", "p", []), ("data", data), ("endtag", "p")],
    "molt_html_parser_close": lambda handle: [("comment", "done")],
    "molt_html_parser_drop": lambda handle: None,
    "molt_stdlib_probe": lambda: None,
    "molt_weakref_register": lambda ref, obj, callback: None,
    "molt_weakref_get": lambda ref: None,
    "molt_weakref_callback": lambda ref: None,
    "molt_weakref_peek": lambda ref: None,
    "molt_weakref_drop": lambda ref: None,
    "molt_weakref_collect": lambda: None,
    "molt_weakref_find_nocallback": lambda obj: None,
    "molt_weakref_refs": lambda obj: [],
    "molt_weakref_count": lambda obj: 0,
    "molt_weakref_finalize_track": lambda finalizer, obj, func, args, kwargs, atexit: None,
    "molt_weakref_finalize_untrack": lambda finalizer: None,
    "molt_weakkeydict_set": lambda *args: None,
    "molt_weakkeydict_get": lambda *args: None,
    "molt_weakkeydict_del": lambda *args: None,
    "molt_weakkeydict_contains": lambda *args: False,
    "molt_weakkeydict_len": lambda *args: 0,
    "molt_weakkeydict_items": lambda *args: [],
    "molt_weakkeydict_keyrefs": lambda *args: [],
    "molt_weakkeydict_popitem": lambda *args: None,
    "molt_weakkeydict_clear": lambda *args: None,
    "molt_weakvaluedict_set": lambda *args: None,
    "molt_weakvaluedict_get": lambda *args: None,
    "molt_weakvaluedict_del": lambda *args: None,
    "molt_weakvaluedict_contains": lambda *args: False,
    "molt_weakvaluedict_len": lambda *args: 0,
    "molt_weakvaluedict_items": lambda *args: [],
    "molt_weakvaluedict_valuerefs": lambda *args: [],
    "molt_weakvaluedict_popitem": lambda *args: None,
    "molt_weakvaluedict_clear": lambda *args: None,
    "molt_weakset_add": lambda *args: None,
    "molt_weakset_discard": lambda *args: None,
    "molt_weakset_remove": lambda *args: None,
    "molt_weakset_pop": lambda *args: None,
    "molt_weakset_contains": lambda *args: False,
    "molt_weakset_len": lambda *args: 0,
    "molt_weakset_items": lambda *args: [],
    "molt_weakset_clear": lambda *args: None,
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


platform_mod = _load_module("molt_test_platform", {str(STDLIB_ROOT / "platform.py")!r})
ipaddress_mod = _load_module("molt_test_ipaddress", {str(STDLIB_ROOT / "ipaddress.py")!r})
selectors_mod = _load_module("molt_test_selectors", {str(STDLIB_ROOT / "selectors.py")!r})
html_parser_mod = _load_module("molt_test_html_parser", {str(STDLIB_ROOT / "html" / "parser.py")!r})
class Recorder(html_parser_mod.HTMLParser):
    def __init__(self):
        super().__init__()
        self.events = []

    def handle_starttag(self, tag, attrs):
        self.events.append(("start", tag))

    def handle_data(self, data):
        self.events.append(("data", data))

    def handle_endtag(self, tag):
        self.events.append(("end", tag))

    def handle_comment(self, data):
        self.events.append(("comment", data))


parser = Recorder()
parser.feed("hello")
parser.close()
sel = selectors_mod.DefaultSelector()
sel.register(3, selectors_mod.EVENT_READ, "x")

checks = {{
    "platform": (
        platform_mod.system() == "MoltOS"
        and platform_mod.uname().processor == "moltcpu"
        and "molt_platform_system" not in platform_mod.__dict__
    ),
    "ipaddress": (
        str(ipaddress_mod.ip_address("127.0.0.1")) == "127.0.0.1"
        and ipaddress_mod.ip_network("192.168.0.0/24").prefixlen == 24
        and "molt_ipaddress_v4_new" not in ipaddress_mod.__dict__
    ),
    "selectors": (
        sel.select(0.0)[0][0].data == "x"
        and "molt_select_selector_new" not in selectors_mod.__dict__
    ),
    "html_parser": (
        parser.events == [("start", "p"), ("data", "hello"), ("end", "p"), ("comment", "done")]
        and "molt_html_parser_new" not in html_parser_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_i() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "CHECK":
            checks[rest[0]] = rest[1]
    assert checks == {
        "html_parser": "True",
        "ipaddress": "True",
        "platform": "True",
        "selectors": "True",
    }
