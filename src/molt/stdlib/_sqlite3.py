"""Low-level Python wrapper for the Molt SQLite3 driver.

CPython's `sqlite3` package imports `Connection`, `Cursor`, `Row`, the error
hierarchy, and the constants (`PARSE_DECLTYPES`, `PARSE_COLNAMES`,
`sqlite_version`, `sqlite_version_info`, `version`, `version_info`,
`apilevel`, `threadsafety`, `paramstyle`) from the C extension module
`_sqlite3`.  Higher-level Python files (`sqlite3/__init__.py` and
`sqlite3/dbapi2.py`) then re-export those names without modification.

We mirror that layout here: this module wraps the Rust intrinsics installed
by `runtime/molt-runtime/src/builtins/sqlite3.rs` into the DB-API 2.0
surface, and `sqlite3.dbapi2`/`sqlite3.__init__` re-export from this file.
"""

# fmt: off
# pylint: disable=all
# ruff: noqa

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_SQLITE3_CONNECT = _require_intrinsic("molt_sqlite3_connect")
_MOLT_SQLITE3_CLOSE = _require_intrinsic("molt_sqlite3_close")
_MOLT_SQLITE3_COMMIT = _require_intrinsic("molt_sqlite3_commit")
_MOLT_SQLITE3_ROLLBACK = _require_intrinsic("molt_sqlite3_rollback")
_MOLT_SQLITE3_IN_TRANSACTION = _require_intrinsic("molt_sqlite3_in_transaction")
_MOLT_SQLITE3_TOTAL_CHANGES = _require_intrinsic("molt_sqlite3_total_changes")
_MOLT_SQLITE3_CURSOR = _require_intrinsic("molt_sqlite3_cursor")
_MOLT_SQLITE3_CURSOR_CLOSE = _require_intrinsic("molt_sqlite3_cursor_close")
_MOLT_SQLITE3_CURSOR_DROP = _require_intrinsic("molt_sqlite3_cursor_drop")
_MOLT_SQLITE3_EXECUTE = _require_intrinsic("molt_sqlite3_execute")
_MOLT_SQLITE3_EXECUTEMANY = _require_intrinsic("molt_sqlite3_executemany")
_MOLT_SQLITE3_EXECUTESCRIPT = _require_intrinsic("molt_sqlite3_executescript")
_MOLT_SQLITE3_FETCHONE = _require_intrinsic("molt_sqlite3_fetchone")
_MOLT_SQLITE3_FETCHALL = _require_intrinsic("molt_sqlite3_fetchall")
_MOLT_SQLITE3_FETCHMANY = _require_intrinsic("molt_sqlite3_fetchmany")
_MOLT_SQLITE3_LASTROWID = _require_intrinsic("molt_sqlite3_lastrowid")
_MOLT_SQLITE3_ROWCOUNT = _require_intrinsic("molt_sqlite3_rowcount")
_MOLT_SQLITE3_ARRAYSIZE_GET = _require_intrinsic("molt_sqlite3_arraysize_get")
_MOLT_SQLITE3_ARRAYSIZE_SET = _require_intrinsic("molt_sqlite3_arraysize_set")
_MOLT_SQLITE3_DESCRIPTION = _require_intrinsic("molt_sqlite3_description")
_MOLT_SQLITE3_LIBRARY_VERSION = _require_intrinsic("molt_sqlite3_library_version")
_MOLT_SQLITE3_COMPLETE_STATEMENT = _require_intrinsic("molt_sqlite3_complete_statement")


__all__ = [
    # Connect/factory
    "connect",
    "complete_statement",
    "Connection",
    "Cursor",
    "Row",
    # DB-API exception hierarchy
    "Warning",
    "Error",
    "InterfaceError",
    "DatabaseError",
    "DataError",
    "OperationalError",
    "IntegrityError",
    "InternalError",
    "ProgrammingError",
    "NotSupportedError",
    # Constants
    "apilevel",
    "paramstyle",
    "threadsafety",
    "sqlite_version",
    "sqlite_version_info",
    "version",
    "version_info",
    "PARSE_DECLTYPES",
    "PARSE_COLNAMES",
    "SQLITE_OK",
    "SQLITE_DENY",
    "SQLITE_IGNORE",
]

