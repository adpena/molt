"""Half of an import cycle. B reads A's global before A stores it, then
after."""

seen = "unset"
from . import cycle_b  # noqa: E402

late = "late value"
seen = cycle_b.early
after = cycle_b.read_late()
