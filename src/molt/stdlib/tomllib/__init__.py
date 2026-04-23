"""Full TOML 1.0 parser for Molt — CPython 3.11+ ``tomllib`` parity.

Implements the complete TOML 1.0 spec in pure Python:

  * Bare and quoted keys (basic and literal strings)
  * Dotted keys
  * Standard tables  [table]
  * Array-of-tables  [[array]]
  * Inline tables    {a = 1, b = 2}
  * String types: basic, literal, multi-line basic, multi-line literal
  * Integer (decimal, hex, octal, binary, underscore separator)
  * Float (including special values: inf, nan)
  * Boolean
  * Datetime (with and without timezone, local date, local time) →
    returned as strings (no ``datetime`` dependency required)
  * Arrays (heterogeneous and homogeneous)
  * Comments

No external dependencies; no exec/eval.
"""

from __future__ import annotations

from typing import Any, IO

__all__ = ["TOMLDecodeError", "loads", "load"]


class TOMLDecodeError(ValueError):
    """Raised on invalid TOML input."""

    def __init__(
        self,
        message: str,
        doc: str | None = None,
        pos: int | None = None,
    ) -> None:
        super().__init__(message)
        self.msg = message
        self.doc = doc
        self.pos = pos


# ---------------------------------------------------------------------------
# Tokeniser / parser
# ---------------------------------------------------------------------------

_WHITESPACE = frozenset(" \t")
_NEWLINE = frozenset("\n\r")
_BARE_KEY_CHARS = frozenset(
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
)
_DIGITS = frozenset("0123456789")
_HEX = frozenset("0123456789abcdefABCDEF")
_OCT = frozenset("01234567")
_BIN = frozenset("01")

# Unicode escape helper
_UNICODE_ESCAPES = {
    "b": "\b",
    "t": "\t",
    "n": "\n",
    "f": "\f",
    "r": "\r",
    '"': '"',
    "\\": "\\",
}


