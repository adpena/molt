from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from molt.cli.c_api_symbols import C_API_TOKEN as _C_API_TOKEN
from molt.cli.c_api_symbols import C_API_TOKEN_RE as _C_API_TOKEN_RE

_C_BLOCK_COMMENT_RE = re.compile(r"/\*.*?\*/", flags=re.DOTALL)
_C_LINE_COMMENT_RE = re.compile(r"//.*?$", flags=re.MULTILINE)
_CYTHON_PREPROCESSOR_DIRECTIVE_RE = re.compile(
    r"^\s*#\s*(?:include|define|if|ifdef|ifndef|elif|elifdef|elifndef|else|endif|pragma|error|warning)\b"
)
_PY_TRIPLE_STRING_RE = re.compile(
    r'"""(?:\\.|[\s\S])*?"""|\'\'\'(?:\\.|[\s\S])*?\'\'\''
)
_C_STRING_LITERAL_RE = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'')
_C_PREPROCESSOR_CONDITION_RE = re.compile(
    r"(?m)^\s*#\s*(?:if|ifdef|ifndef|elif|elifdef|elifndef)\b.*$"
)
_C_PREPROCESSOR_INCLUDE_RE = re.compile(r"(?m)^\s*#\s*include\b.*$")
_NUMPY_FAIL_FAST_SYMBOL_RE = re.compile(
    rf"_molt_numpy_unavailable_[A-Za-z0-9_]+\(\s*\"(?P<symbol>{_C_API_TOKEN})\""
)
_C_IDENTIFIER = r"[A-Za-z_][A-Za-z0-9_]*"
_C_PREPROCESSOR_DIRECTIVE_RE = re.compile(
    r"^\s*#\s*(?P<kind>if|ifdef|ifndef|elif|else|endif|define|undef)\b(?P<expr>.*)$"
)
_C_PREPROCESSOR_DEFINE_RE = re.compile(
    rf"^\s*#\s*define\s+(?P<symbol>{_C_IDENTIFIER})\b"
)
_C_DEFINE_SYMBOL_RE = re.compile(rf"^\s*#\s*define\s+(?P<symbol>{_C_API_TOKEN})\b")
_C_FUNCTION_LIKE_DEFINE_RE = re.compile(
    rf"^\s*#\s*define\s+{_C_IDENTIFIER}\s*\((?P<params>[^)]*)\)"
)
_C_TAG_SYMBOL_RE = re.compile(
    rf"\b(?:struct|enum|union)\s+(?P<symbol>{_C_API_TOKEN})\b"
)
_C_CPP_TYPE_SYMBOL_RE = re.compile(
    rf"\b(?:class|typename)\s+(?P<symbol>{_C_API_TOKEN})\b"
)
_C_TYPEDEF_FUNCTION_SYMBOL_RE = re.compile(
    rf"\(\s*(?P<symbol>{_C_API_TOKEN})\s*\)\s*\("
)
_C_CPP_TEMPLATE_BLOCK_RE = re.compile(
    r"\btemplate\s*<(?P<body>.*?)>",
    flags=re.DOTALL,
)
_C_CPP_TEMPLATE_FUNCTION_PARAMETER_RE = re.compile(
    rf"\b{_C_IDENTIFIER}\s+(?:\(\*\s*)?(?P<symbol>{_C_API_TOKEN})\s*(?:\)|\()"
)
_C_GENERATED_C_API_PREFIX_RE = re.compile(
    r"\b(?P<prefix>(?:_?Py|Npy|npy|NPY_)[A-Za-z0-9_]*_)\s*##\s*"
    r"[A-Za-z_][A-Za-z0-9_]*"
)
_C_GENERATED_C_API_BROAD_PREFIXES = frozenset(
    {
        "NPY_CPU_FEATURE_",
        "NPY_FPE_",
        "NPY_SIZEOF_",
        "PyArray_",
        "PyDataType_",
        "PyDict_EVENT_",
        "PyFunction_EVENT_",
        "PyUFunc_",
        "_PyConfig_MEMBER_",
        "_Py_asdl_",
    }
)
_C_ENUM_BODY_RE = re.compile(r"\benum\b[^{;]*\{(?P<body>.*?)\}", flags=re.DOTALL)
_C_ENUM_MEMBER_SYMBOL_RE = re.compile(rf"^\s*(?P<symbol>{_C_API_TOKEN})\b")
_C_TOKEN_BEFORE_PAREN_RE = re.compile(rf"\b(?P<symbol>{_C_API_TOKEN})\s*\(")
_CYTHON_INLINE_SYMBOL_RE = re.compile(
    rf"^\s*cdef\s+inline\b.*\b(?P<symbol>{_C_API_TOKEN})\s*\("
)
_C_SINGLE_LINE_FUNCTION_SYMBOL_RE = re.compile(
    rf"(?m)^\s*(?:static\s+inline|static|inline|NPY_NO_EXPORT|PyMODINIT_FUNC)"
    rf"[^\n;{{}}]*\b(?P<symbol>{_C_API_TOKEN})\s*\([^;\n{{}}]*\)\s*\{{"
)
_C_SPLIT_LINE_FUNCTION_SYMBOL_RE = re.compile(
    rf"(?m)^\s*(?:static\s+inline|static|inline|NPY_NO_EXPORT|PyMODINIT_FUNC)"
    rf"[^\n;{{}}]*\n\s*(?P<symbol>{_C_API_TOKEN})\s*"
    rf"\([^;\n{{}}]*\)\s*\n?\s*\{{"
)
_C_PLAIN_SPLIT_LINE_FUNCTION_SYMBOL_RE = re.compile(
    rf"(?m)^\s*[A-Za-z_][A-Za-z0-9_\s\*]*\*?\s*\n"
    rf"\s*(?P<symbol>{_C_API_TOKEN})\s*\([^;\n{{}}]*\)\s*\n?\s*\{{"
)
_C_MULTILINE_FUNCTION_SYMBOL_RE = re.compile(
    rf"(?ms)^\s*(?:static\s+inline|static|inline|NPY_NO_EXPORT|PyMODINIT_FUNC)"
    rf"[^;{{}}]*?\b(?P<symbol>{_C_API_TOKEN})\s*\([^;{{}}]*?\)\s*\{{"
)
_C_SIMPLE_FUNCTION_DECLARATION_RE = re.compile(
    rf"(?m)^\s*(?!typedef\b)(?!#)"
    rf"[A-Za-z_][A-Za-z0-9_\s\*]*\b(?P<symbol>{_C_API_TOKEN})"
    rf"\s*\([^;\n{{}}]*\)\s*;"
)
_C_CONTROL_PREFIXES = (
    "if ",
    "if(",
    "for ",
    "for(",
    "while ",
    "while(",
    "switch ",
    "switch(",
    "return ",
    "else",
    "do ",
)


