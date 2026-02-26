"""CSV reader/writer implementation for Molt."""

from __future__ import annotations
from typing import Iterable, Iterator, cast

from _intrinsics import require_intrinsic as _require_intrinsic

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

_MOLT_CSV_RUNTIME_READY = _require_intrinsic("molt_csv_runtime_ready", globals())
_MOLT_CSV_QUOTE_MINIMAL = _require_intrinsic("molt_csv_quote_minimal", globals())
_MOLT_CSV_QUOTE_ALL = _require_intrinsic("molt_csv_quote_all", globals())
_MOLT_CSV_QUOTE_NONNUMERIC = _require_intrinsic("molt_csv_quote_nonnumeric", globals())
_MOLT_CSV_QUOTE_NONE = _require_intrinsic("molt_csv_quote_none", globals())
_MOLT_CSV_QUOTE_STRINGS = _require_intrinsic("molt_csv_quote_strings", globals())
_MOLT_CSV_QUOTE_NOTNULL = _require_intrinsic("molt_csv_quote_notnull", globals())
_MOLT_CSV_FIELD_SIZE_LIMIT = _require_intrinsic("molt_csv_field_size_limit", globals())
_MOLT_CSV_REGISTER_DIALECT = _require_intrinsic("molt_csv_register_dialect", globals())
_MOLT_CSV_UNREGISTER_DIALECT = _require_intrinsic(
    "molt_csv_unregister_dialect", globals()
)
_MOLT_CSV_LIST_DIALECTS = _require_intrinsic("molt_csv_list_dialects", globals())
_MOLT_CSV_GET_DIALECT = _require_intrinsic("molt_csv_get_dialect", globals())
_MOLT_CSV_READER_NEW = _require_intrinsic("molt_csv_reader_new", globals())
_MOLT_CSV_READER_PARSE_LINE = _require_intrinsic(
    "molt_csv_reader_parse_line", globals()
)
_MOLT_CSV_READER_DROP = _require_intrinsic("molt_csv_reader_drop", globals())
_MOLT_CSV_DICT_PROJECT = _require_intrinsic("molt_csv_dict_project", globals())
_MOLT_CSV_WRITER_NEW = _require_intrinsic("molt_csv_writer_new", globals())
_MOLT_CSV_WRITER_WRITEROW = _require_intrinsic("molt_csv_writer_writerow", globals())
_MOLT_CSV_WRITER_WRITEROWS = _require_intrinsic("molt_csv_writer_writerows", globals())
_MOLT_CSV_WRITER_DROP = _require_intrinsic("molt_csv_writer_drop", globals())
_MOLT_CSV_SNIFF = _require_intrinsic("molt_csv_sniff", globals())
_MOLT_CSV_HAS_HEADER = _require_intrinsic("molt_csv_has_header", globals())

_MOLT_CSV_RUNTIME_READY()

QUOTE_MINIMAL = int(_MOLT_CSV_QUOTE_MINIMAL())
QUOTE_ALL = int(_MOLT_CSV_QUOTE_ALL())
QUOTE_NONNUMERIC = int(_MOLT_CSV_QUOTE_NONNUMERIC())
QUOTE_NONE = int(_MOLT_CSV_QUOTE_NONE())
QUOTE_STRINGS = int(_MOLT_CSV_QUOTE_STRINGS())
QUOTE_NOTNULL = int(_MOLT_CSV_QUOTE_NOTNULL())


class Error(Exception):
    """CSV parsing error."""


def field_size_limit(new_limit: int | None = None) -> int:
    """Get or set the maximum field size."""
    if new_limit is None:
        return int(_MOLT_CSV_FIELD_SIZE_LIMIT(None))
    return int(_MOLT_CSV_FIELD_SIZE_LIMIT(int(new_limit)))


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
        delimiter = cast(str, overrides.get("delimiter", self.delimiter))
        quotechar = cast(str | None, overrides.get("quotechar", self.quotechar))
        escapechar = cast(str | None, overrides.get("escapechar", self.escapechar))
        doublequote = cast(bool, overrides.get("doublequote", self.doublequote))
        skipinitialspace = cast(
            bool, overrides.get("skipinitialspace", self.skipinitialspace)
        )
        lineterminator = cast(str, overrides.get("lineterminator", self.lineterminator))
        quoting = cast(int, overrides.get("quoting", self.quoting))
        strict = cast(bool, overrides.get("strict", self.strict))
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


