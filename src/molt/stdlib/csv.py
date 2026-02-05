"""CSV reader/writer implementation for Molt."""

from __future__ import annotations

from typing import Iterable, Iterator

__all__ = [
    "Dialect",
    "Error",
    "DictReader",
    "DictWriter",
    "Sniffer",
    "QUOTE_ALL",
    "QUOTE_MINIMAL",
    "QUOTE_NONNUMERIC",
    "QUOTE_NONE",
    "QUOTE_STRINGS",
    "QUOTE_NOTNULL",
    "excel",
    "excel_tab",
    "unix_dialect",
    "reader",
    "writer",
    "register_dialect",
    "unregister_dialect",
    "get_dialect",
    "list_dialects",
    "field_size_limit",
]


QUOTE_MINIMAL = 0
QUOTE_ALL = 1
QUOTE_NONNUMERIC = 2
QUOTE_NONE = 3
QUOTE_STRINGS = 4
QUOTE_NOTNULL = 5


class Error(Exception):
    """CSV parsing error."""


_FIELD_SIZE_LIMIT = 131072


def field_size_limit(new_limit: int | None = None) -> int:
    """Get or set the maximum field size."""
    global _FIELD_SIZE_LIMIT
    old_limit = _FIELD_SIZE_LIMIT
    if new_limit is not None:
        _FIELD_SIZE_LIMIT = int(new_limit)
    return old_limit


class Dialect:
    __slots__ = (
        "delimiter",
        "quotechar",
        "escapechar",
        "doublequote",
        "skipinitialspace",
        "lineterminator",
        "quoting",
        "strict",
    )

    def __init__(
        self,
        delimiter: str = ",",
        quotechar: str | None = '"',
        escapechar: str | None = None,
        doublequote: bool = True,
        skipinitialspace: bool = False,
        lineterminator: str = "\r\n",
        quoting: int = QUOTE_MINIMAL,
        strict: bool = False,
    ) -> None:
        self.delimiter = delimiter
        self.quotechar = quotechar
        self.escapechar = escapechar
        self.doublequote = doublequote
        self.skipinitialspace = skipinitialspace
        self.lineterminator = lineterminator
        self.quoting = quoting
        self.strict = strict

    delimiter: str = ","
    quotechar: str | None = '"'
    escapechar: str | None = None
    doublequote: bool = True
    skipinitialspace: bool = False
    lineterminator: str = "\r\n"
    quoting: int = QUOTE_MINIMAL
    strict: bool = False

    def clone(self, **overrides: object) -> Dialect:
        delimiter = overrides.get("delimiter", self.delimiter)
        quotechar = overrides.get("quotechar", self.quotechar)
        escapechar = overrides.get("escapechar", self.escapechar)
        doublequote = overrides.get("doublequote", self.doublequote)
        skipinitialspace = overrides.get("skipinitialspace", self.skipinitialspace)
        lineterminator = overrides.get("lineterminator", self.lineterminator)
        quoting = overrides.get("quoting", self.quoting)
        strict = overrides.get("strict", self.strict)
        return Dialect(
            delimiter,
            quotechar,
            escapechar,
            doublequote,
            skipinitialspace,
            lineterminator,
            quoting,
            strict,
        )


excel = Dialect()
excel_tab = Dialect(delimiter="\t")
unix_dialect = Dialect(lineterminator="\n", quoting=QUOTE_ALL)


_dialects: dict[str, Dialect] = {
    "excel": excel,
    "excel-tab": excel_tab,
    "unix": unix_dialect,
}


def _dialect_from_obj(obj: object) -> Dialect:
    if isinstance(obj, Dialect):
        return obj.clone()
    return Dialect(
        delimiter=getattr(obj, "delimiter", ","),
        quotechar=getattr(obj, "quotechar", '"'),
        escapechar=getattr(obj, "escapechar", None),
        doublequote=getattr(obj, "doublequote", True),
        skipinitialspace=getattr(obj, "skipinitialspace", False),
        lineterminator=getattr(obj, "lineterminator", "\r\n"),
        quoting=getattr(obj, "quoting", QUOTE_MINIMAL),
        strict=getattr(obj, "strict", False),
    )


