#!/usr/bin/env python3
"""Per-pass translation validation for the Molt mid-end optimizer.

This module provides *structural* invariant checks that can be run after
each compiler pass to detect obviously-broken transformations cheaply
(O(n) in the number of IR ops).  It is NOT a semantic equivalence checker --
full semantic correctness is delegated to differential testing
(``translation_validate.py``) and Lean4 formal proofs.

The entry point is ``TranslationValidator``, which exposes:

  * Generic structural invariants (op-count monotonicity, no new variables,
    control-flow nesting balance, pure-op preservation).
  * Per-pass validators for DCE, SCCP, CSE, and LICM.
  * A ``validate_pass`` convenience method that dispatches to the right
    per-pass validator and accumulates a JSON report.

All checks operate on serialised IR snapshots (lists of dicts) so they
can run out-of-process against JSON files emitted by ``tv_hooks.py``.
"""

from __future__ import annotations

import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


# ---------------------------------------------------------------------------
# Serialised-IR helpers
# ---------------------------------------------------------------------------

# An "op dict" is the JSON-round-trippable form of MoltOp:
#   {"kind": str, "args": [...], "result": {"name": str, "type_hint": str},
#    "metadata": {...} | null}
OpDict = dict[str, Any]


def _op_result_name(op: OpDict) -> str:
    """Extract the result name from a serialised op."""
    result = op.get("result")
    if isinstance(result, dict):
        return result.get("name", "none")
    return "none"


def _collect_defined_names(ops: list[OpDict]) -> set[str]:
    """Return the set of value names defined (result != "none") by *ops*."""
    names: set[str] = set()
    for op in ops:
        name = _op_result_name(op)
        if name != "none":
            names.add(name)
    return names


def _collect_used_names(ops: list[OpDict]) -> set[str]:
    """Return every value name referenced in any arg position."""
    used: set[str] = set()

    def _walk(obj: Any) -> None:
        if isinstance(obj, dict):
            # A MoltValue reference has a "name" key
            if "name" in obj and "type_hint" in obj:
                n = obj["name"]
                if n != "none":
                    used.add(n)
            else:
                for v in obj.values():
                    _walk(v)
        elif isinstance(obj, list):
            for item in obj:
                _walk(item)

    for op in ops:
        for arg in op.get("args", []):
            _walk(arg)
    return used


# Control-flow ops whose nesting must remain balanced.
_CF_OPEN = {"IF", "LOOP_START", "TRY_START"}
_CF_CLOSE = {"END_IF", "LOOP_END", "TRY_END"}
_CF_MID = {"ELSE"}  # ELSE sits between IF and END_IF

_CF_MATCH = {
    "IF": "END_IF",
    "LOOP_START": "LOOP_END",
    "TRY_START": "TRY_END",
}

# Pure ops according to the Molt effect model.
_PURE_OPS: frozenset[str] = frozenset(
    {
        "CONST",
        "CONST_BIGINT",
        "CONST_BOOL",
        "CONST_FLOAT",
        "CONST_STR",
        "CONST_BYTES",
        "CONST_NONE",
        "CONST_NOT_IMPLEMENTED",
        "CONST_ELLIPSIS",
        "MISSING",
        "PHI",
        "NOT",
        "IS",
        "TYPE_OF",
        "ADD",
        "SUB",
        "MUL",
        "ABS",
        "AND",
        "OR",
        "EQ",
        "NE",
        "LT",
        "LE",
        "GT",
        "GE",
        "STRING_EQ",
    }
)


# ---------------------------------------------------------------------------
# Check result
# ---------------------------------------------------------------------------


@dataclass
class CheckResult:
    """Outcome of a single structural check."""

    check: str
    passed: bool
    detail: str = ""

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"check": self.check, "passed": self.passed}
        if self.detail:
            d["detail"] = self.detail
        return d


@dataclass
class PassReport:
    """Aggregated report for one pass on one function."""

    function: str
    pass_name: str
    checks: list[CheckResult] = field(default_factory=list)

    @property
    def passed(self) -> bool:
        return all(c.passed for c in self.checks)

    def to_dict(self) -> dict[str, Any]:
        return {
            "function": self.function,
            "pass": self.pass_name,
            "passed": self.passed,
            "checks": [c.to_dict() for c in self.checks],
        }