# ---------------------------------------------------------------------------
# Module constants (DB-API 2.0 §10)
# ---------------------------------------------------------------------------

apilevel = "2.0"
paramstyle = "qmark"
# CPython's _sqlite3 publishes 1 (threads may share the module but not
# connections).  We match that contract.
threadsafety = 1

# Bitmask constants for ``Connection.detect_types``.
PARSE_DECLTYPES = 1
PARSE_COLNAMES = 2

# Authorizer return values (mirrors libsqlite3's SQLITE_OK / DENY / IGNORE).
SQLITE_OK = 0
SQLITE_DENY = 1
SQLITE_IGNORE = 2

# Underlying SQLite library version, queried via the runtime intrinsic.
sqlite_version = _MOLT_SQLITE3_LIBRARY_VERSION()
sqlite_version_info = tuple(int(part) for part in sqlite_version.split("."))

# pysqlite "module" version.  CPython hard-codes "2.6.0" for compatibility
# with anything that used to inspect it; we follow suit so library code that
# checks ``sqlite3.version >= "2.6.0"`` continues to work.  CPython has
# deprecated this constant; users should prefer ``sqlite_version``.
version = "2.6.0"
version_info = (2, 6, 0)
_deprecated_version = version
_deprecated_version_info = version_info


# ---------------------------------------------------------------------------
# Exception hierarchy (DB-API 2.0 §5)
# ---------------------------------------------------------------------------

class Warning(Exception):  # noqa: N818 — DB-API 2.0 mandates this name
    """Important warnings such as truncation while inserting."""


class Error(Exception):
    """Base class for all sqlite3-related errors."""


class InterfaceError(Error):
    """Errors related to the database interface itself."""


class DatabaseError(Error):
    """Errors related to the database."""


class DataError(DatabaseError):
    """Errors due to problems with processed data (out of range, ...)."""


class OperationalError(DatabaseError):
    """Errors related to the database's operation, often outside our control."""


class IntegrityError(DatabaseError):
    """Errors when the relational integrity of the database is affected."""


class InternalError(DatabaseError):
    """Internal database errors (cursor not valid, transaction out of sync)."""


class ProgrammingError(DatabaseError):
    """Programming errors — bad SQL, wrong number of parameters, etc."""


class NotSupportedError(DatabaseError):
    """A method or API was used that is not supported by the database."""


# Mapping from prefix tokens emitted by the Rust driver to DB-API exception
# subclasses.  The driver formats every error as ``"__Kind__ message"`` so
# the Python wrapper can pick the right subclass without relying on a
# richer exception ABI.
_ERROR_PREFIX_MAP = {
    "__Warning__": Warning,
    "__InterfaceError__": InterfaceError,
    "__DatabaseError__": DatabaseError,
    "__DataError__": DataError,
    "__OperationalError__": OperationalError,
    "__IntegrityError__": IntegrityError,
    "__InternalError__": InternalError,
    "__ProgrammingError__": ProgrammingError,
    "__NotSupportedError__": NotSupportedError,
}


def _translate_runtime_error(exc: BaseException) -> BaseException:
    """Map a ``RuntimeError`` raised by an intrinsic into the proper
    DB-API exception subclass, leaving other exception types untouched.

    The Rust driver formats messages as ``"<__Kind__> <message>"`` so the
    Python wrapper can route them into the right subclass without any
    extra runtime metadata.  When the prefix is missing we conservatively
    fall back to ``OperationalError`` — the parent class of most runtime
    failures, matching CPython's fallback for unclassified errors.
    """
    if not isinstance(exc, RuntimeError):
        return exc
    text = str(exc)
    for prefix, cls in _ERROR_PREFIX_MAP.items():
        if text.startswith(prefix):
            stripped = text[len(prefix):].lstrip()
            return cls(stripped or text)
    return OperationalError(text)


