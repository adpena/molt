from .helper import ping
from probe_pkg.sibling import VALUE

print(ping(), VALUE)
print("before-missing-helper")
import helper  # noqa: E402 - distinguish the completed self-imports from failure.

print("unexpected-import", helper)