def _split_top_level_bool_operator(expr: str, operator: str) -> list[str]:
    parts: list[str] = []
    depth = 0
    start = 0
    idx = 0
    while idx < len(expr):
        char = expr[idx]
        if char == "(":
            depth += 1
            idx += 1
            continue
        if char == ")":
            depth = max(0, depth - 1)
            idx += 1
            continue
        if depth == 0 and expr.startswith(operator, idx):
            parts.append(expr[start:idx].strip())
            idx += len(operator)
            start = idx
            continue
        idx += 1
    if parts:
        parts.append(expr[start:].strip())
    return parts


def _strip_outer_parens(expr: str) -> str:
    stripped = expr.strip()
    while stripped.startswith("(") and stripped.endswith(")"):
        depth = 0
        balanced = True
        for idx, char in enumerate(stripped):
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
                if depth == 0 and idx != len(stripped) - 1:
                    balanced = False
                    break
            if depth < 0:
                balanced = False
                break
        if not balanced or depth != 0:
            break
        stripped = stripped[1:-1].strip()
    return stripped


@dataclass
class _PreprocessorFrame:
    parent_active: bool
    active: bool
    saw_active_branch: bool
    unknown_condition: bool


def _evaluate_preprocessor_condition(
    expr: str,
    *,
    defined_symbols: frozenset[str],
) -> bool | None:
    stripped = _strip_outer_parens(expr.replace("\\", "").strip())
    or_parts = _split_top_level_bool_operator(stripped, "||")
    if or_parts:
        values = [
            _evaluate_preprocessor_condition(part, defined_symbols=defined_symbols)
            for part in or_parts
        ]
        if any(value is True for value in values):
            return True
        if all(value is False for value in values):
            return False
        return None
    and_parts = _split_top_level_bool_operator(stripped, "&&")
    if and_parts:
        values = [
            _evaluate_preprocessor_condition(part, defined_symbols=defined_symbols)
            for part in and_parts
        ]
        if any(value is False for value in values):
            return False
        if all(value is True for value in values):
            return True
        return None
    comparison_match = re.fullmatch(
        r"(?P<left>[0-9]+)\s*(?P<op>==|!=)\s*(?P<right>[0-9]+)",
        stripped,
    )
    if comparison_match is not None:
        left = int(comparison_match.group("left"))
        right = int(comparison_match.group("right"))
        if comparison_match.group("op") == "==":
            return left == right
        return left != right
    if stripped in {"0", "0L", "0UL", "0U"}:
        return False
    if re.fullmatch(r"[1-9][0-9]*[uUlL]*", stripped):
        return True

    defined_match = re.fullmatch(
        rf"defined\s*(?:\(\s*(?P<paren>{_C_IDENTIFIER})\s*\)|(?P<bare>{_C_IDENTIFIER}))",
        stripped,
    )
    if defined_match is not None:
        symbol = defined_match.group("paren") or defined_match.group("bare")
        return symbol in defined_symbols

    not_defined_match = re.fullmatch(
        rf"!\s*defined\s*(?:\(\s*(?P<paren>{_C_IDENTIFIER})\s*\)|(?P<bare>{_C_IDENTIFIER}))",
        stripped,
    )
    if not_defined_match is not None:
        symbol = not_defined_match.group("paren") or not_defined_match.group("bare")
        return symbol not in defined_symbols

    identifier_match = re.fullmatch(_C_IDENTIFIER, stripped)
    if identifier_match is not None:
        return stripped in defined_symbols

    return None


