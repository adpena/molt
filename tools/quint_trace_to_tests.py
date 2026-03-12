"""Generate concrete differential tests from Quint model traces.

Bridges formal verification (Quint models) with empirical testing by:
1. Running ``quint run <model> --max-steps=N`` to produce execution traces
2. Parsing the Quint pretty-printed state sequences
3. Generating concrete Python test programs exercising the properties
   each model verifies

Supported models (under ``formal/quint/``):
  - molt_build_determinism.qnt
  - molt_runtime_determinism.qnt
  - molt_midend_pipeline.qnt
  - molt_calling_convention.qnt
  - molt_cross_version.qnt
  - molt_luau_transpiler.qnt

Usage::

    uv run --python 3.12 python3 tools/quint_trace_to_tests.py \\
        --model formal/quint/molt_build_determinism.qnt \\
        --max-steps 10 --count 5 \\
        --output-dir tests/differential/generated

    # JSON report
    uv run --python 3.12 python3 tools/quint_trace_to_tests.py \\
        --model formal/quint/molt_build_determinism.qnt --json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path


# ---------------------------------------------------------------------------
# Quint trace parser
# ---------------------------------------------------------------------------

_STATE_HEADER_RE = re.compile(r"^\[State (\d+)\]")


def _run_quint_trace(
    model_path: str,
    *,
    max_steps: int = 10,
    invariant: str | None = None,
) -> str:
    """Run ``quint run`` and return raw stdout."""
    cmd: list[str] = [
        "quint",
        "run",
        model_path,
        f"--max-steps={max_steps}",
    ]
    if invariant:
        cmd += [f"--invariant={invariant}"]

    result = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=120,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"quint run failed (rc={result.returncode}):\n"
            f"  cmd: {' '.join(cmd)}\n"
            f"  stderr: {result.stderr.strip()}"
        )
    return result.stdout


def parse_quint_trace(raw: str) -> list[dict[str, str]]:
    """Parse Quint pretty-printed trace into a list of state dicts.

    Each state is a dict mapping variable names to their string
    representations.  We do *not* attempt full Quint value parsing —
    generators only need high-level structural info (set membership,
    field values, list lengths).
    """
    states: list[dict[str, str]] = []
    current_block: list[str] = []
    in_state = False

    for line in raw.splitlines():
        header = _STATE_HEADER_RE.match(line.strip())
        if header:
            if in_state and current_block:
                states.append(_parse_state_block(current_block))
            current_block = []
            in_state = True
            continue
        if in_state:
            current_block.append(line)

    if in_state and current_block:
        states.append(_parse_state_block(current_block))

    return states


def _parse_state_block(lines: list[str]) -> dict[str, str]:
    """Extract top-level key-value pairs from a Quint state block.

    The block is wrapped in ``{ ... }``.  Top-level keys sit at indent
    level 2 and are followed by ``:`` then a value (which may span
    multiple lines).
    """
    joined = "\n".join(lines)
    # Strip outer braces
    joined = joined.strip()
    if joined.startswith("{"):
        joined = joined[1:]
    if joined.rstrip().endswith("}"):
        joined = joined[: joined.rstrip().rfind("}")]

    result: dict[str, str] = {}
    current_key: str | None = None
    current_value_lines: list[str] = []

    _KV_RE = re.compile(r"^\s{2}(\w[\w_]*)\s*:\s*(.*)")

    for line in joined.split("\n"):
        m = _KV_RE.match(line)
        if m:
            if current_key is not None:
                result[current_key] = "\n".join(current_value_lines).strip()
            current_key = m.group(1)
            current_value_lines = [m.group(2)]
        else:
            current_value_lines.append(line)

    if current_key is not None:
        result[current_key] = "\n".join(current_value_lines).strip()

    return result


# ---------------------------------------------------------------------------
# Utility helpers for extracting values from Quint representations
# ---------------------------------------------------------------------------

_SET_RE = re.compile(r"Set\(([^)]*)\)")
_LIST_RE = re.compile(r"\[([^\]]*)\]")


def _extract_set_ints(val: str) -> list[int]:
    """Extract integer members from a Quint ``Set(1, 2, 3)``."""
    m = _SET_RE.search(val)
    if not m:
        return []
    inner = m.group(1).strip()
    if not inner:
        return []
    return [int(x.strip()) for x in inner.split(",") if x.strip().lstrip("-").isdigit()]


def _extract_list_items(val: str) -> list[str]:
    """Extract items from a Quint ``[a, b, c]``."""
    m = _LIST_RE.search(val)
    if not m:
        return []
    inner = m.group(1).strip()
    if not inner:
        return []
    return [x.strip() for x in inner.split(",") if x.strip()]


def _extract_set_strings(val: str) -> list[str]:
    """Extract string members from a Quint ``Set("a", "b")``."""
    return re.findall(r'"([^"]*)"', val)


def _trace_fingerprint(trace: list[dict[str, str]]) -> str:
    """Compute a short fingerprint for a trace to deduplicate."""
    h = hashlib.sha256()
    for s in trace:
        for k in sorted(s.keys()):
            h.update(f"{k}={s[k]}".encode())
    return h.hexdigest()[:12]


# ---------------------------------------------------------------------------
# Model name detection
# ---------------------------------------------------------------------------

_MODEL_INVARIANTS: dict[str, str] = {
    "molt_build_determinism": "Inv",
    "molt_runtime_determinism": "Inv",
    "molt_midend_pipeline": "inv",
    "molt_calling_convention": "Inv",
    "molt_cross_version": "Inv",
    "molt_luau_transpiler": "Inv",
}


def _detect_model_name(model_path: str) -> str:
    stem = Path(model_path).stem
    for known in _MODEL_INVARIANTS:
        if stem == known:
            return known
    raise ValueError(f"Unknown model: {stem}")


# ---------------------------------------------------------------------------
# Test generators — one per model type
# ---------------------------------------------------------------------------


def _gen_build_determinism(
    trace: list[dict[str, str]],
    trace_id: int,
    fp: str,
) -> str:
    """Generate a program exercising build-order-sensitive hash paths.

    The build_determinism model shows modules compiled in nondeterministic
    order yet producing the same final artifact digest.  The concrete test
    creates data structures whose iteration order could vary with hash
    seeds, then verifies deterministic output.
    """
    # Extract the build order from the trace: which modules were done at each
    # step.
    build_order: list[int] = []
    prev_done: set[int] = set()
    for state in trace:
        if "done" in state:
            cur_done = set(_extract_set_ints(state["done"]))
            new = cur_done - prev_done
            build_order.extend(sorted(new))
            prev_done = cur_done

    n_modules = len(build_order) if build_order else 4
    # Create a Python program with multiple dicts that must merge
    # deterministically regardless of hash ordering.
    lines = [
        f'"""Purpose: model-based build determinism test (trace {trace_id}, fp {fp}).',
        "",
        "Exercises hash-order-sensitive dict merge paths and verifies",
        'deterministic output regardless of iteration order."""',
        "",
    ]

    # Build N dicts simulating module compilation artifacts
    for i in range(n_modules):
        seed = (i * 1103515245 + 12345) % 100
        lines.append(f"mod_{i} = {{'digest': {seed}, 'name': 'mod_{i}'}}")

    lines.append("")
    lines.append("# Merge all modules — order must not matter")
    lines.append("merged = {}")
    # Use the trace build order to merge
    order = build_order if build_order else list(range(n_modules))
    for idx in order:
        if idx < n_modules:
            lines.append(f"merged[mod_{idx}['name']] = mod_{idx}['digest']")

    lines.append("")
    lines.append("# Canonical output: sorted by key for determinism")
    lines.append("for k in sorted(merged.keys()):")
    lines.append("    print(f'{k}={merged[k]}')")
    lines.append("")

    # Also test set operations (order-independent)
    lines.append("# Set union (order-independent)")
    lines.append("all_digests = set()")
    for i in range(n_modules):
        lines.append(f"all_digests.add(mod_{i}['digest'])")
    lines.append("print(f'total_unique={len(all_digests)}')")
    lines.append("print(f'digest_sum={sum(sorted(all_digests))}')")

    return "\n".join(lines) + "\n"