@dataclass
class FunctionReport:
    """All pass reports for a single function."""

    function: str
    passes: list[PassReport] = field(default_factory=list)

    @property
    def passed(self) -> bool:
        return all(p.passed for p in self.passes)

    def to_dict(self) -> dict[str, Any]:
        return {
            "function": self.function,
            "passed": self.passed,
            "passes": [p.to_dict() for p in self.passes],
        }


# ---------------------------------------------------------------------------
# TranslationValidator
# ---------------------------------------------------------------------------


class TranslationValidator:
    """Structural invariant checker for Molt mid-end passes."""

    # ------------------------------------------------------------------
    # Generic structural invariants
    # ------------------------------------------------------------------

    @staticmethod
    def check_op_count_monotonic(
        before: list[OpDict],
        after: list[OpDict],
    ) -> CheckResult:
        """Ops should not *increase* for shrinking passes (DCE/Prune).

        This is a soft check: some passes (like CSE phi-trim + renaming)
        may temporarily add ops.  The caller chooses which passes this
        applies to.
        """
        if len(after) > len(before):
            return CheckResult(
                check="op_count_monotonic",
                passed=False,
                detail=(
                    f"op count increased: {len(before)} -> {len(after)} "
                    f"(+{len(after) - len(before)})"
                ),
            )
        return CheckResult(check="op_count_monotonic", passed=True)

    @staticmethod
    def check_no_new_variables(
        before: list[OpDict],
        after: list[OpDict],
    ) -> CheckResult:
        """Passes should not introduce new variable names (except PHI nodes).

        PHI nodes are allowed to create fresh names because CSE/edge-
        threading may canonicalise phi edges.
        """
        defs_before = _collect_defined_names(before)
        defs_after = _collect_defined_names(after)

        # Filter out PHI-produced names
        phi_names: set[str] = set()
        for op in after:
            if op.get("kind") == "PHI":
                n = _op_result_name(op)
                if n != "none":
                    phi_names.add(n)

        new_names = defs_after - defs_before - phi_names
        if new_names:
            sample = sorted(new_names)[:10]
            return CheckResult(
                check="no_new_variables",
                passed=False,
                detail=f"{len(new_names)} new variable(s): {sample}",
            )
        return CheckResult(check="no_new_variables", passed=True)

    @staticmethod
    def check_control_flow_structure(
        before: list[OpDict],
        after: list[OpDict],
    ) -> CheckResult:
        """IF/ELSE/END_IF and LOOP_START/LOOP_END nesting must remain balanced."""

        def _nesting_signature(ops: list[OpDict]) -> list[str] | str:
            """Return the sequence of CF bracket kinds, or an error string."""
            stack: list[str] = []
            sig: list[str] = []
            for idx, op in enumerate(ops):
                kind = op.get("kind", "")
                if kind in _CF_OPEN:
                    stack.append(kind)
                    sig.append(kind)
                elif kind in _CF_CLOSE:
                    if not stack:
                        return f"unmatched {kind} at index {idx}"
                    top = stack.pop()
                    expected_close = _CF_MATCH.get(top)
                    if expected_close != kind:
                        return (
                            f"mismatched nesting: expected {expected_close} "
                            f"but got {kind} at index {idx} (opener was {top})"
                        )
                    sig.append(kind)
                elif kind in _CF_MID:
                    sig.append(kind)
            if stack:
                return f"unclosed control-flow: {stack}"
            return sig

        after_sig = _nesting_signature(after)
        if isinstance(after_sig, str):
            return CheckResult(
                check="control_flow_structure",
                passed=False,
                detail=f"after-IR nesting error: {after_sig}",
            )
        # We only need the after-IR to be well-formed; passes are allowed
        # to prune branches (so before/after signatures may differ).
        return CheckResult(check="control_flow_structure", passed=True)

    @staticmethod
    def check_pure_op_preservation(
        before: list[OpDict],
        after: list[OpDict],
    ) -> CheckResult:
        """Pure ops can be removed, but if removed their results must not be
        used in the after-IR.
        """
        # Collect result names of pure ops present before but absent after.
        before_pure_results: dict[str, str] = {}  # name -> kind
        for op in before:
            kind = op.get("kind", "")
            if kind in _PURE_OPS:
                n = _op_result_name(op)
                if n != "none":
                    before_pure_results[n] = kind

        after_defs = _collect_defined_names(after)
        removed_pure_names = set(before_pure_results) - after_defs

        if not removed_pure_names:
            return CheckResult(check="pure_op_preservation", passed=True)

        # Check that none of the removed names are used in after-IR.
        after_uses = _collect_used_names(after)
        dangling = removed_pure_names & after_uses
        if dangling:
            sample = sorted(dangling)[:10]
            return CheckResult(
                check="pure_op_preservation",
                passed=False,
                detail=(
                    f"{len(dangling)} removed pure op result(s) still used: {sample}"
                ),
            )
        return CheckResult(check="pure_op_preservation", passed=True)

    @staticmethod
    def check_constant_propagation_valid(
        before: list[OpDict],
        after: list[OpDict],
        sccp_result: dict[str, Any] | None = None,
    ) -> CheckResult:
        """SCCP: if a variable is replaced with a constant op, the constant
        must match the SCCP lattice value (when available).

        Without an ``sccp_result`` dict we fall back to a weaker check:
        every CONST op in *after* whose result name existed in *before*
        (but was NOT a CONST) must still be consistent (i.e. not
        contradicted by another definition of the same name).
        """
        if sccp_result is None:
            # Weak check: just verify no duplicate result names were
            # introduced by the substitution.
            after_names: dict[str, int] = {}
            for op in after:
                n = _op_result_name(op)
                if n != "none":
                    after_names[n] = after_names.get(n, 0) + 1
            duplicates = {n for n, c in after_names.items() if c > 1}
            if duplicates:
                sample = sorted(duplicates)[:10]
                return CheckResult(
                    check="constant_propagation_valid",
                    passed=False,
                    detail=f"duplicate result names after SCCP: {sample}",
                )
            return CheckResult(check="constant_propagation_valid", passed=True)

        # Strong check: walk the lattice values and verify constants match.
        lattice = sccp_result.get("lattice", {})
        const_ops_after: dict[str, Any] = {}
        for op in after:
            kind = op.get("kind", "")
            if kind.startswith("CONST") and kind != "CONST_NOT_IMPLEMENTED":
                n = _op_result_name(op)
                if n != "none":
                    args = op.get("args", [])
                    const_ops_after[n] = args[0] if args else None

        mismatches: list[str] = []
        for name, const_val in const_ops_after.items():
            if name in lattice:
                lattice_val = lattice[name]
                # Only compare when the lattice has a concrete constant
                if isinstance(lattice_val, dict) and "const" in lattice_val:
                    expected = lattice_val["const"]
                    if const_val != expected:
                        mismatches.append(
                            f"{name}: lattice={expected!r}, ir={const_val!r}"
                        )

        if mismatches:
            return CheckResult(
                check="constant_propagation_valid",
                passed=False,
                detail=f"{len(mismatches)} constant mismatch(es): {mismatches[:5]}",
            )
        return CheckResult(check="constant_propagation_valid", passed=True)

    # ------------------------------------------------------------------
    # Per-pass validators
    # ------------------------------------------------------------------

    def validate_dce(
        self,
        before: list[OpDict],
        after: list[OpDict],
    ) -> PassReport:
        """Validate Dead Code Elimination.

        Removed ops must be unused pure ops, and the op count must not
        increase.
        """
        report = PassReport(function="", pass_name="dce")
        report.checks.append(self.check_op_count_monotonic(before, after))
        report.checks.append(self.check_no_new_variables(before, after))
        report.checks.append(self.check_control_flow_structure(before, after))
        report.checks.append(self.check_pure_op_preservation(before, after))

        # Extra: every op removed must have been a pure op with an unused
        # result in the *before* IR.
        before_used = _collect_used_names(before)
        before_by_result: dict[str, OpDict] = {}
        for op in before:
            n = _op_result_name(op)
            if n != "none":
                before_by_result[n] = op

        after_defs = _collect_defined_names(after)
        removed_names = set(before_by_result) - after_defs

        bad_removals: list[str] = []
        for name in sorted(removed_names):
            op = before_by_result[name]
            kind = op.get("kind", "")
            # DCE is only allowed to remove pure or trivially-dead ops
            if kind not in _PURE_OPS:
                # Allow removal if the result was unused even in before-IR
                if name in before_used:
                    bad_removals.append(f"{name} ({kind})")

        if bad_removals:
            report.checks.append(
                CheckResult(
                    check="dce_only_removes_dead",
                    passed=False,
                    detail=(
                        f"{len(bad_removals)} non-pure used op(s) removed: "
                        f"{bad_removals[:10]}"
                    ),
                )
            )
        else:
            report.checks.append(
                CheckResult(check="dce_only_removes_dead", passed=True)
            )

        return report

    def validate_sccp(
        self,
        before: list[OpDict],
        after: list[OpDict],
        sccp_result: dict[str, Any] | None = None,
    ) -> PassReport:
        """Validate Sparse Conditional Constant Propagation / edge-threading."""
        report = PassReport(function="", pass_name="sccp")
        report.checks.append(self.check_no_new_variables(before, after))
        report.checks.append(self.check_control_flow_structure(before, after))
        report.checks.append(
            self.check_constant_propagation_valid(before, after, sccp_result)
        )
        report.checks.append(self.check_pure_op_preservation(before, after))
        return report

    def validate_cse(
        self,
        before: list[OpDict],
        after: list[OpDict],
    ) -> PassReport:
        """Validate Common Sub-expression Elimination.

        Replaced ops must have been equivalent (same kind + args).
        """
        report = PassReport(function="", pass_name="cse")
        report.checks.append(self.check_no_new_variables(before, after))
        report.checks.append(self.check_control_flow_structure(before, after))
        report.checks.append(self.check_pure_op_preservation(before, after))

        # CSE-specific: for each op in *before* whose result name is still
        # used in *after* but whose defining op was removed, there must be
        # another op with the same (kind, args) signature whose result
        # replaced it.
        before_ops_by_name: dict[str, OpDict] = {}
        for op in before:
            n = _op_result_name(op)
            if n != "none":
                before_ops_by_name[n] = op

        after_defs = _collect_defined_names(after)
        after_uses = _collect_used_names(after)

        # Names that were defined before, are used after, but are not
        # defined after => they were CSE-replaced.
        replaced = (set(before_ops_by_name) & after_uses) - after_defs

        # For each replaced name, check that a structurally equivalent op
        # exists in the after-IR (the canonical representative).
        def _op_sig(op: OpDict) -> tuple[str, str]:
            return (op.get("kind", ""), json.dumps(op.get("args", []), sort_keys=True))

        after_sigs: set[tuple[str, str]] = set()
        for op in after:
            after_sigs.add(_op_sig(op))

        bad_replacements: list[str] = []
        for name in sorted(replaced):
            orig = before_ops_by_name[name]
            sig = _op_sig(orig)
            if sig not in after_sigs:
                bad_replacements.append(f"{name} ({orig.get('kind', '?')})")

        if bad_replacements:
            report.checks.append(
                CheckResult(
                    check="cse_replaced_ops_equivalent",
                    passed=False,
                    detail=(
                        f"{len(bad_replacements)} replaced op(s) have no "
                        f"equivalent in after-IR: {bad_replacements[:10]}"
                    ),
                )
            )
        else:
            report.checks.append(
                CheckResult(check="cse_replaced_ops_equivalent", passed=True)
            )

        return report

    def validate_licm(
        self,
        before: list[OpDict],
        after: list[OpDict],
    ) -> PassReport:
        """Validate Loop-Invariant Code Motion.

        Hoisted ops must be loop-invariant: their args must all be defined
        outside the loop they were in.
        """
        report = PassReport(function="", pass_name="licm")
        report.checks.append(self.check_no_new_variables(before, after))
        report.checks.append(self.check_control_flow_structure(before, after))
        report.checks.append(self.check_pure_op_preservation(before, after))

        # LICM-specific: identify ops that moved from inside a loop to
        # outside.  For each such op, all value-name args must have been
        # defined outside the loop in the before-IR.
        before_loop_depth = _compute_loop_depth_map(before)
        after_loop_depth = _compute_loop_depth_map(after)

        before_name_to_depth: dict[str, int] = {}
        for idx, op in enumerate(before):
            n = _op_result_name(op)
            if n != "none":
                before_name_to_depth[n] = before_loop_depth.get(idx, 0)

        hoisted_bad: list[str] = []
        for idx, op in enumerate(after):
            n = _op_result_name(op)
            if n == "none":
                continue
            after_depth = after_loop_depth.get(idx, 0)
            before_depth = before_name_to_depth.get(n)
            if before_depth is not None and after_depth < before_depth:
                # This op was hoisted. Check its args.
                arg_names = _extract_value_names_from_args(op.get("args", []))
                for arg_name in arg_names:
                    arg_before_depth = before_name_to_depth.get(arg_name)
                    if arg_before_depth is not None and arg_before_depth >= before_depth:
                        hoisted_bad.append(
                            f"{n}: arg {arg_name} was at loop depth "
                            f"{arg_before_depth} (>= {before_depth})"
                        )

        if hoisted_bad:
            report.checks.append(
                CheckResult(
                    check="licm_hoisted_ops_invariant",
                    passed=False,
                    detail=(
                        f"{len(hoisted_bad)} hoisted op(s) have loop-variant "
                        f"args: {hoisted_bad[:10]}"
                    ),
                )
            )
        else:
            report.checks.append(
                CheckResult(check="licm_hoisted_ops_invariant", passed=True)
            )

        return report

    # ------------------------------------------------------------------
    # Convenience dispatcher
    # ------------------------------------------------------------------

    def validate_pass(
        self,
        function_name: str,
        pass_name: str,
        before: list[OpDict],
        after: list[OpDict],
        *,
        sccp_result: dict[str, Any] | None = None,
    ) -> PassReport:
        """Validate *pass_name* and return a ``PassReport``.

        Dispatches to the appropriate per-pass validator, or runs generic
        structural checks for unknown passes.
        """
        dispatch = {
            "dce": lambda: self.validate_dce(before, after),
            "sccp": lambda: self.validate_sccp(before, after, sccp_result),
            "sccp_edge_thread": lambda: self.validate_sccp(
                before, after, sccp_result
            ),
            "cse": lambda: self.validate_cse(before, after),
            "licm": lambda: self.validate_licm(before, after),
        }

        validator = dispatch.get(pass_name)
        if validator is not None:
            report = validator()
        else:
            # Generic structural checks for simplify, prune, join_canonicalize,
            # guard_hoist, etc.
            report = PassReport(function="", pass_name=pass_name)
            report.checks.append(self.check_control_flow_structure(before, after))
            report.checks.append(self.check_pure_op_preservation(before, after))
            report.checks.append(self.check_no_new_variables(before, after))

        report.function = function_name
        return report

    # ------------------------------------------------------------------
    # Batch validation from a TV dump directory
    # ------------------------------------------------------------------

    def validate_dump_directory(
        self,
        dump_dir: str | Path,
    ) -> list[FunctionReport]:
        """Read JSON snapshots from *dump_dir* and validate every pass.

        Expected file naming: ``{function}_{pass}_before.json`` and
        ``{function}_{pass}_after.json``.

        Returns one ``FunctionReport`` per function found.
        """
        dump_path = Path(dump_dir)
        if not dump_path.is_dir():
            return []

        # Discover (function, pass) pairs from *_before.json files.
        pairs: dict[str, list[str]] = {}  # function -> [pass_name, ...]
        for before_file in sorted(dump_path.glob("*_before.json")):
            stem = before_file.stem  # e.g. "my_func_dce_before"
            # Remove the trailing "_before"
            base = stem.rsplit("_before", 1)[0]
            # Split into function name and pass name on the *last* underscore
            parts = base.rsplit("_", 1)
            if len(parts) != 2:
                continue
            func_name, pass_name = parts
            after_file = dump_path / f"{func_name}_{pass_name}_after.json"
            if after_file.exists():
                pairs.setdefault(func_name, []).append(pass_name)

        reports: list[FunctionReport] = []
        for func_name in sorted(pairs):
            func_report = FunctionReport(function=func_name)
            for pass_name in pairs[func_name]:
                before_file = dump_path / f"{func_name}_{pass_name}_before.json"
                after_file = dump_path / f"{func_name}_{pass_name}_after.json"
                try:
                    before_data = json.loads(before_file.read_text(encoding="utf-8"))
                    after_data = json.loads(after_file.read_text(encoding="utf-8"))
                except (json.JSONDecodeError, OSError) as exc:
                    pr = PassReport(function=func_name, pass_name=pass_name)
                    pr.checks.append(
                        CheckResult(
                            check="file_read",
                            passed=False,
                            detail=str(exc),
                        )
                    )
                    func_report.passes.append(pr)
                    continue

                before_ops = before_data.get("ops", before_data)
                after_ops = after_data.get("ops", after_data)
                if not isinstance(before_ops, list) or not isinstance(after_ops, list):
                    pr = PassReport(function=func_name, pass_name=pass_name)
                    pr.checks.append(
                        CheckResult(
                            check="valid_ops_format",
                            passed=False,
                            detail="ops field is not a list",
                        )
                    )
                    func_report.passes.append(pr)
                    continue

                sccp_result = before_data.get("sccp_result") or after_data.get(
                    "sccp_result"
                )
                pr = self.validate_pass(
                    func_name,
                    pass_name,
                    before_ops,
                    after_ops,
                    sccp_result=sccp_result,
                )
                func_report.passes.append(pr)

            reports.append(func_report)
        return reports


