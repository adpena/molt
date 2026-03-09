"""
luau_physics_to_ir.py — Luau Physics Module → Molt SimpleIR Transpiler

Parses the subset of Luau used in Vertigo's physics modules and emits
SimpleIR JSON that the Molt WASM backend can compile.

Supported Luau subset:
  - local variable declarations
  - number literals (int + float)
  - arithmetic: + - * / ^ %
  - comparison: < > <= >= == ~=
  - math.* calls: sqrt, exp, cos, sin, atan, abs, floor, min, max
  - function definitions (local function, module function)
  - if/elseif/else/end
  - for i = start, stop do / for i = start, stop, step do
  - return (single + multi-value)
  - table constructors (simple key-value)
  - table field access (dot notation)
  - boolean literals, nil

Usage:
  uv run tools/luau_physics_to_ir.py --input path/to/Physics/ --output physics-ir.json
  uv run tools/luau_physics_to_ir.py --input path/to/Trajectory.luau --output trajectory-ir.json --function applyDrag
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path


# ---------------------------------------------------------------------------
# SimpleIR types (matching Rust struct definitions in molt-backend/src/lib.rs)
# ---------------------------------------------------------------------------


@dataclass
class OpIR:
    kind: str
    value: int | None = None
    f_value: float | None = None
    s_value: str | None = None
    var: str | None = None
    args: list[str] | None = None
    out: str | None = None
    fast_int: bool | None = None
    fast_float: bool | None = None
    type_hint: str | None = None
    bytes: list[int] | None = None
    task_kind: str | None = None
    container_type: str | None = None
    stack_eligible: bool | None = None
    raw_int: int | None = None

    def to_dict(self) -> dict:
        d: dict = {"kind": self.kind}
        for fld in (
            "value", "f_value", "s_value", "var", "args", "out",
            "fast_int", "fast_float", "type_hint", "bytes",
            "task_kind", "container_type", "stack_eligible", "raw_int",
        ):
            v = getattr(self, fld)
            if v is not None:
                d[fld] = v
        return d


@dataclass
class FunctionIR:
    name: str
    params: list[str]
    ops: list[OpIR] = field(default_factory=list)

    def to_dict(self) -> dict:
        return {
            "name": self.name,
            "params": self.params,
            "ops": [op.to_dict() for op in self.ops],
        }


@dataclass
class SimpleIR:
    functions: list[FunctionIR] = field(default_factory=list)

    def to_dict(self) -> dict:
        return {
            "ir_contract_name": "molt.simple_ir",
            "ir_contract_version": 1,
            "functions": [f.to_dict() for f in self.functions],
        }

    def to_json(self, indent: int = 2) -> str:
        return json.dumps(self.to_dict(), indent=indent)


# ---------------------------------------------------------------------------
# Math intrinsic mapping: Luau math.* → WASM-friendly IR ops
# ---------------------------------------------------------------------------

MATH_INTRINSICS = {
    "math.sqrt": "math_sqrt",
    "math.exp": "math_exp",
    "math.cos": "math_cos",
    "math.sin": "math_sin",
    "math.atan": "math_atan",
    "math.abs": "math_abs",
    "math.floor": "math_floor",
    "math.min": "math_min",
    "math.max": "math_max",
    "math.log": "math_log",
}


# ---------------------------------------------------------------------------
# Luau tokenizer (minimal, physics-module-targeted)
# ---------------------------------------------------------------------------

# Token patterns ordered by priority
TOKEN_PATTERNS = [
    ("COMMENT_BLOCK", r"--\[\[.*?\]\]"),
    ("COMMENT", r"--[^\n]*"),
    ("STRING_DQ", r'"[^"]*"'),
    ("STRING_SQ", r"'[^']*'"),
    ("NUMBER", r"\b\d+\.?\d*(?:[eE][+-]?\d+)?\b"),
    ("DOTDOT", r"\.\."),
    ("DOT", r"\."),
    ("LPAREN", r"\("),
    ("RPAREN", r"\)"),
    ("LBRACE", r"\{"),
    ("RBRACE", r"\}"),
    ("LBRACKET", r"\["),
    ("RBRACKET", r"\]"),
    ("COMMA", r","),
    ("SEMICOLON", r";"),
    ("COLON", r":"),
    ("NEQ", r"~="),
    ("LTEQ", r"<="),
    ("GTEQ", r">="),
    ("EQ", r"=="),
    ("ASSIGN", r"="),
    ("LT", r"<"),
    ("GT", r">"),
    ("PLUS", r"\+"),
    ("MINUS", r"-"),
    ("STAR", r"\*"),
    ("SLASH", r"/"),
    ("CARET", r"\^"),
    ("PERCENT", r"%"),
    ("HASH", r"#"),
    ("IDENT", r"[a-zA-Z_][a-zA-Z0-9_]*"),
    ("NEWLINE", r"\n"),
    ("WS", r"[ \t]+"),
]

KEYWORDS = {
    "local", "function", "end", "if", "then", "else", "elseif",
    "for", "do", "while", "repeat", "until", "return", "and", "or",
    "not", "true", "false", "nil", "in", "break", "continue",
}

_token_re = re.compile(
    "|".join(f"(?P<{name}>{pattern})" for name, pattern in TOKEN_PATTERNS),
    re.DOTALL,
)


@dataclass
class Token:
    type: str
    value: str
    line: int
    col: int


def tokenize(source: str) -> list[Token]:
    """Tokenize Luau source into a list of tokens."""
    tokens: list[Token] = []
    line = 1
    col = 1
    for m in _token_re.finditer(source):
        kind = m.lastgroup
        value = m.group()
        if kind == "NEWLINE":
            line += 1
            col = 1
            continue
        if kind in ("WS", "COMMENT", "COMMENT_BLOCK"):
            # Track newlines in block comments
            newlines = value.count("\n")
            if newlines:
                line += newlines
                col = len(value) - value.rfind("\n")
            else:
                col += len(value)
            continue
        # Classify keywords
        if kind == "IDENT" and value in KEYWORDS:
            kind = f"KW_{value.upper()}"
        tokens.append(Token(kind, value, line, col))
        col += len(value)
    return tokens


# ---------------------------------------------------------------------------
# Parser — targeted at physics module patterns
# ---------------------------------------------------------------------------


class LuauPhysicsParser:
    """
    Minimal Luau parser for physics modules.
    Extracts function signatures and body structure.
    Does NOT handle: metatables, coroutines, closures, varargs, OOP patterns.
    """

    def __init__(self, tokens: list[Token]):
        self.tokens = tokens
        self.pos = 0
        self.functions: list[FunctionIR] = []
        self._temp_counter = 0

    def _temp(self) -> str:
        self._temp_counter += 1
        return f"__t{self._temp_counter}"

    def peek(self, offset: int = 0) -> Token | None:
        idx = self.pos + offset
        if idx < len(self.tokens):
            return self.tokens[idx]
        return None

    def advance(self) -> Token:
        tok = self.tokens[self.pos]
        self.pos += 1
        return tok

    def expect(self, token_type: str) -> Token:
        tok = self.advance()
        if tok.type != token_type:
            raise SyntaxError(
                f"Expected {token_type}, got {tok.type} ({tok.value!r}) "
                f"at line {tok.line}:{tok.col}"
            )
        return tok

    def match(self, token_type: str) -> Token | None:
        tok = self.peek()
        if tok and tok.type == token_type:
            return self.advance()
        return None

    def parse(self) -> SimpleIR:
        """Parse the token stream and extract function definitions."""
        while self.pos < len(self.tokens):
            tok = self.peek()
            if tok is None:
                break
            # Skip annotations like --[[@native]]
            if tok.type == "KW_LOCAL" and self._is_function_def():
                self._parse_local_function()
            elif tok.type == "KW_FUNCTION" and self._is_module_function():
                self._parse_module_function()
            else:
                self.advance()  # skip non-function top-level tokens
        return SimpleIR(functions=self.functions)

    def _is_function_def(self) -> bool:
        """Check if 'local' is followed by 'function'."""
        tok = self.peek(1)
        return tok is not None and tok.type == "KW_FUNCTION"

    def _is_module_function(self) -> bool:
        """Check if 'function' is followed by Module.name pattern."""
        tok1 = self.peek(1)
        tok2 = self.peek(2)
        tok3 = self.peek(3)
        return (
            tok1 is not None
            and tok1.type == "IDENT"
            and tok2 is not None
            and tok2.type == "DOT"
            and tok3 is not None
            and tok3.type == "IDENT"
        )

    def _parse_local_function(self) -> None:
        self.expect("KW_LOCAL")
        self.expect("KW_FUNCTION")
        name_tok = self.expect("IDENT")
        params = self._parse_param_list()
        ops: list[OpIR] = []
        self._parse_body(ops)
        self.expect("KW_END")
        # Ensure function ends with a return
        if not ops or ops[-1].kind not in ("ret", "ret_void"):
            ops.append(OpIR(kind="ret_void"))
        self.functions.append(FunctionIR(
            name=name_tok.value,
            params=params,
            ops=ops,
        ))

    def _parse_module_function(self) -> None:
        self.expect("KW_FUNCTION")
        module_tok = self.expect("IDENT")
        self.expect("DOT")
        name_tok = self.expect("IDENT")
        full_name = f"{module_tok.value}_{name_tok.value}"
        params = self._parse_param_list()
        ops: list[OpIR] = []
        self._parse_body(ops)
        self.expect("KW_END")
        if not ops or ops[-1].kind not in ("ret", "ret_void"):
            ops.append(OpIR(kind="ret_void"))
        self.functions.append(FunctionIR(
            name=full_name,
            params=params,
            ops=ops,
        ))

    def _parse_param_list(self) -> list[str]:
        self.expect("LPAREN")
        params: list[str] = []
        while True:
            tok = self.peek()
            if tok is None or tok.type == "RPAREN":
                break
            if tok.type == "IDENT":
                params.append(self.advance().value)
                # Skip type annotation
                if self.match("COLON"):
                    self._skip_type_annotation()
            if not self.match("COMMA"):
                break
        self.expect("RPAREN")
        # Skip return type annotation
        if self.match("COLON"):
            self._skip_type_annotation()
        return params

    def _skip_type_annotation(self) -> None:
        """Skip a type annotation like `: number` or `: (number, number)` or `: Vector3`."""
        depth = 0
        while True:
            tok = self.peek()
            if tok is None:
                break
            if tok.type == "LPAREN":
                depth += 1
                self.advance()
            elif tok.type == "RPAREN":
                if depth == 0:
                    break
                depth -= 1
                self.advance()
            elif tok.type == "LBRACE":
                depth += 1
                self.advance()
            elif tok.type == "RBRACE":
                if depth == 0:
                    break
                depth -= 1
                self.advance()
            elif depth == 0 and tok.type in ("COMMA", "KW_END", "KW_LOCAL", "KW_FUNCTION", "KW_IF", "KW_FOR", "KW_RETURN"):
                break
            elif depth == 0 and tok.value in (")", "]", "}"):
                break
            else:
                self.advance()

    def _parse_body(self, ops: list[OpIR]) -> None:
        """Parse statements until 'end', 'else', 'elseif'."""
        while True:
            tok = self.peek()
            if tok is None:
                break
            if tok.type in ("KW_END", "KW_ELSE", "KW_ELSEIF"):
                break
            self._parse_statement(ops)

    def _parse_statement(self, ops: list[OpIR]) -> None:
        tok = self.peek()
        if tok is None:
            return

        if tok.type == "KW_LOCAL":
            self._parse_local_decl(ops)
        elif tok.type == "KW_RETURN":
            self._parse_return(ops)
        elif tok.type == "KW_IF":
            self._parse_if(ops)
        elif tok.type == "KW_FOR":
            self._parse_for(ops)
        elif tok.type == "IDENT":
            self._parse_assignment_or_call(ops)
        else:
            self.advance()  # skip unknown

    def _parse_local_decl(self, ops: list[OpIR]) -> None:
        """Parse: local name [: type] [= expr]

        Also handles multi-assignment: local a, b = func()
        """
        self.expect("KW_LOCAL")
        name_tok = self.expect("IDENT")
        name = name_tok.value

        # Check for multi-variable declaration: local a, b = ...
        extra_names: list[str] = []
        while self.match("COMMA"):
            extra_names.append(self.expect("IDENT").value)

        # Skip type annotation
        if self.match("COLON"):
            self._skip_type_annotation()
        if self.match("ASSIGN"):
            expr_var = self._parse_expression(ops)
            if extra_names:
                # Multi-return assignment: local a, b = func()
                # The call result is a tuple; unpack it
                ops.append(OpIR(kind="copy", args=[expr_var], out=name))
                for i, extra in enumerate(extra_names):
                    # For multi-return, we need tuple_get
                    ops.append(OpIR(kind="tuple_get", args=[expr_var], value=i + 1, out=extra))
            elif expr_var != name:
                # Rename the last op's output to the target variable name
                # This avoids store_local which doesn't populate 'defined'
                if ops and ops[-1].out == expr_var:
                    ops[-1].out = name
                else:
                    ops.append(OpIR(kind="copy", args=[expr_var], out=name))
        else:
            ops.append(OpIR(kind="const", out=name, value=0))

    def _parse_return(self, ops: list[OpIR]) -> None:
        self.expect("KW_RETURN")
        tok = self.peek()
        if tok is None or tok.type in ("KW_END", "KW_ELSE", "KW_ELSEIF"):
            ops.append(OpIR(kind="ret_void"))
            return
        ret_var = self._parse_expression(ops)
        # Check for multi-return (comma-separated)
        if self.peek() and self.peek().type == "COMMA":  # type: ignore[union-attr]
            ret_vars = [ret_var]
            while self.match("COMMA"):
                ret_vars.append(self._parse_expression(ops))
            # Pack multi-return as tuple
            out = self._temp()
            ops.append(OpIR(kind="tuple_build", args=ret_vars, out=out))
            ops.append(OpIR(kind="ret", args=[out]))
        else:
            ops.append(OpIR(kind="ret", args=[ret_var]))

    def _parse_expression(self, ops: list[OpIR]) -> str:
        """Parse an expression and return the variable name holding the result."""
        return self._parse_or_expr(ops)

    def _parse_or_expr(self, ops: list[OpIR]) -> str:
        left = self._parse_and_expr(ops)
        while self.peek() and self.peek().type == "KW_OR":  # type: ignore[union-attr]
            self.advance()
            right = self._parse_and_expr(ops)
            out = self._temp()
            ops.append(OpIR(kind="bool_or", args=[left, right], out=out))
            left = out
        return left

    def _parse_and_expr(self, ops: list[OpIR]) -> str:
        left = self._parse_comparison(ops)
        while self.peek() and self.peek().type == "KW_AND":  # type: ignore[union-attr]
            self.advance()
            right = self._parse_comparison(ops)
            out = self._temp()
            ops.append(OpIR(kind="bool_and", args=[left, right], out=out))
            left = out
        return left

    def _parse_comparison(self, ops: list[OpIR]) -> str:
        left = self._parse_add_sub(ops)
        cmp_ops = {"LT": "compare_lt", "GT": "compare_gt", "LTEQ": "compare_le",
                    "GTEQ": "compare_ge", "EQ": "compare_eq", "NEQ": "compare_ne"}
        while self.peek() and self.peek().type in cmp_ops:  # type: ignore[union-attr]
            op_tok = self.advance()
            right = self._parse_add_sub(ops)
            out = self._temp()
            ops.append(OpIR(kind=cmp_ops[op_tok.type], args=[left, right], out=out))
            left = out
        return left

    def _parse_add_sub(self, ops: list[OpIR]) -> str:
        left = self._parse_mul_div(ops)
        while self.peek() and self.peek().type in ("PLUS", "MINUS"):  # type: ignore[union-attr]
            op_tok = self.advance()
            right = self._parse_mul_div(ops)
            out = self._temp()
            kind = "add" if op_tok.type == "PLUS" else "sub"
            ops.append(OpIR(kind=kind, args=[left, right], out=out, fast_float=True))
            left = out
        return left

    def _parse_mul_div(self, ops: list[OpIR]) -> str:
        left = self._parse_power(ops)
        while self.peek() and self.peek().type in ("STAR", "SLASH", "PERCENT"):  # type: ignore[union-attr]
            op_tok = self.advance()
            right = self._parse_power(ops)
            out = self._temp()
            kind_map = {"STAR": "mul", "SLASH": "div", "PERCENT": "mod"}
            ops.append(OpIR(kind=kind_map[op_tok.type], args=[left, right], out=out, fast_float=True))
            left = out
        return left

    def _parse_power(self, ops: list[OpIR]) -> str:
        base = self._parse_unary(ops)
        if self.peek() and self.peek().type == "CARET":  # type: ignore[union-attr]
            self.advance()
            exp = self._parse_unary(ops)
            out = self._temp()
            ops.append(OpIR(kind="pow", args=[base, exp], out=out, fast_float=True))
            base = out
        return base

    def _parse_unary(self, ops: list[OpIR]) -> str:
        tok = self.peek()
        if tok and tok.type == "MINUS":
            self.advance()
            operand = self._parse_primary(ops)
            out = self._temp()
            ops.append(OpIR(kind="neg", args=[operand], out=out, fast_float=True))
            return out
        if tok and tok.type == "KW_NOT":
            self.advance()
            operand = self._parse_primary(ops)
            out = self._temp()
            ops.append(OpIR(kind="bool_not", args=[operand], out=out))
            return out
        if tok and tok.type == "HASH":
            self.advance()
            operand = self._parse_primary(ops)
            out = self._temp()
            ops.append(OpIR(kind="len", args=[operand], out=out, type_hint="int"))
            return out
        return self._parse_primary(ops)

    def _parse_primary(self, ops: list[OpIR]) -> str:
        tok = self.peek()
        if tok is None:
            raise SyntaxError("Unexpected end of input in expression")

        # Number literal
        if tok.type == "NUMBER":
            self.advance()
            out = self._temp()
            val = float(tok.value)
            if val == int(val) and "." not in tok.value and "e" not in tok.value.lower():
                ops.append(OpIR(kind="const", out=out, value=int(val)))
            else:
                ops.append(OpIR(kind="const_float", out=out, f_value=val))
            return self._parse_postfix(ops, out)

        # Boolean
        if tok.type == "KW_TRUE":
            self.advance()
            out = self._temp()
            ops.append(OpIR(kind="const", out=out, value=1))
            return out
        if tok.type == "KW_FALSE":
            self.advance()
            out = self._temp()
            ops.append(OpIR(kind="const", out=out, value=0))
            return out

        # Nil
        if tok.type == "KW_NIL":
            self.advance()
            out = self._temp()
            ops.append(OpIR(kind="const_none", out=out))
            return out

        # Parenthesized expression
        if tok.type == "LPAREN":
            self.advance()
            result = self._parse_expression(ops)
            self.expect("RPAREN")
            return self._parse_postfix(ops, result)

        # Table constructor
        if tok.type == "LBRACE":
            return self._parse_table_constructor(ops)

        # Identifier (variable, function call, field access)
        if tok.type == "IDENT":
            name = self.advance().value
            return self._parse_postfix(ops, name)

        # String
        if tok.type in ("STRING_DQ", "STRING_SQ"):
            self.advance()
            out = self._temp()
            ops.append(OpIR(kind="const_string", out=out, s_value=tok.value[1:-1]))
            return out

        raise SyntaxError(
            f"Unexpected token {tok.type} ({tok.value!r}) at line {tok.line}:{tok.col}"
        )

    def _parse_postfix(self, ops: list[OpIR], base: str) -> str:
        """Handle .field, [index], (args) after a primary expression."""
        while True:
            tok = self.peek()
            if tok is None:
                break

            # Function call
            if tok.type == "LPAREN":
                self.advance()
                call_args = []
                while True:
                    t = self.peek()
                    if t is None or t.type == "RPAREN":
                        break
                    call_args.append(self._parse_expression(ops))
                    if not self.match("COMMA"):
                        break
                self.expect("RPAREN")
                out = self._temp()
                # Check for math intrinsics
                if base in MATH_INTRINSICS:
                    intrinsic = MATH_INTRINSICS[base]
                    ops.append(OpIR(
                        kind="call_intrinsic",
                        s_value=intrinsic,
                        args=call_args,
                        out=out,
                        fast_float=True,
                    ))
                else:
                    ops.append(OpIR(
                        kind="call_func",
                        s_value=base,
                        args=call_args,
                        out=out,
                    ))
                base = out

            # Field access
            elif tok.type == "DOT":
                self.advance()
                field_tok = self.expect("IDENT")
                # Compose qualified name (e.g., "math.sqrt")
                base = f"{base}.{field_tok.value}"

            # Index access
            elif tok.type == "LBRACKET":
                self.advance()
                idx_var = self._parse_expression(ops)
                self.expect("RBRACKET")
                out = self._temp()
                ops.append(OpIR(
                    kind="get_item",
                    args=[base, idx_var],
                    out=out,
                ))
                base = out

            else:
                break

        return base

    def _parse_table_constructor(self, ops: list[OpIR]) -> str:
        self.expect("LBRACE")
        out = self._temp()
        ops.append(OpIR(kind="dict_new", out=out, args=[]))
        while True:
            tok = self.peek()
            if tok is None or tok.type == "RBRACE":
                break
            # Key = Value
            if tok.type == "IDENT" and self.peek(1) and self.peek(1).type == "ASSIGN":  # type: ignore[union-attr]
                key_name = self.advance().value
                self.expect("ASSIGN")
                val_var = self._parse_expression(ops)
                key_tmp = self._temp()
                ops.append(OpIR(kind="const_string", out=key_tmp, s_value=key_name))
                ops.append(OpIR(kind="set_item", args=[out, key_tmp, val_var]))
            else:
                # Array-style element
                val_var = self._parse_expression(ops)
                ops.append(OpIR(kind="list_append", args=[out, val_var]))
            self.match("COMMA")
            self.match("SEMICOLON")
        self.expect("RBRACE")
        return out

    def _parse_if(self, ops: list[OpIR], is_elseif: bool = False) -> None:
        if is_elseif:
            self.expect("KW_ELSEIF")
        else:
            self.expect("KW_IF")
        label_id = self._temp().replace("__t", "")

        cond_var = self._parse_expression(ops)
        self.expect("KW_THEN")

        else_label = f"__else_{label_id}"
        end_label = f"__endif_{label_id}"

        ops.append(OpIR(kind="jump_if_false", args=[cond_var], s_value=else_label))

        # Then body
        self._parse_body(ops)

        tok = self.peek()
        if tok and tok.type == "KW_ELSEIF":
            ops.append(OpIR(kind="jump", s_value=end_label))
            ops.append(OpIR(kind="label", s_value=else_label))
            self._parse_if(ops, is_elseif=True)  # Recursive for elseif
            ops.append(OpIR(kind="label", s_value=end_label))
        elif tok and tok.type == "KW_ELSE":
            self.advance()
            ops.append(OpIR(kind="jump", s_value=end_label))
            ops.append(OpIR(kind="label", s_value=else_label))
            self._parse_body(ops)
            self.expect("KW_END")
            ops.append(OpIR(kind="label", s_value=end_label))
        else:
            self.expect("KW_END")
            ops.append(OpIR(kind="label", s_value=else_label))

    def _parse_for(self, ops: list[OpIR]) -> None:
        self.expect("KW_FOR")
        var_tok = self.expect("IDENT")
        var_name = var_tok.value

        self.expect("ASSIGN")
        start_var = self._parse_expression(ops)
        self.expect("COMMA")
        stop_var = self._parse_expression(ops)

        step_var = None
        if self.match("COMMA"):
            step_var = self._parse_expression(ops)

        self.expect("KW_DO")

        label_id = self._temp().replace("__t", "")
        loop_label = f"__for_loop_{label_id}"
        end_label = f"__for_end_{label_id}"

        # Initialize counter
        ops.append(OpIR(kind="copy", args=[start_var], out=var_name))

        # Loop header
        ops.append(OpIR(kind="label", s_value=loop_label))

        # Condition check: counter <= stop (for positive step)
        cond = self._temp()
        ops.append(OpIR(kind="compare_le", args=[var_name, stop_var], out=cond))
        ops.append(OpIR(kind="jump_if_false", args=[cond], s_value=end_label))

        # Loop body
        self._parse_body(ops)
        self.expect("KW_END")

        # Increment
        if step_var:
            ops.append(OpIR(kind="add", args=[var_name, step_var], out=var_name, fast_float=True))
        else:
            one = self._temp()
            ops.append(OpIR(kind="const", out=one, value=1))
            ops.append(OpIR(kind="add", args=[var_name, one], out=var_name, fast_float=True))

        ops.append(OpIR(kind="jump", s_value=loop_label))
        ops.append(OpIR(kind="label", s_value=end_label))

    def _parse_assignment_or_call(self, ops: list[OpIR]) -> None:
        """Parse assignment (a = expr, a.b = expr, a[i] = expr) or bare function call."""
        name = self.advance().value

        # Handle compound names like s.value, s.velocity, etc.
        if self.peek() and self.peek().type == "DOT":  # type: ignore[union-attr]
            self.advance()
            field = self.expect("IDENT").value
            target = f"{name}.{field}"

            if self.peek() and self.peek().type == "ASSIGN":  # type: ignore[union-attr]
                self.advance()
                val_var = self._parse_expression(ops)
                # Field store
                key_tmp = self._temp()
                ops.append(OpIR(kind="const_string", out=key_tmp, s_value=field))
                ops.append(OpIR(kind="set_attr", args=[name, key_tmp, val_var], s_value=field))
                return

            # Could be a function call: Module.func(...)
            if self.peek() and self.peek().type == "LPAREN":  # type: ignore[union-attr]
                result = self._parse_postfix(ops, target)
                return

        # Simple assignment or compound assignment
        if self.peek() and self.peek().type == "ASSIGN":  # type: ignore[union-attr]
            self.advance()
            val_var = self._parse_expression(ops)
            # Rename the last op's output to the target variable name
            if ops and ops[-1].out == val_var:
                ops[-1].out = name
            else:
                ops.append(OpIR(kind="copy", args=[val_var], out=name))
            return

        # += operator
        tok = self.peek()
        if tok and tok.value == "+" and self.peek(1) and self.peek(1).type == "ASSIGN":  # type: ignore[union-attr]
            self.advance()  # +
            self.advance()  # =
            val_var = self._parse_expression(ops)
            ops.append(OpIR(kind="add", args=[name, val_var], out=name, fast_float=True))
            return

        # Bare function call
        if self.peek() and self.peek().type == "LPAREN":  # type: ignore[union-attr]
            self._parse_postfix(ops, name)
            return

        # Index assignment: name[expr] = expr
        if self.peek() and self.peek().type == "LBRACKET":  # type: ignore[union-attr]
            self.advance()
            idx_var = self._parse_expression(ops)
            self.expect("RBRACKET")
            self.expect("ASSIGN")
            val_var = self._parse_expression(ops)
            ops.append(OpIR(kind="set_item", args=[name, idx_var, val_var]))
            return


# ---------------------------------------------------------------------------
# High-level API
# ---------------------------------------------------------------------------


def parse_luau_file(path: Path, function_filter: str | None = None) -> SimpleIR:
    """Parse a Luau file and return SimpleIR."""
    source = path.read_text(encoding="utf-8")
    # Strip --!strict directive
    source = re.sub(r"^--!strict\s*\n?", "", source)
    # Strip @native annotations
    source = re.sub(r"--\[\[@native\]\]\s*\n?", "", source)

    tokens = tokenize(source)
    parser = LuauPhysicsParser(tokens)
    ir = parser.parse()

    if function_filter:
        ir.functions = [f for f in ir.functions if function_filter in f.name]

    return ir


def parse_physics_directory(physics_dir: Path, function_filter: str | None = None) -> SimpleIR:
    """Parse all .luau files in a physics directory."""
    combined = SimpleIR()
    for luau_file in sorted(physics_dir.glob("*.luau")):
        # Skip spec and bench files
        if ".spec." in luau_file.name or ".bench." in luau_file.name:
            continue
        try:
            ir = parse_luau_file(luau_file, function_filter)
            combined.functions.extend(ir.functions)
            print(f"  Parsed {luau_file.name}: {len(ir.functions)} functions", file=sys.stderr)
        except SyntaxError as e:
            print(f"  SKIP {luau_file.name}: {e}", file=sys.stderr)
    return combined


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Transpile Vertigo physics Luau modules to Molt SimpleIR"
    )
    parser.add_argument("--input", required=True, help="Path to .luau file or directory")
    parser.add_argument("--output", default="-", help="Output JSON file (default: stdout)")
    parser.add_argument("--function", default=None, help="Only emit functions matching this name")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output")
    args = parser.parse_args()

    input_path = Path(args.input)
    if input_path.is_dir():
        ir = parse_physics_directory(input_path, args.function)
    elif input_path.is_file():
        ir = parse_luau_file(input_path, args.function)
    else:
        print(f"Error: {input_path} does not exist", file=sys.stderr)
        sys.exit(1)

    print(f"Total functions: {len(ir.functions)}", file=sys.stderr)
    for func in ir.functions:
        print(f"  {func.name}({', '.join(func.params)}): {len(func.ops)} ops", file=sys.stderr)

    indent = 2 if args.pretty else None
    json_str = json.dumps(ir.to_dict(), indent=indent)

    if args.output == "-":
        print(json_str)
    else:
        Path(args.output).write_text(json_str, encoding="utf-8")
        print(f"Written to {args.output}", file=sys.stderr)


if __name__ == "__main__":
    main()
