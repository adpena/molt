#!/usr/bin/env python3
"""Mutation testing tool for the Molt compiler.

Systematically mutates Python source programs at the AST level and verifies
that the Molt compiler correctly propagates each semantic change — i.e., the
compiled output differs from the unmutated original.  If a mutation does NOT
change the output when it should, the compiler may be silently dropping
semantics.

Mutation score = killed / (killed + survived).

Usage:
    python tools/mutation_test.py [--source DIR] [--count N] [--timeout SEC] \
                                  [--verbose] [--build-profile PROFILE]

Exit codes:
    0 — all mutations were killed (score = 100%)
    1 — at least one mutation survived
    2 — infrastructure error (build/setup failure)
"""

import argparse
import ast
import copy
import json
import os
import random
import subprocess
import sys
import tempfile
import textwrap
import time
from dataclasses import dataclass, field
from pathlib import Path


# ---------------------------------------------------------------------------
# Data types
# ---------------------------------------------------------------------------


@dataclass
class MutationResult:
    """Outcome of a single mutation attempt."""

    source_file: str
    operator: str
    description: str
    status: str  # "killed", "survived", "build_fail", "timeout", "skip"
    original_output: str = ""
    mutated_output: str = ""
    error: str | None = None


@dataclass
class MutationReport:
    """Aggregate report across all mutations."""

    results: list[MutationResult] = field(default_factory=list)

    @property
    def killed(self) -> int:
        return sum(1 for r in self.results if r.status == "killed")

    @property
    def survived(self) -> int:
        return sum(1 for r in self.results if r.status == "survived")

    @property
    def build_fail(self) -> int:
        return sum(1 for r in self.results if r.status == "build_fail")

    @property
    def timeout(self) -> int:
        return sum(1 for r in self.results if r.status == "timeout")

    @property
    def skipped(self) -> int:
        return sum(1 for r in self.results if r.status == "skip")

    @property
    def total_scoreable(self) -> int:
        return self.killed + self.survived

    @property
    def score(self) -> float:
        total = self.total_scoreable
        if total == 0:
            return 0.0
        return self.killed / total

    def summary(self) -> str:
        lines = [
            "",
            "=" * 60,
            "  Mutation Testing Report",
            "=" * 60,
            f"  Total mutations applied : {len(self.results)}",
            f"  Killed (detected)       : {self.killed}",
            f"  Survived (undetected)   : {self.survived}",
            f"  Build failures          : {self.build_fail}",
            f"  Timeouts                : {self.timeout}",
            f"  Skipped                 : {self.skipped}",
            "-" * 60,
            f"  Mutation score          : {self.score:.1%}"
            f"  ({self.killed}/{self.total_scoreable})",
            "=" * 60,
            "",
        ]
        return "\n".join(lines)


# ---------------------------------------------------------------------------
# Mutation operators (ast.NodeTransformer subclasses)
# ---------------------------------------------------------------------------


class ArithSwap(ast.NodeTransformer):
    """Replace arithmetic operators: + <-> -, * <-> //, ** <-> %."""

    name = "ArithSwap"
    _swap = {
        ast.Add: ast.Sub,
        ast.Sub: ast.Add,
        ast.Mult: ast.FloorDiv,
        ast.FloorDiv: ast.Mult,
        ast.Mod: ast.Pow,
        ast.Pow: ast.Mod,
    }

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []

    def visit_BinOp(self, node: ast.BinOp) -> ast.BinOp:
        self.generic_visit(node)
        replacement = self._swap.get(type(node.op))
        if replacement is not None:
            old_name = type(node.op).__name__
            new_name = replacement.__name__
            self.mutations.append(f"line {node.lineno}: {old_name} -> {new_name}")
            node.op = replacement()
        return node


class CompSwap(ast.NodeTransformer):
    """Replace comparison operators: == <-> !=, < <-> >=, > <-> <=."""

    name = "CompSwap"
    _swap = {
        ast.Eq: ast.NotEq,
        ast.NotEq: ast.Eq,
        ast.Lt: ast.GtE,
        ast.GtE: ast.Lt,
        ast.Gt: ast.LtE,
        ast.LtE: ast.Gt,
    }

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []

    def visit_Compare(self, node: ast.Compare) -> ast.Compare:
        self.generic_visit(node)
        new_ops = []
        for op in node.ops:
            replacement = self._swap.get(type(op))
            if replacement is not None:
                old_name = type(op).__name__
                new_name = replacement.__name__
                self.mutations.append(f"line {node.lineno}: {old_name} -> {new_name}")
                new_ops.append(replacement())
            else:
                new_ops.append(op)
        node.ops = new_ops
        return node


