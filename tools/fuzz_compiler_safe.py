from __future__ import annotations

from random import Random

from tools.fuzz_compiler_types import (
    T_ANY,
    T_BOOL,
    T_DICT,
    T_FLOAT,
    T_INT,
    T_LIST_INT,
    T_LIST_STR,
    T_NONE,
    T_SET_INT,
    T_STR,
    T_TUPLE,
)

# ---------------------------------------------------------------------------
# Safe program generator (type-tracked)
# ---------------------------------------------------------------------------


class SafeProgramGenerator:
    """Generates random valid Python 3.12+ programs with full type tracking.

    Every variable tracks its type.  Expressions are generated to produce a
    specific type, so arithmetic never touches strings, ordering comparisons
    never mix types, and undefined variables are never referenced.
    """

    STR_LITERALS = [
        "hello",
        "world",
        "foo",
        "bar",
        "baz",
        "molt",
        "test",
        "abc",
        "XYZ",
        "  spaced  ",
        "123",
        "a b c",
    ]
    INT_RANGE = (-200, 200)
    FLOAT_RANGE = (-50.0, 50.0)

    def __init__(self, rng: Random, *, max_depth: int = 4, max_stmts: int = 20):
        self.rng = rng
        self.max_depth = max_depth
        self.max_stmts = max_stmts
        self._var_counter = 0
        self._func_counter = 0
        self._defined_vars: list[tuple[str, str]] = []  # (name, type_tag)
        self._scope_stack: list[int] = []  # save points
        self._defined_funcs: list[str] = []
        self._defined_classes: list[tuple] = []
        self._defined_closures: list[tuple[str, str]] = []
        self._defined_kwonly_funcs: list[tuple] = []
        self._defined_starargs_funcs: list[tuple[str, str]] = []

    # -- Scope management ---------------------------------------------------

    def _push_scope(self):
        """Save current variable state before entering a block."""
        self._scope_stack.append(len(self._defined_vars))

    def _pop_scope(self):
        """Restore variable state after leaving a block."""
        restore_point = self._scope_stack.pop()
        self._defined_vars = self._defined_vars[:restore_point]

    # -- Variable helpers ---------------------------------------------------

    def _fresh_var(self) -> str:
        name = f"v{self._var_counter}"
        self._var_counter += 1
        return name

    def _fresh_func(self) -> str:
        bases = ["compute", "transform", "helper", "process", "calc", "combine"]
        base = self.rng.choice(bases)
        name = f"{base}_{self._func_counter}"
        self._func_counter += 1
        return name

    def _add_var(self, name: str, type_tag: str):
        self._defined_vars.append((name, type_tag))

    def _known_var_of_type(self, *type_tags: str) -> str | None:
        """Get a defined variable matching one of the given type tags."""
        candidates = [(n, t) for n, t in self._defined_vars if t in type_tags]
        if not candidates:
            return None
        return self.rng.choice(candidates)[0]

    def _any_known_var(self) -> tuple[str, str] | None:
        """Get any defined variable as (name, type_tag)."""
        if not self._defined_vars:
            return None
        return self.rng.choice(self._defined_vars)

    # -- Literal generators -------------------------------------------------

    def gen_int_literal(self) -> str:
        return str(self.rng.randint(*self.INT_RANGE))

    def gen_float_literal(self) -> str:
        val = self.rng.uniform(*self.FLOAT_RANGE)
        return f"{val:.4f}"

    def gen_bool_literal(self) -> str:
        return "True" if self.rng.random() < 0.5 else "False"

    def gen_str_literal(self) -> str:
        s = self.rng.choice(self.STR_LITERALS)
        return repr(s)

    def gen_none_literal(self) -> str:
        return "None"

    # -- Typed expression generators ----------------------------------------

    def gen_typed_expr(self, depth: int, target_type: str) -> str:
        """Generate an expression guaranteed to produce the given type."""
        if target_type == T_INT:
            return self._gen_int_expr(depth)
        elif target_type == T_FLOAT:
            return self._gen_float_expr(depth)
        elif target_type == T_STR:
            return self._gen_str_expr(depth)
        elif target_type == T_BOOL:
            return self._gen_bool_expr(depth)
        elif target_type == T_LIST_INT:
            return self._gen_list_int_expr(depth)
        elif target_type == T_LIST_STR:
            return self._gen_list_str_expr(depth)
        elif target_type == T_TUPLE:
            return self._gen_tuple_expr(depth)
        elif target_type == T_DICT:
            return self._gen_dict_expr(depth)
        elif target_type == T_SET_INT:
            return self._gen_set_int_expr(depth)
        elif target_type == T_NONE:
            return "None"
        else:
            return self.gen_int_literal()

    def gen_any_expr(self, depth: int) -> tuple[str, str]:
        """Generate a random expression, returning (code, type_tag)."""
        target = self.rng.choice(
            [T_INT, T_FLOAT, T_STR, T_BOOL, T_LIST_INT, T_DICT, T_NONE]
        )
        return self.gen_typed_expr(depth, target), target

    # -- Int expressions ----------------------------------------------------

    def _gen_int_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_INT)
            if var and self.rng.random() < 0.4:
                return var
            return self.gen_int_literal()
        kind = self.rng.choices(
            ["literal", "var", "arith", "abs", "len", "int_call", "min_max"],
            weights=[25, 15, 25, 10, 10, 10, 5],
        )[0]
        if kind == "literal":
            return self.gen_int_literal()
        elif kind == "var":
            var = self._known_var_of_type(T_INT)
            return var if var else self.gen_int_literal()
        elif kind == "arith":
            op = self.rng.choice(["+", "-", "*", "//", "%", "**"])
            left = self._gen_int_expr(depth + 1)
            right = self._gen_int_expr(depth + 1)
            if op in ("//", "%"):
                right = f"({right} or 1)"
            elif op == "**":
                right = f"(abs({right}) % 6)"
            return f"({left} {op} {right})"
        elif kind == "abs":
            return f"abs({self._gen_int_expr(depth + 1)})"
        elif kind == "len":
            # len of a string literal or list literal
            if self.rng.random() < 0.5:
                return f"len({self._gen_str_expr(depth + 1)})"
            else:
                return f"len({self._gen_list_int_expr(depth + 1)})"
        elif kind == "int_call":
            return f"int({self._gen_float_expr(depth + 1)})"
        else:  # min_max
            fn = self.rng.choice(["min", "max"])
            a = self._gen_int_expr(depth + 1)
            b = self._gen_int_expr(depth + 1)
            return f"{fn}({a}, {b})"

    # -- Float expressions --------------------------------------------------

    def _gen_float_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_FLOAT)
            if var and self.rng.random() < 0.4:
                return var
            return self.gen_float_literal()
        kind = self.rng.choices(
            ["literal", "var", "arith", "float_call"],
            weights=[35, 15, 35, 15],
        )[0]
        if kind == "literal":
            return self.gen_float_literal()
        elif kind == "var":
            var = self._known_var_of_type(T_FLOAT)
            return var if var else self.gen_float_literal()
        elif kind == "arith":
            op = self.rng.choice(["+", "-", "*"])
            left = self._gen_float_expr(depth + 1)
            right = self._gen_float_expr(depth + 1)
            return f"({left} {op} {right})"
        else:
            return f"float({self._gen_int_expr(depth + 1)})"

    # -- String expressions -------------------------------------------------

    def _gen_str_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_STR)
            if var and self.rng.random() < 0.4:
                return var
            return self.gen_str_literal()
        kind = self.rng.choices(
            [
                "literal",
                "var",
                "concat",
                "method",
                "fstring",
                "repeat",
                "str_call",
                "index",
                "slice",
            ],
            weights=[20, 10, 12, 18, 10, 8, 8, 7, 7],
        )[0]
        if kind == "literal":
            return self.gen_str_literal()
        elif kind == "var":
            var = self._known_var_of_type(T_STR)
            return var if var else self.gen_str_literal()
        elif kind == "concat":
            left = self._gen_str_expr(depth + 1)
            right = self._gen_str_expr(depth + 1)
            return f"({left} + {right})"
        elif kind == "method":
            return self._gen_str_method(depth)
        elif kind == "fstring":
            return self._gen_fstring(depth)
        elif kind == "repeat":
            s = self._gen_str_expr(depth + 1)
            n = self.rng.randint(0, 4)
            return f"({s} * {n})"
        elif kind == "str_call":
            inner = self._gen_int_expr(depth + 1)
            return f"str({inner})"
        elif kind == "index":
            return self._gen_str_index()
        else:  # slice
            return self._gen_str_slice()

    def _gen_str_method(self, depth: int) -> str:
        method = self.rng.choice(
            [
                "upper",
                "lower",
                "strip",
                "title",
                "lstrip",
                "rstrip",
                "replace",
                "center",
            ]
        )
        s = self._gen_str_expr(depth + 1)
        if method in ("upper", "lower", "strip", "title", "lstrip", "rstrip"):
            return f"{s}.{method}()"
        elif method == "replace":
            old = self.rng.choice(["a", "o", "l", " "])
            new = self.rng.choice(["X", "_", ""])
            return f"{s}.replace({repr(old)}, {repr(new)})"
        else:  # center
            width = self.rng.randint(5, 15)
            return f"{s}.center({width})"

    def _gen_str_index(self) -> str:
        s = self.rng.choice([x for x in self.STR_LITERALS if x]) or "hello"
        idx = self.rng.randint(0, len(s) - 1)
        return f"{repr(s)}[{idx}]"

    def _gen_str_slice(self) -> str:
        s = self.rng.choice([x for x in self.STR_LITERALS if x]) or "hello"
        start = self.rng.randint(0, max(0, len(s) - 1))
        end = self.rng.randint(start, len(s))
        return f"{repr(s)}[{start}:{end}]"

    def _gen_fstring(self, depth: int) -> str:
        num_parts = self.rng.randint(1, 3)
        parts: list[str] = []
        for _ in range(num_parts):
            if self.rng.random() < 0.5:
                parts.append(self.rng.choice(["hello", "val=", "result:", " "]))
            else:
                inner = self._gen_fstring_inner()
                parts.append("{" + inner + "}")
        return 'f"' + "".join(parts) + '"'

    def _gen_fstring_inner(self) -> str:
        kind = self.rng.choice(["int", "arith", "str_method"])
        if kind == "int":
            return str(self.rng.randint(-50, 50))
        elif kind == "arith":
            a = self.rng.randint(-10, 10)
            b = self.rng.randint(1, 10)
            op = self.rng.choice(["+", "-", "*"])
            return f"{a} {op} {b}"
        else:
            s = self.rng.choice(["hello", "world", "test"])
            return f"'{s}'.upper()"

    # -- Bool expressions ---------------------------------------------------

    def _gen_bool_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_BOOL)
            if var and self.rng.random() < 0.4:
                return var
            return self.gen_bool_literal()
        kind = self.rng.choices(
            [
                "literal",
                "var",
                "comparison",
                "and_or",
                "not",
                "isinstance",
                "membership",
                "chained_cmp",
            ],
            weights=[15, 10, 25, 15, 10, 10, 10, 5],
        )[0]
        if kind == "literal":
            return self.gen_bool_literal()
        elif kind == "var":
            var = self._known_var_of_type(T_BOOL)
            return var if var else self.gen_bool_literal()
        elif kind == "comparison":
            return self._gen_comparison(depth)
        elif kind == "and_or":
            op = self.rng.choice(["and", "or"])
            left = self._gen_bool_expr(depth + 1)
            right = self._gen_bool_expr(depth + 1)
            return f"({left} {op} {right})"
        elif kind == "not":
            operand = self._gen_bool_expr(depth + 1)
            return f"(not {operand})"
        elif kind == "isinstance":
            return self._gen_isinstance_check()
        elif kind == "membership":
            return self._gen_membership_test(depth)
        else:  # chained_cmp
            return self._gen_chained_cmp(depth)

    def _gen_comparison(self, depth: int) -> str:
        op = self.rng.choice(["==", "!=", "<", ">", "<=", ">="])
        if op in ("<", ">", "<=", ">="):
            # Ordering: same-type only
            if self.rng.random() < 0.6:
                left = self._gen_int_expr(depth + 1)
                right = self._gen_int_expr(depth + 1)
            else:
                left = self.gen_str_literal()
                right = self.gen_str_literal()
        else:
            # Equality: safe with any type
            if self.rng.random() < 0.5:
                left = self._gen_int_expr(depth + 1)
                right = self._gen_int_expr(depth + 1)
            else:
                left = self._gen_str_expr(depth + 1)
                right = self._gen_str_expr(depth + 1)
        return f"({left} {op} {right})"

    def _gen_isinstance_check(self) -> str:
        kind = self.rng.choice(["int", "str", "float", "bool", "list"])
        if kind == "int":
            val = self.gen_int_literal()
        elif kind == "str":
            val = self.gen_str_literal()
        elif kind == "float":
            val = self.gen_float_literal()
        elif kind == "bool":
            val = self.gen_bool_literal()
        else:
            val = "[1, 2, 3]"
        check = self.rng.choice(["int", "str", "float", "bool", "list", "tuple"])
        return f"isinstance({val}, {check})"

    def _gen_membership_test(self, depth: int) -> str:
        needle = self._gen_int_expr(depth + 1)
        n = self.rng.randint(1, 4)
        elems = [self.gen_int_literal() for _ in range(n)]
        return f"({needle} in [{', '.join(elems)}])"

    def _gen_chained_cmp(self, depth: int) -> str:
        n = self.rng.randint(3, 4)
        operands = [self._gen_int_expr(depth + 1) for _ in range(n)]
        ops = [
            self.rng.choice(["<", "<=", ">", ">=", "==", "!="]) for _ in range(n - 1)
        ]
        parts = [operands[0]]
        for op, operand in zip(ops, operands[1:]):
            parts.extend([op, operand])
        return f"({' '.join(parts)})"

    # -- List expressions ---------------------------------------------------

    def _gen_list_int_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_LIST_INT)
            if var and self.rng.random() < 0.4:
                return var
            n = self.rng.randint(0, 4)
            elems = [self.gen_int_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        kind = self.rng.choices(
            ["literal", "var", "comp", "sorted", "slice"],
            weights=[35, 15, 20, 15, 15],
        )[0]
        if kind == "literal":
            n = self.rng.randint(0, 5)
            elems = [self._gen_int_expr(depth + 1) for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        elif kind == "var":
            var = self._known_var_of_type(T_LIST_INT)
            if var:
                return var
            n = self.rng.randint(1, 4)
            elems = [self.gen_int_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        elif kind == "comp":
            lv = self._fresh_var()
            bound = self.rng.randint(1, 6)
            op = self.rng.choice(["+", "-", "*"])
            val = self.rng.randint(1, 10)
            body = f"({lv} {op} {val})"
            if self.rng.random() < 0.3:
                threshold = self.rng.randint(0, bound)
                return f"[{body} for {lv} in range({bound}) if {lv} > {threshold}]"
            return f"[{body} for {lv} in range({bound})]"
        elif kind == "sorted":
            inner = self._gen_list_int_expr(depth + 1)
            return f"sorted({inner})"
        else:  # slice
            n = self.rng.randint(2, 5)
            elems = [self.gen_int_literal() for _ in range(n)]
            lst = "[" + ", ".join(elems) + "]"
            start = self.rng.randint(0, n - 1)
            end = self.rng.randint(start, n)
            return f"{lst}[{start}:{end}]"

    def _gen_list_str_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_LIST_STR)
            if var and self.rng.random() < 0.4:
                return var
            n = self.rng.randint(0, 3)
            elems = [self.gen_str_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        kind = self.rng.choices(["literal", "var", "split"], weights=[50, 20, 30])[0]
        if kind == "literal":
            n = self.rng.randint(0, 4)
            elems = [self.gen_str_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        elif kind == "var":
            var = self._known_var_of_type(T_LIST_STR)
            if var:
                return var
            n = self.rng.randint(1, 3)
            elems = [self.gen_str_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        else:  # split
            s = self.gen_str_literal()
            return f"{s}.split()"

    # -- Tuple expressions --------------------------------------------------

    def _gen_tuple_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_TUPLE)
            if var and self.rng.random() < 0.3:
                return var
            n = self.rng.randint(0, 3)
            elems = [self.gen_int_literal() for _ in range(n)]
            if n == 1:
                return f"({elems[0]},)"
            return "(" + ", ".join(elems) + ")"
        n = self.rng.randint(0, 4)
        elems: list[str] = []
        for _ in range(n):
            e, _ = self.gen_any_expr(depth + 1)
            elems.append(e)
        if n == 1:
            return f"({elems[0]},)"
        return "(" + ", ".join(elems) + ")"

    # -- Dict expressions ---------------------------------------------------

    def _gen_dict_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_DICT)
            if var and self.rng.random() < 0.3:
                return var
            n = self.rng.randint(0, 3)
            pairs = []
            for _ in range(n):
                key = self.gen_str_literal()
                val = self.gen_int_literal()
                pairs.append(f"{key}: {val}")
            return "{" + ", ".join(pairs) + "}"
        kind = self.rng.choices(["literal", "var", "comp"], weights=[50, 20, 30])[0]
        if kind == "literal":
            n = self.rng.randint(0, 4)
            pairs = []
            for _ in range(n):
                key = self.gen_str_literal()
                val_code, _ = self.gen_any_expr(depth + 1)
                pairs.append(f"{key}: {val_code}")
            return "{" + ", ".join(pairs) + "}"
        elif kind == "var":
            var = self._known_var_of_type(T_DICT)
            if var:
                return var
            return "{" + repr("a") + ": 1}"
        else:  # comp
            lv = self._fresh_var()
            bound = self.rng.randint(1, 5)
            val_body = self.rng.choice([f"{lv} * {lv}", f"str({lv})", f"{lv} * 2"])
            return f"{{{lv}: {val_body} for {lv} in range({bound})}}"

    # -- Set expressions ----------------------------------------------------

    def _gen_set_int_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_SET_INT)
            if var and self.rng.random() < 0.3:
                return var
            n = self.rng.randint(1, 4)
            elems = [self.gen_int_literal() for _ in range(n)]
            return "{" + ", ".join(elems) + "}"
        kind = self.rng.choices(
            ["literal", "var", "comp", "op"], weights=[35, 15, 25, 25]
        )[0]
        if kind == "literal":
            n = self.rng.randint(1, 4)
            elems = [self.gen_int_literal() for _ in range(n)]
            return "{" + ", ".join(elems) + "}"
        elif kind == "var":
            var = self._known_var_of_type(T_SET_INT)
            if var:
                return var
            return "{1, 2, 3}"
        elif kind == "comp":
            lv = self._fresh_var()
            bound = self.rng.randint(1, 6)
            body = self.rng.choice([lv, f"({lv} % 3)", f"abs({lv})"])
            return f"{{{body} for {lv} in range({bound})}}"
        else:  # op
            op = self.rng.choice(
                ["union", "intersection", "difference", "symmetric_difference"]
            )
            s1 = self._gen_set_int_expr(depth + 1)
            s2 = self._gen_set_int_expr(depth + 1)
            return f"{s1}.{op}({s2})"

    # -- Statement generators -----------------------------------------------

    def gen_stmt(self, depth: int = 0, indent: int = 0) -> str:
        if depth >= self.max_depth:
            return self._gen_simple_stmt(indent)

        kind = self.rng.choices(
            [
                "assign",
                "print",
                "if",
                "for_loop",
                "while_loop",
                "augmented_assign",
                "multi_assign",
                "try_except",
                "break_continue_for",
                "dict_iteration",
                "unpack",
                "multi_except",
                "list_method",
                "enumerate",
                "zip",
                "nested_loop",
                "assert",
                "del",
            ],
            weights=[
                14,
                16,
                10,
                8,
                5,
                5,
                4,
                5,
                4,
                4,
                4,
                3,
                4,
                3,
                3,
                3,
                2,
                2,
            ],
        )[0]

        method = getattr(self, f"gen_{kind}_stmt", None)
        if method is None:
            return self._gen_simple_stmt(indent)
        return method(depth, indent)

    def _gen_simple_stmt(self, indent: int = 0) -> str:
        if self.rng.random() < 0.5:
            return self.gen_print_stmt(0, indent)
        return self.gen_assign_stmt(0, indent)

    def gen_assign_stmt(self, depth: int = 0, indent: int = 0) -> str:
        var = self._fresh_var()
        # Choose a target type and generate a typed expression
        target_type = self.rng.choice(
            [T_INT, T_FLOAT, T_STR, T_BOOL, T_LIST_INT, T_DICT, T_NONE]
        )
        expr = self.gen_typed_expr(depth + 1, target_type)
        self._add_var(var, target_type)
        prefix = "    " * indent
        return f"{prefix}{var} = {expr}"

    def gen_augmented_assign_stmt(self, depth: int = 0, indent: int = 0) -> str:
        var = self._fresh_var()
        init_val = self.gen_int_literal()
        self._add_var(var, T_INT)
        op = self.rng.choice(["+=", "-=", "*="])
        val = self.rng.randint(1, 10)
        prefix = "    " * indent
        return f"{prefix}{var} = {init_val}\n{prefix}{var} {op} {val}"

    def gen_multi_assign_stmt(self, depth: int = 0, indent: int = 0) -> str:
        n = self.rng.randint(2, 3)
        names = [self._fresh_var() for _ in range(n)]
        vals: list[str] = []
        types: list[str] = []
        for _ in range(n):
            t = self.rng.choice([T_INT, T_STR, T_BOOL])
            vals.append(self.gen_typed_expr(depth + 1, t))
            types.append(t)
        for name, t in zip(names, types):
            self._add_var(name, t)
        prefix = "    " * indent
        return f"{prefix}{', '.join(names)} = {', '.join(vals)}"

    def gen_print_stmt(self, depth: int = 0, indent: int = 0) -> str:
        n_args = self.rng.randint(1, 3)
        args: list[str] = []
        for _ in range(n_args):
            code, _ = self.gen_any_expr(depth + 1)
            args.append(code)
        prefix = "    " * indent
        return f"{prefix}print({', '.join(args)})"

    def gen_if_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        cond = self._gen_bool_expr(depth + 1)
        self._push_scope()
        body_stmts = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        lines = [f"{prefix}if {cond}:"]
        lines.extend(body_stmts)
        if self.rng.random() < 0.3:
            elif_cond = self._gen_bool_expr(depth + 1)
            self._push_scope()
            elif_body = self._gen_body(depth + 1, indent + 1)
            self._pop_scope()
            lines.append(f"{prefix}elif {elif_cond}:")
            lines.extend(elif_body)
        if self.rng.random() < 0.5:
            self._push_scope()
            else_body = self._gen_body(depth + 1, indent + 1)
            self._pop_scope()
            lines.append(f"{prefix}else:")
            lines.extend(else_body)
        return "\n".join(lines)

    def gen_for_loop_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        loop_var = self._fresh_var()
        bound = self.rng.randint(1, 8)
        self._push_scope()
        self._add_var(loop_var, T_INT)
        body_stmts = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        lines = [f"{prefix}for {loop_var} in range({bound}):"]
        lines.extend(body_stmts)
        return "\n".join(lines)

    def gen_while_loop_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        counter = self._fresh_var()
        self._add_var(counter, T_INT)
        limit = self.rng.randint(1, 6)
        self._push_scope()
        body_stmts = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        inner_prefix = "    " * (indent + 1)
        lines = [
            f"{prefix}{counter} = 0",
            f"{prefix}while {counter} < {limit}:",
        ]
        lines.extend(body_stmts)
        lines.append(f"{inner_prefix}{counter} += 1")
        return "\n".join(lines)

    def gen_try_except_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        self._push_scope()
        try_body = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        self._push_scope()
        except_body = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        exc_type = self.rng.choice(
            [
                "Exception",
                "ValueError",
                "TypeError",
                "ZeroDivisionError",
                "IndexError",
                "KeyError",
            ]
        )
        lines = [f"{prefix}try:"]
        lines.extend(try_body)
        lines.append(f"{prefix}except {exc_type}:")
        lines.extend(except_body)
        return "\n".join(lines)

    def gen_break_continue_for_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        inner2 = "    " * (indent + 2)
        loop_var = self._fresh_var()
        bound = self.rng.randint(2, 8)
        threshold = self.rng.randint(0, bound - 1)
        action = self.rng.choice(["break", "continue"])
        lines = [
            f"{prefix}for {loop_var} in range({bound}):",
            f"{inner}if {loop_var} == {threshold}:",
            f"{inner2}{action}",
            f"{inner}print({loop_var})",
        ]
        return "\n".join(lines)

    def gen_dict_iteration_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        n = self.rng.randint(1, 4)
        keys = [repr(self.rng.choice(["a", "b", "c", "d", "x"])) for _ in range(n)]
        vals = [str(self.rng.randint(0, 100)) for _ in range(n)]
        pairs = [f"{k}: {v}" for k, v in zip(keys, vals)]
        d = "{" + ", ".join(pairs) + "}"
        kvar = self._fresh_var()
        vvar = self._fresh_var()
        lines = [
            f"{prefix}for {kvar}, {vvar} in sorted({d}.items()):",
            f"{inner}print({kvar}, {vvar})",
        ]
        return "\n".join(lines)

    def gen_unpack_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        kind = self.rng.choice(["starred", "nested_tuple", "swap"])
        if kind == "starred":
            n = self.rng.randint(3, 5)
            vals = [str(self.rng.randint(0, 50)) for _ in range(n)]
            first = self._fresh_var()
            rest = self._fresh_var()
            self._add_var(first, T_INT)
            self._add_var(rest, T_LIST_INT)
            return (
                f"{prefix}{first}, *{rest} = [{', '.join(vals)}]\n"
                f"{prefix}print({first}, {rest})"
            )
        elif kind == "nested_tuple":
            a = self._fresh_var()
            b = self._fresh_var()
            c = self._fresh_var()
            v1 = self.rng.randint(0, 50)
            v2 = self.rng.randint(0, 50)
            v3 = self.rng.randint(0, 50)
            self._add_var(a, T_INT)
            self._add_var(b, T_INT)
            self._add_var(c, T_INT)
            return (
                f"{prefix}({a}, ({b}, {c})) = ({v1}, ({v2}, {v3}))\n"
                f"{prefix}print({a}, {b}, {c})"
            )
        else:  # swap
            a = self._fresh_var()
            b = self._fresh_var()
            v1 = self.rng.randint(0, 50)
            v2 = self.rng.randint(0, 50)
            self._add_var(a, T_INT)
            self._add_var(b, T_INT)
            return (
                f"{prefix}{a}, {b} = {v1}, {v2}\n"
                f"{prefix}{a}, {b} = {b}, {a}\n"
                f"{prefix}print({a}, {b})"
            )

    def gen_multi_except_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        self._push_scope()
        try_body = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        types = [
            "ValueError",
            "TypeError",
            "ZeroDivisionError",
            "IndexError",
            "KeyError",
            "AttributeError",
        ]
        self.rng.shuffle(types)
        n_except = self.rng.randint(2, 3)
        lines = [f"{prefix}try:"]
        lines.extend(try_body)
        for i in range(n_except):
            exc = types[i % len(types)]
            self._push_scope()
            except_body = self._gen_body(depth + 1, indent + 1)
            self._pop_scope()
            lines.append(f"{prefix}except {exc}:")
            lines.extend(except_body)
        return "\n".join(lines)

    def gen_list_method_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        var = self._fresh_var()
        n = self.rng.randint(1, 4)
        elems = [str(self.rng.randint(0, 50)) for _ in range(n)]
        self._add_var(var, T_LIST_INT)
        lines = [f"{prefix}{var} = [{', '.join(elems)}]"]
        for _ in range(self.rng.randint(1, 3)):
            method = self.rng.choice(["append", "sort", "reverse", "pop"])
            if method == "append":
                val = self.rng.randint(0, 50)
                lines.append(f"{prefix}{var}.append({val})")
            elif method in ("sort", "reverse"):
                lines.append(f"{prefix}{var}.{method}()")
            elif method == "pop":
                lines.append(f"{prefix}if {var}: {var}.pop()")
        lines.append(f"{prefix}print({var})")
        return "\n".join(lines)

    def gen_enumerate_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        idx_var = self._fresh_var()
        val_var = self._fresh_var()
        n = self.rng.randint(2, 5)
        elems = [str(self.rng.randint(0, 50)) for _ in range(n)]
        iterable = "[" + ", ".join(elems) + "]"
        lines = [
            f"{prefix}for {idx_var}, {val_var} in enumerate({iterable}):",
            f"{inner}print({idx_var}, {val_var})",
        ]
        return "\n".join(lines)

    def gen_zip_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        a_var = self._fresh_var()
        b_var = self._fresh_var()
        n = self.rng.randint(2, 4)
        list_a = "[" + ", ".join(str(self.rng.randint(0, 50)) for _ in range(n)) + "]"
        list_b = (
            "["
            + ", ".join(repr(self.rng.choice(["a", "b", "c", "d"])) for _ in range(n))
            + "]"
        )
        lines = [
            f"{prefix}for {a_var}, {b_var} in zip({list_a}, {list_b}):",
            f"{inner}print({a_var}, {b_var})",
        ]
        return "\n".join(lines)

    def gen_nested_loop_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        inner2 = "    " * (indent + 2)
        i_var = self._fresh_var()
        j_var = self._fresh_var()
        bound_i = self.rng.randint(1, 4)
        bound_j = self.rng.randint(1, 4)
        lines = [
            f"{prefix}for {i_var} in range({bound_i}):",
            f"{inner}for {j_var} in range({bound_j}):",
            f"{inner2}print({i_var}, {j_var})",
        ]
        return "\n".join(lines)

    def gen_assert_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        kind = self.rng.choice(["true", "isinstance", "len"])
        if kind == "true":
            return f"{prefix}assert True"
        elif kind == "isinstance":
            val = self.rng.randint(0, 100)
            return f"{prefix}assert isinstance({val}, int)"
        else:
            s = self.rng.choice(self.STR_LITERALS)
            return f"{prefix}assert len({repr(s)}) >= 0"

    def gen_del_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        var = self._fresh_var()
        val = self.rng.randint(0, 100)
        # Do NOT add var to _defined_vars since we immediately delete it
        return f"{prefix}{var} = {val}\n{prefix}del {var}"

    def _gen_body(self, depth: int, indent: int) -> list[str]:
        """Generate block body statements."""
        n = self.rng.randint(1, 3)
        stmts: list[str] = []
        for _ in range(n):
            stmts.append(self.gen_stmt(depth, indent))
        return stmts

    # -- Function generators ------------------------------------------------

    def gen_function_def(self) -> str:
        func_name = self._fresh_func()
        self._defined_funcs.append(func_name)
        n_pos = self.rng.randint(0, 3)
        n_default = self.rng.randint(0, 2)
        params: list[str] = []
        param_names: list[str] = []
        for _ in range(n_pos):
            name = self._fresh_var()
            params.append(name)
            param_names.append(name)
        for _ in range(n_default):
            name = self._fresh_var()
            default = self.gen_int_literal()
            params.append(f"{name}={default}")
            param_names.append(name)
        # Save outer vars and set function scope
        outer_vars = self._defined_vars[:]
        self._defined_vars = [(n, T_ANY) for n in param_names]
        lines = [f"def {func_name}({', '.join(params)}):"]
        for _ in range(self.rng.randint(1, 4)):
            lines.append(self.gen_stmt(depth=2, indent=1))
        # Return something safe
        ret_code, _ = self.gen_any_expr(depth=2)
        lines.append(f"    return {ret_code}")
        self._defined_vars = outer_vars
        return "\n".join(lines)

    def _gen_func_call(self, func_name: str) -> str:
        n_args = self.rng.randint(0, 3)
        args: list[str] = []
        for _ in range(n_args):
            # Use only hashable types for function args (functions may hash them)
            code = self.gen_typed_expr(
                depth=2, target_type=self.rng.choice([T_INT, T_STR, T_BOOL, T_FLOAT])
            )
            args.append(code)
        result_var = self._fresh_var()
        # Do NOT add to _defined_vars: the var is inside try/except so may not be defined
        lines = [
            f"{result_var} = None",
            "try:",
            f"    {result_var} = {func_name}({', '.join(args)})",
            f"    print({result_var})",
            "except (TypeError, ValueError, ZeroDivisionError, OverflowError, AttributeError) as _fuzz_e:",
            "    print(type(_fuzz_e).__name__)",
        ]
        self._add_var(result_var, T_ANY)
        return "\n".join(lines)

    # -- Class generators ---------------------------------------------------

    def gen_class_def(self) -> str:
        class_names = ["Point", "Box", "Counter", "Wrapper", "Pair", "Node"]
        cls_name = self.rng.choice(class_names) + f"_{self._func_counter}"
        self._func_counter += 1
        n_fields = self.rng.randint(1, 3)
        field_names = [f"f{i}" for i in range(n_fields)]
        lines = [f"class {cls_name}:"]
        init_params = ", ".join(field_names)
        lines.append(f"    def __init__(self, {init_params}):")
        for name in field_names:
            lines.append(f"        self.{name} = {name}")
        repr_parts = ", ".join(f"{name}={{self.{name}!r}}" for name in field_names)
        lines.append("    def __repr__(self):")
        lines.append(f'        return f"{cls_name}({repr_parts})"')
        n_methods = self.rng.randint(1, 2)
        method_names: list[str] = []
        for m_idx in range(n_methods):
            mname = f"method_{m_idx}"
            method_names.append(mname)
            kind = self.rng.choice(["getter", "compute", "transform"])
            if kind == "getter":
                f = self.rng.choice(field_names)
                lines.append(f"    def {mname}(self):")
                lines.append(f"        return self.{f}")
            elif kind == "compute":
                f = self.rng.choice(field_names)
                lines.append(f"    def {mname}(self):")
                lines.append(f"        return str(self.{f}) + '_computed'")
            else:
                lines.append(f"    def {mname}(self, x):")
                f = self.rng.choice(field_names)
                lines.append(f"        return str(self.{f}) + str(x)")
        self._defined_classes.append((cls_name, field_names, method_names))
        return "\n".join(lines)

    def gen_class_usage(self, cls_name, field_names, method_names) -> str:
        lines: list[str] = []
        args: list[str] = []
        for _ in field_names:
            code, _ = self.gen_any_expr(depth=2)
            args.append(code)
        inst_var = self._fresh_var()
        # Initialize before try so it's always defined
        lines.append(f"{inst_var} = None")
        self._add_var(inst_var, T_ANY)
        lines.append("try:")
        lines.append(f"    {inst_var} = {cls_name}({', '.join(args)})")
        lines.append(f"    print(repr({inst_var}))")
        for fname in field_names:
            if self.rng.random() < 0.5:
                lines.append(f"    print({inst_var}.{fname})")
        for mname in method_names:
            if self.rng.random() < 0.7:
                lines.append("    try:")
                lines.append(f"        print({inst_var}.{mname}())")
                lines.append("    except TypeError:")
                arg = self.rng.choice(["42", "'hello'", "3.14"])
                lines.append(f"        print({inst_var}.{mname}({arg}))")
        lines.append(
            "except (TypeError, ValueError, ZeroDivisionError, OverflowError, AttributeError) as _cls_e:"
        )
        lines.append("    print(type(_cls_e).__name__)")
        return "\n".join(lines)

    def gen_inheritance_class(self) -> str:
        if not self._defined_classes:
            return self.gen_class_def()
        parent_name, parent_fields, parent_methods = self.rng.choice(
            self._defined_classes
        )
        child_name = f"Sub{parent_name}"
        extra_field = f"extra_{self._func_counter}"
        self._func_counter += 1
        all_params = ", ".join(parent_fields + [extra_field])
        lines = [f"class {child_name}({parent_name}):"]
        lines.append(f"    def __init__(self, {all_params}):")
        lines.append(f"        super().__init__({', '.join(parent_fields)})")
        lines.append(f"        self.{extra_field} = {extra_field}")
        all_fields = parent_fields + [extra_field]
        repr_parts = ", ".join(f"{f}={{self.{f}!r}}" for f in all_fields)
        lines.append("    def __repr__(self):")
        lines.append(f'        return f"{child_name}({repr_parts})"')
        lines.append("    def get_extra(self):")
        lines.append(f"        return self.{extra_field}")
        self._defined_classes.append(
            (child_name, all_fields, parent_methods + ["get_extra"])
        )
        return "\n".join(lines)

    # -- Closure generators --------------------------------------------------

    def gen_closure_def(self) -> str:
        outer_name = self._fresh_func()
        inner_name = f"_inner_{self._func_counter}"
        self._func_counter += 1
        captured_var = self._fresh_var()
        captured_val = self.rng.randint(1, 50)
        param = self._fresh_var()
        kind = self.rng.choice(["simple", "counter", "accumulator"])
        if kind == "simple":
            lines = [
                f"def {outer_name}({param}):",
                f"    {captured_var} = {captured_val}",
                f"    def {inner_name}(x):",
                f"        return x + {captured_var} + {param}",
                f"    return {inner_name}",
            ]
        elif kind == "counter":
            lines = [
                f"def {outer_name}():",
                f"    {captured_var} = [0]",
                f"    def {inner_name}():",
                f"        {captured_var}[0] += 1",
                f"        return {captured_var}[0]",
                f"    return {inner_name}",
            ]
        else:
            lines = [
                f"def {outer_name}(start):",
                f"    {captured_var} = [start]",
                f"    def {inner_name}(val):",
                f"        {captured_var}[0] += val",
                f"        return {captured_var}[0]",
                f"    return {inner_name}",
            ]
        self._defined_closures.append((outer_name, kind))
        return "\n".join(lines)

    def gen_closure_usage(self, func_name: str, kind: str) -> str:
        result_var = self._fresh_var()
        lines: list[str] = []
        if kind == "simple":
            arg = self.rng.randint(1, 20)
            lines.append(f"{result_var} = {func_name}({arg})")
            call_arg = self.rng.randint(1, 20)
            lines.append(f"print({result_var}({call_arg}))")
        elif kind == "counter":
            lines.append(f"{result_var} = {func_name}()")
            for _ in range(self.rng.randint(2, 5)):
                lines.append(f"print({result_var}())")
        else:
            start = self.rng.randint(0, 10)
            lines.append(f"{result_var} = {func_name}({start})")
            for _ in range(self.rng.randint(2, 5)):
                val = self.rng.randint(1, 10)
                lines.append(f"print({result_var}({val}))")
        return "\n".join(lines)

    # -- Keyword-only and *args generators ----------------------------------

    def gen_kwonly_function_def(self) -> str:
        func_name = self._fresh_func()
        n_pos = self.rng.randint(1, 2)
        n_kw = self.rng.randint(1, 2)
        pos_params: list[str] = []
        kw_params: list[str] = []
        param_names: list[str] = []
        for _ in range(n_pos):
            name = self._fresh_var()
            pos_params.append(name)
            param_names.append(name)
        for _ in range(n_kw):
            name = self._fresh_var()
            default = self.gen_int_literal()
            kw_params.append(f"{name}={default}")
            param_names.append(name)
        all_params = ", ".join(pos_params + ["*"] + kw_params)
        outer_vars = self._defined_vars[:]
        self._defined_vars = [(n, T_ANY) for n in param_names]
        lines = [f"def {func_name}({all_params}):"]
        for _ in range(self.rng.randint(1, 3)):
            lines.append(self.gen_stmt(depth=2, indent=1))
        ret_code, _ = self.gen_any_expr(depth=2)
        lines.append(f"    return {ret_code}")
        self._defined_vars = outer_vars
        self._defined_kwonly_funcs.append((func_name, n_pos, kw_params))
        return "\n".join(lines)

    def gen_kwonly_call(self, func_name, n_pos, kw_params) -> str:
        pos_args: list[str] = []
        for _ in range(n_pos):
            code, _ = self.gen_any_expr(depth=2)
            pos_args.append(code)
        kw_args: list[str] = []
        for kp in kw_params:
            name = kp.split("=")[0]
            if self.rng.random() < 0.6:
                val_code, _ = self.gen_any_expr(depth=2)
                kw_args.append(f"{name}={val_code}")
        all_args = pos_args + kw_args
        result_var = self._fresh_var()
        self._add_var(result_var, T_ANY)
        lines = [
            f"{result_var} = None",
            "try:",
            f"    {result_var} = {func_name}({', '.join(all_args)})",
            f"    print({result_var})",
            "except (TypeError, ValueError, ZeroDivisionError, OverflowError) as _fuzz_e:",
            "    print(type(_fuzz_e).__name__)",
        ]
        return "\n".join(lines)

    def gen_starargs_function_def(self) -> str:
        func_name = self._fresh_func()
        kind = self.rng.choice(["args_only", "kwargs_only", "both"])
        if kind == "args_only":
            lines = [
                f"def {func_name}(*args):",
                "    result = 0",
                "    for a in args:",
                "        result += hash(a) % 100",
                "    return result",
            ]
        elif kind == "kwargs_only":
            lines = [
                f"def {func_name}(**kwargs):",
                "    parts = []",
                "    for k in sorted(kwargs):",
                "        parts.append(f'{k}={kwargs[k]}')",
                "    return ', '.join(parts)",
            ]
        else:
            lines = [
                f"def {func_name}(*args, **kwargs):",
                "    result = len(args)",
                "    for k in sorted(kwargs):",
                "        result += len(str(kwargs[k]))",
                "    return result",
            ]
        self._defined_starargs_funcs.append((func_name, kind))
        return "\n".join(lines)

    def gen_starargs_call(self, func_name: str, kind: str) -> str:
        lines: list[str] = []
        result_var = self._fresh_var()
        if kind == "args_only":
            n = self.rng.randint(0, 5)
            args: list[str] = []
            for _ in range(n):
                # Use only hashable types (args_only function hashes its args)
                code = self.gen_typed_expr(
                    depth=2,
                    target_type=self.rng.choice([T_INT, T_STR, T_BOOL, T_FLOAT]),
                )
                args.append(code)
            lines.append(f"{result_var} = {func_name}({', '.join(args)})")
        elif kind == "kwargs_only":
            # Use unique keys to avoid SyntaxError: keyword argument repeated
            all_keys = ["x", "y", "z", "name", "val"]
            self.rng.shuffle(all_keys)
            n = self.rng.randint(0, min(3, len(all_keys)))
            kwargs: list[str] = []
            for i in range(n):
                key = all_keys[i]
                val_code, _ = self.gen_any_expr(depth=2)
                kwargs.append(f"{key}={val_code}")
            lines.append(f"{result_var} = {func_name}({', '.join(kwargs)})")
        else:
            n_pos = self.rng.randint(0, 3)
            pos: list[str] = []
            for _ in range(n_pos):
                code, _ = self.gen_any_expr(depth=2)
                pos.append(code)
            # Use unique keys to avoid SyntaxError: keyword argument repeated
            all_keys = ["a", "b", "c", "d", "e"]
            self.rng.shuffle(all_keys)
            n_kw = self.rng.randint(0, min(2, len(all_keys)))
            kw: list[str] = []
            for i in range(n_kw):
                key = all_keys[i]
                val_code, _ = self.gen_any_expr(depth=2)
                kw.append(f"{key}={val_code}")
            lines.append(f"{result_var} = {func_name}({', '.join(pos + kw)})")
        lines.append(f"print({result_var})")
        return "\n".join(lines)

    # -- Deterministic output helpers ---------------------------------------

    def _gen_dict_print(self, depth: int) -> str:
        """Generate a dict print that produces deterministic output."""
        d = self._gen_dict_expr(depth)
        op = self.rng.choice(["keys", "values", "items", "get", "in", "len"])
        if op == "keys":
            return f"print(sorted({d}.keys()))"
        elif op == "values":
            return f"print(sorted({d}.values(), key=str))"
        elif op == "items":
            return f"print(sorted({d}.items()))"
        elif op == "get":
            key = self.rng.choice(["a", "b", "c", "z"])
            default = self.rng.randint(-1, 99)
            return f"print({d}.get({repr(key)}, {default}))"
        elif op == "in":
            key = self.rng.choice(["a", "b", "z"])
            return f"print({repr(key)} in {d})"
        else:
            return f"print(len({d}))"

    def _gen_set_print(self, depth: int) -> str:
        """Generate a set print that produces deterministic output."""
        s = self._gen_set_int_expr(depth)
        op = self.rng.choice(["sorted", "len", "in", "op"])
        if op == "sorted":
            return f"print(sorted({s}))"
        elif op == "len":
            return f"print(len({s}))"
        elif op == "in":
            val = self.rng.randint(0, 10)
            return f"print({val} in {s})"
        else:
            method = self.rng.choice(
                ["union", "intersection", "difference", "symmetric_difference"]
            )
            s2 = self._gen_set_int_expr(depth + 1)
            return f"print(sorted({s}.{method}({s2})))"

    # -- Top-level program generator ----------------------------------------

    def generate(self) -> str:
        self._var_counter = 0
        self._func_counter = 0
        self._defined_vars = []
        self._scope_stack = []
        self._defined_funcs = []
        self._defined_classes = []
        self._defined_closures = []
        self._defined_kwonly_funcs = []
        self._defined_starargs_funcs = []

        sections: list[str] = []

        # Classes
        for _ in range(self.rng.randint(0, 2)):
            sections.append(self.gen_class_def())
            sections.append("")
            if self.rng.random() < 0.3 and self._defined_classes:
                sections.append(self.gen_inheritance_class())
                sections.append("")

        # Functions
        for _ in range(self.rng.randint(0, 3)):
            sections.append(self.gen_function_def())
            sections.append("")

        # Closures
        for _ in range(self.rng.randint(0, 2)):
            sections.append(self.gen_closure_def())
            sections.append("")

        # Kwonly functions
        if self.rng.random() < 0.4:
            sections.append(self.gen_kwonly_function_def())
            sections.append("")

        # *args/**kwargs functions
        if self.rng.random() < 0.4:
            sections.append(self.gen_starargs_function_def())
            sections.append("")

        # Main body statements
        n_stmts = self.rng.randint(5, self.max_stmts)
        for _ in range(n_stmts):
            # Mix in some dict/set prints for determinism coverage
            r = self.rng.random()
            if r < 0.08:
                sections.append(self._gen_dict_print(depth=1))
            elif r < 0.14:
                sections.append(self._gen_set_print(depth=1))
            else:
                sections.append(self.gen_stmt(depth=0, indent=0))

        # Call functions
        for func_name in self._defined_funcs:
            sections.append(self._gen_func_call(func_name))

        # Use closures
        for func_name, kind in self._defined_closures:
            sections.append(self.gen_closure_usage(func_name, kind))

        # Call kwonly functions
        for func_name, n_pos, kw_params in self._defined_kwonly_funcs:
            sections.append(self.gen_kwonly_call(func_name, n_pos, kw_params))

        # Call *args/**kwargs functions
        for func_name, kind in self._defined_starargs_funcs:
            sections.append(self.gen_starargs_call(func_name, kind))

        # Instantiate classes
        for cls_info in self._defined_classes:
            sections.append(self.gen_class_usage(*cls_info))

        # Final summary print
        if self._defined_vars:
            # Pick a few vars that are still in scope
            n_pick = min(3, len(self._defined_vars))
            chosen = self.rng.sample(self._defined_vars, n_pick)
            summary_args = ", ".join(f"repr({v[0]})" for v in chosen)
            sections.append(f"print({summary_args})")

        return "\n".join(sections) + "\n"
