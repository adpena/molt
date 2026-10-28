from __future__ import annotations

from pathlib import Path
from molt.checkout_state import scratch_state_root
from collections.abc import Iterable
from dataclasses import dataclass, field, replace
from fnmatch import fnmatchcase
import glob
import json
import os
import re
import shlex
import tomllib
from typing import NamedTuple
import warnings

import pytest
import yaml

from molt import tool_releases

from tools.proof_counts import fail_closed_proof_exit_code


REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_ROOT = REPO_ROOT / ".github" / "workflows"


def _read(path: str) -> str:
    return (REPO_ROOT / path).read_text(encoding="utf-8")


def _literal_job_needs(job: dict[str, object]) -> tuple[str, ...]:
    raw = job.get("needs")
    if raw is None:
        return ()
    if isinstance(raw, str):
        return (raw,)
    assert isinstance(raw, list)
    assert all(isinstance(item, str) for item in raw)
    return tuple(raw)


def _named_step_blocks(workflow_text: str) -> list[str]:
    blocks: list[list[str]] = []
    current: list[str] = []
    for line in workflow_text.splitlines():
        if line.startswith("      - name: "):
            if current:
                blocks.append(current)
            current = [line]
        elif current:
            current.append(line)
    if current:
        blocks.append(current)
    return ["\n".join(block) for block in blocks]


def _default_python_version() -> str:
    """The exact tooling CPython; generator oracles differ between patches."""
    version = _read(".python-version").strip()
    components = version.split(".")
    assert len(components) == 3
    assert all(component.isdigit() for component in components)
    return version


# A step's `run` script is read as bash reads it: a lexer for words, quoting,
# comments, substitutions, parameter expansions, heredocs and operators; a
# parser for lists, pipelines, compound commands and function definitions; and
# a reading that follows the environment, the working directory and the
# project sync through them. Two rules keep the reading on the safe side where
# it does not model bash (the HF-456 row lists those constructs):
#
# 1. Unknown state is conservative. A construct this reading does not model
#    that can change the environment or the working directory marks the shell
#    unknown, and a later molt run there counts as provided only through `uv
#    run` with the project. A value the reading cannot know (`read NAME`, a
#    branch, loop or `&&` that may or may not set it) is unknown, and an
#    unknown value names no `src` and may be true.
# 2. A backstop. Each `-m molt` text of a command or script, and each
#    substitution or heredoc body whose text runs molt, that no reading
#    accounted for is reported as unprovided, so no reading gap passes.
_WORKSPACE_EXPRESSION = re.compile(r"\$\{\{\s*github\.workspace\s*\}\}")
_GITHUB_EXPRESSION = re.compile(r"\$\{\{.*?\}\}")
# `-m molt`, `-mmolt`, `-Bm molt` and `--module molt`, quoted or not.
_MOLT_MODULE_TEXT = re.compile(
    r"(?<![\w-])-(?:[A-Za-z]*m\s*|-module[\s=]\s*)['\"]?molt\b"
)
_SUBSTITUTION = "MOLT-SUBSTITUTION"
_ARITHMETIC = "MOLT-ARITHMETIC"
_ANSI_C_STRING = "MOLT-ANSI-C-STRING"
# A value this reading cannot know; a value with an unknown part is unknown.
_UNKNOWN = "MOLT-UNKNOWN"
_OPERATORS = (
    "&>>", ";;&", "<<<", "<<-", "&&", "||", ";;", ";&", "|&", ">>", "<<",
    ">&", "<&", "<>", ">|", "&>", ";", "&", "|", "(", ")", "<", ">",
)  # fmt: skip
_REDIRECTIONS = frozenset(
    {"<", ">", ">>", "<<", "<<-", "<<<", "<&", ">&", "<>", ">|", "&>", "&>>"}
)
_COMMAND_SEPARATORS = frozenset({"\n", ";", ";;", "&", "&&", "|", "||", "|&"})
# Reserved words after which a command starts.
_COMMAND_PREFIX_WORDS = frozenset(
    {"if", "then", "elif", "else", "do", "while", "until", "!", "{", "time"}
)
# Reserved words that close a compound command.
_CLOSING_WORDS = frozenset({"then", "elif", "else", "fi", "do", "done", "esac", "}"})
_ASSIGNMENT = re.compile(
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)(?P<append>\+?)=(?P<value>.*)", re.S
)
_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_ARRAY_ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\+?=")
_VARIABLE_REFERENCE = re.compile(r"\$\{?(?P<name>[A-Za-z_][A-Za-z0-9_]*)\}?")
_ASSIGNING_EXPANSION = re.compile(r"\$\{(?P<name>[A-Za-z_][A-Za-z0-9_]*):?=")
# A name an arithmetic expression assigns: `x = 1`, `x += 1`, `x++`, `--x`.
_ARITHMETIC_ASSIGNMENT = re.compile(
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?:\+\+|--|(?:[-+*/%&|^]|<<|>>)?=(?!=))"
    r"|(?:\+\+|--)\s*(?P<prefixed>[A-Za-z_][A-Za-z0-9_]*)"
)
_PYTHON_EXECUTABLE = re.compile(r"python[0-9.]*")
_SHELLS = frozenset({"sh", "bash", "dash", "zsh"})
# CPython's command line: these options take an argument, attached or next.
_PYTHON_OPTIONS_WITH_ARGUMENT = frozenset("WX")
# -I implies -E; both make the interpreter ignore PYTHONPATH.
_PYTHON_IGNORES_PYTHONPATH = frozenset("IE")
# Commands that run the rest of their words as a command: how many of their
# own operands precede it, and which of their options take a value.
_COMMAND_WRAPPERS = {
    "time": (0, frozenset({"-f", "-o"})),
    "nice": (0, frozenset({"-n"})),
    "nohup": (0, frozenset()),
    "timeout": (1, frozenset({"-s", "-k", "--signal", "--kill-after"})),
    "exec": (0, frozenset({"-a"})),
}
_DECLARATIONS = frozenset(
    {"export", "unset", "declare", "typeset", "local", "readonly"}
)
# Builtins that assign the variables they name: `read`, `mapfile` and
# `readarray` their operands, `printf -v`, `wait -p`, `getopts` and `let`.
_ASSIGNING_BUILTINS = frozenset(
    {"read", "mapfile", "readarray", "printf", "wait", "getopts", "let"}
)
# Builtins and keywords that change no variable and not the directory.
_INERT_COMMANDS = frozenset(
    {
        ":", "[", "[[", _ARITHMETIC, "bg", "bind", "caller", "compgen",
        "complete", "compopt", "dirs", "disown", "echo", "exit", "false", "fg",
        "hash", "help", "history", "jobs", "kill", "logout", "pwd", "return",
        "shift", "suspend", "test", "times", "true", "type", "ulimit", "umask",
        "unalias",
    }
)  # fmt: skip
# Bash builtins this reading neither models nor knows to be inert.
_UNMODELED_BUILTINS = frozenset({"enable", "fc"})
# Spellings of the repository's `src` in a PYTHONPATH entry: relative ones
# hold only while the script has not changed directory.
_WORKSPACE_SRC = frozenset({"$GITHUB_WORKSPACE/src", "${GITHUB_WORKSPACE}/src"})
_RELATIVE_SRC = frozenset({"src", "./src", "$PWD/src", "${PWD}/src"})
_WORKSPACE = frozenset({"$GITHUB_WORKSPACE", "${GITHUB_WORKSPACE}"})
_TRUE = frozenset({"1", "true", "yes", "on"})
# `uv run` options that take a value (`uv run --help`, uv 0.13.5).
_UV_RUN_VALUE_OPTIONS = frozenset(
    {
        "--extra", "--no-extra", "--group", "--no-group", "--only-group",
        "--no-editable-package", "--env-file", "-w", "--with", "--with-editable",
        "--with-requirements", "--package", "--python-platform", "--index",
        "--default-index", "-i", "--index-url", "--extra-index-url", "-f",
        "--find-links", "--index-strategy", "--keyring-provider", "-P",
        "--upgrade-package", "--upgrade-group", "--resolution", "--prerelease",
        "--prerelease-package", "--fork-strategy", "--exclude-newer",
        "--exclude-newer-package", "--no-sources-package", "--reinstall-package",
        "--link-mode", "-C", "--config-setting", "--config-settings-package",
        "--no-build-isolation-package", "--no-build-package",
        "--no-binary-package", "--cache-dir", "--refresh-package", "-p",
        "--python", "--color", "--allow-insecure-host", "--directory",
        "--project", "--config-file",
    }
)  # fmt: skip
# `uv run` options that leave the project out of the environment.
_UV_RUN_WITHOUT_PROJECT = frozenset({"--no-project", "--only-dev", "--only-group"})
# `uv run` options and variables that choose where uv finds the project.
_UV_RUN_LOCATIONS = {"--directory": "UV_WORKING_DIR", "--project": "UV_PROJECT"}


@dataclass(frozen=True)
class _Token:
    """A word or an operator of a script. ``text`` is the word after quote
    removal, with each command, arithmetic or process substitution, ANSI-C
    string and array value a placeholder and each parameter expansion as
    written. ``runs_molt`` counts the substitutions, strings and heredoc
    bodies in it whose text runs molt; ``assigns`` names the variables its
    expansions assign (`${X:=...}`, arithmetic)."""

    text: str
    operator: bool = False
    quoted: bool = False
    runs_molt: int = 0
    assigns: tuple[str, ...] = ()


@dataclass(frozen=True)
class _Expansion:
    """An expansion or quoted string: where it ends and what it reads as."""

    end: int
    text: str
    runs_molt: int = 0
    assigns: tuple[str, ...] = ()


def _molt_occurrences(tokens: list[_Token]) -> int:
    """How many molt runs the backstop counts in ``tokens``."""
    text = " ".join(token.text for token in tokens)
    return len(_MOLT_MODULE_TEXT.findall(text)) + sum(t.runs_molt for t in tokens)


def _runs_molt(text: str) -> int:
    return int(_MOLT_MODULE_TEXT.search(text) is not None)


def _arithmetic_assignments(text: str) -> tuple[str, ...]:
    return tuple(
        match["name"] or match["prefixed"]
        for match in _ARITHMETIC_ASSIGNMENT.finditer(text)
    )


def _quote_end(text: str, index: int) -> int:
    """The index after the `'` closing the one at ``index``."""
    end = text.find("'", index + 1)
    return len(text) if end < 0 else end + 1


def _escaped_end(text: str, index: int, closer: str) -> int:
    """The index after the unescaped ``closer`` that ends the string whose
    body starts at ``index`` (a `$'...'` string or a backquote)."""
    while index < len(text):
        if text[index] == "\\":
            index += 2
        elif text[index] == closer:
            return index + 1
        else:
            index += 1
    return len(text)


def _double_quoted(text: str, index: int) -> _Expansion:
    """The double-quoted string at ``index``, read as bash expands it."""
    parts: list[str] = []
    runs_molt = 0
    assigns: list[str] = []
    position = index + 1
    while position < len(text):
        char = text[position]
        if char == '"':
            return _Expansion(position + 1, "".join(parts), runs_molt, tuple(assigns))
        if char == "\\":
            following = text[position + 1 : position + 2]
            if following == "\n":
                position += 2
            elif following and following in '$`"\\':
                parts.append(following)
                position += 2
            else:
                parts.append(char)
                position += 1
            continue
        expansion = _expansion(text, position)
        if expansion is None:
            parts.append(char)
            position += 1
            continue
        parts.append(expansion.text)
        runs_molt += expansion.runs_molt
        assigns.extend(expansion.assigns)
        position = expansion.end
    return _Expansion(len(text), "".join(parts), runs_molt, tuple(assigns))


def _parameter_end(text: str, index: int) -> int:
    """The index after the `}` closing the `${` at ``index``."""
    position = index + 2
    while position < len(text):
        char = text[position]
        if char == "}":
            return position + 1
        if char == "\\":
            position += 2
        elif char == "'":
            position = _quote_end(text, position)
        elif char == '"':
            position = _double_quoted(text, position).end
        elif (expansion := _expansion(text, position)) is not None:
            position = expansion.end
        else:
            position += 1
    return len(text)


def _arithmetic_end(text: str, index: int) -> tuple[int, bool]:
    """The index after the `)` matching the `(` at ``index``, and whether the
    `((` there closes as `))`: an arithmetic expression, as bash tries first."""
    depth, position, inner = 0, index, -1
    while position < len(text):
        char = text[position]
        if char == "\\":
            position += 2
            continue
        if char == "'":
            position = _quote_end(text, position)
            continue
        if char == '"':
            position = _double_quoted(text, position).end
            continue
        if (expansion := _expansion(text, position)) is not None:
            position = expansion.end
            continue
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 1:
                inner = position
            elif depth == 0:
                return position + 1, inner == position - 1
        position += 1
    return len(text), False


def _paired_end(text: str, index: int) -> int:
    """The index after the `)` matching the `(` at ``index`` (an array value)."""
    depth, position = 0, index
    while position < len(text):
        char = text[position]
        if char == "\\":
            position += 2
            continue
        if char == "'":
            position = _quote_end(text, position)
            continue
        if char == '"':
            position = _double_quoted(text, position).end
            continue
        if (expansion := _expansion(text, position)) is not None:
            position = expansion.end
            continue
        depth += {"(": 1, ")": -1}.get(char, 0)
        position += 1
        if depth == 0:
            return position
    return len(text)


def _expansion(text: str, index: int) -> _Expansion | None:
    """The command, arithmetic or parameter expansion starting at ``index``."""
    if text.startswith("$((", index):
        end, arithmetic = _arithmetic_end(text, index + 1)
        if arithmetic:
            body = text[index + 3 : end - 2]
            return _Expansion(
                end, _ARITHMETIC, _runs_molt(body), _arithmetic_assignments(body)
            )
    if text.startswith("$(", index):
        lexer = _Lexer(text, index + 2, closing=True)
        runs_molt = int(_molt_occurrences(lexer.run()) > 0)
        return _Expansion(lexer.index, _SUBSTITUTION, runs_molt)
    if text.startswith("`", index):
        end = _escaped_end(text, index + 1, "`")
        body = re.sub(r"\\([\\`$])", r"\1", text[index + 1 : end - 1])
        runs_molt = int(_molt_occurrences(_Lexer(body).run()) > 0)
        return _Expansion(end, _SUBSTITUTION, runs_molt)
    if text.startswith("${", index):
        end = _parameter_end(text, index)
        assigned = _ASSIGNING_EXPANSION.match(text, index)
        return _Expansion(
            end, text[index:end], 0, (assigned["name"],) if assigned else ()
        )
    return None


class _Lexer:
    """Splits a script into tokens as bash's lexer does. With ``closing`` it
    reads the body of a `$(...)` and stops after the `)` that closes it, which
    a `case` pattern's `)`, a quoted one or one in a comment does not."""

    def __init__(self, text: str, index: int = 0, *, closing: bool = False) -> None:
        self.text = text
        self.index = index
        self.closing = closing
        self.tokens: list[_Token] = []
        # Heredocs whose bodies start after the next newline: their
        # delimiter, whether `<<-` strips tabs, and the delimiter's token.
        self.heredocs: list[tuple[str, bool, int]] = []
        self.command_start = True
        self.depth = 0
        # For each open `case`: "subject", "pattern" or "body".
        self.cases: list[str] = []
        self.conditional = False  # inside `[[ ... ]]`

    def run(self) -> list[_Token]:
        text = self.text
        while self.index < len(text):
            char = text[self.index]
            if char in " \t\r":
                self.index += 1
            elif text.startswith("\\\n", self.index):
                self.index += 2
            elif char == "\n":
                self.index += 1
                if not self.conditional:
                    self.emit(_Token("\n", operator=True))
                self.read_heredocs()
            elif char == "#":
                end = text.find("\n", self.index)
                self.index = len(text) if end < 0 else end
            elif (
                char == ")"
                and self.closing
                and self.depth == 0
                and not self.conditional
                and self.cases[-1:] != ["pattern"]
            ):
                self.index += 1
                return self.tokens
            elif char in "<>" and text.startswith("(", self.index + 1):
                self.word()
            elif text.startswith("((", self.index) and (
                self.command_start or self.follows("for")
            ):
                end, arithmetic = _arithmetic_end(text, self.index)
                if arithmetic:
                    body = text[self.index + 2 : end - 2]
                    self.emit(
                        _Token(
                            _ARITHMETIC,
                            runs_molt=_runs_molt(body),
                            assigns=_arithmetic_assignments(body),
                        )
                    )
                    self.index = end
                else:
                    self.operator()
            elif char in ";&|()<>":
                self.operator()
            else:
                self.word()
        return self.tokens

    def follows(self, word: str) -> bool:
        return bool(self.tokens) and self.tokens[-1] == _Token(word)

    def emit(self, token: _Token) -> None:
        """Append ``token`` and track where commands start, `case` patterns,
        `[[ ... ]]` and the parentheses a `$(...)` body has open."""
        self.tokens.append(token)
        text, word = token.text, not token.operator and not token.quoted
        state = self.cases[-1] if self.cases else ""
        if self.conditional:
            self.conditional = not (word and text == "]]")
            self.command_start = False
        elif state == "subject":
            if word and text == "in":
                self.cases[-1] = "pattern"
        elif state == "pattern":
            if word and text == "esac":
                self.cases.pop()
                self.command_start = False
            elif token.operator and text == ")":
                self.cases[-1] = "body"
                self.command_start = True
        elif token.operator:
            if state == "body" and text in {";;", ";&", ";;&"}:
                self.cases[-1] = "pattern"
            else:
                self.depth += {"(": 1, ")": -1}.get(text, 0)
            self.command_start = text not in _REDIRECTIONS
        elif self.command_start and word and text == "case":
            self.cases.append("subject")
            self.command_start = False
        elif self.command_start and word and text == "esac" and state == "body":
            self.cases.pop()
            self.command_start = False
        elif self.command_start and word and text == "[[":
            self.conditional = True
        else:
            self.command_start = (
                self.command_start and word and (text in _COMMAND_PREFIX_WORDS)
            )

    def operator(self) -> None:
        op = next(op for op in _OPERATORS if self.text.startswith(op, self.index))
        self.index += len(op)
        # Inside `[[ ... ]]`, `<`, `&&`, `(` and the like are operands.
        self.emit(_Token(op, operator=not self.conditional))
        if op in {"<<", "<<-"} and not self.conditional:
            while self.text.startswith((" ", "\t"), self.index):
                self.index += 1
            if self.text[self.index : self.index + 1] in {"", *" \t\r\n;&|()<>"}:
                return  # no delimiter: bash refuses the script
            delimiter = self.word()
            if delimiter is not None:
                strip_tabs = op == "<<-"
                self.heredocs.append((delimiter.text, strip_tabs, len(self.tokens) - 1))

    def read_heredocs(self) -> None:
        """Skip the bodies of the heredocs the line just ended opened; a body
        whose text runs molt marks its delimiter."""
        for delimiter, strip_tabs, position in self.heredocs:
            body: list[str] = []
            while self.index < len(self.text):
                end = self.text.find("\n", self.index)
                end = len(self.text) if end < 0 else end
                line = self.text[self.index : end]
                self.index = end + 1
                if (line.lstrip("\t") if strip_tabs else line) == delimiter:
                    break
                body.append(line)
            if _runs_molt("\n".join(body)):
                token = self.tokens[position]
                self.tokens[position] = replace(token, runs_molt=token.runs_molt + 1)
        self.heredocs.clear()

    def word(self) -> _Token | None:
        """Read the word at the index; None for a redirection's descriptor
        (`2>`, `{fd}>`), which names no word."""
        text, start = self.text, self.index
        index = start
        parts: list[str] = []
        quoted = False
        runs_molt = 0
        assigns: list[str] = []
        while index < len(text):
            char = text[index]
            if char in "<>" and text.startswith("(", index + 1) and index == start:
                # A process substitution, read as a command substitution.
                lexer = _Lexer(text, index + 2, closing=True)
                runs_molt += int(_molt_occurrences(lexer.run()) > 0)
                parts.append(_SUBSTITUTION)
                index = lexer.index
                continue
            if (
                char == "("
                and not quoted
                and _ARRAY_ASSIGNMENT.fullmatch("".join(parts))
            ):
                end = _paired_end(text, index)
                parts.append(_UNKNOWN)
                runs_molt += _runs_molt(text[index:end])
                index = end
                continue
            if char in " \t\r\n;&|()<>":
                break
            expansion = None
            if char == "\\":
                if not text.startswith("\n", index + 1):
                    parts.append(text[index + 1 : index + 2])
                    quoted = True
                index += 2
            elif char == "'":
                close = text.find("'", index + 1)
                close = len(text) if close < 0 else close
                parts.append(text[index + 1 : close])
                quoted = True
                index = close + 1
            elif text.startswith("$'", index):
                end = _escaped_end(text, index + 2, "'")
                parts.append(_ANSI_C_STRING)
                quoted = True
                runs_molt += _runs_molt(text[index:end])
                index = end
            elif text.startswith('$"', index):
                index += 1
            elif char == '"':
                quoted = True
                expansion = _double_quoted(text, index)
            else:
                expansion = _expansion(text, index)
                if expansion is None:
                    parts.append(char)
                    index += 1
            if expansion is not None:
                parts.append(expansion.text)
                runs_molt += expansion.runs_molt
                assigns.extend(expansion.assigns)
                index = expansion.end
        self.index = index
        word = "".join(parts)
        if (
            not quoted
            and text[index : index + 1] in {"<", ">"}
            and not text.startswith("(", index + 1)
            and (word.isdigit() or re.fullmatch(r"\{[A-Za-z_][A-Za-z0-9_]*\}", word))
        ):
            return None
        token = _Token(word, quoted=quoted, runs_molt=runs_molt, assigns=tuple(assigns))
        self.emit(token)
        return token


def _tokens(run: str) -> list[_Token]:
    """The tokens of ``run``, GitHub's expressions first replaced as GitHub
    does before the shell starts: the workspace one by $GITHUB_WORKSPACE."""
    text = _GITHUB_EXPRESSION.sub(
        "GITHUB_EXPRESSION", _WORKSPACE_EXPRESSION.sub("$GITHUB_WORKSPACE", run)
    )
    return _Lexer(text).run()


def _shell_words(run: str) -> list[str]:
    return [token.text for token in _tokens(run)]


# The shell grammar's nodes: a simple command, a compound command (`{ }`,
# `( )`, if, while, until, for, select, case, coproc), a function definition,
# a pipeline, an `&&`/`||` list and a list of those, each in the foreground or
# the background.
@dataclass
class _Simple:
    tokens: list[_Token]


@dataclass
class _Compound:
    kind: str
    parts: list[_List]
    # Its own words: a loop's name and values, a case's subject and
    # patterns, and its redirections.
    tokens: list[_Token] = field(default_factory=list)
    names: list[str] = field(default_factory=list)  # what a for loop assigns
    fallthrough: bool = False  # a case branch ends with `;&` or `;;&`


@dataclass
class _Function:
    name: str
    body: _Simple | _Compound | _Function
    tokens: list[_Token]  # the body's


@dataclass
class _Pipeline:
    commands: list[_Simple | _Compound | _Function]


@dataclass
class _AndOr:
    pipelines: list[_Pipeline]


@dataclass
class _List:
    items: list[tuple[_AndOr, bool]]  # each with whether it runs in the background


