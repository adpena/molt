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
# 3.15 is the only Tier 1 target (owner decision 2026-11-16) and becomes the
# default when it compiles code as fast as 3.12 (HF-900: bench_attr_access is
# 11.0x and bench_struct 7.6x more instructions at target 3.15 today).
DEFAULT_TARGET_PYTHON_SHORT_VERSION: TargetPythonShortVersion = "3.12"
