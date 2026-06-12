#!/usr/bin/env python3
"""Inner-repeat benchmark transform — defeats launch/page-in profile domination.

Why this exists (#76)
---------------------
A one-shot molt benchmark binary spends ~85-92% of its *leaf self-time* in
``_dyld_start`` — process launch plus first-touch page-in of molt's large static
binary (measured directly in the #69 quiet board:
``bench_exception_heavy`` = 91.7% ``_dyld_start``, ``bench_etl_orders`` = 88.5%).
The steady-state Python hot path therefore never dominates a sample leaderboard,
so Rule 1 ("warm-red optimization requires *cycle* attribution") is
UNSATISFIABLE for warm hot paths: the sampler attributes cycles to the launcher,
not to the code we would optimize.

The prior cycle-profile path ran the binary ``repeat_runs`` times back-to-back,
but each run was a SEPARATE process (a shell ``for`` loop re-exec'ing the binary),
so every iteration re-paid ``_dyld_start``. Launch still dominated. The
structural fix is to amortize launch over many iterations of the ACTUAL hot path
*inside ONE process*: wrap the benchmark body in ``for _ in range(N): <body>`` so
``_dyld_start`` is paid exactly once and the steady-state work dominates the
leaf-self-time leaderboard. This mirrors pyperf's ``inner_loops`` model — and we
record ``inner_loops`` in provenance exactly as pyperf does.

Semantics preservation (fail-closed)
------------------------------------
Repeating ``main()`` is only valid if the repeat cannot change observable
behavior. We transform ONLY the canonical molt-bench shape and REFUSE everything
else (never silently emit a non-equivalent variant — the zero-workaround
policy). The required, AST-verified shape is:

  * a single top-level ``def main(...)`` whose call needs no arguments
    (zero required params), and which declares NO ``global`` / ``nonlocal``
    (otherwise a repeat would accumulate into module state and diverge);
  * a ``if __name__ == "__main__":`` guard whose body is EXACTLY one call to
    ``main()`` (extra statements would be multiplied incorrectly by the loop).

Under that shape, each iteration re-initializes all of ``main``'s state locally
(every benchmark in scope builds its working set inside ``main``), so the only
observable effect of N iterations is the same ``print`` N times — identical
per-iteration output. The transform replaces the guard body with the loop and
leaves the rest of the module byte-for-byte intact. A refusal returns a typed
reason so the caller can fall back to the one-shot path with an explicit note,
never a fabricated looped run.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT / "tools") not in sys.path:
    sys.path.insert(0, str(REPO_ROOT / "tools"))

import harness_memory_guard


@dataclass(frozen=True)
class InnerRepeatPlan:
    """The result of analyzing a benchmark for inner-repeat eligibility.

    ``ok`` True  -> ``source`` is the transformed (looped) program text and
                    ``inner_loops`` is N; the program prints the same output N
                    times. ``entry`` is the wrapped entry name (``main``).
    ``ok`` False -> ``reason`` documents WHY the benchmark could not be looped
                    semantics-preservingly; ``source``/``inner_loops`` are None.
    """

    ok: bool
    inner_loops: int | None = None
    source: str | None = None
    entry: str | None = None
    reason: str | None = None


_GUARD_DUNDER = "__name__"
_GUARD_VALUE = "__main__"
_DEFAULT_ENTRY = "main"


def _is_main_guard(node: ast.stmt) -> bool:
    """True iff ``node`` is ``if __name__ == "__main__":`` (either operand order)."""
    if not isinstance(node, ast.If):
        return False
    test = node.test
    if not isinstance(test, ast.Compare):
        return False
    if len(test.ops) != 1 or not isinstance(test.ops[0], ast.Eq):
        return False
    left, right = test.left, test.comparators[0]
    pairs = ((left, right), (right, left))
    for name_node, const_node in pairs:
        if (
            isinstance(name_node, ast.Name)
            and name_node.id == _GUARD_DUNDER
            and isinstance(const_node, ast.Constant)
            and const_node.value == _GUARD_VALUE
        ):
            return True
    return False


def _guard_calls_entry(guard: ast.If, entry: str) -> bool:
    """True iff the guard body is EXACTLY one bare ``entry()`` call (no args)."""
    if len(guard.body) != 1:
        return False
    stmt = guard.body[0]
    if not isinstance(stmt, ast.Expr) or not isinstance(stmt.value, ast.Call):
        return False
    call = stmt.value
    if call.args or call.keywords:
        return False
    return isinstance(call.func, ast.Name) and call.func.id == entry


def _entry_is_repeat_safe(func: ast.FunctionDef) -> tuple[bool, str | None]:
    """The entry is repeat-safe iff it needs no args and mutates no module state.

    * zero REQUIRED positional/kw params (the guard calls it with none), and
    * no ``global`` / ``nonlocal`` anywhere in its body (a repeat would
      accumulate into shared state and the second iteration would diverge).
    """
    a = func.args
    required_pos = len(a.posonlyargs) + len(a.args) - len(a.defaults)
    required_kw = sum(1 for d in a.kw_defaults if d is None)
    if required_pos > 0 or required_kw > 0:
        return False, f"entry {func.name}() takes required arguments; not loop-callable"
    for sub in ast.walk(func):
        if isinstance(sub, (ast.Global, ast.Nonlocal)):
            return (
                False,
                f"entry {func.name}() declares global/nonlocal — a repeat would "
                "mutate module state and diverge (not semantics-preserving)",
            )
    return True, None


def analyze(source: str, *, inner_loops: int, entry: str = _DEFAULT_ENTRY) -> InnerRepeatPlan:
    """Analyze ``source`` and, if eligible, return a looped variant.

    ``inner_loops`` (>=2) is the wrap factor N. ``entry`` is the function the
    ``__main__`` guard must call (default ``main``). On ANY precondition failure
    this returns ``ok=False`` with a documented ``reason`` — never a guessed or
    partially-transformed program.
    """
    if inner_loops < 2:
        return InnerRepeatPlan(
            ok=False, reason=f"inner_loops={inner_loops} < 2 (nothing to amortize)"
        )
    try:
        tree = ast.parse(source)
    except SyntaxError as exc:
        return InnerRepeatPlan(ok=False, reason=f"benchmark does not parse: {exc!r}")

    # Locate the single top-level entry function and the single __main__ guard.
    entry_funcs = [
        n
        for n in tree.body
        if isinstance(n, ast.FunctionDef) and n.name == entry
    ]
    guards = [n for n in tree.body if _is_main_guard(n)]

    if len(entry_funcs) != 1:
        return InnerRepeatPlan(
            ok=False,
            reason=(
                f"expected exactly one top-level def {entry}(); "
                f"found {len(entry_funcs)} — cannot inner-repeat"
            ),
        )
    if len(guards) != 1:
        return InnerRepeatPlan(
            ok=False,
            reason=(
                f'expected exactly one `if __name__ == "__main__":` guard; '
                f"found {len(guards)} — cannot inner-repeat"
            ),
        )
    guard = guards[0]
    if not _guard_calls_entry(guard, entry):
        return InnerRepeatPlan(
            ok=False,
            reason=(
                f"__main__ guard body is not exactly `{entry}()` — refusing to "
                "loop an unrecognized entry shape (would not be semantics-preserving)"
            ),
        )
    safe, why = _entry_is_repeat_safe(entry_funcs[0])
    if not safe:
        return InnerRepeatPlan(ok=False, reason=why)

    # Build `for _ in range(N): main()` and replace the guard body in place.
    loop = ast.For(
        target=ast.Name(id="_", ctx=ast.Store()),
        iter=ast.Call(
            func=ast.Name(id="range", ctx=ast.Load()),
            args=[ast.Constant(value=inner_loops)],
            keywords=[],
        ),
        body=[
            ast.Expr(
                value=ast.Call(
                    func=ast.Name(id=entry, ctx=ast.Load()), args=[], keywords=[]
                )
            )
        ],
        orelse=[],
    )
    guard.body = [loop]
    ast.fix_missing_locations(tree)

    try:
        new_source = ast.unparse(tree)
    except Exception as exc:  # noqa: BLE001 - ast.unparse is total on valid trees
        return InnerRepeatPlan(
            ok=False, reason=f"could not unparse looped variant: {exc!r}"
        )
    # A header makes the artifact self-documenting if it is ever inspected on disk.
    header = (
        f"# AUTO-GENERATED by tools/perf_inner_repeat.py — inner_loops={inner_loops}\n"
        f"# Semantics-preserving: {entry}() repeated N times in ONE process so\n"
        f"# launch/page-in (_dyld_start) amortizes and the hot path dominates the\n"
        f"# CPU sample. Output is the one-shot output printed {inner_loops} times.\n"
    )
    return InnerRepeatPlan(
        ok=True,
        inner_loops=inner_loops,
        source=header + new_source + "\n",
        entry=entry,
    )


__all__ = ["InnerRepeatPlan", "analyze"]


def _self_test() -> int:
    """Tiny self-check: the canonical shape wraps; the unsafe shapes refuse."""
    ok_src = (
        "def main():\n    print(1)\n"
        '\nif __name__ == "__main__":\n    main()\n'
    )
    plan = analyze(ok_src, inner_loops=7)
    assert plan.ok and plan.inner_loops == 7 and "for _ in range(7):" in plan.source
    # Semantics-preserving: looped source prints the one-shot output N times.
    import tempfile as _tf

    one = harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", ok_src],
        prefix="MOLT_BENCH",
        capture_output=True,
        text=True,
        timeout=30.0,
        cwd=REPO_ROOT,
    ).stdout
    with _tf.NamedTemporaryFile("w", suffix=".py", delete=False) as fh:
        fh.write(plan.source)
        p = fh.name
    looped = harness_memory_guard.guarded_completed_process(
        [sys.executable, p],
        prefix="MOLT_BENCH",
        capture_output=True,
        text=True,
        timeout=30.0,
        cwd=REPO_ROOT,
    ).stdout
    import os as _os

    _os.unlink(p)
    assert looped == one * 7, (looped, one)
    # Refusals (fail-closed).
    for bad, needle in (
        ("g=0\ndef main():\n global g\n g+=1\n"
         'if __name__ == "__main__":\n main()\n', "global"),
        ("def main(x):\n pass\n"
         'if __name__ == "__main__":\n main()\n', "argument"),
        ("def main():\n pass\nmain()\n", "guard"),
    ):
        r = analyze(bad, inner_loops=5)
        assert not r.ok and needle in r.reason, (bad, r.reason)
    print("perf_inner_repeat self-test: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(_self_test())
