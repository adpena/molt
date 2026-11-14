"""Helper for module_static_cells.py: half of an import cycle. B reads A's
global before A stores it, then after."""

seen = "unset"
import module_static_cells_cycle_b  # noqa: E402

late = "late value"
seen = module_static_cells_cycle_b.early
after = module_static_cells_cycle_b.read_late()
