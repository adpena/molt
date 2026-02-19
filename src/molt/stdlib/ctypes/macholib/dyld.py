"""Public API surface shim for ``ctypes.macholib.dyld``."""

from __future__ import annotations

import os

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


class accumulate:
    pass


class batched:
    pass


class chain:
    pass


class combinations:
    pass


class combinations_with_replacement:
    pass


class compress:
    pass


class count:
    pass


class cycle:
    pass


class dropwhile:
    pass


class filterfalse:
    pass


class groupby:
    pass


class islice:
    pass


class pairwise:
    pass


class permutations:
    pass


class product:
    pass


class repeat:
    pass


class starmap:
    pass


class takewhile:
    pass


tee = len


class zip_longest:
    pass


DEFAULT_FRAMEWORK_FALLBACK = [
    "/Library/Frameworks",
    "/System/Library/Frameworks",
]
DEFAULT_LIBRARY_FALLBACK = [
    "/usr/local/lib",
    "/usr/lib",
]


def dylib_info(_path: str):
    return None


def framework_info(_path: str):
    return None


def dyld_env(env: str, default: list[str] | None = None) -> list[str]:
    raw = os.environ.get(env, "")
    if raw:
        return [entry for entry in raw.split(":") if entry]
    return list(default or [])


def dyld_image_suffix() -> str | None:
    suffix = os.environ.get("DYLD_IMAGE_SUFFIX", "")
    return suffix or None


def dyld_framework_path() -> list[str]:
    return dyld_env("DYLD_FRAMEWORK_PATH")


def dyld_library_path() -> list[str]:
    return dyld_env("DYLD_LIBRARY_PATH")


def dyld_fallback_framework_path() -> list[str]:
    return dyld_env("DYLD_FALLBACK_FRAMEWORK_PATH", DEFAULT_FRAMEWORK_FALLBACK)


def dyld_fallback_library_path() -> list[str]:
    return dyld_env("DYLD_FALLBACK_LIBRARY_PATH", DEFAULT_LIBRARY_FALLBACK)


def dyld_executable_path_search(name: str) -> list[str]:
    exe_dir = os.path.dirname(os.path.realpath(getattr(os, "__file__", "") or ""))
    return [os.path.join(exe_dir, name)] if exe_dir else [name]


def dyld_override_search(name: str) -> list[str]:
    return [name]


def dyld_default_search(name: str) -> list[str]:
    return [name]


def dyld_image_suffix_search(iterator):
    suffix = dyld_image_suffix()
    if not suffix:
        for name in iterator:
            yield name
        return
    for name in iterator:
        root, ext = os.path.splitext(name)
        yield f"{root}{suffix}{ext}"
        yield name


def dyld_find(name: str, executable_path: str | None = None, env: dict | None = None):
    del executable_path, env
    for candidate in dyld_image_suffix_search(dyld_override_search(name)):
        if os.path.isabs(candidate) and os.path.exists(candidate):
            return candidate
    return name


def framework_find(name: str):
    return name