# ---------------------------------------------------------------------------
# Utility functions
# ---------------------------------------------------------------------------


def _compute_loop_depth_map(ops: list[OpDict]) -> dict[int, int]:
    """Return a mapping from op index to loop nesting depth."""
    depth = 0
    depth_map: dict[int, int] = {}
    for idx, op in enumerate(ops):
        kind = op.get("kind", "")
        if kind == "LOOP_START":
            depth += 1
        depth_map[idx] = depth
        if kind == "LOOP_END":
            depth = max(0, depth - 1)
    return depth_map


def _extract_value_names_from_args(args: list[Any]) -> set[str]:
    """Extract all MoltValue names from a serialised args list."""
    names: set[str] = set()

    def _walk(obj: Any) -> None:
        if isinstance(obj, dict):
            if "name" in obj and "type_hint" in obj:
                n = obj["name"]
                if n != "none":
                    names.add(n)
            else:
                for v in obj.values():
                    _walk(v)
        elif isinstance(obj, list):
            for item in obj:
                _walk(item)

    for arg in args:
        _walk(arg)
    return names


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    """Validate a TV dump directory and print a JSON report."""
    import argparse

    parser = argparse.ArgumentParser(
        description="Validate per-pass IR snapshots from a TV dump directory.",
    )
    parser.add_argument(
        "dump_dir",
        help="Directory containing {func}_{pass}_{before|after}.json files",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Print per-check details",
    )
    args = parser.parse_args(argv)

    validator = TranslationValidator()
    reports = validator.validate_dump_directory(args.dump_dir)

    if not reports:
        print(f"No TV snapshots found in {args.dump_dir}", file=sys.stderr)
        return 2

    total_passes = 0
    total_failures = 0
    output: list[dict[str, Any]] = []
    for fr in reports:
        output.append(fr.to_dict())
        for pr in fr.passes:
            total_passes += 1
            if not pr.passed:
                total_failures += 1

    print(json.dumps(output, indent=2))

    if args.verbose:
        print(file=sys.stderr)
        for fr in reports:
            for pr in fr.passes:
                status = "PASS" if pr.passed else "FAIL"
                print(
                    f"  [{status}] {fr.function} / {pr.pass_name}",
                    file=sys.stderr,
                )
                if not pr.passed:
                    for c in pr.checks:
                        if not c.passed:
                            print(
                                f"         {c.check}: {c.detail}",
                                file=sys.stderr,
                            )

    print(file=sys.stderr)
    print(
        f"Translation validation: {total_passes} pass(es) checked, "
        f"{total_failures} failure(s).",
        file=sys.stderr,
    )

    return 1 if total_failures > 0 else 0


if __name__ == "__main__":
    sys.exit(main())