class _Parser:
    """Parses tokens by the shell grammar. A syntax error, or a construct it
    does not parse, counts in ``errors``; parsing goes on past it."""

    def __init__(self, tokens: list[_Token]) -> None:
        self.tokens = tokens
        self.index = 0
        self.errors = 0

    def peek(self, offset: int = 0) -> _Token | None:
        position = self.index + offset
        return self.tokens[position] if position < len(self.tokens) else None

    def at_word(self, *texts: str, offset: int = 0) -> bool:
        token = self.peek(offset)
        return (
            token is not None
            and not token.operator
            and not token.quoted
            and token.text in texts
        )

    def at_operator(self, *texts: str, offset: int = 0) -> bool:
        token = self.peek(offset)
        return token is not None and token.operator and token.text in texts

    def take(self) -> _Token:
        self.index += 1
        return self.tokens[self.index - 1]

    def expect(self, word: str) -> None:
        if self.at_word(word):
            self.index += 1
        else:
            self.errors += 1

    def newlines(self) -> None:
        while self.at_operator("\n"):
            self.index += 1

    def program(self) -> _List:
        items: list[tuple[_AndOr, bool]] = []
        while self.peek() is not None:
            items.extend(self.list_(frozenset()).items)
            if self.peek() is not None:  # a closer nothing opened
                self.errors += 1
                self.index += 1
        return _List(items)

    def list_(self, closers: frozenset[str]) -> _List:
        items: list[tuple[_AndOr, bool]] = []
        self.newlines()
        while (token := self.peek()) is not None and not (
            token.text in closers and (token.operator or not token.quoted)
        ):
            start = self.index
            and_or = self.and_or()
            background = self.at_operator("&")
            if background or self.at_operator(";", "\n"):
                self.index += 1
            items.append((and_or, background))
            self.newlines()
            if self.index == start:  # nothing parsed: step over the token
                self.errors += 1
                self.index += 1
        return _List(items)

    def and_or(self) -> _AndOr:
        pipelines = [self.pipeline()]
        while self.at_operator("&&", "||"):
            self.index += 1
            self.newlines()
            pipelines.append(self.pipeline())
        return _AndOr(pipelines)

    def pipeline(self) -> _Pipeline:
        while self.at_word("!", "time"):
            self.index += (
                2 if self.at_word("time") and self.at_word("-p", offset=1) else 1
            )
        commands = [self.command()]
        while self.at_operator("|", "|&"):
            self.index += 1
            self.newlines()
            commands.append(self.command())
        return _Pipeline(commands)

    def command(self) -> _Simple | _Compound | _Function:
        if self.at_operator("("):
            self.index += 1
            body = self.list_(frozenset({")"}))
            if self.at_operator(")"):
                self.index += 1
            else:
                self.errors += 1
            return self.redirected(_Compound("subshell", [body]))
        if self.at_word("{"):
            self.index += 1
            body = self.list_(frozenset({"}"}))
            self.expect("}")
            return self.redirected(_Compound("brace", [body]))
        if self.at_word("if"):
            return self.if_()
        if self.at_word("while", "until"):
            kind = self.take().text
            condition = self.list_(frozenset({"do"}))
            self.expect("do")
            body = self.list_(frozenset({"done"}))
            self.expect("done")
            return self.redirected(_Compound(kind, [condition, body]))
        if self.at_word("for", "select"):
            return self.for_()
        if self.at_word("case"):
            return self.case()
        if self.at_word("coproc"):
            self.index += 1
            if self.at_word("{", "(", offset=1) or self.at_operator("(", offset=1):
                self.index += 1  # the coprocess's name
            command = self.command()
            return _Compound(
                "coproc", [_List([(_AndOr([_Pipeline([command])]), False)])]
            )
        token = self.peek()
        named = token is not None and not token.operator and not token.quoted
        if self.at_word("function") and self.peek(1) is not None:
            self.index += 1
            return self.function()
        if (
            named
            and self.at_operator("(", offset=1)
            and self.at_operator(")", offset=2)
        ):
            return self.function()
        if named and token is not None and token.text in _CLOSING_WORDS:
            self.errors += 1
            self.index += 1
            return _Simple([])
        return self.simple()

    def simple(self) -> _Simple:
        tokens: list[_Token] = []
        while (token := self.peek()) is not None:
            if token.operator and token.text not in _REDIRECTIONS:
                break
            tokens.append(self.take())
            target = self.peek()
            if token.operator and target is not None and not target.operator:
                tokens.append(self.take())
        return _Simple(tokens)

    def redirected(self, node: _Compound) -> _Compound:
        while (
            (token := self.peek()) is not None
            and token.operator
            and (token.text in _REDIRECTIONS)
        ):
            node.tokens.append(self.take())
            target = self.peek()
            if target is not None and not target.operator:
                node.tokens.append(self.take())
        return node

    def if_(self) -> _Compound:
        """`if` as [condition, body, condition, body, ..., else-body]."""
        self.index += 1
        parts = [self.list_(frozenset({"then"}))]
        self.expect("then")
        parts.append(self.list_(frozenset({"elif", "else", "fi"})))
        while self.at_word("elif"):
            self.index += 1
            parts.append(self.list_(frozenset({"then"})))
            self.expect("then")
            parts.append(self.list_(frozenset({"elif", "else", "fi"})))
        if self.at_word("else"):
            self.index += 1
            parts.append(self.list_(frozenset({"fi"})))
        self.expect("fi")
        return self.redirected(_Compound("if", parts))

    def for_(self) -> _Compound:
        node = _Compound(self.take().text, [])
        if (token := self.peek()) is not None and not token.operator:
            node.tokens.append(self.take())
            if token.text != _ARITHMETIC:
                node.names.append(token.text)
        self.newlines()
        if self.at_word("in"):
            self.index += 1
            while (token := self.peek()) is not None and not token.operator:
                node.tokens.append(self.take())
        if self.at_operator(";", "\n"):
            self.index += 1
        self.newlines()
        closer = "done" if self.at_word("do") else "}"
        self.expect("do" if closer == "done" else "{")
        node.parts.append(self.list_(frozenset({closer})))
        self.expect(closer)
        return self.redirected(node)

    def case(self) -> _Compound:
        node = _Compound("case", [])
        self.index += 1
        if (token := self.peek()) is not None and not token.operator:
            node.tokens.append(self.take())
        self.newlines()
        self.expect("in")
        self.newlines()
        while self.peek() is not None and not self.at_word("esac"):
            if self.at_operator("("):
                self.index += 1
            while (token := self.peek()) is not None and (
                not token.operator or token.text == "|"
            ):
                node.tokens.append(self.take())
            if not self.at_operator(")"):
                self.errors += 1
                break
            self.index += 1
            node.parts.append(self.list_(frozenset({";;", ";&", ";;&", "esac"})))
            node.fallthrough |= self.at_operator(";&", ";;&")
            if self.at_operator(";;", ";&", ";;&"):
                self.index += 1
            self.newlines()
        self.expect("esac")
        return self.redirected(node)

    def function(self) -> _Function:
        name = self.take().text
        if self.at_operator("(") and self.at_operator(")", offset=1):
            self.index += 2
        self.newlines()
        start = self.index
        body = self.command()
        return _Function(name, body, self.tokens[start : self.index])


def _python_module(words: list[str]) -> tuple[str | None, bool]:
    """The module `-m` names after an interpreter's options ("" when its
    operand is missing, as under `xargs`), and whether an option makes the
    interpreter ignore PYTHONPATH."""
    ignores_pythonpath = False
    index = 0
    while index < len(words):
        word = words[index]
        if word == "--" or not word.startswith("-") or word == "-":
            return None, ignores_pythonpath
        if word.startswith("--"):
            index += 2 if word == "--check-hash-based-pycs" else 1
            continue
        for position, option in enumerate(word[1:], start=1):
            rest = word[position + 1 :]
            if option == "m":
                module = rest or (words[index + 1] if index + 1 < len(words) else "")
                return module, ignores_pythonpath
            if option == "c":
                return None, ignores_pythonpath
            if option in _PYTHON_OPTIONS_WITH_ARGUMENT:
                index += 0 if rest else 1
                break
            ignores_pythonpath |= option in _PYTHON_IGNORES_PYTHONPATH
        index += 1
    return None, ignores_pythonpath


def _is_molt(module: str | None) -> bool:
    return module is not None and (module == "molt" or module.startswith("molt."))


def _is_expansion(word: str) -> bool:
    """Whether a word holds a value this reading does not resolve."""
    return any(mark in word for mark in ("$", "`", "MOLT-", "GITHUB_EXPRESSION"))


def _may_be_molt(module: str | None) -> bool:
    """Whether a module operand this reading cannot read may name molt."""
    return module is not None and (module == "" or _is_expansion(module))


def _with_expressions(value: object) -> str:
    """A value from YAML env, its GitHub expressions read as a script's are."""
    return _GITHUB_EXPRESSION.sub(
        "GITHUB_EXPRESSION",
        _WORKSPACE_EXPRESSION.sub("$GITHUB_WORKSPACE", str(value or "")),
    )


def _names_src(pythonpath: object, *, relative_ok: bool) -> bool:
    """Whether a PYTHONPATH value has an entry naming the repository's `src`.

    A value from YAML env still holds GitHub's expressions; they read as the
    script's do.
    """
    value = _with_expressions(pythonpath)
    return any(
        entry in _WORKSPACE_SRC or (relative_ok and entry in _RELATIVE_SRC)
        for entry in (part.rstrip("/") for part in value.split(":"))
    )


def _flag(value: object) -> bool:
    """Whether a uv boolean variable may be set: true, or a value this
    reading cannot know."""
    text = _with_expressions(value)
    return value is not None and (_is_expansion(text) or text.lower() in _TRUE)


def _names_workspace(value: object, *, moved: bool) -> bool:
    text = _with_expressions(value).rstrip("/")
    return text in _WORKSPACE or (not moved and text in {"", ".", "$PWD", "${PWD}"})


@dataclass
class _ShellState:
    """What one shell holds: exported and plain variables, whether it may have
    left the workspace directory, why its state is unknown ("" when it is
    known), the names local to the function being read (None outside one),
    and whether the project is synced."""

    exported: dict[str, object]
    variables: dict[str, object]
    moved: bool
    unknown: str = ""
    locals_: set[str] | None = None
    synced: bool = False

    def copy(self) -> _ShellState:
        return _ShellState(
            dict(self.exported),
            dict(self.variables),
            self.moved,
            self.unknown,
            None if self.locals_ is None else set(self.locals_),
            self.synced,
        )

    def become(self, other: _ShellState) -> None:
        self.exported, self.variables = other.exported, other.variables
        self.moved, self.unknown = other.moved, other.unknown
        self.locals_, self.synced = other.locals_, other.synced

    def entry(self, name: str) -> tuple[bool, object] | None:
        if name in self.exported:
            return True, self.exported[name]
        if name in self.variables:
            return False, self.variables[name]
        return None


def _join(states: list[_ShellState]) -> _ShellState:
    """What holds after one of ``states``, not knowing which: a variable they
    disagree on is unknown, and exported when any of them exports it."""
    joined = states[0].copy()
    names = {name for state in states for name in (*state.exported, *state.variables)}
    for name in names:
        entries = [state.entry(name) for state in states]
        if all(entry == entries[0] for entry in entries):
            continue
        joined.exported.pop(name, None)
        joined.variables.pop(name, None)
        if any(entry is not None and entry[0] for entry in entries):
            joined.exported[name] = _UNKNOWN
        else:
            joined.variables[name] = _UNKNOWN
    joined.moved = any(state.moved for state in states)
    joined.unknown = next((state.unknown for state in states if state.unknown), "")
    joined.synced = all(state.synced for state in states)
    if joined.locals_ is not None:
        joined.locals_ |= {n for state in states for n in state.locals_ or ()}
    return joined


def _mark_unknown(state: _ShellState, reason: str) -> None:
    if not state.unknown:
        state.unknown = reason


def _assign_unknown(state: _ShellState, names: Iterable[str]) -> None:
    for name in names:
        target = state.exported if name in state.exported else state.variables
        target[name] = _UNKNOWN


def _assigned(held: object, assignment: re.Match[str]) -> object:
    """The value `NAME=VALUE` or `NAME+=VALUE` leaves, ``held`` before it.
    A value with an unknown part is unknown as a whole."""
    value = assignment["value"]
    if assignment["append"] and held is not None:
        value = f"{held}{value}"
    return _UNKNOWN if _UNKNOWN in value else value


def _assign(state: _ShellState, word: str) -> None:
    """A plain assignment in the shell. A variable that may or may not be
    exported (unknown) stays unknown: the value may not reach the env."""
    assignment = _ASSIGNMENT.fullmatch(word)
    assert assignment is not None
    name = assignment["name"]
    if name in state.exported:
        held = state.exported[name]
        state.exported[name] = (
            _UNKNOWN if held == _UNKNOWN else _assigned(held, assignment)
        )
    else:
        state.variables[name] = _assigned(state.variables.get(name), assignment)


@dataclass
class _Reading:
    """Where a reading records molt runs, whether it reads a function body
    (whose runs happen where it is called), and for each enclosing loop the
    states `break` and `continue` leave it with."""

    found: list[tuple[str, bool]]
    deferred: bool = False
    loops: list[tuple[list[_ShellState], list[_ShellState]]] = field(
        default_factory=list
    )


def _molt_module_runs(
    run: str,
    env: dict[str, object],
    *,
    project_synced: bool,
    moved: bool = False,
) -> list[tuple[str, bool]]:
    """Each command that runs molt as a module, and whether molt can be
    imported there: `uv run` with the project (with `--no-sync`, only after a
    sync earlier in the job or script) or a PYTHONPATH entry naming the
    repository's `src` that the interpreter honors. The step, job and workflow
    env come first; the script is read by the grammar and the two rules
    above, so a run this reading cannot attribute is reported, never
    passed."""
    reading = _Reading([])
    state = _ShellState(dict(env), {}, moved, synced=project_synced)
    _read_script(run, state, reading)
    return reading.found


def _account(tokens: list[_Token], before: int, reading: _Reading) -> None:
    """The backstop: report each molt run counted in ``tokens`` that the
    reading of them (which recorded from ``before`` on) did not record."""
    missing = _molt_occurrences(tokens) - (len(reading.found) - before)
    command = " ".join(token.text for token in tokens if token.text != "\n")
    reading.found.extend([(f"{command} (unread molt run)", False)] * missing)


def _read_script(run: str, state: _ShellState, reading: _Reading) -> None:
    """Read ``run`` in ``state``'s shell."""
    tokens = _tokens(run)
    parser = _Parser(tokens)
    tree = parser.program()
    before = len(reading.found)
    if parser.errors:
        _mark_unknown(state, "a construct this reading does not parse")
    _read_list(tree, state, reading)
    _account(tokens, before, reading)


def _read_list(node: _List, state: _ShellState, reading: _Reading) -> None:
    for and_or, background in node.items:
        # A background command runs in a subshell, maybe still running later.
        _read_and_or(and_or, state.copy() if background else state, reading)


def _read_and_or(node: _AndOr, state: _ShellState, reading: _Reading) -> None:
    _read_pipeline(node.pipelines[0], state, reading)
    for pipeline in node.pipelines[1:]:
        skipped = state.copy()  # `&&` and `||` may skip the pipeline
        _read_pipeline(pipeline, state, reading)
        state.become(_join([state, skipped]))


def _read_pipeline(node: _Pipeline, state: _ShellState, reading: _Reading) -> None:
    if len(node.commands) == 1:
        _read_node(node.commands[0], state, reading)
        return
    for command in node.commands:  # each element runs in a subshell
        element = state.copy()
        _read_node(command, element, reading)
        state.synced |= element.synced


def _read_node(
    node: _Simple | _Compound | _Function, state: _ShellState, reading: _Reading
) -> None:
    if isinstance(node, _Simple):
        before = len(reading.found)
        _read_command(node.tokens, state, reading)
        _account(node.tokens, before, reading)
    elif isinstance(node, _Function):
        # The body runs where the function is called: its runs are read
        # deferred, and a body that can change the shell makes it unknown.
        body = state.copy()
        body.locals_ = set()
        _read_node(node.body, body, _Reading(reading.found, deferred=True))
        if _body_changes_shell(node, state):
            _mark_unknown(state, f"function {node.name}, whose body changes the shell")
    else:
        _read_compound(node, state, reading)


def _body_changes_shell(node: _Function, state: _ShellState) -> bool:
    """Whether a function body, wherever it is called, can change a variable,
    an export, the directory or what this reading knows. It is read from two
    probes in which each name its words hold has a value no assignment
    repeats, exported in one and not in the other, so any write shows."""
    names = {
        name
        for token in node.tokens
        for name in (*_NAME.findall(token.text), *token.assigns)
    }
    for exported in (True, False):
        probe = state.copy()
        probe.locals_ = set()
        for name in names:
            probe.exported.pop(name, None)
            probe.variables.pop(name, None)
            (probe.exported if exported else probe.variables)[name] = f"MOLT-{name}"
        before = probe.copy()
        _read_node(node.body, probe, _Reading([], deferred=True))
        written = {
            *probe.exported,
            *probe.variables,
            *before.exported,
            *before.variables,
        }
        if (
            probe.moved != before.moved
            or probe.unknown != before.unknown
            or any(
                probe.entry(name) != before.entry(name)
                for name in written - (probe.locals_ or set())
            )
        ):
            return True
    return False


def _read_compound(node: _Compound, state: _ShellState, reading: _Reading) -> None:
    _assign_unknown(state, [name for token in node.tokens for name in token.assigns])
    if node.kind == "brace":
        _read_list(node.parts[0], state, reading)
    elif node.kind in {"subshell", "coproc"}:
        inner = state.copy()
        _read_list(node.parts[0], inner, reading)
        state.synced |= node.kind == "subshell" and inner.synced
    elif node.kind == "if":
        _read_if(node.parts, state, reading)
    elif node.kind == "case":
        if node.fallthrough:
            _mark_unknown(state, "a case branch that falls through")
        branches = [state.copy() for _ in node.parts]
        for part, branch in zip(node.parts, branches):
            _read_list(part, branch, reading)
        state.become(_join([state, *branches]))
    else:
        _read_loop(node, state, reading)
    _account(node.tokens, len(reading.found), reading)


def _read_if(parts: list[_List], state: _ShellState, reading: _Reading) -> None:
    """Each branch from the state its condition leaves; then their join."""
    has_else = len(parts) % 2 == 1
    outcomes: list[_ShellState] = []
    _read_list(parts[0], state, reading)  # the first condition always runs
    pending = state.copy()  # every condition so far false
    for index in range(1, len(parts) - has_else, 2):
        if index > 1:
            _read_list(parts[index - 1], pending, reading)
        branch = pending.copy()
        _read_list(parts[index], branch, reading)
        outcomes.append(branch)
    if has_else:
        _read_list(parts[-1], pending, reading)
    state.become(_join([*outcomes, pending]))


def _read_loop(node: _Compound, state: _ShellState, reading: _Reading) -> None:
    """A loop's body runs any number of times: read it from the join of the
    states every pass can start with (a fixed point; values only turn
    unknown, so it settles), recording its runs once, from that state."""

    def one_pass(
        start: _ShellState, recording: _Reading
    ) -> tuple[list[_ShellState], list[_ShellState]]:
        _assign_unknown(start, node.names)
        breaks: list[_ShellState] = []
        continues: list[_ShellState] = []
        inner = _Reading(
            recording.found, recording.deferred, [*recording.loops, (breaks, continues)]
        )
        for part in node.parts:
            _read_list(part, start, inner)
        return breaks, continues

    head = state.copy()
    while True:
        trial = head.copy()
        _breaks, continues = one_pass(
            trial, _Reading([], reading.deferred, reading.loops)
        )
        joined = _join([head, trial, *continues])
        if joined == head:
            break
        head = joined
    final = head.copy()
    breaks, _continues = one_pass(final, reading)
    state.become(_join([head, final, *breaks]))


def _declare(argv: list[str], state: _ShellState) -> None:
    """Apply `export`, `unset`, `declare`, `typeset`, `local` or `readonly`."""
    command = argv[0]
    options = "".join(word[1:] for word in argv[1:] if word[:1] == "-" and word != "--")
    removed = "".join(word[1:] for word in argv[1:] if word[:1] == "+")
    operands = [word for word in argv[1:] if word[:1] not in {"-", "+"}]
    if command == "unset":
        if "f" in options and "v" not in options:
            return  # functions, not variables
        if any(not _NAME.fullmatch(name) for name in operands):
            _mark_unknown(state, f"`{shlex.join(argv)}`, which names an expansion")
            return
        for name in operands:
            state.exported.pop(name, None)
            state.variables.pop(name, None)
        return
    if "n" in options and command != "export":
        _mark_unknown(state, f"`{command} -n`, a name reference")
        return
    if command == "local" and state.locals_ is None:
        _mark_unknown(state, "`local` outside a function")
        return
    if "f" in options or "F" in options:
        return  # functions, not variables
    if any(
        not _NAME.fullmatch(word.split("=", 1)[0].removesuffix("+"))
        for word in operands
    ):
        _mark_unknown(state, f"`{shlex.join(argv)}`, which names an expansion")
        return
    if operands and (command == "readonly" or "r" in options):
        # bash refuses each later write to the name, keeping its value.
        _mark_unknown(state, f"`{shlex.join(argv)}`, which refuses later writes")
    exporting = "x" in options or (command == "export" and "n" not in options)
    unexporting = "x" in removed or (command == "export" and "n" in options)
    # Integer, case-changing and array attributes rewrite the value.
    rewrites = any(flag in options for flag in "iluacA")
    scoped = state.locals_ is not None and (
        command == "local" or (command in {"declare", "typeset"} and "g" not in options)
    )
    for word in operands:
        assignment = _ASSIGNMENT.fullmatch(word)
        name = assignment["name"] if assignment else word
        if scoped and state.locals_ is not None:
            state.locals_.add(name)
        held = state.exported.get(name, state.variables.get(name))
        value = None if assignment is None else _assigned(held, assignment)
        if rewrites and (value is not None or held is not None):
            value = _UNKNOWN
        if unexporting:
            state.exported.pop(name, None)
            state.variables[name] = held if value is None else value
        elif exporting:
            state.variables.pop(name, None)
            state.exported[name] = held if value is None else value
        elif value is not None:
            target = state.exported if name in state.exported else state.variables
            target[name] = value


def _sets_allexport(words: list[str]) -> bool:
    """Whether `set` turns on allexport (`-a`) or keyword (`-k`), which make
    assignments export."""
    index = 0
    while index < len(words):
        word = words[index]
        if word == "--" or word[:1] not in {"-", "+"} or len(word) < 2:
            return False
        if word[0] == "-" and ("a" in word[1:] or "k" in word[1:]):
            return True
        if "o" in word[1:]:
            index += 1
            if word[0] == "-" and words[index : index + 1] in (
                ["allexport"],
                ["keyword"],
            ):
                return True
        index += 1
    return False


# Traps that run in an ordinary run of a script: on each command, on each
# return, on a failed command and on a child's exit. Others run only at exit
# or on a signal from outside.
_ORDINARY_TRAPS = frozenset({"DEBUG", "RETURN", "ERR", "CHLD", "CLD"})


def _trap_runs_code(words: list[str]) -> bool:
    """Whether `trap` sets code that runs in an ordinary run of the script."""
    operands = [word for word in words if word != "--"]
    if not operands or operands[0] in {"-l", "-p", "-P", "-", ""}:
        return False
    return any(
        signal.upper().removeprefix("SIG") in _ORDINARY_TRAPS
        or (signal.isdigit() and signal != "0")
        for signal in operands[1:]
    )


def _unmodeled(argv: list[str]) -> bool:
    """Whether a builtin command changes the shell in a way this reading does
    not model: `source`, an alias, allexport or keyword, lastpipe, a trap
    that runs code, `enable` or `fc`."""
    name, words = argv[0], argv[1:]
    return (
        name in {"source", "."}
        or name in _UNMODELED_BUILTINS
        or (name == "alias" and any("=" in word for word in words))
        or (name == "set" and _sets_allexport(words))
        or (name == "shopt" and "-s" in words and "lastpipe" in words)
        or (name == "trap" and _trap_runs_code(words))
    )


def _assigned_names(argv: list[str]) -> list[str] | None:
    """The variables an assigning builtin may set; None when an expansion
    names one."""
    command, words = argv[0], argv[1:]
    if command == "let":
        if any(_is_expansion(word) for word in words):
            return None
        return [name for word in words for name in _arithmetic_assignments(word)]
    if command in {"printf", "wait"}:
        flag = "-v" if command == "printf" else "-p"
        words = [
            following if word == flag else word[2:]
            for word, following in zip(words, [*words[1:], ""])
            if word.startswith(flag)
        ]
    if any(_is_expansion(word) for word in words):
        return None
    return [word for word in words if _NAME.fullmatch(word)]


def _record(
    reading: _Reading, state: _ShellState, argv: list[str], provided: bool
) -> None:
    command = shlex.join(argv)
    if not provided and (reading.deferred or state.unknown):
        command += f" (after {state.unknown or 'a function body'})"
    reading.found.append((command, provided))


