# MOLT_ENV: MOLT_CAPABILITIES=env.read,env.write
"""Purpose: differential coverage for locale basics."""

import locale


print(locale.setlocale(locale.LC_ALL, "C"))
print(locale.getpreferredencoding(False))
print(locale.getlocale())
print(locale.setlocale(locale.LC_ALL, None))

try:
    locale.setlocale(locale.LC_ALL, 1)
except Exception as exc:
    print("setlocale_bad", type(exc).__name__)


# Category queries and preferred encoding have distinct owners: a live locale
# mutation changes getencoding, while UTF-8 mode remains a startup decision.
import _locale
import sys

saved = locale.setlocale(locale.LC_CTYPE)
startup_flags = sys.flags.utf8_mode
startup_fs = (sys.getfilesystemencoding(), sys.getfilesystemencodeerrors())
try:
    print("tuple-locale", locale.setlocale(locale.LC_CTYPE, (None, None)))
    print(
        "live-locale", locale.getlocale(), locale.getencoding() == _locale.getencoding()
    )
    print(
        "startup-stable",
        sys.flags.utf8_mode == startup_flags,
        (sys.getfilesystemencoding(), sys.getfilesystemencodeerrors()) == startup_fs,
    )
    print(
        "preferred-owner",
        locale.getpreferredencoding(False)
        == ("utf-8" if sys.flags.utf8_mode else locale.getencoding()),
    )
    print(
        "normalization",
        [
            locale.normalize(value)
            for value in ("en_US", "de_DE@euro", "ja_JP.SJIS", "unknown-unchanged")
        ],
    )
    print(
        "native-kind",
        type(_locale.setlocale).__name__,
        _locale.setlocale.__module__,
        _locale.setlocale.__text_signature__,
    )
    print(
        "encoding-kind",
        type(_locale.getencoding).__name__,
        _locale.getencoding.__module__,
        _locale.getencoding.__text_signature__,
    )
    try:
        _locale.setlocale(category=locale.LC_CTYPE)
    except TypeError:
        print("positional-only", True)
    try:
        locale.setlocale(locale.LC_CTYPE, "molt.invalid.locale")
    except locale.Error:
        print("platform-refusal", True)
finally:
    locale.setlocale(locale.LC_CTYPE, saved)

# putenv mutates the process environment, while os.environ remains the Python
# mapping snapshot used by path expansion. Locale selection reads libc instead.
import os

key = "MOLT_LOCALE_MAPPING_ORACLE"
saved_value = os.environ.get(key)
try:
    os.environ[key] = "mapping-value"
    os.putenv(key, "process-value")
    print("mapping-expansion", os.path.expandvars("${" + key + "}/tail"))
    os.unsetenv(key)
    print("mapping-after-unsetenv", os.environ[key])
    print("mapping-snapshot", os.environ.copy()[key])
    print("mapping-iteration", key in list(os.environ), dict(os.environ.items())[key])
    print("bytes-expansion", os.path.expandvars(os.fsencode("${" + key + "}/tail")))
    print("bytes-home-type", isinstance(os.path.expanduser(b"~/tail"), bytes))
finally:
    if saved_value is None:
        del os.environ[key]
    else:
        os.environ[key] = saved_value

# Raw process environment transport must round-trip through every mapping view.
if os.name == "posix":
    raw_key = "MOLT_RAW_MAPPING_" + os.fsdecode(b"\xff")
    raw_value = os.fsdecode(b"value-\xfe")
    prior = os.environ.get(raw_key)
    try:
        os.environ[raw_key] = raw_value
        print("raw-environment", os.fsencode(os.environ[raw_key]) == b"value-\xfe")
        print("raw-snapshot", os.environ.copy()[raw_key] == raw_value)
        print("raw-iteration", raw_key in list(os.environ),
              dict(os.environ.items())[raw_key] == raw_value)
        os.putenv(raw_key, "process-only")
        print("raw-mapping-isolation", os.environ.copy()[raw_key] == raw_value)
        os.unsetenv(raw_key)
        print("raw-unset-isolation", os.environ[raw_key] == raw_value,
              os.environ.copy()[raw_key] == raw_value)
    finally:
        if prior is None:
            del os.environ[raw_key]
        else:
            os.environ[raw_key] = prior

# Windows publishes its actual user-locale/codepage provider; POSIX keeps the
# environment policy. No unavailable provider is replaced by host Python.
import warnings

print(
    "default-provider",
    hasattr(_locale, "_getdefaultlocale") == (sys.platform == "win32"),
)
if sys.platform == "win32":
    code, encoding = _locale._getdefaultlocale()
    expected_code = code
    if sys.version_info < (3, 15) and code and code.startswith("0x"):
        expected_code = locale.windows_locale.get(int(code, 0))
    print(
        "default-native",
        code is None or isinstance(code, str),
        encoding.startswith("cp"),
        encoding[2:].isdigit(),
    )
    print("default-policy", locale._getdefaultlocale() == (expected_code, encoding))
    print(
        "default-kind",
        type(_locale._getdefaultlocale).__name__,
        _locale._getdefaultlocale.__text_signature__,
    )
else:
    print("default-posix-empty", locale._getdefaultlocale(()) == (None, None))
with warnings.catch_warnings(record=True) as observed:
    warnings.simplefilter("always", DeprecationWarning)
    locale.getdefaultlocale(())
    print(
        "default-warning",
        len(observed),
        all(item.category is DeprecationWarning for item in observed),
    )