def _strip_inactive_preprocessor_blocks(
    text: str,
    *,
    defined_symbols: frozenset[str],
) -> str:
    lines: list[str] = []
    input_lines = text.splitlines(keepends=True)
    stack: list[_PreprocessorFrame] = []
    active_definitions = set(defined_symbols)

    def current_active() -> bool:
        return stack[-1].active if stack else True

    idx = 0
    while idx < len(input_lines):
        line = input_lines[idx]
        body = line.rstrip("\r\n")
        newline = line[len(body) :]
        directive_body = body
        directive_newlines = [newline]
        while directive_body.rstrip().endswith("\\") and idx + 1 < len(input_lines):
            idx += 1
            continuation = input_lines[idx]
            continuation_body = continuation.rstrip("\r\n")
            directive_body = directive_body.rstrip()[:-1] + " " + continuation_body
            directive_newlines.append(continuation[len(continuation_body) :])

        directive_match = _C_PREPROCESSOR_DIRECTIVE_RE.match(directive_body)
        if directive_match is not None:
            kind = directive_match.group("kind")
            expr = directive_match.group("expr")
            if kind == "define":
                if current_active():
                    define_match = _C_PREPROCESSOR_DEFINE_RE.match(directive_body)
                    if define_match is not None:
                        active_definitions.add(define_match.group("symbol"))
                lines.extend(directive_newlines)
                idx += 1
                continue
            if kind == "undef":
                if current_active():
                    symbol = expr.strip().split(None, 1)[0] if expr.strip() else ""
                    active_definitions.discard(symbol)
                lines.extend(directive_newlines)
                idx += 1
                continue
            if kind in {"ifdef", "ifndef", "if"}:
                parent_active = current_active()
                if kind == "ifdef":
                    condition = expr.strip() in active_definitions
                elif kind == "ifndef":
                    condition = expr.strip() not in active_definitions
                else:
                    condition = _evaluate_preprocessor_condition(
                        expr,
                        defined_symbols=frozenset(active_definitions),
                    )
                if condition is None:
                    stack.append(
                        _PreprocessorFrame(
                            parent_active=parent_active,
                            active=parent_active,
                            saw_active_branch=False,
                            unknown_condition=True,
                        )
                    )
                else:
                    stack.append(
                        _PreprocessorFrame(
                            parent_active=parent_active,
                            active=parent_active and condition,
                            saw_active_branch=condition,
                            unknown_condition=False,
                        )
                    )
            elif kind == "elif" and stack:
                frame = stack[-1]
                if frame.unknown_condition:
                    frame.active = frame.parent_active
                elif frame.saw_active_branch:
                    frame.active = False
                else:
                    condition = _evaluate_preprocessor_condition(
                        expr,
                        defined_symbols=frozenset(active_definitions),
                    )
                    if condition is None:
                        frame.active = frame.parent_active
                        frame.unknown_condition = True
                    else:
                        frame.active = frame.parent_active and condition
                        frame.saw_active_branch = condition
            elif kind == "else" and stack:
                frame = stack[-1]
                if frame.unknown_condition:
                    frame.active = frame.parent_active
                else:
                    frame.active = frame.parent_active and not frame.saw_active_branch
                    frame.saw_active_branch = True
            elif kind == "endif" and stack:
                stack.pop()
            lines.extend(directive_newlines)
            idx += 1
            continue

        if current_active():
            lines.append(line)
        else:
            lines.append(newline)
        idx += 1

    return "".join(lines)


