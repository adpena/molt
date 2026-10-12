"""CSV reader/writer implementation for Molt — fully intrinsic-backed."""

from __future__ import annotations
from typing import Iterable, Iterator, cast
import sys as _sys

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

_MOLT_CSV_QUOTE_MINIMAL = _require_intrinsic("molt_csv_quote_minimal")
_MOLT_CSV_QUOTE_ALL = _require_intrinsic("molt_csv_quote_all")
_MOLT_CSV_QUOTE_NONNUMERIC = _require_intrinsic("molt_csv_quote_nonnumeric")
_MOLT_CSV_QUOTE_NONE = _require_intrinsic("molt_csv_quote_none")
_MOLT_CSV_QUOTE_STRINGS = _require_intrinsic("molt_csv_quote_strings")
_MOLT_CSV_QUOTE_NOTNULL = _require_intrinsic("molt_csv_quote_notnull")
_MOLT_CSV_FIELD_SIZE_LIMIT = _require_intrinsic("molt_csv_field_size_limit")
_MOLT_CSV_REGISTER_DIALECT = _require_intrinsic("molt_csv_register_dialect")
_MOLT_CSV_UNREGISTER_DIALECT = _require_intrinsic("molt_csv_unregister_dialect")
_MOLT_CSV_LIST_DIALECTS = _require_intrinsic("molt_csv_list_dialects")
_MOLT_CSV_GET_DIALECT = _require_intrinsic("molt_csv_get_dialect")
_MOLT_CSV_READER_NEW = _require_intrinsic("molt_csv_reader_new")
_MOLT_CSV_READER_PARSE_LINE = _require_intrinsic("molt_csv_reader_parse_line")
_MOLT_CSV_READER_DROP = _require_intrinsic("molt_csv_reader_drop")
_MOLT_CSV_DICT_PROJECT = _require_intrinsic("molt_csv_dict_project")
_MOLT_CSV_WRITER_NEW = _require_intrinsic("molt_csv_writer_new")
_MOLT_CSV_WRITER_WRITEROW = _require_intrinsic("molt_csv_writer_writerow")
_MOLT_CSV_WRITER_WRITEROWS = _require_intrinsic("molt_csv_writer_writerows")
_MOLT_CSV_WRITER_DROP = _require_intrinsic("molt_csv_writer_drop")
_MOLT_CSV_SNIFF = _require_intrinsic("molt_csv_sniff")
_MOLT_CSV_HAS_HEADER = _require_intrinsic("molt_csv_has_header")
_MOLT_CSV_VALIDATE_FMTPARAMS = _require_intrinsic("molt_csv_validate_fmtparams")
_MOLT_CSV_VALIDATE_DIALECT = _require_intrinsic("molt_csv_validate_dialect")
_MOLT_CSV_NORMALIZE_ROW = _require_intrinsic("molt_csv_normalize_row")
_MOLT_CSV_DIALECT_LOOKUP_NAME = _require_intrinsic("molt_csv_dialect_lookup_name")


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


_DIALECT_UNSET = object()


