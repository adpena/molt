from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import _locale as reference_locale
import sys
import tempfile
import zipfile
from pathlib import Path
from tests.stdlib_intrinsic_registry import install_registry


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


install_registry({{
    "molt_http_client_connection_new": lambda *args, **kwargs: None,
    "molt_http_status_reason": lambda code: "OK" if code == 200 else "",
    "molt_http_cookies_parse": lambda raw: [("session", "abc")],
    "molt_http_cookies_render_morsel": lambda key, value, path, secure, httponly, max_age, expires: f"{{key}}={{value}}",
    "molt_sys_platform": lambda: sys.platform,
    "molt_sys_version_info": lambda: tuple(sys.version_info),
    **({{"molt_locale_getdefaultlocale": reference_locale._getdefaultlocale}} if sys.platform == "win32" else {{}}),
    "molt_locale_setlocale": lambda category, locale=None: "C",
    "molt_locale_getencoding": lambda: "utf-8",
    "molt_locale_constants": lambda: {{name: getattr(reference_locale, name) for name in dir(reference_locale) if name.startswith("LC_") or name == "CHAR_MAX"}},
    "molt_function_set_builtin": lambda function: None,
    "molt_shlex_quote": lambda s: "'" + s + "'",
    "molt_shlex_split_ex": lambda source, whitespace, posix, comments, whitespace_split, commenters, punctuation_chars: source.split(),
    "molt_shlex_join": lambda parts: " ".join(parts),
}})

http_mod = _load_module("molt_test_http", {str(STDLIB_ROOT / "http" / "__init__.py")!r})
cookies_mod = _load_module("molt_test_http_cookies", {str(STDLIB_ROOT / "http" / "cookies.py")!r})
_load_module("_locale", {str(STDLIB_ROOT / "_locale.py")!r})
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
        and "molt_http_client_connection_new" not in http_mod.__dict__
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
        "cookies": "True",
        "ctypes_util": "True",
        "doctest": "True",
        "http": "True",
        "locale": "True",
        "shlex": "True",
        "zipapp": "True",
    }


def test_locale_policy_uses_current_low_level_category_authority(monkeypatch) -> None:
    """Real CPython libc operations are the oracle, not a second locale model."""
    import _locale as reference_native
    import importlib.util
    import locale as reference_policy

    import pytest

    from tests.stdlib_intrinsic_registry import intrinsic_registry

    constants = {
        name: getattr(reference_native, name)
        for name in dir(reference_native)
        if name.startswith("LC_") or name == "CHAR_MAX"
    }
    promoted = []
    category_calls = []

    def setlocale(category, value):
        category_calls.append((category, value))
        try:
            return reference_native.setlocale(category, value)
        except reference_native.Error:
            return None

    def load(name, filename):
        spec = importlib.util.spec_from_file_location(name, STDLIB_ROOT / filename)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        monkeypatch.setitem(sys.modules, name, module)
        spec.loader.exec_module(module)
        return module

    original = reference_native.setlocale(reference_native.LC_ALL)
    expected_aliases = {
        spelling: reference_policy.normalize(spelling)
        for spelling in (
            "C",
            "POSIX",
            "en_US",
            "en_US.UTF8",
            "de_DE@euro",
            "ja_JP.SJIS",
            "unknown-unchanged",
        )
    }
    with intrinsic_registry(
        {
            "molt_sys_platform": lambda: sys.platform,
            "molt_sys_version_info": lambda: tuple(sys.version_info),
            **(
                {"molt_locale_getdefaultlocale": reference_native._getdefaultlocale}
                if sys.platform == "win32"
                else {}
            ),
            "molt_locale_setlocale": setlocale,
            "molt_locale_getencoding": reference_native.getencoding,
            "molt_locale_constants": lambda: constants,
            "molt_function_set_builtin": promoted.append,
        }
    ):
        native = load("_locale", "_locale.py")
        policy = load("molt_test_locale_policy", "locale.py")
        try:
            assert constants == {name: getattr(native, name) for name in constants}
            expected_promoted = [native.setlocale, native.getencoding]
            if sys.platform == "win32":
                expected_promoted.append(native._getdefaultlocale)
            assert promoted == expected_promoted
            assert (
                native.setlocale.__text_signature__
                == reference_native.setlocale.__text_signature__
            )
            assert (
                native.getencoding.__text_signature__
                == reference_native.getencoding.__text_signature__
            )
            assert policy.setlocale(policy.LC_CTYPE, (None, None)) == "C"
            assert policy.getlocale() == reference_policy.getlocale() == (None, None)
            assert policy.getencoding() == reference_native.getencoding()
            assert policy.getpreferredencoding(
                False
            ) == reference_policy.getpreferredencoding(False)
            assert {
                key: policy.normalize(key) for key in expected_aliases
            } == expected_aliases
            before = reference_native.setlocale(reference_native.LC_CTYPE)
            assert policy.getpreferredencoding(
                True
            ) == reference_policy.getpreferredencoding(True)
            assert reference_native.setlocale(reference_native.LC_CTYPE) == before
            # No native high-level getlocale/preferred-encoding owner remains.
            assert "molt_locale_getlocale" not in vars(policy)
            assert "molt_locale_getpreferredencoding" not in vars(policy)
            with pytest.raises(TypeError):
                native.setlocale(category=native.LC_CTYPE)
            with pytest.raises(native.Error):
                policy.setlocale(policy.LC_CTYPE, "molt.invalid.locale")
            with pytest.raises(TypeError):
                policy.setlocale(policy.LC_CTYPE, 1)
            assert category_calls
        finally:
            reference_native.setlocale(reference_native.LC_ALL, original)


