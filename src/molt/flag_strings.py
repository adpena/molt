"""Argument vectors carried in one string, as Cargo and cc-rs read them.

Cargo and cc-rs each split a flag variable with their own rule, and none of
the rules removes shell quotes unless it says so. Each rule here ports the
consumer's code; ``encode`` refuses a token the consumer would split, so a
flag never reaches rustc or a C compiler changed.

- Cargo ``rustflags_from_env`` (``build_context/target_info.rs``):
  ``CARGO_ENCODED_RUSTFLAGS`` splits at ``\\x1f``; ``RUSTFLAGS`` splits at
  ``' '`` and trims each piece.
- Cargo ``StringList`` (``context/de.rs``, ``get_env_list``): a config string
  and a ``CARGO_*`` list variable split with ``split_whitespace``.
- cc-rs ``envflags``: ``*FLAGS`` split with ``split_ascii_whitespace``, or with
  the ``shlex`` crate's ``Shlex`` when ``CC_SHELL_ESCAPED_FLAGS`` is true.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from enum import Enum
import re
import shlex

# Rust ``char::is_whitespace`` (Unicode White_Space). Python's ``str.split``
# also splits at U+001C..U+001F, which Rust does not.
_RUST_WHITESPACE = "\t\n\x0b\x0c\r \x85\xa0                　"
# Rust ``u8::is_ascii_whitespace``: no vertical tab.
_ASCII_WHITESPACE = " \t\n\x0c\r"
# The ``shlex`` crate separates words only at these.
_SHELL_BLANKS = " \t\n"

_RUST_WHITESPACE_RUN = re.compile(f"[{re.escape(_RUST_WHITESPACE)}]+")
_ASCII_WHITESPACE_RUN = re.compile(f"[{re.escape(_ASCII_WHITESPACE)}]+")

CC_SHELL_ESCAPED_FLAGS = "CC_SHELL_ESCAPED_FLAGS"


class FlagStringError(ValueError):
    pass


class FlagStringRule(Enum):
    CARGO_ENCODED = "cargo-encoded"
    CARGO_RUSTFLAGS = "cargo-rustflags"
    CARGO_LIST = "cargo-list"
    CC_WHITESPACE = "cc-whitespace"
    CC_SHELL = "cc-shell"

    @classmethod
    def cc(cls, env: Mapping[str, str]) -> FlagStringRule:
        """The rule cc-rs applies to ``*FLAGS`` under this environment."""
        # cc-rs ``env_value_is_true``: any value but "", "0", "false", "no".
        value = env.get(CC_SHELL_ESCAPED_FLAGS)
        if value is None or value in {"", "0", "false", "no"}:
            return cls.CC_WHITESPACE
        return cls.CC_SHELL

    def decode(self, value: str) -> tuple[str, ...]:
        """The argument vector the consumer reads from ``value``."""
        if self is FlagStringRule.CARGO_ENCODED:
            return tuple(value.split("\x1f")) if value else ()
        if self is FlagStringRule.CARGO_RUSTFLAGS:
            pieces = (piece.strip(_RUST_WHITESPACE) for piece in value.split(" "))
            return tuple(piece for piece in pieces if piece)
        if self is FlagStringRule.CARGO_LIST:
            return tuple(item for item in _RUST_WHITESPACE_RUN.split(value) if item)
        if self is FlagStringRule.CC_WHITESPACE:
            return tuple(item for item in _ASCII_WHITESPACE_RUN.split(value) if item)
        return _shell_words(value)

    def encode(self, tokens: Sequence[str], *, variable: str) -> str:
        """A string the consumer decodes to exactly ``tokens``."""
        for token in tokens:
            if not isinstance(token, str) or not token or "\0" in token:
                raise FlagStringError(f"{variable} cannot carry the token {token!r}")
            if not self.carries(token):
                raise FlagStringError(
                    f"{variable} cannot carry {token!r}: {self.consumer} splits it; "
                    "select a path without that whitespace or separator"
                )
        if self is FlagStringRule.CARGO_ENCODED:
            return "\x1f".join(tokens)
        if self is FlagStringRule.CC_SHELL:
            return shlex.join(tokens)
        return " ".join(tokens)

    def carries(self, token: str) -> bool:
        """Whether one token survives this rule's split unchanged."""
        if self is FlagStringRule.CARGO_ENCODED:
            return "\x1f" not in token
        if self is FlagStringRule.CARGO_RUSTFLAGS:
            return " " not in token and token == token.strip(_RUST_WHITESPACE)
        if self is FlagStringRule.CARGO_LIST:
            return not any(char in _RUST_WHITESPACE for char in token)
        if self is FlagStringRule.CC_WHITESPACE:
            return not any(char in _ASCII_WHITESPACE for char in token)
        return True

    @property
    def consumer(self) -> str:
        return _CONSUMERS[self]


_CONSUMERS = {
    FlagStringRule.CARGO_ENCODED: "Cargo's 0x1f-separated flag list",
    FlagStringRule.CARGO_RUSTFLAGS: "Cargo's space-separated RUSTFLAGS",
    FlagStringRule.CARGO_LIST: "Cargo's whitespace-separated config list",
    FlagStringRule.CC_WHITESPACE: "cc-rs's whitespace-separated flags",
    FlagStringRule.CC_SHELL: "cc-rs's shell-escaped flags",
}


def _shell_words(value: str) -> tuple[str, ...]:
    """Port of the ``shlex`` 2.0 crate's ``Shlex`` iterator, which cc-rs uses.

    The crate drops a malformed last word without an error; cc-rs then runs
    the compiler without it. Molt refuses that input instead.
    """
    words: list[str] = []
    index, end = 0, len(value)
    while True:
        while index < end:
            char = value[index]
            if char in _SHELL_BLANKS:
                index += 1
            elif char == "#":
                newline = value.find("\n", index)
                index = end if newline < 0 else newline + 1
            else:
                break
        if index >= end:
            return tuple(words)
        word: list[str] = []
        while index < end:
            char = value[index]
            index += 1
            if char in _SHELL_BLANKS:
                break
            if char == "'":
                close = value.find("'", index)
                if close < 0:
                    raise FlagStringError(
                        "shell-escaped flags end inside a '...' quote"
                    )
                word.append(value[index:close])
                index = close + 1
            elif char == '"':
                while True:
                    if index >= end:
                        raise FlagStringError(
                            'shell-escaped flags end inside a "..." quote'
                        )
                    char = value[index]
                    index += 1
                    if char == '"':
                        break
                    if char != "\\":
                        word.append(char)
                        continue
                    if index >= end:
                        raise FlagStringError(
                            'shell-escaped flags end inside a "..." quote'
                        )
                    escaped = value[index]
                    index += 1
                    if escaped in '$`"\\':
                        word.append(escaped)
                    elif escaped != "\n":
                        word.append("\\" + escaped)
            elif char == "\\":
                if index >= end:
                    raise FlagStringError("shell-escaped flags end with a backslash")
                escaped = value[index]
                index += 1
                if escaped != "\n":
                    word.append(escaped)
            else:
                word.append(char)
        words.append("".join(word))