class _Dialect:
    """Validated, immutable CSV settings shared by all runtime consumers."""

    __slots__ = ("_values",)
    __module__ = "_csv"

    def __new__(
        cls,
        dialect=None,
        delimiter=_DIALECT_UNSET,
        doublequote=_DIALECT_UNSET,
        escapechar=_DIALECT_UNSET,
        lineterminator=_DIALECT_UNSET,
        quotechar=_DIALECT_UNSET,
        quoting=_DIALECT_UNSET,
        skipinitialspace=_DIALECT_UNSET,
        strict=_DIALECT_UNSET,
        **fmtparams,
    ):
        # These are the public _csv.Dialect positional arguments. Missing
        # settings remain distinct from an explicitly supplied None.
        for name, value in (
            ("delimiter", delimiter),
            ("doublequote", doublequote),
            ("escapechar", escapechar),
            ("lineterminator", lineterminator),
            ("quotechar", quotechar),
            ("quoting", quoting),
            ("skipinitialspace", skipinitialspace),
            ("strict", strict),
        ):
            if value is not _DIALECT_UNSET:
                fmtparams[name] = value
        return _dialect_from_obj(dialect, fmtparams, cls)

    @property
    def delimiter(self):
        return self._values[0]

    @property
    def quotechar(self):
        return self._values[1]

    @property
    def escapechar(self):
        return self._values[2]

    @property
    def doublequote(self):
        return self._values[3]

    @property
    def skipinitialspace(self):
        return self._values[4]

    @property
    def lineterminator(self):
        return self._values[5]

    @property
    def quoting(self):
        return self._values[6]

    @property
    def strict(self):
        return self._values[7]

    def __setattr__(self, name, value):
        if name in ("doublequote", "skipinitialspace", "strict"):
            raise AttributeError("readonly attribute")
        if isinstance(getattr(_Dialect, name, None), property):
            raise AttributeError(
                f"attribute '{name}' of '_csv.Dialect' objects is not writable"
            )
        if name == "_values":
            raise AttributeError("readonly attribute")
        object.__setattr__(self, name, value)

    def __delattr__(self, name):
        if name in ("doublequote", "skipinitialspace", "strict"):
            raise AttributeError("readonly attribute")
        if isinstance(getattr(_Dialect, name, None), property):
            raise AttributeError(
                f"attribute '{name}' of '_csv.Dialect' objects is not writable"
            )
        if name == "_values":
            raise AttributeError("readonly attribute")
        object.__delattr__(self, name)


_Dialect.__name__ = "Dialect"
_Dialect.__qualname__ = "Dialect"


class Dialect:
    """Subclassable CSV configuration, validated without overwriting defaults."""

    _name = ""
    _valid = False
    delimiter = None
    quotechar = None
    escapechar = None
    doublequote = None
    skipinitialspace = None
    lineterminator = None
    quoting = None

    def __init__(self):
        if self.__class__ is not Dialect:
            self._valid = True
        self._validate()

    def _validate(self):
        try:
            _Dialect(self)
        except TypeError as exc:
            if _sys.version_info < (3, 13):
                raise Error(str(exc))
            raise Error(str(exc)) from None


class excel(Dialect):
    delimiter = ","
    quotechar = '"'
    doublequote = True
    skipinitialspace = False
    lineterminator = "\r\n"
    quoting = QUOTE_MINIMAL


class excel_tab(excel):
    delimiter = "\t"


class unix_dialect(Dialect):
    delimiter = ","
    quotechar = '"'
    doublequote = True
    skipinitialspace = False
    lineterminator = "\n"
    quoting = QUOTE_ALL


def _dialect_from_obj(
    obj: object, fmtparams: dict[str, object], cls: type = _Dialect
) -> _Dialect:
    _MOLT_CSV_VALIDATE_FMTPARAMS(list(fmtparams.keys()))
    if isinstance(obj, str):
        obj = get_dialect(obj)
    if isinstance(obj, _Dialect) and not fmtparams:
        return obj

    def setting(name, default):
        if name in fmtparams:
            return fmtparams[name]
        # CPython's DIALECT_GETATTR clears errors from optional attributes,
        # including errors other than AttributeError, before using a default.
        try:
            return getattr(obj, name)
        except BaseException:
            return default

    # Attribute lookup order follows _csv; conversion and validation have one
    # native owner. The returned tuple shares get_dialect's existing field order.
    delimiter = setting("delimiter", ",")
    doublequote = setting("doublequote", True)
    escapechar = setting("escapechar", None)
    lineterminator = setting("lineterminator", "\r\n")
    quotechar = setting("quotechar", '"')
    quoting = setting("quoting", _DIALECT_UNSET)
    skipinitialspace = setting("skipinitialspace", False)
    strict = setting("strict", False)
    quoting_omitted = quoting is _DIALECT_UNSET
    if quoting_omitted:
        quoting = QUOTE_MINIMAL
    values = _MOLT_CSV_VALIDATE_DIALECT(
        delimiter,
        quotechar,
        escapechar,
        doublequote,
        skipinitialspace,
        lineterminator,
        quoting,
        strict,
        quoting_omitted,
        _sys.version_info[1],
    )
    return _dialect_from_intrinsic(values, cls)


