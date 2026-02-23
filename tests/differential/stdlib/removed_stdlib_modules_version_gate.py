"""Purpose: verify PEP 594 removed-module import gating across Python versions."""

import importlib
import sys
import warnings

REMOVED_MODULES = (
    "aifc",
    "audioop",
    "cgi",
    "cgitb",
    "chunk",
    "crypt",
    "imghdr",
    "mailcap",
    "msilib",
    "nis",
    "nntplib",
    "ossaudiodev",
    "pipes",
    "sndhdr",
    "spwd",
    "sunau",
    "telnetlib",
    "uu",
    "xdrlib",
)

# Keep output deterministic when running under warning-heavy configurations.
warnings.filterwarnings("ignore", category=DeprecationWarning)

version = tuple(sys.version_info[:3])
is_313_plus = sys.version_info >= (3, 13)
print("version", version)
print("is_313_plus", is_313_plus)

if not is_313_plus:
    print("pre_313_skip", True)
    raise SystemExit(0)

for module_name in REMOVED_MODULES:
    try:
        importlib.import_module(module_name)
    except ModuleNotFoundError:
        status = "absent"
    except Exception as exc:  # noqa: BLE001
        raise AssertionError(
            f"{module_name} must raise ModuleNotFoundError on Python >= 3.13, got {type(exc).__name__}"
        ) from exc
    else:
        raise AssertionError(
            f"{module_name} must raise ModuleNotFoundError on Python >= 3.13, got present"
        )
    print(module_name, "absent")