def _validate_dialect(dialect: Dialect) -> None:
    if not isinstance(dialect.delimiter, str) or len(dialect.delimiter) != 1:
        raise Error("delimiter must be a 1-character string")
    if dialect.quotechar is not None:
        if not isinstance(dialect.quotechar, str) or len(dialect.quotechar) != 1:
            raise Error("quotechar must be a 1-character string")
    if dialect.escapechar is not None:
        if not isinstance(dialect.escapechar, str) or len(dialect.escapechar) != 1:
            raise Error("escapechar must be a 1-character string")
    if dialect.quoting not in {
        QUOTE_MINIMAL,
        QUOTE_ALL,
        QUOTE_NONNUMERIC,
        QUOTE_NONE,
        QUOTE_STRINGS,
        QUOTE_NOTNULL,
    }:
        raise Error("unknown quoting value")
    if dialect.quotechar is None and dialect.quoting != QUOTE_NONE:
        raise Error("quotechar must be set unless QUOTE_NONE")


def _resolve_dialect(dialect: object, fmtparams: dict[str, object]) -> Dialect:
    if isinstance(dialect, str):
        if dialect not in _dialects:
            raise Error(f"unknown dialect {dialect!r}")
        base = _dialects[dialect]
    else:
        base = dialect
    resolved = _dialect_from_obj(base).clone(**fmtparams)
    _validate_dialect(resolved)
    return resolved


def register_dialect(
    name: str, dialect: object | None = None, **fmtparams: object
) -> None:
    if dialect is None:
        resolved = _resolve_dialect(excel, fmtparams)
    else:
        if fmtparams:
            raise TypeError("specify either a dialect or keyword arguments")
        resolved = _dialect_from_obj(dialect)
        _validate_dialect(resolved)
    _dialects[name] = resolved


def unregister_dialect(name: str) -> None:
    if name not in _dialects:
        raise Error(f"unknown dialect {name!r}")
    del _dialects[name]


def get_dialect(name: str) -> Dialect:
    if name not in _dialects:
        raise Error(f"unknown dialect {name!r}")
    return _dialects[name].clone()


def list_dialects() -> list[str]:
    return list(_dialects.keys())


def _is_number(value: object) -> bool:
    return isinstance(value, (int, float))


def _iter_csvfile(csvfile) -> Iterator[str]:
    try:
        return iter(csvfile)
    except TypeError:
        pass

    if hasattr(csvfile, "readline"):

        def _readline_iter():
            while True:
                line = csvfile.readline()
                if line == "":
                    return
                yield line

        return _readline_iter()

    if hasattr(csvfile, "read"):
        data = csvfile.read()
        if data == "":
            return iter(())
        return iter(data.splitlines(True))

    raise TypeError(f"{type(csvfile).__name__!r} object is not iterable")


def reader(*args: object, **fmtparams: object):
    if not args:
        raise TypeError("reader() missing required argument 'csvfile'")
    if len(args) > 2:
        raise TypeError("reader() takes at most 2 positional arguments")
    csvfile = args[0]
    dialect = args[1] if len(args) == 2 else "excel"
    resolved = _resolve_dialect(dialect, fmtparams)
    return _Reader(_iter_csvfile(csvfile), resolved)


def writer(*args: object, **fmtparams: object):
    if not args:
        raise TypeError("writer() missing required argument 'csvfile'")
    if len(args) > 2:
        raise TypeError("writer() takes at most 2 positional arguments")
    csvfile = args[0]
    dialect = args[1] if len(args) == 2 else "excel"
    resolved = _resolve_dialect(dialect, fmtparams)
    return _Writer(csvfile, resolved)


