#!/usr/bin/env python3
"""Z3-based verification of TIR optimization passes.

Takes a TIR function (JSON format, SimpleIR schema) as input, encodes
integer operations as Z3 formulas, and checks for redundant operations
whose result is always equal to a simpler expression.

This is a TESTING TOOL, not a production pass.  It reports missed
optimization opportunities so a human or agent can add the corresponding
pass to the TIR pipeline.

Usage:
    python3 tools/z3_pass_verify.py <tir.json>
    python3 tools/z3_pass_verify.py --stdin < tir.json

Requires: pip install z3-solver
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass, field
from typing import Any

try:
    from z3 import (
        BitVec,
        BitVecVal,
        Context,
        Solver,
        UDiv,
        URem,
        unsat,
    )
except ImportError:
    print("Install z3-solver: pip install z3-solver", file=sys.stderr)
    sys.exit(1)


# ---------------------------------------------------------------------------
# Data model
# ---------------------------------------------------------------------------


@dataclass
class Op:
    kind: str
    out: str | None = None
    args: list[str] | None = None
    value: int | None = None
    f_value: float | None = None
    s_value: str | None = None
    fast_int: bool | None = None
    raw_int: bool | None = None


@dataclass
class Function:
    name: str
    params: list[str]
    ops: list[Op]


@dataclass
class Report:
    function: str
    findings: list[Finding] = field(default_factory=list)


@dataclass
class Finding:
    op_index: int
    kind: str
    out: str
    category: str
    message: str


# ---------------------------------------------------------------------------
# Z3 encoding
# ---------------------------------------------------------------------------

# Use 64-bit bitvectors to model Python int semantics (within the i64 range
# that the TIR fast-int path uses).
BV_WIDTH = 64


def _parse_op(raw: dict[str, Any]) -> Op:
    return Op(
        kind=raw["kind"],
        out=raw.get("out"),
        args=raw.get("args"),
        value=raw.get("value"),
        f_value=raw.get("f_value"),
        s_value=raw.get("s_value"),
        fast_int=raw.get("fast_int"),
        raw_int=raw.get("raw_int"),
    )


def _parse_function(raw: dict[str, Any]) -> Function:
    return Function(
        name=raw["name"],
        params=raw.get("params", []),
        ops=[_parse_op(op) for op in raw.get("ops", [])],
    )


def _is_integer_op(op: Op) -> bool:
    """Return True if *op* is a pure integer computation suitable for Z3 encoding."""
    return op.kind in _INTEGER_OPS


_INTEGER_OPS = {
    "const",
    "const_int",
    "add",
    "sub",
    "mul",
    "floor_div",
    "mod",
    "neg",
    "bitwise_and",
    "bitwise_or",
    "bitwise_xor",
    "bitwise_not",
    "lshift",
    "rshift",
    "lt",
    "le",
    "gt",
    "ge",
    "eq",
    "ne",
}

_CMP_OPS = {"lt", "le", "gt", "ge", "eq", "ne"}


class Z3Encoder:
    """Encode a sequence of integer TIR ops into Z3 bitvector formulas."""

    def __init__(self) -> None:
        self.ctx = Context()
        self.env: dict[str, Any] = {}  # SSA name -> Z3 BitVec expression
        self._counter = 0

    def _fresh(self, prefix: str = "x") -> Any:
        self._counter += 1
        return BitVec(f"{prefix}_{self._counter}", BV_WIDTH, ctx=self.ctx)

    def _get(self, name: str) -> Any:
        if name not in self.env:
            self.env[name] = self._fresh(name)
        return self.env[name]

    def _const(self, val: int) -> Any:
        return BitVecVal(val, BV_WIDTH, ctx=self.ctx)

    def encode_op(self, op: Op) -> bool:
        """Encode *op* into the environment.  Returns False if not encodable."""
        if op.out is None:
            return False

        if op.kind in ("const", "const_int"):
            if op.value is not None:
                self.env[op.out] = self._const(op.value)
                return True
            return False

        args = op.args or []
        if op.kind in (
            "add",
            "sub",
            "mul",
            "floor_div",
            "mod",
            "bitwise_and",
            "bitwise_or",
            "bitwise_xor",
            "lshift",
            "rshift",
        ):
            if len(args) < 2:
                return False
            lhs = self._get(args[0])
            rhs = self._get(args[1])
            result = {
                "add": lhs + rhs,
                "sub": lhs - rhs,
                "mul": lhs * rhs,
                "floor_div": UDiv(lhs, rhs),
                "mod": URem(lhs, rhs),
                "bitwise_and": lhs & rhs,
                "bitwise_or": lhs | rhs,
                "bitwise_xor": lhs ^ rhs,
                "lshift": lhs << rhs,
                "rshift": lhs >> rhs,  # arithmetic shift
            }[op.kind]
            self.env[op.out] = result
            return True

        if op.kind == "neg":
            if len(args) < 1:
                return False
            self.env[op.out] = -self._get(args[0])
            return True

        if op.kind == "bitwise_not":
            if len(args) < 1:
                return False
            self.env[op.out] = ~self._get(args[0])
            return True

        if op.kind in _CMP_OPS:
            if len(args) < 2:
                return False
            lhs = self._get(args[0])
            rhs = self._get(args[1])
            # Comparisons produce a 1-bit result; extend to BV_WIDTH
            from z3 import If

            cmp_expr = {
                "lt": lhs < rhs,
                "le": lhs <= rhs,
                "gt": lhs > rhs,
                "ge": lhs >= rhs,
                "eq": lhs == rhs,
                "ne": lhs != rhs,
            }[op.kind]
            self.env[op.out] = If(cmp_expr, self._const(1), self._const(0))
            return True

        return False


# ---------------------------------------------------------------------------
# Redundancy checks
# ---------------------------------------------------------------------------


def _check_add_zero(encoder: Z3Encoder, op: Op, idx: int) -> Finding | None:
    """Detect x + 0 or 0 + x."""
    if op.kind != "add" or not op.args or len(op.args) < 2 or op.out is None:
        return None
    lhs_name, rhs_name = op.args[0], op.args[1]
    lhs = encoder.env.get(lhs_name)
    rhs = encoder.env.get(rhs_name)
    if lhs is None or rhs is None:
        return None
    zero = encoder._const(0)
    s = Solver(ctx=encoder.ctx)
    # Check if rhs is always zero
    s.push()
    s.add(rhs != zero)
    if s.check() == unsat:
        s.pop()
        return Finding(
            op_index=idx,
            kind=op.kind,
            out=op.out,
            category="add-zero",
            message=f"{op.out} = {lhs_name} + {rhs_name}: rhs is always 0, result equals {lhs_name}",
        )
    s.pop()
    # Check if lhs is always zero
    s.push()
    s.add(lhs != zero)
    if s.check() == unsat:
        s.pop()
        return Finding(
            op_index=idx,
            kind=op.kind,
            out=op.out,
            category="add-zero",
            message=f"{op.out} = {lhs_name} + {rhs_name}: lhs is always 0, result equals {rhs_name}",
        )
    s.pop()
    return None


def _check_mul_one(encoder: Z3Encoder, op: Op, idx: int) -> Finding | None:
    """Detect x * 1 or 1 * x."""
    if op.kind != "mul" or not op.args or len(op.args) < 2 or op.out is None:
        return None
    lhs_name, rhs_name = op.args[0], op.args[1]
    lhs = encoder.env.get(lhs_name)
    rhs = encoder.env.get(rhs_name)
    if lhs is None or rhs is None:
        return None
    one = encoder._const(1)
    s = Solver(ctx=encoder.ctx)
    # Check if rhs is always one
    s.push()
    s.add(rhs != one)
    if s.check() == unsat:
        s.pop()
        return Finding(
            op_index=idx,
            kind=op.kind,
            out=op.out,
            category="mul-one",
            message=f"{op.out} = {lhs_name} * {rhs_name}: rhs is always 1, result equals {lhs_name}",
        )
    s.pop()
    # Check if lhs is always one
    s.push()
    s.add(lhs != one)
    if s.check() == unsat:
        s.pop()
        return Finding(
            op_index=idx,
            kind=op.kind,
            out=op.out,
            category="mul-one",
            message=f"{op.out} = {lhs_name} * {rhs_name}: lhs is always 1, result equals {rhs_name}",
        )
    s.pop()
    return None


def _check_mul_zero(encoder: Z3Encoder, op: Op, idx: int) -> Finding | None:
    """Detect x * 0 or 0 * x."""
    if op.kind != "mul" or not op.args or len(op.args) < 2 or op.out is None:
        return None
    lhs_name, rhs_name = op.args[0], op.args[1]
    lhs = encoder.env.get(lhs_name)
    rhs = encoder.env.get(rhs_name)
    if lhs is None or rhs is None:
        return None
    zero = encoder._const(0)
    s = Solver(ctx=encoder.ctx)
    s.push()
    s.add(rhs != zero)
    if s.check() == unsat:
        s.pop()
        return Finding(
            op_index=idx,
            kind=op.kind,
            out=op.out,
            category="mul-zero",
            message=f"{op.out} = {lhs_name} * {rhs_name}: rhs is always 0, result is always 0",
        )
    s.pop()
    s.push()
    s.add(lhs != zero)
    if s.check() == unsat:
        s.pop()
        return Finding(
            op_index=idx,
            kind=op.kind,
            out=op.out,
            category="mul-zero",
            message=f"{op.out} = {lhs_name} * {rhs_name}: lhs is always 0, result is always 0",
        )
    s.pop()
    return None


def _check_sub_self(encoder: Z3Encoder, op: Op, idx: int) -> Finding | None:
    """Detect x - x (always 0)."""
    if op.kind != "sub" or not op.args or len(op.args) < 2 or op.out is None:
        return None
    lhs_name, rhs_name = op.args[0], op.args[1]
    if lhs_name != rhs_name:
        return None
    return Finding(
        op_index=idx,
        kind=op.kind,
        out=op.out,
        category="sub-self",
        message=f"{op.out} = {lhs_name} - {rhs_name}: subtracting a value from itself is always 0",
    )


def _check_bitwise_identity(encoder: Z3Encoder, op: Op, idx: int) -> Finding | None:
    """Detect x & x == x, x | x == x, x ^ x == 0."""
    if op.kind not in ("bitwise_and", "bitwise_or", "bitwise_xor"):
        return None
    if not op.args or len(op.args) < 2 or op.out is None:
        return None
    lhs_name, rhs_name = op.args[0], op.args[1]
    if lhs_name != rhs_name:
        return None
    if op.kind == "bitwise_xor":
        return Finding(
            op_index=idx,
            kind=op.kind,
            out=op.out,
            category="bitwise-identity",
            message=f"{op.out} = {lhs_name} ^ {rhs_name}: XOR of same value is always 0",
        )
    return Finding(
        op_index=idx,
        kind=op.kind,
        out=op.out,
        category="bitwise-identity",
        message=f"{op.out} = {lhs_name} {op.kind} {rhs_name}: applying {op.kind} to same value is identity",
    )


def _check_double_neg(ops: list[Op], idx: int) -> Finding | None:
    """Detect -(-x)."""
    op = ops[idx]
    if op.kind != "neg" or not op.args or op.out is None:
        return None
    inner_name = op.args[0]
    # Find the definition of inner_name
    for prev in ops[:idx]:
        if prev.out == inner_name and prev.kind == "neg" and prev.args:
            return Finding(
                op_index=idx,
                kind=op.kind,
                out=op.out,
                category="double-neg",
                message=f"{op.out} = -(-{prev.args[0]}): double negation, result equals {prev.args[0]}",
            )
    return None


def _check_redundant_result(encoder: Z3Encoder, op: Op, idx: int) -> Finding | None:
    """Check if the op's result is provably equal to one of its inputs."""
    if op.out is None or not op.args or len(op.args) < 2:
        return None
    if op.kind in ("const", "const_int") or op.kind in _CMP_OPS:
        return None
    result = encoder.env.get(op.out)
    if result is None:
        return None
    for arg_name in op.args:
        arg_val = encoder.env.get(arg_name)
        if arg_val is None:
            continue
        # Validate the copy without quantifiers: all live SSA values are already
        # symbolic in the solver context, so satisfiability of inequality is a
        # direct counterexample.
        s = Solver(ctx=encoder.ctx)
        s.add(result != arg_val)
        if s.check() == unsat:
            return Finding(
                op_index=idx,
                kind=op.kind,
                out=op.out,
                category="redundant-op",
                message=f"{op.out} = {op.kind}({', '.join(op.args)}): result is always equal to {arg_name}",
            )
    return None