_DIALECT_FMTPARAM_KEYS = frozenset(Dialect.__slots__)


def _validate_fmtparams(fmtparams: dict[str, object]) -> None:
    for key in fmtparams:
        if key not in _DIALECT_FMTPARAM_KEYS:
            raise TypeError(f"this function got an unexpected keyword argument {key!r}")


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
    if not isinstance(dialect.delimiter, str):
        raise TypeError(
            f'"delimiter" must be a unicode character, not {type(dialect.delimiter).__name__}'
        )
    if len(dialect.delimiter) != 1:
        raise TypeError(
            '"delimiter" must be a unicode character, '
            f"not a string of length {len(dialect.delimiter)}"
        )
    if dialect.quotechar is not None:
        if not isinstance(dialect.quotechar, str):
            raise TypeError(
                '"quotechar" must be a unicode character or None, '
                f"not {type(dialect.quotechar).__name__}"
            )
        if len(dialect.quotechar) != 1:
            raise TypeError(
                '"quotechar" must be a unicode character or None, '
                f"not a string of length {len(dialect.quotechar)}"
            )
    if dialect.escapechar is not None:
        if not isinstance(dialect.escapechar, str):
            raise TypeError(
                '"escapechar" must be a unicode character or None, '
                f"not {type(dialect.escapechar).__name__}"
            )
        if len(dialect.escapechar) != 1:
            raise TypeError(
                '"escapechar" must be a unicode character or None, '
                f"not a string of length {len(dialect.escapechar)}"
            )
    if not isinstance(dialect.lineterminator, str):
        raise TypeError(
            f'"lineterminator" must be a string, not {type(dialect.lineterminator).__name__}'
        )
    if dialect.quoting not in {
        QUOTE_MINIMAL,
        QUOTE_ALL,
        QUOTE_NONNUMERIC,
        QUOTE_NONE,
        QUOTE_STRINGS,
        QUOTE_NOTNULL,
    }:
        raise TypeError('bad "quoting" value')
    if dialect.quotechar is None and dialect.quoting != QUOTE_NONE:
        raise TypeError("quotechar must be set if quoting enabled")


def _resolve_dialect(dialect: object, fmtparams: dict[str, object]) -> Dialect:
    _validate_fmtparams(fmtparams)
    if isinstance(dialect, str):
        base = get_dialect(dialect)
    else:
        base = dialect
    resolved = _dialect_from_obj(base).clone(**fmtparams)
    _validate_dialect(resolved)
    return resolved


def _dialect_from_intrinsic(raw: object) -> Dialect:
    if not isinstance(raw, tuple) or len(raw) != 8:
        raise RuntimeError("csv intrinsic returned invalid dialect payload")
    (
        delimiter,
        quotechar,
        escapechar,
        doublequote,
        skipinitialspace,
        lineterminator,
        quoting,
        strict,
    ) = raw
    dialect = Dialect(
        delimiter=cast(str, delimiter),
        quotechar=cast(str | None, quotechar),
        escapechar=cast(str | None, escapechar),
        doublequote=bool(doublequote),
        skipinitialspace=bool(skipinitialspace),
        lineterminator=cast(str, lineterminator),
        quoting=int(quoting),
        strict=bool(strict),
    )
    _validate_dialect(dialect)
    return dialect


def _dialect_lookup_name(name: object) -> str:
    if isinstance(name, str):
        return name
    try:
        hash(name)
    except TypeError:
        typename = type(name).__name__
        raise TypeError(
            f"cannot use '{typename}' as a dict key (unhashable type: '{typename}')"
        ) from None
    raise Error("unknown dialect")


def register_dialect(
    name: str, dialect: object | None = None, **fmtparams: object
) -> None:
    if not isinstance(name, str):
        raise TypeError("dialect name must be a string")
    base: object = "excel" if dialect is None else dialect
    resolved = _resolve_dialect(base, fmtparams)
    _MOLT_CSV_REGISTER_DIALECT(
        name,
        resolved.delimiter,
        resolved.quotechar,
        resolved.escapechar,
        resolved.doublequote,
        resolved.skipinitialspace,
        resolved.lineterminator,
        resolved.quoting,
        resolved.strict,
    )


