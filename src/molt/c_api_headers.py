"""One owned-header closure for C-API consumers and ABI generation."""

from __future__ import annotations

import re
from collections.abc import Sequence
from pathlib import Path


class CAPIHeaderClosureError(ValueError):
    """An owned SDK include cannot be resolved from its declared roots."""


def c_api_header_closure(
    header: Path, *, include_dirs: Sequence[Path]
) -> tuple[Path, ...]:
    """Follow local SDK includes, leaving system headers to the target toolchain.

    Quoted includes search their declaring file's directory first. Angle
    includes search only the selected ABI roots, never host include paths.
    Missing quoted or underscore-private includes are broken SDK custody, not
    permission to silently classify an incomplete declaration surface.
    """
    roots = tuple(directory.resolve() for directory in include_dirs)
    pending = [header]
    visited: set[Path] = set()
    include_re = re.compile(r'^\s*#\s*include\s*([<"])([^>"]+)[>"]', re.M)
    while pending:
        current = pending.pop().resolve()
        if current in visited:
            continue
        visited.add(current)
        source = current.read_text(encoding="utf-8")
        source = re.sub(r"/\*.*?\*/", "", source, flags=re.S)
        source = re.sub(r"//[^\n]*", "", source)
        for match in include_re.finditer(source):
            name = match.group(2)
            quoted = match.group(1) == '"'
            search_dirs = (current.parent, *roots) if quoted else roots
            for directory in search_dirs:
                dependency = directory / name
                if dependency.is_file():
                    pending.append(dependency)
                    break
            else:
                if quoted or Path(name).name.startswith("_"):
                    raise CAPIHeaderClosureError(
                        f"C-API header {current} includes missing local header {name!r}"
                    )
    return tuple(sorted(visited, key=lambda path: path.as_posix()))