class ConstPerturb(ast.NodeTransformer):
    """Perturb integer literals by +1 or -1."""

    name = "ConstPerturb"

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []

    def visit_Constant(self, node: ast.Constant) -> ast.Constant:
        self.generic_visit(node)
        if isinstance(node.value, int) and not isinstance(node.value, bool):
            delta = random.choice([-1, 1])
            old_val = node.value
            node.value = old_val + delta
            self.mutations.append(f"line {node.lineno}: {old_val} -> {node.value}")
        return node


class BoolFlip(ast.NodeTransformer):
    """Replace True with False and vice versa."""

    name = "BoolFlip"

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []

    def visit_Constant(self, node: ast.Constant) -> ast.Constant:
        self.generic_visit(node)
        if isinstance(node.value, bool):
            old_val = node.value
            node.value = not old_val
            self.mutations.append(f"line {node.lineno}: {old_val} -> {node.value}")
        return node


class StringMutate(ast.NodeTransformer):
    """Mutate string literals: append a character or remove the last character."""

    name = "StringMutate"

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []

    def visit_Constant(self, node: ast.Constant) -> ast.Constant:
        self.generic_visit(node)
        if isinstance(node.value, str) and node.value:
            # Skip docstrings (first statement in module/function/class bodies)
            # by checking the col_offset — docstrings at col 0 in an Expr are
            # likely module docstrings; we let them through since the mutation
            # is still valid for testing purposes.
            old_val = node.value
            if random.random() < 0.5:
                # Append a character
                node.value = old_val + "X"
                self.mutations.append(f"line {node.lineno}: appended 'X' to string")
            elif len(old_val) > 1:
                # Remove last character
                node.value = old_val[:-1]
                self.mutations.append(
                    f"line {node.lineno}: removed last char from string"
                )
            else:
                # Single-char string: replace with different char
                node.value = chr((ord(old_val) + 1) % 128) if old_val else "X"
                self.mutations.append(
                    f"line {node.lineno}: replaced single-char string"
                )
        return node


class ReturnDrop(ast.NodeTransformer):
    """Remove return statements (replace with pass)."""

    name = "ReturnDrop"

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []

    def visit_Return(self, node: ast.Return) -> ast.Pass:
        self.generic_visit(node)
        self.mutations.append(f"line {node.lineno}: dropped return")
        replacement = ast.Pass()
        return ast.copy_location(replacement, node)


class CondFlip(ast.NodeTransformer):
    """Negate if-conditions by wrapping in `not (...)`."""

    name = "CondFlip"

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []

    def visit_If(self, node: ast.If) -> ast.If:
        self.generic_visit(node)
        self.mutations.append(f"line {node.lineno}: negated if-condition")
        negated = ast.UnaryOp(
            op=ast.Not(),
            operand=node.test,
        )
        ast.copy_location(negated, node.test)
        node.test = negated
        return node


class LogicSwap(ast.NodeTransformer):
    """Replace boolean operators: ``and`` <-> ``or``."""

    name = "LogicSwap"

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []

    def visit_BoolOp(self, node: ast.BoolOp) -> ast.BoolOp:
        self.generic_visit(node)
        if isinstance(node.op, ast.And):
            self.mutations.append(f"line {node.lineno}: And -> Or")
            node.op = ast.Or()
        elif isinstance(node.op, ast.Or):
            self.mutations.append(f"line {node.lineno}: Or -> And")
            node.op = ast.And()
        return node


class ContainerEmpty(ast.NodeTransformer):
    """Replace non-empty list/dict/set literals with empty ones."""

    name = "ContainerEmpty"

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []

    def visit_List(self, node: ast.List) -> ast.List:
        self.generic_visit(node)
        if node.elts and isinstance(node.ctx, ast.Load):
            self.mutations.append(
                f"line {node.lineno}: emptied list ({len(node.elts)} elts)"
            )
            node.elts = []
        return node

    def visit_Dict(self, node: ast.Dict) -> ast.Dict:
        self.generic_visit(node)
        if node.keys:
            self.mutations.append(
                f"line {node.lineno}: emptied dict ({len(node.keys)} keys)"
            )
            node.keys = []
            node.values = []
        return node

    def visit_Set(self, node: ast.Set) -> ast.Set:
        self.generic_visit(node)
        # Can't create empty set literal `{}` (that's a dict), so skip
        # sets with only 1 element to avoid empty set issues.
        if len(node.elts) > 1:
            self.mutations.append(f"line {node.lineno}: reduced set to 1 elt")
            node.elts = [node.elts[0]]
        return node