def _read_command(tokens: list[_Token], state: _ShellState, reading: _Reading) -> None:
    """Read one simple command in ``state``'s shell."""
    for token in tokens:  # expansions that assign do so as the words expand
        _assign_unknown(state, token.assigns)
    argv: list[str] = []
    redirects: list[tuple[str, str]] = []
    index = 0
    while index < len(tokens):
        if tokens[index].operator:  # the parser put each target after its operator
            target = tokens[index + 1 : index + 2]
            redirects.append((tokens[index].text, target[0].text if target else ""))
            index += 1 + len(target)
        else:
            argv.append(tokens[index].text)
            index += 1
    if not argv:
        return
    if all(_ASSIGNMENT.fullmatch(word) for word in argv):
        for word in argv:
            _assign(state, word)
        return
    local = dict(state.exported)
    moved = state.moved
    while argv:
        if assignment := _ASSIGNMENT.fullmatch(argv[0]):
            # A prefix `+=` appends to the shell's value, exported or not.
            name = assignment["name"]
            held = local[name] if name in local else state.variables.get(name)
            local[name] = _assigned(held, assignment)
            argv = argv[1:]
        elif argv[0] == "env":
            argv = argv[1:]
            while argv and argv[0].startswith("-"):
                option = argv.pop(0)
                if option == "--":
                    break
                if option in {"-i", "-", "--ignore-environment"}:
                    local.clear()
                elif option in {"-u", "--unset"} and argv:
                    local.pop(argv.pop(0), None)
                elif option.startswith(("-u", "--unset=")):
                    local.pop(option.split("=", 1)[-1].removeprefix("-u"), None)
                elif option in {"-C", "--chdir"} and argv:
                    argv.pop(0)
                    moved = True
                elif option.startswith(("-C", "--chdir=")):
                    moved = True
                elif option not in {"-0", "--null", "-v", "--debug"}:
                    # `-S` splits a string into words; other options are
                    # unknown to this reading.
                    _mark_unknown(state, f"`env {option}`")
                    if option in {"-S", "--split-string"} and argv:
                        argv = [*_shell_words(argv[0]), *argv[1:]]
                    elif option.startswith(("-S", "--split-string=")):
                        split = (
                            option.split("=", 1)[-1] if "=" in option else option[2:]
                        )
                        argv = [*_shell_words(split), *argv]
        elif argv[0] in _COMMAND_WRAPPERS:
            operands, valued = _COMMAND_WRAPPERS[argv[0]]
            argv = argv[1:]
            while argv and argv[0].startswith("-"):
                argv = argv[2:] if argv[0] in valued else argv[1:]
            argv = argv[operands:]
        elif argv[0] in {"command", "builtin"}:
            prefix, argv = argv[0], argv[1:]
            options = []
            while argv and argv[0].startswith("-") and argv[0] != "-":
                options.append(argv.pop(0))
            if prefix == "command" and any("v" in o or "V" in o for o in options):
                return  # a query: it runs nothing
            # It runs a builtin or a command bypassing functions: what it runs
            # is read on, and the shell is unknown after it.
            _mark_unknown(state, f"`{prefix}`")
        else:
            break
    if not argv:
        return
    name = argv[0]
    if name in {"cd", "pushd", "popd"}:
        operands = [
            word for word in argv[1:] if not word.startswith("-") or word == "-"
        ]
        if name == "cd" and operands[:1] == ["."]:
            return
        state.moved = (
            name != "cd" or len(operands) != 1 or operands[0] not in _WORKSPACE
        )
        return
    if name in _DECLARATIONS:
        _declare(argv, state)
        return
    if name == "eval":  # its string runs in this shell: its changes stay
        _read_script(" ".join(argv[1:]), state, reading)
        return
    if name in {"break", "continue"} and reading.loops:
        count = int(argv[1]) if argv[1:2] and argv[1].isdigit() else 1
        breaks, continues = reading.loops[-min(max(count, 1), len(reading.loops))]
        (breaks if name == "break" else continues).append(state.copy())
        return
    if _unmodeled(argv):
        _mark_unknown(state, f"`{shlex.join(argv)}`")
        return
    if name in _ASSIGNING_BUILTINS:
        names = _assigned_names(argv)
        if names is None:
            _mark_unknown(
                state, f"`{shlex.join(argv)}`, which assigns a name an expansion holds"
            )
        else:
            _assign_unknown(state, names)
        return
    if name in _INERT_COMMANDS or name in {
        "set", "shopt", "trap", "alias", "break", "continue"
    }:  # fmt: skip
        return
    uv_provided = False
    if argv[:2] == ["uv", "sync"]:
        state.synced = True
        return
    if argv[:2] == ["uv", "run"]:
        rest = argv[2:]
        uv_provided = not _flag(local.get("UV_NO_PROJECT"))
        no_sync = _flag(local.get("UV_NO_SYNC"))
        locations = {
            option: local.get(variable)
            for option, variable in _UV_RUN_LOCATIONS.items()
        }
        module = None
        while rest and rest[0].startswith("-"):
            option, _, attached = rest.pop(0).partition("=")
            if option in {"-m", "--module"}:
                module = attached or (rest[0] if rest else "")
                break
            uv_provided &= option not in _UV_RUN_WITHOUT_PROJECT
            no_sync |= option == "--no-sync"
            if option in _UV_RUN_VALUE_OPTIONS and not attached and rest:
                attached = rest.pop(0)
            if option in locations:
                locations[option] = attached
        # uv finds the project from the directory it runs in, unless told.
        uv_provided &= all(
            _names_workspace(value, moved=moved)
            for value in locations.values()
            if value is not None
        ) and (not moved or any(value is not None for value in locations.values()))
        uv_provided &= state.synced or not no_sync
        if module is not None:
            if _is_molt(module) or _may_be_molt(module):
                _record(reading, state, argv, uv_provided and _is_molt(module))
            return
        argv = rest
        if not argv:
            return
    program = argv[0].rsplit("/", 1)[-1]
    if program in _SHELLS:
        script = _shell_script(argv[1:], redirects)
        if script is not None:  # a child shell: it inherits only the env
            child = _ShellState(
                dict(local), {}, moved, state.unknown, synced=state.synced
            )
            _read_script(script, child, _Reading(reading.found, reading.deferred))
            state.synced |= child.synced
        return
    if program == "py":  # the Windows launcher, as `py -3.12`
        argv = argv[1:]
        if argv and re.fullmatch(r"-[0-9.]+(-[0-9]+)?", argv[0]):
            argv = argv[1:]
    elif _PYTHON_EXECUTABLE.fullmatch(program):
        argv = argv[1:]
    elif _is_expansion(argv[0]):
        if _python_module(argv[1:])[0] is None:
            # It may name a builtin or a function, which can change the shell,
            # and a value that runs molt (from the step's env, say).
            reference = _VARIABLE_REFERENCE.fullmatch(argv[0])
            name = reference["name"] if reference else ""
            if _runs_molt(str(local.get(name, state.variables.get(name, "")))):
                _record(reading, state, argv, False)
            _mark_unknown(state, f"`{argv[0]}`, a command named by an expansion")
            return
        argv = argv[1:]
    else:
        # A command this reading does not know (uv's other commands run no
        # shell code). Before an interpreter it may be a wrapper that changes
        # what the interpreter runs with, or runs it in this shell.
        for index, word in enumerate(argv[1:], start=1):
            program = word.rsplit("/", 1)[-1]
            if argv[0] != "uv" and (
                program in _SHELLS
                or program == "py"
                or _PYTHON_EXECUTABLE.fullmatch(program)
            ):
                _mark_unknown(state, f"`{argv[0]}` before an interpreter")
                if _may_be_molt(_python_module(argv[index + 1 :])[0]):
                    _record(reading, state, argv, False)
                break
        return
    module, ignores_pythonpath = _python_module(argv)
    if _is_molt(module):
        _record(
            reading,
            state,
            argv,
            uv_provided
            or (
                not reading.deferred
                and not state.unknown
                and not ignores_pythonpath
                and _names_src(local.get("PYTHONPATH"), relative_ok=not moved)
            ),
        )
    elif _may_be_molt(module):
        _record(reading, state, argv, False)


# Shell options that take the next word as their value.
_SHELL_VALUE_OPTIONS = frozenset({"-o", "+o", "-O", "+O", "--rcfile", "--init-file"})


def _shell_script(words: list[str], redirects: list[tuple[str, str]]) -> str | None:
    """The script a shell command runs from a string: its `-c` operand, or a
    `<<<` string; None when it runs a file or reads stdin."""
    command_string = False
    operands: list[str] = []
    index = 0
    while index < len(words):
        word = words[index]
        if word in _SHELL_VALUE_OPTIONS:
            index += 2
            continue
        if word == "--":
            operands = words[index + 1 :]
            break
        if word.startswith("--"):
            index += 1
            continue
        if word[:1] in {"-", "+"} and len(word) > 1:
            command_string |= "c" in word[1:]
            index += 1
            continue
        operands = words[index:]
        break
    if command_string:
        return operands[0] if operands else ""
    strings = [target for operator, target in redirects if operator == "<<<"]
    return strings[0] if strings else None


def _github_env_writes(run: str) -> dict[str, str]:
    """`NAME=VALUE` lines a step appends to $GITHUB_ENV, which later steps of
    the job receive as env."""
    written: dict[str, str] = {}
    command: list[str] = []
    for token in [*_tokens(run), _Token("\n", operator=True)]:
        if not (token.operator and token.text in _COMMAND_SEPARATORS):
            command.append(token.text)
            continue
        if (
            command[:1] == ["echo"]
            and len(command) == 4
            and command[2] == ">>"
            and command[3] in {"$GITHUB_ENV", "${GITHUB_ENV}"}
            and (assignment := _ASSIGNMENT.fullmatch(command[1]))
        ):
            written[assignment["name"]] = assignment["value"]
        command = []
    return written


# A composite action's step that runs only when the action's `sync` input is
# true (its `if` requires it) runs after the action's sync step.
_SYNC_INPUT_CONDITION = re.compile(
    r"(?:inputs|steps\.inputs\.outputs)\.sync\s*==\s*'true'"
)


def _requires_sync_input(condition: str) -> bool:
    """Whether a step's `if` requires the action's sync input: one of its
    top-level `&&` conjuncts, with no `||` or negation anywhere."""
    text = condition.strip()
    if text.startswith("${{") and text.endswith("}}"):
        text = text[3:-2].strip()
    if "||" in text or "!" in text.replace("!=", ""):
        return False
    return any(
        _SYNC_INPUT_CONDITION.fullmatch(part.strip().strip("()").strip())
        for part in text.split("&&")
    )


_WORKSPACE_DIRECTORIES = frozenset(
    {"", ".", "$GITHUB_WORKSPACE", "${{ github.workspace }}"}
)


@dataclass(frozen=True)
class _StepRun:
    name: str
    run: str
    env: dict[str, object]
    project_synced: bool
    moved: bool


def _step_runs_with_env(payload: dict[str, object]) -> list[_StepRun]:
    """Each workflow or composite step's run script, with the env GitHub gives
    it (workflow, job, step, and what earlier steps appended to $GITHUB_ENV),
    whether the project was synced before it (setup-project with `sync: true`
    or a `uv sync` in a job; the action's sync input in a composite action's
    `if`), and whether `working-directory` moves it off the workspace."""
    top = dict(payload.get("env") or {})
    found: list[_StepRun] = []
    jobs = payload.get("jobs") or {}
    groups = [
        ({**top, **dict(job.get("env") or {})}, job.get("steps") or [], False)
        for job in jobs.values()
    ]
    runs = payload.get("runs")
    if isinstance(runs, dict):
        groups.append((top, runs.get("steps") or [], True))
    for env, steps, composite in groups:
        synced = False
        appended: dict[str, object] = {}
        for step in steps:
            run = step.get("run")
            if isinstance(run, str):
                condition = str(step.get("if") or "")
                directory = str(step.get("working-directory") or "").rstrip("/")
                found.append(
                    _StepRun(
                        str(step.get("name")),
                        run,
                        {**env, **appended, **dict(step.get("env") or {})},
                        synced or (composite and _requires_sync_input(condition)),
                        directory not in _WORKSPACE_DIRECTORIES,
                    )
                )
                words = _shell_words(run)
                synced |= any(
                    words[index : index + 2] == ["uv", "sync"]
                    for index in range(len(words))
                )
                appended.update(_github_env_writes(run))
            elif step.get("uses") == "./.github/actions/setup-project":
                synced |= str((step.get("with") or {}).get("sync")).lower() == "true"
    return found


def test_every_step_that_runs_a_molt_module_can_import_molt() -> None:
    """A bare `python3 -m molt.<module>` finds molt only through PYTHONPATH.

    molt lives in `src`, and setup-project syncs the project only when its
    `sync` input is true, so such a step needs `uv run` (which syncs; with
    `--no-sync`, after a sync earlier in the job), an inline `PYTHONPATH`
    naming `src`, or a step, job or workflow `PYTHONPATH` naming `src`
    (HF-456 review A-N1: formal.yml's Quint install lost it when it moved
    to the pinned npm).
    """
    files = [
        *sorted((REPO_ROOT / ".github/workflows").glob("*.yml")),
        *sorted((REPO_ROOT / ".github/actions").glob("*/action.yml")),
    ]
    offenders = []
    checked = 0
    for path in files:
        payload = yaml.safe_load(path.read_text(encoding="utf-8"))
        for step in _step_runs_with_env(payload):
            for command, provided in _molt_module_runs(
                step.run,
                step.env,
                project_synced=step.project_synced,
                moved=step.moved,
            ):
                checked += 1
                if not provided:
                    offenders.append(
                        f"{path.relative_to(REPO_ROOT)}: {step.name}: {command}"
                    )
    assert checked
    assert offenders == []


class _Case(NamedTuple):
    """A run script and the reading's verdict for the one molt run in it:
    whether molt can be imported there, or None when no command runs molt as
    a module. ``bash`` is the oldest bash major the oracle runs it under, or
    None when the oracle cannot (the tree lacks a tool it needs, or molt runs
    where the oracle cannot see it). ``conservative`` marks a case the
    reading reports though bash imports molt; the oracle checks that too."""

    run: str
    provided: bool | None
    bash: int | None = 3
    conservative: bool = False


_MOLT_RUN_CASES = [
    _Case("python3 -m molt.tool_releases run npm", False),
    _Case("python3 -B -u -m molt.tool_releases provision uv", False),
    _Case("python3.12 -m molt build app.py", False, None),
    _Case('"$PYTHON" -m molt.checkout_provisioning ensure', False),
    _Case("${PYTHON} -m molt", False),
    _Case("python3 \\\n  -m molt.tool_releases run npm", False),
    _Case("cd . && PYTHONPATH=src python3 -m molt.llvm_toolchain", True),
    _Case(
        "uv run --no-sync python3 -m molt.tool_releases provision sccache", True, None
    ),
    _Case("uv run python3 -m molt.checkout_provisioning ensure", True, None),
    _Case("uv run -m molt.x", True, None),
    _Case("uv run --python python3.12 python -m molt.x", True, None),
    # HF-456 follow-up review L-1: spellings the earlier matcher misread.
    _Case("uv run --no-sync --no-project python3 -m molt.x", False, None),
    _Case("echo PYTHONPATH=src; python3 -m molt.x", False),
    _Case("uv run ruff check . && python3 -m molt.x", False, None),
    _Case("PYTHONPATH=src python3 -I -m molt.x", False),
    _Case("PYTHONPATH=src python3 -E -m molt.x", False),
    _Case("PYTHONPATH=/opt/notsrc python3 -m molt.x", False),
    _Case("PYTHONPATH=src python3 -X importtime -m molt.x", True),
    _Case("PYTHONPATH=src python3 -W error -m molt.x", True),
    _Case("PYTHONPATH=src python3 -mmolt.x", True),
    _Case("PYTHONPATH=src py -3.12 -m molt.x", True, None),
    _Case("PYTHONPATH=src $VENV_PY -m molt.x", True),
    _Case("# python3 -m molt.x", None),
    _Case("export PYTHONPATH=src\npython3 -m molt.x", True),
    _Case("export PYTHONPATH=src\nunset PYTHONPATH\npython3 -m molt.x", False),
    _Case("env PYTHONPATH=src python3 -m molt.x", True),
    _Case("env -i PATH=$PATH python3 -m molt.x", False),
    _Case("timeout 60 env PYTHONPATH=src python3 -m molt.x", True, None),
    _Case("PYTHONPATH='${{ github.workspace }}/src' python3 -m molt.x", True, None),
    _Case("python3 --check-hash-based-pycs always -m molt.x", False),
    # Follow-up review v2: comments, strings a shell runs, heredocs, scopes.
    _Case("export FOO=1 # note\npython3 -m molt.x", False),
    _Case("export PYTHONPATH=src\nunset PYTHONPATH # gone\npython3 -m molt.x", False),
    _Case("echo hi # comment\nPYTHONPATH=src python3 -m molt.x", True),
    _Case("echo a#b; PYTHONPATH=src python3 -m molt.x", True),
    _Case("bash -c 'python3 -m molt.x'", False),
    _Case("bash -ec 'PYTHONPATH=src python3 -m molt.x'", True),
    _Case('eval "python3 -m molt.x"', False),
    _Case("bash <<< 'python3 -m molt.x'", False),
    _Case(
        "cat > /dev/null <<EOF\nexport PYTHONPATH=src\nEOF\npython3 -m molt.x", False
    ),
    # A heredoc fed to a shell is not read: the backstop reports its body.
    _Case("bash <<'EOF'\nPYTHONPATH=src python3 -m molt.x\nEOF", False, 3, True),
    _Case("( export PYTHONPATH=src ); python3 -m molt.x", False),
    _Case("(export PYTHONPATH=src); python3 -m molt.x", False),
    _Case("( export PYTHONPATH=src && python3 -m molt.x )", True),
    _Case("mkdir -p sub && cd sub && PYTHONPATH=src python3 -m molt.x", False),
    _Case("PYTHONPATH=vendor/src python3 -m molt.x", False),
    _Case("PYTHONPATH=$PWD/src python3 -m molt.x", True),
    _Case("PYTHONPATH=$GITHUB_WORKSPACE/src python3 -m molt.x", True),
    _Case("export PYTHONPATH=src\nPYTHONPATH=/opt\npython3 -m molt.x", False),
    _Case("PYTHONPATH=src\nexport PYTHONPATH\npython3 -m molt.x", True),
    _Case("PYTHONPATH=src\npython3 -m molt.x", False),
    _Case("export PYTHONPATH=src\nexport -n PYTHONPATH\npython3 -m molt.x", False),
    _Case("UV_NO_PROJECT=1 uv run python3 -m molt.x", False, None),
    _Case("uv run --only-dev python3 -m molt.x", False, None),
    _Case("if PYTHONPATH=src python3 -m molt.x; then :; fi", True),
    _Case("{ PYTHONPATH=src python3 -m molt.x; }", True),
    _Case("exec env PYTHONPATH=src python3 -m molt.x", True),
    _Case("nice -n 5 env PYTHONPATH=src python3 -m molt.x", True),
    # Follow-up review v3: options with values, quoting, eval, scopes.
    _Case("bash -o pipefail -c 'python3 -m molt.x'", False),
    _Case("bash -O extglob -c 'PYTHONPATH=src python3 -m molt.x'", True),
    _Case(
        "cat > /dev/null <<\\EOF\nexport PYTHONPATH=src\nEOF\npython3 -m molt.x", False
    ),
    _Case("echo $'a\\'b'\npython3 -m molt.x", False),
    _Case('echo "$(echo "a # b")"; python3 -m molt.x', False),
    _Case("( case x in x) export PYTHONPATH=src ;; esac ); python3 -m molt.x", False),
    # A case branch may not run: what it sets is unknown after the case.
    _Case(
        "case x in x) export PYTHONPATH=src ;; esac\npython3 -m molt.x", False, 3, True
    ),
    _Case('export PYTHONPATH=src\neval "unset PYTHONPATH"\npython3 -m molt.x', False),
    _Case('eval "cd /tmp"\nPYTHONPATH=src python3 -m molt.x', False),
    _Case(
        "mkdir -p sub; pushd sub >/dev/null; PYTHONPATH=src python3 -m molt.x", False
    ),
    _Case("export PYTHONPATH=src\ndeclare +x PYTHONPATH\npython3 -m molt.x", False),
    _Case("declare -x PYTHONPATH=src\npython3 -m molt.x", True),
    _Case("export PYTHONPATH=src | cat\npython3 -m molt.x", False),
    _Case("export PYTHONPATH=src &\nwait\npython3 -m molt.x", False),
    # A substitution that runs molt is reported; its output is not seen.
    _Case("x=$(PYTHONPATH=src python3 -m molt.x)", False, None),
    _Case("sudo python3 -m molt.x", False, None),
    _Case("python3 -m moltlib.thing", None),
    _Case("python3 tools/proof_plan.py --check", None, None),
    # Review round 5, B-1: a heredoc delimiter is any shell word.
    _Case(
        "cat >/dev/null <<'END-X'\nexport PYTHONPATH=src\nEND-X\npython3 -m molt.x",
        False,
    ),
    _Case(
        'cat >/dev/null <<"END.X"\nexport PYTHONPATH=src\nEND.X\npython3 -m molt.x',
        False,
    ),
    _Case(
        "cat >/dev/null <<1EOF\nexport PYTHONPATH=src\n1EOF\npython3 -m molt.x", False
    ),
    _Case("cat >/dev/null << -X\nexport PYTHONPATH=src\n-X\npython3 -m molt.x", False),
    _Case('cat >/dev/null <<E"O"F\nfoo\nEOF\nPYTHONPATH=src python3 -m molt.x', True),
    _Case("cat <<EOF | bash\npython3 -m molt.x\nEOF", False),
    # B-2: comments, case patterns and ${...} inside substitutions and
    # scopes, and quoted operators.
    _Case("( x=$(echo hi # )\n); export PYTHONPATH=src ); python3 -m molt.x", False),
    _Case(
        "( x=$(case a in a) echo y;; esac); export PYTHONPATH=src ); python3 -m molt.x",
        False,
        5,
    ),
    _Case("x=$(case a in a) echo y;; esac); PYTHONPATH=src python3 -m molt.x", True, 5),
    _Case("( x=${y:-)}; export PYTHONPATH=src ); python3 -m molt.x", False),
    _Case("x=${y:-)}; PYTHONPATH=src python3 -m molt.x", True),
    _Case('echo ")"; PYTHONPATH=src python3 -m molt.x', True),
    _Case("x=$(echo hi # )\n); PYTHONPATH=src python3 -m molt.x", True),
    _Case("cat >/dev/null <<'END-X'\n)\nEND-X\nPYTHONPATH=src python3 -m molt.x", True),
    _Case('x="${y:-)}"; ( export PYTHONPATH=src ); python3 -m molt.x', False),
    _Case('( echo ")"; export PYTHONPATH=src ); python3 -m molt.x', False),
    _Case("( x=$')'; export PYTHONPATH=src ); python3 -m molt.x", False),
    _Case(
        "x=$(cat <<EOF\n)\nEOF\n); export PYTHONPATH=src; python3 -m molt.x", True, 5
    ),
    _Case("cd . 2>/dev/null && PYTHONPATH=src python3 -m molt.x", True),
    _Case("[[ -n x && -n y ]] && PYTHONPATH=src python3 -m molt.x", True),
    _Case(
        "diff <(echo a) <(echo a) >/dev/null && PYTHONPATH=src python3 -m molt.x", True
    ),
    # B-3: `+=` appends, plain, as a prefix and in a declaration.
    _Case("export PYTHONPATH=src; PYTHONPATH+=x; python3 -m molt.x", False),
    _Case("export PYTHONPATH=/opt; PYTHONPATH+=:src; python3 -m molt.x", True),
    _Case("export PYTHONPATH=src; export PYTHONPATH+=x; python3 -m molt.x", False),
    _Case("PYTHONPATH=src; PYTHONPATH+=x python3 -m molt.x", False),
    _Case(
        "declare -x PYTHONPATH=/opt; declare PYTHONPATH+=:src; python3 -m molt.x", True
    ),
    # B-4: `local` outside a function and a name reference are unknown.
    _Case("local -x PYTHONPATH=src; python3 -m molt.x", False),
    _Case(
        "export PYTHONPATH=src; local PYTHONPATH=/opt; python3 -m molt.x",
        False,
        3,
        True,
    ),
    _Case(
        "export PYTHONPATH=src; declare -n r=PYTHONPATH; unset r; python3 -m molt.x",
        False,
        5,
    ),
    _Case(
        "export PYTHONPATH=/opt; declare -n r=PYTHONPATH; r=src; python3 -m molt.x",
        False,
        5,
        True,
    ),
    # B-5: prefixes that hide a run are read through, or the run is reported.
    _Case("command bash -c 'python3 -m molt.x'", False),
    _Case("builtin eval 'python3 -m molt.x'", False),
    _Case("env -S 'PYTHONPATH=src python3 -m molt.x'", False, 3, True),
    _Case("echo molt.x | xargs python3 -m", False),
    _Case("command -v python3 >/dev/null && PYTHONPATH=src python3 -m molt.x", True),
    # Rule 1: what this reading does not model makes the shell unknown.
    _Case(
        "export PYTHONPATH=src; echo 'unset PYTHONPATH' > u.sh; . ./u.sh; python3 -m molt.x",
        False,
    ),
    _Case("set -a; PYTHONPATH=src; python3 -m molt.x", False, 3, True),
    _Case("set -o allexport\nPYTHONPATH=src\npython3 -m molt.x", False, 3, True),
    _Case(
        "shopt -s lastpipe; echo src | read PYTHONPATH; export PYTHONPATH; python3 -m molt.x",
        False,
        5,
        True,
    ),
    _Case("alias python3='env PYTHONPATH=src python3'\npython3 -m molt.x", False),
    _Case(
        "f() { unset PYTHONPATH; }; export PYTHONPATH=src; f; python3 -m molt.x", False
    ),
    _Case("f() { cd /tmp; }; PYTHONPATH=src python3 -m molt.x", False, 3, True),
    _Case("f() { python3 -m molt.x; }; export PYTHONPATH=src; f", False, 3, True),
    _Case(
        "export PYTHONPATH=src; f() { python3 -m molt.x; }; unset PYTHONPATH; f", False
    ),
    _Case(
        'run() { "$@"; }; export PYTHONPATH=src; run unset PYTHONPATH; python3 -m molt.x',
        False,
    ),
    _Case("c=unset; export PYTHONPATH=src; $c PYTHONPATH; python3 -m molt.x", False),
    _Case(
        "trap 'unset PYTHONPATH' DEBUG; export PYTHONPATH=src; python3 -m molt.x", False
    ),
    _Case("trap 'rm -f gone' EXIT; PYTHONPATH=src python3 -m molt.x", True),
    _Case(
        "case a in a) export PYTHONPATH=/opt;& b) export PYTHONPATH=src;; esac; python3 -m molt.x",
        False,
        5,
        True,
    ),
    # Values this reading cannot know name no `src`.
    _Case(
        "export PYTHONPATH=src; read PYTHONPATH </dev/null; python3 -m molt.x", False
    ),
    _Case(
        "export PYTHONPATH=src; for PYTHONPATH in a; do :; done; python3 -m molt.x",
        False,
    ),
    _Case(": ${PYTHONPATH:=src}; export PYTHONPATH; python3 -m molt.x", False, 3, True),
    _Case("export PYTHONPATH=src; (( PYTHONPATH = 3 )); python3 -m molt.x", False),
    _Case("export PYTHONPATH=src; x=$((PYTHONPATH = 3)); python3 -m molt.x", False),
    _Case("export PYTHONPATH=src; printf -v PYTHONPATH %s x; python3 -m molt.x", False),
    _Case("export PYTHONPATH=src; PYTHONPATH=(x); python3 -m molt.x", False),
    _Case("export PYTHONPATH=src:; PYTHONPATH+=(x); python3 -m molt.x", False),
    _Case("declare -x -u PYTHONPATH=src; python3 -m molt.x", False),
    _Case("export PYTHONPATH=src; unset -f PYTHONPATH; python3 -m molt.x", True),
    _Case("cd; PYTHONPATH=src python3 -m molt.x", False),
    _Case("readonly PYTHONPATH=/opt; export PYTHONPATH=src; python3 -m molt.x", False),
    _Case('cd /tmp; cd "$GITHUB_WORKSPACE"; PYTHONPATH=src python3 -m molt.x', True),
    # Branches, loops, `&&` and `||` may skip what they set: a join.
    _Case("if false; then export PYTHONPATH=src; fi; python3 -m molt.x", False),
    _Case("false && export PYTHONPATH=src; python3 -m molt.x", False),
    _Case(
        "export PYTHONPATH=src; true || unset PYTHONPATH; python3 -m molt.x",
        False,
        3,
        True,
    ),
    _Case(
        "if false; then export PYTHONPATH=src; else export PYTHONPATH=src; fi\n"
        "python3 -m molt.x",
        True,
    ),
    _Case(
        "export PYTHONPATH=src; for i in 1 2; do python3 -m molt.x; unset PYTHONPATH; done",
        False,
    ),
    _Case(
        "export PYTHONPATH=src; while :; do unset PYTHONPATH; break; done; python3 -m molt.x",
        False,
    ),
    _Case("export PYTHONPATH=src; while :; do break; done; python3 -m molt.x", True),
    _Case("{ export PYTHONPATH=src; } | cat; python3 -m molt.x", False),
    _Case(
        "echo x | while read l; do export PYTHONPATH=src; done; python3 -m molt.x",
        False,
    ),
    _Case("if true; then export PYTHONPATH=src; fi | cat; python3 -m molt.x", False),
    _Case(
        "export PYTHONPATH=src; i=0; until [ $i = 2 ]; do python3 -m molt.x; "
        "unset PYTHONPATH; i=$((i+1)); done",
        False,
    ),
    _Case(
        "f() { eval \"$1\"; }; export PYTHONPATH=src; f 'unset PYTHONPATH'; "
        "python3 -m molt.x",
        False,
    ),
    _Case(
        "f() { g() { unset PYTHONPATH; }; }; export PYTHONPATH=src; f; g; "
        "python3 -m molt.x",
        False,
    ),
    _Case("coproc { export PYTHONPATH=src; }; wait; python3 -m molt.x", False, 5),
    # Rule 2: molt text no reading attributes is reported.
    _Case('echo "python3 -m molt.x"', False),
    _Case("bash -c $'python3 -m molt.x'", False),
    _Case("trap 'python3 -m molt.x' EXIT", False),
    _Case('MODULE=molt.x; PYTHONPATH=src python3 -m "$MODULE"', False, 3, True),
    _Case("uv run --project /tmp python -m molt.x", False, None),
    _Case("cd sub && uv run python -m molt.x", False, None),
    _Case(
        'cd /tmp && uv run --directory "$GITHUB_WORKSPACE" python -m molt.x', True, None
    ),
]


