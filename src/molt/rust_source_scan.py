"""Offset-preserving Rust lexical projections for source-analysis tools.

This leaf has no compiler, CLI, generator, or release-tool dependencies. Its lexer
recognizes comments and literals; conservative item/module projections share
that lexical boundary. For lexical masking, malformed unterminated
comments/strings extend to EOF so their contents cannot become apparent code.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable, Iterator
from contextlib import contextmanager
from pathlib import Path
import re
from threading import local
from typing import Literal, NamedTuple, TypeVar, cast

_T = TypeVar("_T")

# A whole-repository scan queries the same file text from many probes. The
# projections below are pure functions of the text, so inside scan_memo() they
# are memoized by content: one lexical pass per file per scan, released when
# the scan ends. A mutated file is a different key, never a stale hit.
_SCAN_STATE = local()


@contextmanager
def scan_memo() -> Iterator[None]:
    """Share content projections in a synchronous, thread-owned scan.

    Nested callers in this thread reuse the outer memo. Independent threads
    never share its mutable dictionary. Do not suspend an async task inside
    this synchronous scope; the outermost exit drops every retained buffer.
    """
    if getattr(_SCAN_STATE, "memo", None) is not None:
        yield
        return
    _SCAN_STATE.memo = {}
    try:
        yield
    finally:
        del _SCAN_STATE.memo


def _memoized(kind: str, option: object, text: str, compute: Callable[[], _T]) -> _T:
    memo = getattr(_SCAN_STATE, "memo", None)
    if memo is None:
        return compute()
    key = (kind, option, text)
    if key not in memo:
        memo[key] = compute()
    return cast(_T, memo[key])


# Python's Unicode \w is exactly the isalnum-or-underscore token boundary
# used here. Search only potential non-code starts, not every ordinary code
# character; quote alternatives still admit a literal after an invalid prefix.
_NON_CODE_START = re.compile(
    r"""//|/\*|(?<!\w)(?:br|cr|r)(?P<hashes>\#*)"|(?<!\w)(?:[bc]"|b')|["']"""
)
_COMMENT_DELIMITER = re.compile(r"/\*|\*/")
_STRING_DELIMITER = re.compile(r'["\\]')
_CHAR_LITERAL = re.compile(
    r"""'(?:[^'\\\r\n]|\\(?:[nrt\\0'"]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}))'"""
)
_NON_NEWLINE = re.compile(r"[^\r\n]+")


class _NonCodeSpan(NamedTuple):
    kind: Literal["comment", "literal"]
    start: int
    end: int


def _non_code_spans(text: str) -> Iterator[_NonCodeSpan]:
    index = 0
    length = len(text)
    while match := _NON_CODE_START.search(text, index):
        start = match.start()
        token = match.group()
        if token == "//":
            end = text.find("\n", match.end())
            index = length if end < 0 else end
            yield _NonCodeSpan("comment", start, index)
            continue
        if token == "/*":
            depth = 1
            index = match.end()
            while depth:
                delimiter = _COMMENT_DELIMITER.search(text, index)
                if delimiter is None:
                    index = length
                    break
                depth += 1 if delimiter.group() == "/*" else -1
                index = delimiter.end()
            yield _NonCodeSpan("comment", start, index)
            continue
        hashes = match.group("hashes")
        if hashes is not None:
            terminator = '"' + hashes
            end = text.find(terminator, match.end())
            index = length if end < 0 else end + len(terminator)
            yield _NonCodeSpan("literal", start, index)
            continue
        if token.endswith('"'):
            index = match.end()
            while True:
                delimiter = _STRING_DELIMITER.search(text, index)
                if delimiter is None:
                    index = length
                    break
                if delimiter.group() == '"':
                    index = delimiter.end()
                    break
                # Backslash quotes exactly the following character, including
                # a newline. Advancing past it also handles escaped backslashes.
                index = min(length, delimiter.end() + 1)
            yield _NonCodeSpan("literal", start, index)
            continue
        char = _CHAR_LITERAL.match(text, match.end() - 1)
        if char is not None:
            index = char.end()
            yield _NonCodeSpan("literal", start, index)
        else:
            # The apostrophe belongs to a lifetime or malformed char, not a
            # literal. Prefix b was already proved not to begin a valid char.
            index = match.end()