def _gen_runtime_determinism(
    trace: list[dict[str, str]],
    trace_id: int,
    fp: str,
) -> str:
    """Generate a program with nondeterminism sources that Molt must pin.

    The runtime_determinism model checks that task execution order does not
    affect final results.  The test creates dict/set iteration and verifies
    deterministic sorted output.
    """
    # Extract task completion order from trace
    exec_order: list[int] = []
    for state in trace:
        if "execOrder" in state:
            items = _extract_list_items(state["execOrder"])
            if items:
                exec_order = [int(x) for x in items if x.isdigit()]

    n_tasks = max(len(exec_order), 4)
    task_results = {i: i * 7 + 3 for i in range(n_tasks)}

    lines = [
        f'"""Purpose: model-based runtime determinism test '
        f"(trace {trace_id}, fp {fp}).",
        "",
        "Verifies that dict/set iteration, sorted output, and",
        'task-result computation are deterministic."""',
        "",
        "# Simulate task results computed in arbitrary order",
        "results = {}",
    ]

    # Insert tasks in the observed execution order
    order = exec_order if exec_order else list(range(n_tasks))
    for t in order:
        if t < n_tasks:
            lines.append(f"results[{t}] = {task_results.get(t, t * 7 + 3)}")

    lines.append("")
    lines.append("# Deterministic output: sorted by task ID")
    lines.append("for task_id in sorted(results.keys()):")
    lines.append("    print(f'task_{task_id}={results[task_id]}')")
    lines.append("")

    # Set ordering test
    lines.append("# Set ordering must be deterministic when sorted")
    lines.append(f"task_set = set(range({n_tasks}))")
    lines.append("print(f'tasks={sorted(task_set)}')")
    lines.append("")

    # Dict comprehension order
    lines.append("# Dict comprehension determinism")
    lines.append(f"mapped = {{k: k * 7 + 3 for k in range({n_tasks})}}")
    lines.append("for k in sorted(mapped):")
    lines.append("    print(f'mapped_{k}={mapped[k]}')")

    return "\n".join(lines) + "\n"