def _extract_preprocessor_defined_symbols(text: str) -> set[str]:
    sanitized = _strip_c_like_comments_and_literals(text)
    return {
        match.group("symbol")
        for line in sanitized.splitlines()
        if (match := _C_PREPROCESSOR_DEFINE_RE.match(line)) is not None
    }


def _extract_project_generated_c_api_prefixes(text: str) -> set[str]:
    sanitized = _strip_c_like_comments_and_literals_preserving_hash(text)
    return {
        match.group("prefix")
        for match in _C_GENERATED_C_API_PREFIX_RE.finditer(sanitized)
        if len(match.group("prefix")) > 5
        and match.group("prefix") not in _C_GENERATED_C_API_BROAD_PREFIXES
    }


def _matches_project_generated_c_api_prefix(
    symbol: str,
    prefixes: frozenset[str] | set[str],
) -> bool:
    return any(symbol.startswith(prefix) for prefix in prefixes)


def _add_py_parameter_names(header: str, symbols: set[str]) -> None:
    if "(" not in header or ")" not in header:
        return
    params = header.rsplit("(", 1)[1].rsplit(")", 1)[0]
    for param in params.split(","):
        tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(param)]
        if len(tokens) >= 2 and tokens[-1] != tokens[0]:
            symbols.add(tokens[-1])


def _add_macro_parameter_names(text: str, symbols: set[str]) -> None:
    for line in text.splitlines():
        define_match = _C_FUNCTION_LIKE_DEFINE_RE.match(line)
        if define_match is None:
            continue
        for param in define_match.group("params").split(","):
            stripped = param.strip()
            if _C_API_TOKEN_RE.fullmatch(stripped):
                symbols.add(stripped)


def _add_cpp_template_parameter_names(text: str, symbols: set[str]) -> None:
    for template_match in _C_CPP_TEMPLATE_BLOCK_RE.finditer(text):
        body = template_match.group("body")
        for type_match in _C_CPP_TYPE_SYMBOL_RE.finditer(body):
            symbols.add(type_match.group("symbol"))
        for function_match in _C_CPP_TEMPLATE_FUNCTION_PARAMETER_RE.finditer(body):
            symbols.add(function_match.group("symbol"))


def _function_symbol_from_header(header: str) -> str | None:
    matches = list(_C_TOKEN_BEFORE_PAREN_RE.finditer(header))
    if not matches:
        return None
    return matches[0].group("symbol")


