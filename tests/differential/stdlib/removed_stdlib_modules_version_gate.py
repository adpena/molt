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
statuses: list[tuple[str, str]] = []

for module_name in REMOVED_MODULES:
    try:
        importlib.import_module(module_name)
    except ModuleNotFoundError:
        status = "absent"
    except Exception as exc:  # noqa: BLE001
        status = f"error:{type(exc).__name__}"
    else:
        status = "present"

    if is_313_plus and status != "absent":
        raise AssertionError(
            f"{module_name} must raise ModuleNotFoundError on Python >= 3.13, got {status}"
        )
    statuses.append((module_name, status))

print("version", version)
print("is_313_plus", is_313_plus)
for module_name, status in statuses:
    print(module_name, status)
