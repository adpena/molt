# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,os.uname
"""Purpose: the shape of sysconfig.get_platform().

get_platform() names the running host, so the program checks it against the
rule CPython documents for that host and prints only whether it holds.
"""

import os
import re
import sys

import sysconfig


def _platform_follows_host_rule(platform: str) -> bool:
    if os.name == "nt":
        return platform in {"win32", "win-amd64", "win-arm32", "win-arm64"}
    if not hasattr(os, "uname"):
        return platform == sys.platform
    machine = os.uname().machine.replace(" ", "_").replace("/", "-")
    if sys.platform == "darwin":
        # macosx-<deployment or system version>-<machine>, or the plain
        # darwin-<release>-<machine> when neither version is known.
        osname, _, rest = platform.partition("-")
        release, _, arch = rest.rpartition("-")
        if osname == "macosx":
            return re.fullmatch(r"\d+\.\d+", release) is not None and arch in {
                machine,
                "universal2",
                "fat",
                "fat3",
                "fat64",
                "intel",
                "universal",
            }
        return osname == "darwin" and arch == machine and bool(release)
    if sys.platform.startswith("linux"):
        return platform == f"linux-{machine}"
    parts = platform.split("-")
    return len(parts) >= 2 and parts[-1] == machine and all(parts)


platform = sysconfig.get_platform()
print("platform_type", type(platform).__name__)
print("platform_stable", sysconfig.get_platform() == platform)
print("platform_no_space", platform == platform.strip() and " " not in platform)
print("platform_follows_host_rule", _platform_follows_host_rule(platform))