@pytest.mark.parametrize("case", _MOLT_RUN_CASES, ids=[c.run for c in _MOLT_RUN_CASES])
def test_molt_module_run_recognizes_each_spelling(case: _Case) -> None:
    """The topology rule's reading of a step (HF-456 reviews)."""
    runs = _molt_module_runs(case.run, {}, project_synced=True)
    expected = [] if case.provided is None else [case.provided]
    assert [found for _command, found in runs] == expected, runs


def test_the_backstop_reports_each_command_it_cannot_attribute() -> None:
    """Rule 2 counts per command: a run reported for one command (here a
    module xargs supplies) does not cover molt text in another."""
    runs = _molt_module_runs(
        'echo molt.x | xargs python3 -m; echo "python3 -m molt.y"',
        {},
        project_synced=True,
    )
    assert [provided for _command, provided in runs] == [False, False]
    assert "molt.y" in runs[1][0]


@pytest.mark.parametrize(
    ("run", "env", "provided"),
    [
        ("python3 -m molt.x", {"PYTHONPATH": "${{ github.workspace }}/src"}, True),
        ("python3 -m molt.x", {"PYTHONPATH": "/w/srcs"}, False),
        ("python3 -m molt.x", {"PYTHONPATH": "/opt/elsewhere/src"}, False),
        ("python3 -m molt.x", {"PYTHONPATH": "src"}, True),
        ("python3 -I -m molt.x", {"PYTHONPATH": "${{ github.workspace }}/src"}, False),
        (
            "PYTHONPATH=/opt/lib python3 -m molt.x",
            {"PYTHONPATH": "${{ github.workspace }}/src"},
            False,
        ),
        (
            "unset PYTHONPATH\npython3 -m molt.x",
            {"PYTHONPATH": "${{ github.workspace }}/src"},
            False,
        ),
        (
            "env -u PYTHONPATH python3 -m molt.x",
            {"PYTHONPATH": "${{ github.workspace }}/src"},
            False,
        ),
        # A command a variable names runs what the step's env holds.
        ('"$MOLT_COMMAND"', {"MOLT_COMMAND": "python3 -m molt.x"}, False),
        (
            "export PYTHONPATH=src; $SETUP; python3 -m molt.x",
            {"SETUP": "source env.sh"},
            False,
        ),
    ],
)
def test_a_step_job_or_workflow_pythonpath_provides_molt(
    run: str, env: dict[str, object], provided: bool
) -> None:
    """The env GitHub gives a step, unless the script overrides or ignores it."""
    assert [
        found for _command, found in _molt_module_runs(run, env, project_synced=True)
    ] == [provided]


def test_uv_run_without_sync_needs_an_earlier_sync() -> None:
    """`uv run --no-sync` uses the environment as it is: molt is there only
    after setup-project's sync or a `uv sync` (follow-up review L-1)."""
    run = "uv run --no-sync python3 -m molt.x"
    assert _molt_module_runs(run, {}, project_synced=False)[0][1] is False
    assert _molt_module_runs(run, {}, project_synced=True)[0][1] is True
    assert _molt_module_runs(f"uv sync --frozen\n{run}", {}, project_synced=False)[0][1]
    steps = _step_runs_with_env(
        {
            "jobs": {
                "job": {
                    "steps": [
                        {"name": "before", "run": run},
                        {
                            "uses": "./.github/actions/setup-project",
                            "with": {"sync": "true"},
                        },
                        {"name": "after", "run": run},
                    ]
                }
            }
        }
    )
    assert [(step.name, step.project_synced) for step in steps] == [
        ("before", False),
        ("after", True),
    ]


@pytest.mark.parametrize(
    ("condition", "required"),
    [
        ("steps.inputs.outputs.sync == 'true'", True),
        ("${{ inputs.sync == 'true' && runner.os != 'Windows' }}", True),
        ("inputs.sync == 'true' || runner.os == 'Linux'", False),
        ("!(inputs.sync == 'true')", False),
        ("runner.os != 'Windows'", False),
    ],
)
def test_a_composite_step_counts_as_synced_only_when_its_if_requires_it(
    condition: str, required: bool
) -> None:
    """Follow-up review v3: a mention of the sync input is not a requirement."""
    assert _requires_sync_input(condition) is required


def test_github_env_and_working_directory_reach_later_steps() -> None:
    """$GITHUB_ENV appends become later steps' env; a `working-directory`
    off the workspace makes a relative `src` name another directory."""
    run = "python3 -m molt.x"
    steps = _step_runs_with_env(
        {
            "jobs": {
                "job": {
                    "steps": [
                        {"name": "before", "run": run},
                        {
                            "name": "export",
                            "run": 'echo "PYTHONPATH=${{ github.workspace }}/src" >> "$GITHUB_ENV"',
                        },
                        {"name": "after", "run": run},
                        {
                            "name": "moved",
                            "run": "PYTHONPATH=src python3 -m molt.x",
                            "working-directory": "sub",
                        },
                    ]
                }
            }
        }
    )
    found = {
        step.name: [
            provided
            for _command, provided in _molt_module_runs(
                step.run,
                step.env,
                project_synced=step.project_synced,
                moved=step.moved,
            )
        ]
        for step in steps
    }
    assert found == {"before": [False], "export": [], "after": [True], "moved": [False]}


@dataclass(frozen=True)
class _ShellOracle:
    """The bash that runs the cases, and the tree it runs them in."""

    bash: str
    version: str
    major: int
    root: Path
    environment: dict[str, str]


@pytest.fixture(scope="module")
def shell_oracle(tmp_path_factory: pytest.TempPathFactory) -> _ShellOracle:
    """A tree whose `src/molt` prints a marker, and the newest bash the host
    has: the first bash 5 or later on PATH or in Homebrew's prefixes, else
    the first bash found. GitHub's runners run bash 5."""
    import sys

    from molt.process_environment import ProcessEnvironment
    from tests.process_guard_common import run_guarded_test_process

    root = tmp_path_factory.mktemp("shell-oracle")
    package = root / "src" / "molt"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text("", encoding="utf-8")
    marker = "print('molt-imported')\n"
    for module in ("__main__", "x", "tool_releases", "checkout_provisioning",
                   "llvm_toolchain"):  # fmt: skip
        (package / f"{module}.py").write_text(marker, encoding="utf-8")
    (root / "home").mkdir()
    # Every interpreter the scripts reach is the base CPython through a shim
    # outside any virtual environment, with no user site: the only molt it
    # can import is this tree's src/molt, never the checkout's own.
    shims = root / "bin"
    shims.mkdir()
    base = Path(os.path.realpath(sys.executable))
    for name in ("python3", "python"):
        (shims / name).symlink_to(base)
    environment = {
        "PATH": f"{shims}:/usr/bin:/bin",
        "HOME": str(root / "home"),
        "GITHUB_WORKSPACE": str(root),
        "PYTHON": str(shims / "python3"),
        "VENV_PY": str(shims / "python3"),
        "PYTHONNOUSERSITE": "1",
    }
    probe = run_guarded_test_process(
        [
            str(shims / "python3"),
            "-c",
            "import importlib.util as u; print(u.find_spec('molt'))",
        ],
        cwd=shims,
        env=ProcessEnvironment(environment),
        timeout=60,
    )
    assert probe.stdout.strip() == "None", probe.stdout + probe.stderr
    candidates = [
        *(
            Path(entry) / "bash"
            for entry in os.environ.get("PATH", "").split(os.pathsep)
            if entry
        ),
        Path("/opt/homebrew/bin/bash"),
        Path("/usr/local/bin/bash"),
        Path("/bin/bash"),
    ]
    found: list[tuple[int, str, str]] = []
    seen: set[str] = set()
    for candidate in candidates:
        real = os.path.realpath(candidate)
        if real in seen or not os.access(candidate, os.X_OK) or not candidate.is_file():
            continue
        seen.add(real)
        result = run_guarded_test_process(
            [str(candidate), "-c", 'echo "${BASH_VERSINFO[0]} $BASH_VERSION"'],
            env=ProcessEnvironment(environment),
            timeout=60,
        )
        major, _, version = result.stdout.strip().partition(" ")
        if major.isdigit():
            found.append((int(major), version, str(candidate)))
    assert found, "no bash on PATH or in /opt/homebrew/bin, /usr/local/bin, /bin"
    major, version, bash = next((entry for entry in found if entry[0] >= 5), found[0])
    warnings.warn(f"the shell oracle runs {bash} {version}", stacklevel=1)
    return _ShellOracle(bash, version, major, root, environment)


_ORACLE_CASES = [case for case in _MOLT_RUN_CASES if case.bash is not None]


@pytest.mark.skipif(os.name == "nt", reason="the oracle runs POSIX shell scripts")
@pytest.mark.parametrize("case", _ORACLE_CASES, ids=[c.run for c in _ORACLE_CASES])
def test_molt_module_runs_match_real_execution(
    case: _Case, shell_oracle: _ShellOracle
) -> None:
    """An independent oracle: bash runs the case in a tree whose `src/molt`
    prints a marker, so molt was importable exactly when every run printed
    it and none failed to find molt. The reading's verdict must match, or,
    for a conservative case, bash must import molt where the reading
    reports."""
    from molt.process_environment import ProcessEnvironment
    from tests.process_guard_common import run_guarded_test_process

    assert case.bash is not None
    if shell_oracle.major < case.bash:
        pytest.skip(
            f"needs bash {case.bash} or later; the oracle runs "
            f"{shell_oracle.bash} {shell_oracle.version}"
        )
    result = run_guarded_test_process(
        [shell_oracle.bash, "-c", case.run],
        cwd=shell_oracle.root,
        env=ProcessEnvironment(shell_oracle.environment),
        timeout=60,
    )
    imported = (
        "molt-imported" in result.stdout and "No module named" not in result.stderr
    )
    expected = True if case.conservative else bool(case.provided)
    assert imported is expected, (
        f"{shell_oracle.bash} {shell_oracle.version}",
        case.run,
        result.stdout,
        result.stderr,
    )


def test_setup_project_callers_use_declared_inputs() -> None:
    action = yaml.safe_load(_read(".github/actions/setup-project/action.yml"))
    declared = set(action["inputs"])
    calls = 0
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        payload = yaml.safe_load(workflow.read_text(encoding="utf-8"))
        for job in payload.get("jobs", {}).values():
            for step in job.get("steps", []):
                if step.get("uses") != "./.github/actions/setup-project":
                    continue
                calls += 1
                assert set(step.get("with", {})) <= declared, workflow
    assert calls >= 10


def test_every_cargo_cache_job_saves_even_when_its_proofs_fail() -> None:
    # actions/cache saves only after a successful job, so a red job never
    # warmed its own cache and cold-compiled every dependency on every run.
    cache_jobs = 0
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        payload = yaml.safe_load(workflow.read_text(encoding="utf-8"))
        for name, job in payload.get("jobs", {}).items():
            steps = job.get("steps", [])
            caches = any(
                step.get("uses") == "./.github/actions/setup-project"
                and str(step.get("with", {}).get("cache-cargo")) == "true"
                for step in steps
            )
            saves = [
                index
                for index, step in enumerate(steps)
                if step.get("uses") == "./.github/actions/save-cargo-cache"
            ]
            assert bool(saves) == caches, (workflow.name, name)
            if caches:
                cache_jobs += 1
                assert saves == [len(steps) - 1], (workflow.name, name)
                assert steps[-1]["if"] == "${{ !cancelled() }}", (workflow.name, name)
    assert cache_jobs >= 10

    save = yaml.safe_load(_read(".github/actions/save-cargo-cache/action.yml"))
    prune, persist = save["runs"]["steps"]
    restore = next(
        step
        for step in yaml.safe_load(_read(".github/actions/setup-project/action.yml"))[
            "runs"
        ]["steps"]
        if step.get("id") == "cargo-restore"
    )
    main_only = "env.MOLT_CARGO_CACHE_KEY != '' && github.ref == 'refs/heads/main'"
    assert prune["if"] == persist["if"] == main_only
    assert prune["run"] == "python3 tools/ci_cargo_cache.py prune"
    assert re.fullmatch(r"actions/cache/save@[0-9a-f]{40}", persist["uses"])
    assert persist["uses"].split("@")[1] == restore["uses"].split("@")[1]
    # actions/cache folds the path list into the cache version.
    assert persist["with"]["path"] == restore["with"]["path"]
    assert persist["with"]["key"] == "${{ env.MOLT_CARGO_CACHE_KEY }}"


def test_setup_project_cache_identity_is_complete_and_non_incremental() -> None:
    action = _read(".github/actions/setup-project/action.yml")
    normalizer = _read(".github/actions/setup-project/normalize-inputs.sh")
    for token in (
        "inputs.cache-namespace",
        "inputs.rust-toolchain",
        "inputs.rust-components",
        "inputs.rust-targets",
        "rust-toolchain.toml",
        "Cargo.lock",
        "**/Cargo.toml",
        "config/llvm_toolchain_releases.toml",
        "config/llvm_toolchain_arches.toml",
    ):
        assert token in action
    # The proof plan is not a Cargo input; hashing it re-keyed every cache on
    # each unrelated proof edit.
    assert "tools/proof_plan.toml" not in action
    # Jobs build different crate sets into different target directories, and
    # actions/cache folds paths into the cache version: the job id must be part
    # of the identity or one job's key shadows every other job's cache.
    assert '"${GITHUB_JOB:?}" | git hash-object --stdin' in normalizer
    steps = yaml.safe_load(action)["runs"]["steps"]
    configure = next(step for step in steps if step.get("id") == "cargo-cache")
    restore = next(step for step in steps if step.get("id") == "cargo-restore")
    record = next(
        step for step in steps if step.get("name") == "Record Cargo cache save key"
    )
    assert re.fullmatch(r"actions/cache/restore@[0-9a-f]{40}", restore["uses"])
    assert (
        configure["if"]
        == restore["if"]
        == record["if"]
        == "steps.inputs.outputs.cache-cargo == 'true'"
    )
    assert configure["run"] == "python3 tools/ci_cargo_cache.py configure"
    cached_paths = restore["with"]["path"].split()
    assert "${{ env.MOLT_CARGO_CACHE_TARGET }}" in restore["with"]["path"]
    for source in (
        "~/.cargo/registry/index",
        "~/.cargo/registry/cache",
        "~/.cargo/git/db",
    ):
        assert source in cached_paths
    for token in ("runner.os", "runner.arch", "steps.inputs.outputs.rust-cache-token"):
        assert token in restore["with"]["key"]
        assert token in restore["with"]["restore-keys"]
    # A per-run key is never an exact hit, so main always saves a fresh cache;
    # restore-keys prefer the newest cache for the same lockfile.
    assert "github.run_id" in restore["with"]["key"]
    assert "github.run_attempt" in restore["with"]["key"]
    assert "github.run_id" not in restore["with"]["restore-keys"]
    assert "hashFiles(" in restore["with"]["key"]
    assert "steps.cargo-restore.outputs.cache-primary-key" in str(record["env"])
    assert "MOLT_CARGO_CACHE_KEY=" in record["run"]
    install = next(step for step in steps if step.get("name") == "Install exact Rust")
    assert (
        steps.index(install)
        < steps.index(configure)
        < steps.index(restore)
        < steps.index(record)
    )
    # One Cargo restore; the Lean and actionlint restores are main-only twins.
    assert (
        sum(
            "actions/cache/restore@" in str(step.get("uses"))
            and "~/.cargo/registry/index" in str((step.get("with") or {}).get("path"))
            for step in steps
        )
        == 1
    )
    assert "cache-uv requires uv" in normalizer
    assert "sync requires uv" in normalizer
    assert "cache-cargo requires rust-toolchain" in normalizer
    assert "actionlint requires python" in normalizer
    assert "dtolnay/rust-toolchain@" not in action
    assert (
        install["run"]
        == "python3 -I -B .github/actions/setup-project/provision-rust.py"
    )
    assert install["env"]["RUST_TOOLCHAIN_ROLE"] == "${{ inputs.rust-toolchain }}"
    assert "RUSTUP_AUTO_INSTALL=0" in steps[0]["run"]
    assert 'if [[ -n "$SYNC_GROUPS" ]]' in action
    group_guard = action.split('if [[ -n "$SYNC_GROUPS" ]]', 1)[1].split(
        "        fi", 1
    )[0]
    assert 'for group in "${groups[@]}"' in group_guard
    assert "steps.inputs.outputs.rust-cache-token" in action
    assert "steps.inputs.outputs.cache-namespace" in action
    assert "sync-args" not in action
    assert "normalize-inputs.sh" in action
    assert (
        'run: bash .github/actions/setup-project/normalize-inputs.sh "$GITHUB_OUTPUT"'
        in action
    )
    assert "inputs.rust-components }}-${{ inputs.rust-targets" not in action


def test_workflow_shells_do_not_select_artifacts_with_ls() -> None:
    perf_demo = _read(".github/workflows/perf_demo.yml")
    release = _read(".github/workflows/release.yml")
    assert "latest=$(ls " not in perf_demo
    assert "WHEEL=$(ls " not in release
    assert "set -- dist/molt-*.whl" not in release
    assert release.count("release_authority select-one") == 4


def test_composite_action_shells_never_interpolate_inputs_directly() -> None:
    actions_root = REPO_ROOT / ".github" / "actions"
    checked = 0
    for action_path in sorted(actions_root.glob("*/action.y*ml")):
        payload = yaml.safe_load(action_path.read_text(encoding="utf-8"))
        for step in payload.get("runs", {}).get("steps", []):
            run = step.get("run")
            if not isinstance(run, str):
                continue
            checked += 1
            assert re.search(r"\$\{\{\s*inputs\.", run) is None, (
                action_path,
                step.get("name"),
            )
    assert checked >= 5


def test_release_candidate_admits_both_installed_guest_targets() -> None:
    steps = yaml.safe_load(_read(".github/workflows/release.yml"))["jobs"][
        "candidates"
    ]["steps"]
    setups = [
        step for step in steps if step.get("uses") == "./.github/actions/setup-project"
    ]
    assert len(setups) == 1
    setup = setups[0]["with"]
    rust = tomllib.loads(_read("rust-toolchain.toml"))["toolchain"]
    assert setup["node-version"] == "pinned"
    assert setup["rust-toolchain"] == "pinned"
    assert {part.strip() for part in setup["rust-targets"].split(",")} >= set(
        rust["targets"]
    )
    consumer_index = next(
        index
        for index, step in enumerate(steps)
        if "tools.release.verify_consumer" in str(step.get("run", ""))
    )
    assert steps.index(setups[0]) < consumer_index
    provision_index = next(
        index
        for index, step in enumerate(steps)
        if "tools.release.provision_execution_archives" in str(step.get("run", ""))
    )
    build_index = next(
        index
        for index, step in enumerate(steps)
        if "tools.release.build_compiler" in str(step.get("run", ""))
    )
    assert steps.index(setups[0]) < provision_index < build_index < consumer_index
    provision = steps[provision_index]
    consumer = steps[consumer_index]
    assert "if" not in provision and not provision.get("continue-on-error", False)
    assert '--target "${{ matrix.id }}"' in provision["run"]
    cache = (
        '--execution-archive-cache "${{ runner.temp }}/molt-release-execution-archives"'
    )
    assert cache in provision["run"] and cache in consumer["run"]
    assert "tools/guarded_exec.py --prefix MOLT_RELEASE --" in provision["run"]


@pytest.mark.parametrize(
    ("event", "selected", "expected"),
    [
        ("schedule", False, True),
        ("push", False, False),
        ("pull_request", False, False),
        ("workflow_dispatch", False, False),
        ("push", True, True),
        ("pull_request", True, True),
        ("workflow_dispatch", True, True),
    ],
)
def test_security_reusable_selection_truth_table(
    event: str, selected: bool, expected: bool
) -> None:
    assert (event == "schedule" or selected) is expected
    text = _read(".github/workflows/security_hardening.yml")
    assert "if: github.event_name == 'schedule' || inputs.python_security" in text
    assert "if: github.event_name == 'schedule' || inputs.rust_security" in text


@pytest.mark.parametrize(
    ("workflow", "job"),
    [
        ("ci.yml", "docs-gates"),
        ("ci.yml", "platform-portability"),
        ("ci.yml", "native-integration"),
        ("ci.yml", "rust-build-unit-smoke"),
        ("ci.yml", "llvm-backend"),
        ("molt-wasm-ci.yml", "wasm-build"),
        ("security_hardening.yml", "rust-security"),
    ],
)
def test_ci_rust_consumers_share_compiled_artifact_cache(
    workflow: str, job: str
) -> None:
    steps = yaml.safe_load(_read(f".github/workflows/{workflow}"))["jobs"][job]["steps"]
    setups = [
        step for step in steps if step.get("uses") == "./.github/actions/setup-project"
    ]
    assert len(setups) == 1
    assert setups[0]["with"]["rust-toolchain"]
    assert setups[0]["with"]["cache-cargo"] == "true"
    assert not any("rust-cache@" in step.get("uses", "") for step in steps)


