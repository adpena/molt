"""Install-scheme authority for a Molt wheel and its distribution data."""

from __future__ import annotations

import os
from pathlib import Path
import sysconfig

# Repository tools may select the layout before accepting a loaded package.
__molt_neutral_authority__ = True

PACKAGED_DISTRIBUTION_PATH = ("share", "molt", "distribution")


def distribution_for_package(package: Path) -> Path | None:
    """Locate data through the install scheme that owns this package path."""
    package_parent = os.path.normcase(str(package.resolve().parent))
    for scheme in sysconfig.get_scheme_names():
        try:
            paths = sysconfig.get_paths(scheme)
        except KeyError:
            continue
        libraries = {
            os.path.normcase(os.path.realpath(paths[key]))
            for key in ("purelib", "platlib")
            if key in paths
        }
        if package_parent in libraries and "data" in paths:
            return Path(paths["data"]).joinpath(*PACKAGED_DISTRIBUTION_PATH)
    return None