def _gen_midend_pipeline(
    trace: list[dict[str, str]],
    trace_id: int,
    fp: str,
) -> str:
    """Generate a program with optimization opportunities.

    The midend_pipeline model verifies that the optimization pass pipeline
    (ConstFold, DCE, SCCP, CSE, LICM, etc.) produces correct output
    regardless of pass ordering.  The test creates code with dead branches,
    constant expressions, and common subexpressions.
    """
    # Extract pass history from trace
    pass_history: list[str] = []
    final_size = 100
    for state in trace:
        if "ir" in state:
            ir_val = state["ir"]
            history_match = re.search(r"history:\s*\[([^\]]*)\]", ir_val)
            if history_match:
                items = [
                    x.strip() for x in history_match.group(1).split(",") if x.strip()
                ]
                if items:
                    pass_history = items
            size_match = re.search(r"size:\s*(\d+)", ir_val)
            if size_match:
                final_size = int(size_match.group(1))

    lines = [
        f'"""Purpose: model-based midend pipeline test (trace {trace_id}, fp {fp}).',
        "",
        "Exercises optimization opportunities: constant folding, dead code",
        "elimination, common subexpression elimination, and loop-invariant",
        f"code motion.  Trace pass history: {pass_history}.",
        f'Final IR size proxy: {final_size}."""',
        "",
    ]

    # Constant folding opportunities
    if "ConstFold" in pass_history or not pass_history:
        lines.extend(
            [
                "# Constant folding",
                "x = 2 + 3 * 4",
                "y = 100 // 7",
                "z = (x + y) * 2",
                "print(f'const_fold: x={x} y={y} z={z}')",
                "",
            ]
        )

    # Dead code elimination
    if "DCE" in pass_history or not pass_history:
        lines.extend(
            [
                "# Dead code paths (should be eliminated but not affect output)",
                "def compute(n):",
                "    result = 0",
                "    for i in range(n):",
                "        result += i",
                "        unused = i * 999  # dead store",
                "    return result",
                "",
                "print(f'dce: {compute(10)}')",
                "",
            ]
        )

    # Common subexpression elimination
    if "CSE" in pass_history or not pass_history:
        lines.extend(
            [
                "# Common subexpressions",
                "a = 17",
                "b = 23",
                "c1 = a * b + a",
                "c2 = a * b + a  # same expression",
                "print(f'cse: c1={c1} c2={c2} same={c1 == c2}')",
                "",
            ]
        )

    # Loop-invariant code motion
    if "LICM" in pass_history or not pass_history:
        lines.extend(
            [
                "# Loop-invariant computation",
                "total = 0",
                "base = 42",
                "for i in range(5):",
                "    invariant = base * 3  # loop-invariant",
                "    total += i + invariant",
                "print(f'licm: total={total}')",
                "",
            ]
        )

    # SCCP (sparse conditional constant propagation)
    if "SCCP" in pass_history or not pass_history:
        lines.extend(
            [
                "# Conditional constant propagation",
                "flag = True",
                "if flag:",
                "    val = 42",
                "else:",
                "    val = 0",
                "print(f'sccp: val={val}')",
            ]
        )

    return "\n".join(lines) + "\n"