def test_portability_image_donor_resolves_groups_after_verified_python() -> None:
    steps = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"][
        "platform-portability"
    ]["steps"]
    setup = next(
        step for step in steps if step.get("uses") == "./.github/actions/setup-project"
    )
    assert "if" not in setup
    assert setup["with"]["sync"] == "true"
    assert setup["with"]["sync-frozen"] == "true"
    assert setup["with"]["sync-project-groups"] == "true"
    assert "sync-groups" not in setup["with"]
    assert "sync-dev" not in setup["with"]
    action = yaml.safe_load(_read(".github/actions/setup-project/action.yml"))
    assert action["inputs"]["sync-project-groups"]["default"] == "false"
    action_steps = action["runs"]["steps"]
    python = next(step for step in action_steps if step.get("id") == "python")
    sync = next(
        step
        for step in action_steps
        if step.get("name") == "Synchronize project dependencies"
    )
    assert action_steps.index(python) < action_steps.index(sync)
    assert (
        sync["env"]["SYNC_PROJECT_GROUPS"]
        == "${{ steps.inputs.outputs.sync-project-groups }}"
    )
    assert "DxProject(root).dependency_groups()" in sync["run"]
    assert "python3 -I -S -B" in sync["run"]


@pytest.mark.parametrize(
    ("workflow", "job"),
    [("ci.yml", "rust-build-unit-smoke"), ("molt-wasm-ci.yml", "wasm-build")],
)
def test_ci_luau_runner_uses_digest_bound_prebuilt_authority(
    workflow: str, job: str
) -> None:
    text = _read(f".github/workflows/{workflow}")
    steps = yaml.safe_load(text)["jobs"][job]["steps"]
    provision = next(
        step
        for step in steps
        if step.get("name") == "Install pinned executable Luau proof runner"
    )
    assert provision["run"] == (
        'python3 -m molt.tool_releases provision lune --github-path "$GITHUB_PATH"'
    )
    assert "cargo install lune" not in text
    assert steps.index(provision) < next(
        index
        for index, step in enumerate(steps)
        if "--run-family" in step.get("run", "")
    )


def test_ci_push_path_is_cheap_only() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    docs_gate = ci_text.split("  docs-gates:", 1)[1].split("\n  classify-changes:", 1)[
        0
    ]
    rustfmt_setup = docs_gate.index("uses: ./.github/actions/setup-project")
    repository_executor = docs_gate.index("Execute repository policy partitions")
    assert rustfmt_setup < repository_executor
    assert "rust-components: rustfmt, clippy" in docs_gate
    assert "rust-targets: wasm32-wasip1" in docs_gate

    assert "concurrency:" in ci_text
    assert "merge_group:" in ci_text
    assert "github.event_name == 'pull_request'" in ci_text
    assert "format('pr-{0}', github.event.pull_request.number)" in ci_text
    assert "cancel-in-progress: ${{ github.event_name == 'pull_request' }}" in (ci_text)
    assert "docs-gates:" in ci_text
    assert "classify-changes:" in ci_text
    assert "name: Changed Path Classifier" in ci_text
    # The frontend-Python ty type-check is a zero-diagnostic ratchet enforced in
    # CI (pre-commit is not run in Actions), mirroring the pre-commit `ty` hook.
    assert 'argv = ["uv", "run", "ty", "check", "src"]' in _read(
        "tools/proof_plan.toml"
    )
    # Proof spelling lives only in the manifest; docs-gates is executor mechanics.
    assert "--run-family repository_policy --receipt" in ci_text
    assert 'id = "repository.differential.layout"' in _read("tools/proof_plan.toml")
    assert "uv run python3 tools/check_differential_suite_layout.py" not in ci_text
    assert "python-static:" in ci_text
    assert "python-unit:" in ci_text
    assert "native-integration:" in ci_text
    assert "needs: classify-changes" in ci_text
    assert "if: needs.classify-changes.outputs.python_static == 'true'" in ci_text
    assert "rust-build-unit-smoke:" in ci_text
    assert "if: needs.classify-changes.outputs.rust == 'true'" in ci_text
    assert "llvm-backend:" in ci_text
    assert "if: needs.classify-changes.outputs.llvm == 'true'" in ci_text
    assert "      - docs-gates" in ci_text
    assert "differential-tests:" not in ci_text
    assert "benchmark:" not in ci_text
    assert "parity:" not in ci_text
    assert "runs-on: ubuntu-latest" in ci_text
    assert "runs-on: ${{ matrix.runner }}" in ci_text
    # Each matrix job consumes only its own family's classifier output.
    for name in ("platform_portability", "python_unit"):
        assert (
            f"matrix: ${{{{ fromJSON(needs.classify-changes.outputs.{name}_matrix) }}}}"
            in ci_text
        )
    assert "classify-changes.outputs.matrix" not in ci_text
    assert "Swatinem/rust-cache@" not in ci_text
    assert "uses: ./.github/actions/setup-project" in ci_text
    # setup-project plans every job's resources once; no workflow restates it.
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        assert "ci_resource_env.py" not in workflow.read_text(encoding="utf-8")
    setup_steps = yaml.safe_load(_read(".github/actions/setup-project/action.yml"))[
        "runs"
    ]["steps"]
    names = [step.get("name") for step in setup_steps]
    plan_step = setup_steps[names.index("Plan job resources")]
    assert plan_step["run"] == (
        'python3 tools/ci_resource_env.py --github-env "$GITHUB_ENV"'
    )
    assert names.index("Plan job resources") > names.index(
        "Provision and bind repository Python"
    )
    assert 'CARGO_BUILD_JOBS: "1"' not in ci_text
    assert 'sync: "true"' in ci_text
    assert 'sync-frozen: "true"' in ci_text
    assert "sync-groups: dev" in ci_text
    proof_plan_text = _read("tools/proof_plan.toml")
    assert '"-m", "not slow"' in proof_plan_text
    assert "native.integration.bench-cli" in proof_plan_text
    assert "native.integration.capability-manifest" in proof_plan_text
    assert '"tests/test_manifest_pipeline_e2e.py"' in proof_plan_text
    assert "tests/test_bench_tool.py::test_bench_no_cpython_sets_null_baseline" not in (
        ci_text
    )
    assert (
        "tests/test_bench_tool.py::test_bench_runtime_timeout_marks_molt_not_ok"
        not in (ci_text)
    )
    assert "tests/test_bench_harness.py" in proof_plan_text
    assert "tests/test_bench_tool.py" in proof_plan_text
    assert "tests/test_ci_workflow_topology.py" in proof_plan_text
    assert "tests/test_harness_conformance.py" in proof_plan_text
    assert "tests/test_harness_layers.py" in proof_plan_text
    assert "tests/test_monty_conformance_runner.py" in proof_plan_text
    assert "Setup canonical native linker SDK" in ci_text
    assert "proof-receipts/evidence/cargo-test-truth.json" in ci_text
    assert "proof-receipts/evidence/llvm-differential-truth.json" in ci_text
    assert "target/**/.molt_state/build_failures/*.json" not in ci_text
    llvm_upload = ci_text.split("- name: Upload LLVM/MLIR/linker receipt", 1)[1].split(
        "- name: Summarize guarded command hotspots", 1
    )[0]
    assert "include-hidden-files: true" in llvm_upload
    assert "uses: ./.github/actions/setup-llvm" in ci_text
    assert "sudo apt-get install -y lld" not in ci_text
    assert "timeout_seconds" in proof_plan_text
    # Four jobs summarize hotspots: docs-gates, python-tooling-smoke,
    # rust-build-unit-smoke, and the LLVM backend job.
    assert ci_text.count("Summarize guarded command hotspots") == 4
    assert ci_text.count("python3 tools/profile_hotspots.py --limit 20") == 4


@pytest.mark.parametrize(
    ("workflow", "job", "upload_name"),
    [
        (".github/workflows/ci.yml", "llvm-backend", "Upload LLVM/MLIR/linker receipt"),
        (".github/workflows/molt-wasm-ci.yml", "wasm-build", "Upload WASM receipt"),
    ],
)
def test_ci_control_evidence_uses_shared_path_command(workflow, job, upload_name):
    steps = yaml.safe_load(_read(workflow))["jobs"][job]["steps"]
    resolver = next(step for step in steps if step.get("id") == "build-control")
    assert resolver["uses"] == "./.github/actions/upload-proof-diagnostics"
    action = yaml.safe_load(
        _read(".github/actions/upload-proof-diagnostics/action.yml")
    )
    path_step = next(
        step for step in action["runs"]["steps"] if step.get("id") == "paths"
    )
    assert path_step["run"] == (
        'uv run --no-sync python -m tools.build_control_path >> "$GITHUB_OUTPUT"'
    )
    executor = next(step for step in steps if "--run-family" in step.get("run", ""))
    assert steps.index(executor) < steps.index(resolver)
    upload = next(step for step in steps if step.get("name") == upload_name)
    paths = upload["with"]["path"].splitlines()
    assert "${{ steps.build-control.outputs.root }}/build_failures/*.json" in paths
    assert not any(".molt_state" in path for path in paths)
    if job == "wasm-build":
        assert (
            "${{ steps.build-control.outputs.root }}/runtime_wasm_generations/*.json"
            in paths
        )
        assert (
            "${{ steps.build-control.outputs.proof_scratch_root != '' && format('{0}/differential/runtime-import/', steps.build-control.outputs.proof_scratch_root) || '' }}"
            in paths
        )
        assert "proof-receipts/evidence/import-from-codec/" not in paths
    assert steps.index(resolver) < steps.index(upload)


@pytest.mark.parametrize("family", ["rust", "llvm"])
def test_ci_command_diagnostics_survive_missing_final_receipts(
    family: str, tmp_path: Path
) -> None:
    jobs = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]
    job = jobs["rust-build-unit-smoke" if family == "rust" else "llvm-backend"]
    steps = job["steps"]
    diagnostics = [
        step
        for step in steps
        if step.get("with", {}).get("name") == f"proof-diagnostics-{family}"
    ]
    assert len(diagnostics) == 1
    upload = diagnostics[0]
    assert upload["uses"].startswith("actions/upload-artifact@")
    assert upload["with"]["include-hidden-files"] is True
    assert upload["with"]["if-no-files-found"] == "warn"
    # A killed phase may leave only its manifest and hidden partial log.
    # Final receipt publication must not gate collection of those bytes.
    assert upload["if"] == "always()"
    executor = next(
        step
        for step in steps
        if f"--run-family {family} --receipt" in step.get("run", "")
    )
    assert steps.index(executor) < steps.index(upload)
    assert "continue-on-error" not in executor
    strict_download = next(
        step
        for step in jobs["proof-plan-verdict"]["steps"]
        if step.get("uses", "").startswith("actions/download-artifact@")
    )
    assert not fnmatchcase(upload["with"]["name"], strict_download["with"]["pattern"])
    paths = upload["with"]["path"].splitlines()
    if family == "rust":
        resolver = next(step for step in steps if step.get("id") == "build-control")
        assert resolver["uses"] == "./.github/actions/upload-proof-diagnostics"
        assert resolver["if"] == "always()"
        assert steps.index(executor) < steps.index(resolver) < steps.index(upload)
        assert paths == [
            "proof-receipts/evidence/cargo-test-truth-runs/",
            "${{ steps.build-control.outputs.proof_scratch_root != '' && format('{0}/runtime-gate-runs/', steps.build-control.outputs.proof_scratch_root) || '' }}",
            "proof-receipts/evidence/runtime-extension-admission/",
            "${{ env.CARGO_TARGET_DIR != '' && format('{0}/**/molt-test-artifacts/**/*.log', env.CARGO_TARGET_DIR) || '' }}",
            "${{ env.CARGO_TARGET_DIR != '' && format('{0}/**/cargo-timings/cargo-timing-*.html', env.CARGO_TARGET_DIR) || '' }}",
        ]
        scratch = tmp_path / "current custody" / "tmp"
        run = scratch / "runtime-gate-runs" / "failed-runtime"
        expected = {
            run / name
            for name in (
                "aggregate.json",
                "child-0/.partial.stderr.log",
                "child-0/raw.stderr.log",
                "custody-cas/bytes/ab/cd",
            )
        }
        for path in expected:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"interrupted runtime witness\x00\xff")
        foreign = (
            tmp_path
            / "foreign custody/tmp/runtime-gate-runs/failed-runtime/aggregate.json"
        )
        foreign.parent.mkdir(parents=True)
        foreign.write_bytes(b"foreign runtime witness")
        stale = (
            tmp_path / "proof-receipts/evidence/runtime-gate-runs/old/aggregate.json"
        )
        stale.parent.mkdir(parents=True)
        stale.write_bytes(b"stale checkout witness")
        selected: set[Path] = set()
        for line in paths:
            rendered = _diagnostic_path_expression(
                line,
                {
                    "steps.build-control.outputs.proof_scratch_root": str(scratch),
                    "env.CARGO_TARGET_DIR": "",
                },
            )
            if not rendered:
                continue
            pattern = Path(rendered)
            if not pattern.is_absolute():
                pattern = tmp_path / pattern
            for match in glob.glob(str(pattern), recursive=True, include_hidden=True):
                path = Path(match)
                if path.is_dir():
                    selected.update(
                        child for child in path.rglob("*") if child.is_file()
                    )
                else:
                    selected.add(path)
        assert selected == expected
        assert foreign not in selected and stale not in selected
    else:
        assert job["env"]["MOLT_DEBUG_ARTIFACT_DIR"] == (
            "${{ github.workspace }}/proof-receipts/evidence/llvm-backend-debug"
        )
        assert paths == [
            "proof-receipts/llvm.json",
            "proof-receipts/evidence/llvm-differential-truth.json",
            "proof-receipts/evidence/llvm-backend-debug/native-batch-failures/",
            "proof-receipts/evidence/llvm-backend-debug/native-execution-failures/",
            "${{ steps.build-control.outputs.root != '' && format('{0}/backend_daemon/*.log', steps.build-control.outputs.root) || '' }}",
            "${{ steps.build-control.outputs.root != '' && format('{0}/backend_daemon/*.log.old', steps.build-control.outputs.root) || '' }}",
        ]
        # Exercise upload selection with no final llvm.json. Only the declared
        # invocation bundle is public; a sibling scratch file stays private.
        debug = tmp_path / "proof-receipts/evidence/llvm-backend-debug"
        bundle = debug / "native-execution-failures/invocation"
        bundle.mkdir(parents=True)
        expected = {
            bundle / name
            for name in ("source.py", "executable", "run.stdout", "manifest.json")
        }
        for path in expected:
            path.write_bytes(b"failure-witness\x00\xff")
        (debug / "private.txt").write_text("private", encoding="utf-8")
        selected: set[Path] = set()
        for line in paths:
            rendered = _diagnostic_path_expression(
                line, {"steps.build-control.outputs.root": ""}
            )
            if not rendered:
                continue
            for match in glob.glob(
                str(tmp_path / rendered), recursive=True, include_hidden=True
            ):
                path = Path(match)
                if path.is_dir():
                    selected.update(
                        child for child in path.rglob("*") if child.is_file()
                    )
                else:
                    selected.add(path)
        assert selected == expected
        assert all(path.read_bytes() == b"failure-witness\x00\xff" for path in selected)


def _diagnostic_path_expression(line: str, values: dict[str, str]) -> str:
    """Model only the upload's declared GitHub path-expression subset."""
    conditional = re.fullmatch(
        r"\$\{\{ ([\w.-]+) != '' && format\('([^']+)', \1\) \|\| '' \}\}",
        line,
    )
    if conditional:
        value = values[conditional[1]]
        return conditional[2].format(value) if value else ""
    return re.sub(r"\$\{\{ ([\w.-]+) \}\}", lambda match: values[match[1]], line)


@pytest.mark.parametrize(
    ("family", "os_name"),
    [("platform_portability", os_name) for os_name in ("linux", "macos", "windows")]
    + [("native_integration", "linux"), ("wasm", "linux")],
)
@pytest.mark.parametrize(
    "state",
    [
        "failed-with-receipt",
        "failed-without-receipt",
        "setup-no-output",
        "setup-no-root",
    ],
)
def test_ci_diagnostics_retain_current_custody_without_final_receipt(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    family: str,
    os_name: str,
    state: str,
) -> None:
    from molt import dx
    from tools.build_control_path import build_control_output
    from tests.test_dx_run_context import _github_actions_custody_env

    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    native = family == "native_integration"
    wasm = family == "wasm"
    detailed = native or wasm
    cell = ""
    if not detailed:
        cells = [
            cell
            for cell in plan["matrix_cell"]
            if cell["backend"] == "proof-queue" and cell["os"] == os_name
        ]
        assert len(cells) == 1
        cell = cells[0]["id"]
    jobs = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]
    job = (
        yaml.safe_load(_read(".github/workflows/molt-wasm-ci.yml"))["jobs"][
            "wasm-build"
        ]
        if wasm
        else jobs[family.replace("_", "-")]
    )
    if detailed:
        assert job["env"]["MOLT_GUARD_PROFILE"] == "all"
    suffix = (
        "wasm"
        if wasm
        else "native-integration"
        if native
        else "platform-portability-${{ matrix.cell }}"
    )
    steps = job["steps"]
    upload = next(
        step
        for step in steps
        if step.get("with", {}).get("name") == f"proof-diagnostics-{suffix}"
    )
    assert upload["if"] == "always()", "missing final receipts cannot gate diagnostics"
    assert upload["uses"].startswith("actions/upload-artifact@")
    assert upload["with"]["include-hidden-files"] is True
    assert upload["with"]["if-no-files-found"] == "warn"
    resolver = next(step for step in steps if step.get("id") == "build-control")
    assert resolver["uses"] == "./.github/actions/upload-proof-diagnostics"
    assert resolver["if"] == "always()"
    action = yaml.safe_load(
        _read(".github/actions/upload-proof-diagnostics/action.yml")
    )
    path_step, common_upload = action["runs"]["steps"]
    assert path_step["shell"] == "bash"
    assert path_step["run"] == (
        'uv run --no-sync python -m tools.build_control_path >> "$GITHUB_OUTPUT"'
    )
    assert common_upload["with"]["include-hidden-files"] is True
    assert common_upload["with"]["if-no-files-found"] == "warn"
    executor = next(
        step for step in steps if f"--run-family {family}" in step.get("run", "")
    )
    setup = next(
        step for step in steps if step.get("uses") == "./.github/actions/setup-project"
    )
    assert (
        steps.index(setup)
        < steps.index(executor)
        < steps.index(resolver)
        < steps.index(upload)
    )
    assert "continue-on-error" not in executor
    receipt_upload = next(
        step
        for step in steps
        if step.get("with", {}).get("name") == f"proof-receipt-{suffix}"
    )
    assert receipt_upload["if"] == (
        f"always() && hashFiles('proof-receipts/{family}.json') != ''"
        if detailed
        else "always() && hashFiles(format('proof-receipts/platform-portability-{0}.json', matrix.cell)) != ''"
    )
    assert receipt_upload["with"]["if-no-files-found"] == "error"
    strict_download = next(
        step
        for step in jobs["proof-plan-verdict"]["steps"]
        if step.get("uses", "").startswith("actions/download-artifact@")
    )

    workspace = tmp_path / "runner-work" / "molt" / "molt"
    workspace.mkdir(parents=True)
    runner_temp = tmp_path / "runner-temp"
    environment = _github_actions_custody_env(workspace, runner_temp)
    custody = runner_temp / "mp-12345-2"
    environment[dx.GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV] = str(custody)
    environment["GITHUB_JOB"] = "wasm-build" if wasm else family.replace("_", "-")
    # Only this synthetic workspace has synthetic Git identity. Concurrent
    # custody observers retain real checkout observation for their own roots.
    checkout_head = dx.git_checkout_head
    monkeypatch.setattr(
        dx,
        "git_checkout_head",
        lambda root: (
            environment["GITHUB_SHA"] if root == workspace else checkout_head(root)
        ),
    )
    outputs = dict(
        line.split("=", 1)
        for line in build_control_output(environment, repo_root=workspace).splitlines()
    )
    profile = custody / "tmp" / "harness_memory_guard" / "commands.jsonl"
    pytest_dir = custody / "tmp" / "pytest-memory-guard"
    metadata = runner_temp / "mr-12345-2"
    assert outputs["profile_log"] == str(profile)
    assert outputs["incident_root"] == str(profile.parent / "incidents")
    assert outputs["guard_state_root"] == str(custody / "tmp" / "memory_guard")
    assert outputs["pytest_guard_root"] == str(pytest_dir)
    assert outputs["proof_scratch_root"] == str(
        scratch_state_root(custody / "tmp", owner=workspace, create=False)
    )
    values = {
        "matrix.cell": cell,
        "env.CARGO_TARGET_DIR": str(custody / "target")
        if state.startswith("failed-")
        else "",
        "env.MOLT_CI_EPHEMERAL_CUSTODY_ROOT": ""
        if state == "setup-no-root"
        else str(custody),
        "env.MOLT_PROOF_TEST_METADATA_ROOT": ""
        if state == "setup-no-root"
        else str(metadata),
        **{
            f"steps.build-control.outputs.{name}": value
            if state.startswith("failed-")
            else ""
            for name, value in outputs.items()
        },
        **{
            f"steps.paths.outputs.{name}": value if state.startswith("failed-") else ""
            for name, value in outputs.items()
        },
    }
    name = _diagnostic_path_expression(upload["with"]["name"], values)
    assert not fnmatchcase(name, strict_download["with"]["pattern"])
    specialized_paths = upload["with"]["path"].splitlines()
    common_profiles = common_upload["with"]["path"].splitlines()
    paths = [*specialized_paths, *common_profiles]
    assert specialized_paths == (
        [
            f"proof-receipts/{family}.json",
            *(
                [
                    "${{ steps.build-control.outputs.proof_scratch_root != '' && format('{0}/differential/runtime-import/', steps.build-control.outputs.proof_scratch_root) || '' }}",
                    "proof-receipts/evidence/split-runtime/",
                ]
                if wasm
                else []
            ),
        ]
        if detailed
        else [
            "proof-receipts/platform-portability-${{ matrix.cell }}.json",
            "${{ steps.build-control.outputs.proof_scratch_root != '' && format('{0}/runtime-gate-runs/', steps.build-control.outputs.proof_scratch_root) || '' }}",
            "${{ env.CARGO_TARGET_DIR != '' && format('{0}/**/molt-test-artifacts/**/*.log', env.CARGO_TARGET_DIR) || '' }}",
            "${{ env.MOLT_PROOF_TEST_METADATA_ROOT != '' && format('{0}/**/metadata-owner.json', env.MOLT_PROOF_TEST_METADATA_ROOT) || '' }}",
            "${{ env.MOLT_PROOF_TEST_METADATA_ROOT != '' && format('{0}/**/*.supervisor-receipt.json', env.MOLT_PROOF_TEST_METADATA_ROOT) || '' }}",
            "${{ env.MOLT_PROOF_TEST_METADATA_ROOT != '' && format('{0}/**/*.supervisor-receipt.json.[0-9]*.[0-9]*.tmp', env.MOLT_PROOF_TEST_METADATA_ROOT) || '' }}",
            "${{ env.MOLT_PROOF_TEST_METADATA_ROOT != '' && format('{0}/**/*.supervisor-receipt.json.events.*.tmp', env.MOLT_PROOF_TEST_METADATA_ROOT) || '' }}",
            "${{ env.MOLT_PROOF_TEST_METADATA_ROOT != '' && format('{0}/**/*.supervisor-receipt.json.events.*.jsonl', env.MOLT_PROOF_TEST_METADATA_ROOT) || '' }}",
        ]
    )
    rendered = [_diagnostic_path_expression(line, values) for line in paths]
    if not state.startswith("failed-"):
        assert all(
            not _diagnostic_path_expression(line, values) for line in common_profiles
        )
        if detailed or state == "setup-no-root":
            assert not any(
                _diagnostic_path_expression(line, values)
                for line in paths
                if line.startswith("${{")
            )

    expected: set[Path] = set()
    inner = {
        "prefix": "MOLT_PERF_CALIBRATION",
        "returncode": 125,
        "temporary_artifacts": {"closure": {"closed": False}},
        "termination_reports": [{"reason": "tracked_orphan_cleanup"}],
    }
    retained_files = [
        profile,
        profile.with_name(profile.name + ".1"),
        profile.parent / "incidents" / "timeout-owner.json",
    ]
    retained_files += [
        custody / "tmp/memory_guard/commands/current" / name
        for name in ("custody.json", "startup.json", "guard.json")
    ]
    retained_files += [
        pytest_dir / name
        for name in (
            "pytest-1_outer-guard.json",
            "test-custody-2.json",
            ".workers/gw0_current-test.json",
        )
    ]
    proof_scratch = scratch_state_root(custody / "tmp", owner=workspace, create=False)
    if detailed:
        if wasm:
            retained_files += [
                proof_scratch / "differential/runtime-import" / name
                for name in ("results.jsonl", "summary.json", "output.log")
            ]
            retained_files += [
                workspace / "proof-receipts/evidence/split-runtime" / name
                for name in (
                    "files/sha256/data/ab/cd/molt_runtime.wasm",
                    "blobs/sha256/ef/failure.json.gz",
                )
            ]
    else:
        retained_files += [
            proof_scratch / "runtime-gate-runs/run/aggregate.json",
            proof_scratch / "runtime-gate-runs/run/child-0/raw.stderr.log",
            custody / "target/release-output/deps/molt-test-artifacts/run/stderr.log",
            metadata / "case/metadata-owner.json",
            metadata / "case/execution.supervisor-receipt.json",
            metadata / "case/execution.supervisor-receipt.json.123.0.tmp",
            metadata / "case/execution.supervisor-receipt.json.events.123.0.tmp",
            metadata / "case/execution.supervisor-receipt.json.events.abcdef.jsonl",
        ]
    for retained in retained_files:
        # Same filenames under a foreign run must never be swept into upload.
        owner = (
            custody
            if retained.is_relative_to(custody)
            else metadata
            if retained.is_relative_to(metadata)
            else workspace
        )
        foreign = runner_temp / "mp-99999-1" / retained.relative_to(owner)
        foreign.parent.mkdir(parents=True, exist_ok=True)
        foreign.write_text('{"foreign":true}', encoding="utf-8")
        if state.startswith("failed-"):
            retained.parent.mkdir(parents=True, exist_ok=True)
            retained.write_text(json.dumps(inner) + "\n", encoding="utf-8")
            expected.add(retained)
    if state.startswith("failed-"):
        (pytest_dir / "unrelated.txt").write_text("not guard JSON", encoding="utf-8")
        if not detailed:
            # A raw policy/result/CAS may contain command or environment secrets.
            # Retain native receipts (including interrupted atomic publication),
            # journals and fixture ownership; never raw policy or workload data.
            for name in (
                "execution.supervisor-policy.json",
                "execution.supervisor-policy.json.123.0.tmp",
                "execution.supervisor-receipt.json.native-environment.tmp",
                "execution.native-environment.json",
                "execution.native-environment.json.123.0.tmp",
                "execution.json",
                "execution.request.json",
                "execution.command.stdout.log",
                "execution.command.stderr.log",
                "execution.stdout.bin",
                "execution.stderr.bin",
                "custody-cas/private.json",
            ):
                secret = metadata / "case" / name
                secret.parent.mkdir(parents=True, exist_ok=True)
                secret.write_text("fixture-secret-must-not-upload", encoding="utf-8")
    if state == "failed-with-receipt":
        receipt = workspace / rendered[0]
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_text('{"status":"failed"}', encoding="utf-8")
        expected.add(receipt)
    retained_paths: set[Path] = set()
    for path in filter(None, rendered):
        candidate = Path(path)
        if not candidate.is_absolute():
            candidate = workspace / candidate
        assert (
            candidate.is_relative_to(custody)
            or (not detailed and candidate.is_relative_to(metadata))
            or candidate == workspace / rendered[0]
            or (
                wasm
                and candidate
                in {
                    workspace / "proof-receipts/evidence/split-runtime",
                }
            )
        )
        for found in glob.glob(str(candidate), recursive=True, include_hidden=True):
            path = Path(found)
            if path.is_dir():
                retained_paths.update(
                    child for child in path.rglob("*") if child.is_file()
                )
            else:
                retained_paths.add(path)
    assert retained_paths == expected
    if state.startswith("failed-"):
        assert json.loads(profile.read_text(encoding="utf-8")) == inner
        assert profile.with_name(profile.name + ".1") in retained_paths


