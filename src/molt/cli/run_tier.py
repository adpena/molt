"""How ``molt run`` executes a native program.

Each tier is one canonical path. Selection is explicit: no tier falls back to
another, so a program the JIT tier cannot run fails with a diagnostic that
names the construct (``docs/design/dev_jit_tier.md``).
"""

from __future__ import annotations

from enum import StrEnum


class RunTier(StrEnum):
    # Build a native executable (object, link, publication), then run it.
    AOT = "aot"
    # Compile the program into the molt-jit runner's memory and run it there.
    JIT = "jit"

    @property
    def build_emit(self) -> str:
        """The ``molt build --emit`` artifact this tier runs."""
        match self:
            case RunTier.AOT:
                return "bin"
            case RunTier.JIT:
                return "jit"


DEFAULT_RUN_TIER = RunTier.AOT

# A JIT build publishes a launch manifest and the program's backend IR beside
# it, both named after the executable an AOT build would publish.
JIT_LAUNCH_SUFFIX = ".jit.json"
JIT_PROGRAM_SUFFIX = ".jit.msgpack"