def _dialect_from_intrinsic(raw: object, cls: type = _Dialect) -> _Dialect:
    if not isinstance(raw, tuple) or len(raw) != 8:
        raise RuntimeError("csv intrinsic returned invalid dialect payload")
    dialect = object.__new__(cls)
    object.__setattr__(dialect, "_values", raw)
    return dialect


def _dialect_lookup_name(name: object) -> str:
    return str(_MOLT_CSV_DIALECT_LOOKUP_NAME(name))


def register_dialect(
    name: str, dialect: object | None = None, **fmtparams: object
) -> None:
    if not isinstance(name, str):
        raise TypeError("dialect name must be a string")
    resolved = _dialect_from_obj(dialect, fmtparams)
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


def get_dialect(name: str) -> _Dialect:
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
    result = _MOLT_CSV_NORMALIZE_ROW(row)
    if result is not None:
        return row  # type: ignore[return-value]
    # Fallback: try to convert to list
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
        dialect = args[1] if len(args) == 2 else None
    resolved = _dialect_from_obj(dialect, fmtparams)
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
        dialect = args[1] if len(args) == 2 else None
    resolved = _dialect_from_obj(dialect, fmtparams)
    return _Writer(csvfile, resolved)


class _Reader:
    def __init__(self, csvfile: Iterable[str], dialect: _Dialect) -> None:
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

    def __del__(self, _drop=_MOLT_CSV_READER_DROP) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        object.__setattr__(self, "_handle", None)
        try:
            _drop(handle)
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
                # Pass `self._eof` so the parser knows whether more physical
                # lines can still follow. While not at EOF, an unterminated
                # quoted field (a quote that opens but does not close before the
                # buffer ends — e.g. a field with an embedded newline) raises
                # "unexpected end of data"; we then read the next physical line
                # and re-parse the accumulated buffer. At EOF a non-strict
                # dialect instead commits the truncated field (no exception),
                # while a strict dialect still raises — matching CPython's `_csv`.
                row = _MOLT_CSV_READER_PARSE_LINE(
                    self._handle, self._pending, self._eof
                )
            except ValueError as exc:
                msg = str(exc)
                if msg == "unexpected end of data" and not self._eof:
                    continue
                raise Error(msg) from None

            self._pending = ""
            return cast(list[object], row)


class _Writer:
    def __init__(self, csvfile, dialect: _Dialect) -> None:
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

    def __del__(self, _drop=_MOLT_CSV_WRITER_DROP) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        object.__setattr__(self, "_handle", None)
        try:
            _drop(handle)
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
    def sniff(self, sample: str, delimiters: str | None = None) -> type[Dialect]:
        delimiter, doublequote, quotechar, skipinitialspace = _MOLT_CSV_SNIFF(
            sample, delimiters
        )

        class dialect(Dialect):
            _name = "sniffed"
            lineterminator = "\r\n"
            quoting = QUOTE_MINIMAL

        dialect.delimiter = delimiter
        dialect.quotechar = quotechar or '"'
        dialect.doublequote = doublequote
        dialect.skipinitialspace = skipinitialspace
        return dialect

    def has_header(self, sample: str) -> bool:
        try:
            return bool(_MOLT_CSV_HAS_HEADER(sample))
        except ValueError as exc:
            raise Error(str(exc)) from None


globals().pop("_require_intrinsic", None)