def _strip_cython_hash_comments(text: str) -> str:
    def cython_comment_start(body: str) -> int:
        search_from = 0
        while True:
            comment_start = body.find("#", search_from)
            if comment_start < 0:
                return -1
            previous_is_hash = comment_start > 0 and body[comment_start - 1] == "#"
            next_is_hash = (
                comment_start + 1 < len(body) and body[comment_start + 1] == "#"
            )
            if previous_is_hash or next_is_hash:
                search_from = comment_start + 1
                continue
            return comment_start

    lines: list[str] = []
    for line in text.splitlines(keepends=True):
        body = line.rstrip("\r\n")
        newline = line[len(body) :]
        stripped = body.lstrip()
        if stripped.startswith("#"):
            lines.append(
                line if _CYTHON_PREPROCESSOR_DIRECTIVE_RE.match(body) else newline
            )
            continue
        comment_start = cython_comment_start(body)
        if comment_start >= 0:
            lines.append(body[:comment_start].rstrip() + newline)
        else:
            lines.append(line)
    return "".join(lines)


def _strip_preprocessor_macro_definitions(text: str) -> str:
    lines: list[str] = []
    in_macro = False
    for line in text.splitlines(keepends=True):
        body = line.rstrip("\r\n")
        newline = line[len(body) :]
        if in_macro:
            in_macro = body.rstrip().endswith("\\")
            lines.append(newline)
            continue
        if body.lstrip().startswith("#"):
            in_macro = body.rstrip().endswith("\\")
            lines.append(newline)
            continue
        lines.append(line)
    return "".join(lines)


def _strip_c_like_comments_and_literals(text: str) -> str:
    without_blocks = _C_BLOCK_COMMENT_RE.sub(" ", text)
    without_lines = _C_LINE_COMMENT_RE.sub(" ", without_blocks)
    without_triple_strings = _PY_TRIPLE_STRING_RE.sub(" ", without_lines)
    without_string_literals = _C_STRING_LITERAL_RE.sub(" ", without_triple_strings)
    return _strip_cython_hash_comments(without_string_literals)


def _strip_c_like_comments_and_literals_preserving_hash(text: str) -> str:
    without_blocks = _C_BLOCK_COMMENT_RE.sub(" ", text)
    without_lines = _C_LINE_COMMENT_RE.sub(" ", without_blocks)
    without_triple_strings = _PY_TRIPLE_STRING_RE.sub(" ", without_lines)
    return _C_STRING_LITERAL_RE.sub(" ", without_triple_strings)


def _extract_c_api_tokens(
    text: str,
    *,
    strip_py_condition_blocks: bool = True,
    active_preprocessor_symbols: frozenset[str] | None = None,
) -> set[str]:
    del strip_py_condition_blocks
    sanitized = _strip_c_like_comments_and_literals(text)
    if active_preprocessor_symbols is not None:
        sanitized = _strip_inactive_preprocessor_blocks(
            sanitized,
            defined_symbols=active_preprocessor_symbols,
        )
    sanitized = _C_PREPROCESSOR_INCLUDE_RE.sub(" ", sanitized)
    sanitized = _C_PREPROCESSOR_CONDITION_RE.sub(" ", sanitized)
    return {
        match.group("symbol")
        for match in _C_API_TOKEN_RE.finditer(sanitized)
        if not match.group("symbol").endswith("_")
    }