class RustSourceProjection(NamedTuple):
    masked_code: str
    comments: list[tuple[int, str]]


def _project_rust_source(
    text: str,
    *,
    include_mask: bool,
    include_comments: bool,
    preserve_literals: bool = False,
    spans: Iterable[_NonCodeSpan] | None = None,
) -> RustSourceProjection:
    output: list[str] = []
    comments: list[tuple[int, str]] = []
    cursor = 0
    line = 1
    for span in _non_code_spans(text) if spans is None else spans:
        if include_mask:
            output.append(text[cursor : span.start])
            original = text[span.start : span.end]
            output.append(
                original
                if preserve_literals and span.kind == "literal"
                else _NON_NEWLINE.sub(lambda run: " " * len(run[0]), original)
            )
        if include_comments:
            line += text.count("\n", cursor, span.start)
            if span.kind == "comment":
                comments.append((line, text[span.start : span.end]))
            line += text.count("\n", span.start, span.end)
        cursor = span.end
    if include_mask:
        output.append(text[cursor:])
    return RustSourceProjection("".join(output) if include_mask else text, comments)


def project_rust_source(text: str) -> RustSourceProjection:
    """Produce code and line-numbered comments in one offset-preserving scan."""
    return _project_rust_source(text, include_mask=True, include_comments=True)


def mask_rust_comments_and_strings(
    text: str, *, preserve_literals: bool = False
) -> str:
    """Blank comments and, by default, literals; preserve offsets and CR/LF.

    Pattern scanners may retain literals while using the same lexical boundaries.
    """
    return _memoized(
        "mask",
        preserve_literals,
        text,
        lambda: (
            _project_rust_source(
                text,
                include_mask=True,
                include_comments=False,
                preserve_literals=preserve_literals,
            ).masked_code
        ),
    )


class RustSourceToken(NamedTuple):
    text: str
    start: int
    end: int


def _rust_token_index(text: str) -> tuple[tuple[RustSourceToken, ...], tuple[str, ...]]:
    def compute() -> tuple[tuple[RustSourceToken, ...], tuple[str, ...]]:
        tokens = tuple(_rust_source_tokens(text, _non_code_spans(text)))
        return tokens, tuple(token.text for token in tokens)

    return _memoized("tokens", None, text, compute)


def rust_source_tokens(text: str) -> list[RustSourceToken]:
    """Code tokens and verbatim literal atoms, omitting only real trivia.

    Raw/byte/C strings and character literals use the very same spans as the
    masker. Their interior whitespace, delimiters and quotes never become code
    tokens. No literal is decoded, normalized or rewritten by this projection.
    """
    return list(_rust_token_index(text)[0])


def _rust_source_tokens(
    text: str, spans: Iterable[_NonCodeSpan]
) -> list[RustSourceToken]:
    tokens = []
    cursor = 0

    def code_tokens(start: int, end: int) -> None:
        tokens.extend(
            RustSourceToken(match[0], start + match.start(), start + match.end())
            for match in re.finditer(r"\w+|[^\s]", text[start:end])
        )

    for span in spans:
        code_tokens(cursor, span.start)
        if span.kind == "literal":
            tokens.append(
                RustSourceToken(text[span.start : span.end], span.start, span.end)
            )
        cursor = span.end
    code_tokens(cursor, len(text))
    return tokens


def rust_token_range(
    text: str, fragment: str, *, depth: int | None = None
) -> tuple[int, int] | None:
    """Locate one exact token sequence within its requested brace depth.

    Scope filters candidates before the uniqueness check. A repeated nested
    header cannot make its distinct outer sibling ambiguous, and a nested
    candidate can never stand in for a missing dominating outer statement.
    """
    tokens, texts = _rust_token_index(text)
    wanted = _rust_token_index(fragment)[1]
    if not wanted:
        return None
    width = len(wanted)
    first = wanted[0]
    matches = [
        index
        for index in range(len(texts) - width + 1)
        if texts[index] == first and texts[index : index + width] == wanted
    ]
    if depth is not None:
        code = mask_rust_comments_and_strings(text)
        matches = [
            index
            for index in matches
            if code[: tokens[index].start].count("{")
            - code[: tokens[index].start].count("}")
            == depth
        ]
    if len(matches) != 1:
        return None
    index = matches[0]
    return tokens[index].start, tokens[index + len(wanted) - 1].end