class AssignDrop(ast.NodeTransformer):
    """Drop assignment statements (replace with pass)."""

    name = "AssignDrop"

    def __init__(self) -> None:
        super().__init__()
        self.mutations: list[str] = []
        self._count = 0

    def visit_Assign(self, node: ast.Assign) -> ast.AST:
        self.generic_visit(node)
        # Only drop every other assignment to avoid breaking too much.
        self._count += 1
        if self._count % 2 == 0:
            self.mutations.append(f"line {node.lineno}: dropped assignment")
            replacement = ast.Pass()
            return ast.copy_location(replacement, node)
        return node


# All operators in application order.
ALL_OPERATORS: list[type[ast.NodeTransformer]] = [
    ArithSwap,
    CompSwap,
    ConstPerturb,
    BoolFlip,
    StringMutate,
    ReturnDrop,
    CondFlip,
    LogicSwap,
    ContainerEmpty,
    AssignDrop,
]


# ---------------------------------------------------------------------------
# Build + run helpers
# ---------------------------------------------------------------------------


def _extract_binary(build_json: dict) -> str | None:
    """Extract the binary path from build JSON, unwrapping data envelope."""
    data = build_json
    if "data" in build_json and isinstance(build_json["data"], dict):
        data = build_json["data"]
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return data[key]
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return data["build"][key]
    return None


def _build_env() -> dict[str, str]:
    """Build environment for Molt CLI invocations."""
    env = os.environ.copy()
    # Ensure src/ is on PYTHONPATH for the molt package.
    existing = env.get("PYTHONPATH", "")
    src_dir = str(Path(__file__).resolve().parents[1] / "src")
    if src_dir not in existing.split(os.pathsep):
        env["PYTHONPATH"] = src_dir + (os.pathsep + existing if existing else "")
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"
    return env