def _extract_file_local_c_api_symbols(
    text: str,
    *,
    active_preprocessor_symbols: frozenset[str] | None = None,
) -> set[str]:
    """Return C/API identifiers that are local to the source file.

    These names are not external C/API requirements, but they also must not be
    promoted to project-wide definitions. Filtering them per file prevents a
    local variable or parameter named like an ABI symbol from hiding a real
    missing symbol in another translation unit.
    """
    local: set[str] = set()
    sanitized = _strip_c_like_comments_and_literals(text)
    _add_macro_parameter_names(sanitized, local)
    _add_cpp_template_parameter_names(sanitized, local)
    for type_match in _C_CPP_TYPE_SYMBOL_RE.finditer(sanitized):
        local.add(type_match.group("symbol"))
    if active_preprocessor_symbols is not None:
        sanitized = _strip_inactive_preprocessor_blocks(
            sanitized,
            defined_symbols=active_preprocessor_symbols,
        )
    sanitized = _strip_preprocessor_macro_definitions(sanitized)
    for function_re in (
        _C_SINGLE_LINE_FUNCTION_SYMBOL_RE,
        _C_SPLIT_LINE_FUNCTION_SYMBOL_RE,
        _C_PLAIN_SPLIT_LINE_FUNCTION_SYMBOL_RE,
        _C_MULTILINE_FUNCTION_SYMBOL_RE,
    ):
        for match in function_re.finditer(sanitized):
            header = match.group(0).lstrip()
            if header.startswith("static"):
                local.add(match.group("symbol"))
    signature_parts: list[str] = []
    brace_depth = 0

    for raw_line in sanitized.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        lowered = line.lower()
        is_control = lowered.startswith(_C_CONTROL_PREFIXES)

        if brace_depth == 0 and not is_control:
            if line.lstrip().startswith("static ") and "=" in line:
                lhs = line.split("=", 1)[0]
                if "(" not in lhs:
                    tokens = [
                        match.group("symbol") for match in _C_API_TOKEN_RE.finditer(lhs)
                    ]
                    if tokens:
                        local.add(tokens[-1])
            signature_parts.append(line)
            if "{" in line:
                header = " ".join(signature_parts).split("{", 1)[0]
                _add_py_parameter_names(header, local)
                if header.lstrip().startswith("static"):
                    symbol = _function_symbol_from_header(header)
                    if symbol is not None:
                        local.add(symbol)
                signature_parts = []
            elif ";" in line or "}" in line:
                signature_parts = []
        elif brace_depth > 0 and not is_control:
            if "=" in line:
                lhs = line.split("=", 1)[0]
                if "(" not in lhs:
                    tokens = [
                        match.group("symbol") for match in _C_API_TOKEN_RE.finditer(lhs)
                    ]
                    if tokens:
                        local.add(tokens[-1])
            elif line.endswith(";") and "(" not in line:
                statement = line[:-1]
                tokens = [
                    match.group("symbol")
                    for match in _C_API_TOKEN_RE.finditer(statement)
                ]
                if tokens:
                    local.add(tokens[-1])

        brace_depth += line.count("{") - line.count("}")
        if brace_depth < 0:
            brace_depth = 0

    return local