def _rust_delimiter_end_masked(code: str, opening: int) -> int | None:
    if not 0 <= opening < len(code) or code[opening] not in "([{":
        return None
    stack = []
    for index in range(opening, len(code)):
        char = code[index]
        if char in "([{":
            stack.append(char)
        elif char in ")]}":
            if not stack or {")": "(", "]": "[", "}": "{"}[char] != stack.pop():
                return None
            if not stack:
                return index + 1
    return None


def rust_delimiter_end(text: str, opening: int) -> int | None:
    """End offset of one balanced delimiter region in actual Rust code."""
    return _rust_delimiter_end_masked(mask_rust_comments_and_strings(text), opening)


def rust_block_region(
    text: str, header: str, depth: int | None = None
) -> tuple[int, int] | None:
    found = rust_token_range(text, header + " {", depth=depth)
    if found is None:
        return None
    _, opening_end = found
    code = mask_rust_comments_and_strings(text)
    end = _rust_delimiter_end_masked(code, opening_end - 1)
    return (opening_end, end - 1) if end is not None else None


class RustMatchArm(NamedTuple):
    start: int
    end: int
    pattern: str
    body: str
    body_start: int


def rust_match_arms(text: str, expression: str) -> list[RustMatchArm] | None:
    """Closed match-arm projection with separate structure and trivia views.

    Literal bytes are masked for punctuation, but preserved when skipping
    trivia. Treating the punctuation mask's blanks as trivia would erase quoted
    patterns and literal RHS expressions. Unsupported/ambiguous structure fails
    closed; this is not a general Rust expression parser.
    """
    region = rust_block_region(text, "match " + expression)
    if region is None:
        return None
    begin, end = region
    code = mask_rust_comments_and_strings(text)
    trivia = mask_rust_comments_and_strings(text, preserve_literals=True)
    cursor = begin
    result = []
    while cursor < end:
        while cursor < end and (trivia[cursor].isspace() or code[cursor] == ","):
            cursor += 1
        if cursor == end:
            break
        start = cursor
        while cursor < end and not code.startswith("=>", cursor):
            if code[cursor] in "([{":
                close = _rust_delimiter_end_masked(code, cursor)
                if close is None or close > end:
                    return None
                cursor = close
            else:
                cursor += 1
        if cursor >= end:
            return None
        pattern = text[start:cursor]
        if not rust_source_tokens(pattern):
            return None
        cursor += 2
        while cursor < end and trivia[cursor].isspace():
            cursor += 1
        if cursor >= end:
            return None
        body_start = cursor
        if code[cursor] == "{":
            close = _rust_delimiter_end_masked(code, cursor)
            if close is None or close > end:
                return None
            cursor = close
        else:
            while cursor < end and code[cursor] != ",":
                if code.startswith("=>", cursor):
                    # An omitted comma after an unsupported expression with a
                    # block must not absorb the following arm into this domain.
                    return None
                if code[cursor] in "([{":
                    close = _rust_delimiter_end_masked(code, cursor)
                    if close is None or close > end:
                        return None
                    cursor = close
                else:
                    cursor += 1
        result.append(
            RustMatchArm(start, cursor, pattern, text[body_start:cursor], body_start)
        )
    return result


def rust_literal_pattern_names(pattern: str) -> frozenset[str] | None:
    """An unguarded union of ordinary, verbatim identifier string literals."""
    tokens = [token.text for token in rust_source_tokens(pattern)]
    if (
        not tokens
        or len(tokens) % 2 == 0
        or any(token != "|" for token in tokens[1::2])
    ):
        return None
    names = tokens[::2]
    if not all(re.fullmatch(r'"[A-Za-z_][A-Za-z0-9_]*"', name) for name in names):
        return None
    return frozenset(name[1:-1] for name in names)


