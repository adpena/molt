"""The builtins body runs on demand, inside sys's initialization (HF-974).

The runtime builds the builtins namespace when a frame first needs it. Under
the full profile that happens while sys's body runs, so builtins' own reads
of other modules (its version gate, `_sitebuiltins.help`) run before those
modules finish. A read compiled as an init-once cell load there returned the
cell's unbound word as a value; a by-name read raised AttributeError.
"""

# MOLT_META: stdlib_profile=full

import builtins
import sys

print(type(builtins.help).__name__, type(builtins.exit).__name__)
print(builtins.quit.__class__ is builtins.exit.__class__)
print(eval.__text_signature__)
print(exec.__text_signature__)
print(breakpoint.__text_signature__)
print(sys.version_info >= (3, 12))