def test_every_proof_execution_exports_common_diagnostics_after_execution() -> None:
    action = yaml.safe_load(
        _read(".github/actions/upload-proof-diagnostics/action.yml")
    )
    resolver, upload = action["runs"]["steps"]
    assert resolver["id"] == "paths"
    assert resolver["shell"] == "bash"
    assert resolver["env"] == {"PYTHONPATH": "${{ github.workspace }}/src"}
    assert resolver["run"] == (
        'uv run --no-sync python -m tools.build_control_path >> "$GITHUB_OUTPUT"'
    )
    assert upload["if"] == "always() && steps.paths.outcome == 'success'"
    assert upload["uses"] == (
        "actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9"
    )
    assert upload["with"]["name"] == "${{ inputs.name }}-${{ github.run_attempt }}"
    assert upload["with"]["include-hidden-files"] is True
    assert upload["with"]["if-no-files-found"] == "warn"
    assert upload["with"]["path"].splitlines() == [
        "${{ steps.paths.outputs.profile_log }}",
        "${{ steps.paths.outputs.profile_log != '' && format('{0}.1', steps.paths.outputs.profile_log) || '' }}",
        "${{ steps.paths.outputs.incident_root != '' && format('{0}/*.json', steps.paths.outputs.incident_root) || '' }}",
        "${{ steps.paths.outputs.guard_state_root != '' && format('{0}/commands/**/*.json', steps.paths.outputs.guard_state_root) || '' }}",
        "${{ steps.paths.outputs.pytest_guard_root != '' && format('{0}/**/*.json', steps.paths.outputs.pytest_guard_root) || '' }}",
    ]
    seen = set()
    for workflow in (
        ".github/workflows/ci.yml",
        ".github/workflows/molt-wasm-ci.yml",
        ".github/workflows/security_hardening.yml",
        ".github/workflows/formal.yml",
    ):
        jobs = yaml.safe_load(_read(workflow))["jobs"]
        for job_name, job in jobs.items():
            steps = job.get("steps", [])
            executions = [
                step
                for step in steps
                if "tools/proof_plan.py" in step.get("run", "")
                and re.search(r"--run-(?:family|command)\b", step["run"])
            ]
            if not executions:
                continue
            exports = [
                step
                for step in steps
                if step.get("uses") == "./.github/actions/upload-proof-diagnostics"
            ]
            assert len(executions) == len(exports) == 1, (workflow, job_name)
            export = exports[0]
            assert export["if"] == "always()"
            assert export["id"] == "build-control"
            assert steps.index(export) > steps.index(executions[0])
            assert not any(
                "tools.build_control_path" in step.get("run", "") for step in steps
            )
            assert "continue-on-error" not in executions[0]
            name = export["with"]["name"]
            assert name.startswith("proof-diagnostics-")
            assert not fnmatchcase(name, "proof-receipt-*")
            assert name not in seen
            seen.add(name)
            if "${{ matrix.cell }}" in executions[0]["run"]:
                assert "${{ matrix.cell }}" in name
    assert len(seen) == 15


@pytest.mark.usefixtures("no_ambient_guard_caps", "session_sentinel_paused")
def test_common_diagnostics_include_real_timeout_incident_after_passing_neighbor(
    tmp_path: Path,
) -> None:
    import sys

    from tools import harness_memory_guard
    from tools.build_control_path import build_control_output
    from tests.process_guard_common import HUNG_CHILD_SOURCE

    profile = tmp_path / "selected" / "commands.jsonl"
    profile.parent.mkdir()
    # A full previous log exercises the existing rotation authority; the named
    # incident must remain independently selectable after later passing work.
    profile.write_text('{"previous":true}\n' * 100, encoding="utf-8")
    environment = {
        **os.environ,
        "MOLT_GUARD_PROFILE_LOG": str(profile),
        "MOLT_GUARD_PROFILE": "incident",
        "MOLT_GUARD_PROFILE_MAX_MB": "0.001",
        "PYTEST_CURRENT_TEST": "fixture/owner.py::waiting_owner (call)",
    }
    result = harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", HUNG_CHILD_SOURCE],
        prefix="MOLT_TEST_DIAGNOSTIC_EXPORT",
        env=environment,
        timeout=2.0,
    )
    assert result.returncode == 124 and result.timed_out
    assert result.descendants_closed is True, result.stderr
    incident_dir = tmp_path / "selected" / "incidents"
    (incident,) = incident_dir.glob("*.json")
    original = incident.read_bytes()
    record = json.loads(original)
    assert record["status"] == "timeout"
    assert record["repro"]["pytest"]["current_test"] == (
        "fixture/owner.py::waiting_owner (call)"
    )
    neighbor = harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", "pass"],
        prefix="MOLT_TEST_DIAGNOSTIC_EXPORT",
        env={
            **environment,
            "PYTEST_CURRENT_TEST": "fixture/neighbor.py::passing (call)",
        },
        timeout=10.0,
    )
    assert neighbor.returncode == 0 and neighbor.descendants_closed is True
    assert list(incident_dir.glob("*.json")) == [incident]
    assert incident.read_bytes() == original

    foreign = tmp_path / "foreign" / "incidents" / incident.name
    foreign.parent.mkdir(parents=True)
    foreign.write_text('{"foreign":true}', encoding="utf-8")
    # A payload or a policy alongside diagnostics is not a declared artifact.
    private = profile.parent / "private-payload.bin"
    private.write_bytes(b"not a diagnostic")
    outputs = dict(
        line.split("=", 1)
        for line in build_control_output(environment, repo_root=REPO_ROOT).splitlines()
    )
    assert outputs["incident_root"] == str(incident_dir)
    values = {f"steps.paths.outputs.{key}": value for key, value in outputs.items()}
    action = yaml.safe_load(
        _read(".github/actions/upload-proof-diagnostics/action.yml")
    )
    upload = action["runs"]["steps"][1]
    selected = {
        Path(found)
        for line in upload["with"]["path"].splitlines()
        for pattern in [_diagnostic_path_expression(line, values)]
        if pattern
        for found in glob.glob(pattern, recursive=True, include_hidden=True)
    }
    assert {incident, profile, profile.with_name("commands.jsonl.1")} <= selected
    assert foreign not in selected and private not in selected
    assert json.loads(incident.read_bytes())["repro"]["pytest"]["current_test"] == (
        "fixture/owner.py::waiting_owner (call)"
    )


def test_ci_heavy_jobs_are_path_classified() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert 'python3 tools/proof_plan.py --github-output "$GITHUB_OUTPUT"' in (ci_text)
    assert "python_static: ${{ steps.paths.outputs.python_static }}" in ci_text
    assert "python_unit: ${{ steps.paths.outputs.python_unit }}" in ci_text
    assert (
        "native_integration: ${{ steps.paths.outputs.native_integration }}" in ci_text
    )
    assert "rust: ${{ steps.paths.outputs.rust }}" in ci_text
    assert "llvm: ${{ steps.paths.outputs.llvm }}" in ci_text
    assert "python_security: ${{ steps.paths.outputs.python_security }}" in ci_text
    assert "rust_security: ${{ steps.paths.outputs.rust_security }}" in ci_text
    assert (
        "platform_portability: ${{ steps.paths.outputs.platform_portability }}"
        in ci_text
    )
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    for family in plan["ci_family"]:
        if family["executor"] == "github-matrix":
            output = f"{family['name']}_matrix"
            assert f"{output}: ${{{{ steps.paths.outputs.{output} }}}}" in ci_text
    assert "steps.paths.outputs.matrix }}" not in ci_text
    assert "topology: ${{ steps.paths.outputs.topology }}" in ci_text
    assert "selected: ${{ steps.paths.outputs.selected }}" in ci_text
    assert ci_text.count("needs: classify-changes") >= 4
    assert "proof-plan-verdict:" in ci_text
    assert "name: Proof Plan Verdict" in ci_text
    assert (
        "--verify-selected '${{ needs.classify-changes.outputs.selected }}'" in ci_text
    )
    assert "--receipt-dir proof-receipts" in ci_text
    assert "uses: actions/download-artifact@" in ci_text
    assert "== 'success' && 1 || 0" not in ci_text
    # Full history exactly where a job reads it: the path classifier diffs
    # against the event base, and docs-gates runs the commit-attribution
    # policy over every commit the event introduces.
    jobs = yaml.safe_load(ci_text)["jobs"]
    full_history = {
        name
        for name, job in jobs.items()
        for step in job.get("steps", [])
        if step.get("uses", "").startswith("actions/checkout@")
        and step.get("with", {}).get("fetch-depth") == 0
    }
    assert full_history == {"classify-changes", "docs-gates"}
    assert ci_text.count("fetch-depth: 0") == 2
    assert "--depth=1" not in ci_text


def test_ci_proof_families_are_admitted_independently() -> None:
    """A selected sibling failure must never mask another proof family."""

    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    ci_jobs = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]
    families = plan["ci_family"]

    assert all(family["dependencies"] == [] for family in families)
    admission_jobs = {family["admission_job"] for family in families}
    assert admission_jobs == {
        "browser-simd",
        "docs-gates",
        "formal-verification",
        "llvm-backend",
        "native-integration",
        "platform-portability",
        "python-static",
        "python-unit",
        "python-unit-collection",
        "rust-build-unit-smoke",
        "security-hardening",
        "wasm-validation",
    }

    for family in families:
        assert family["admission_workflow"] == ".github/workflows/ci.yml"
        job_name = family["admission_job"]
        assert _literal_job_needs(ci_jobs[job_name]) == tuple(family["admission_needs"])
        assert "continue-on-error" not in ci_jobs[job_name]

    assert _literal_job_needs(ci_jobs["docs-gates"]) == ()
    for job_name in admission_jobs - {"docs-gates"}:
        assert _literal_job_needs(ci_jobs[job_name]) == ("classify-changes",)
        condition = str(ci_jobs[job_name].get("if", ""))
        assert ".result" not in condition

    # Simulate every family admission failing in turn. Every other selected
    # admission still has all of its own prerequisites satisfied because no
    # proof admission consumes a sibling result.
    for failed_job in admission_jobs:
        statuses = {"classify-changes": "success", failed_job: "failure"}
        for candidate_job in admission_jobs - {failed_job}:
            assert all(
                statuses.get(dependency) == "success"
                for dependency in _literal_job_needs(ci_jobs[candidate_job])
            )

    verdict_needs = _literal_job_needs(ci_jobs["proof-plan-verdict"])
    assert set(verdict_needs) == {"classify-changes", *admission_jobs}
    assert len(verdict_needs) == len(set(verdict_needs))
    assert ci_jobs["proof-plan-verdict"]["if"] == "always()"


def test_proof_plan_validation_rejects_cross_family_admission_masking(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools import proof_plan

    plan = proof_plan.ProofPlan.load()
    original = proof_plan._workflow_job_needs

    def masked_needs(block: str) -> tuple[str, ...]:
        if block.startswith("  wasm-validation:"):
            return ("classify-changes", "rust-build-unit-smoke")
        return original(block)

    monkeypatch.setattr(proof_plan, "_workflow_job_needs", masked_needs)
    validation_errors = plan.validate()
    assert any(
        "wasm: admission job 'wasm-validation' needs " in error
        and "rust-build-unit-smoke" in error
        for error in validation_errors
    )


def test_llvm_ci_resolves_toolchain_from_manifest_authority() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    perf_text = _read(".github/workflows/perf-gate.yml")
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    action_text = _read(".github/actions/setup-llvm/action.yml")
    action = yaml.safe_load(action_text)
    action_steps = action["runs"]["steps"]
    steps = {step["name"]: step for step in action_steps}

    assert "uses: ./.github/actions/setup-llvm" in ci_text
    assert "uses: ./.github/actions/setup-llvm" in perf_text
    assert "PYTHONPATH=src python -m molt.llvm_toolchain" in action_text
    assert "python3" not in action_text
    assert '--github-output "$GITHUB_OUTPUT"' in action_text
    assert '--github-env "$GITHUB_ENV"' in action_text
    assert "steps.contract.outputs.apt_packages" in action_text
    assert "steps.contract.outputs.apt_installer_url" in action_text
    assert "steps.contract.outputs.apt_installer_sha256" in action_text
    assert 'installer="$RUNNER_TEMP/molt-llvm-apt.sh"' in action_text
    assert "sha256sum --check --strict" in action_text
    assert "wget -qO /tmp" not in action_text
    assert "the wasm SDK profile requires wasi=true" in action_text
    # Host packages serve only the Linux full SDK. Every WebAssembly tool comes
    # from the one manifest-owned wasi-sdk provisioner on every host.
    apt_step = steps["Provision full LLVM SDK packages"]
    assert apt_step["if"] == "inputs.profile == 'full' && runner.os == 'Linux'"
    source_step = steps["Provision full LLVM SDK from pinned source"]
    assert source_step["if"] == "inputs.profile == 'full' && runner.os != 'Linux'"
    assert "python -m tools.bootstrap_llvm" in source_step["run"]
    assert "curl" not in source_step["run"]
    assert sum("apt-get" in str(step.get("run", "")) for step in action_steps) == 1
    assert "lld-$LLVM_MAJOR" not in action_text
    cache = steps["Cache verified wasi-sdk archive"]
    assert cache["if"] == "inputs.wasi == 'true' && github.ref == 'refs/heads/main'"
    assert re.fullmatch(r"actions/cache@[0-9a-f]{40}", cache["uses"])
    assert cache["with"] == {
        "path": "${{ runner.temp }}/molt-wasi-sdk-downloads",
        "key": "${{ steps.contract.outputs.wasi_sdk_cache_key }}",
    }
    restore = steps["Restore verified wasi-sdk archive"]
    assert restore["if"] == "inputs.wasi == 'true' && github.ref != 'refs/heads/main'"
    assert re.fullmatch(r"actions/cache/restore@[0-9a-f]{40}", restore["uses"])
    assert restore["with"] == cache["with"]
    provision = steps["Provision pinned host wasi-sdk"]
    assert provision["if"] == "inputs.wasi == 'true'"
    assert provision["id"] == "wasi-sdk"
    assert "PYTHONPATH=src python -m tools.provision_wasi_sdk" in provision["run"]
    assert '--downloads "$RUNNER_TEMP/molt-wasi-sdk-downloads"' in provision["run"]
    assert '--github-output "$GITHUB_OUTPUT"' in provision["run"]
    for shell_install in ("curl", "tar ", "sha256sum", "stat "):
        assert shell_install not in provision["run"]
    verify_wasm = steps["Verify and project WebAssembly SDK identity"]
    assert verify_wasm["env"] == {
        "WASI_SDK_INSTALL": "${{ steps.wasi-sdk.outputs.install }}"
    }
    assert "--verify-wasm" in verify_wasm["run"]
    assert '--wasi-sdk "$WASI_SDK_INSTALL"' in verify_wasm["run"]
    assert "--verify" in steps["Verify and project full SDK identity"]["run"]
    step_names = list(steps)
    full_verify = step_names.index("Verify and project full SDK identity")
    assert full_verify < step_names.index("Verify and project WebAssembly SDK identity")
    assert action["outputs"]["wasi_sdk"]["value"] == (
        "${{ steps.wasi-sdk.outputs.install }}"
    )
    assert action["outputs"]["wasi_sysroot"]["value"] == (
        "${{ steps.wasi-sdk.outputs.sysroot }}"
    )
    assert "--wasi-sysroot" not in action_text
    assert "wasi_sysroot_url" not in action_text
    # These command families declare both native ld.lld and SDK wasm-ld. The
    # WebAssembly-only SDK must never stand in for the full native LLVM SDK.
    jobs = yaml.safe_load(ci_text)["jobs"]
    for job in ("rust-build-unit-smoke", "native-integration"):
        sdk_steps = [
            step
            for step in jobs[job]["steps"]
            if step.get("uses") == "./.github/actions/setup-llvm"
        ]
        assert len(sdk_steps) == 1
        assert sdk_steps[0]["with"] == {"profile": "full", "wasi": "true"}
    wasm_steps = yaml.safe_load(wasm_text)["jobs"]["wasm-build"]["steps"]
    llvm_steps = [
        step
        for step in wasm_steps
        if step.get("uses") == "./.github/actions/setup-llvm"
    ]
    assert len(llvm_steps) == 1
    assert llvm_steps[0]["with"] == {"profile": "full", "wasi": "true"}
    assert all("wasi-libc" not in str(step.get("run", "")) for step in wasm_steps)
    release_steps = yaml.safe_load(_read(".github/workflows/release.yml"))["jobs"][
        "candidates"
    ]["steps"]
    release_llvm = [
        index
        for index, step in enumerate(release_steps)
        if step.get("uses") == "./.github/actions/setup-llvm"
    ]
    assert [release_steps[index]["with"] for index in release_llvm] == [
        {"profile": "full", "wasi": "true"}
    ]
    identity_index = next(
        index
        for index, step in enumerate(release_steps)
        if step.get("name") == "Verify candidate toolchain identity"
    )
    assert release_llvm[0] < identity_index
    semantic = yaml.safe_load(_read(".github/workflows/verified-subset.yml"))["jobs"]
    semantic_sdk = [
        step
        for job in semantic.values()
        for step in job.get("steps", [])
        if step.get("uses") == "./.github/actions/setup-llvm"
    ]
    assert {(step["if"], step["with"]["profile"]) for step in semantic_sdk} == {
        ("matrix.backend == 'llvm'", "full"),
        ("matrix.backend == 'wasm'", "wasm"),
    }
    assert '"$MOLT_WASM_LD" --version' in release_steps[identity_index]["run"]
    assert "grep -oE" not in ci_text
    assert "grep -oE" not in perf_text
    assert "LLVM_SYS_${MAJOR}1_PREFIX" not in ci_text
    assert "LLVM_SYS_${MAJOR}1_PREFIX" not in perf_text


def test_wasm_ci_provisions_pinned_optimizer_before_linked_partitions() -> None:
    workflow = yaml.safe_load(_read(".github/workflows/molt-wasm-ci.yml"))
    steps = workflow["jobs"]["wasm-build"]["steps"]
    optimizer_steps = [
        (index, step)
        for index, step in enumerate(steps)
        if step.get("uses") == "./.github/actions/setup-binaryen"
    ]
    assert len(optimizer_steps) == 1
    optimizer_index, optimizer = optimizer_steps[0]
    assert optimizer["id"] == "binaryen"
    partition_index, partition = next(
        (index, step)
        for index, step in enumerate(steps)
        if "--run-family wasm" in step.get("run", "")
    )
    assert optimizer_index < partition_index
    assert partition["env"]["MOLT_WASM_OPT"] == (
        "${{ steps.binaryen.outputs.wasm_opt }}"
    )
    action = yaml.safe_load(_read(".github/actions/setup-binaryen/action.yml"))
    assert action["outputs"]["wasm_opt"]["value"] == (
        "${{ steps.binaryen.outputs.wasm_opt }}"
    )
    provision = next(
        step for step in action["runs"]["steps"] if step.get("id") == "binaryen"
    )
    assert "python -m tools.provision_binaryen" in provision["run"]
    assert '--github-output "$GITHUB_OUTPUT"' in provision["run"]


def test_pr_trust_labeler_is_advisory_not_authoritative() -> None:
    labeler_text = _read(".github/workflows/pr_trust_labeler.yml")
    gate_text = _read(".github/workflows/pr_trust_gate.yml")

    assert "Trust gate remains authoritative" in labeler_text
    assert "error?.status === 403" in labeler_text
    assert "core.warning" in labeler_text
    assert "github.rest.issues.addLabels" in labeler_text
    assert "core.setFailed" not in labeler_text
    assert "core.setFailed" in gate_text


def test_kani_incompatibility_is_an_explicit_advisory_receipt_not_a_proof() -> None:
    kani_text = _read(".github/workflows/kani.yml")

    assert "name: Kani Advisory Compatibility Probe" in kani_text
    assert "name: Kani advisory compatibility probe" in kani_text
    assert '"schema": "molt.kani-compatibility.v1"' in kani_text
    assert '"authoritative": False' in kani_text
    assert '"required": False' in kani_text
    assert '"proofs_executed": 0' in kani_text
    assert '"status": "compatible" if compatible else "unavailable"' in kani_text
    assert "this advisory probe does not claim bounded verification" in kani_text
    assert "cargo kani --tests" not in kani_text
    assert not any(
        "bounded verification" in line.lower()
        for line in kani_text.splitlines()
        if line.lstrip().startswith("name:")
    )


def test_kani_workflow_is_scheduled_manual_and_standalone() -> None:
    kani_text = _read(".github/workflows/kani.yml")

    assert "workflow_dispatch:" in kani_text
    assert "schedule:" in kani_text
    assert "push:" not in kani_text
    assert "pull_request:" not in kani_text
    assert "classify-changes:" not in kani_text
    assert "proof_plan.py" not in kani_text


def test_kani_workflow_gates_verifier_rust_version_honestly() -> None:
    kani_text = _read(".github/workflows/kani.yml")

    assert "Check Kani toolchain compatibility" in kani_text
    assert "id: kani-toolchain" not in kani_text
    assert 'workspace_manifest["workspace"]["package"]["rust-version"]' in kani_text
    assert 'runtime" / "molt-obj-model" / "Cargo.toml"' in kani_text
    assert 'runtime" / "molt-runtime" / "Cargo.toml"' in kani_text
    assert "compatible = version_key(kani_rustc) >= version_key(required)" in kani_text
    assert "this advisory probe does not claim bounded verification" in kani_text
    assert "outputs.compatible" not in kani_text
    assert "GITHUB_OUTPUT" not in kani_text
    assert "Report skipped Kani proofs" not in kani_text
    assert "molt.kani-compatibility.v1" in kani_text
    assert "proofs_executed" in kani_text
    assert "if-no-files-found: error" in kani_text
    assert "--ignore-rust-version" not in kani_text


def test_github_workflows_opt_into_node24_action_runtime() -> None:
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        if "uses:" not in text:
            continue

        assert 'FORCE_JAVASCRIPT_ACTIONS_TO_NODE24: "true"' in text, workflow
        assert "ACTIONS_ALLOW_USE_UNSECURE_NODE_VERSION" not in text, workflow


def test_github_workflows_do_not_reintroduce_node20_action_pins() -> None:
    node20_action_pins = {
        "actions/checkout@v4",
        "actions/checkout@v5",
        "actions/setup-python@v5",
        "actions/setup-node@v4",
        "actions/cache@v4",
        "actions/upload-artifact@v4",
        "actions/download-artifact@v4",
        "actions/github-script@v7",
        "actions/attest-build-provenance@v2",
        "astral-sh/setup-uv@v3",
        "astral-sh/setup-uv@v4",
        "astral-sh/setup-uv@v7",
        "astral-sh/setup-uv@v8.1.0",
        "softprops/action-gh-release@v2",
    }

    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        for action_pin in sorted(node20_action_pins):
            assert action_pin not in text, (workflow, action_pin)


def test_github_workflows_pin_every_external_action_to_full_sha() -> None:
    action_files = [
        *WORKFLOW_ROOT.glob("*.yml"),
        *REPO_ROOT.glob(".github/actions/*/action.yml"),
    ]
    uses_pattern = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)", re.MULTILINE)
    # owner/repo[/path]@sha: subpath actions such as actions/cache/restore.
    sha_pattern = re.compile(r"^[^/@]+/[^/@]+(?:/[^/@]+)*@[0-9a-f]{40}$", re.IGNORECASE)
    found = 0
    for workflow in sorted(action_files):
        text = workflow.read_text(encoding="utf-8")
        for target in uses_pattern.findall(text):
            if target.startswith("./") or target.startswith("docker://"):
                continue
            found += 1
            assert sha_pattern.fullmatch(target), (workflow, target)
    assert found > 0