def test_default_locale_provider_publication_and_versioned_policy(monkeypatch) -> None:
    import _locale as reference_native
    import importlib.util
    import inspect
    import types
    import warnings

    import pytest

    from tests.stdlib_intrinsic_registry import intrinsic_registry

    constants = {
        name: getattr(reference_native, name)
        for name in dir(reference_native)
        if name.startswith("LC_") or name == "CHAR_MAX"
    }

    def load(name, filename):
        spec = importlib.util.spec_from_file_location(name, STDLIB_ROOT / filename)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        monkeypatch.setitem(sys.modules, name, module)
        spec.loader.exec_module(module)
        return module

    # Only the platform operation is supplied. Both real facade definitions and
    # their CP policy/catalog execute through the shared strict registry.
    for minor in (12, 13, 14, 15):
        for platform in ("win32", "linux"):
            result = ("en_US", "cp1252")
            failure = None
            calls = []
            promoted = []

            def provider():
                calls.append("provider")
                if failure is not None:
                    raise failure
                return result

            registry = {
                "molt_sys_platform": lambda: platform,
                "molt_sys_version_info": lambda: (3, minor, 0, "final", 0),
                "molt_locale_setlocale": lambda category, value: "C",
                "molt_locale_getencoding": lambda: "cp1252",
                "molt_locale_constants": lambda: constants,
                "molt_function_set_builtin": promoted.append,
            }
            if platform == "win32":
                registry["molt_locale_getdefaultlocale"] = provider
            with intrinsic_registry(registry):
                native = load("_locale", "_locale.py")
                policy = load("molt_test_default_locale", "locale.py")
                policy.sys = types.SimpleNamespace(platform=platform)
                for function in (policy.getdefaultlocale, policy._getdefaultlocale):
                    assert (
                        str(inspect.signature(function))
                        == "(envvars=('LC_ALL', 'LC_CTYPE', 'LANG', 'LANGUAGE'))"
                    )
                    with pytest.raises(TypeError):
                        function(_legacy_windows=True)
                monkeypatch.setenv("MOLT_DEFAULT_LOCALE_ORACLE", "fr_FR.UTF-8")
                envvars = ("MOLT_DEFAULT_LOCALE_ORACLE",)
                assert hasattr(native, "_getdefaultlocale") == (platform == "win32")
                if platform == "win32":
                    assert len(promoted) == 3
                    assert set(promoted) == {
                        native.setlocale,
                        native.getencoding,
                        native._getdefaultlocale,
                    }
                    assert native._getdefaultlocale.__text_signature__ == "($module, /)"
                    with pytest.raises(TypeError):
                        native._getdefaultlocale(1)
                    with pytest.raises(TypeError):
                        native._getdefaultlocale(ignored=1)
                    assert not calls
                    for language, expected in (
                        ("en_US", "en_US"),
                        ("0x0409", "en_US" if minor < 15 else "0x0409"),
                        ("0xffff", None if minor < 15 else "0xffff"),
                        (None, None),
                    ):
                        result = (language, "cp932")
                        assert policy._getdefaultlocale(envvars) == (expected, "cp932")
                    # Capability/other provider errors are never absence.
                    for error in (
                        PermissionError("locale denied"),
                        RuntimeError("provider failed"),
                    ):
                        failure = error
                        with pytest.raises(type(error)) as caught:
                            policy._getdefaultlocale(envvars)
                        assert caught.value is error
                    # Preserve CP's exact optional-provider aperture, including
                    # AttributeError from the call, not just missing attributes.
                    for error in (
                        AttributeError("provider absent"),
                        ImportError("provider absent"),
                    ):
                        failure = error
                        assert policy._getdefaultlocale(envvars) == ("fr_FR", "UTF-8")
                    failure = None
                    del native._getdefaultlocale
                else:
                    assert promoted == [native.setlocale, native.getencoding]
                    assert not calls
                assert policy._getdefaultlocale(envvars) == ("fr_FR", "UTF-8")
                monkeypatch.setenv("LANGUAGE", "de_DE.UTF-8:fr_FR.UTF-8")
                assert policy._getdefaultlocale(("LANGUAGE",)) == ("de_DE", "UTF-8")
                assert policy._getdefaultlocale(()) == (None, None)
                warning_calls = []
                monkeypatch.setattr(
                    warnings,
                    "_deprecated",
                    lambda *args, **kwargs: warning_calls.append((args, kwargs)),
                )
                assert policy.getdefaultlocale(envvars) == ("fr_FR", "UTF-8")
                assert len(warning_calls) == (0 if minor == 15 else 1)
                if warning_calls:
                    assert warning_calls[0][0][0] == "locale.getdefaultlocale"
                    assert warning_calls[0][1] == {"remove": (3, 15)}