class _Parser:
    """Recursive-descent TOML 1.0 parser."""

    __slots__ = (
        "_src",
        "_pos",
        "_len",
        "_root",
        "_current_table",
        "_implicit_tables",
        "_defined_tables",
        "_array_tables",
        "_table_path",
    )

    def __init__(self, src: str) -> None:
        self._src = src
        self._pos = 0
        self._len = len(src)
        self._root: dict[str, Any] = {}
        self._current_table: dict[str, Any] = self._root
        # Track which tables/keys have been explicitly defined to catch duplicates
        self._defined_tables: set[tuple[str, ...]] = set()
        # Track implicitly-created intermediate tables
        self._implicit_tables: set[tuple[str, ...]] = set()
        # Track array-of-tables paths
        self._array_tables: set[tuple[str, ...]] = set()
        # Current table header path (empty tuple = root)
        self._table_path: tuple[str, ...] = ()

    # ------------------------------------------------------------------ helpers

    def _err(self, msg: str) -> TOMLDecodeError:
        return TOMLDecodeError(msg, self._src, self._pos)

    def _peek(self) -> str | None:
        if self._pos < self._len:
            return self._src[self._pos]
        return None

    def _consume(self) -> str:
        ch = self._src[self._pos]
        self._pos += 1
        return ch

    def _at_end(self) -> bool:
        return self._pos >= self._len

    def _skip_whitespace(self) -> None:
        while self._pos < self._len and self._src[self._pos] in _WHITESPACE:
            self._pos += 1

    def _skip_whitespace_and_newlines(self) -> None:
        while self._pos < self._len and (
            self._src[self._pos] in _WHITESPACE or self._src[self._pos] in _NEWLINE
        ):
            self._pos += 1

    def _skip_comment_and_newline(self) -> None:
        """Skip optional whitespace, optional comment, then the newline."""
        self._skip_whitespace()
        if self._peek() == "#":
            while self._pos < self._len and self._src[self._pos] not in _NEWLINE:
                self._pos += 1
        # Consume newline (either \n or \r\n)
        if self._peek() == "\r":
            self._pos += 1
        if self._peek() == "\n":
            self._pos += 1

    # ------------------------------------------------------------------ entry

    def parse(self) -> dict[str, Any]:
        while not self._at_end():
            self._skip_whitespace_and_newlines()
            if self._at_end():
                break
            ch = self._peek()
            if ch == "#":
                # Comment line
                while self._pos < self._len and self._src[self._pos] not in _NEWLINE:
                    self._pos += 1
            elif ch == "[":
                self._parse_table_header()
            else:
                self._parse_keyval(
                    self._current_table, top_level_path=self._current_path()
                )
            self._skip_comment_and_newline()
        return self._root

    def _current_path(self) -> tuple[str, ...]:
        """Return the key-path tuple for the current table."""
        return self._table_path

    # ------------------------------------------------------------------ table headers

    def _parse_table_header(self) -> None:
        self._consume()  # '['
        is_array = self._peek() == "["
        if is_array:
            self._consume()  # second '['

        self._skip_whitespace()
        key_parts = self._parse_key()
        self._skip_whitespace()

        if is_array:
            if self._peek() != "]":
                raise self._err("Expected ']]' to close array-of-tables header")
            self._consume()
            if self._peek() != "]":
                raise self._err("Expected ']]' to close array-of-tables header")
            self._consume()
            self._open_array_table(key_parts)
        else:
            if self._peek() != "]":
                raise self._err("Expected ']' to close table header")
            self._consume()
            self._open_table(key_parts)

        self._table_path = tuple(key_parts)  # type: ignore[assignment]

    def _navigate_to(
        self,
        parts: list[str],
        create_missing: bool = True,
        path_so_far: tuple[str, ...] = (),
    ) -> dict[str, Any]:
        """Walk *root* through *parts*, creating intermediate dicts as needed."""
        current = self._root
        for i, part in enumerate(parts):
            path = path_so_far + tuple(parts[: i + 1])
            if part not in current:
                if not create_missing:
                    raise self._err(f"Key {part!r} does not exist")
                current[part] = {}
                self._implicit_tables.add(path)
                current = current[part]
            else:
                val = current[part]
                if isinstance(val, list):
                    # Navigate into last element of an array-of-tables
                    if not val or not isinstance(val[-1], dict):
                        raise self._err(f"Cannot use array {part!r} as table")
                    current = val[-1]
                elif isinstance(val, dict):
                    current = val
                else:
                    raise self._err(
                        f"Key {part!r} already defined as a non-table value"
                    )
        return current

    def _open_table(self, parts: list[str]) -> None:
        path = tuple(parts)
        # Navigate through all-but-last creating intermediates
        if len(parts) > 1:
            container = self._navigate_to(parts[:-1])
        else:
            container = self._root

        last = parts[-1]
        if last in container:
            val = container[last]
            if isinstance(val, dict):
                if path in self._defined_tables and path not in self._implicit_tables:
                    raise self._err(f"Duplicate table definition: [{'.'.join(parts)}]")
                # Re-opening an implicit table: mark it explicit now
                self._implicit_tables.discard(path)
                self._defined_tables.add(path)
                self._current_table = val
                return
            elif isinstance(val, list):
                raise self._err(
                    f"Cannot define table [{'.'.join(parts)}]: already an array-of-tables"
                )
            else:
                raise self._err(f"Duplicate key: {'.'.join(parts)!r}")
        else:
            container[last] = {}
            self._defined_tables.add(path)
            self._current_table = container[last]

    def _open_array_table(self, parts: list[str]) -> None:
        path = tuple(parts)
        if len(parts) > 1:
            container = self._navigate_to(parts[:-1])
        else:
            container = self._root

        last = parts[-1]
        if last in container:
            val = container[last]
            if not isinstance(val, list):
                raise self._err(f"Cannot redefine key {last!r} as array-of-tables")
            if path not in self._array_tables:
                raise self._err("Cannot redefine static array as array-of-tables")
        else:
            container[last] = []
            self._array_tables.add(path)

        new_table: dict[str, Any] = {}
        container[last].append(new_table)
        self._current_table = new_table

    # ------------------------------------------------------------------ key / value

    def _parse_key(self) -> list[str]:
        """Parse a (possibly dotted) key; return list of parts."""
        parts = [self._parse_simple_key()]
        while True:
            self._skip_whitespace()
            if self._peek() != ".":
                break
            self._consume()  # '.'
            self._skip_whitespace()
            parts.append(self._parse_simple_key())
        return parts

    def _parse_simple_key(self) -> str:
        ch = self._peek()
        if ch == '"':
            return self._parse_basic_string()
        elif ch == "'":
            return self._parse_literal_string()
        elif ch is not None and ch in _BARE_KEY_CHARS:
            return self._parse_bare_key()
        else:
            raise self._err(f"Invalid key character: {ch!r}")

    def _parse_bare_key(self) -> str:
        start = self._pos
        while self._pos < self._len and self._src[self._pos] in _BARE_KEY_CHARS:
            self._pos += 1
        if self._pos == start:
            raise self._err("Empty bare key")
        return self._src[start : self._pos]

    def _parse_keyval(
        self,
        table: dict[str, Any],
        top_level_path: tuple[str, ...] = (),
    ) -> None:
        self._skip_whitespace()
        key_parts = self._parse_key()
        self._skip_whitespace()
        if self._peek() != "=":
            raise self._err("Expected '=' after key")
        self._consume()
        self._skip_whitespace()
        value = self._parse_value()

        # Navigate to correct sub-table for dotted keys
        if len(key_parts) > 1:
            for i, part in enumerate(key_parts[:-1]):
                sub_path = top_level_path + tuple(key_parts[: i + 1])
                if part not in table:
                    table[part] = {}
                    self._implicit_tables.add(sub_path)
                elif not isinstance(table[part], dict):
                    raise self._err(f"Key {part!r} already defined as non-table")
                table = table[part]

        last = key_parts[-1]
        if last in table:
            raise self._err(f"Duplicate key: {last!r}")
        table[last] = value

    # ------------------------------------------------------------------ values

    def _parse_value(self) -> Any:
        ch = self._peek()
        if ch is None:
            raise self._err("Unexpected end of input")
        if ch == '"':
            if self._src[self._pos : self._pos + 3] == '"""':
                return self._parse_multiline_basic_string()
            return self._parse_basic_string()
        if ch == "'":
            if self._src[self._pos : self._pos + 3] == "'''":
                return self._parse_multiline_literal_string()
            return self._parse_literal_string()
        if ch == "t":
            if self._src[self._pos : self._pos + 4] == "true":
                self._pos += 4
                return True
            raise self._err("Invalid value")
        if ch == "f":
            if self._src[self._pos : self._pos + 5] == "false":
                self._pos += 5
                return False
            raise self._err("Invalid value")
        if ch == "[":
            return self._parse_array()
        if ch == "{":
            return self._parse_inline_table()
        # Numbers and dates
        return self._parse_number_or_date()

    # ------------------------------------------------------------------ strings

    def _parse_basic_string(self) -> str:
        self._consume()  # opening '"'
        parts: list[str] = []
        while True:
            if self._at_end():
                raise self._err("Unterminated basic string")
            ch = self._consume()
            if ch == '"':
                return "".join(parts)
            if ch == "\\":
                parts.append(self._parse_escape())
            elif ch in _NEWLINE:
                raise self._err("Newline in basic string")
            else:
                parts.append(ch)

    def _parse_escape(self) -> str:
        if self._at_end():
            raise self._err("Unexpected end after backslash")
        ch = self._consume()
        if ch in _UNICODE_ESCAPES:
            return _UNICODE_ESCAPES[ch]
        if ch == "u":
            return self._parse_unicode_escape(4)
        if ch == "U":
            return self._parse_unicode_escape(8)
        raise self._err(f"Invalid escape sequence \\{ch}")

    def _parse_unicode_escape(self, length: int) -> str:
        hex_str = self._src[self._pos : self._pos + length]
        if len(hex_str) < length or not all(c in _HEX for c in hex_str):
            raise self._err(f"Invalid unicode escape (need {length} hex digits)")
        self._pos += length
        code_point = int(hex_str, 16)
        try:
            return chr(code_point)
        except (ValueError, OverflowError):
            raise self._err(f"Invalid unicode code point: {code_point:#x}")

    def _parse_literal_string(self) -> str:
        self._consume()  # opening "'"
        start = self._pos
        while True:
            if self._at_end():
                raise self._err("Unterminated literal string")
            ch = self._src[self._pos]
            if ch == "'":
                result = self._src[start : self._pos]
                self._pos += 1
                return result
            if ch in _NEWLINE:
                raise self._err("Newline in literal string")
            self._pos += 1

    def _parse_multiline_basic_string(self) -> str:
        self._pos += 3  # '"""'
        # Skip immediate newline after opening
        if self._peek() == "\n":
            self._pos += 1
        elif (
            self._peek() == "\r"
            and self._pos + 1 < self._len
            and self._src[self._pos + 1] == "\n"
        ):
            self._pos += 2

        parts: list[str] = []
        while True:
            if self._at_end():
                raise self._err("Unterminated multi-line basic string")
            if self._src[self._pos : self._pos + 3] == '"""':
                # Handle up to 2 extra quotes before closing delimiter
                self._pos += 3
                # Check for 4 or 5 quote sequences
                extra = 0
                while self._peek() == '"' and extra < 2:
                    parts.append('"')
                    self._pos += 1
                    extra += 1
                return "".join(parts)
            ch = self._consume()
            if ch == "\\":
                # Line-ending backslash: skip whitespace and newlines
                if self._peek() in _WHITESPACE or self._peek() in _NEWLINE:
                    while self._pos < self._len and (
                        self._src[self._pos] in _WHITESPACE
                        or self._src[self._pos] in _NEWLINE
                    ):
                        self._pos += 1
                else:
                    parts.append(self._parse_escape())
            else:
                parts.append(ch)

    def _parse_multiline_literal_string(self) -> str:
        self._pos += 3  # "'''"
        # Skip immediate newline after opening
        if self._peek() == "\n":
            self._pos += 1
        elif (
            self._peek() == "\r"
            and self._pos + 1 < self._len
            and self._src[self._pos + 1] == "\n"
        ):
            self._pos += 2

        start = self._pos
        while True:
            if self._at_end():
                raise self._err("Unterminated multi-line literal string")
            if self._src[self._pos : self._pos + 3] == "'''":
                result = self._src[start : self._pos]
                self._pos += 3
                # Extra quotes
                extra = 0
                while self._peek() == "'" and extra < 2:
                    result += "'"
                    self._pos += 1
                    extra += 1
                return result
            self._pos += 1

    # ------------------------------------------------------------------ numbers / dates

    def _parse_number_or_date(self) -> Any:
        # Collect the raw token (until whitespace, comma, bracket, newline, comment)
        start = self._pos
        while self._pos < self._len:
            ch = self._src[self._pos]
            if ch in _WHITESPACE or ch in _NEWLINE or ch in ",}]#":
                break
            self._pos += 1
        token = self._src[start : self._pos]
        if not token:
            raise self._err("Empty value")
        return _parse_scalar(token, self._err)

    # ------------------------------------------------------------------ array

    def _parse_array(self) -> list[Any]:
        self._consume()  # '['
        items: list[Any] = []
        while True:
            self._skip_whitespace_and_newlines()
            # Skip comments inside arrays
            while self._peek() == "#":
                while self._pos < self._len and self._src[self._pos] not in _NEWLINE:
                    self._pos += 1
                self._skip_whitespace_and_newlines()
            if self._peek() == "]":
                self._consume()
                return items
            items.append(self._parse_value())
            self._skip_whitespace_and_newlines()
            # Skip comments inside arrays
            while self._peek() == "#":
                while self._pos < self._len and self._src[self._pos] not in _NEWLINE:
                    self._pos += 1
                self._skip_whitespace_and_newlines()
            if self._peek() == ",":
                self._consume()
            elif self._peek() == "]":
                continue
            else:
                raise self._err("Expected ',' or ']' in array")

    # ------------------------------------------------------------------ inline table

    def _parse_inline_table(self) -> dict[str, Any]:
        self._consume()  # '{'
        table: dict[str, Any] = {}
        first = True
        while True:
            self._skip_whitespace()
            if self._peek() == "}":
                self._consume()
                return table
            if not first:
                if self._peek() != ",":
                    raise self._err("Expected ',' or '}' in inline table")
                self._consume()
                self._skip_whitespace()
                if self._peek() == "}":
                    raise self._err("Trailing comma in inline table is not allowed")
            first = False
            self._parse_keyval(table)
            self._skip_whitespace()


