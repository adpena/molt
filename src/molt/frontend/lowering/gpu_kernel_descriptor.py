"""Compile ordinary function IR into the existing hardware descriptor contract.

This is a projection of captured IR, never a source-spelling specialization.
Executed Python IR remains untouched. Binding identities and dynamic bounds are
obligations of hardware admission, not assumptions made by this serializer.
"""

from __future__ import annotations

import ast
import json
from dataclasses import dataclass

from typing import Any

from molt.compiler_analysis.python_source_keys import python_ast_digest


# These are descriptor semantic roles, not runtime class registrations. Evidence
# emitted by module lowering is untrusted until final assembly compares it with
# the captured compiler reference. Actual function symbols remain per-program.
GPU_PYTHON_BODY_REFERENCES: dict[str, tuple[str, dict[str, str]]] = {
    "molt.gpu": (
        "gpu/__init__.py",
        {"Buffer.__getitem__": "buffer_get", "Buffer.__setitem__": "buffer_set"},
    ),
    "struct": (
        "stdlib/struct.py",
        {
            "pack_into": "struct_pack_into",
            "unpack_from": "struct_unpack_from",
            "_normalize_format": "struct_normalize_format",
        },
    ),
}


def body_origin_evidence(
    module_name: str, tree: ast.Module, target_python: tuple[int, int]
) -> dict[int, dict[str, Any]]:
    """Describe the actual parsed generation without opening source again.

    The caller owns the byte lease and compilation context. Typed AST identity
    includes future statements, signatures, defaults, annotations and source
    spans. Module spelling only bounds this cheap candidate census; it cannot
    grant origin. Reference parsing is deferred until a program uses a kernel.
    """
    reference = GPU_PYTHON_BODY_REFERENCES.get(module_name)
    if reference is None:
        return {}
    module_digest = python_ast_digest(tree)
    names = reference[1]
    result: dict[int, dict[str, Any]] = {}

    def visit(statements: list[ast.stmt], prefix: str = "") -> None:
        for statement in statements:
            if isinstance(statement, ast.ClassDef):
                visit(statement.body, prefix + statement.name + ".")
            elif isinstance(statement, ast.FunctionDef):
                role = names.get(prefix + statement.name)
                if role is not None:
                    if any(value is not None for value in statement.args.kw_defaults):
                        continue
                    defaults = []
                    for value in statement.args.defaults:
                        if not isinstance(value, ast.Constant) or type(
                            value.value
                        ) not in (type(None), bool, int):
                            break
                        defaults.append(value.value)
                    else:
                        result[id(statement)] = {
                            "role": role,
                            "module": module_name,
                            "module_ast": module_digest,
                            "body_ast": python_ast_digest(statement),
                            "target_python": list(target_python),
                            # A semantic change to these literal defaults must
                            # extend the explicit callable contract.
                            "defaults": defaults,
                        }

    visit(tree.body)
    return result


def rebind_pruned_body_origins(
    tree: ast.Module,
    evidence: dict[int, dict[str, Any]],
) -> dict[int, dict[str, Any]]:
    """Carry a captured origin through a content-preserving support-node copy.

    Native-support pruning may shallow-copy top-level definitions. Reusing an
    origin by spelling would admit removed decorators or a changed body; require
    the complete typed body identity before attaching it to the copied node.
    """
    if not evidence:
        return evidence
    by_body = {record["body_ast"]: record for record in evidence.values()}
    rebound = {}
    for node in ast.walk(tree):
        if not isinstance(node, ast.FunctionDef):
            continue
        record = evidence.get(id(node))
        if record is None:
            record = by_body.get(python_ast_digest(node))
        if record is not None:
            rebound[id(node)] = record
    return rebound