def _gen_calling_convention(
    trace: list[dict[str, str]],
    trace_id: int,
    fp: str,
) -> str:
    """Generate programs testing function call patterns.

    The calling_convention model verifies that builtin, user-defined, and
    closure functions resolve correctly via the args-tuple convention.
    """
    # Extract which builtins and call patterns appear in the trace
    registered_ids: list[int] = []
    has_args_tuple_call = False
    has_direct_call = False

    for state in trace:
        if "registeredIds" in state:
            registered_ids = _extract_set_ints(state["registeredIds"])
        if "callSites" in state:
            val = state["callSites"]
            if "argsAsTuple: true" in val:
                has_args_tuple_call = True
            if "argsAsTuple: false" in val:
                has_direct_call = True

    # Map model builtin IDs to Python builtins
    builtin_map = {0: "max", 1: "min", 2: "round", 3: "len", 4: "abs"}

    lines = [
        f'"""Purpose: model-based calling convention test (trace {trace_id}, fp {fp}).',
        "",
        "Verifies builtin, user-defined, and closure function calls work",
        f"correctly.  Registered builtins: {registered_ids}.",
        f"Has args-tuple calls: {has_args_tuple_call}.",
        f'Has direct calls: {has_direct_call}."""',
        "",
    ]

    # Test direct builtin calls
    lines.append("# Direct builtin calls")
    for bid in registered_ids:
        name = builtin_map.get(bid)
        if name == "max":
            lines.append("print(f'max={max(1, 5, 3)}')")
        elif name == "min":
            lines.append("print(f'min={min(1, 5, 3)}')")
        elif name == "round":
            lines.append("print(f'round={round(3.7)}')")
        elif name == "len":
            lines.append("print(f'len={len([1, 2, 3])}')")
        elif name == "abs":
            lines.append("print(f'abs={abs(-42)}')")

    lines.append("")

    # Test builtins passed as first-class references
    if has_args_tuple_call or registered_ids:
        lines.append("# Builtins as first-class callable references")
        for bid in registered_ids:
            name = builtin_map.get(bid)
            if name:
                lines.append(f"fn_{name} = {name}")
        for bid in registered_ids:
            name = builtin_map.get(bid)
            if name == "max":
                lines.append("print(f'ref_max={fn_max(10, 20)}')")
            elif name == "min":
                lines.append("print(f'ref_min={fn_min(10, 20)}')")
            elif name == "abs":
                lines.append("print(f'ref_abs={fn_abs(-7)}')")
        lines.append("")

    # Test user-defined functions with various calling patterns
    lines.extend(
        [
            "# User-defined function with multiple call patterns",
            "def add(a, b):",
            "    return a + b",
            "",
            "print(f'direct={add(3, 4)}')",
            "print(f'kwargs={add(a=3, b=4)}')",
            "print(f'mixed={add(3, b=4)}')",
            "",
        ]
    )

    # Test closure captures
    lines.extend(
        [
            "# Closure function reference",
            "def make_adder(n):",
            "    def adder(x):",
            "        return x + n",
            "    return adder",
            "",
            "add5 = make_adder(5)",
            "print(f'closure={add5(10)}')",
            "",
        ]
    )

    # Test *args/**kwargs
    if has_args_tuple_call:
        lines.extend(
            [
                "# Args-tuple calling convention",
                "def variadic(*args, **kwargs):",
                "    return (args, tuple(sorted(kwargs.items())))",
                "",
                "print(f'variadic={variadic(1, 2, 3, x=4, y=5)}')",
                "",
            ]
        )

    # Apply via tuple unpacking (mirrors the args-tuple convention)
    lines.extend(
        [
            "# apply-style call (tuple unpacking)",
            "args = (1, 2)",
            "print(f'apply={add(*args)}')",
        ]
    )

    return "\n".join(lines) + "\n"