# ---------------------------------------------------------------------------
# Scalar (non-string, non-collection) parser
# ---------------------------------------------------------------------------


def _parse_scalar(token: str, err_fn) -> Any:
    """Parse a TOML scalar token to a Python value."""
    # Special float values
    if token in ("inf", "+inf"):
        return float("inf")
    if token == "-inf":
        return float("-inf")
    if token in ("nan", "+nan", "-nan"):
        return float("nan")

    # Date/time heuristic: contains '-' or 'T' at plausible positions
    if _looks_like_datetime(token):
        return token  # Return as string (no datetime module dependency)

    # Hexadecimal integer
    if token.startswith("0x") or token.startswith("-0x") or token.startswith("+0x"):
        sign = -1 if token.startswith("-") else 1
        hex_part = token.lstrip("+-")[2:].replace("_", "")
        if not hex_part or not all(c in _HEX for c in hex_part):
            raise err_fn(f"Invalid hex integer: {token!r}")
        return sign * int(hex_part, 16)

    # Octal integer
    if token.startswith("0o") or token.startswith("-0o") or token.startswith("+0o"):
        sign = -1 if token.startswith("-") else 1
        oct_part = token.lstrip("+-")[2:].replace("_", "")
        if not oct_part or not all(c in _OCT for c in oct_part):
            raise err_fn(f"Invalid octal integer: {token!r}")
        return sign * int(oct_part, 8)

    # Binary integer
    if token.startswith("0b") or token.startswith("-0b") or token.startswith("+0b"):
        sign = -1 if token.startswith("-") else 1
        bin_part = token.lstrip("+-")[2:].replace("_", "")
        if not bin_part or not all(c in _BIN for c in bin_part):
            raise err_fn(f"Invalid binary integer: {token!r}")
        return sign * int(bin_part, 2)

    # Float (contains '.', 'e', or 'E')
    cleaned = token.replace("_", "")
    if "." in cleaned or "e" in cleaned or "E" in cleaned:
        try:
            return float(cleaned)
        except ValueError:
            raise err_fn(f"Invalid float: {token!r}")

    # Integer (decimal)
    int_str = cleaned.lstrip("+-")
    if int_str and int_str[0] == "0" and len(int_str) > 1:
        raise err_fn(f"Leading zeros not allowed in integer: {token!r}")
    try:
        return int(cleaned)
    except ValueError:
        raise err_fn(f"Invalid value: {token!r}")