def unregister_dialect(name: str) -> None:
    lookup_name = _dialect_lookup_name(name)
    try:
        _MOLT_CSV_UNREGISTER_DIALECT(lookup_name)
    except ValueError as exc:
        if str(exc) == "unknown dialect":
            raise Error("unknown dialect") from None
        raise


def get_dialect(name: str) -> Dialect:
    lookup_name = _dialect_lookup_name(name)
    try:
        return _dialect_from_intrinsic(_MOLT_CSV_GET_DIALECT(lookup_name))
    except ValueError as exc:
        if str(exc) == "unknown dialect":
            raise Error("unknown dialect") from None
        raise


def list_dialects() -> list[str]:
    return [cast(str, item) for item in _MOLT_CSV_LIST_DIALECTS()]


def _iter_csvfile(csvfile: object) -> Iterator[str]:
    try:
        return iter(csvfile)  # type: ignore[arg-type]
    except TypeError:
        pass

    if hasattr(csvfile, "readline"):

        def _readline_iter() -> Iterator[str]:
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


def _normalize_row(row: Iterable[object]) -> list[object] | tuple[object, ...]:
    if isinstance(row, (list, tuple)):
        return row
    try:
        return list(row)
    except TypeError:
        typename = type(row).__name__
        raise Error(f"iterable expected, not {typename}") from None


def reader(*args: object, **fmtparams: object):
    if not args:
        raise TypeError("reader() missing required argument 'csvfile'")
    if len(args) > 2:
        raise TypeError("reader() takes at most 2 positional arguments")
    csvfile = args[0]
    if "dialect" in fmtparams:
        if len(args) == 2:
            raise TypeError("dialect specified both positionally and as keyword")
        dialect = fmtparams.pop("dialect")
    else:
        dialect = args[1] if len(args) == 2 else "excel"
    resolved = _resolve_dialect(dialect, fmtparams)
    return _Reader(_iter_csvfile(csvfile), resolved)


def writer(*args: object, **fmtparams: object):
    if not args:
        raise TypeError("writer() missing required argument 'csvfile'")
    if len(args) > 2:
        raise TypeError("writer() takes at most 2 positional arguments")
    csvfile = args[0]
    if "dialect" in fmtparams:
        if len(args) == 2:
            raise TypeError("dialect specified both positionally and as keyword")
        dialect = fmtparams.pop("dialect")
    else:
        dialect = args[1] if len(args) == 2 else "excel"
    resolved = _resolve_dialect(dialect, fmtparams)
    return _Writer(csvfile, resolved)


class _Reader:
    def __init__(self, csvfile: Iterable[str], dialect: Dialect) -> None:
        self.dialect = dialect
        self._iter = iter(csvfile)
        self._pending = ""
        self._eof = False
        self.line_num = 0
        self._handle = _MOLT_CSV_READER_NEW(
            dialect.delimiter,
            dialect.quotechar,
            dialect.escapechar,
            dialect.doublequote,
            dialect.skipinitialspace,
            dialect.quoting,
            dialect.strict,
        )

    def __iter__(self) -> Iterator[list[object]]:
        return self

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        try:
            _MOLT_CSV_READER_DROP(handle)
        except Exception:
            pass

    def _next_physical_line(self) -> str | None:
        if self._eof:
            return None
        try:
            line = next(self._iter)
        except StopIteration:
            self._eof = True
            return None
        if not isinstance(line, str):
            typename = type(line).__name__
            raise Error(
                f"iterator should return strings, not {typename} "
                "(the file should be opened in text mode)"
            )
        self.line_num += 1
        return line

    def __next__(self) -> list[object]:
        while True:
            line = self._next_physical_line()
            if line is not None:
                self._pending += line

            if not self._pending:
                raise StopIteration

            try:
                row = _MOLT_CSV_READER_PARSE_LINE(self._handle, self._pending)
            except ValueError as exc:
                msg = str(exc)
                if msg == "unexpected end of data" and not self._eof:
                    continue
                raise Error(msg) from None

            self._pending = ""
            return cast(list[object], row)