def _call_intrinsic(fn, *args):
    """Call a runtime intrinsic and re-raise any RuntimeError as the proper
    DB-API exception subclass.  Used for every driver call so that callers
    see ``IntegrityError``/``ProgrammingError``/etc. exactly the way they
    would from CPython's pysqlite."""
    try:
        return fn(*args)
    except RuntimeError as exc:
        raise _translate_runtime_error(exc) from None


# ---------------------------------------------------------------------------
# Row factory
# ---------------------------------------------------------------------------

class Row:
    """DB-API 2.0 row helper.

    A ``Row`` is a thin wrapper over a tuple that adds case-insensitive
    column name lookup and ``keys()``.  CPython's ``sqlite3.Row`` is a C
    type with the same surface; we provide a pure-Python equivalent that
    is constructible from ``Row(cursor, tuple_of_values)`` to match the
    pysqlite contract.
    """

    __slots__ = ("_data", "_columns")

    def __init__(self, cursor: "Cursor", data: tuple):
        if not isinstance(data, tuple):
            data = tuple(data)
        # Cursor.description is a tuple of 7-tuples (or None).  Extract just
        # the column names — case-insensitive lookup is always against the
        # exact name stored here.
        desc = cursor.description
        if desc is None:
            columns = ()
        else:
            columns = tuple(entry[0] for entry in desc)
        if len(columns) != len(data):
            # CPython tolerates mismatches by clipping to the data length;
            # we do the same so callers can build Rows from arbitrary tuples
            # without worrying about description state.
            columns = columns[: len(data)]
        self._data = data
        self._columns = columns

    def __len__(self) -> int:
        return len(self._data)

    def __iter__(self):
        return iter(self._data)

    def __contains__(self, item) -> bool:
        return item in self._data

    def __eq__(self, other) -> bool:
        if isinstance(other, Row):
            return self._data == other._data
        if isinstance(other, tuple):
            return self._data == other
        return NotImplemented

    def __ne__(self, other) -> bool:
        result = self.__eq__(other)
        if result is NotImplemented:
            return result
        return not result

    def __hash__(self) -> int:
        return hash((tuple(c.lower() for c in self._columns), self._data))

    def __repr__(self) -> str:
        return f"<Row {self._data!r}>"

    def __getitem__(self, key):
        if isinstance(key, int):
            return self._data[key]
        if isinstance(key, slice):
            return self._data[key]
        if isinstance(key, str):
            lk = key.lower()
            for idx, name in enumerate(self._columns):
                if name.lower() == lk:
                    return self._data[idx]
            raise IndexError(f"No item with that key: {key!r}")
        raise IndexError(f"Row indices must be integers or strings, not {type(key).__name__}")

    def keys(self) -> list:
        return list(self._columns)


# ---------------------------------------------------------------------------
# Cursor
# ---------------------------------------------------------------------------