def _extract_project_defined_c_api_symbols(
    text: str,
    *,
    include_static_inline: bool = False,
    include_declarations: bool = False,
) -> set[str]:
    """Return C/API symbols defined by the scanned package itself.

    The extension scanner gates unresolved external C/API requirements. Large
    packages such as NumPy define many Py-prefixed, Npy-prefixed, npy-prefixed,
    and NPY-prefixed C functions, macros, type objects, and typedefs in their
    own source tree; counting those as missing Molt C/API symbols makes the scan
    both noisy and wrong. This pass is intentionally line-oriented and
    conservative so it stays fast on generated megabyte-scale C files.
    """
    defined: set[str] = set()
    for raw_line in text.splitlines():
        line = raw_line.strip()
        cython_inline_match = _CYTHON_INLINE_SYMBOL_RE.match(line)
        if cython_inline_match is not None:
            defined.add(cython_inline_match.group("symbol"))
        elif line.startswith("cdef inline "):
            symbol = _function_symbol_from_header(line)
            if symbol is not None:
                defined.add(symbol)

    sanitized_with_macros = _strip_c_like_comments_and_literals(text)
    for macro_line in sanitized_with_macros.splitlines():
        define_match = _C_DEFINE_SYMBOL_RE.match(macro_line)
        if define_match is not None:
            defined.add(define_match.group("symbol"))
    sanitized = _strip_preprocessor_macro_definitions(sanitized_with_macros)
    if include_declarations:
        for declaration_match in _C_SIMPLE_FUNCTION_DECLARATION_RE.finditer(sanitized):
            defined.add(declaration_match.group("symbol"))
    for raw_line in sanitized.splitlines():
        line = raw_line.strip()
        if line.lower().startswith("static const ") and "=" in line:
            lhs = line.split("=", 1)[0]
            tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(lhs)]
            if tokens:
                defined.add(tokens[-1])
    for enum_match in _C_ENUM_BODY_RE.finditer(sanitized):
        for raw_member in enum_match.group("body").split(","):
            member_match = _C_ENUM_MEMBER_SYMBOL_RE.match(raw_member)
            if member_match is not None:
                defined.add(member_match.group("symbol"))
    for function_re in (
        _C_SINGLE_LINE_FUNCTION_SYMBOL_RE,
        _C_SPLIT_LINE_FUNCTION_SYMBOL_RE,
        _C_PLAIN_SPLIT_LINE_FUNCTION_SYMBOL_RE,
        _C_MULTILINE_FUNCTION_SYMBOL_RE,
    ):
        for match in function_re.finditer(sanitized):
            header = match.group(0).lstrip()
            header_prefix = header.split("(", 1)[0]
            static_inline = header.startswith("static") and "inline" in header_prefix
            if not header.startswith("static") or (
                include_static_inline and static_inline
            ):
                defined.add(match.group("symbol"))
    typedef_parts: list[str] | None = None
    signature_parts: list[str] = []
    brace_depth = 0

    for raw_line in sanitized.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        lowered = line.lower()

        for tag_match in _C_TAG_SYMBOL_RE.finditer(line):
            defined.add(tag_match.group("symbol"))
        for type_match in _C_CPP_TYPE_SYMBOL_RE.finditer(line):
            defined.add(type_match.group("symbol"))

        if typedef_parts is not None:
            typedef_parts.append(line)
            if ";" in line:
                statement = " ".join(typedef_parts).split(";", 1)[0]
                typedef_function_match = _C_TYPEDEF_FUNCTION_SYMBOL_RE.search(statement)
                if typedef_function_match is not None:
                    defined.add(typedef_function_match.group("symbol"))
                else:
                    tokens = [
                        match.group("symbol")
                        for match in _C_API_TOKEN_RE.finditer(statement)
                    ]
                    if tokens:
                        defined.add(tokens[-1])
                typedef_parts = None
        elif line.startswith("typedef "):
            typedef_parts = [line]
            if ";" in line:
                statement = line.split(";", 1)[0]
                typedef_function_match = _C_TYPEDEF_FUNCTION_SYMBOL_RE.search(statement)
                if typedef_function_match is not None:
                    defined.add(typedef_function_match.group("symbol"))
                else:
                    tokens = [
                        match.group("symbol")
                        for match in _C_API_TOKEN_RE.finditer(statement)
                    ]
                    if tokens:
                        defined.add(tokens[-1])
                typedef_parts = None

        if line.startswith("}") and line.endswith(";"):
            tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(line)]
            if tokens:
                defined.add(tokens[-1])

        if brace_depth == 0:
            is_control = lowered.startswith(_C_CONTROL_PREFIXES)
            if include_declarations and not is_control and line.endswith(";"):
                statement = line[:-1]
                if "(" in statement and not lowered.startswith(("typedef ", "#")):
                    symbol = _function_symbol_from_header(statement)
                    if symbol is not None:
                        defined.add(symbol)
                elif not lowered.startswith(("#", "typedef ")):
                    tokens = [
                        match.group("symbol")
                        for match in _C_API_TOKEN_RE.finditer(statement)
                    ]
                    if tokens:
                        defined.add(tokens[-1])

            if not is_control and "=" in line and not lowered.startswith("extern "):
                lhs = line.split("=", 1)[0]
                if not lowered.startswith("static ") or lowered.startswith(
                    "static const "
                ):
                    tokens = [
                        match.group("symbol") for match in _C_API_TOKEN_RE.finditer(lhs)
                    ]
                    if tokens:
                        defined.add(tokens[-1])
            elif (
                not is_control
                and line.endswith(";")
                and "(" not in line
                and not lowered.startswith(("extern ", "static ", "typedef ", "#"))
            ):
                statement = line[:-1]
                tokens = [
                    match.group("symbol")
                    for match in _C_API_TOKEN_RE.finditer(statement)
                ]
                if tokens:
                    defined.add(tokens[-1])

            if not is_control:
                signature_parts.append(line)
                if "{" in line:
                    header = " ".join(signature_parts).split("{", 1)[0]
                    stripped_header = header.lstrip()
                    header_prefix = stripped_header.split("(", 1)[0]
                    static_inline = (
                        stripped_header.startswith("static")
                        and "inline" in header_prefix
                    )
                    if not stripped_header.startswith("static") or (
                        include_static_inline and static_inline
                    ):
                        symbol = _function_symbol_from_header(header)
                        if symbol is not None:
                            defined.add(symbol)
                    signature_parts = []
                elif ";" in line or "}" in line:
                    signature_parts = []

        elif include_declarations and line.endswith(";") and "(" not in line:
            statement = line[:-1]
            tokens = [
                match.group("symbol") for match in _C_API_TOKEN_RE.finditer(statement)
            ]
            if tokens:
                defined.add(tokens[-1])

        brace_depth += line.count("{") - line.count("}")
        if brace_depth < 0:
            brace_depth = 0

    return defined