def _gen_cross_version(
    trace: list[dict[str, str]],
    trace_id: int,
    fp: str,
) -> str:
    """Generate programs testing version-gated features.

    The cross_version model verifies that Molt correctly gates features
    by Python version.  Tests exercise version-portable patterns.
    """
    # Extract compilation units and features from trace
    target_versions: list[int] = []
    used_feature_ids: list[int] = []

    for state in trace:
        if "units" in state:
            val = state["units"]
            for m in re.finditer(r"targetVersion:\s*(\d+)", val):
                v = int(m.group(1))
                if v not in target_versions:
                    target_versions.append(v)
            for m in re.finditer(r"usedFeatures:\s*Set\(([^)]*)\)", val):
                for fid in m.group(1).split(","):
                    fid = fid.strip()
                    if fid.isdigit() and int(fid) not in used_feature_ids:
                        used_feature_ids.append(int(fid))

    if not target_versions:
        target_versions = [12]

    # Feature catalog from the model
    feature_names = {
        0: "deferred_annotations",
        1: "locals_snapshot",
        2: "glob_translation",
        3: "type_statement",
        4: "deprecated_module",
        5: "type_param_default",
    }

    lines = [
        f'"""Purpose: model-based cross-version test (trace {trace_id}, fp {fp}).',
        "",
        f"Target versions: {[f'3.{v}' for v in target_versions]}.",
        f"Features exercised: "
        f"{[feature_names.get(f, f'f{f}') for f in used_feature_ids]}.",
        "",
        "Tests version-portable patterns that must work across all",
        'supported Python versions (3.12+)."""',
        "",
        "import sys",
        "",
        "ver = sys.version_info[:2]",
        "print(f'python_version={ver[0]}.{ver[1]}')",
        "",
    ]

    # Type statement (PEP 695 — available 3.12+, so always safe)
    if 3 in used_feature_ids or not used_feature_ids:
        lines.extend(
            [
                "# type statement (PEP 695) — available on all Molt targets",
                "# Using traditional TypeAlias for compatibility",
                "from typing import TypeAlias",
                "Vector: TypeAlias = list[float]",
                "v: Vector = [1.0, 2.0, 3.0]",
                "print(f'type_alias={v}')",
                "",
            ]
        )

    # locals() behavior differences
    if 1 in used_feature_ids or not used_feature_ids:
        lines.extend(
            [
                "# locals() snapshot semantics",
                "def test_locals():",
                "    x = 42",
                "    loc = locals()",
                "    print(f'locals_x={loc[\"x\"]}')",
                "",
                "test_locals()",
                "",
            ]
        )

    # Version-gated conditional
    lines.extend(
        [
            "# Version-gated behavior pattern",
            "if ver >= (3, 13):",
            "    print('version_gate=new_path')",
            "else:",
            "    print('version_gate=legacy_path')",
            "",
        ]
    )

    # Deterministic output regardless of version
    lines.extend(
        [
            "# Cross-version deterministic computation",
            "data = [3, 1, 4, 1, 5, 9, 2, 6]",
            "print(f'sorted={sorted(data)}')",
            "print(f'sum={sum(data)}')",
        ]
    )

    return "\n".join(lines) + "\n"


