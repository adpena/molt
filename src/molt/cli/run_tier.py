"""How ``molt run`` executes a native program.

Each tier is one canonical path. Selection is explicit: no tier falls back to
another, so a program the JIT tier cannot run fails with a diagnostic that
names the construct (``docs/design/dev_jit_tier.md``).
"""

from __future__ import annotations

from collections.abc import Sequence
from enum import StrEnum
from pathlib import Path


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

# A JIT build publishes one executable JIT image, named after the executable
# an AOT build would publish (src/molt/cli/jit_image.py).
JIT_IMAGE_SUFFIX = ".jit"

# What `molt-jit --start-receipt <path>` writes after the program compiled,
# immediately before the program starts (runtime/molt-jit/src/main.rs).
JIT_START_RECEIPT = '{"schema":"molt.jit-start.v1"}\n'


def jit_runner_argv(
    runner: str,
    image: str,
    program_arguments: Sequence[str],
    *,
    start_receipt: str | None = None,
) -> list[str]:
    """The runner command that starts the program in ``image``.

    ``image`` is also the program's argv[0], so ``sys.executable`` names the
    image, which re-runs the program as an AOT executable's path does.
    """
    argv = [runner, "--launch", image]
    if start_receipt is not None:
        argv += ["--start-receipt", start_receipt]
    return [*argv, "--", image, *program_arguments]


def jit_program_started(receipt: Path) -> bool:
    """Whether the runner given ``--start-receipt receipt`` started its program.

    The JIT tier compiles when it runs. A runner that leaves no receipt failed
    before the program started, which is the JIT tier's form of a failed build.
    """
    try:
        return receipt.read_text(encoding="utf-8") == JIT_START_RECEIPT
    except FileNotFoundError:
        return False
