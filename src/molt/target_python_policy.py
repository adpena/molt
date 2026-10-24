"""Admitted language coordinates, independent of generated source-release facts.

Adding a source pin does not admit a target. This policy also bootstraps the
generators when their installed release-fact projection needs regeneration.
"""

SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS: tuple[str, ...] = ("3.12", "3.13", "3.14")
DEFAULT_TARGET_PYTHON_SHORT_VERSION = "3.12"
