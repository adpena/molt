from __future__ import annotations

import re
import sys
from typing import Mapping


_PYTHON_WARNING_RE = re.compile(
    r"^.+:\d+: (?:Syntax|Deprecation|Runtime|User|Future|Pending\s*Deprecation)Warning: "
)

# Backend diagnostic knobs whose truthy presence means captured stderr should
# be streamed to the user rather than silently swallowed by JSON-mode wrappers.
_BACKEND_DIAGNOSTIC_ENV_KNOBS = frozenset(
    {
        "TIR_DUMP",
        "TIR_OPT_STATS",
        "MOLT_DUMP_CLIF",
        "MOLT_DUMP_CLIF_ON_ERROR",
        "MOLT_DUMP_CLIF_ON_CFG_ERROR",
        "MOLT_DUMP_CLIF_FUNC",
        "MOLT_DUMP_CLIF_FILE",
        "MOLT_DUMP_CLIF_FILE_FILTER",
        "MOLT_DUMP_FINAL_FUNC_IR",
        "MOLT_DUMP_IR",
        "MOLT_OVERFLOW_PEEL_STATS",
        "MOLT_PROMOTE_DEBUG",
        "MOLT_INLINE_STATS",
        "MOLT_DEBUG_BIND",
        "MOLT_DEBUG_CHECK_EXC",
        "MOLT_DEBUG_CHECK_EXCEPTION",
        "MOLT_LLVM_DUMP_IR",
        "MOLT_BACKEND_TIMING",
        "MOLT_MEMGVN_REPORT",
        "MOLT_MEMGVN_REPORT_BASELINE",
        "MOLT_MEMGVN_DIAG",
        "MOLT_MEMGVN_DUMP",
        "MOLT_DEBUG_DROP",
    }
)
_FALSY_ENV_VALUES = frozenset({"", "0", "false", "no", "off"})


def _env_requests_backend_diagnostics(env: Mapping[str, str]) -> bool:
    """True if any backend-diagnostic env knob is truthy in ``env``."""
    for key in _BACKEND_DIAGNOSTIC_ENV_KNOBS:
        value = env.get(key)
        if value is None:
            continue
        if value.strip().lower() not in _FALSY_ENV_VALUES:
            return True
    return False


def _forward_compilation_warnings(stderr: str) -> None:
    """Forward Python warnings from build subprocess stderr to the user."""
    lines = stderr.splitlines(keepends=True)
    i = 0
    while i < len(lines):
        if _PYTHON_WARNING_RE.match(lines[i]):
            sys.stderr.write(lines[i])
            if i + 1 < len(lines) and lines[i + 1].startswith("  "):
                sys.stderr.write(lines[i + 1])
                i += 1
        i += 1