def _gen_luau_transpiler(
    trace: list[dict[str, str]],
    trace_id: int,
    fp: str,
) -> str:
    """Generate programs testing Luau transpiler concerns.

    The luau_transpiler model verifies index adjustment (0->1 based),
    builtin resolution, module imports, and variable declarations.
    """
    # Extract discovered IR elements from the trace
    ir_vars: list[str] = []
    ir_builtins: list[str] = []
    ir_modules: list[str] = []
    index_accesses: list[tuple[str, int]] = []

    for state in trace:
        if "irVars" in state:
            ir_vars = _extract_set_strings(state["irVars"]) or ir_vars
        if "irBuiltins" in state:
            ir_builtins = _extract_set_strings(state["irBuiltins"]) or ir_builtins
        if "irModules" in state:
            ir_modules = _extract_set_strings(state["irModules"]) or ir_modules
        if "indexAccesses" in state:
            for m in re.finditer(r"original_index:\s*(\d+)", state["indexAccesses"]):
                idx = int(m.group(1))
                var = ir_vars[0] if ir_vars else "x"
                index_accesses.append((var, idx))

    lines = [
        f'"""Purpose: model-based Luau transpiler test (trace {trace_id}, fp {fp}).',
        "",
        "Exercises patterns the Luau backend must handle correctly:",
        f"  Variables: {ir_vars or ['x', 'y']}",
        f"  Builtins: {ir_builtins or ['len', 'range']}",
        f"  Modules: {ir_modules or []}",
        f'  Index accesses: {len(index_accesses)} operations."""',
        "",
    ]

    # Variable declaration and usage
    var_names = ir_vars if ir_vars else ["x", "y", "z"]
    for i, var in enumerate(var_names):
        lines.append(f"{var} = [10, 20, 30, 40, 50]")

    lines.append("")

    # Index accesses (tests 0-based indexing correctness)
    lines.append("# Index access patterns (0-based)")
    for var in var_names[:2]:
        for idx in range(min(4, 5)):
            lines.append(f"print(f'{var}[{idx}]={{{var}[{idx}]}}')")

    lines.append("")

    # Negative indexing
    lines.append("# Negative indexing")
    var0 = var_names[0]
    lines.append(f"print(f'{var0}[-1]={{{var0}[-1]}}')")
    lines.append(f"print(f'{var0}[-2]={{{var0}[-2]}}')")
    lines.append("")

    # Builtin function usage
    builtins_to_test = ir_builtins if ir_builtins else ["len", "range"]
    lines.append("# Builtin function calls")
    for b in builtins_to_test:
        if b == "len":
            lines.append(f"print(f'len={len.__name__}({var0})={{len({var0})}}')")
        elif b == "range":
            lines.append("print(f'range={list(range(5))}')")
        elif b == "print":
            lines.append("print('print=ok')")
        elif b == "type":
            lines.append(f"print(f'type={{type({var0}).__name__}}')")
        elif b == "int":
            lines.append("print(f'int={int(3.14)}')")

    lines.append("")

    # Slice operations (also index-related)
    lines.append("# Slice operations")
    lines.append(f"print(f'slice={{{var0}[1:3]}}')")
    lines.append(f"print(f'slice_step={{{var0}[::2]}}')")

    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# Generator dispatch
