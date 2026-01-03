from __future__ import annotations

import ast
from dataclasses import dataclass
from typing import Any, Literal, TypedDict, cast

from molt.compat import CompatibilityError, CompatibilityReporter, FallbackPolicy


@dataclass
class MoltValue:
    name: str
    type_hint: str = "Unknown"


@dataclass
class MoltOp:
    kind: str
    args: list[Any]
    result: MoltValue
    metadata: dict[str, Any] | None = None


class ClassInfo(TypedDict, total=False):
    fields: dict[str, int]
    size: int
    field_order: list[str]
    defaults: dict[str, ast.expr]
    dataclass: bool
    frozen: bool
    eq: bool
    repr: bool


class FuncInfo(TypedDict):
    params: list[str]
    ops: list[MoltOp]


class SimpleTIRGenerator(ast.NodeVisitor):
    def __init__(
        self,
        parse_codec: Literal["msgpack", "cbor", "json"] = "msgpack",
        type_hint_policy: Literal["ignore", "trust", "check"] = "ignore",
        fallback_policy: FallbackPolicy = "error",
        source_path: str | None = None,
    ) -> None:
        self.funcs_map: dict[str, FuncInfo] = {"molt_main": {"params": [], "ops": []}}
        self.current_func_name: str = "molt_main"
        self.current_ops: list[MoltOp] = self.funcs_map["molt_main"]["ops"]
        self.var_count: int = 0
        self.state_count: int = 0
        self.classes: dict[str, ClassInfo] = {}
        self.locals: dict[str, MoltValue] = {}
        self.boxed_locals: dict[str, MoltValue] = {}
        self.globals: dict[str, MoltValue] = {}
        self.async_locals: dict[str, int] = {}
        self.parse_codec = parse_codec
        self.type_hint_policy = type_hint_policy
        self.explicit_type_hints: dict[str, str] = {}
        self.fallback_policy = fallback_policy
        self.compat = CompatibilityReporter(fallback_policy, source_path)

    def visit(self, node: ast.AST) -> Any:
        try:
            return super().visit(node)
        except CompatibilityError:
            raise
        except NotImplementedError as exc:
            raise self.compat.unsupported(
                node,
                feature=str(exc),
                tier="bridge",
                impact="high",
            ) from exc

    def next_var(self) -> str:
        name = f"v{self.var_count}"
        self.var_count += 1
        return name

    def emit(self, op: MoltOp) -> None:
        self.current_ops.append(op)

    def _fast_int_enabled(self) -> bool:
        return self.type_hint_policy in {"trust", "check"}

    def _should_fast_int(self, op: MoltOp) -> bool:
        if not self._fast_int_enabled():
            return False
        if op.kind not in {"ADD", "SUB", "MUL", "LT", "EQ"}:
            return False
        return all(
            isinstance(arg, MoltValue) and arg.type_hint == "int" for arg in op.args
        )

    def _emit_bridge_unavailable(self, message: str) -> MoltValue:
        msg_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[message], result=msg_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="BRIDGE_UNAVAILABLE", args=[msg_val], result=res))
        return res

    def _bridge_fallback(
        self,
        node: ast.AST,
        feature: str,
        *,
        impact: Literal["low", "medium", "high"] = "high",
        alternative: str | None = None,
        detail: str | None = None,
    ) -> MoltValue:
        issue = self.compat.bridge_unavailable(
            node, feature, impact=impact, alternative=alternative, detail=detail
        )
        if self.fallback_policy != "bridge":
            raise self.compat.error(issue)
        return self._emit_bridge_unavailable(issue.runtime_message())

    def _emit_nullcontext(self, payload: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="context_manager")
        self.emit(MoltOp(kind="CONTEXT_NULL", args=[payload], result=res))
        return res

    def start_function(self, name: str, params: list[str] | None = None) -> None:
        if name not in self.funcs_map:
            self.funcs_map[name] = FuncInfo(params=params or [], ops=[])
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]
        self.locals = {}
        self.async_locals = {}
        self.explicit_type_hints = {}

    def resume_function(self, name: str) -> None:
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]

    def is_async(self) -> bool:
        return self.current_func_name.endswith("_poll")

    def _normalize_type_hint(self, name: str) -> str | None:
        normalized = name.lower()
        mapping = {
            "int": "int",
            "float": "float",
            "str": "str",
            "bytes": "bytes",
            "bytearray": "bytearray",
            "bool": "bool",
            "list": "list",
            "tuple": "tuple",
            "dict": "dict",
            "range": "range",
            "slice": "slice",
            "buffer2d": "buffer2d",
            "any": "Any",
            "optional": "Any",
            "union": "Any",
        }
        return mapping.get(normalized)

    def _annotation_to_hint(self, node: ast.expr | None) -> str | None:
        if node is None:
            return None
        if isinstance(node, ast.Name):
            return self._normalize_type_hint(node.id)
        if isinstance(node, ast.Attribute):
            if isinstance(node.value, ast.Name) and node.value.id == "typing":
                return self._normalize_type_hint(node.attr)
        if isinstance(node, ast.Subscript):
            base = node.value
            if isinstance(base, ast.Name):
                return self._normalize_type_hint(base.id)
            if isinstance(base, ast.Attribute):
                if isinstance(base.value, ast.Name) and base.value.id == "typing":
                    return self._normalize_type_hint(base.attr)
        return None

    def _guard_tag_for_hint(self, hint: str) -> int | None:
        mapping = {
            "Any": 0,
            "Unknown": 0,
            "int": 1,
            "float": 2,
            "bool": 3,
            "None": 4,
            "str": 5,
            "bytes": 6,
            "bytearray": 7,
            "list": 8,
            "tuple": 9,
            "dict": 10,
            "range": 11,
            "slice": 12,
            "dataclass": 13,
            "buffer2d": 14,
        }
        return mapping.get(hint)

    def _emit_guard_type(self, value: MoltValue, hint: str) -> None:
        tag = self._guard_tag_for_hint(hint)
        if tag is None or tag == 0:
            return
        tag_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[tag], result=tag_val))
        self.emit(
            MoltOp(kind="GUARD_TYPE", args=[value, tag_val], result=MoltValue("none"))
        )

    def _apply_explicit_hint(self, name: str, value: MoltValue) -> None:
        hint = self.explicit_type_hints.get(name)
        if hint is None:
            return
        if self.type_hint_policy == "check":
            self._emit_guard_type(value, hint)
            value.type_hint = hint
            return
        if self.type_hint_policy == "trust":
            value.type_hint = hint

    def visit_Name(self, node: ast.Name) -> Any:
        if isinstance(node.ctx, ast.Load):
            if self.is_async():
                if node.id in self.async_locals:
                    offset = self.async_locals[node.id]
                    res = MoltValue(self.next_var())
                    self.emit(
                        MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res)
                    )
                    return res
            if node.id in self.boxed_locals:
                cell = self.boxed_locals[node.id]
                idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=idx))
                res = MoltValue(self.next_var())
                self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=res))
                return res
            return self.locals.get(node.id)
        return node.id

    def _box_local(self, name: str) -> None:
        if name in self.boxed_locals:
            return
        if name in self.locals:
            init = self.locals[name]
        else:
            init = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=init))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[init], result=cell))
        self.boxed_locals[name] = cell
        self.locals[name] = cell

    def _collect_assigned_names(self, nodes: list[ast.stmt]) -> set[str]:
        class AssignCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        self.names.add(target.id)
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                if isinstance(node.target, ast.Name):
                    self.names.add(node.target.id)
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                if isinstance(node.target, ast.Name):
                    self.names.add(node.target.id)
                self.generic_visit(node.value)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = AssignCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _load_local_value(self, name: str) -> MoltValue | None:
        if self.is_async():
            if name in self.async_locals:
                offset = self.async_locals[name]
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res)
                )
                return res
        if name in self.boxed_locals:
            cell = self.boxed_locals[name]
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            res = MoltValue(self.next_var())
            self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=res))
            return res
        return self.locals.get(name)

    def _store_local_value(self, name: str, value: MoltValue) -> None:
        if name in self.boxed_locals:
            cell = self.boxed_locals[name]
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, idx, value],
                    result=MoltValue("none"),
                )
            )
        else:
            self.locals[name] = value

    def _iterable_is_indexable(self, iterable: MoltValue | None) -> bool:
        if iterable is None:
            return False
        return iterable.type_hint in {
            "list",
            "tuple",
            "dict_keys_view",
            "dict_values_view",
            "dict_items_view",
            "range",
        }

    def _match_vector_sum_loop(self, node: ast.For) -> tuple[str, str] | None:
        if not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        target_name = node.target.id
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, ast.Add):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if not isinstance(stmt.value, ast.Name):
                return None
            if stmt.value.id != target_name:
                return None
            if stmt.target.id == target_name:
                return None
            return (stmt.target.id, target_name)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            dest = stmt.targets[0].id
            if dest == target_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if not isinstance(left, ast.Name) or left.id != dest:
                return None
            if not isinstance(right, ast.Name) or right.id != target_name:
                return None
            return (dest, target_name)
        return None

    def _emit_iter_loop(self, node: ast.For, iterable: MoltValue) -> None:
        target = node.target
        assert isinstance(target, ast.Name)
        iter_obj = MoltValue(self.next_var(), type_hint="iter")
        self.emit(MoltOp(kind="ITER_NEW", args=[iterable], result=iter_obj))

        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))

        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self._store_local_value(target.id, item)
        for stmt in node.body:
            self.visit(stmt)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _emit_index_loop(self, node: ast.For, iterable: MoltValue) -> None:
        target = node.target
        assert isinstance(target, ast.Name)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        length = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[iterable], result=length))

        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[zero], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, length], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[iterable, idx], result=item))
        self._store_local_value(target.id, item)
        for stmt in node.body:
            self.visit(stmt)
        next_idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _parse_range_call(
        self, node: ast.AST
    ) -> tuple[MoltValue, MoltValue, MoltValue] | None:
        if not isinstance(node, ast.Call):
            return None
        if not isinstance(node.func, ast.Name) or node.func.id != "range":
            return None
        if len(node.args) == 1:
            start = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start))
            stop = self.visit(node.args[0])
            step = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=step))
            return start, stop, step
        if len(node.args) == 2:
            start = self.visit(node.args[0])
            stop = self.visit(node.args[1])
            step = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=step))
            return start, stop, step
        if len(node.args) == 3:
            start = self.visit(node.args[0])
            stop = self.visit(node.args[1])
            step = self.visit(node.args[2])
            return start, stop, step
        raise NotImplementedError("range expects 1, 2, or 3 arguments")

    def _emit_range_loop(
        self, node: ast.For, start: MoltValue, stop: MoltValue, step: MoltValue
    ) -> None:
        target = node.target
        assert isinstance(target, ast.Name)
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        step_pos = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[zero, step], result=step_pos))
        self.emit(MoltOp(kind="IF", args=[step_pos], result=MoltValue("none")))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self._store_local_value(target.id, idx)
        for stmt in node.body:
            self.visit(stmt)
        next_idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        step_neg = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[step, zero], result=step_neg))
        self.emit(MoltOp(kind="IF", args=[step_neg], result=MoltValue("none")))
        idx_neg = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx_neg))
        cond_neg = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[stop, idx_neg], result=cond_neg))
        self.emit(
            MoltOp(
                kind="LOOP_BREAK_IF_FALSE",
                args=[cond_neg],
                result=MoltValue("none"),
            )
        )
        self._store_local_value(target.id, idx_neg)
        for stmt in node.body:
            self.visit(stmt)
        next_idx_neg = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx_neg, step], result=next_idx_neg))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx_neg], result=idx_neg))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_range_list(
        self, start: MoltValue, stop: MoltValue, step: MoltValue
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        step_pos = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[zero, step], result=step_pos))
        self.emit(MoltOp(kind="IF", args=[step_pos], result=MoltValue("none")))

        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LIST_APPEND", args=[res, idx], result=MoltValue("none")))
        next_idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        step_neg = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[step, zero], result=step_neg))
        self.emit(MoltOp(kind="IF", args=[step_neg], result=MoltValue("none")))
        idx_neg = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx_neg))
        cond_neg = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[stop, idx_neg], result=cond_neg))
        self.emit(
            MoltOp(
                kind="LOOP_BREAK_IF_FALSE",
                args=[cond_neg],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(kind="LIST_APPEND", args=[res, idx_neg], result=MoltValue("none"))
        )
        next_idx_neg = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx_neg, step], result=next_idx_neg))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx_neg], result=idx_neg))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return res

    def _emit_list_from_iter(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        iter_obj = MoltValue(self.next_var(), type_hint="iter")
        self.emit(MoltOp(kind="ITER_NEW", args=[iterable], result=iter_obj))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(
            MoltOp(kind="LIST_APPEND", args=[res, item], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return res

    def _emit_for_loop(self, node: ast.For, iterable: MoltValue) -> None:
        if self._iterable_is_indexable(iterable):
            self._emit_index_loop(node, iterable)
        else:
            self._emit_iter_loop(node, iterable)

    def _match_counted_while(
        self, node: ast.While
    ) -> tuple[str, int, list[ast.stmt]] | None:
        if node.orelse:
            return None
        if not isinstance(node.test, ast.Compare):
            return None
        if len(node.test.ops) != 1 or not isinstance(node.test.ops[0], ast.Lt):
            return None
        if not isinstance(node.test.left, ast.Name):
            return None
        if len(node.test.comparators) != 1:
            return None
        bound = node.test.comparators[0]
        if not (isinstance(bound, ast.Constant) and isinstance(bound.value, int)):
            return None
        if not node.body:
            return None
        index_name = node.test.left.id
        incr_stmt = node.body[-1]
        if not self._is_unit_increment(incr_stmt, index_name):
            return None
        if index_name in self._collect_assigned_names(node.body[:-1]):
            return None
        return index_name, bound.value, node.body[:-1]

    def _is_unit_increment(self, stmt: ast.stmt, name: str) -> bool:
        if isinstance(stmt, ast.AugAssign):
            if isinstance(stmt.target, ast.Name) and stmt.target.id == name:
                return (
                    isinstance(stmt.op, ast.Add)
                    and isinstance(stmt.value, ast.Constant)
                    and stmt.value.value == 1
                )
            return False
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return False
            if stmt.targets[0].id != name:
                return False
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return False
            left = stmt.value.left
            right = stmt.value.right
            if (
                isinstance(left, ast.Name)
                and left.id == name
                and isinstance(right, ast.Constant)
                and right.value == 1
            ):
                return True
            if (
                isinstance(right, ast.Name)
                and right.id == name
                and isinstance(left, ast.Constant)
                and left.value == 1
            ):
                return True
        return False

    def _emit_counted_while(
        self, index_name: str, bound: int, body: list[ast.stmt]
    ) -> None:
        start = self._load_local_value(index_name)
        if start is None:
            start = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        stop = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[bound], result=stop))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self._store_local_value(index_name, idx)
        for stmt in body:
            self.visit(stmt)
        next_idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def visit_BinOp(self, node: ast.BinOp) -> Any:
        left = self.visit(node.left)
        right = self.visit(node.right)
        res_type = "Unknown"
        if isinstance(node.op, ast.Add):
            op_kind = "ADD"
            if left.type_hint == right.type_hint and left.type_hint in {
                "int",
                "float",
                "str",
                "bytes",
                "bytearray",
                "list",
                "tuple",
            }:
                res_type = left.type_hint
            elif {left.type_hint, right.type_hint} == {"int", "float"}:
                res_type = "float"
        elif isinstance(node.op, ast.Sub):
            op_kind = "SUB"
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
        elif isinstance(node.op, ast.Mult):
            op_kind = "MUL"
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
        else:
            op_kind = "UNKNOWN"
        res = MoltValue(self.next_var(), type_hint=res_type)
        self.emit(MoltOp(kind=op_kind, args=[left, right], result=res))
        return res

    def visit_Constant(self, node: ast.Constant) -> Any:
        if node.value is None:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            return res
        if isinstance(node.value, bytes):
            res = MoltValue(self.next_var(), type_hint="bytes")
            self.emit(MoltOp(kind="CONST_BYTES", args=[node.value], result=res))
            return res
        if isinstance(node.value, float):
            res = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[node.value], result=res))
            return res
        if isinstance(node.value, str):
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.value], result=res))
            return res
        res = MoltValue(self.next_var(), type_hint=type(node.value).__name__)
        self.emit(MoltOp(kind="CONST", args=[node.value], result=res))
        return res

    def _emit_str_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STR_FROM_OBJ", args=[value], result=res))
        return res

    def _emit_string_join(self, parts: list[MoltValue]) -> MoltValue:
        if not parts:
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[""], result=res))
            return res
        if len(parts) == 1:
            return parts[0]
        sep = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=sep))
        items = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=parts, result=items))
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STRING_JOIN", args=[sep, items], result=res))
        return res

    def _emit_string_format(self, value: MoltValue, spec: str) -> MoltValue:
        spec_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[spec], result=spec_val))
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STRING_FORMAT", args=[value, spec_val], result=res))
        return res

    def _format_spec_to_str(self, node: ast.expr) -> str:
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.JoinedStr):
            parts: list[str] = []
            for item in node.values:
                if isinstance(item, ast.Constant) and isinstance(item.value, str):
                    parts.append(item.value)
                else:
                    raise NotImplementedError(
                        "Dynamic f-string format specs are not supported"
                    )
            return "".join(parts)
        raise NotImplementedError("Unsupported f-string format spec")

    def _parse_format_literal(self, text: str) -> list[tuple[str, str | int, str]]:
        parts: list[tuple[str, str | int, str]] = []
        idx = 0
        implicit = 0
        auto_used = False
        manual_used = False
        while idx < len(text):
            ch = text[idx]
            if ch == "{":
                if idx + 1 < len(text) and text[idx + 1] == "{":
                    parts.append(("text", "{", ""))
                    idx += 2
                    continue
                end = text.find("}", idx + 1)
                if end == -1:
                    raise NotImplementedError("Unclosed format placeholder")
                inner = text[idx + 1 : end]
                if "!" in inner:
                    raise NotImplementedError(
                        "Format conversion flags are not supported"
                    )
                if ":" in inner:
                    field, spec = inner.split(":", 1)
                else:
                    field, spec = inner, ""
                if field == "":
                    auto_used = True
                    if manual_used:
                        raise NotImplementedError(
                            "Cannot mix automatic and manual field numbering"
                        )
                    parts.append(("arg", implicit, spec))
                    implicit += 1
                elif field.isdigit():
                    manual_used = True
                    if auto_used:
                        raise NotImplementedError(
                            "Cannot mix automatic and manual field numbering"
                        )
                    parts.append(("arg", int(field), spec))
                else:
                    if "." in field or "[" in field:
                        raise NotImplementedError(
                            "Format field access is not supported"
                        )
                    if not (field[0].isalpha() or field[0] == "_"):
                        raise NotImplementedError("Invalid format field name")
                    if not field.replace("_", "").isalnum():
                        raise NotImplementedError("Invalid format field name")
                    manual_used = True
                    if auto_used:
                        raise NotImplementedError(
                            "Cannot mix automatic and manual field numbering"
                        )
                    parts.append(("arg", field, spec))
                idx = end + 1
                continue
            if ch == "}":
                if idx + 1 < len(text) and text[idx + 1] == "}":
                    parts.append(("text", "}", ""))
                    idx += 2
                    continue
                raise NotImplementedError("Single '}' in format string")
            start = idx
            while idx < len(text) and text[idx] not in "{}":
                idx += 1
            parts.append(("text", text[start:idx], ""))
        return parts

    def _parse_molt_buffer_call(
        self, node: ast.Call, name: str
    ) -> list[ast.expr] | None:
        if (
            isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "molt_buffer"
            and node.func.attr == name
        ):
            return node.args
        return None

    def _match_matmul_loop(self, node: ast.For) -> tuple[str, str, str] | None:
        if node.orelse or not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1 or not isinstance(node.body[0], ast.For):
            return None
        outer_i = node.target.id
        j_loop = node.body[0]
        if j_loop.orelse or not isinstance(j_loop.target, ast.Name):
            return None
        inner_j = j_loop.target.id
        if len(j_loop.body) != 3:
            return None
        init = j_loop.body[0]
        k_loop = j_loop.body[1]
        store = j_loop.body[2]
        if not isinstance(init, ast.Assign):
            return None
        if len(init.targets) != 1 or not isinstance(init.targets[0], ast.Name):
            return None
        acc_name = init.targets[0].id
        if not isinstance(init.value, ast.Constant) or init.value.value != 0:
            return None
        if not isinstance(k_loop, ast.For) or k_loop.orelse:
            return None
        if not isinstance(k_loop.target, ast.Name):
            return None
        inner_k = k_loop.target.id
        if len(k_loop.body) != 1 or not isinstance(k_loop.body[0], ast.Assign):
            return None
        acc_assign = k_loop.body[0]
        if (
            len(acc_assign.targets) != 1
            or not isinstance(acc_assign.targets[0], ast.Name)
            or acc_assign.targets[0].id != acc_name
        ):
            return None
        if not isinstance(acc_assign.value, ast.BinOp) or not isinstance(
            acc_assign.value.op, ast.Add
        ):
            return None
        add_left = acc_assign.value.left
        add_right = acc_assign.value.right
        if not isinstance(add_left, ast.Name) or add_left.id != acc_name:
            return None
        if not isinstance(add_right, ast.BinOp) or not isinstance(
            add_right.op, ast.Mult
        ):
            return None
        left_get = add_right.left
        right_get = add_right.right
        if not (isinstance(left_get, ast.Call) and isinstance(right_get, ast.Call)):
            return None
        left_args = self._parse_molt_buffer_call(left_get, "get")
        right_args = self._parse_molt_buffer_call(right_get, "get")
        if left_args is None or right_args is None:
            return None
        if len(left_args) != 3 or len(right_args) != 3:
            return None
        if not all(isinstance(arg, ast.Name) for arg in left_args[1:]):
            return None
        if not all(isinstance(arg, ast.Name) for arg in right_args[1:]):
            return None
        left_buf = left_args[0]
        right_buf = right_args[0]
        if not isinstance(left_buf, ast.Name) or not isinstance(right_buf, ast.Name):
            return None
        a_name = left_buf.id
        b_name = right_buf.id
        left_i = cast(ast.Name, left_args[1]).id
        left_k = cast(ast.Name, left_args[2]).id
        right_k = cast(ast.Name, right_args[1]).id
        right_j = cast(ast.Name, right_args[2]).id
        if left_i != outer_i or left_k != inner_k:
            return None
        if right_k != inner_k or right_j != inner_j:
            return None
        if not isinstance(store, ast.Expr) or not isinstance(store.value, ast.Call):
            return None
        store_args = self._parse_molt_buffer_call(store.value, "set")
        if store_args is None or len(store_args) != 4:
            return None
        if not isinstance(store_args[0], ast.Name):
            return None
        out_name = store_args[0].id
        if not all(isinstance(arg, ast.Name) for arg in store_args[1:3]):
            return None
        if (
            cast(ast.Name, store_args[1]).id != outer_i
            or cast(ast.Name, store_args[2]).id != inner_j
        ):
            return None
        if not isinstance(store_args[3], ast.Name) or store_args[3].id != acc_name:
            return None
        return out_name, a_name, b_name

    def visit_JoinedStr(self, node: ast.JoinedStr) -> Any:
        parts: list[MoltValue] = []
        for item in node.values:
            if isinstance(item, ast.Constant) and isinstance(item.value, str):
                lit = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[item.value], result=lit))
                parts.append(lit)
                continue
            if isinstance(item, ast.FormattedValue):
                if item.conversion != -1:
                    raise NotImplementedError(
                        "Formatted value conversion not supported"
                    )
                value = self.visit(item.value)
                if item.format_spec is None:
                    parts.append(self._emit_str_from_obj(value))
                    continue
                spec_text = self._format_spec_to_str(item.format_spec)
                parts.append(self._emit_string_format(value, spec_text))
                continue
            raise NotImplementedError("Unsupported f-string segment")
        return self._emit_string_join(parts)

    def visit_List(self, node: ast.List) -> Any:
        elems = [self.visit(elt) for elt in node.elts]
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=elems, result=res))
        return res

    def visit_Tuple(self, node: ast.Tuple) -> Any:
        elems = [self.visit(elt) for elt in node.elts]
        res = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=elems, result=res))
        return res

    def visit_Dict(self, node: ast.Dict) -> Any:
        items: list[MoltValue] = []
        for key, value in zip(node.keys, node.values):
            if key is None:
                raise NotImplementedError("Dict unpacking is not supported")
            items.append(self.visit(key))
            items.append(self.visit(value))
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=items, result=res))
        return res

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        dataclass_opts = None
        if node.decorator_list:
            for deco in node.decorator_list:
                if isinstance(deco, ast.Name) and deco.id == "dataclass":
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
                    dataclass_opts = {"frozen": False, "eq": True, "repr": True}
                    continue
                if (
                    isinstance(deco, ast.Call)
                    and isinstance(deco.func, ast.Name)
                    and deco.func.id == "dataclass"
                ):
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
                    dataclass_opts = {"frozen": False, "eq": True, "repr": True}
                    for kw in deco.keywords:
                        if kw.arg not in {"frozen", "eq", "repr"}:
                            raise NotImplementedError(
                                f"Unsupported dataclass option: {kw.arg}"
                            )
                        if not isinstance(kw.value, ast.Constant) or not isinstance(
                            kw.value.value, bool
                        ):
                            raise NotImplementedError(
                                f"dataclass {kw.arg} must be a boolean literal"
                            )
                        dataclass_opts[kw.arg] = kw.value.value
                    continue
                raise NotImplementedError("Unsupported class decorator")

        if dataclass_opts is not None:
            field_order: list[str] = []
            field_defaults: dict[str, ast.expr] = {}
            for item in node.body:
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    name = item.target.id
                    field_order.append(name)
                    if item.value is not None:
                        field_defaults[name] = item.value
            field_indices = {name: idx for idx, name in enumerate(field_order)}
            self.classes[node.name] = {
                "fields": field_indices,
                "field_order": field_order,
                "defaults": field_defaults,
                "size": len(field_order) * 8,
                "dataclass": True,
                "frozen": dataclass_opts["frozen"],
                "eq": dataclass_opts["eq"],
                "repr": dataclass_opts["repr"],
            }
            return None

        fields: dict[str, int] = {}
        offset = 0
        for item in node.body:
            if isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name):
                fields[item.target.id] = offset
                offset += 8
        self.classes[node.name] = ClassInfo(fields=fields, size=offset)
        return None

    def visit_Call(self, node: ast.Call) -> Any:
        if isinstance(node.func, ast.Attribute):
            # ...
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "contextlib"
                and node.func.attr == "nullcontext"
            ):
                if len(node.args) > 1:
                    raise NotImplementedError("nullcontext expects 0 or 1 argument")
                if node.args:
                    payload = self.visit(node.args[0])
                else:
                    payload = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=payload))
                return self._emit_nullcontext(payload)
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_json"
            ):
                if node.func.attr == "parse":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if self.parse_codec == "cbor":
                        kind = "CBOR_PARSE"
                    elif self.parse_codec == "json":
                        kind = "JSON_PARSE"
                    else:
                        kind = "MSGPACK_PARSE"
                    self.emit(MoltOp(kind=kind, args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_msgpack"
            ):
                if node.func.attr == "parse":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="MSGPACK_PARSE", args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_cbor"
            ):
                if node.func.attr == "parse":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="CBOR_PARSE", args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_buffer"
            ):
                if node.func.attr == "new":
                    if len(node.args) not in (2, 3):
                        raise NotImplementedError(
                            "molt_buffer.new expects 2 or 3 arguments"
                        )
                    rows = self.visit(node.args[0])
                    cols = self.visit(node.args[1])
                    if len(node.args) == 3:
                        init = self.visit(node.args[2])
                    else:
                        init = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[0], result=init))
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(kind="BUFFER2D_NEW", args=[rows, cols, init], result=res)
                    )
                    return res
                if node.func.attr == "get":
                    if len(node.args) != 3:
                        raise NotImplementedError("molt_buffer.get expects 3 arguments")
                    buf = self.visit(node.args[0])
                    row = self.visit(node.args[1])
                    col = self.visit(node.args[2])
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="BUFFER2D_GET", args=[buf, row, col], result=res)
                    )
                    return res
                if node.func.attr == "set":
                    if len(node.args) != 4:
                        raise NotImplementedError("molt_buffer.set expects 4 arguments")
                    buf = self.visit(node.args[0])
                    row = self.visit(node.args[1])
                    col = self.visit(node.args[2])
                    val = self.visit(node.args[3])
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(
                            kind="BUFFER2D_SET", args=[buf, row, col, val], result=res
                        )
                    )
                    return res
                if node.func.attr == "matmul":
                    if len(node.args) != 2:
                        raise NotImplementedError(
                            "molt_buffer.matmul expects 2 arguments"
                        )
                    lhs = self.visit(node.args[0])
                    rhs = self.visit(node.args[1])
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(kind="BUFFER2D_MATMUL", args=[lhs, rhs], result=res)
                    )
                    return res
            elif (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "asyncio"
            ):
                if node.func.attr == "run":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="ASYNC_BLOCK_ON", args=[arg], result=res))
                    return res
                elif node.func.attr == "sleep":
                    res = MoltValue(self.next_var(), type_hint="Future")
                    self.emit(
                        MoltOp(kind="CALL_ASYNC", args=["molt_async_sleep"], result=res)
                    )
                    return res

            receiver = self.visit(node.func.value)
            if receiver is None:
                receiver = MoltValue("unknown_obj", type_hint="Unknown")
            method = node.func.attr
            if method == "append":
                if len(node.args) != 1:
                    raise NotImplementedError("list.append expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_APPEND", args=[receiver, arg], result=res))
                return res
            if method == "extend":
                if len(node.args) != 1:
                    raise NotImplementedError("list.extend expects 1 argument")
                other = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_EXTEND", args=[receiver, other], result=res)
                )
                return res
            if method == "insert":
                if len(node.args) != 2:
                    raise NotImplementedError("list.insert expects 2 arguments")
                idx = self.visit(node.args[0])
                val = self.visit(node.args[1])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_INSERT", args=[receiver, idx, val], result=res)
                )
                return res
            if method == "remove":
                if len(node.args) != 1:
                    raise NotImplementedError("list.remove expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REMOVE", args=[receiver, val], result=res))
                return res
            if method == "count" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.count expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LIST_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.index expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LIST_INDEX", args=[receiver, val], result=res))
                return res
            if method == "pop":
                if receiver.type_hint == "dict":
                    if len(node.args) not in (1, 2):
                        raise NotImplementedError("dict.pop expects 1 or 2 arguments")
                    key = self.visit(node.args[0])
                    if len(node.args) == 2:
                        default = self.visit(node.args[1])
                        has_default = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[1], result=has_default))
                    else:
                        default = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                        has_default = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[0], result=has_default))
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="DICT_POP",
                            args=[receiver, key, default, has_default],
                            result=res,
                        )
                    )
                    return res
                if len(node.args) > 1:
                    raise NotImplementedError("list.pop expects 0 or 1 argument")
                if node.args:
                    idx = self.visit(node.args[0])
                else:
                    idx = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=idx))
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="LIST_POP", args=[receiver, idx], result=res))
                return res
            if method == "get":
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("dict.get expects 1 or 2 arguments")
                key = self.visit(node.args[0])
                if len(node.args) == 2:
                    default = self.visit(node.args[1])
                else:
                    default = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(kind="DICT_GET", args=[receiver, key, default], result=res)
                )
                return res
            if method == "keys":
                res = MoltValue(self.next_var(), type_hint="dict_keys_view")
                self.emit(MoltOp(kind="DICT_KEYS", args=[receiver], result=res))
                return res
            if method == "values":
                res = MoltValue(self.next_var(), type_hint="dict_values_view")
                self.emit(MoltOp(kind="DICT_VALUES", args=[receiver], result=res))
                return res
            if method == "items":
                res = MoltValue(self.next_var(), type_hint="dict_items_view")
                self.emit(MoltOp(kind="DICT_ITEMS", args=[receiver], result=res))
                return res
            if method == "count" and receiver.type_hint == "tuple":
                if len(node.args) != 1:
                    raise NotImplementedError("tuple.count expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="TUPLE_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "tuple":
                if len(node.args) != 1:
                    raise NotImplementedError("tuple.index expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="TUPLE_INDEX", args=[receiver, val], result=res))
                return res
            if method == "count":
                if len(node.args) != 1:
                    raise NotImplementedError("count expects 1 argument")
                needle = self.visit(node.args[0])
                if (
                    receiver.type_hint in {"Any", "Unknown"}
                    and needle.type_hint == "str"
                ):
                    receiver.type_hint = "str"
                if receiver.type_hint == "str":
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="STRING_COUNT", args=[receiver, needle], result=res)
                    )
                    return res
            if method == "startswith":
                if len(node.args) != 1:
                    raise NotImplementedError("startswith expects 1 argument")
                needle = self.visit(node.args[0])
                if (
                    receiver.type_hint in {"Any", "Unknown"}
                    and needle.type_hint == "str"
                ):
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(
                            kind="STRING_STARTSWITH",
                            args=[receiver, needle],
                            result=res,
                        )
                    )
                    return res
            if method == "endswith":
                if len(node.args) != 1:
                    raise NotImplementedError("endswith expects 1 argument")
                needle = self.visit(node.args[0])
                if (
                    receiver.type_hint in {"Any", "Unknown"}
                    and needle.type_hint == "str"
                ):
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(
                            kind="STRING_ENDSWITH", args=[receiver, needle], result=res
                        )
                    )
                    return res
            if method == "join":
                if len(node.args) != 1:
                    raise NotImplementedError("join expects 1 argument")
                items = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_JOIN", args=[receiver, items], result=res)
                    )
                    return res
            if method == "format":
                if not (
                    isinstance(node.func.value, ast.Constant)
                    and isinstance(node.func.value.value, str)
                ):
                    raise NotImplementedError(
                        "format requires a string literal receiver"
                    )
                fmt_parts = self._parse_format_literal(node.func.value.value)
                fmt_values = [self.visit(arg) for arg in node.args]
                kw_values: dict[str, MoltValue] = {}
                for kw in node.keywords:
                    if kw.arg is None:
                        raise NotImplementedError("format **kwargs are not supported")
                    kw_values[kw.arg] = self.visit(kw.value)
                str_parts: list[MoltValue] = []
                for kind, value, spec in fmt_parts:
                    if kind == "text":
                        if value:
                            lit = MoltValue(self.next_var(), type_hint="str")
                            self.emit(
                                MoltOp(kind="CONST_STR", args=[value], result=lit)
                            )
                            str_parts.append(lit)
                        continue
                    if isinstance(value, int):
                        if value >= len(fmt_values):
                            raise NotImplementedError("format placeholder out of range")
                        item = fmt_values[value]
                    elif isinstance(value, str):
                        if value not in kw_values:
                            raise NotImplementedError(
                                f"format placeholder missing keyword: {value}"
                            )
                        item = kw_values[value]
                    else:
                        raise NotImplementedError(
                            "format placeholder type not supported"
                        )
                    if spec:
                        str_parts.append(self._emit_string_format(item, spec))
                    else:
                        str_parts.append(self._emit_str_from_obj(item))
                return self._emit_string_join(str_parts)
            if method == "split":
                if len(node.args) != 1:
                    raise NotImplementedError("split expects 1 argument")
                needle = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if needle.type_hint == "str":
                        receiver.type_hint = "str"
                    elif needle.type_hint == "bytearray":
                        receiver.type_hint = "bytearray"
                    elif needle.type_hint == "bytes":
                        receiver.type_hint = "bytes"
                res = MoltValue(self.next_var(), type_hint="list")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_SPLIT", args=[receiver, needle], result=res)
                    )
                    return res
                if receiver.type_hint == "bytes":
                    self.emit(
                        MoltOp(kind="BYTES_SPLIT", args=[receiver, needle], result=res)
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_SPLIT", args=[receiver, needle], result=res
                        )
                    )
                    return res
            if method == "replace":
                if len(node.args) != 2:
                    raise NotImplementedError("replace expects 2 arguments")
                old = self.visit(node.args[0])
                new = self.visit(node.args[1])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if "str" in {old.type_hint, new.type_hint}:
                        receiver.type_hint = "str"
                    elif "bytearray" in {old.type_hint, new.type_hint}:
                        receiver.type_hint = "bytearray"
                    elif "bytes" in {old.type_hint, new.type_hint}:
                        receiver.type_hint = "bytes"
                res = MoltValue(self.next_var(), type_hint=receiver.type_hint)
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(
                            kind="STRING_REPLACE",
                            args=[receiver, old, new],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytes":
                    self.emit(
                        MoltOp(
                            kind="BYTES_REPLACE",
                            args=[receiver, old, new],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_REPLACE",
                            args=[receiver, old, new],
                            result=res,
                        )
                    )
                    return res
            if method == "find":
                if len(node.args) != 1:
                    raise NotImplementedError("find expects 1 argument")
                needle = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if needle.type_hint == "str":
                        receiver.type_hint = "str"
                    elif needle.type_hint == "bytearray":
                        receiver.type_hint = "bytearray"
                    elif needle.type_hint == "bytes":
                        receiver.type_hint = "bytes"
                res = MoltValue(self.next_var(), type_hint="int")
                if receiver.type_hint == "bytes":
                    self.emit(
                        MoltOp(kind="BYTES_FIND", args=[receiver, needle], result=res)
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_FIND", args=[receiver, needle], result=res
                        )
                    )
                    return res
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_FIND", args=[receiver, needle], result=res)
                    )
                    return res

        if isinstance(node.func, ast.Name):
            func_id = node.func.id
            if func_id == "open":
                return self._bridge_fallback(
                    node,
                    "open()",
                    impact="high",
                    alternative="use molt.stdlib.io.open or molt.stdlib.io.stream",
                    detail="file I/O is capability-gated and not yet native",
                )
            if func_id == "nullcontext":
                if len(node.args) > 1:
                    raise NotImplementedError("nullcontext expects 0 or 1 argument")
                if node.args:
                    payload = self.visit(node.args[0])
                else:
                    payload = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=payload))
                return self._emit_nullcontext(payload)
            if func_id == "print":
                if len(node.args) == 0:
                    self.emit(
                        MoltOp(kind="PRINT_NEWLINE", args=[], result=MoltValue("none"))
                    )
                    return None
                if len(node.args) != 1:
                    raise NotImplementedError("print expects 0 or 1 arguments")
                arg = self.visit(node.args[0])
                if arg is None:
                    arg = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=arg))
                self.emit(MoltOp(kind="PRINT", args=[arg], result=MoltValue("none")))
                return None
            elif func_id == "molt_spawn":
                arg = self.visit(node.args[0])
                self.emit(MoltOp(kind="SPAWN", args=[arg], result=MoltValue("none")))
                return None
            elif func_id == "molt_chan_new":
                res = MoltValue(self.next_var(), type_hint="Channel")
                self.emit(MoltOp(kind="CHAN_NEW", args=[], result=res))
                return res
            elif func_id == "molt_chan_send":
                chan = self.visit(node.args[0])
                val = self.visit(node.args[1])
                self.state_count += 1
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CHAN_SEND_YIELD",
                        args=[chan, val, self.state_count],
                        result=res,
                    )
                )
                return res
            elif func_id == "molt_chan_recv":
                chan = self.visit(node.args[0])
                self.state_count += 1
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CHAN_RECV_YIELD",
                        args=[chan, self.state_count],
                        result=res,
                    )
                )
                return res
            elif func_id in self.classes:
                class_info = self.classes[func_id]
                if class_info.get("dataclass"):
                    if any(kw.arg is None for kw in node.keywords):
                        raise NotImplementedError(
                            "Dataclass **kwargs are not supported"
                        )
                    field_order = class_info["field_order"]
                    defaults = class_info["defaults"]
                    if len(node.args) > len(field_order):
                        raise NotImplementedError(
                            "Too many dataclass positional arguments"
                        )
                    field_values: list[MoltValue] = []
                    kw_values = {
                        kw.arg: self.visit(kw.value)
                        for kw in node.keywords
                        if kw.arg is not None
                    }
                    for idx, name in enumerate(field_order):
                        if idx < len(node.args):
                            val = self.visit(node.args[idx])
                            field_values.append(val)
                            continue
                        if name in kw_values:
                            field_values.append(kw_values[name])
                            continue
                        if name in defaults:
                            field_values.append(self.visit(defaults[name]))
                            continue
                        raise NotImplementedError(f"Missing dataclass field: {name}")
                    extra = set(kw_values) - set(field_order)
                    if extra:
                        raise NotImplementedError(
                            f"Unknown dataclass field(s): {', '.join(sorted(extra))}"
                        )
                    name_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[func_id], result=name_val))
                    field_name_vals: list[MoltValue] = []
                    for field in field_order:
                        field_val = MoltValue(self.next_var(), type_hint="str")
                        self.emit(
                            MoltOp(kind="CONST_STR", args=[field], result=field_val)
                        )
                        field_name_vals.append(field_val)
                    field_names_tuple = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="TUPLE_NEW",
                            args=field_name_vals,
                            result=field_names_tuple,
                        )
                    )
                    values_tuple = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="TUPLE_NEW",
                            args=field_values,
                            result=values_tuple,
                        )
                    )
                    flags = 0
                    if class_info.get("frozen"):
                        flags |= 0x1
                    if class_info.get("eq"):
                        flags |= 0x2
                    if class_info.get("repr"):
                        flags |= 0x4
                    flags_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[flags], result=flags_val))
                    res = MoltValue(self.next_var(), type_hint=func_id)
                    self.emit(
                        MoltOp(
                            kind="DATACLASS_NEW",
                            args=[name_val, field_names_tuple, values_tuple, flags_val],
                            result=res,
                        )
                    )
                    return res
                res = MoltValue(self.next_var(), type_hint=func_id)
                self.emit(MoltOp(kind="ALLOC", args=[func_id], result=res))
                return res

            # Check locals then globals
            target_info = self.locals.get(func_id) or self.globals.get(func_id)
            if target_info and str(target_info.type_hint).startswith("AsyncFunc:"):
                parts = target_info.type_hint.split(":")
                poll_func = parts[1]
                closure_size = int(parts[2])
                args = [self.visit(arg) for arg in node.args]
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(
                        kind="ALLOC_FUTURE",
                        args=[poll_func, closure_size] + args,
                        result=res,
                    )
                )
                return res

            if target_info and str(target_info.type_hint).startswith("Func:"):
                target_name = target_info.type_hint.split(":")[1]
                args = [self.visit(arg) for arg in node.args]
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CALL", args=[target_name] + args, result=res))
                return res

            if func_id == "len":
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LEN", args=[arg], result=res))
                return res
            if func_id == "str":
                if len(node.args) > 1:
                    raise NotImplementedError("str expects 0 or 1 arguments")
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[""], result=res))
                    return res
                arg = self.visit(node.args[0])
                if arg is None:
                    arg = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=arg))
                return self._emit_str_from_obj(arg)
            if func_id == "range":
                range_args = self._parse_range_call(node)
                if range_args is None:
                    raise NotImplementedError("Unsupported range invocation")
                start, stop, step = range_args
                res = MoltValue(self.next_var(), type_hint="range")
                self.emit(
                    MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=res)
                )
                return res
            if func_id == "slice":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("slice expects 1-3 arguments")
                if len(node.args) == 1:
                    start = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
                    stop = self.visit(node.args[0])
                    step = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                elif len(node.args) == 2:
                    start = self.visit(node.args[0])
                    stop = self.visit(node.args[1])
                    step = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                else:
                    start = self.visit(node.args[0])
                    stop = self.visit(node.args[1])
                    step = self.visit(node.args[2])
                res = MoltValue(self.next_var(), type_hint="slice")
                self.emit(
                    MoltOp(kind="SLICE_NEW", args=[start, stop, step], result=res)
                )
                return res
            if func_id == "list":
                if len(node.args) > 1:
                    raise NotImplementedError("list expects 0 or 1 arguments")
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="list")
                    self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
                    return res
                range_args = self._parse_range_call(node.args[0])
                if range_args is not None:
                    start, stop, step = range_args
                    range_obj = MoltValue(self.next_var(), type_hint="range")
                    self.emit(
                        MoltOp(
                            kind="RANGE_NEW",
                            args=[start, stop, step],
                            result=range_obj,
                        )
                    )
                    return self._emit_list_from_iter(range_obj)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported list input")
                return self._emit_list_from_iter(iterable)
            if func_id == "bytearray":
                if len(node.args) > 1:
                    raise NotImplementedError("bytearray expects 0 or 1 arguments")
                if node.args:
                    arg = self.visit(node.args[0])
                else:
                    arg = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(MoltOp(kind="CONST_BYTES", args=[b""], result=arg))
                res = MoltValue(self.next_var(), type_hint="bytearray")
                self.emit(MoltOp(kind="BYTEARRAY_FROM_OBJ", args=[arg], result=res))
                return res

            res = MoltValue(self.next_var(), type_hint="Unknown")
            self.emit(MoltOp(kind="CALL_DUMMY", args=[func_id], result=res))
            return res

    def visit_Subscript(self, node: ast.Subscript) -> Any:
        target = self.visit(node.value)
        if isinstance(node.slice, ast.Slice):
            lower = node.slice.lower
            upper = node.slice.upper
            step_val = node.slice.step
            if lower is None:
                start = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
            else:
                start = self.visit(lower)
            if upper is None:
                end = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
            else:
                end = self.visit(upper)
            res_type = "Any"
            if target is not None and target.type_hint in {
                "bytes",
                "bytearray",
                "list",
                "tuple",
                "str",
            }:
                res_type = target.type_hint
            if step_val is None:
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(MoltOp(kind="SLICE", args=[target, start, end], result=res))
                return res
            step = self.visit(step_val)
            slice_obj = MoltValue(self.next_var(), type_hint="slice")
            self.emit(
                MoltOp(kind="SLICE_NEW", args=[start, end, step], result=slice_obj)
            )
            res = MoltValue(self.next_var(), type_hint=res_type)
            self.emit(MoltOp(kind="INDEX", args=[target, slice_obj], result=res))
            return res
        index_val = self.visit(node.slice)
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[target, index_val], result=res))
        return res
        return None

    def visit_Attribute(self, node: ast.Attribute) -> Any:
        obj = self.visit(node.value)
        if obj is None:
            obj = MoltValue("unknown_obj", type_hint="Unknown")
        class_info = self.classes.get(obj.type_hint)
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if node.attr not in field_map:
                raise NotImplementedError(f"Unknown dataclass field: {node.attr}")
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_map[node.attr]], result=idx_val))
            res = MoltValue(self.next_var())
            self.emit(MoltOp(kind="DATACLASS_GET", args=[obj, idx_val], result=res))
            return res
        res = MoltValue(self.next_var())
        class_name = list(self.classes.keys())[-1] if self.classes else "None"
        self.emit(
            MoltOp(
                kind="GUARDED_GETATTR", args=[obj, node.attr, class_name], result=res
            )
        )
        return res

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        if not isinstance(node.target, (ast.Name, ast.Attribute)):
            raise NotImplementedError("Only simple annotated assignments are supported")
        hint = None
        if self.type_hint_policy in {"trust", "check"}:
            hint = self._annotation_to_hint(node.annotation)
            if isinstance(node.target, ast.Name) and hint is not None:
                self.explicit_type_hints[node.target.id] = hint
        if node.value is None:
            return None
        value_node = self.visit(node.value)
        if isinstance(node.target, ast.Name):
            if hint is not None:
                value_node.type_hint = hint
            self._apply_explicit_hint(node.target.id, value_node)
            if self.is_async():
                if node.target.id not in self.async_locals:
                    self.async_locals[node.target.id] = len(self.async_locals) * 8
                offset = self.async_locals[node.target.id]
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", offset, value_node],
                        result=MoltValue("none"),
                    )
                )
            else:
                if node.target.id in self.boxed_locals:
                    cell = self.boxed_locals[node.target.id]
                    idx = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=idx))
                    self.emit(
                        MoltOp(
                            kind="STORE_INDEX",
                            args=[cell, idx, value_node],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    self.locals[node.target.id] = value_node
            return None

        obj = self.visit(node.target.value)
        class_info = None
        if obj is not None:
            class_info = self.classes.get(obj.type_hint)
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if node.target.attr not in field_map:
                raise NotImplementedError(
                    f"Unknown dataclass field: {node.target.attr}"
                )
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[field_map[node.target.attr]], result=idx_val)
            )
            self.emit(
                MoltOp(
                    kind="DATACLASS_SET",
                    args=[obj, idx_val, value_node],
                    result=MoltValue("none"),
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="SETATTR",
                    args=[obj, node.target.attr, value_node],
                    result=MoltValue("none"),
                )
            )
        return None

    def visit_Assign(self, node: ast.Assign) -> None:
        value_node = self.visit(node.value)
        for target in node.targets:
            if isinstance(target, ast.Attribute):
                obj = self.visit(target.value)
                class_info = None
                if obj is not None:
                    class_info = self.classes.get(obj.type_hint)
                if class_info and class_info.get("dataclass"):
                    field_map = class_info["fields"]
                    if target.attr not in field_map:
                        raise NotImplementedError(
                            f"Unknown dataclass field: {target.attr}"
                        )
                    idx_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(
                            kind="CONST", args=[field_map[target.attr]], result=idx_val
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="DATACLASS_SET",
                            args=[obj, idx_val, value_node],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    self.emit(
                        MoltOp(
                            kind="SETATTR",
                            args=[obj, target.attr, value_node],
                            result=MoltValue("none"),
                        )
                    )
            elif isinstance(target, ast.Name):
                if self.is_async():
                    if target.id not in self.async_locals:
                        self.async_locals[target.id] = len(self.async_locals) * 8
                    offset = self.async_locals[target.id]
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", offset, value_node],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    if target.id in self.boxed_locals:
                        cell = self.boxed_locals[target.id]
                        idx = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[0], result=idx))
                        self.emit(
                            MoltOp(
                                kind="STORE_INDEX",
                                args=[cell, idx, value_node],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        self._apply_explicit_hint(target.id, value_node)
                        self.locals[target.id] = value_node
            elif isinstance(target, ast.Subscript):
                target_obj = self.visit(target.value)
                if isinstance(target.slice, ast.Slice):
                    raise NotImplementedError("Slice assignment is not supported")
                index_val = self.visit(target.slice)
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[target_obj, index_val, value_node],
                        result=MoltValue("none"),
                    )
                )
        return None

    def visit_Compare(self, node: ast.Compare) -> Any:
        left = self.visit(node.left)
        right = self.visit(node.comparators[0])
        res = MoltValue(self.next_var(), type_hint="bool")
        if isinstance(node.ops[0], ast.Lt):
            op_kind = "LT"
        elif isinstance(node.ops[0], ast.Eq):
            op_kind = "EQ"
        else:
            raise NotImplementedError("Comparison operator not supported")
        self.emit(MoltOp(kind=op_kind, args=[left, right], result=res))
        return res

    def visit_UnaryOp(self, node: ast.UnaryOp) -> Any:
        operand = self.visit(node.operand)
        if isinstance(node.op, ast.UAdd):
            return operand
        if isinstance(node.op, ast.USub):
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="SUB", args=[zero, operand], result=res))
            return res
        raise NotImplementedError("Unary operator not supported")

    def visit_If(self, node: ast.If) -> None:
        cond = self.visit(node.test)
        self.emit(MoltOp(kind="IF", args=[cond], result=MoltValue("none")))
        for item in node.body:
            self.visit(item)
        if node.orelse:
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            for item in node.orelse:
                self.visit(item)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return None

    def visit_With(self, node: ast.With) -> None:
        if self.is_async():
            self._bridge_fallback(
                node,
                "async with",
                impact="high",
                alternative="avoid async context managers or use explicit try/finally",
                detail="async with lowering is not implemented yet",
            )
            return None
        if len(node.items) != 1:
            self._bridge_fallback(
                node,
                "with (multiple context managers)",
                impact="high",
                alternative="nest with blocks",
                detail="only a single context manager is supported",
            )
            return None

        item = node.items[0]
        ctx_val = self.visit(item.context_expr)
        if ctx_val is None:
            self._bridge_fallback(
                node,
                "with",
                impact="high",
                alternative="use contextlib.nullcontext for now",
                detail="context expression did not lower",
            )
            return None

        enter_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CONTEXT_ENTER", args=[ctx_val], result=enter_val))
        if item.optional_vars is not None:
            if not isinstance(item.optional_vars, ast.Name):
                self._bridge_fallback(
                    item.optional_vars,
                    "with (destructuring targets)",
                    impact="high",
                    alternative="bind to a single name",
                    detail="only simple name targets are supported",
                )
                return None
            self._store_local_value(item.optional_vars.id, enter_val)

        for stmt in node.body:
            self.visit(stmt)

        exc_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=exc_val))
        self.emit(
            MoltOp(
                kind="CONTEXT_EXIT", args=[ctx_val, exc_val], result=MoltValue("none")
            )
        )
        return None

    def visit_For(self, node: ast.For) -> None:
        if node.orelse:
            raise NotImplementedError("for-else is not supported")
        matmul_match = self._match_matmul_loop(node)
        if matmul_match is not None:
            out_name, a_name, b_name = matmul_match
            a_val = self.locals.get(a_name) or self.globals.get(a_name)
            b_val = self.locals.get(b_name) or self.globals.get(b_name)
            if a_val is None or b_val is None:
                raise NotImplementedError("Matmul operands must be simple locals")
            res = MoltValue(self.next_var(), type_hint="buffer2d")
            self.emit(MoltOp(kind="BUFFER2D_MATMUL", args=[a_val, b_val], result=res))
            self.locals[out_name] = res
            return None
        if not isinstance(node.target, ast.Name):
            raise NotImplementedError("Only simple for targets are supported")
        assigned = self._collect_assigned_names(node.body)
        assigned.add(node.target.id)
        for name in sorted(assigned):
            if not self.is_async():
                self._box_local(name)
        range_args = self._parse_range_call(node.iter)
        if range_args is not None:
            start, stop, step = range_args
            self._emit_range_loop(node, start, stop, step)
            return None
        iterable = self.visit(node.iter)
        if iterable is None:
            raise NotImplementedError("Unsupported iterable in for loop")
        vector_info = None if self.is_async() else self._match_vector_sum_loop(node)
        if (
            vector_info
            and iterable.type_hint in {"list", "tuple"}
            and self._iterable_is_indexable(iterable)
        ):
            acc_name, _ = vector_info
            acc_val = self._load_local_value(acc_name)
            if acc_val is not None:
                zero = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                one = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[1], result=one))
                pair = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(
                    MoltOp(kind="VEC_SUM_INT", args=[iterable, acc_val], result=pair)
                )
                sum_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=sum_val))
                ok_val = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="INDEX", args=[pair, one], result=ok_val))
                self.emit(MoltOp(kind="IF", args=[ok_val], result=MoltValue("none")))
                self._store_local_value(acc_name, sum_val)
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self._emit_for_loop(node, iterable)
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                return None

        self._emit_for_loop(node, iterable)
        return None

    def visit_While(self, node: ast.While) -> None:
        if node.orelse:
            raise NotImplementedError("while-else is not supported")
        assigned = self._collect_assigned_names(node.body)
        for name in sorted(assigned):
            if not self.is_async():
                self._box_local(name)
        counted = self._match_counted_while(node)
        if counted is not None and not self.is_async():
            index_name, bound, body = counted
            self._emit_counted_while(index_name, bound, body)
            return None
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        cond = self.visit(node.test)
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        for item in node.body:
            self.visit(item)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return None

    def visit_Return(self, node: ast.Return) -> None:
        val = self.visit(node.value) if node.value else None
        if val is None:
            val = MoltValue(self.next_var())
            self.emit(MoltOp(kind="CONST", args=[0], result=val))
        self.emit(MoltOp(kind="ret", args=[val], result=MoltValue("none")))
        return None

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        if node.decorator_list:
            raise NotImplementedError("Function decorators are not supported yet")
        poll_func_name = f"{node.name}_poll"
        prev_func = self.current_func_name
        prev_async_locals = self.async_locals
        prev_hints = self.explicit_type_hints

        # Add to globals to support calls from other scopes
        self.globals[node.name] = MoltValue(
            node.name, type_hint=f"AsyncFunc:{poll_func_name}:0"
        )  # Placeholder size

        self.start_function(poll_func_name, params=["self"])
        for i, arg in enumerate(node.args.args):
            self.async_locals[arg.arg] = i * 8
            if self.type_hint_policy in {"trust", "check"}:
                hint = self._annotation_to_hint(arg.annotation)
                if hint is not None:
                    self.explicit_type_hints[arg.arg] = hint
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        if self.type_hint_policy == "check":
            for arg in node.args.args:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
        for item in node.body:
            self.visit(item)
        res = MoltValue(self.next_var())
        self.emit(MoltOp(kind="CONST", args=[0], result=res))
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        closure_size = len(self.async_locals) * 8
        self.resume_function(prev_func)
        self.async_locals = prev_async_locals
        self.explicit_type_hints = prev_hints
        # Update closure size
        self.globals[node.name] = MoltValue(
            node.name, type_hint=f"AsyncFunc:{poll_func_name}:{closure_size}"
        )
        return None

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        if node.decorator_list:
            raise NotImplementedError("Function decorators are not supported yet")
        func_name = node.name
        prev_func = self.current_func_name
        prev_hints = self.explicit_type_hints
        params = [arg.arg for arg in node.args.args]

        self.globals[func_name] = MoltValue(func_name, type_hint=f"Func:{func_name}")

        self.start_function(func_name, params=params)
        for arg in node.args.args:
            hint = None
            if self.type_hint_policy in {"trust", "check"}:
                hint = self._annotation_to_hint(arg.annotation)
                if hint is not None:
                    self.explicit_type_hints[arg.arg] = hint
            if hint is None and self.type_hint_policy in {"trust", "check"}:
                hint = "Any"
            self.locals[arg.arg] = MoltValue(arg.arg, type_hint=hint or "int")
        if self.type_hint_policy == "check":
            for arg in node.args.args:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        for item in node.body:
            self.visit(item)
        if not (self.current_ops and self.current_ops[-1].kind == "ret"):
            res = MoltValue(self.next_var())
            self.emit(MoltOp(kind="CONST", args=[0], result=res))
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        self.explicit_type_hints = prev_hints
        return None

    def visit_Await(self, node: ast.Await) -> Any:
        coro = self.visit(node.value)
        self.state_count += 1
        res = MoltValue(self.next_var())
        self.emit(
            MoltOp(kind="STATE_TRANSITION", args=[coro, self.state_count], result=res)
        )
        return res

    def map_ops_to_json(self, ops: list[MoltOp]) -> list[dict[str, Any]]:
        json_ops: list[dict[str, Any]] = []
        for op in ops:
            if op.kind == "CONST":
                json_ops.append(
                    {"kind": "const", "value": op.args[0], "out": op.result.name}
                )
            elif op.kind == "CONST_FLOAT":
                json_ops.append(
                    {
                        "kind": "const_float",
                        "f_value": op.args[0],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONST_STR":
                json_ops.append(
                    {"kind": "const_str", "s_value": op.args[0], "out": op.result.name}
                )
            elif op.kind == "CONST_BYTES":
                json_ops.append(
                    {
                        "kind": "const_bytes",
                        "bytes": list(op.args[0]),
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONST_NONE":
                json_ops.append({"kind": "const_none", "out": op.result.name})
            elif op.kind == "ADD":
                add_entry: dict[str, Any] = {
                    "kind": "add",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    add_entry["fast_int"] = True
                json_ops.append(add_entry)
            elif op.kind == "SUB":
                sub_entry: dict[str, Any] = {
                    "kind": "sub",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    sub_entry["fast_int"] = True
                json_ops.append(sub_entry)
            elif op.kind == "MUL":
                mul_entry: dict[str, Any] = {
                    "kind": "mul",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    mul_entry["fast_int"] = True
                json_ops.append(mul_entry)
            elif op.kind == "LT":
                lt_entry: dict[str, Any] = {
                    "kind": "lt",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    lt_entry["fast_int"] = True
                json_ops.append(lt_entry)
            elif op.kind == "EQ":
                eq_entry: dict[str, Any] = {
                    "kind": "eq",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    eq_entry["fast_int"] = True
                json_ops.append(eq_entry)
            elif op.kind == "IF":
                json_ops.append({"kind": "if", "args": [op.args[0].name]})
            elif op.kind == "ELSE":
                json_ops.append({"kind": "else"})
            elif op.kind == "END_IF":
                json_ops.append({"kind": "end_if"})
            elif op.kind == "CALL":
                target = op.args[0]
                json_ops.append(
                    {
                        "kind": "call",
                        "s_value": target,
                        "args": [arg.name for arg in op.args[1:]],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_NULL":
                json_ops.append(
                    {
                        "kind": "context_null",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_ENTER":
                json_ops.append(
                    {
                        "kind": "context_enter",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_EXIT":
                json_ops.append(
                    {
                        "kind": "context_exit",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PRINT":
                json_ops.append(
                    {
                        "kind": "print",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                    }
                )
            elif op.kind == "PRINT_NEWLINE":
                json_ops.append({"kind": "print_newline"})
            elif op.kind == "ALLOC":
                json_ops.append(
                    {
                        "kind": "alloc",
                        "value": self.classes[op.args[0]]["size"],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_NEW":
                json_ops.append(
                    {
                        "kind": "dataclass_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SETATTR":
                obj, attr, val = op.args
                offset = (
                    self.classes[list(self.classes.keys())[-1]]["fields"][attr] + 24
                )
                json_ops.append(
                    {"kind": "store", "args": [obj.name, val.name], "value": offset}
                )
            elif op.kind == "DATACLASS_GET":
                json_ops.append(
                    {
                        "kind": "dataclass_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_SET":
                json_ops.append(
                    {
                        "kind": "dataclass_set",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GETATTR":
                obj, attr = op.args
                offset = (
                    self.classes[list(self.classes.keys())[-1]]["fields"][attr] + 24
                )
                json_ops.append(
                    {
                        "kind": "load",
                        "args": [obj.name],
                        "value": offset,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GUARDED_GETATTR":
                obj, attr, expected_class = op.args
                offset = self.classes[expected_class]["fields"][attr] + 24
                json_ops.append(
                    {
                        "kind": "guarded_load",
                        "args": [obj.name],
                        "s_value": attr,
                        "value": offset,
                        "out": op.result.name,
                        "metadata": {"expected_type_id": 100},
                    }
                )
            elif op.kind == "GUARD_TYPE":
                json_ops.append(
                    {
                        "kind": "guard_type",
                        "args": [arg.name for arg in op.args],
                    }
                )
            elif op.kind == "JSON_PARSE":
                json_ops.append(
                    {
                        "kind": "json_parse",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MSGPACK_PARSE":
                json_ops.append(
                    {
                        "kind": "msgpack_parse",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CBOR_PARSE":
                json_ops.append(
                    {
                        "kind": "cbor_parse",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LEN":
                json_ops.append(
                    {
                        "kind": "len",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_NEW":
                json_ops.append(
                    {
                        "kind": "list_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "RANGE_NEW":
                json_ops.append(
                    {
                        "kind": "range_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_NEW":
                json_ops.append(
                    {
                        "kind": "tuple_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_APPEND":
                json_ops.append(
                    {
                        "kind": "list_append",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_POP":
                json_ops.append(
                    {
                        "kind": "list_pop",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_EXTEND":
                json_ops.append(
                    {
                        "kind": "list_extend",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_INSERT":
                json_ops.append(
                    {
                        "kind": "list_insert",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_REMOVE":
                json_ops.append(
                    {
                        "kind": "list_remove",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_COUNT":
                json_ops.append(
                    {
                        "kind": "list_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_INDEX":
                json_ops.append(
                    {
                        "kind": "list_index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "bytearray_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_NEW":
                json_ops.append(
                    {
                        "kind": "dict_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_GET":
                json_ops.append(
                    {
                        "kind": "dict_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_POP":
                json_ops.append(
                    {
                        "kind": "dict_pop",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_KEYS":
                json_ops.append(
                    {
                        "kind": "dict_keys",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_VALUES":
                json_ops.append(
                    {
                        "kind": "dict_values",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_ITEMS":
                json_ops.append(
                    {
                        "kind": "dict_items",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_COUNT":
                json_ops.append(
                    {
                        "kind": "tuple_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_INDEX":
                json_ops.append(
                    {
                        "kind": "tuple_index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ITER_NEW":
                json_ops.append(
                    {
                        "kind": "iter",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ITER_NEXT":
                json_ops.append(
                    {
                        "kind": "iter_next",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INDEX":
                json_ops.append(
                    {
                        "kind": "index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STORE_INDEX":
                json_ops.append(
                    {
                        "kind": "store_index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOOP_START":
                json_ops.append({"kind": "loop_start"})
            elif op.kind == "LOOP_INDEX_START":
                json_ops.append(
                    {
                        "kind": "loop_index_start",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOOP_INDEX_NEXT":
                json_ops.append(
                    {
                        "kind": "loop_index_next",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOOP_BREAK_IF_TRUE":
                json_ops.append(
                    {"kind": "loop_break_if_true", "args": [op.args[0].name]}
                )
            elif op.kind == "LOOP_BREAK_IF_FALSE":
                json_ops.append(
                    {"kind": "loop_break_if_false", "args": [op.args[0].name]}
                )
            elif op.kind == "LOOP_CONTINUE":
                json_ops.append({"kind": "loop_continue"})
            elif op.kind == "LOOP_END":
                json_ops.append({"kind": "loop_end"})
            elif op.kind == "VEC_SUM_INT":
                json_ops.append(
                    {
                        "kind": "vec_sum_int",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SLICE":
                json_ops.append(
                    {
                        "kind": "slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SLICE_NEW":
                json_ops.append(
                    {
                        "kind": "slice_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_FIND":
                json_ops.append(
                    {
                        "kind": "bytes_find",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_FIND":
                json_ops.append(
                    {
                        "kind": "bytearray_find",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STR_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "str_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_FIND":
                json_ops.append(
                    {
                        "kind": "string_find",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_FORMAT":
                json_ops.append(
                    {
                        "kind": "string_format",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_NEW":
                json_ops.append(
                    {
                        "kind": "buffer2d_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_GET":
                json_ops.append(
                    {
                        "kind": "buffer2d_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_SET":
                json_ops.append(
                    {
                        "kind": "buffer2d_set",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_MATMUL":
                json_ops.append(
                    {
                        "kind": "buffer2d_matmul",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_STARTSWITH":
                json_ops.append(
                    {
                        "kind": "string_startswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_ENDSWITH":
                json_ops.append(
                    {
                        "kind": "string_endswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_COUNT":
                json_ops.append(
                    {
                        "kind": "string_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_JOIN":
                json_ops.append(
                    {
                        "kind": "string_join",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_SPLIT":
                json_ops.append(
                    {
                        "kind": "string_split",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_REPLACE":
                json_ops.append(
                    {
                        "kind": "string_replace",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_SPLIT":
                json_ops.append(
                    {
                        "kind": "bytes_split",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_SPLIT":
                json_ops.append(
                    {
                        "kind": "bytearray_split",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_REPLACE":
                json_ops.append(
                    {
                        "kind": "bytes_replace",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_REPLACE":
                json_ops.append(
                    {
                        "kind": "bytearray_replace",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ASYNC_BLOCK_ON":
                json_ops.append(
                    {
                        "kind": "block_on",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_DUMMY":
                json_ops.append({"kind": "const", "value": 0, "out": op.result.name})
            elif op.kind == "BRIDGE_UNAVAILABLE":
                json_ops.append(
                    {
                        "kind": "bridge_unavailable",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ret":
                json_ops.append({"kind": "ret", "var": op.args[0].name})
            elif op.kind == "ALLOC_FUTURE":
                poll_func = op.args[0]
                size = op.args[1]
                args = op.args[2:]
                json_ops.append(
                    {
                        "kind": "alloc_future",
                        "s_value": poll_func,
                        "value": size,
                        "args": [arg.name for arg in args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STATE_SWITCH":
                json_ops.append({"kind": "state_switch"})
            elif op.kind == "SPAWN":
                json_ops.append({"kind": "spawn", "args": [op.args[0].name]})
            elif op.kind == "CHAN_NEW":
                json_ops.append({"kind": "chan_new", "out": op.result.name})
            elif op.kind == "CHAN_SEND_YIELD":
                chan, val, next_state = op.args
                json_ops.append(
                    {
                        "kind": "chan_send_yield",
                        "args": [chan.name, val.name],
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHAN_RECV_YIELD":
                chan, next_state = op.args
                json_ops.append(
                    {
                        "kind": "chan_recv_yield",
                        "args": [chan.name],
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_ASYNC":
                json_ops.append(
                    {"kind": "call_async", "s_value": op.args[0], "out": op.result.name}
                )
            elif op.kind == "LOAD_CLOSURE":
                self_ptr, offset = op.args
                json_ops.append(
                    {
                        "kind": "load",
                        "args": [self_ptr],
                        "value": offset,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STORE_CLOSURE":
                self_ptr, offset, val = op.args
                json_ops.append(
                    {"kind": "store", "args": [self_ptr, val.name], "value": offset}
                )

        if ops and ops[-1].kind != "ret":
            json_ops.append({"kind": "ret_void"})
        return json_ops

    def to_json(self) -> dict[str, Any]:
        funcs_json: list[dict[str, Any]] = []
        for name, data in self.funcs_map.items():
            funcs_json.append(
                {
                    "name": name,
                    "params": data["params"],
                    "ops": self.map_ops_to_json(data["ops"]),
                }
            )
        return {"functions": funcs_json}


def compile_to_tir(
    source: str,
    parse_codec: Literal["msgpack", "cbor", "json"] = "msgpack",
    type_hint_policy: Literal["ignore", "trust", "check"] = "ignore",
    fallback_policy: FallbackPolicy = "error",
) -> dict[str, Any]:
    tree = ast.parse(source)
    gen = SimpleTIRGenerator(
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
    )
    gen.visit(tree)
    return gen.to_json()
