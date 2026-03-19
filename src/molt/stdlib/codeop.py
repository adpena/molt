"""Intrinsic-backed codeop implementation (Python 3.12+)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CODEOP_COMPILE = _require_intrinsic("molt_codeop_compile")
_MOLT_CODEOP_COMPILE_COMMAND = _require_intrinsic(
    "molt_codeop_compile_command"
)

__all__ = ["compile_command", "Compile", "CommandCompiler"]

# The following flags match Include/cpython/compile.h in CPython 3.12.
PyCF_DONT_IMPLY_DEDENT = 0x200
PyCF_ALLOW_INCOMPLETE_INPUT = 0x4000


def compile_command(source, filename="<input>", symbol="single"):
    code, _next_flags = _MOLT_CODEOP_COMPILE_COMMAND(source, filename, symbol, 0)
    return code


class Compile:
    """Stateful compile wrapper that tracks __future__ flags."""

    def __init__(self):
        self.flags = PyCF_DONT_IMPLY_DEDENT | PyCF_ALLOW_INCOMPLETE_INPUT

    def __call__(self, source, filename, symbol, **kwargs):
        incomplete_input = kwargs.get("incomplete_input", True)
        flags = self.flags
        if incomplete_input is False:
            flags &= ~PyCF_DONT_IMPLY_DEDENT
            flags &= ~PyCF_ALLOW_INCOMPLETE_INPUT
        codeob, next_flags = _MOLT_CODEOP_COMPILE(
            source, filename, symbol, flags, incomplete_input
        )
        self.flags = next_flags
        return codeob


class CommandCompiler:
    """Callable compile_command-like object with __future__ flag memory."""

    def __init__(self):
        self.compiler = Compile()

    def __call__(self, source, filename="<input>", symbol="single"):
        codeob, next_flags = _MOLT_CODEOP_COMPILE_COMMAND(
            source, filename, symbol, self.compiler.flags
        )
        self.compiler.flags = next_flags
        return codeob
