from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import email
import importlib.util
import io
import os
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
    "molt_email_utils_make_msgid": lambda domain=None: "<molt@example.test>",
    "molt_email_utils_getaddresses": lambda fieldvalues: [("Alice", "alice@example.test")],
    "molt_email_utils_parsedate_tz": lambda date: (2026, 3, 18, 12, 0, 0, 0, 78, -1, 0),
    "molt_email_utils_format_datetime": lambda dt: "Tue, 18 Mar 2026 12:00:00 +0000",
    "molt_email_utils_parsedate_to_datetime": lambda data: ("parsed", data),
    "molt_email_message_as_string": lambda handle: "Subject: test\\n\\nbody",
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

mime_pkg = types.ModuleType("email.mime")
mime_pkg.__path__ = [{str(STDLIB_ROOT / "email" / "mime")!r}]
sys.modules["email.mime"] = mime_pkg

support_pkg = types.ModuleType("test.support")
sys.modules["test.support"] = support_pkg
sys.modules["test.support.import_helper"] = types.ModuleType("test.support.import_helper")
sys.modules["test.support.os_helper"] = types.ModuleType("test.support.os_helper")
sys.modules["test.support.warnings_helper"] = types.ModuleType("test.support.warnings_helper")
sys.modules["test.list_tests"] = types.ModuleType("test.list_tests")
sys.modules["test.seq_tests"] = types.ModuleType("test.seq_tests")

mime_init_mod = _load_module("email.mime", {str(STDLIB_ROOT / "email" / "mime" / "__init__.py")!r})
mime_message_mod = _load_module("email.mime.message", {str(STDLIB_ROOT / "email" / "mime" / "message.py")!r})
mime_multipart_mod = _load_module("email.mime.multipart", {str(STDLIB_ROOT / "email" / "mime" / "multipart.py")!r})
iterators_mod = _load_module("molt_test_email_iterators", {str(STDLIB_ROOT / "email" / "iterators.py")!r})
charset_mod = _load_module("molt_test_email_charset", {str(STDLIB_ROOT / "email" / "charset.py")!r})
utils_mod = _load_module("molt_test_email_utils", {str(STDLIB_ROOT / "email" / "utils.py")!r})
generator_mod = _load_module("molt_test_email_generator", {str(STDLIB_ROOT / "email" / "generator.py")!r})
parser_mod = _load_module("molt_test_email_parser", {str(STDLIB_ROOT / "email" / "parser.py")!r})
encoders_mod = _load_module("molt_test_email_encoders", {str(STDLIB_ROOT / "email" / "encoders.py")!r})
test_mod = _load_module("test", {str(STDLIB_ROOT / "test" / "__init__.py")!r})


class _Part:
    def __init__(self, payload, maintype="text", subtype="plain"):
        self._payload = payload
        self._maintype = maintype
        self._subtype = subtype

    def walk(self):
        yield self

    def get_payload(self, decode=False):
        return self._payload

    def get_content_maintype(self):
        return self._maintype

    def get_content_subtype(self):
        return self._subtype


class _Msg:
    def __init__(self):
        self._handle = object()


buffer = io.StringIO()
generator_mod.Generator(buffer).flatten(_Msg())

os.environ["MOLT_REGRTEST_CPYTHON_DIR"] = ""

checks = {{
    "mime_init": "molt_capabilities_has" not in mime_init_mod.__dict__,
    "mime_message": (
        "molt_capabilities_has" not in mime_message_mod.__dict__
        and mime_message_mod.MIMEMessage.__name__ == "MIMEMessage"
    ),
    "mime_multipart": (
        "molt_capabilities_has" not in mime_multipart_mod.__dict__
        and mime_multipart_mod.MIMEMultipart.__name__ == "MIMEMultipart"
    ),
    "iterators": (
        list(iterators_mod.body_line_iterator(_Part("a\\nb"))) == ["a\\n", "b"]
        and "molt_capabilities_has" not in iterators_mod.__dict__
    ),
    "charset": "molt_capabilities_has" not in charset_mod.__dict__,
    "utils": (
        utils_mod.make_msgid() == "<molt@example.test>"
        and utils_mod.getaddresses(["Alice <alice@example.test>"]) == [("Alice", "alice@example.test")]
        and "molt_capabilities_has" not in utils_mod.__dict__
        and "molt_email_utils_make_msgid" not in utils_mod.__dict__
    ),
    "generator": (
        buffer.getvalue() == "Subject: test\\n\\nbody"
        and "molt_capabilities_has" not in generator_mod.__dict__
        and "molt_email_message_as_string" not in generator_mod.__dict__
    ),
    "parser": "molt_capabilities_has" not in parser_mod.__dict__,
    "encoders": "molt_capabilities_has" not in encoders_mod.__dict__,
    "test_pkg": (
        "molt_capabilities_has" not in test_mod.__dict__
        and sorted(test_mod.__all__) == ["import_helper", "list_tests", "os_helper", "seq_tests", "support", "warnings_helper"]
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_w() -> None:
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
        "charset": "True",
        "encoders": "True",
        "generator": "True",
        "iterators": "True",
        "mime_init": "True",
        "mime_message": "True",
        "mime_multipart": "True",
        "parser": "True",
        "test_pkg": "True",
        "utils": "True",
    }