class Cursor:
    """DB-API 2.0 cursor backed by a runtime cursor handle."""

    __slots__ = (
        "_handle",
        "_connection",
        "_closed",
        "row_factory",
        "_arraysize_local",
    )

    def __init__(self, connection: "Connection") -> None:
        self._connection = connection
        self._handle = _call_intrinsic(_MOLT_SQLITE3_CURSOR, connection._handle)
        self._closed = False
        # `row_factory` mirrors pysqlite: when set, every returned row is
        # post-processed via ``row_factory(cursor, row)``.  Defaults to None
        # (return raw tuples), which matches CPython.
        self.row_factory = None
        self._arraysize_local = 1

    # -- DB-API properties -------------------------------------------------

    @property
    def connection(self) -> "Connection":
        return self._connection

    @property
    def description(self):
        if self._closed:
            return None
        return _call_intrinsic(_MOLT_SQLITE3_DESCRIPTION, self._handle)

    @property
    def rowcount(self) -> int:
        if self._closed:
            return -1
        return _call_intrinsic(_MOLT_SQLITE3_ROWCOUNT, self._handle)

    @property
    def lastrowid(self):
        if self._closed:
            return None
        return _call_intrinsic(_MOLT_SQLITE3_LASTROWID, self._handle)

    @property
    def arraysize(self) -> int:
        return self._arraysize_local

    @arraysize.setter
    def arraysize(self, value: int) -> None:
        if not isinstance(value, int):
            raise TypeError("arraysize must be an integer")
        if value < 1:
            raise ValueError("arraysize must be >= 1")
        self._arraysize_local = value
        if not self._closed:
            _call_intrinsic(_MOLT_SQLITE3_ARRAYSIZE_SET, self._handle, value)

    # -- Lifecycle ---------------------------------------------------------

    def close(self) -> None:
        if not self._closed:
            _call_intrinsic(_MOLT_SQLITE3_CURSOR_CLOSE, self._handle)
            self._closed = True

    def __del__(self) -> None:
        # Best-effort cleanup — drop the runtime handle entirely so the
        # thread-local map does not leak across long-running sessions.
        if not self._closed:
            try:
                _MOLT_SQLITE3_CURSOR_DROP(self._handle)
            except Exception:
                # __del__ must never raise; the runtime side already
                # tolerates double-drop / unknown handles silently.
                pass
            self._closed = True

    def _check_open(self) -> None:
        if self._closed:
            raise ProgrammingError("Cannot operate on a closed cursor.")
        if self._connection._closed:
            raise ProgrammingError("Cannot operate on a closed database.")

    # -- Statement execution ----------------------------------------------

    def execute(self, sql: str, parameters=()) -> "Cursor":
        self._check_open()
        params = _coerce_params(parameters)
        _call_intrinsic(_MOLT_SQLITE3_EXECUTE, self._handle, sql, params)
        return self

    def executemany(self, sql: str, seq_of_parameters) -> "Cursor":
        self._check_open()
        seq = _coerce_params_seq(seq_of_parameters)
        _call_intrinsic(_MOLT_SQLITE3_EXECUTEMANY, self._handle, sql, seq)
        return self

    def executescript(self, sql_script: str) -> "Cursor":
        self._check_open()
        if not isinstance(sql_script, (str, bytes)):
            raise ValueError("script argument must be unicode")
        if isinstance(sql_script, bytes):
            sql_script = sql_script.decode("utf-8")
        _call_intrinsic(_MOLT_SQLITE3_EXECUTESCRIPT, self._handle, sql_script)
        return self

    # -- Result iteration --------------------------------------------------

    def _wrap_row(self, row):
        if row is None:
            return None
        factory = self.row_factory
        if factory is None:
            return row
        return factory(self, row)

    def fetchone(self):
        self._check_open()
        row = _call_intrinsic(_MOLT_SQLITE3_FETCHONE, self._handle)
        return self._wrap_row(row)

    def fetchmany(self, size: int | None = None):
        self._check_open()
        if size is None:
            size = self._arraysize_local
        rows = _call_intrinsic(_MOLT_SQLITE3_FETCHMANY, self._handle, int(size))
        if self.row_factory is None:
            return rows
        return [self.row_factory(self, r) for r in rows]

    def fetchall(self):
        self._check_open()
        rows = _call_intrinsic(_MOLT_SQLITE3_FETCHALL, self._handle)
        if self.row_factory is None:
            return rows
        return [self.row_factory(self, r) for r in rows]

    # -- Iterator protocol -------------------------------------------------

    def __iter__(self) -> "Cursor":
        return self

    def __next__(self):
        row = self.fetchone()
        if row is None:
            raise StopIteration
        return row

    # -- Context manager (CPython parity) ---------------------------------

    def __enter__(self) -> "Cursor":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()

    # -- DB-API placeholders ----------------------------------------------

    def setinputsizes(self, sizes) -> None:
        # Implementations may ignore this per DB-API 2.0; pysqlite does too.
        return None

    def setoutputsize(self, size, column=None) -> None:
        return None