def test_github_actions_pin_one_exact_release_per_action() -> None:
    """Each external action resolves to one commit and one exact release tag.

    Dependabot rewrites every ``uses:`` line of an action together; a second
    digest or a floating ``# v7`` comment means a partial or hand-made bump.
    """
    action_files = [
        *WORKFLOW_ROOT.glob("*.yml"),
        *REPO_ROOT.glob(".github/actions/*/action.yml"),
    ]
    uses_pattern = re.compile(
        r"^\s*(?:-\s*)?uses:\s*([^\s#@]+)@([0-9a-f]{40})(?:\s*#\s*(\S+))?\s*$",
        re.MULTILINE,
    )
    release = re.compile(r"^v\d+\.\d+\.\d+$")
    pins: dict[str, set[tuple[str, str | None]]] = {}
    for path in sorted(action_files):
        for action, sha, comment in uses_pattern.findall(path.read_text("utf-8")):
            repository = "/".join(action.split("/")[:2])
            pins.setdefault(repository, set()).add((sha, comment or None))
            assert comment and release.fullmatch(comment), (path, action, comment)
    assert pins
    divergent = {repo: sorted(found) for repo, found in pins.items() if len(found) != 1}
    assert divergent == {}


def test_setup_uv_installs_the_manifest_pinned_uv() -> None:
    policy = {
        entry["name"]: entry
        for entry in tomllib.loads(_read("tools/proof_plan.toml"))["toolchain_policy"]
    }
    setup_project = _read(".github/actions/setup-project/action.yml")
    assert setup_project.count("astral-sh/setup-uv@") == 1
    release = tool_releases.tool_release("uv", REPO_ROOT)
    assert policy["uv"]["projection"] == "tool-release"
    assert policy["uv"]["setup_value"] == release.version
    assert "version: ${{ steps.inputs.outputs.uv-version }}" in setup_project
    normalizer = _read(".github/actions/setup-project/normalize-inputs.sh")
    assert "config/tool_releases.toml [tools.uv] version" in normalizer
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        assert "astral-sh/setup-uv@" not in workflow.read_text("utf-8"), workflow


def test_executable_receipt_root_is_git_ignored() -> None:
    ignored = {
        line.strip()
        for line in _read(".gitignore").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    }
    assert "proof-receipts/" in ignored
    for relative in (
        ".github/workflows/ci.yml",
        ".github/workflows/formal.yml",
        ".github/workflows/molt-wasm-ci.yml",
        ".github/workflows/security_hardening.yml",
    ):
        for line in _read(relative).splitlines():
            if "--receipt " in line or "--receipt-dir " in line:
                assert "proof-receipts" in line, (relative, line)


def test_executable_proof_workflows_contain_only_executor_mechanics() -> None:
    allowed_tools = {
        "tools/ci_resource_env.py",
        "tools/guarded_exec.py",
        "tools/profile_hotspots.py",
        "tools/proof_plan.py",
    }
    for relative in (
        ".github/workflows/ci.yml",
        ".github/workflows/formal.yml",
        ".github/workflows/molt-wasm-ci.yml",
        ".github/workflows/security_hardening.yml",
    ):
        for line_number, line in enumerate(_read(relative).splitlines(), start=1):
            # Setup/cache inputs may legitimately live under tools/ without
            # executing repository policy. Only Python tool invocations are
            # proof-authority candidates that must route through the plan.
            if "tools/" not in line or ".py" not in line:
                continue
            assert any(tool in line for tool in allowed_tools), (
                relative,
                line_number,
                line,
            )


def test_github_workflows_keep_cargo_target_dirs_cache_stable() -> None:
    unstable_tokens = ("${{ github.run_id }}", "${{ github.run_attempt }}")
    offenders: list[str] = []
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        for line_no, line in enumerate(
            workflow.read_text(encoding="utf-8").splitlines(), start=1
        ):
            if "CARGO_TARGET_DIR" not in line:
                continue
            if any(token in line for token in unstable_tokens):
                rel = workflow.relative_to(REPO_ROOT).as_posix()
                offenders.append(f"{rel}:{line_no}: {line.strip()}")

    assert offenders == []


def test_rust_security_reuses_cached_tool_builds() -> None:
    workflow_text = _read(".github/workflows/security_hardening.yml")
    rust_security = workflow_text.split("  rust-security:", 1)[1]

    assert (
        "MOLT_SESSION_ID: rust-security-${{ github.run_id }}-${{ github.run_attempt }}"
        in rust_security
    )
    assert "uses: ./.github/actions/setup-project" in rust_security
    assert "rust-toolchain: pinned" in rust_security
    assert 'cache-cargo: "true"' in rust_security
    setup_project = _read(".github/actions/setup-project/action.yml")
    assert "Restore Cargo dependency cache" in setup_project
    assert "${{ env.MOLT_CARGO_CACHE_TARGET }}" in setup_project
    assert "cargo install cargo-deny --version 0.20.2 --locked" in rust_security
    assert "cargo install cargo-audit --version 0.22.2 --locked" in rust_security
    assert "rm -rf" not in rust_security
    assert "tmp/security/cargo-audit-advisory-db" in _read("tools/proof_plan.toml")


def test_platform_portability_is_one_generated_cross_os_authority() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))

    assert not (WORKFLOW_ROOT / "proof-queue-portability.yml").exists()
    family = next(
        family
        for family in plan["ci_family"]
        if family["name"] == "platform_portability"
    )
    assert family["executor"] == "github-matrix"
    assert family["workflow"] == ".github/workflows/ci.yml"
    assert family["job"] == "platform-portability"
    assert "fail-fast: false" in ci_text
    assert "runs-on: ${{ matrix.runner }}" in ci_text
    setup_action = yaml.safe_load(_read(".github/actions/setup-project/action.yml"))
    # Every job that sets up the project gets custody outside the checkout:
    # runtime fixtures and guard scratch never land in the source tree.
    first = setup_action["runs"]["steps"][0]
    assert first["name"] == "Configure verified ephemeral custody"
    custody = first["run"]
    assert "MOLT_CI_EPHEMERAL_CUSTODY_ROOT=$custodyRoot" in custody
    assert "UV_PROJECT_ENVIRONMENT=$(Join-Path $custodyRoot 'venv')" in custody
    assert "MOLT_VENV=$(Join-Path $custodyRoot 'venv')" in custody
    sync = next(
        step
        for step in setup_action["runs"]["steps"]
        if step["name"] == "Synchronize project dependencies"
    )
    assert "args=(sync)" in sync["run"]
    assert 'uv "${args[@]}"' in sync["run"]
    assert "$env:RUNNER_TEMP" in custody
    assert "$env:RUNNER_TEMP" in ci_text
    assert not (REPO_ROOT / ".github/actions/ephemeral-custody").exists()
    assert "${{ runner.temp }}" not in ci_text
    assert "--run-family platform_portability --receipt" in ci_text
    assert '--matrix-cell "${{ matrix.cell }}"' in ci_text
    assert "uv run --active --project . --no-sync python -m pytest" not in ci_text

    cells = {
        cell["id"]: cell
        for cell in plan["matrix_cell"]
        if cell["backend"] == "proof-queue"
    }
    assert {(cell["os"], cell["runner"]) for cell in cells.values()} == {
        ("linux", "ubuntu-latest"),
        ("macos", "macos-14"),
        ("windows", "windows-2022"),
    }
    commands = [
        command
        for command in plan["command"]
        if command["family"] == "platform_portability"
    ]
    # Besides the three queue cells, the family carries two Rust cells: the
    # macOS cell (the primary host's workspace clippy and runtime gate) and the
    # aarch64 Linux cell (workspace clippy where C char is unsigned).
    rust_cells = {
        cell["id"]: cell
        for cell in plan["matrix_cell"]
        if cell["id"] in {command["cell"] for command in commands}
        and cell["backend"] == "rust"
    }
    assert {
        (cell["os"], cell["arch"], cell["runner"]) for cell in rust_cells.values()
    } == {("linux", "aarch64", "ubuntu-24.04-arm"), ("macos", "aarch64", "macos-14")}
    assert {command["cell"] for command in commands} == set(cells) | set(rust_cells)
    commands = [command for command in commands if command["cell"] in cells]
    queue_commands = [command for command in commands if ".queue." in command["id"]]
    ir_commands = [command for command in commands if ".ir." in command["id"]]
    # A row runs its argv prefix, then its typed pytest declaration.
    declarations = {
        (
            tuple(command["argv"]),
            tuple(command["pytest"]),
            command.get("pytest_worker_memory_gb"),
        )
        for command in queue_commands
    }
    assert len(declarations) == 1
    assert {command["cell"] for command in ir_commands} == {
        "macos-arm64-py312-queue-portability",
        "windows-x86_64-py312-queue-portability",
    }


def test_shell_completion_qualification_requires_actual_shells_in_existing_linux_job():
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    command = next(
        row for row in plan["command"] if row["id"] == "portability.completion.linux"
    )
    assert command["family"] == "platform_portability"
    assert command["cell"] == "linux-x86_64-py312-queue-portability"
    assert set(command["tiers"]) == {"pr", "main"}
    assert {"bash", "fish", "zsh"} <= set(command["toolchains"])
    assert command["pytest"] == [
        "tests/cli/test_cli_smoke.py::test_cli_completion_uses_parser_commands_flags_and_positional_semantics"
    ]
    policies = {row["name"]: row for row in plan["toolchain_policy"]}
    for shell in ("bash", "fish", "zsh"):
        assert policies[shell]["executable"] == shell
        assert policies[shell]["version_args"] == ["--version"]
    steps = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"][
        "platform-portability"
    ]["steps"]
    setup = next(
        row for row in steps if row.get("name") == "Install completion test shells"
    )
    execute = next(
        row
        for row in steps
        if row.get("name") == "Execute selected platform portability partition"
    )
    assert setup["if"] == "matrix.cell == 'linux-x86_64-py312-queue-portability'"
    assert "apt-get install --no-install-recommends --yes fish zsh" in setup["run"]
    assert steps.index(setup) < steps.index(execute)


def test_python_unit_runs_one_receipted_job_per_generated_cell() -> None:
    jobs = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]
    job = jobs["python-unit"]
    assert job["runs-on"] == "${{ matrix.runner }}"
    assert job["strategy"] == {
        "fail-fast": False,
        "matrix": "${{ fromJSON(needs.classify-changes.outputs.python_unit_matrix) }}",
    }
    steps = job["steps"]
    executor = next(
        step for step in steps if "--run-family python_unit" in step.get("run", "")
    )
    assert executor["run"].split() == [
        "python3",
        "tools/proof_plan.py",
        "--run-family",
        "python_unit",
        "--receipt",
        "proof-receipts/python-unit-${{",
        "matrix.cell",
        "}}.json",
        "--matrix-cell",
        '"${{',
        "matrix.cell",
        '}}"',
    ]
    uploads = [
        step for step in steps if step.get("name") == "Upload Python unit receipt"
    ]
    assert len(uploads) == 1
    upload = uploads[0]
    # One artifact per cell carries its receipt and any Mac preparation output;
    # the verdict still merges every proof-receipt-* artifact. The receipt guard
    # remains independent of whether the prerequisite produced any evidence.
    assert upload["if"] == (
        "always() && hashFiles(format('proof-receipts/python-unit-{0}.json', "
        "matrix.cell)) != ''"
    )
    assert upload["with"]["name"] == "proof-receipt-python-unit-${{ matrix.cell }}"
    assert upload["with"]["path"].splitlines() == [
        "proof-receipts/python-unit-${{ matrix.cell }}.json",
        "proof-results/compiled-contracts/macos",
    ]
    assert upload["with"]["if-no-files-found"] == "error"
    assert set(upload["with"]) == {"name", "path", "if-no-files-found"}
    assert steps.index(executor) < steps.index(upload)
    download = next(
        step
        for step in jobs["proof-plan-verdict"]["steps"]
        if step.get("uses", "").startswith("actions/download-artifact@")
    )
    assert download["with"]["pattern"] == "proof-receipt-*"
    assert download["with"]["path"] == "proof-receipts"
    assert download["with"]["merge-multiple"] is True


def test_checkouts_drop_persisted_credentials_and_permissions_are_bounded() -> None:
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        lines = text.splitlines()
        for index, line in enumerate(lines):
            if "uses: actions/checkout@" not in line:
                continue
            block = "\n".join(lines[index : index + 6])
            assert "persist-credentials: false" in block, workflow

        if workflow.name in {"pr_trust_gate.yml", "pr_trust_labeler.yml"}:
            continue
        assert "\npermissions:\n  contents: read\n" in text, workflow

    release = _read(".github/workflows/release.yml")
    assert (
        release.count("contents: write") == 2
    )  # Draft read admission and protected promotion.
    assert release.count("id-token: write") == 1
    assert release.count("attestations: write") == 1
    assert release.count("artifact-metadata: write") == 1


def test_pre_commit_hooks_are_read_only_by_default() -> None:
    default_python = _default_python_version()
    pre_commit_text = _read(".pre-commit-config.yaml")

    assert "- id: ruff" in pre_commit_text
    assert "repo: https://github.com/astral-sh/ruff-pre-commit" not in pre_commit_text
    # The hooks pass staged paths explicitly; only --force-exclude makes ruff
    # honour pyproject's extend-exclude (generated modules) for explicit paths.
    assert "uv run ruff check --force-exclude" in pre_commit_text
    assert f"--python {default_python}" not in pre_commit_text
    assert "--fix" not in pre_commit_text
    assert "- id: ruff-format" in pre_commit_text
    assert "uv run ruff format --check --force-exclude" in pre_commit_text
    assert "uv run ty check src" in pre_commit_text
    assert "tools/secret_guard.py --staged" in pre_commit_text
    assert "- id: end-of-file-fixer" not in pre_commit_text
    assert "- id: trailing-whitespace" not in pre_commit_text
    assert "git diff --cached --check" in pre_commit_text
    ruff = tomllib.loads(_read("pyproject.toml"))["tool"]["ruff"]
    assert ruff["force-exclude"] is True
    assert "src/molt/_wasm_abi_generated.py" in ruff["extend-exclude"]


def test_default_ci_python_version_comes_from_single_file() -> None:
    default_python = _default_python_version()

    checked_files = [".pre-commit-config.yaml"] + [
        f".github/workflows/{path.name}" for path in sorted(WORKFLOW_ROOT.glob("*.yml"))
    ]
    minor = default_python.rsplit(".", 1)[0]
    for path in checked_files:
        text = _read(path)
        for version in (default_python, minor):
            assert f"--python {version}" not in text
            assert f"uv python install {version}" not in text
            assert f'python-version: "{version}"' not in text
            assert f"python-version: '{version}'" not in text

    setup_project = _read(".github/actions/setup-project/action.yml")
    assert "actions/setup-python@" not in setup_project
    assert "bash .github/actions/setup-project/provision-python.sh" in setup_project
    bootstrap = _read(".github/actions/setup-project/provision-python.sh")
    assert "cat .python-version" in bootstrap
    for workflow in ("ci.yml", "formal.yml", "release.yml"):
        assert "uses: ./.github/actions/setup-project" in _read(
            f".github/workflows/{workflow}"
        )


def test_repo_githook_delegates_to_pre_commit_authority() -> None:
    hook_text = _read(".githooks/pre-commit")

    assert "pre-commit run --hook-stage pre-commit" in hook_text
    assert "tools/secret_guard.py" not in hook_text


def test_ci_clippy_failures_are_not_swallowed() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    proof_plan_text = _read("tools/proof_plan.toml")
    assert "rust.clippy.workspace-default" in proof_plan_text
    assert "rust.clippy.feature-surfaces" in proof_plan_text
    assert "--run-family rust --receipt" in ci_text
    assert "continue-on-error" not in ci_text


def test_ci_rust_compile_truth_has_no_redundant_subset_commands() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    commands = {command["id"]: command for command in plan["command"]}
    backend_manifest = tomllib.loads(_read("runtime/molt-backend/Cargo.toml"))

    # Workspace tests compile every package's normal library/bin target before
    # executing test targets, while all-target Clippy covers libs, bins,
    # examples, tests, and benches.  A preceding workspace build/runtime check
    # therefore adds no target or feature coverage and can starve both truths.
    for redundant_id in (
        "rust.check.runtime-default",
        "rust.build.workspace",
        "rust.test.backend-native-feature",
        "rust.clippy.backend-native",
    ):
        assert redundant_id not in commands
    assert "native-backend" in backend_manifest["features"]["default"]
    assert commands["rust.test.default-truth"]["dependencies"] == []
    # The compiler-build resource runs one command at a time, so ordering edges
    # add no starvation protection; they would only let one red truth hide the
    # independent lint truths behind "required dependency failed".
    assert commands["rust.clippy.workspace-default"]["dependencies"] == []
    assert commands["rust.clippy.feature-surfaces"]["dependencies"] == []
    compiler_policy = next(
        policy
        for policy in plan["resource_policy"]
        if policy["name"] == "compiler-build-resource"
    )
    assert compiler_policy["max_parallel"] == 1
    assert commands["rust.test.default-truth"]["argv"] == [
        "uv",
        "run",
        "--frozen",
        "python3",
        "tools/run_cargo_test_truth.py",
    ]
    rust_job = ci_text.split("\n  rust-build-unit-smoke:", 1)[1].split(
        "\n  llvm-backend:", 1
    )[0]
    assert 'uv: "false"' not in rust_job
    assert 'cache-uv: "false"' not in rust_job
    assert commands["rust.test.default-truth"]["timeout_budget"] == "suite"
    assert commands["rust.clippy.workspace-default"]["argv"] == [
        "cargo",
        "clippy",
        "--config",
        'build.warnings="deny"',
        "--locked",
        "--workspace",
        "--all-targets",
        "--keep-going",
    ]
    assert "logs/ci-cargo-build.log" not in ci_text


def test_ci_memory_intensive_steps_use_memory_guard() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert "--run-family repository_policy --receipt" in ci_text
    assert "uv run python3 -m pytest -q" not in ci_text
    assert "--run-family rust --receipt" in ci_text
    assert "--run-family llvm --receipt" in ci_text
    proof_executor = _read("tools/proof_plan.py")
    assert "peak_rss_bytes" in proof_executor
    assert "guard_metrics_schema" in proof_executor
    assert "os.killpg" not in proof_executor
    assert "psutil" not in proof_executor
    assert "python3 tools/profile_hotspots.py --limit 20" in ci_text


def test_kani_intrinsic_contracts_avoid_symbolic_std_sort() -> None:
    kani_text = _read("runtime/molt-obj-model/tests/kani_intrinsic_contracts.rs")

    assert "struct BoundedI64List" in kani_text
    assert "struct BoundedBoolList" in kani_text
    assert "Vec<" not in kani_text
    assert "Vec::" not in kani_text
    assert ".collect()" not in kani_text
    assert "DefaultHasher" not in kani_text
    assert "std::hash" not in kani_text
    assert "wrapping_mul" not in kani_text
    assert ".dedup()" not in kani_text
    assert ".sort()" not in kani_text


def test_kani_advisory_probe_has_single_cargo_cache_authority() -> None:
    kani_workflow = _read(".github/workflows/kani.yml")

    assert "uses: ./.github/actions/setup-project" in kani_workflow
    assert "cache-namespace: kani" in kani_workflow
    assert "actions/cache@v4" not in kani_workflow
    assert "Cache cargo registry and target" not in kani_workflow
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo install --locked kani-verifier"
    ) in kani_workflow
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- cargo kani setup"
    ) in kani_workflow
    assert "cd runtime/molt-obj-model && cargo kani --tests" not in kani_workflow
    assert "cd runtime/molt-runtime && cargo kani --tests" not in kani_workflow
    assert "cargo kani --tests" not in kani_workflow


def test_formal_workflow_uses_bounded_blocking_quint_gate() -> None:
    formal_workflow = _read(".github/workflows/formal.yml")

    assert "--run-command formal.quint.models --receipt" in formal_workflow
    assert "for model in *.qnt" not in formal_workflow
    assert 'quint verify "$model"' not in formal_workflow
    assert "failed verification (non-blocking)" not in formal_workflow


def test_lean_workflows_share_exact_provisioning_authority() -> None:
    setup_action = _read(".github/actions/setup-lean/action.yml")
    formal_workflow = _read(".github/workflows/formal.yml")
    nightly_workflow = _read(".github/workflows/nightly.yml")

    assert "toolchain=\"$(tr -d '\\r\\n' < formal/lean/lean-toolchain)\"" in (
        setup_action
    )
    probe = 'if ! "$elan" run "$toolchain" lean --version >/dev/null 2>&1; then'
    assert '"$elan" toolchain install "$toolchain"' in setup_action
    assert probe in setup_action
    assert setup_action.index(probe) < setup_action.index(
        '"$elan" toolchain install "$toolchain"'
    )
    assert 'expected_version="${toolchain##*:v}"' in setup_action
    assert '"$exact_version" != "Lean (version $expected_version,"*' in setup_action
    assert '"$selected_version" != "$exact_version"' in setup_action
    setup_project = _read(".github/actions/setup-project/action.yml")
    assert "formal/lean/.lake" in setup_project
    assert "~/.elan/toolchains" in setup_project
    assert 'cache-lean: "true"' in formal_workflow
    assert formal_workflow.count("uses: ./.github/actions/setup-lean") == 1
    assert "uses: ./.github/actions/setup-lean" not in nightly_workflow
    assert "elan/master" not in setup_action
    assert "tools.release.fetch_pinned_tool" in setup_action
    assert "elan-init.sh" not in formal_workflow
    assert "elan-init.sh" not in nightly_workflow


def test_quint_workflows_pin_patched_node24_toolchain() -> None:
    formal_workflow = _read(".github/workflows/formal.yml")
    nightly_workflow = _read(".github/workflows/nightly.yml")

    setup_project = _read(".github/actions/setup-project/action.yml")
    assert "uses: actions/setup-node@" in setup_project
    assert "node-version: pinned" in formal_workflow
    assert "check-latest: true" not in setup_project
    quint = next(
        policy
        for policy in tomllib.loads(_read("tools/proof_plan.toml"))["toolchain_policy"]
        if policy["name"] == "quint"
    )
    assert (
        f'MOLT_QUINT_NPM_PACKAGE: "@informalsystems/quint@{quint["setup_value"]}"'
        in formal_workflow
    )
    # Quint pins its own evaluator release; the workflow pins that exact release.
    assert re.search(
        r'MOLT_QUINT_RUST_EVALUATOR_VERSION: "v\d+\.\d+\.\d+"', formal_workflow
    )
    assert re.search(
        r'MOLT_QUINT_RUST_EVALUATOR_SHA256: "[0-9a-f]{64}"', formal_workflow
    )
    assert "Install Quint Rust evaluator" in formal_workflow
    assert "sha256sum --check" in formal_workflow
    # Quint installs with the pinned npm into its own prefix, never PATH's npm.
    assert (
        'python3 -m molt.tool_releases run npm -- install --global --prefix "$RUNNER_TEMP/quint"'
        in formal_workflow
    )
    assert "npm install -g" not in formal_workflow

    assert 'npm install -g "$MOLT_QUINT_NPM_PACKAGE"' not in nightly_workflow
    assert "actions/setup-node@" not in nightly_workflow
    assert "node-version:" not in nightly_workflow
    assert "Install Quint Rust evaluator" not in nightly_workflow