def rust_comment_segments(text: str) -> list[tuple[int, str]]:
    """Return one-based source lines and original comments from the same scan."""
    return _project_rust_source(
        text, include_mask=False, include_comments=True
    ).comments


# Test scope and file-module ownership share the lexer with all source probes.
# The projections below retain byte/character offsets and physical LF lines.
_RUST_TEST_CFG_RE = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
_RUST_MODULE_OR_SCOPE_RE = re.compile(
    r"(?P<attribute>#\s*!?\s*\[)|"
    r"\b(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?mod\s+"
    r"(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)"
    r"\s*(?P<body>[;{])|(?P<scope>[{}])"
)
_RUST_PATH_ATTR_RE = re.compile(r'#\s*\[\s*path\s*=\s*"(?P<path>[^"]+)"\s*\]')


def _rust_delimiter_ends(code: str) -> dict[int, int]:
    ends: dict[int, int] = {}
    stack: list[tuple[str, int]] = []
    opening = {")": "(", "]": "[", "}": "{"}
    for match in re.finditer(r"[()\[\]{}]", code):
        token = match.group()
        if token in "([{":
            stack.append((token, match.start()))
        elif stack and stack[-1][0] == opening[token]:
            _, start = stack.pop()
            ends[start] = match.end()
        else:
            raise ValueError("unmatched Rust delimiter")
    if stack:
        raise ValueError("unclosed Rust delimiter")
    return ends


class _RustItemPrefix(NamedTuple):
    start: int
    end: int
    test_only: bool
    path: str | None


def _rust_item_prefixes(
    tokens: list[RustSourceToken], trivia: str, code: str, ends: dict[int, int]
) -> dict[int, _RustItemPrefix]:
    """Attach complete outer-attribute chains to their next code token.

    Leading trivia belongs to the prefix, but the previous token (including a
    literal or enclosing delimiter) never does. Balanced attribute bodies are
    opaque: nested punctuation and quoted attribute-like text cannot become
    another item prefix. Inner attributes belong to their enclosing scope.
    """
    prefixes: dict[int, _RustItemPrefix] = {}
    index = 0
    while index + 1 < len(tokens):
        if tokens[index].text != "#":
            index += 1
            continue
        inner = tokens[index + 1].text == "!"
        opening = index + (2 if inner else 1)
        if opening >= len(tokens) or tokens[opening].text != "[":
            index += 1
            continue
        start = tokens[index - 1].end if index else 0
        test_only = False
        path = None
        while True:
            attr_start = tokens[index].start
            attr_end = ends[tokens[opening].start]
            test_only |= (
                _RUST_TEST_CFG_RE.fullmatch(code, attr_start, attr_end) is not None
            )
            if path_attr := _RUST_PATH_ATTR_RE.fullmatch(trivia, attr_start, attr_end):
                path = path_attr.group("path")
            while index < len(tokens) and tokens[index].start < attr_end:
                index += 1
            if (
                inner
                or index + 1 >= len(tokens)
                or tokens[index].text != "#"
                or tokens[index + 1].text != "["
            ):
                break
            opening = index + 1
        if not inner and index < len(tokens):
            prefixes[tokens[index].start] = _RustItemPrefix(
                start, attr_end, test_only, path
            )
    return prefixes


class _RustItemProjection(NamedTuple):
    code: str
    ends: dict[int, int]
    prefixes: dict[int, _RustItemPrefix]


def _rust_item_projection(text: str) -> _RustItemProjection:
    """One operation-local lexical receipt for both item consumers."""
    spans = tuple(_non_code_spans(text))
    code = (
        _project_rust_source(
            text, include_mask=True, include_comments=False, spans=spans
        )
        .masked_code.replace("\u200e", " ")
        .replace("\u200f", " ")
    )
    trivia = _project_rust_source(
        text,
        include_mask=True,
        include_comments=False,
        preserve_literals=True,
        spans=spans,
    ).masked_code
    tokens = _rust_source_tokens(
        text.replace("\u200e", " ").replace("\u200f", " "), spans
    )
    ends = _rust_delimiter_ends(code)
    return _RustItemProjection(
        code, ends, _rust_item_prefixes(tokens, trivia, code, ends)
    )