def _build_and_run(
    source_path: str,
    profile: str,
    timeout: int,
    env: dict[str, str],
) -> tuple[str, str, int | None, str | None]:
    """Build a Python file with Molt and run the binary.

    Returns (stdout, stderr, returncode, error_msg).
    error_msg is None on success.
    """
    build_cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "--deterministic",
        "--json",
        source_path,
    ]
    try:
        build_result = subprocess.run(
            build_cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return "", "", None, "build timeout"

    if build_result.returncode != 0:
        stderr_snippet = (build_result.stderr or "")[:500]
        return (
            "",
            "",
            None,
            f"build failed (exit {build_result.returncode}): {stderr_snippet}",
        )

    try:
        build_info = json.loads(build_result.stdout)
    except json.JSONDecodeError:
        return "", "", None, f"invalid build JSON: {build_result.stdout[:300]}"

    binary = _extract_binary(build_info)
    if binary is None:
        return (
            "",
            "",
            None,
            f"no binary in build output (keys: {list(build_info.keys())})",
        )

    if not Path(binary).exists():
        return "", "", None, f"binary not found: {binary}"

    try:
        run_result = subprocess.run(
            [binary],
            capture_output=True,
            text=True,
            env=env,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return "", "", None, "run timeout"

    return run_result.stdout, run_result.stderr, run_result.returncode, None


# ---------------------------------------------------------------------------
# Core mutation logic
# ---------------------------------------------------------------------------


def _apply_mutation(
    source: str,
    operator_cls: type[ast.NodeTransformer],
) -> tuple[str | None, str]:
    """Apply a mutation operator to the source.

    Returns (mutated_source_or_None, description).
    Returns None if the operator did not produce any mutations.
    """
    try:
        tree = ast.parse(source)
    except SyntaxError as e:
        return None, f"parse error: {e}"

    tree_copy = copy.deepcopy(tree)
    transformer = operator_cls()
    mutated_tree = transformer.visit(tree_copy)

    if not transformer.mutations:
        return None, "no applicable nodes"

    ast.fix_missing_locations(mutated_tree)

    try:
        mutated_source = ast.unparse(mutated_tree)
    except Exception as e:
        return None, f"unparse error: {e}"

    # Verify the mutation actually changed the source.
    try:
        original_unparsed = ast.unparse(ast.parse(source))
    except Exception:
        original_unparsed = source

    if mutated_source == original_unparsed:
        return None, "mutation produced identical source"

    desc_parts = [f"{operator_cls.name}: {m}" for m in transformer.mutations]  # type: ignore[attr-defined]
    return mutated_source, "; ".join(desc_parts)


def _mutate_file(
    source_path: str,
    profile: str,
    timeout: int,
    env: dict[str, str],
    verbose: bool,
    count: int,
    report: MutationReport,
) -> None:
    """Apply all mutation operators to a single source file."""
    try:
        source = Path(source_path).read_text()
    except OSError as e:
        report.results.append(
            MutationResult(
                source_file=source_path,
                operator="N/A",
                description=f"cannot read file: {e}",
                status="skip",
            )
        )
        return

    # Skip empty / unparseable files.
    try:
        ast.parse(source)
    except SyntaxError:
        report.results.append(
            MutationResult(
                source_file=source_path,
                operator="N/A",
                description="syntax error in source",
                status="skip",
            )
        )
        return

    # Build and run the original program once.
    orig_stdout, orig_stderr, orig_rc, orig_error = _build_and_run(
        source_path, profile, timeout, env
    )
    if orig_error is not None:
        if verbose:
            print(f"  [skip] original build/run failed: {orig_error}")
        report.results.append(
            MutationResult(
                source_file=source_path,
                operator="N/A",
                description=f"original build/run failed: {orig_error}",
                status="skip",
            )
        )
        return

    if verbose:
        print(f"  original output: {orig_stdout[:80]!r}...")

    # Apply each mutation operator.
    for operator_cls in ALL_OPERATORS:
        op_name = operator_cls.name  # type: ignore[attr-defined]
        mutated_source, description = _apply_mutation(source, operator_cls)

        if mutated_source is None:
            if verbose:
                print(f"  [{op_name}] skip: {description}")
            report.results.append(
                MutationResult(
                    source_file=source_path,
                    operator=op_name,
                    description=description,
                    status="skip",
                )
            )
            continue

        # Write the mutated source to a temp file.
        tmp_fd = None
        tmp_path = None
        try:
            tmp_fd, tmp_path = tempfile.mkstemp(
                suffix=".py", prefix=f"mutation_{op_name}_"
            )
            os.write(tmp_fd, mutated_source.encode("utf-8"))
            os.close(tmp_fd)
            tmp_fd = None

            mut_stdout, mut_stderr, mut_rc, mut_error = _build_and_run(
                tmp_path, profile, timeout, env
            )
        finally:
            if tmp_fd is not None:
                try:
                    os.close(tmp_fd)
                except OSError:
                    pass
            if tmp_path is not None:
                try:
                    os.unlink(tmp_path)
                except OSError:
                    pass

        if mut_error is not None:
            # Build/run failure on the mutant counts as "killed" — the compiler
            # detected the semantic change (e.g., type error, division by zero).
            if verbose:
                print(f"  [{op_name}] killed (build/run error): {mut_error[:80]}")
            report.results.append(
                MutationResult(
                    source_file=source_path,
                    operator=op_name,
                    description=description,
                    status="killed",
                    original_output=orig_stdout,
                    error=mut_error,
                )
            )
            continue

        # Compare outputs. A mutation is "killed" if the output changed.
        output_changed = orig_stdout != mut_stdout or orig_rc != mut_rc

        if output_changed:
            if verbose:
                print(f"  [{op_name}] killed: output changed")
            report.results.append(
                MutationResult(
                    source_file=source_path,
                    operator=op_name,
                    description=description,
                    status="killed",
                    original_output=orig_stdout,
                    mutated_output=mut_stdout,
                )
            )
        else:
            if verbose:
                print(f"  [{op_name}] SURVIVED: output unchanged")
                print(f"           mutation: {description}")
                print(f"           output:   {mut_stdout[:120]!r}")
            report.results.append(
                MutationResult(
                    source_file=source_path,
                    operator=op_name,
                    description=description,
                    status="survived",
                    original_output=orig_stdout,
                    mutated_output=mut_stdout,
                )
            )

        # Respect the per-file mutation count limit.
        scoreable = sum(
            1
            for r in report.results
            if r.source_file == source_path and r.status in ("killed", "survived")
        )
        if count > 0 and scoreable >= count:
            break


# ---------------------------------------------------------------------------
# File discovery
# ---------------------------------------------------------------------------


def _collect_python_files(source_dir: str) -> list[str]:
    """Recursively collect .py files from a directory, sorted for determinism."""
    root = Path(source_dir)
    if root.is_file() and root.suffix == ".py":
        return [str(root)]
    if not root.is_dir():
        return []
    files = sorted(str(p) for p in root.rglob("*.py") if p.is_file())
    return files


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Mutation testing for the Molt compiler.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=textwrap.dedent("""\
            Applies AST-level mutations to Python source programs, compiles
            each mutant with Molt, and checks whether the output changes.

            Mutation operators:
              ArithSwap      Replace + with -, * with //, etc.
              CompSwap       Replace == with !=, < with >=, etc.
              ConstPerturb   Change integer literals by +/-1
              BoolFlip       Replace True with False and vice versa
              StringMutate   Append/remove characters in string literals
              ReturnDrop     Remove return statements
              CondFlip       Negate if-conditions
              LogicSwap      Replace and with or and vice versa
              ContainerEmpty Empty list/dict literals, reduce set literals
              AssignDrop     Drop every other assignment statement
        """),
    )
    default_source = str(
        Path(__file__).resolve().parents[1] / "tests" / "differential" / "basic"
    )
    parser.add_argument(
        "--source",
        default=default_source,
        help="Directory (or single .py file) of test programs to mutate "
        "(default: tests/differential/basic/)",
    )
    parser.add_argument(
        "--count",
        type=int,
        default=0,
        help="Max scoreable mutations per source file (0 = unlimited, "
        "default: 0). Use a small number for quick smoke tests.",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=30,
        help="Timeout in seconds per build+run cycle (default: 30)",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Print per-mutation details",
    )
    parser.add_argument(
        "--build-profile",
        default="dev",
        help="Molt build profile (default: dev)",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=42,
        help="Random seed for reproducible mutations (default: 42)",
    )
    parser.add_argument(
        "--json-out",
        default=None,
        help="Write JSON results to this file",
    )
    args = parser.parse_args()

    random.seed(args.seed)

    source_dir = args.source
    if not Path(source_dir).exists():
        print(f"Error: source path does not exist: {source_dir}", file=sys.stderr)
        return 2

    files = _collect_python_files(source_dir)
    if not files:
        print(f"Error: no .py files found in {source_dir}", file=sys.stderr)
        return 2

    env = _build_env()
    report = MutationReport()

    print(f"Mutation testing: {len(files)} source file(s) from {source_dir}")
    print(
        f"Build profile: {args.build_profile}, timeout: {args.timeout}s, seed: {args.seed}"
    )
    print(f"Operators: {', '.join(op.name for op in ALL_OPERATORS)}")  # type: ignore[attr-defined]
    print()

    t0 = time.monotonic()

    for i, fpath in enumerate(files, 1):
        rel = os.path.relpath(fpath, Path(__file__).resolve().parents[1])
        print(f"[{i}/{len(files)}] {rel}")

        _mutate_file(
            source_path=fpath,
            profile=args.build_profile,
            timeout=args.timeout,
            env=env,
            verbose=args.verbose,
            count=args.count,
            report=report,
        )

    elapsed = time.monotonic() - t0
    print(report.summary())
    print(f"Elapsed: {elapsed:.1f}s")

    # Print survived mutations for visibility.
    survived = [r for r in report.results if r.status == "survived"]
    if survived:
        print("Survived mutations (potential gaps):")
        for r in survived:
            rel = os.path.relpath(r.source_file, Path(__file__).resolve().parents[1])
            print(f"  {rel} [{r.operator}]: {r.description}")
        print()

    # Write JSON report if requested.
    if args.json_out:
        json_data = {
            "score": report.score,
            "killed": report.killed,
            "survived": report.survived,
            "build_fail": report.build_fail,
            "timeout": report.timeout,
            "skipped": report.skipped,
            "total": len(report.results),
            "elapsed_s": round(elapsed, 1),
            "results": [
                {
                    "source_file": r.source_file,
                    "operator": r.operator,
                    "description": r.description,
                    "status": r.status,
                    "error": r.error,
                }
                for r in report.results
            ],
        }
        out_path = Path(args.json_out)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(json_data, indent=2) + "\n")
        print(f"JSON report written to {args.json_out}")

    if report.survived > 0:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