class _Reader:
    def __init__(self, csvfile: Iterable[str], dialect: Dialect) -> None:
        self.dialect = dialect
        self._iter = iter(csvfile)
        self._buffer = ""
        self._pos = 0
        self._eof = False
        self.line_num = 0

    def __iter__(self) -> Iterator[list[object]]:
        return self

    def _fill_buffer(self) -> bool:
        if self._eof:
            return False
        try:
            self._buffer = next(self._iter)
            self._pos = 0
            self.line_num += 1
            return True
        except StopIteration:
            self._eof = True
            return False

    def _next_char(self) -> str | None:
        while self._pos >= len(self._buffer):
            if not self._fill_buffer():
                return None
        ch = self._buffer[self._pos]
        self._pos += 1
        return ch

    def _peek_char(self) -> str | None:
        if self._pos < len(self._buffer):
            return self._buffer[self._pos]
        return None

    def __next__(self) -> list[object]:
        row: list[object] = []
        field_chars = []
        field_was_quoted = False
        in_quotes = False
        after_quote = False

        def finalize_field() -> None:
            nonlocal field_chars, field_was_quoted
            text = "".join(field_chars)
            field_chars = []
            was_quoted = field_was_quoted
            field_was_quoted = False
            if self.dialect.quoting == QUOTE_NONNUMERIC:
                if not was_quoted and text:
                    try:
                        value: object = float(text)
                    except ValueError as exc:
                        raise ValueError(str(exc)) from None
                    row.append(value)
                else:
                    row.append(text)
            else:
                row.append(text)

        def append_char(ch: str) -> None:
            field_chars.append(ch)
            if len(field_chars) > _FIELD_SIZE_LIMIT:
                raise Error("field larger than field limit")

        while True:
            ch = self._next_char()
            if ch is None:
                if in_quotes:
                    if self.dialect.strict:
                        raise Error("unexpected end of data")
                    in_quotes = False
                if field_chars or field_was_quoted or row:
                    finalize_field()
                    return row
                raise StopIteration

            if after_quote:
                if ch == self.dialect.delimiter:
                    finalize_field()
                    after_quote = False
                    continue
                if ch in ("\n", "\r"):
                    if ch == "\r" and self._peek_char() == "\n":
                        self._pos += 1
                    finalize_field()
                    after_quote = False
                    return row
                if self.dialect.strict:
                    raise Error(
                        f"{self.dialect.delimiter!r} expected after {self.dialect.quotechar!r}"
                    )
                append_char(ch)
                after_quote = False
                continue

            if in_quotes:
                if self.dialect.escapechar and ch == self.dialect.escapechar:
                    next_ch = self._next_char()
                    if next_ch is None:
                        if self.dialect.strict:
                            raise Error("unexpected end of data")
                        append_char(self.dialect.escapechar)
                    else:
                        append_char(next_ch)
                    continue
                if ch == self.dialect.quotechar:
                    if (
                        self.dialect.doublequote
                        and self._peek_char() == self.dialect.quotechar
                    ):
                        self._pos += 1
                        append_char(self.dialect.quotechar)
                    else:
                        in_quotes = False
                        after_quote = True
                    continue
                append_char(ch)
                continue

            if ch == self.dialect.delimiter:
                finalize_field()
                continue
            if ch in ("\n", "\r"):
                if ch == "\r" and self._peek_char() == "\n":
                    self._pos += 1
                if row or field_chars or field_was_quoted:
                    finalize_field()
                else:
                    row = []
                return row

            if (
                self.dialect.skipinitialspace
                and not field_chars
                and ch == " "
                and not field_was_quoted
            ):
                continue

            if self.dialect.quotechar and ch == self.dialect.quotechar:
                in_quotes = True
                field_was_quoted = True
                continue

            if self.dialect.escapechar and ch == self.dialect.escapechar:
                next_ch = self._next_char()
                if next_ch is None:
                    if self.dialect.strict:
                        raise Error("unexpected end of data")
                    append_char(self.dialect.escapechar)
                else:
                    append_char(next_ch)
                continue

            append_char(ch)


