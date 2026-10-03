"""AnalysisPatternMixin: loop/comprehension pattern recognizers.

Move-only extraction from frontend/__init__.py. These helpers recognize
frontend AST shapes for vector reductions, counted loops, dict increments and
range comprehensions. A recognizer whose lowering reads a binding once, where
Python reads it inside the loop, also requires the binding analysis to prove
that source read clean, or bound where an early read must not raise.
Recognizers only select a shape; the fused lowering's runtime kernel or guard
admits it on the values the loop reads. Recognizers emit nothing.
"""

from __future__ import annotations

import ast

from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend._types import MoltValue


class AnalysisPatternMixin(GeneratorMixinBase):
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
        return self._parse_range_call(comp.iter)

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
        if self._specializable_builtin_name(comp.iter) != "range":
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
        if self._specializable_builtin_name(comp.iter) != "range":
            return None
        if len(comp.iter.args) > 3 or comp.iter.keywords:
            return None
        return node.elt

    def _match_counted_while(
        self, node: ast.While
    ) -> tuple[ast.Name, int, list[ast.stmt]] | None:
        """``while i < BOUND: ...; i += 1``: the test's read of the index, BOUND
        and the body before the increment.

        The counted lowering keeps the index in an induction variable, so the
        increment's own read of it must be proven clean: nothing in the body may
        have rebound it, including a callback through a frame proxy (3.13+).
        BOUND is an int literal or the analysis's constant for the test's read.
        The lowering guards the start's exact int type at run time and keeps the
        ordinary loop beside the counted one, so a body holding another
        ``while`` is left to the ordinary lowering rather than copied again.
        """
        if node.orelse:
            return None
        if not isinstance(node.test, ast.Compare):
            return None
        if len(node.test.ops) != 1 or not isinstance(node.test.ops[0], ast.Lt):
            return None
        index = node.test.left
        if not isinstance(index, ast.Name):
            return None
        if len(node.test.comparators) != 1:
            return None
        bound_value = self._const_int_from_expr(node.test.comparators[0])
        if bound_value is None:
            return None
        if not node.body:
            return None
        increment = self._unit_increment_read(node.body[-1], index.id)
        if increment is None:
            return None
        if index.id in self._collect_assigned_names(node.body[:-1]):
            return None
        if any(
            isinstance(child, ast.While)
            for stmt in node.body
            for child in ast.walk(stmt)
        ):
            return None
        if self._expression_has_invalidated_binding(increment) is not False:
            return None
        return index, bound_value, node.body[:-1]

    def _match_bytearray_fill_counted_while(
        self, index: ast.Name, bound: int, body: list[ast.stmt]
    ) -> tuple[ast.Name, int] | None:
        """``buf[i] = FILL`` under a counted ``while``: the container's read and
        the fill byte.

        The cached type and length of ``buf`` only select the shape; the fused
        fill's own guard admits it on the container and index the loop reads.
        """
        if len(body) != 1:
            return None
        stmt = body[0]
        if not isinstance(stmt, ast.Assign) or len(stmt.targets) != 1:
            return None
        target = stmt.targets[0]
        if not isinstance(target, ast.Subscript):
            return None
        container_read = target.value
        if not isinstance(container_read, ast.Name) or container_read.id == index.id:
            return None
        if not isinstance(target.slice, ast.Name) or target.slice.id != index.id:
            return None
        container = self.locals.get(container_read.id)
        if container is None or container.type_hint != "bytearray":
            return None
        bytearray_len = self._bytearray_len_hint_for(container_read.id, container)
        if bytearray_len is None or bound > bytearray_len:
            return None
        fill = self._const_int_from_expr(stmt.value)
        if fill is None or not 0 <= fill <= 255:
            return None
        return container_read, fill

    def _match_dict_increment_assign(
        self, node: ast.Assign
    ) -> tuple[ast.Name, ast.expr, ast.expr] | None:
        """``d[k] = d.get(k, 0) + delta``: the read of ``d``, the key and delta.

        Only this operand order: ``delta + d.get(k, 0)`` calls ``delta``'s
        ``__add__`` first, so it is a different statement. ``k`` must read the
        same way twice (a name, a constant or an exact dataclass field).
        """
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
        get = node.value.left
        if not isinstance(get, ast.Call) or get.keywords:
            return None
        if not isinstance(get.func, ast.Attribute) or get.func.attr != "get":
            return None
        if not isinstance(get.func.value, ast.Name) or get.func.value.id != dict_name:
            return None
        if len(get.args) != 2:
            return None
        key_expr, default_expr = get.args
        if ast.dump(key_expr, include_attributes=False) != key_dump:
            return None
        if not (
            isinstance(default_expr, ast.Constant)
            and type(default_expr.value) is int
            and default_expr.value == 0
        ):
            return None
        return target.value, target_key, node.value.right

    def _match_split_dict_increment_for_loop(
        self, node: ast.For
    ) -> tuple[ast.Name, ast.Name, str | None, ast.expr] | None:
        """``for w in line.split([SEP]): d[w] = d.get(w, 0) + delta``: the reads
        of ``d`` and ``line``, SEP and ``delta``.

        The fused op reads ``d`` and ``delta`` once, before the loop, where the
        loop reads them per word. That is unobservable only when neither can
        raise there (the analysis proves both bound) and the loop cannot rebind
        either (neither is the target; the kernel runs no Python code). SEP is
        absent or a str literal, ``delta`` an int literal or a name. A function
        body only: the target is a frame binding, stored once, after the fact.
        """
        if (
            self.is_async()
            or self.current_func_name == "molt_main"
            or self._class_ns_stack
            or not isinstance(node.target, ast.Name)
        ):
            return None
        if len(node.body) != 1 or not isinstance(node.body[0], ast.Assign):
            return None
        iter_call = node.iter
        if not isinstance(iter_call, ast.Call) or iter_call.keywords:
            return None
        if (
            not isinstance(iter_call.func, ast.Attribute)
            or iter_call.func.attr != "split"
            or not isinstance(iter_call.func.value, ast.Name)
            or len(iter_call.args) > 1
        ):
            return None
        sep: str | None = None
        if iter_call.args:
            sep_expr = iter_call.args[0]
            if (
                not isinstance(sep_expr, ast.Constant)
                or type(sep_expr.value) is not str
            ):
                return None
            sep = sep_expr.value
        match = self._match_dict_increment_assign(node.body[0])
        if match is None:
            return None
        dict_read, key_expr, delta_expr = match
        target = node.target.id
        if not isinstance(key_expr, ast.Name) or key_expr.id != target:
            return None
        if dict_read.id == target or not self._name_read_definitely_bound(dict_read):
            return None
        if isinstance(delta_expr, ast.Name):
            if delta_expr.id == target or not self._name_read_definitely_bound(
                delta_expr
            ):
                return None
        elif not (
            isinstance(delta_expr, ast.Constant)
            and type(delta_expr.value) in {int, bool}
        ):
            return None
        return dict_read, iter_call.func.value, sep, delta_expr
