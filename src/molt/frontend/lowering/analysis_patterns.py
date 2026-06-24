"""AnalysisPatternMixin: loop/comprehension pattern recognizers.

Move-only extraction from frontend/__init__.py. These helpers recognize pure
frontend AST shapes for vector reductions, counted loops, dict increments,
range comprehensions, matmul loops, and TAQ ingest loops.
"""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    cast,
)

from molt.frontend._types import MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class AnalysisPatternMixin(_MixinBase):
    @staticmethod
    def _match_optional_intrinsic_loader_expr(expr: ast.AST) -> str | None:
        if not isinstance(expr, ast.Call) or expr.keywords or len(expr.args) != 1:
            return None
        if (
            not isinstance(expr.func, ast.Name)
            or expr.func.id != "_load_optional_intrinsic"
        ):
            return None
        arg = expr.args[0]
        if not isinstance(arg, ast.Constant) or not isinstance(arg.value, str):
            return None
        return arg.value

    def _match_vector_reduction_loop(
        self, node: ast.For
    ) -> tuple[str, str, str] | None:
        if not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        target_name = node.target.id
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, (ast.Add, ast.Mult)):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if not isinstance(stmt.value, ast.Name):
                return None
            if stmt.value.id != target_name:
                return None
            if stmt.target.id == target_name:
                return None
            kind = "sum" if isinstance(stmt.op, ast.Add) else "prod"
            return (stmt.target.id, target_name, kind)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            dest = stmt.targets[0].id
            if dest == target_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, (ast.Add, ast.Mult)
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if isinstance(left, ast.Name) and left.id == dest:
                if isinstance(right, ast.Name) and right.id == target_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, target_name, kind)
            if isinstance(right, ast.Name) and right.id == dest:
                if isinstance(left, ast.Name) and left.id == target_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, target_name, kind)
        return None

    def _match_indexed_vector_reduction_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        idx_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Call):
            return None
        if not isinstance(node.iter.func, ast.Name) or node.iter.func.id != "range":
            return None
        args = node.iter.args
        if not args or len(args) > 3:
            return None
        start = None
        stop = None
        step = None
        if len(args) == 1:
            stop = args[0]
            step = ast.Constant(value=1)
        elif len(args) == 2:
            start = args[0]
            stop = args[1]
            step = ast.Constant(value=1)
        else:
            start = args[0]
            stop = args[1]
            step = args[2]
        start_expr = None
        if start is not None:
            if isinstance(start, ast.Constant):
                if not isinstance(start.value, int) or start.value < 0:
                    return None
                if start.value > 0:
                    start_expr = start
            else:
                start_expr = self._range_start_expr(start)
                if start_expr is None:
                    return None
        if not isinstance(step, ast.Constant) or step.value != 1:
            return None
        if not isinstance(stop, ast.Call):
            return None
        if not isinstance(stop.func, ast.Name) or stop.func.id != "len":
            return None
        if len(stop.args) != 1 or not isinstance(stop.args[0], ast.Name):
            return None
        seq_name = stop.args[0].id
        stmt = node.body[0]
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, (ast.Add, ast.Mult)):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if stmt.target.id == idx_name:
                return None
            if not self._subscript_matches(stmt.value, seq_name, idx_name):
                return None
            kind = "sum" if isinstance(stmt.op, ast.Add) else "prod"
            return (stmt.target.id, seq_name, kind, start_expr)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            dest = stmt.targets[0].id
            if dest == idx_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, (ast.Add, ast.Mult)
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if isinstance(left, ast.Name) and left.id == dest:
                if self._subscript_matches(right, seq_name, idx_name):
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, seq_name, kind, start_expr)
            if isinstance(right, ast.Name) and right.id == dest:
                if self._subscript_matches(left, seq_name, idx_name):
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, seq_name, kind, start_expr)
        return None

    def _match_indexed_vector_minmax_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        idx_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Call):
            return None
        if not isinstance(node.iter.func, ast.Name) or node.iter.func.id != "range":
            return None
        args = node.iter.args
        if not args or len(args) > 3:
            return None
        start = None
        stop = None
        step = None
        if len(args) == 1:
            stop = args[0]
            step = ast.Constant(value=1)
        elif len(args) == 2:
            start = args[0]
            stop = args[1]
            step = ast.Constant(value=1)
        else:
            start = args[0]
            stop = args[1]
            step = args[2]
        start_expr = None
        if start is not None:
            if isinstance(start, ast.Constant):
                if not isinstance(start.value, int) or start.value < 0:
                    return None
                if start.value > 0:
                    start_expr = start
            else:
                start_expr = self._range_start_expr(start)
                if start_expr is None:
                    return None
        if not isinstance(step, ast.Constant) or step.value != 1:
            return None
        if not isinstance(stop, ast.Call):
            return None
        if not isinstance(stop.func, ast.Name) or stop.func.id != "len":
            return None
        if len(stop.args) != 1 or not isinstance(stop.args[0], ast.Name):
            return None
        seq_name = stop.args[0].id
        stmt = node.body[0]
        if not isinstance(stmt, ast.If) or stmt.orelse:
            return None
        if len(stmt.body) != 1:
            return None
        assign = stmt.body[0]
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        acc_name = assign.targets[0].id
        if acc_name == idx_name:
            return None
        if not self._subscript_matches(assign.value, seq_name, idx_name):
            return None
        test = stmt.test
        if not isinstance(test, ast.Compare):
            return None
        if len(test.ops) != 1 or len(test.comparators) != 1:
            return None
        op = test.ops[0]
        left = test.left
        right = test.comparators[0]
        left_is_acc = isinstance(left, ast.Name) and left.id == acc_name
        right_is_acc = isinstance(right, ast.Name) and right.id == acc_name
        left_is_item = self._subscript_matches(left, seq_name, idx_name)
        right_is_item = self._subscript_matches(right, seq_name, idx_name)
        if not ((left_is_acc and right_is_item) or (left_is_item and right_is_acc)):
            return None
        if isinstance(op, ast.Lt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "min", start_expr
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "max", start_expr
        if isinstance(op, ast.Gt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "max", start_expr
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "min", start_expr
        return None

    def _match_iter_vector_reduction_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        item_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Name):
            return None
        seq_name = node.iter.id
        stmt = node.body[0]
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, (ast.Add, ast.Mult)):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if stmt.target.id == item_name:
                return None
            if isinstance(stmt.value, ast.Name) and stmt.value.id == item_name:
                kind = "sum" if isinstance(stmt.op, ast.Add) else "prod"
                return (stmt.target.id, seq_name, kind, None)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            acc_name = stmt.targets[0].id
            if acc_name == item_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, (ast.Add, ast.Mult)
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if isinstance(left, ast.Name) and left.id == acc_name:
                if isinstance(right, ast.Name) and right.id == item_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (acc_name, seq_name, kind, None)
            if isinstance(right, ast.Name) and right.id == acc_name:
                if isinstance(left, ast.Name) and left.id == item_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (acc_name, seq_name, kind, None)
        return None

    def _match_iter_vector_minmax_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        item_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Name):
            return None
        seq_name = node.iter.id
        stmt = node.body[0]
        if not isinstance(stmt, ast.If) or stmt.orelse:
            return None
        if len(stmt.body) != 1:
            return None
        assign = stmt.body[0]
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        acc_name = assign.targets[0].id
        if acc_name == item_name:
            return None
        if not isinstance(assign.value, ast.Name) or assign.value.id != item_name:
            return None
        test = stmt.test
        if not isinstance(test, ast.Compare):
            return None
        if len(test.ops) != 1 or len(test.comparators) != 1:
            return None
        op = test.ops[0]
        left = test.left
        right = test.comparators[0]
        left_is_acc = isinstance(left, ast.Name) and left.id == acc_name
        right_is_acc = isinstance(right, ast.Name) and right.id == acc_name
        left_is_item = isinstance(left, ast.Name) and left.id == item_name
        right_is_item = isinstance(right, ast.Name) and right.id == item_name
        if not ((left_is_acc and right_is_item) or (left_is_item and right_is_acc)):
            return None
        if isinstance(op, ast.Lt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "min", None
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "max", None
        if isinstance(op, ast.Gt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "max", None
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "min", None
        return None

    def _match_vector_minmax_loop(self, node: ast.For) -> tuple[str, str, str] | None:
        if not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        if not isinstance(stmt, ast.If) or stmt.orelse:
            return None
        if len(stmt.body) != 1:
            return None
        assign = stmt.body[0]
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        acc_name = assign.targets[0].id
        item_name = node.target.id
        if acc_name == item_name:
            return None
        if not isinstance(assign.value, ast.Name) or assign.value.id != item_name:
            return None
        test = stmt.test
        if not isinstance(test, ast.Compare):
            return None
        if len(test.ops) != 1 or len(test.comparators) != 1:
            return None
        op = test.ops[0]
        left = test.left
        right = test.comparators[0]
        if not isinstance(left, ast.Name) or not isinstance(right, ast.Name):
            return None
        if {left.id, right.id} != {item_name, acc_name}:
            return None
        if isinstance(op, ast.Lt):
            if left.id == item_name and right.id == acc_name:
                return acc_name, item_name, "min"
            if left.id == acc_name and right.id == item_name:
                return acc_name, item_name, "max"
        if isinstance(op, ast.Gt):
            if left.id == item_name and right.id == acc_name:
                return acc_name, item_name, "max"
            if left.id == acc_name and right.id == item_name:
                return acc_name, item_name, "min"
        return None

    def _match_simple_range_list_comp(
        self, node: ast.ListComp
    ) -> tuple[MoltValue, MoltValue, MoltValue] | None:
        if len(node.generators) != 1:
            return None
        comp = node.generators[0]
        if comp.is_async or comp.ifs:
            return None
        if not isinstance(comp.target, ast.Name):
            return None
        if not isinstance(node.elt, ast.Name) or node.elt.id != comp.target.id:
            return None
        parsed = self._parse_range_call(comp.iter)
        if parsed is None:
            return None
        start, stop, step, _ = parsed
        return start, stop, step

    def _match_const_int_range_list_comp(self, node: ast.ListComp) -> int | None:
        if len(node.generators) != 1:
            return None
        comp = node.generators[0]
        if comp.is_async or comp.ifs:
            return None
        if not isinstance(comp.target, ast.Name):
            return None
        if not isinstance(node.elt, ast.Constant):
            return None
        value = node.elt.value
        if not isinstance(value, int) or isinstance(value, bool):
            return None
        if not isinstance(comp.iter, ast.Call):
            return None
        if not isinstance(comp.iter.func, ast.Name) or comp.iter.func.id != "range":
            return None
        if len(comp.iter.args) > 3 or comp.iter.keywords:
            return None
        return int(value)

    def _match_const_range_list_comp(self, node: ast.ListComp) -> ast.Constant | None:
        if len(node.generators) != 1:
            return None
        comp = node.generators[0]
        if comp.is_async or comp.ifs:
            return None
        if not isinstance(comp.target, ast.Name):
            return None
        if not isinstance(node.elt, ast.Constant):
            return None
        value = node.elt.value
        if isinstance(value, int) and not isinstance(value, bool):
            return None
        if not isinstance(comp.iter, ast.Call):
            return None
        if not isinstance(comp.iter.func, ast.Name) or comp.iter.func.id != "range":
            return None
        if len(comp.iter.args) > 3 or comp.iter.keywords:
            return None
        return node.elt

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
        bound_value = self._const_int_from_expr(node.test.comparators[0])
        if bound_value is None:
            return None
        if not node.body:
            return None
        index_name = node.test.left.id
        incr_stmt = node.body[-1]
        if not self._is_unit_increment(incr_stmt, index_name):
            return None
        if index_name in self._collect_assigned_names(node.body[:-1]):
            return None
        return index_name, bound_value, node.body[:-1]

    def _match_bytearray_fill_counted_while(
        self, index_name: str, bound: int, body: list[ast.stmt]
    ) -> tuple[str, int, int, int] | None:
        if len(body) != 1:
            return None
        stmt = body[0]
        if not isinstance(stmt, ast.Assign) or len(stmt.targets) != 1:
            return None
        target = stmt.targets[0]
        if not isinstance(target, ast.Subscript):
            return None
        if not isinstance(target.value, ast.Name):
            return None
        if not isinstance(target.slice, ast.Name) or target.slice.id != index_name:
            return None
        container_name = target.value.id
        container = self.locals.get(container_name)
        if container is None or container.type_hint != "bytearray":
            return None
        bytearray_len = self._bytearray_len_hint_for(container_name, container)
        if bytearray_len is None:
            return None
        start = self._const_int_for_local(index_name)
        if start is None or start < 0 or bound <= start or bound > bytearray_len:
            return None
        fill = self._const_int_from_expr(stmt.value)
        if fill is None or not 0 <= fill <= 255:
            return None
        return container_name, start, bound, fill

    def _match_counted_while_sum(
        self, index_name: str, body: list[ast.stmt]
    ) -> str | None:
        if len(body) != 1:
            return None
        stmt = body[0]
        if isinstance(stmt, ast.AugAssign):
            if (
                isinstance(stmt.op, ast.Add)
                and isinstance(stmt.target, ast.Name)
                and isinstance(stmt.value, ast.Name)
                and stmt.value.id == index_name
            ):
                return stmt.target.id
            return None
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            acc_name = stmt.targets[0].id
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if (
                isinstance(left, ast.Name)
                and isinstance(right, ast.Name)
                and (
                    {left.id, right.id} == {acc_name, index_name}
                    and left.id != right.id
                )
            ):
                return acc_name
        return None

    def _match_const_increment(self, stmt: ast.stmt) -> tuple[str, int] | None:
        if isinstance(stmt, ast.AugAssign):
            if (
                isinstance(stmt.op, ast.Add)
                and isinstance(stmt.target, ast.Name)
                and isinstance(stmt.value, ast.Constant)
                and isinstance(stmt.value.value, int)
            ):
                return stmt.target.id, stmt.value.value
            return None
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            acc_name = stmt.targets[0].id
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if (
                isinstance(left, ast.Name)
                and left.id == acc_name
                and isinstance(right, ast.Constant)
                and isinstance(right.value, int)
            ):
                return acc_name, right.value
            if (
                isinstance(right, ast.Name)
                and right.id == acc_name
                and isinstance(left, ast.Constant)
                and isinstance(left.value, int)
            ):
                return acc_name, left.value
        return None

    def _match_counted_while_const_increment(
        self, body: list[ast.stmt]
    ) -> tuple[str, int] | None:
        if len(body) == 1:
            return self._match_const_increment(body[0])
        if len(body) != 2:
            return None
        init, inner = body
        if not isinstance(init, ast.Assign):
            return None
        if len(init.targets) != 1 or not isinstance(init.targets[0], ast.Name):
            return None
        if not isinstance(init.value, ast.Constant) or not isinstance(
            init.value.value, int
        ):
            return None
        if not isinstance(inner, ast.While):
            return None
        inner_match = self._match_counted_while(inner)
        if inner_match is None:
            return None
        inner_index, inner_bound, inner_body = inner_match
        if inner_index != init.targets[0].id:
            return None
        inner_inc = self._match_counted_while_const_increment(inner_body)
        if inner_inc is None:
            return None
        acc_name, delta = inner_inc
        start_val = init.value.value
        if start_val >= inner_bound:
            return acc_name, 0
        return acc_name, (inner_bound - start_val) * delta

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

    def _match_dict_increment_assign(
        self, node: ast.Assign
    ) -> tuple[ast.expr, ast.expr, ast.expr] | None:
        if len(node.targets) != 1:
            return None
        target = node.targets[0]
        if not isinstance(target, ast.Subscript) or isinstance(target.slice, ast.Slice):
            return None
        if not isinstance(target.value, ast.Name):
            return None
        target_key = target.slice
        if not self._dict_increment_key_is_single_eval_safe(target_key):
            return None
        if not isinstance(node.value, ast.BinOp) or not isinstance(
            node.value.op, ast.Add
        ):
            return None

        dict_name = target.value.id
        key_dump = ast.dump(target_key, include_attributes=False)

        def is_matching_get(expr: ast.expr) -> bool:
            if not isinstance(expr, ast.Call) or expr.keywords:
                return False
            if not isinstance(expr.func, ast.Attribute) or expr.func.attr != "get":
                return False
            if (
                not isinstance(expr.func.value, ast.Name)
                or expr.func.value.id != dict_name
            ):
                return False
            if len(expr.args) == 1:
                key_expr = expr.args[0]
                default_expr: ast.expr = ast.Constant(value=0)
            elif len(expr.args) == 2:
                key_expr, default_expr = expr.args
            else:
                return False
            if ast.dump(key_expr, include_attributes=False) != key_dump:
                return False
            return (
                isinstance(default_expr, ast.Constant)
                and isinstance(default_expr.value, int)
                and default_expr.value == 0
            )

        if is_matching_get(node.value.left):
            delta_expr = node.value.right
        elif is_matching_get(node.value.right):
            delta_expr = node.value.left
        else:
            return None
        return target.value, target_key, delta_expr

    def _match_split_dict_increment_for_loop(
        self, node: ast.For
    ) -> tuple[ast.expr, ast.expr, ast.expr | None, ast.expr] | None:
        if self.is_async():
            return None
        if not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1 or not isinstance(node.body[0], ast.Assign):
            return None
        iter_call = node.iter
        if not isinstance(iter_call, ast.Call) or iter_call.keywords:
            return None
        if (
            not isinstance(iter_call.func, ast.Attribute)
            or iter_call.func.attr != "split"
        ):
            return None
        if len(iter_call.args) > 1:
            return None
        assign = node.body[0]
        match = self._match_dict_increment_assign(assign)
        if match is None:
            return None
        dict_expr, key_expr, delta_expr = match
        if not isinstance(key_expr, ast.Name) or key_expr.id != node.target.id:
            return None
        if not isinstance(dict_expr, ast.Name):
            return None
        sep_expr: ast.expr | None = None
        if iter_call.args:
            sep_expr = iter_call.args[0]
        return dict_expr, iter_call.func.value, sep_expr, delta_expr

    def _match_taq_ingest_loop_body(
        self, body: list[ast.stmt]
    ) -> tuple[str | None, str, str, str, ast.expr] | None:
        if self.is_async():
            return None
        idx = 0
        header_name: str | None = None
        if body:
            header_name = self._is_taq_header_guard(body[0])
            if header_name is not None:
                idx = 1
        rest = body[idx:]
        if len(rest) != 7:
            return None

        split_stmt = rest[0]
        guard_stmt = rest[1]
        ts_stmt = rest[2]
        sym_stmt = rest[3]
        vol_stmt = rest[4]
        setdefault_stmt = rest[5]
        append_stmt = rest[6]

        if not isinstance(split_stmt, ast.Assign):
            return None
        if len(split_stmt.targets) != 1 or not isinstance(
            split_stmt.targets[0], ast.Name
        ):
            return None
        split_name = split_stmt.targets[0].id
        if not isinstance(split_stmt.value, ast.Call) or split_stmt.value.keywords:
            return None
        split_call = split_stmt.value
        if len(split_call.args) != 1:
            return None
        if (
            not isinstance(split_call.func, ast.Attribute)
            or split_call.func.attr != "split"
        ):
            return None
        if not isinstance(split_call.func.value, ast.Name):
            return None
        line_name = split_call.func.value.id
        if not (
            isinstance(split_call.args[0], ast.Constant)
            and split_call.args[0].value == "|"
        ):
            return None

        def match_sub(name: str, index: int, expr: ast.expr) -> bool:
            return (
                isinstance(expr, ast.Subscript)
                and not isinstance(expr.slice, ast.Slice)
                and isinstance(expr.value, ast.Name)
                and expr.value.id == name
                and isinstance(expr.slice, ast.Constant)
                and expr.slice.value == index
            )

        if not isinstance(guard_stmt, ast.If):
            return None
        if guard_stmt.orelse or len(guard_stmt.body) != 1:
            return None
        if not isinstance(guard_stmt.body[0], ast.Continue):
            return None
        guard_test = guard_stmt.test
        if not isinstance(guard_test, ast.BoolOp) or not isinstance(
            guard_test.op, ast.Or
        ):
            return None
        if len(guard_test.values) != 2:
            return None
        checks: set[tuple[int, str]] = set()
        for clause in guard_test.values:
            if not isinstance(clause, ast.Compare):
                return None
            if len(clause.ops) != 1 or not isinstance(clause.ops[0], ast.Eq):
                return None
            if len(clause.comparators) != 1:
                return None
            rhs = clause.comparators[0]
            if not isinstance(rhs, ast.Constant) or not isinstance(rhs.value, str):
                return None
            if not isinstance(clause.left, ast.Subscript):
                return None
            if not isinstance(clause.left.value, ast.Name):
                return None
            if clause.left.value.id != split_name:
                return None
            if not isinstance(clause.left.slice, ast.Constant):
                return None
            idx_val = clause.left.slice.value
            if not isinstance(idx_val, int):
                return None
            if idx_val not in (0, 4):
                return None
            checks.add((idx_val, rhs.value))
        if checks != {(0, "END"), (4, "ENDP")}:
            return None

        if not isinstance(ts_stmt, ast.Assign):
            return None
        if len(ts_stmt.targets) != 1 or not isinstance(ts_stmt.targets[0], ast.Name):
            return None
        ts_name = ts_stmt.targets[0].id
        if (
            not isinstance(ts_stmt.value, ast.Call)
            or ts_stmt.value.keywords
            or len(ts_stmt.value.args) != 1
            or not isinstance(ts_stmt.value.func, ast.Name)
            or ts_stmt.value.func.id != "int"
            or not match_sub(split_name, 0, ts_stmt.value.args[0])
        ):
            return None

        if not isinstance(sym_stmt, ast.Assign):
            return None
        if len(sym_stmt.targets) != 1 or not isinstance(sym_stmt.targets[0], ast.Name):
            return None
        sym_name = sym_stmt.targets[0].id
        if not match_sub(split_name, 2, sym_stmt.value):
            return None

        if not isinstance(vol_stmt, ast.Assign):
            return None
        if len(vol_stmt.targets) != 1 or not isinstance(vol_stmt.targets[0], ast.Name):
            return None
        vol_name = vol_stmt.targets[0].id
        if (
            not isinstance(vol_stmt.value, ast.Call)
            or vol_stmt.value.keywords
            or len(vol_stmt.value.args) != 1
            or not isinstance(vol_stmt.value.func, ast.Name)
            or vol_stmt.value.func.id != "int"
            or not match_sub(split_name, 4, vol_stmt.value.args[0])
        ):
            return None

        if not isinstance(setdefault_stmt, ast.Assign):
            return None
        if len(setdefault_stmt.targets) != 1 or not isinstance(
            setdefault_stmt.targets[0], ast.Name
        ):
            return None
        series_name = setdefault_stmt.targets[0].id
        if (
            not isinstance(setdefault_stmt.value, ast.Call)
            or setdefault_stmt.value.keywords
            or len(setdefault_stmt.value.args) != 2
            or not isinstance(setdefault_stmt.value.func, ast.Attribute)
            or setdefault_stmt.value.func.attr != "setdefault"
            or not isinstance(setdefault_stmt.value.func.value, ast.Name)
            or not isinstance(setdefault_stmt.value.args[0], ast.Name)
            or not isinstance(setdefault_stmt.value.args[1], ast.List)
            or setdefault_stmt.value.args[1].elts
        ):
            return None
        data_name = setdefault_stmt.value.func.value.id
        if setdefault_stmt.value.args[0].id != sym_name:
            return None

        if not isinstance(append_stmt, ast.Expr) or not isinstance(
            append_stmt.value, ast.Call
        ):
            return None
        append_call = append_stmt.value
        if (
            append_call.keywords
            or len(append_call.args) != 1
            or not isinstance(append_call.func, ast.Attribute)
            or append_call.func.attr != "append"
            or not isinstance(append_call.func.value, ast.Name)
            or append_call.func.value.id != series_name
        ):
            return None
        arg0 = append_call.args[0]
        if not isinstance(arg0, ast.Tuple) or len(arg0.elts) != 2:
            return None
        first, second = arg0.elts
        if not (
            isinstance(first, ast.BinOp)
            and isinstance(first.op, ast.FloorDiv)
            and isinstance(first.left, ast.Name)
            and first.left.id == ts_name
            and isinstance(second, ast.Name)
            and second.id == vol_name
        ):
            return None

        return header_name, data_name, line_name, split_name, first.right