def rust_test_item_spans(text: str) -> list[tuple[int, int]]:
    """Return actual cfg(test) item spans, including same-line declarations.

    A test attribute owns its complete outer-attribute prefix and next item.
    Prefix trivia is included; previous tokens, enclosing delimiters and later
    production siblings cannot extend its scope.
    """
    return _rust_test_item_spans(_rust_item_projection(text))


def _rust_test_item_spans(projection: _RustItemProjection) -> list[tuple[int, int]]:
    code, ends, prefixes = projection
    spans: list[tuple[int, int]] = []
    for prefix in prefixes.values():
        if not prefix.test_only or (spans and prefix.start < spans[-1][1]):
            continue
        cursor = prefix.end
        body_declaration = (
            re.match(
                r"\s*(?:pub(?:\([^)]*\))?\s+)?"
                r"(?:(?:async|unsafe|extern|const)\s+)*"
                r"(?:fn|impl|trait|struct|enum|mod|union)\b",
                code[cursor:],
            )
            is not None
        )
        angles = 0
        while token := re.search(r"[\[({;,<>}\])]", code[cursor:]):
            start = cursor + token.start()
            char = code[start]
            if char == "<":
                angles += 1
                cursor = start + 1
            elif char == ">":
                angles = max(angles - 1, 0)
                cursor = start + 1
            elif char == ";" or (char == "," and not angles and not body_declaration):
                cursor = start + 1
                break
            elif char in "})]":
                # The enclosing container belongs to production; an annotated
                # field must never mask its closing delimiter or later items.
                cursor = start
                break
            elif char in "([{":
                cursor = ends[start]
                if char == "{" and not angles:
                    break
            else:
                cursor = start + 1
        else:
            cursor = len(code)
        spans.append((prefix.start, cursor))
    return spans


def mask_rust_test_items(text: str) -> str:
    """Blank test items while preserving offsets, literals and production."""
    return _memoized(
        "tests",
        None,
        text,
        lambda: _mask_rust_test_items(text, _rust_item_projection(text)),
    )


def _mask_rust_test_items(text: str, projection: _RustItemProjection) -> str:
    chunks: list[str] = []
    cursor = 0
    for start, end in _rust_test_item_spans(projection):
        chunks.extend(
            (
                text[cursor:start],
                _NON_NEWLINE.sub(lambda m: " " * len(m[0]), text[start:end]),
            )
        )
        cursor = end
    chunks.append(text[cursor:])
    return "".join(chunks)


def _rust_module_file(
    source: Path,
    module_dir: Path,
    explicit_dir: Path,
    explicit_path: str | None,
    name: str,
    *,
    strict: bool,
) -> Path | None:
    candidates = (
        (explicit_dir / explicit_path,)
        if explicit_path is not None
        else (
            module_dir / f"{name.removeprefix('r#')}.rs",
            module_dir / name.removeprefix("r#") / "mod.rs",
        )
    )
    matches = tuple(
        candidate.resolve() for candidate in candidates if candidate.is_file()
    )
    if len(matches) == 1:
        return matches[0]
    if len(matches) > 1:
        raise RuntimeError(
            f"declared Rust module {name!r} has ambiguous sources: {matches}"
        )
    if strict:
        raise FileNotFoundError(
            f"declared Rust module {name!r} has no source from {source}: {candidates}"
        )
    return None


def rust_file_module_declarations(
    source: Path,
    text: str,
    *,
    strict: bool = True,
    include_tests: bool = True,
) -> list[tuple[Path, bool]]:
    """Resolve file-module edges and their lexical test scope.

    Non-module bodies are skipped. Unknown feature cfgs retain their production
    possibility. A missing optional/platform module may be omitted by audits;
    authority readers use strict resolution and fail closed.
    """
    return _rust_file_module_declarations(
        source,
        text,
        _rust_item_projection(text),
        strict=strict,
        include_tests=include_tests,
    )