class _Writer:
    def __init__(self, csvfile, dialect: Dialect) -> None:
        self.dialect = dialect
        self._csvfile = csvfile

    def writerow(self, row: Iterable[object]) -> int:
        parts = []
        for field in row:
            is_none = field is None
            text = "" if is_none else str(field)
            quote_field = False

            if self.dialect.quoting == QUOTE_ALL:
                quote_field = True
            elif self.dialect.quoting == QUOTE_NONNUMERIC:
                quote_field = not _is_number(field)
            elif self.dialect.quoting == QUOTE_STRINGS:
                quote_field = isinstance(field, str)
            elif self.dialect.quoting == QUOTE_NOTNULL:
                quote_field = not is_none
            elif self.dialect.quoting == QUOTE_MINIMAL:
                if (
                    self.dialect.delimiter in text
                    or "\n" in text
                    or "\r" in text
                    or (self.dialect.quotechar and self.dialect.quotechar in text)
                    or (self.dialect.skipinitialspace and text.startswith(" "))
                ):
                    quote_field = True
            elif self.dialect.quoting == QUOTE_NONE:
                quote_field = False

            if self.dialect.quoting == QUOTE_NONE:
                if self.dialect.escapechar is None:
                    if (
                        self.dialect.delimiter in text
                        or "\n" in text
                        or "\r" in text
                        or (self.dialect.quotechar and self.dialect.quotechar in text)
                    ):
                        raise Error("need escapechar when quoting=QUOTE_NONE")
                if self.dialect.escapechar:
                    escaped = []
                    for ch in text:
                        if ch in {
                            self.dialect.delimiter,
                            "\n",
                            "\r",
                            self.dialect.quotechar or "",
                            self.dialect.escapechar,
                        }:
                            escaped.append(self.dialect.escapechar)
                        escaped.append(ch)
                    text = "".join(escaped)
                parts.append(text)
                continue

            if quote_field:
                if not self.dialect.quotechar:
                    raise Error("quotechar must be set to quote fields")
                escaped = []
                for ch in text:
                    if ch == self.dialect.quotechar:
                        if self.dialect.doublequote:
                            escaped.append(self.dialect.quotechar)
                            escaped.append(self.dialect.quotechar)
                        elif self.dialect.escapechar:
                            escaped.append(self.dialect.escapechar)
                            escaped.append(ch)
                        else:
                            raise Error("need escapechar when doublequote=False")
                    else:
                        escaped.append(ch)
                text = f"{self.dialect.quotechar}{''.join(escaped)}{self.dialect.quotechar}"
            parts.append(text)

        record = self.dialect.delimiter.join(parts) + self.dialect.lineterminator
        return self._csvfile.write(record)

    def writerows(self, rows: Iterable[Iterable[object]]) -> None:
        for row in rows:
            self.writerow(row)


class DictReader:
    def __init__(
        self,
        csvfile: Iterable[str],
        fieldnames: list[str] | None = None,
        restkey: str | None = None,
        restval: object = "",
        dialect: object = "excel",
        **fmtparams: object,
    ) -> None:
        self._reader = reader(csvfile, dialect=dialect, **fmtparams)
        self.fieldnames = fieldnames
        self.restkey = restkey
        self.restval = restval

    def __iter__(self):
        return self

    def __next__(self) -> dict[str, object]:
        if self.fieldnames is None:
            header = next(self._reader)
            self.fieldnames = [str(name) for name in header]
        row = next(self._reader)
        mapping: dict[str, object] = {}
        assert self.fieldnames is not None
        for idx, name in enumerate(self.fieldnames):
            if idx < len(row):
                mapping[name] = row[idx]
            else:
                mapping[name] = self.restval
        if len(row) > len(self.fieldnames) and self.restkey is not None:
            mapping[self.restkey] = row[len(self.fieldnames) :]
        return mapping


class DictWriter:
    def __init__(
        self,
        csvfile,
        fieldnames: list[str],
        restval: object = "",
        extrasaction: str = "raise",
        dialect: object = "excel",
        **fmtparams: object,
    ) -> None:
        self.fieldnames = fieldnames
        self.restval = restval
        self.extrasaction = extrasaction
        self._writer = writer(csvfile, dialect=dialect, **fmtparams)

    def writeheader(self) -> int:
        return self.writerow({name: name for name in self.fieldnames})

    def writerow(self, rowdict: dict[str, object]) -> int:
        extras = set(rowdict.keys()) - set(self.fieldnames)
        if extras:
            action = self.extrasaction.lower()
            if action == "raise":
                raise ValueError("dict contains fields not in fieldnames")
            if action != "ignore":
                raise ValueError("extrasaction must be 'raise' or 'ignore'")
        row = [rowdict.get(name, self.restval) for name in self.fieldnames]
        return self._writer.writerow(row)

    def writerows(self, rows: Iterable[dict[str, object]]) -> None:
        for row in rows:
            self.writerow(row)


class Sniffer:
    def sniff(self, sample: str, delimiters: str | None = None) -> Dialect:
        if delimiters is None:
            delimiters = ",\t;|:"
        candidates = [delim for delim in delimiters if delim in sample]
        if not candidates:
            return excel.clone()
        best = candidates[0]
        best_count = sample.count(best)
        for delim in candidates[1:]:
            count = sample.count(delim)
            if count > best_count:
                best = delim
                best_count = count
        return excel.clone(delimiter=best)

    def has_header(self, sample: str) -> bool:
        lines = [line for line in sample.splitlines() if line]
        if len(lines) < 2:
            return False
        first = lines[0].split(",")
        second = lines[1].split(",")
        if len(first) != len(second):
            return False
        return all(not item.isdigit() for item in first)