# ---------------------------------------------------------------------------
# Connection
# ---------------------------------------------------------------------------


class Connection:
    """DB-API 2.0 connection backed by a runtime connection handle."""

    __slots__ = (
        "_handle",
        "_closed",
        "row_factory",
        "text_factory",
        "isolation_level",
        "_database",
    )

    def __init__(self, database: str) -> None:
        self._database = database
        self._handle = _call_intrinsic(_MOLT_SQLITE3_CONNECT, database)
        self._closed = False
        self.row_factory = None
        self.text_factory = str
        # CPython's default is "" (deferred-style implicit transactions);
        # we honor that.  Setting to None enables full autocommit, which
        # we expose through ``isolation_level=None`` (no implicit BEGIN).
        self.isolation_level = ""

    # -- Lifecycle ---------------------------------------------------------

    def close(self) -> None:
        if not self._closed:
            try:
                _call_intrinsic(_MOLT_SQLITE3_CLOSE, self._handle)
            finally:
                self._closed = True

    def __del__(self) -> None:
        if not self._closed:
            try:
                _MOLT_SQLITE3_CLOSE(self._handle)
            except Exception:
                pass
            self._closed = True

    def _check_open(self) -> None:
        if self._closed:
            raise ProgrammingError("Cannot operate on a closed database.")

    # -- DB-API methods ---------------------------------------------------

    def cursor(self, factory: type | None = None) -> Cursor:
        self._check_open()
        cls = factory or Cursor
        cur = cls(self)
        if self.row_factory is not None and getattr(cur, "row_factory", None) is None:
            cur.row_factory = self.row_factory
        return cur

    def commit(self) -> None:
        self._check_open()
        _call_intrinsic(_MOLT_SQLITE3_COMMIT, self._handle)

    def rollback(self) -> None:
        self._check_open()
        _call_intrinsic(_MOLT_SQLITE3_ROLLBACK, self._handle)

    def execute(self, sql: str, parameters=()) -> Cursor:
        cur = self.cursor()
        return cur.execute(sql, parameters)

    def executemany(self, sql: str, seq_of_parameters) -> Cursor:
        cur = self.cursor()
        return cur.executemany(sql, seq_of_parameters)

    def executescript(self, sql_script: str) -> Cursor:
        cur = self.cursor()
        return cur.executescript(sql_script)

    @property
    def in_transaction(self) -> bool:
        if self._closed:
            return False
        return bool(_call_intrinsic(_MOLT_SQLITE3_IN_TRANSACTION, self._handle))

    @property
    def total_changes(self) -> int:
        self._check_open()
        return _call_intrinsic(_MOLT_SQLITE3_TOTAL_CHANGES, self._handle)

    # -- Context manager (CPython parity: commit on success, rollback on
    # exception, do NOT close the connection) ------------------------------

    def __enter__(self) -> "Connection":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        if exc_type is None:
            self.commit()
        else:
            self.rollback()
        # Do NOT close — pysqlite leaves the connection open after `with`.

    # -- DB-API exception attributes (per PEP 249 §5.4) -------------------
    # Allow `connection.OperationalError` access even if the user only
    # imported `sqlite3.connect` and not the exception classes.

    Warning = Warning
    Error = Error
    InterfaceError = InterfaceError
    DatabaseError = DatabaseError
    DataError = DataError
    OperationalError = OperationalError
    IntegrityError = IntegrityError
    InternalError = InternalError
    ProgrammingError = ProgrammingError
    NotSupportedError = NotSupportedError


# ---------------------------------------------------------------------------
# Module-level entry points
# ---------------------------------------------------------------------------


