from __future__ import annotations

from dataclasses import dataclass, field

# ---------------------------------------------------------------------------
# Type tags
# ---------------------------------------------------------------------------

T_INT = "int"
T_FLOAT = "float"
T_STR = "str"
T_BOOL = "bool"
T_LIST_INT = "list_int"
T_LIST_STR = "list_str"
T_TUPLE = "tuple"
T_DICT = "dict"
T_SET_INT = "set_int"
T_NONE = "none"
T_ANY = "any"

NUMERIC_TYPES = {T_INT, T_FLOAT}
ALL_TYPES = [
    T_INT,
    T_FLOAT,
    T_STR,
    T_BOOL,
    T_LIST_INT,
    T_LIST_STR,
    T_TUPLE,
    T_DICT,
    T_SET_INT,
    T_NONE,
]


# ---------------------------------------------------------------------------
# Result types
# ---------------------------------------------------------------------------


@dataclass
class FuzzResult:
    """Outcome of a single fuzz iteration."""

    program_id: int
    seed: int
    source: str
    status: str
    cpython_stdout: str = ""
    cpython_stderr: str = ""
    molt_stdout: str = ""
    molt_stderr: str = ""
    error_detail: str = ""
    elapsed_sec: float = 0.0


@dataclass
class FuzzSummary:
    """Aggregate results from a full fuzz run."""

    total: int = 0
    passed: int = 0
    mismatches: int = 0
    build_errors: int = 0
    cpython_errors: int = 0
    molt_run_errors: int = 0
    timeouts: int = 0
    reject_pass: int = 0
    reject_fail: int = 0
    compile_only_ok: int = 0
    compile_only_crash: int = 0
    failures: list[FuzzResult] = field(default_factory=list)