# ---------------------------------------------------------------------------

_GENERATORS: dict[str, type] = {}  # unused, dispatch via dict below

_GEN_DISPATCH = {
    "molt_build_determinism": _gen_build_determinism,
    "molt_runtime_determinism": _gen_runtime_determinism,
    "molt_midend_pipeline": _gen_midend_pipeline,
    "molt_calling_convention": _gen_calling_convention,
    "molt_cross_version": _gen_cross_version,
    "molt_luau_transpiler": _gen_luau_transpiler,
}


# ---------------------------------------------------------------------------
# Main orchestration
# ---------------------------------------------------------------------------


def generate_tests(
    model_path: str,
    *,
    max_steps: int = 10,
    count: int = 5,
    output_dir: str = "tests/differential/generated",
) -> list[dict[str, str]]:
    """Generate test files from Quint model traces.

    Returns a list of dicts with keys ``path`` and ``fingerprint``.
    """
    model_name = _detect_model_name(model_path)
    invariant = _MODEL_INVARIANTS.get(model_name)
    gen_fn = _GEN_DISPATCH.get(model_name)
    if gen_fn is None:
        raise ValueError(f"No generator for model: {model_name}")

    out_path = Path(output_dir)
    out_path.mkdir(parents=True, exist_ok=True)

    generated: list[dict[str, str]] = []
    seen_fps: set[str] = set()
    attempts = 0
    max_attempts = count * 5  # allow extra attempts for dedup

    while len(generated) < count and attempts < max_attempts:
        attempts += 1
        try:
            raw = _run_quint_trace(
                model_path,
                max_steps=max_steps,
                invariant=invariant,
            )
        except RuntimeError as e:
            print(
                f"WARNING: quint run failed (attempt {attempts}): {e}", file=sys.stderr
            )
            continue

        trace = parse_quint_trace(raw)
        if not trace:
            continue

        fp = _trace_fingerprint(trace)
        if fp in seen_fps:
            continue
        seen_fps.add(fp)

        trace_id = len(generated)
        source = gen_fn(trace, trace_id, fp)

        # Derive filename from model name + trace index
        short_model = model_name.replace("molt_", "")
        filename = f"mbt_{short_model}_{trace_id:03d}_{fp}.py"
        file_path = out_path / filename

        file_path.write_text(source, encoding="utf-8")
        generated.append({"path": str(file_path), "fingerprint": fp})

    return generated


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate differential tests from Quint model traces.",
    )
    parser.add_argument(
        "--model",
        required=True,
        help="Path to the Quint model file (.qnt)",
    )
    parser.add_argument(
        "--max-steps",
        type=int,
        default=10,
        help="Maximum trace length (default: 10)",
    )
    parser.add_argument(
        "--count",
        type=int,
        default=5,
        help="Number of traces to generate (default: 5)",
    )
    parser.add_argument(
        "--output-dir",
        default="tests/differential/generated",
        help="Output directory for generated tests",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_report",
        help="Print JSON report of generated tests",
    )

    args = parser.parse_args()

    results = generate_tests(
        args.model,
        max_steps=args.max_steps,
        count=args.count,
        output_dir=args.output_dir,
    )

    if args.json_report:
        report = {
            "model": args.model,
            "max_steps": args.max_steps,
            "count_requested": args.count,
            "count_generated": len(results),
            "output_dir": args.output_dir,
            "tests": results,
        }
        print(json.dumps(report, indent=2))
    else:
        print(f"Generated {len(results)} tests from {args.model}:")
        for r in results:
            print(f"  {r['path']} (fp: {r['fingerprint']})")


if __name__ == "__main__":
    main()
