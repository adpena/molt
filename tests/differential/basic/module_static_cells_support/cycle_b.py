"""The other half of the import cycle."""

from . import cycle_a

try:
    early = cycle_a.late
except AttributeError as exc:
    early = "before store: " + type(exc).__name__


def read_late():
    return cycle_a.late
