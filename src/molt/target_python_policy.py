"""Admitted language coordinates, independent of generated source-release facts.

Adding a source pin does not admit a target. This policy also bootstraps the
generators when their installed release-fact projection needs regeneration.
"""

from typing import Literal

TargetPythonShortVersion = Literal["3.12", "3.13", "3.14", "3.15"]
SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS: tuple[TargetPythonShortVersion, ...] = (
    "3.12",
    "3.13",
    "3.14",
    "3.15",
)
# The default and only Tier 1 target (owner decision 2026-11-16).
DEFAULT_TARGET_PYTHON_SHORT_VERSION: TargetPythonShortVersion = "3.15"
