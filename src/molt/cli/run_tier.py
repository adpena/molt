"""How ``molt run`` executes a native program.

Each tier is one canonical path. Selection is explicit: no tier falls back to
another, so a program the JIT tier cannot run fails with a diagnostic that
names the construct (``docs/design/dev_jit_tier.md``).
"""

from __future__ import annotations

from collections.abc import Sequence
from enum import StrEnum
from pathlib import Path
from typing import TYPE_CHECKING

from molt.cli.native_launch_abi_generated import (
    JIT_RUNNER_IMAGE_FLAG,
    JIT_RUNNER_START_RECEIPT_FLAG,
    JIT_START_RECEIPT,
)

if TYPE_CHECKING:
    from molt.compilation_target import Target
    from molt.native_target import NativeTargetSpec


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


# The (os, arch) hosts where the JIT-AOT twin gate ran (HF-371). Any other
# host is refused before the frontend runs and before its unverified runtime
# image link and loader run.
VERIFIED_JIT_HOSTS = frozenset({("macos", "aarch64")})


def jit_tier_refusal(
    *,
    target: Target,
    codegen_backend: str,
    host: NativeTargetSpec | None = None,
    compiler_root: Path | None = None,
) -> str | None:
    """Name the first fact that keeps the JIT tier from running, or None.

    The one admission of the JIT tier. Build request resolution applies it to
    every ``emit=jit`` request, so ``molt build --emit jit``, ``molt run
    --tier jit`` and the differential harness refuse an unsupported cell
    before the frontend runs. ``host`` and ``compiler_root`` default to this
    process's host and compiler. External native package artifacts are
    refused by the build's native-artifact custody check, which needs the
    import plan.
    """
    from molt.compilation_target import TargetKind
    from molt.native_target import NativeTargetSpec, resolve_native_target_spec

    if isinstance(target, NativeTargetSpec):
        if not target.is_host:
            return f"the cross target {target.triple} (the JIT tier runs on the host)"
    elif target is not TargetKind.NATIVE:
        return f"the {target.value} target (the JIT tier runs native host code)"
    host = resolve_native_target_spec(None) if host is None else host
    if (host.os, host.arch) not in VERIFIED_JIT_HOSTS:
        return (
            f"the {host.triple} host (the JIT tier is verified only on aarch64 "
            "macOS; HF-371 tracks the other hosts)"
        )
    if codegen_backend != "cranelift":
        return f"the {codegen_backend} code generator (the JIT tier uses Cranelift)"
    if _installed_compiler(compiler_root):
        return (
            "an installed Molt compiler (it ships no JIT runner; use a source checkout)"
        )
    return None


def _installed_compiler(compiler_root: Path | None) -> bool:
    from molt.cli.compiler_metadata import _compiler_root
    from molt.compiler_distribution import installed_compiler

    root = _compiler_root() if compiler_root is None else compiler_root
    return installed_compiler(root) is not None


def jit_tier_refusal_message(refusal: str) -> str:
    """The diagnostic for a refused JIT request."""
    return (
        f"The JIT tier does not support {refusal}. "
        "Run it with `molt run --tier aot` or build it with `molt build`."
    )


def jit_runner_argv(
    runner: str,
    image: str,
    program_arguments: Sequence[str],
    *,
    start_receipt: str | None = None,
) -> list[str]:
    """The runner command that starts the program in ``image``.

    It is the command ``execve`` of the image runs (``#!<runner> --image``),
    with ``--start-receipt`` added. ``image`` is also the program's argv[0],
    so ``sys.executable`` names the image, which re-runs the program as an
    AOT executable's path does.
    """
    argv = [runner]
    if start_receipt is not None:
        argv += [JIT_RUNNER_START_RECEIPT_FLAG, start_receipt]
    return [*argv, JIT_RUNNER_IMAGE_FLAG, image, *program_arguments]


def jit_program_started(receipt: Path) -> bool:
    """Whether the runner given ``--start-receipt receipt`` started its program.

    The JIT tier compiles when it runs. A runner that leaves no receipt failed
    before the program started, which is the JIT tier's form of a failed build.
    """
    try:
        return receipt.read_text(encoding="utf-8") == JIT_START_RECEIPT
    except FileNotFoundError:
        return False