def descriptor_publications(ops: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """The existing internal metadata call's constant descriptor producers."""
    constants = {
        op["out"]: op
        for op in ops
        if isinstance(op, dict)
        and op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
    }
    publications = []
    for op in ops:
        if (
            not isinstance(op, dict)
            or op.get("kind") != "call"
            or op.get("s_value") != "molt_gpu_kernel_descriptor_set"
        ):
            continue
        args = op.get("args", ())
        if len(args) != 2 or args[1] not in constants:
            raise ValueError("GPU metadata publication lost its constant descriptor")
        publications.append(constants[_required_name(args[1])])
    return publications


def descriptor_body_symbols(ops: list[dict[str, Any]]) -> frozenset[str]:
    """Identity references share ordinary function/link-feature reachability."""
    symbols = set()
    for constant in descriptor_publications(ops):
        descriptor = json.loads(constant["s_value"])
        for body in descriptor.get("python_bodies", {}).values():
            symbol = body.get("symbol")
            if not isinstance(symbol, str) or not symbol:
                raise ValueError("GPU body identity lacks its executable symbol")
            symbols.add(symbol)
    return frozenset(symbols)


class UnsupportedKernel(ValueError):
    """The captured body cannot be proved safe for the current shader subset."""


def _required_name(value: object) -> str:
    if not isinstance(value, str) or not value:
        raise UnsupportedKernel("GPU descriptor value or local binding lacks a name")
    return value


def project_kernel_ops(params: list[str], ops: list[dict[str, Any]]) -> dict[str, Any]:
    try:
        return _project(params, ops)
    except (UnsupportedKernel, KeyError, IndexError, TypeError) as exc:
        # Decorating a Python function does not restrict ordinary CPU execution.
        # An explicit hardware launch reports this capability refusal.
        return {"unsupported": str(exc)}


def _project(params: list[str], ops: list[dict[str, Any]]) -> dict[str, Any]:
    def require(condition: bool, message: str) -> None:
        if not condition:
            raise UnsupportedKernel(message)

    require(len(ops) >= 7, "kernel lacks a generated entry/error boundary")
    require(
        ops[0]["kind"] == "trace_enter_slot" and type(ops[0].get("value")) is int,
        "kernel lacks a compiled code slot",
    )
    require(
        ops[1]["kind"] == "check_exception", "kernel entry lacks its error boundary"
    )
    tail = next((i for i, op in enumerate(ops) if op["kind"] == "label"), -1)
    require(
        tail >= 4
        and ops[tail - 2]["kind"] == "trace_exit"
        and ops[tail - 1]["kind"] == "ret",
        "unsupported kernel return/control flow",
    )
    require((len(ops) - tail) % 3 == 0, "invalid generated error tail")
    labels: set[int] = set()
    for i in range(tail, len(ops), 3):
        label, leave, ret = ops[i : i + 3]
        require(
            label["kind"] == "label"
            and type(label.get("value")) is int
            and label["value"] not in labels
            and leave["kind"] == "trace_exit"
            and ret["kind"] == "ret_void",
            "invalid generated error target",
        )
        labels.add(label["value"])
    require(ops[1].get("value") in labels, "entry error escapes generated tail")

    aliases: dict[str, str] = {}
    strings: dict[str, str] = {}
    none: set[str] = set()
    constants: dict[str, int] = {}
    bindings: dict[str, list[str]] = {}
    binding_uses: set[str] = set()
    # Only direct scalar/constant/query coordinates enter dynamic bounds. No
    # runtime expression evaluator or duplicate range lattice is introduced.
    coordinates: dict[str, dict[str, Any]] = {
        name: {"kind": "scalar", "name": name} for name in params
    }
    values = set(params)
    comparisons: dict[str, tuple[str, str]] = {}
    conditions: list[tuple[str, str]] = []
    buffer_names: set[str] = set()
    projected: list[dict[str, Any]] = []
    queries: list[dict[str, Any]] = []
    requirements: list[dict[str, Any]] = []
    context = False

    for original in ops[2 : tail - 2]:
        op = {
            key: value
            for key, value in original.items()
            if key in {"kind", "args", "out", "var", "value", "s_value"}
        }
        kind = op["kind"]
        args = [aliases.get(arg, arg) for arg in op.get("args", [])]
        if "args" in op:
            op["args"] = args
        out = op.get("out")
        if kind == "check_exception":
            require(
                op.get("value") in labels, "exception branch escapes generated tail"
            )
        elif kind == "line":
            pass
        elif kind == "const_none":
            out = _required_name(out)
            none.add(out)
        elif kind == "const_str":
            out = _required_name(out)
            require(isinstance(op.get("s_value"), str), "string has no payload")
            strings[out] = op["s_value"]
        elif kind == "frame_context_set":
            require(
                not context
                and len(args) == 3
                and args[0] in none
                and args[1] in constants
                and args[2] in none,
                "unsupported kernel frame context",
            )
            context = True
        elif kind in {"frame_home_store", "store_var", "load_var"}:
            source = _required_name(op["var"]) if kind == "load_var" else args[0]
            source = aliases.get(source, source)
            target = _required_name(op["var"] if kind == "store_var" else out)
            require(source in values or source in bindings, "unsupported kernel alias")
            # Conditional assignment needs a merge, not an unconditional alias.
            require(
                not conditions or kind == "load_var",
                "conditional local mutation requires a shader merge",
            )
            aliases[target] = source
        elif kind == "module_get_global":
            out = _required_name(out)
            require(
                len(args) == 2 and args[0] in none and args[1] in strings,
                "unsupported global lookup context",
            )
            bindings[out] = [strings[args[1]]]
        elif kind == "get_attr_generic_obj":
            out = _required_name(out)
            require(
                len(args) == 1
                and args[0] in bindings
                and isinstance(op.get("s_value"), str),
                "attribute lookup is not an exact module binding",
            )
            binding_uses.add(args[0])
            bindings[out] = [*bindings[args[0]], op["s_value"]]
        elif kind == "call_func":
            out = _required_name(out)
            require(
                len(args) == 1 and args[0] in bindings,
                "hardware query requires an admitted nullary binding",
            )
            binding_uses.add(args[0])
            queries.append(
                {"out": out, "path": bindings[args[0]], "conditional": bool(conditions)}
            )
            coordinates[out] = {"kind": "query", "name": out}
            values.add(out)
            projected.append({"kind": "gpu_query", "out": out})
        elif kind == "const":
            out = _required_name(out)
            require(
                type(op.get("value")) is int, "unsupported non-integer shader constant"
            )
            constants[out] = op["value"]
            coordinates[out] = {"kind": "constant", "value": op["value"]}
            values.add(out)
            projected.append(op)
        elif kind == "bool":
            out = _required_name(out)
            require(
                len(args) == 1 and args[0] in comparisons,
                "truth conversion requires an existing boolean comparison",
            )
            aliases[out] = args[0]
        elif kind in {"lt", "add", "sub", "mul"}:
            out = _required_name(out)
            require(
                len(args) == 2 and all(arg in values for arg in args),
                "unsupported arithmetic operands",
            )
            if kind == "lt":
                comparisons[out] = (args[0], args[1])
            elif all(arg in constants for arg in args):
                left, right = (constants[arg] for arg in args)
                folded = {"add": int.__add__, "sub": int.__sub__, "mul": int.__mul__}[
                    kind
                ](left, right)
                constants[out] = folded
                coordinates[out] = {"kind": "constant", "value": folded}
                op = {"kind": "const", "out": out, "value": folded}
            values.add(out)
            projected.append(op)
        elif kind == "index" or kind == "store_index":
            require(
                len(args) == (2 if kind == "index" else 3) and args[0] in params,
                "indexed receiver must be a kernel buffer parameter",
            )
            require(
                args[1] in coordinates,
                "index range requires compiler range facts not present in this descriptor",
            )
            upper_limits = [
                coordinates[right]
                for left, right in conditions
                if left == args[1] and right in coordinates
            ]
            requirements.append(
                {
                    "kind": "bounds",
                    "buffer": args[0],
                    "index": coordinates[args[1]],
                    "upper_limits": upper_limits,
                }
            )
            buffer_names.add(args[0])
            if kind == "index":
                out = _required_name(out)
                values.add(out)
            else:
                require(args[2] in values, "unsupported buffer store value")
            projected.append(op)
        elif kind == "if":
            require(
                len(args) == 1 and args[0] in comparisons,
                "branch requires a boolean comparison",
            )
            conditions.append(comparisons[args[0]])
            projected.append(op)
        elif kind == "end_if":
            require(bool(conditions), "unbalanced kernel branch")
            conditions.pop()
            projected.append(op)
        else:
            raise UnsupportedKernel(
                f"unsupported hardware descriptor operation: {kind}"
            )
    require(
        not conditions
        and context
        and ops[tail - 1].get("args", []) in [[name] for name in none],
        "kernel lacks a balanced implicit-None return",
    )
    require(
        set(bindings) <= binding_uses,
        "unconsumed namespace lookup may have observable errors",
    )
    return {
        "code_slot": ops[0]["value"],
        "ops": projected,
        "query_bindings": queries,
        "requirements": requirements,
        "buffers": sorted(buffer_names),
        "numeric": _integral_certificate(params, buffer_names, projected),
    }


@dataclass(frozen=True)
class _IntegralFact:
    # abs(v) < 2**bits; this is magnitude, never significand width.
    bits: int
    nonnegative: bool = False
    nonzero: bool = False
    boolean: bool = False


def _integral_certificate(
    params: list[str], buffers: set[str], ops: list[dict[str, Any]]
) -> dict[str, Any]:
    """Discharge arithmetic once; publish only entry-data obligations.

    The frontend's existing static truth evaluator is for concrete constants,
    while TIR's IntRange saturates and runs later. Neither proves symbolic
    nonoverflow here. This descriptor-local proof uses integer magnitude
    budgets, plus sign/nonzero facts solely to exclude signed-zero changes.
    No analysis graph is serialized or interpreted in the runtime.
    """
    reads = sorted({op["args"][0] for op in ops if op["kind"] == "index"})
    memory_indices = [
        op["args"][1] for op in ops if op["kind"] in {"index", "store_index"}
    ]
    query_outputs = {op["out"] for op in ops if op["kind"] == "gpu_query"}
    modes = (
        ("any", False, False),
        ("nonnegative", True, False),
        ("nonzero", False, True),
        ("positive", True, True),
    )
    alternatives = []
    for mode, nonnegative, nonzero in modes:
        for budget in range(24, -1, -1):
            leaf = _IntegralFact(budget, nonnegative, nonzero)
            facts = {name: leaf for name in params if name not in buffers}
            memory = leaf
            accepted = True
            for op in ops:
                kind, args, out = op["kind"], op.get("args", []), op.get("out")
                if kind == "const":
                    value = op["value"]
                    fact = _IntegralFact(
                        abs(value).bit_length(), value >= 0, value != 0
                    )
                elif kind == "gpu_query":
                    # Current geometry queries are nonnegative. Actual binding
                    # and magnitude are checked once at launch; barrier has no
                    # numeric result and cannot be used as a scalar.
                    fact = _IntegralFact(budget, True, False)
                elif kind == "index":
                    fact = memory
                elif kind in {"add", "sub", "mul", "lt"}:
                    left, right = (facts.get(arg) for arg in args)
                    if left is None or right is None or left.boolean or right.boolean:
                        accepted = False
                        break
                    if kind == "lt":
                        fact = _IntegralFact(1, True, False, True)
                    elif kind == "mul":
                        if not (
                            (left.nonnegative and right.nonnegative)
                            or (left.nonzero and right.nonzero)
                        ):
                            accepted = False
                            break
                        fact = _IntegralFact(
                            left.bits + right.bits,
                            left.nonnegative and right.nonnegative,
                            left.nonzero and right.nonzero,
                        )
                    else:
                        nonnegative_result = (
                            kind == "add" and left.nonnegative and right.nonnegative
                        )
                        fact = _IntegralFact(
                            max(left.bits, right.bits) + 1,
                            nonnegative_result,
                            nonnegative_result and (left.nonzero or right.nonzero),
                        )
                elif kind == "store_index":
                    value = facts.get(args[2])
                    if value is None or value.boolean:
                        accepted = False
                        break
                    # One conservative memory state covers all backing aliases.
                    # Joining on every store also covers a conditional store's
                    # untaken branch without a runtime memory interpreter.
                    memory = _IntegralFact(
                        max(memory.bits, value.bits),
                        memory.nonnegative and value.nonnegative,
                        memory.nonzero and value.nonzero,
                    )
                    continue
                elif kind in {"if", "end_if"}:
                    continue
                else:
                    raise UnsupportedKernel(
                        f"strict numeric proof lacks operation: {kind}"
                    )
                if fact.bits > 24:
                    accepted = False
                    break
                facts[_required_name(out)] = fact
            if accepted:
                alternatives.append({"magnitude_bits": budget, "sign": mode})
                break
    if not alternatives:
        raise UnsupportedKernel(
            "hardware arithmetic lacks strict integral/overflow/signed-zero proof"
        )
    return {
        "kind": "strict_integral_i32",
        "alternatives": alternatives,
        "read_buffers": reads,
        "scalar_params": sorted(set(params) - buffers),
        # Multiple logical threads may only touch their own element. Nonquery
        # coordinates remain valid for one-thread launches after bounds checks.
        "memory_queries": sorted(set(memory_indices) & query_outputs),
        "single_thread": any(index not in query_outputs for index in memory_indices),
    }
