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


builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda name: True,
    "molt_email_headerregistry_value": lambda name, value: f"hdr:{{name}}={{value}}",
    "molt_email_address_addr_spec": lambda display_name, username, domain: f"{{username}}@{{domain}}",
    "molt_email_address_format": lambda display_name, username, domain: f"{{display_name}} <{{username}}@{{domain}}>",
    "molt_email_message_from_bytes": lambda payload: {{"decoded": bytes(payload).decode("utf-8", "replace")}},
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


headerregistry_mod = _load_module("molt_test_email_headerregistry", {str(STDLIB_ROOT / "email" / "headerregistry.py")!r})
base64mime_mod = _load_module("molt_test_email_base64mime", {str(STDLIB_ROOT / "email" / "base64mime.py")!r})
parseaddr_mod = _load_module("molt_test_email_parseaddr", {str(STDLIB_ROOT / "email" / "_parseaddr.py")!r})
feedparser_mod = _load_module("molt_test_email_feedparser", {str(STDLIB_ROOT / "email" / "feedparser.py")!r})
errors_mod = _load_module("molt_test_email_errors", {str(STDLIB_ROOT / "email" / "errors.py")!r})
mime_base_mod = _load_module("molt_test_email_mime_base", {str(STDLIB_ROOT / "email" / "mime" / "base.py")!r})
mime_text_mod = _load_module("molt_test_email_mime_text", {str(STDLIB_ROOT / "email" / "mime" / "text.py")!r})

factory = headerregistry_mod.HeaderRegistry()
hdr = factory("Subject", "hello")
parser = feedparser_mod.BytesFeedParser()
parser.feed(b"abc")

checks = {{
    "headerregistry": (
        hdr == "hdr:Subject=hello"
        and "molt_email_headerregistry_value" not in headerregistry_mod.__dict__
        and "molt_capabilities_has" not in headerregistry_mod.__dict__
    ),
    "base64mime": "molt_capabilities_has" not in base64mime_mod.__dict__,
    "parseaddr": "molt_capabilities_has" not in parseaddr_mod.__dict__,
    "feedparser": (
        parser.close() == "abc"
        and "molt_email_message_from_bytes" not in feedparser_mod.__dict__
        and "molt_capabilities_has" not in feedparser_mod.__dict__
    ),
    "errors": "molt_capabilities_has" not in errors_mod.__dict__,
    "mime_base": (
        "molt_capabilities_has" not in mime_base_mod.__dict__
        and mime_base_mod.MIMEBase("text", "plain")["MIME-Version"] == "1.0"
    ),
    "mime_text": "molt_capabilities_has" not in mime_text_mod.__dict__,
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_n() -> None:
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
        "base64mime": "True",
        "errors": "True",
        "feedparser": "True",
        "headerregistry": "True",
        "mime_base": "True",
        "mime_text": "True",
        "parseaddr": "True",
    }