class _Writer:
    def __init__(self, csvfile, dialect: Dialect) -> None:
        self.dialect = dialect
        self._csvfile = csvfile
        self._handle = _MOLT_CSV_WRITER_NEW(
            dialect.delimiter,
            dialect.quotechar,
            dialect.escapechar,
            dialect.doublequote,
            dialect.quoting,
            dialect.lineterminator,
        )

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        try:
            _MOLT_CSV_WRITER_DROP(handle)
        except Exception:
            pass

    def writerow(self, row: Iterable[object]) -> int:
        normalized = _normalize_row(row)
        try:
            record = _MOLT_CSV_WRITER_WRITEROW(self._handle, normalized)
        except ValueError as exc:
            raise Error(str(exc)) from None
        return self._csvfile.write(record)

    def writerows(self, rows: Iterable[Iterable[object]]) -> None:
        if isinstance(rows, (list, tuple)):
            normalized_rows = [_normalize_row(row) for row in rows]
            try:
                records = _MOLT_CSV_WRITER_WRITEROWS(self._handle, normalized_rows)
            except ValueError as exc:
                raise Error(str(exc)) from None
            self._csvfile.write(records)
            return
        for row in rows:
            self.writerow(row)


class DictReader:
    def __init__(
        self,
        csvfile: Iterable[str],
        fieldnames: Iterable[object] | None = None,
        restkey: str | None = None,
        restval: object | None = None,
        dialect: object = "excel",
        **fmtparams: object,
    ) -> None:
        if fieldnames is not None and iter(fieldnames) is fieldnames:
            fieldnames = list(fieldnames)
        self._fieldnames = fieldnames
        self.restkey = restkey
        self.restval = restval
        self.reader = reader(csvfile, dialect=dialect, **fmtparams)
        self.dialect = dialect
        self.line_num = 0

    def __iter__(self):
        return self

    @property
    def fieldnames(self) -> Iterable[object] | None:
        if self._fieldnames is None:
            try:
                self._fieldnames = next(self.reader)
            except StopIteration:
                pass
        self.line_num = self.reader.line_num
        return self._fieldnames

    @fieldnames.setter
    def fieldnames(self, value: Iterable[object] | None) -> None:
        self._fieldnames = value

    def __next__(self) -> dict[object, object]:
        if self.line_num == 0:
            # Side effect: prime header from first row when fieldnames omitted.
            self.fieldnames
        row = next(self.reader)
        self.line_num = self.reader.line_num
        while row == []:
            row = next(self.reader)
            self.line_num = self.reader.line_num
        raw_fieldnames = self.fieldnames
        assert raw_fieldnames is not None
        fieldnames = list(raw_fieldnames)
        return cast(
            dict[object, object],
            _MOLT_CSV_DICT_PROJECT(fieldnames, row, self.restkey, self.restval),
        )


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
        self.extrasaction = extrasaction.lower()
        if self.extrasaction not in {"raise", "ignore"}:
            raise ValueError(
                f"extrasaction ({extrasaction}) must be 'raise' or 'ignore'"
            )
        self._writer = writer(csvfile, dialect=dialect, **fmtparams)

    def writeheader(self) -> int:
        return self.writerow({name: name for name in self.fieldnames})

    def writerow(self, rowdict: dict[str, object]) -> int:
        extras = set(rowdict.keys()) - set(self.fieldnames)
        if extras:
            if self.extrasaction == "raise":
                raise ValueError("dict contains fields not in fieldnames")
        row = [rowdict.get(name, self.restval) for name in self.fieldnames]
        return self._writer.writerow(row)

    def writerows(self, rows: Iterable[dict[str, object]]) -> None:
        for row in rows:
            self.writerow(row)


class Sniffer:
    def sniff(self, sample: str, delimiters: str | None = None) -> Dialect:
        delimiter, doublequote, quotechar, skipinitialspace = _MOLT_CSV_SNIFF(
            sample, delimiters
        )
        return excel.clone(
            delimiter=cast(str, delimiter),
            quotechar=cast(str | None, quotechar),
            doublequote=bool(doublequote),
            skipinitialspace=bool(skipinitialspace),
        )

    def has_header(self, sample: str) -> bool:
        try:
            return bool(_MOLT_CSV_HAS_HEADER(sample))
        except ValueError as exc:
            raise Error(str(exc)) from None