_CHECKS = [
    _check_add_zero,
    _check_mul_one,
    _check_mul_zero,
    _check_sub_self,
    _check_bitwise_identity,
    _check_redundant_result,
]


# ---------------------------------------------------------------------------
# Main verification
# ---------------------------------------------------------------------------


def verify_function(func: Function) -> Report:
    """Verify a single TIR function for missed optimization opportunities."""
    report = Report(function=func.name)
    encoder = Z3Encoder()

    # Seed parameters as free variables
    for param in func.params:
        encoder.env[param] = BitVec(param, BV_WIDTH, ctx=encoder.ctx)

    for idx, op in enumerate(func.ops):
        if not _is_integer_op(op):
            continue

        encoded = encoder.encode_op(op)
        if not encoded:
            continue

        # Run targeted checks
        for check in _CHECKS:
            finding = check(encoder, op, idx)
            if finding is not None:
                report.findings.append(finding)

        # Run double-neg check (needs full op list)
        finding = _check_double_neg(func.ops, idx)
        if finding is not None:
            report.findings.append(finding)

    return report


def verify_tir(
    tir_json_path: str | None = None, *, data: dict | None = None
) -> list[Report]:
    """Verify all functions in a TIR JSON file.

    Either *tir_json_path* (file path) or *data* (parsed dict) must be provided.
    """
    if data is None:
        if tir_json_path is None:
            raise ValueError("either tir_json_path or data must be provided")
        with open(tir_json_path) as f:
            data = json.load(f)

    functions = data.get("functions", [])
    reports = []
    for raw_func in functions:
        func = _parse_function(raw_func)
        report = verify_function(func)
        reports.append(report)
    return reports