def _rust_file_module_declarations(
    source: Path,
    text: str,
    projection: _RustItemProjection,
    *,
    strict: bool,
    include_tests: bool,
) -> list[tuple[Path, bool]]:
    code, ends, prefixes = projection
    declarations: list[tuple[Path, bool]] = []

    def modules(
        start: int, end: int, module_dir: Path, explicit_dir: Path, inherited_test: bool
    ) -> None:
        cursor = start
        while declaration := _RUST_MODULE_OR_SCOPE_RE.search(code, cursor, end):
            cursor = declaration.end()
            if declaration.group("attribute"):
                cursor = ends[declaration.end() - 1]
                continue
            if declaration.group("scope"):
                if declaration.group("scope") == "{":
                    cursor = ends[declaration.start()]
                continue
            name = declaration.group("name")
            prefix = prefixes.get(declaration.start())
            test_only = inherited_test or (prefix is not None and prefix.test_only)
            explicit_path = prefix.path if prefix is not None else None
            inline = declaration.group("body") == "{"
            if inline:
                cursor = ends[declaration.end() - 1]
            if test_only and not include_tests:
                continue
            if inline:
                child_dir = (
                    explicit_dir / explicit_path
                    if explicit_path is not None
                    else module_dir / name.removeprefix("r#")
                )
                modules(declaration.end(), cursor - 1, child_dir, child_dir, test_only)
            else:
                child = _rust_module_file(
                    source, module_dir, explicit_dir, explicit_path, name, strict=strict
                )
                if child is not None:
                    declarations.append((child, test_only))

    module_dir = (
        source.parent
        if source.name in {"lib.rs", "main.rs", "mod.rs"}
        else source.with_suffix("")
    )
    modules(0, len(text), module_dir, source.parent, False)
    return declarations


def read_rust_module_cluster(root_file: Path) -> str:
    """Read the exact production module graph; unresolved ownership fails closed."""
    sources: dict[Path, str] = {}

    def visit(source: Path) -> None:
        source = source.resolve(strict=True)
        if source in sources:
            return
        text = source.read_text(encoding="utf-8")
        projection = _rust_item_projection(text)
        sources[source] = _mask_rust_test_items(text, projection)
        for child, _ in _rust_file_module_declarations(
            source, text, projection, strict=True, include_tests=False
        ):
            visit(child)

    visit(root_file)
    root = root_file.resolve(strict=True)
    ordered = sorted(sources.keys() - {root}, key=lambda path: path.as_posix())
    return "\n".join(sources[path] for path in [*ordered, root])


def rust_test_only_source_files(paths: Iterable[Path]) -> set[Path]:
    """Classify declared test graphs; any production ownership wins.

    Unreferenced sources remain production candidates. Cargo integration-test
    roots are recognized by an adjacent package manifest, never by a substring
    or message allowlist. A shared path can be visited under both scope modes.
    """
    files = {path.resolve() for path in paths}
    edges = {}
    incoming: set[Path] = set()
    for path in files:
        text = path.read_text(encoding="utf-8", errors="replace")
        edges[path] = rust_file_module_declarations(path, text, strict=False)
        incoming.update(child for child, _ in edges[path])
    cargo_tests = {
        path
        for path in files
        if any(
            parent.name == "tests" and (parent.parent / "Cargo.toml").is_file()
            for parent in path.parents
        )
    }
    visited: set[tuple[Path, bool]] = set()

    def visit(path: Path, test: bool) -> None:
        state = (path, test)
        if state in visited:
            return
        visited.add(state)
        for child, child_test in edges.get(path, ()):
            visit(child, test or child_test)

    for path in (files - incoming) | cargo_tests:
        visit(path, path in cargo_tests)
    # Unrooted cycles cannot prove test-only ownership and remain production.
    return {path for path, test in visited if test and (path, False) not in visited}