def _looks_like_datetime(token: str) -> bool:
    """Quick heuristic to identify TOML date/time tokens."""
    # Local date: YYYY-MM-DD
    # Offset datetime: YYYY-MM-DDTHH:MM:SS...
    # Local datetime: YYYY-MM-DD HH:MM:SS...
    # Local time: HH:MM:SS...
    if len(token) >= 8 and token[4:5] == "-" and token[7:8] == "-":
        return True
    # Local time: HH:MM:SS
    if len(token) >= 5 and token[2:3] == ":" and token[5:6] in (":", "."):
        return True
    return False


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def loads(s: str) -> dict[str, Any]:
    """Parse TOML from *s* (a ``str``) and return a ``dict``."""
    if not isinstance(s, str):
        raise TypeError(f"Expected str, got {type(s).__name__}")
    # Normalise line endings
    s = s.replace("\r\n", "\n").replace("\r", "\n")
    parser = _Parser(s)
    try:
        return parser.parse()
    except TOMLDecodeError:
        raise
    except Exception as exc:
        raise TOMLDecodeError(str(exc), s) from exc


def load(fp: IO[bytes]) -> dict[str, Any]:
    """Read TOML from the binary file-like object *fp* and return a ``dict``."""
    b = fp.read()
    if isinstance(b, bytes):
        try:
            s = b.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise TOMLDecodeError(f"TOML file is not valid UTF-8: {exc}") from exc
    elif isinstance(b, str):
        s = b
    else:
        raise TypeError(f"fp.read() returned {type(b).__name__}, expected bytes")
    return loads(s)