def _format_reports(reports: list[Report]) -> str:
    lines: list[str] = []
    total_findings = sum(len(r.findings) for r in reports)
    lines.append(
        f"Z3 TIR pass verification: {len(reports)} function(s), {total_findings} finding(s)\n"
    )

    for report in reports:
        if not report.findings:
            lines.append(f"  {report.function}: clean")
            continue
        lines.append(f"  {report.function}: {len(report.findings)} finding(s)")
        for f in report.findings:
            lines.append(
                f"    [{f.category}] op #{f.op_index} ({f.kind} -> {f.out}): {f.message}"
            )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", nargs="?", help="Path to TIR JSON file")
    parser.add_argument("--stdin", action="store_true", help="Read TIR JSON from stdin")
    parser.add_argument("--json", action="store_true", help="Output findings as JSON")
    args = parser.parse_args(argv)

    if args.stdin:
        data = json.load(sys.stdin)
    elif args.input:
        with open(args.input) as f:
            data = json.load(f)
    else:
        parser.error("either provide an input file or use --stdin")
        return 1

    reports = verify_tir(data=data)

    if args.json:
        output = []
        for report in reports:
            output.append(
                {
                    "function": report.function,
                    "findings": [
                        {
                            "op_index": f.op_index,
                            "kind": f.kind,
                            "out": f.out,
                            "category": f.category,
                            "message": f.message,
                        }
                        for f in report.findings
                    ],
                }
            )
        json.dump(output, sys.stdout, indent=2)
        print()
    else:
        print(_format_reports(reports))

    total_findings = sum(len(r.findings) for r in reports)
    return 1 if total_findings > 0 else 0


if __name__ == "__main__":
    raise SystemExit(main())
