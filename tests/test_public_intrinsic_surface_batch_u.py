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
import tempfile
import types
import zipfile
from pathlib import Path


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


builtins._molt_intrinsics = {{
    "molt_http_client_execute": lambda *args, **kwargs: None,
    "molt_http_status_reason": lambda code: "OK" if code == 200 else "",
    "molt_http_cookies_parse": lambda raw: [("session", "abc")],
    "molt_http_cookies_render_morsel": lambda key, value, path, secure, httponly, max_age, expires: f"{{key}}={{value}}",
    "molt_locale_setlocale": lambda category, locale=None: "C",
    "molt_locale_getpreferredencoding": lambda do_setlocale=True: "utf-8",
    "molt_locale_getlocale": lambda category=None: ("en_US", "UTF-8"),
    "molt_shlex_quote": lambda s: "'" + s + "'",
    "molt_shlex_split_ex": lambda source, whitespace, posix, comments, whitespace_split, commenters, punctuation_chars: source.split(),
    "molt_shlex_join": lambda parts: " ".join(parts),
    "molt_zipapp_runtime_ready": lambda: True,
    "molt_capabilities_has": lambda name: True,
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

http_mod = _load_module("molt_test_http", {str(STDLIB_ROOT / "http" / "__init__.py")!r})
cookies_mod = _load_module("molt_test_http_cookies", {str(STDLIB_ROOT / "http" / "cookies.py")!r})
locale_mod = _load_module("molt_test_locale", {str(STDLIB_ROOT / "locale.py")!r})
shlex_mod = _load_module("molt_test_shlex", {str(STDLIB_ROOT / "shlex.py")!r})
zipapp_mod = _load_module("molt_test_zipapp", {str(STDLIB_ROOT / "zipapp.py")!r})
ctypes_util_mod = _load_module("molt_test_ctypes_util", {str(STDLIB_ROOT / "ctypes" / "util.py")!r})
doctest_mod = _load_module("molt_test_doctest", {str(STDLIB_ROOT / "doctest.py")!r})


def _raises_runtimeerror(fn):
    try:
        fn()
    except RuntimeError:
        return True
    return False


with tempfile.TemporaryDirectory() as td:
    archive_path = Path(td) / "sample.pyz"
    with zipfile.ZipFile(archive_path, "w") as archive:
        archive.writestr("mod.py", b"print('ok')\\n")
    zipapp_ok = zipapp_mod.is_archive(archive_path)

checks = {{
    "http": (
        http_mod.HTTPStatus.OK.phrase == "OK"
        and "molt_http_client_execute" not in http_mod.__dict__
        and "molt_http_status_reason" not in http_mod.__dict__
    ),
    "cookies": (
        cookies_mod.SimpleCookie().output() == ""
        and "molt_http_cookies_parse" not in cookies_mod.__dict__
        and "molt_http_cookies_render_morsel" not in cookies_mod.__dict__
    ),
    "locale": (
        locale_mod.getpreferredencoding() == "utf-8"
        and "molt_locale_setlocale" not in locale_mod.__dict__
        and "molt_locale_getlocale" not in locale_mod.__dict__
    ),
    "shlex": (
        shlex_mod.join(["a", "b"]) == "a b"
        and "molt_shlex_quote" not in shlex_mod.__dict__
        and "molt_shlex_split_ex" not in shlex_mod.__dict__
        and "molt_shlex_join" not in shlex_mod.__dict__
    ),
    "zipapp": (
        zipapp_ok
        and "molt_zipapp_runtime_ready" not in zipapp_mod.__dict__
    ),
    "ctypes_util": (
        ctypes_util_mod.find_library("") is None
        and "molt_capabilities_has" not in ctypes_util_mod.__dict__
    ),
    "doctest": (
        _raises_runtimeerror(lambda: doctest_mod.DocTestSuite())
        and "molt_capabilities_has" not in doctest_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_u() -> None:
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
        "cookies": "True",
        "ctypes_util": "True",
        "doctest": "True",
        "http": "True",
        "locale": "True",
        "shlex": "True",
        "zipapp": "True",
    }