def _coerce_params(parameters) -> tuple:
    """Coerce a parameter sequence into a tuple, validating shape.

    ``None`` is *not* a valid bind sequence — but we do accept it for the
    common case ``cur.execute(sql)`` where the user omits parameters
    entirely.  Anything else must be a tuple/list (qmark style) or, in the
    future, a mapping (named-style — not yet implemented).
    """
    if parameters is None:
        return ()
    if isinstance(parameters, (tuple, list)):
        return tuple(parameters)
    if isinstance(parameters, dict):
        # Named parameters (`:foo`/`@foo`/`$foo`) are not yet supported by
        # this driver — fail closed with the precise DB-API exception so
        # callers can detect and adapt.
        raise NotSupportedError("named parameter binding is not yet supported")
    # Treat any other iterable as a sequence of values.
    try:
        return tuple(parameters)
    except TypeError as exc:
        raise ProgrammingError(
            "parameters must be a sequence (tuple/list)"
        ) from exc


def _coerce_params_seq(seq_of_parameters) -> list:
    if seq_of_parameters is None:
        return []
    out = []
    for params in seq_of_parameters:
        out.append(_coerce_params(params))
    return out


def connect(
    database,
    timeout: float = 5.0,
    detect_types: int = 0,
    isolation_level: str | None = "",
    check_same_thread: bool = True,
    factory: type | None = None,
    cached_statements: int = 128,
    uri: bool = False,
) -> Connection:
    """Open a connection to the SQLite database at *database*.

    The signature matches CPython 3.12's ``sqlite3.connect``.  Several
    parameters are currently accepted-but-not-honored (``timeout``,
    ``detect_types``, ``cached_statements``, ``check_same_thread``);
    raising on those would gratuitously break library compatibility.
    The ones we *do* honor:

    * ``database`` — path to the database file or ``:memory:`` for a
      transient in-memory database.
    * ``isolation_level`` — currently honored only at the "deferred" /
      ``""`` setting (the CPython default).  ``None`` (autocommit) is
      accepted but treated as deferred until the runtime grows explicit
      autocommit support.
    * ``factory`` — connection factory subclass.
    * ``uri`` — URI-style file paths are passed through to SQLite as-is.
    """
    if isinstance(database, bytes):
        database = database.decode("utf-8")
    elif not isinstance(database, (str,)):
        # PathLike → string.
        database = str(database)
    if uri and not database.startswith("file:"):
        database = "file:" + database
    cls = factory or Connection
    conn = cls(database)
    if isolation_level is None:
        # Best-effort autocommit — set the field but the underlying driver
        # currently still implicitly opens transactions on first DML.  Users
        # depending on strict autocommit must run an explicit ``BEGIN``-less
        # workflow with ``conn.commit()`` after each statement.
        conn.isolation_level = None
    else:
        conn.isolation_level = isolation_level
    return conn


def complete_statement(sql: str) -> bool:
    """Return ``True`` if *sql* appears to contain at least one complete
    SQL statement (terminating semicolon, balanced quotes/comments)."""
    return bool(_call_intrinsic(_MOLT_SQLITE3_COMPLETE_STATEMENT, sql))


# ---------------------------------------------------------------------------
# Adapter / converter registries (stub-only for now)
# ---------------------------------------------------------------------------
# CPython exposes ``register_adapter`` / ``register_converter`` so that
# user-defined Python types can round-trip through SQLite via the column
# decltype / colname pragma machinery.  Until the runtime has type
# detection wired through, we simply remember registrations so that user
# code (notably ``sqlite3/dbapi2.py``'s ``register_adapters_and_converters``
# bootstrap) can install them without errors.

_adapters: dict = {}
_converters: dict = {}


def register_adapter(typ, callable_):  # noqa: A002 — stdlib name
    _adapters[typ] = callable_


def register_converter(name: str, callable_):
    _converters[name.lower()] = callable_


__all__.extend(["register_adapter", "register_converter"])

# Drop the loader helper so it does not leak into ``import _sqlite3``.
globals().pop("_require_intrinsic", None)
