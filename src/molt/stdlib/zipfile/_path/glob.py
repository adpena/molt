"""Glob translation helpers used by ``zipfile.Path``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import re
import sys as _sys

_require_intrinsic("molt_capabilities_has")
_MOLT_ZIPFILE_PATH_TRANSLATE_GLOB = _require_intrinsic(
    "molt_zipfile_path_translate_glob"
)


if _sys.version_info >= (3, 13):
    import os

    _default_seps = os.sep + str(os.altsep) * bool(os.altsep)

    class Translator:
        seps: str

        def __init__(self, seps: str = _default_seps):
            assert seps and set(seps) <= set(_default_seps), "Invalid separators"
            self.seps = seps

        def translate(self, pattern: str) -> str:
            return _MOLT_ZIPFILE_PATH_TRANSLATE_GLOB(pattern, self.seps, True)

        def extend(self, pattern: str) -> str:
            return rf"(?s:{pattern})\Z"

        def match_dirs(self, pattern: str) -> str:
            return rf"{pattern}[/]?"

        def translate_core(self, pattern: str) -> str:
            self.restrict_rglob(pattern)
            return "".join(map(self.replace, separate(self.star_not_empty(pattern))))

        def replace(self, match: re.Match[str]) -> str:
            return match.group("set") or (
                re.escape(match.group(0))
                .replace("\\*\\*", r".*")
                .replace("\\*", rf"[^{re.escape(self.seps)}]*")
                .replace("\\?", r"[^/]")
            )

        def restrict_rglob(self, pattern: str) -> None:
            seps_pattern = rf"[{re.escape(self.seps)}]+"
            segments = re.split(seps_pattern, pattern)
            if any("**" in segment and segment != "**" for segment in segments):
                raise ValueError("** must appear alone in a path segment")

        def star_not_empty(self, pattern: str) -> str:
            def handle_segment(match: re.Match[str]) -> str:
                segment = match.group(0)
                return "?*" if segment == "*" else segment

            not_seps_pattern = rf"[^{re.escape(self.seps)}]+"
            return re.sub(not_seps_pattern, handle_segment, pattern)

    def separate(pattern: str):
        return re.finditer(r"([^\[]+)|(?P<set>[\[].*?[\]])|([\[][^\]]*$)", pattern)

else:

    def translate(pattern: str) -> str:
        return _MOLT_ZIPFILE_PATH_TRANSLATE_GLOB(pattern, "/", False)

    def match_dirs(pattern: str) -> str:
        return rf"{pattern}[/]?"

    def translate_core(pattern: str) -> str:
        return "".join(map(replace, separate(pattern)))

    def separate(pattern: str):
        return re.finditer(r"([^\[]+)|(?P<set>[\[].*?[\]])|([\[][^\]]*$)", pattern)

    def replace(match: re.Match[str]) -> str:
        return match.group("set") or (
            re.escape(match.group(0))
            .replace("\\*\\*", r".*")
            .replace("\\*", r"[^/]*")
            .replace("\\?", r".")
        )