def _load_c_api_scan_surface(
    molt_root: Path,
    *,
    header_path: Path | None = None,
) -> tuple[_ExtensionScanSurface | None, Path, str | None]:
    header_path = header_path or molt_root / "include" / "molt" / "Python.h"
    header_roots: list[Path] = []
    for root in (
        header_path.parent,
        header_path.parent.parent if header_path.parent.name == "molt" else None,
    ):
        if root is None:
            continue
        resolved = root.resolve()
        if resolved not in header_roots:
            header_roots.append(resolved)
    runtime_tokens: set[str] = set()
    numpy_tokens: set[str] = set()
    fail_fast_tokens: set[str] = set()
    try:
        header_text = header_path.read_text()
    except OSError as exc:
        return None, header_path, str(exc)
    runtime_tokens.update(
        _extract_c_api_tokens(header_text, strip_py_condition_blocks=False)
    )
    for datetime_header in tuple(root / "datetime.h" for root in header_roots):
        if not datetime_header.exists():
            continue
        try:
            datetime_text = datetime_header.read_text()
        except OSError:
            datetime_text = ""
        if datetime_text:
            runtime_tokens.update(
                _extract_c_api_tokens(datetime_text, strip_py_condition_blocks=False)
            )
    numpy_include_root = molt_root / "include" / "numpy"
    if numpy_include_root.exists():
        for numpy_header in sorted(numpy_include_root.rglob("*.h")):
            try:
                numpy_text = numpy_header.read_text()
            except OSError:
                continue
            numpy_tokens.update(
                _extract_c_api_tokens(numpy_text, strip_py_condition_blocks=False)
            )
            fail_fast_tokens.update(_extract_numpy_fail_fast_symbols(numpy_text))
    source_compile_only = numpy_tokens - runtime_tokens - fail_fast_tokens
    surface = _ExtensionScanSurface(
        runtime_backed=frozenset(runtime_tokens - fail_fast_tokens),
        source_compile_only=frozenset(source_compile_only),
        fail_fast=frozenset(fail_fast_tokens),
        header_path=header_path,
    )
    return surface, header_path, None


@dataclass(frozen=True)
class _ExtensionScanSurface:
    runtime_backed: frozenset[str]
    source_compile_only: frozenset[str]
    fail_fast: frozenset[str]
    header_path: Path

    @property
    def accepted(self) -> frozenset[str]:
        return self.runtime_backed | self.source_compile_only

    @property
    def known(self) -> frozenset[str]:
        return self.accepted | self.fail_fast

    def status_for(self, symbol: str) -> str:
        if symbol in self.fail_fast:
            return "fail_fast"
        if symbol in self.runtime_backed:
            return "runtime_backed"
        if symbol in self.source_compile_only:
            return "source_compile_only"
        return "missing"


def _extract_numpy_fail_fast_symbols(text: str) -> set[str]:
    return {
        match.group("symbol") for match in _NUMPY_FAIL_FAST_SYMBOL_RE.finditer(text)
    }
