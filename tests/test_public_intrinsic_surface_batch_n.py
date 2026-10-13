from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import sys
from email import feedparser as reference_feedparser
from email.message import Message
from email._policybase import compat32
from tests.stdlib_intrinsic_registry import install_registry


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


install_registry({{
    "molt_email_headerregistry_value": lambda name, value: f"hdr:{{name}}={{value}}",
    "molt_email_address_addr_spec": lambda display_name, username, domain: f"{{username}}@{{domain}}",
    "molt_email_address_format": lambda display_name, username, domain: f"{{display_name}} <{{username}}@{{domain}}>",
}})


headerregistry_mod = _load_module("molt_test_email_headerregistry", {str(STDLIB_ROOT / "email" / "headerregistry.py")!r})
base64mime_mod = _load_module("molt_test_email_base64mime", {str(STDLIB_ROOT / "email" / "base64mime.py")!r})
parseaddr_mod = _load_module("molt_test_email_parseaddr", {str(STDLIB_ROOT / "email" / "_parseaddr.py")!r})
feedparser_mod = _load_module("molt_test_email_feedparser", {str(STDLIB_ROOT / "email" / "feedparser.py")!r})
errors_mod = _load_module("molt_test_email_errors", {str(STDLIB_ROOT / "email" / "errors.py")!r})
mime_base_mod = _load_module("molt_test_email_mime_base", {str(STDLIB_ROOT / "email" / "mime" / "base.py")!r})
mime_text_mod = _load_module("molt_test_email_mime_text", {str(STDLIB_ROOT / "email" / "mime" / "text.py")!r})

factory = headerregistry_mod.HeaderRegistry()
hdr = factory("Subject", "hello")
def _feedparser_results(module):
    records = []
    fixtures = (
        b"Subject: chunked\\r\\n\\r\\nbody\\xff",
        b"Content-Type: multipart/mixed; boundary=x\\r\\n\\r\\n"
        b"preamble\\r\\n--x\\r\\nSubject: part\\r\\n\\r\\nbody\\r\\n--x--\\r\\nepilogue",
    )
    for source in fixtures:
        for width in (1, 7, len(source)):
            parser = module.BytesFeedParser()
            for offset in range(0, len(source), width):
                parser.feed(source[offset:offset + width])
            message = parser.close()
            records.append((
                isinstance(message, Message), message.policy is compat32,
                [(part.items(), part.get_payload(decode=True),
                  [type(defect).__name__ for defect in part.defects])
                 for part in message.walk()],
                message.preamble, message.epilogue,
            ))

    class CustomMessage(Message):
        pass

    def legacy_factory():
        return CustomMessage()

    for factory in (CustomMessage, legacy_factory):
        parser = module.FeedParser(factory)
        parser.feed("Subject: factory\\n\\nbody")
        message = parser.close()
        records.append((isinstance(message, CustomMessage), message["Subject"], message.get_payload()))

    strict = compat32.clone(raise_on_defect=True)
    for source in (": bad\\n\\nbody", "Subject: ok\\nFrom misplaced\\nTo: you\\n\\nbody"):
        parser = module.FeedParser(policy=strict)
        try:
            parser.feed(source)
            message = parser.close()
        except Exception as exc:
            records.append(type(exc).__name__)
        else:
            records.append([type(defect).__name__ for defect in message.defects])
    return records

checks = {{
    "headerregistry": (
        hdr == "hdr:Subject=hello"
        and "molt_email_headerregistry_value" not in headerregistry_mod.__dict__
        and "molt_capabilities_has" not in headerregistry_mod.__dict__
    ),
    "base64mime": "molt_capabilities_has" not in base64mime_mod.__dict__,
    "parseaddr": "molt_capabilities_has" not in parseaddr_mod.__dict__,
    "feedparser": (
        _feedparser_results(feedparser_mod) == _feedparser_results(reference_feedparser)
        and feedparser_mod.NLCRE_eol.pattern == reference_feedparser.NLCRE_eol.pattern
        and hasattr(feedparser_mod, "boundaryendRE") == hasattr(reference_feedparser, "boundaryendRE")
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
    proc = run_surface_test_process(
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
