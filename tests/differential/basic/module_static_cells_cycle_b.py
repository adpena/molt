"""Helper for module_static_cells.py: the other half of the import cycle."""

import module_static_cells_cycle_a

try:
    early = module_static_cells_cycle_a.late
except AttributeError as exc:
    early = "before store: " + type(exc).__name__


def read_late():
    return module_static_cells_cycle_a.late