def test_nightly_contains_correctness_jobs() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")

    assert "schedule:" in nightly_text
    assert "workflow_dispatch:" in nightly_text
    for job in (
        "nightly-prepare:",
        "conformance-shard:",
        "differential-shard:",
        "regrtest-shard:",
        "conformance-aggregate:",
        "differential-aggregate:",
        "regrtest-aggregate:",
    ):
        assert job in nightly_text
    for family in (
        "nightly_shard_prepare",
        "nightly_conformance",
        "nightly_differential",
        "nightly_regrtest",
        "nightly_determinism",
        "nightly_verification_t3",
    ):
        assert f"--run-family {family} --receipt" in nightly_text
    assert "nightly-verdict:" in nightly_text
    assert "uses: actions/download-artifact@" in nightly_text
    assert "--verify-scheduled --receipt-dir nightly-artifacts" in nightly_text
    assert "tools/guarded_exec.py" not in nightly_text
    assert "tests/harness/run_molt_conformance.py" not in nightly_text
    assert "tests/molt_diff.py" not in nightly_text
    assert "tools/cpython_regrtest.py" not in nightly_text
    assert nightly_text.count("tools/nightly_sharding.py run-shard") == 3
    assert nightly_text.count("tools/nightly_runtime_bundle.py verify-extract") == 3
    assert nightly_text.count('MOLT_STDLIB_PROFILE: "full"') == 3
    assert nightly_text.count('MOLT_DIFF_STDLIB_PROFILE: "full"') == 1
    assert "max-parallel: 8" in nightly_text
    assert "max-parallel: 16" in nightly_text
    assert "max-parallel: 4" in nightly_text
    assert "tools/check_reproducible_build.py" not in nightly_text
    proof_plan_text = _read("tools/proof_plan.toml")
    assert 'id = "nightly.verification-t3.reproducibility"' in proof_plan_text
    assert '"--corpus", "full", "--runs", "5", "--audit-ir"' in proof_plan_text
    assert '"proof-results/reproducibility-tier3.json"' in proof_plan_text
    assert "tools/check_deterministic_runtime.py" not in nightly_text
    assert "proof-results/nightly/deterministic-runtime.json" in nightly_text
    assert "proof-results/nightly/ir-verification.json" in nightly_text
    assert "name: proof-receipt-nightly-determinism" in nightly_text
    assert "mkdir -p /tmp/repro_sweep" not in nightly_text
    assert "MOLT_CACHE=/tmp/repro_sweep" not in nightly_text
    assert "~/.molt/build/" not in nightly_text
    assert "cargo build -p molt-runtime" not in nightly_text
    assert "cargo build -p molt-runtime --release" not in nightly_text
    assert "SKIP: build failed" not in nightly_text
    assert "|| true" not in nightly_text
    assert "continue-on-error: true" not in nightly_text


def test_hosted_workflow_heavy_commands_enter_memory_guard() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")
    formal_text = _read(".github/workflows/formal.yml")
    security_text = _read(".github/workflows/security_hardening.yml")
    release_text = _read(".github/workflows/release.yml")

    assert nightly_text.count("python3 tools/proof_plan.py --run-family") == 7
    assert "run: cargo build -p molt-runtime --profile dev-fast" not in nightly_text
    assert "uses: ./.github/workflows/formal.yml" in nightly_text
    assert "quint verify formal/quint/" not in nightly_text
    assert "run: cargo install cargo-deny --locked" not in nightly_text
    assert "run: cargo deny check" not in nightly_text
    assert "          quint verify formal/quint/" not in nightly_text
    assert "uses: ./.github/workflows/security_hardening.yml" not in nightly_text

    assert "--run-command formal.lean.build --receipt" not in formal_text
    assert "--run-command formal.lean.sorry-baseline --receipt" in formal_text
    assert "--run-command formal.quint.models --receipt" in formal_text
    assert "--run-command formal.correspondence --receipt" in formal_text
    assert "run: lake build" not in formal_text
    assert "run: python3 tools/check_formal_methods.py --quint-only" not in formal_text
    assert (
        "run: python3 tools/check_formal_methods.py --check-correspondence"
        not in formal_text
    )

    assert "--run-family python_security --receipt" in security_text
    assert "--run-family rust_security --receipt" in security_text
    policies = {
        policy["name"]: policy["setup_value"]
        for policy in tomllib.loads(_read("tools/proof_plan.toml"))["toolchain_policy"]
        if "setup_value" in policy
    }
    for tool in ("cargo-deny", "cargo-audit"):
        assert (
            "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
            f"cargo install {tool} --version {policies[tool]} --locked"
        ) in security_text
    assert "run: uv run pip-audit --ignore-vuln CVE-2025-69872" not in security_text
    assert "run: cargo deny check" not in security_text
    assert "          cargo install cargo-deny --locked" not in security_text
    assert "          cargo install cargo-audit --locked" not in security_text

    release_workflow = yaml.safe_load(release_text)
    build_steps = [
        step["run"]
        for job in release_workflow["jobs"].values()
        for step in job.get("steps", [])
        if step.get("name") == "Build independent native release generations"
    ]
    assert len(build_steps) == 1
    assert "for lane in primary secondary; do" in build_steps[0]
    assert "tools/guarded_exec.py --prefix MOLT_RELEASE --" in build_steps[0]
    assert "python -m tools.release.build_compiler" in build_steps[0]
    assert '--output "dist/native-$lane"' in build_steps[0]
    assert "run: cargo build -p molt-worker --release" not in release_text


def test_named_proof_lanes_fail_closed_and_share_counted_verdict_authority() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")
    formal_text = _read(".github/workflows/formal.yml")

    for workflow_text in (nightly_text, formal_text):
        assert "continue-on-error: true" not in workflow_text
    assert "SKIP: build failed" not in nightly_text
    assert "if-no-files-found: ignore" not in nightly_text
    for tool in (
        "tools/check_deterministic_runtime.py",
        "tools/check_reproducible_build.py",
        "tools/verify_ir_suite.py",
        "tools/mutation_test.py",
        "tools/translation_validate.py",
    ):
        text = _read(tool)
        assert "fail_closed_proof_exit_code(" in text
    for tool in (
        "tools/check_deterministic_runtime.py",
        "tools/verify_ir_suite.py",
    ):
        text = _read(tool)
        assert '"executed": passed + failed' in text
    reproducibility_text = _read("tools/check_reproducible_build.py")
    assert "def _write_proof_receipt(" in reproducibility_text
    for count in ("selected", "executed", "passed", "failed", "errors"):
        assert f'"{count}": {count}' in reproducibility_text

    assert fail_closed_proof_exit_code(executed=1, failed=0, errors=0) == 0
    assert fail_closed_proof_exit_code(executed=1, failed=1, errors=0) == 1
    assert fail_closed_proof_exit_code(executed=0, failed=0, errors=0) == 2
    assert fail_closed_proof_exit_code(executed=0, failed=0, errors=1) == 2
    with pytest.raises(ValueError, match="cannot exceed"):
        fail_closed_proof_exit_code(executed=0, failed=1, errors=0)


def test_security_hardening_is_reusable_and_ci_uses_one_planner() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    security_text = _read(".github/workflows/security_hardening.yml")

    assert "workflow_call:" in security_text
    assert "classify-changes:" not in security_text
    assert "--github-output" not in security_text
    assert "inputs.python_security" in security_text
    assert "inputs.rust_security" in security_text
    assert "uses: ./.github/workflows/security_hardening.yml" in ci_text
    assert ci_text.count('tools/proof_plan.py --github-output "$GITHUB_OUTPUT"') == 1
    assert ci_text.count("tools/proof_plan.py\n          --verify-selected") == 1


def test_release_and_perf_workflows_exist_for_hosted_validation() -> None:
    release_text = _read(".github/workflows/release.yml")
    perf_text = _read(".github/workflows/perf-gate.yml")

    assert "push:" not in release_text
    assert 'test "$GITHUB_REF" = "refs/tags/$REQUESTED_VERSION"' in release_text
    assert "workflow_dispatch:" in release_text
    release_config = _read("config/release_targets.toml")
    assert "macos-15" in release_config
    assert "ubuntu-24.04" in release_config
    assert "windows-2022" in release_config
    assert "windows-11-arm" in release_config
    assert "fromJSON(needs.plan.outputs.matrix)" in release_text
    assert "schedule:" in perf_text
    assert "MOLT_SESSION_ID: perfscore-${{ matrix.backend }}" in perf_text
    assert "tools/guarded_exec.py --prefix MOLT_BENCH" in perf_text
    assert "tools/perf_scoreboard.py" in perf_text
    assert "backend: [native, llvm]" in perf_text
    assert '--backend "${{ matrix.backend }}"' in perf_text
    assert "--profile release-fast" in perf_text
    assert "--samples 5" in perf_text
    assert "--warmup 2" in perf_text
    assert "--repeat 5" in perf_text
    assert "--classify" in perf_text
    assert "--require-quiescent" in perf_text
    assert "bench/scoreboard/logs_*/" in perf_text
    assert "--no-gate" not in perf_text
    assert "--allow-nonauthoritative" not in perf_text
    assert "tools/bench.py" not in perf_text
    assert "bench/results/" not in perf_text
    assert not (WORKFLOW_ROOT / "perf-validation.yml").exists()


def test_perf_demo_workflow_uses_canonical_env_and_single_uv_sync() -> None:
    perf_demo_text = _read(".github/workflows/perf_demo.yml")
    run_stack_text = _read("bench/scripts/run_stack.sh")

    assert "MOLT_SESSION_ID: perf-demo-${{ github.run_id }}" in perf_demo_text
    assert 'MOLT_UV_SYNC: "0"' in perf_demo_text
    assert 'if [[ "${MOLT_UV_SYNC:-1}" != "0" ]]' in run_stack_text
    assert 'cargo build --profile "$CARGO_PROFILE" -p molt-worker' in run_stack_text
    assert (
        'CARGO_ROOT="${CARGO_TARGET_DIR:-$ROOT/target/sessions/${MOLT_SESSION_ID:-demo-stack}}"'
        in run_stack_text
    )
    assert 'WORKER_BIN="$CARGO_ROOT/$CARGO_PROFILE/molt-worker"' in run_stack_text


def test_wasm_ci_uses_molt_wasm_host_for_imported_modules() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    proof_text = _read("tools/proof_plan.toml")

    assert "--run-family wasm --receipt" in wasm_text
    assert 'id = "wasm.build.host"' in proof_text
    assert 'id = "wasm.run.hello"' in proof_text
    assert 'id = "wasm.run.comprehension"' in proof_text
    assert 'id = "wasm.run.sieve"' in proof_text
    assert "cargo build --profile dev-fast -p molt-wasm-host" not in wasm_text
    assert "wasmtime run /tmp/test_hello.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_comprehension.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_sieve.wasm" not in wasm_text


def test_node_toolchain_consumers_provision_the_canonical_version() -> None:
    from tools.proof_plan import ProofPlan

    plan = ProofPlan.load(REPO_ROOT / "tools/proof_plan.toml")
    node_policy = next(
        policy for policy in plan.toolchain_policies if policy.name == "node"
    )
    node_families = {
        command.family
        for command in plan.commands
        if "node" in plan.required_toolchains(command)
    }
    consumers = {
        "browser_simd": (".github/workflows/ci.yml", "browser-simd"),
        "repository_policy": (".github/workflows/ci.yml", "docs-gates"),
        "rust": (".github/workflows/ci.yml", "rust-build-unit-smoke"),
        "wasm": (".github/workflows/molt-wasm-ci.yml", "wasm-build"),
        "formal": (".github/workflows/formal.yml", "formal-quint"),
        "platform_portability": (".github/workflows/ci.yml", "platform-portability"),
        "python_unit": (".github/workflows/ci.yml", "python-unit"),
    }
    # setup-project resolves `pinned` from config/tool_releases.toml; the
    # plan's preflight pattern must accept exactly that version.
    assert node_policy.data["setup_value"] == tool_releases.tool_release("node").version

    assert node_families == set(consumers)
    for family, (workflow_path, job_name) in consumers.items():
        jobs = yaml.safe_load(_read(workflow_path))["jobs"]
        setup_steps = [
            step
            for step in jobs[job_name]["steps"]
            if step.get("uses") == "./.github/actions/setup-project"
        ]
        assert len(setup_steps) == 1, family
        if family == "python_unit":
            steps = jobs[job_name]["steps"]
            provision = [
                step
                for step in steps
                if step.get("run")
                == "uv run --no-sync python -m molt.tool_releases provision node"
            ]
            assert len(provision) == 1
            assert provision[0]["if"] == "contains(matrix.toolchains, 'node')"
            execute = next(
                step
                for step in steps
                if "--run-family python_unit" in step.get("run", "")
            )
            assert (
                steps.index(setup_steps[0])
                < steps.index(provision[0])
                < steps.index(execute)
            )
            continue
        expected_node_setup = (
            "${{ contains(matrix.toolchains, 'node') && 'pinned' || '' }}"
            if family == "platform_portability"
            else "pinned"
        )
        assert (
            setup_steps[0].get("with", {}).get("node-version") == expected_node_setup
        ), family

        if family in {"repository_policy", "wasm"}:
            steps = jobs[job_name]["steps"]
            setup_index = steps.index(setup_steps[0])
            parser_indices = [
                index
                for index, step in enumerate(steps)
                if "molt.checkout_provisioning ensure browser-acorn"
                in step.get("run", "")
            ]
            partition_index = next(
                index
                for index, step in enumerate(steps)
                if f"--run-family {family}" in step.get("run", "")
            )
            assert len(parser_indices) == 1, family
            assert setup_index < parser_indices[0] < partition_index, family
            assert setup_steps[0]["with"]["node-cache-dependency-path"] == (
                "tools/browser_asset_graph/package-lock.json"
            )


def test_wasm_ci_uses_canonical_artifact_roots_and_dev_profile() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    wasm_commands = [
        command for command in plan["command"] if command["family"] == "wasm"
    ]

    assert "MOLT_EXT_ROOT: /tmp/molt-ext" in wasm_text
    assert "workflow_call:" in wasm_text
    assert "push:" not in wasm_text
    assert "pull_request:" not in wasm_text
    assert "MOLT_CACHE: /tmp/molt-ext/molt_cache" in wasm_text
    assert "MOLT_DIFF_ROOT: /tmp/molt-ext/diff" in wasm_text
    assert "MOLT_DIFF_TMPDIR: /tmp/molt-ext/tmp" in wasm_text
    assert "MOLT_WASM_RUNTIME_DIR: /tmp/molt-ext/wasm" in wasm_text
    assert "concurrency:" in wasm_text
    assert "cancel-in-progress: ${{ github.event_name == 'pull_request' }}" in (
        wasm_text
    )
    assert "MOLT_CI_PYTHON" not in wasm_text
    assert "uses: ./.github/actions/setup-project" in wasm_text
    assert 'cache-cargo: "true"' in wasm_text
    assert "cache-namespace: wasm-ci" in wasm_text
    assert "python3 -m molt.tool_releases provision wasm-tools" in wasm_text
    assert '--github-path "$GITHUB_PATH"' in wasm_text
    assert "taiki-e/install-action" not in wasm_text
    assert (
        "MOLT_SESSION_ID: wasm-ci-${{ github.run_id }}-${{ github.run_attempt }}"
        in wasm_text
    )
    assert "MOLT_WASM_TEST_CHILD_RLIMIT_GB" not in wasm_text
    assert 'MOLT_WASM_TEST_KEEPALIVE_SEC: "20"' in wasm_text
    assert 'MOLT_PREFER_EXTERNAL_ARTIFACTS: "1"' in wasm_text
    assert 'MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC: "2"' in wasm_text
    assert "CARGO_INCREMENTAL:" not in wasm_text
    assert 'CARGO_BUILD_JOBS: "1"' not in wasm_text
    assert "MOLT_WASM_TEST_TIMEOUT_SEC:" not in wasm_text
    assert "MOLT_CARGO_TIMEOUT:" not in wasm_text
    assert "MOLT_BACKEND_DAEMON_SOCKET_DIR" not in wasm_text
    assert "MOLT_BACKEND_DAEMON_CACHE_MB" not in wasm_text
    assert "tools/guarded_exec.py" not in wasm_text
    assert "tools/venv_exec.py" not in wasm_text
    assert "--run-family wasm --receipt" in wasm_text
    assert len(wasm_commands) >= 12
    assert all(command.get("timeout_env") for command in wasm_commands)
    assert all(
        command.get("timeout_budget") or command.get("timeout_seconds")
        for command in wasm_commands
    )
    assert (
        next(
            command for command in wasm_commands if command["id"] == "wasm.build.host"
        )["timeout_budget"]
        == "cold"
    )
    assert any(command["id"] == "wasm.compile.hello" for command in wasm_commands)
    assert any(command["id"] == "wasm.test.control-flow" for command in wasm_commands)
    assert "python3 tools/profile_hotspots.py --limit 20" in wasm_text
    assert "/home/runner/.cache/molt" not in wasm_text


def test_wasm_ci_guarded_steps_have_github_timeout_backstops() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    wasm_family = next(
        family for family in plan["ci_family"] if family["name"] == "wasm"
    )

    assert f"timeout-minutes: {wasm_family['timeout_minutes']}" in wasm_text
    # The reserve is family policy, never an override of nested command bounds.
    assert type(wasm_family["job_reserve_seconds"]) is int
    assert wasm_family["job_reserve_seconds"] > 0
    assert "job_reserve_seconds" not in wasm_text
    assert "--timeout" not in wasm_text
    assert "MOLT_CARGO_TIMEOUT:" not in wasm_text
    assert "MOLT_WASM_TEST_TIMEOUT_SEC:" not in wasm_text


# Repository Actions policy (Settings > Actions > General), read from
# `gh api repos/adpena/molt/actions/permissions/selected-actions`: GitHub-owned
# actions, actions owned by the repository owner, and these patterns. A
# disallowed `uses:` fails every job at "Prepare all required actions" before
# any step runs, so the policy is checked statically here.
_ACTIONS_POLICY_OWNERS = frozenset(("actions", "github", "adpena"))
_ACTIONS_POLICY_PATTERNS = (
    "astral-sh/setup-uv@*",
    "cloudflare/wrangler-action@*",
    "softprops/action-gh-release@*",
    "taiki-e/install-action@*",
)


def _action_references() -> list[tuple[str, str]]:
    references: list[tuple[str, str]] = []
    root = Path(__file__).resolve().parents[1]
    paths = sorted((root / ".github" / "workflows").glob("*.yml")) + sorted(
        (root / ".github" / "actions").glob("*/action.yml")
    )
    for path in paths:
        document = yaml.safe_load(path.read_text(encoding="utf-8"))
        jobs = (document or {}).get("jobs", {}) or {}
        steps = [
            step for job in jobs.values() for step in (job or {}).get("steps", []) or []
        ]
        steps += ((document or {}).get("runs", {}) or {}).get("steps", []) or []
        uses = [
            job["uses"]
            for job in jobs.values()
            if isinstance(job, dict) and "uses" in job
        ]
        uses += [
            step["uses"] for step in steps if isinstance(step, dict) and "uses" in step
        ]
        references.extend((path.relative_to(root).as_posix(), use) for use in uses)
    return references


def test_every_action_reference_is_admitted_by_repository_policy() -> None:
    references = _action_references()
    assert references
    for source, use in references:
        if use.startswith("./"):
            continue
        assert re.fullmatch(r"[\w.-]+/[\w./-]+@[0-9a-f]{40}", use), (source, use)
        owner = use.split("/", 1)[0]
        admitted = owner in _ACTIONS_POLICY_OWNERS or any(
            fnmatchcase(use, pattern) for pattern in _ACTIONS_POLICY_PATTERNS
        )
        assert admitted, (
            f"{source}: {use} is not allowed by the repository Actions policy"
        )


def test_bootstrap_binds_managed_python_before_repository_or_rust_consumers() -> None:
    action = yaml.safe_load(_read(".github/actions/setup-project/action.yml"))
    steps = action["runs"]["steps"]
    names = [step.get("name") for step in steps]
    python = names.index("Provision and bind repository Python")
    assert names.index("Install exact uv") < python < names.index("Install exact Rust")
    assert (
        action["outputs"]["python-path"]["value"]
        == "${{ steps.python.outputs.python-path }}"
    )
    for step in steps[:python]:
        assert not re.search(r"\bpython3?\s", str(step.get("run", "")))
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        payload = yaml.safe_load(workflow.read_text(encoding="utf-8"))
        for job in payload.get("jobs", {}).values():
            for step in job.get("steps", []):
                if step.get("uses") != "./.github/actions/setup-project":
                    continue
                inputs = step.get("with", {})
                if str(inputs.get("python", "true")) == "true":
                    assert str(inputs.get("uv", "true")) == "true", workflow
    job = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]["python-unit"]
    setup = next(
        step
        for step in job["steps"]
        if step.get("uses") == "./.github/actions/setup-project"
    )
    assert setup["with"]["rust-toolchain"] == "pinned"
    execute = next(
        step
        for step in job["steps"]
        if "--run-family python_unit" in step.get("run", "")
    )
    assert job["steps"].index(setup) < job["steps"].index(execute)


def test_workflows_root_no_artifact_state_inside_the_checkout() -> None:
    """setup-project gives every job custody under RUNNER_TEMP; dx rejects a
    canonical root (target, caches, scratch) that points into the checkout."""
    import sys

    sys.path.insert(0, str(REPO_ROOT / "src"))
    from molt.dx import CANONICAL_ROOT_ENV_KEYS

    keys = set(CANONICAL_ROOT_ENV_KEYS)
    offenders = []
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        for number, line in enumerate(
            workflow.read_text(encoding="utf-8").splitlines(), start=1
        ):
            match = re.match(r"^\s+([A-Z_]+):\s*(.*)$", line)
            if match and match[1] in keys and "github.workspace" in match[2]:
                offenders.append(f"{workflow.name}:{number}: {line.strip()}")
    assert offenders == []


_MAIN_REF = "github.ref == 'refs/heads/main'"


def _workflow_and_action_steps() -> list[tuple[str, dict[str, object]]]:
    found: list[tuple[str, dict[str, object]]] = []
    for pattern in (".github/workflows/*.yml", ".github/actions/*/action.yml"):
        for path in sorted(REPO_ROOT.glob(pattern)):
            document = yaml.safe_load(path.read_text(encoding="utf-8"))
            relative = path.relative_to(REPO_ROOT).as_posix()
            runs = document.get("runs")
            if isinstance(runs, dict):
                found.extend((relative, step) for step in runs.get("steps", []))
            for job in (document.get("jobs") or {}).values():
                found.extend((relative, step) for step in job.get("steps", []))
    return found


def test_only_main_saves_github_actions_caches() -> None:
    # The repository has a 10 GB cache quota. Other refs can restore main's
    # entries but not each other's, so a branch's own copy of a content-keyed
    # cache only duplicates main's and evicts main's Cargo caches (HF-139).
    steps = _workflow_and_action_steps()
    saving = []
    for path, step in steps:
        uses = str(step.get("uses", ""))
        condition = str(step.get("if", ""))
        if uses.startswith(("actions/cache@", "actions/cache/save@")):
            assert _MAIN_REF in condition, (path, step.get("name"))
            saving.append((path, step))
        elif uses.startswith("astral-sh/setup-uv@"):
            assert step["with"]["save-cache"] == "${{ " + _MAIN_REF + " }}", path
        elif uses.startswith("actions/setup-node@") and "cache" in (
            step.get("with") or {}
        ):
            # setup-node has no save switch; its npm entries are under 1 MB.
            assert step["with"]["cache"] == "npm", path
    assert saving, "the policy must see the cache steps it governs"
    # Every main-only restore-and-save step has a restore-only twin for other
    # refs with the same path and key.
    for path, step in saving:
        if not str(step["uses"]).startswith("actions/cache@"):
            continue
        twin_condition = str(step["if"]).replace(
            _MAIN_REF, "github.ref != 'refs/heads/main'"
        )
        twins = [
            other
            for other_path, other in steps
            if other_path == path
            and str(other.get("uses", "")).startswith("actions/cache/restore@")
            and other.get("if") == twin_condition
            and other.get("with") == step["with"]
        ]
        assert len(twins) == 1, (path, step.get("name"))
