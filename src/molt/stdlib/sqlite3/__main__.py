"""A simple SQLite CLI for the sqlite3 module.

Mirror of CPython 3.12's :mod:`sqlite3.__main__` adapted to the Molt
runtime.  Uses :class:`code.InteractiveConsole` for the REPL.
"""

# fmt: off
# pylint: disable=all
# ruff: noqa

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY

_require_intrinsic("molt_stdlib_probe")
del _require_intrinsic

import sqlite3
import sys

from argparse import ArgumentParser
from code import InteractiveConsole
from textwrap import dedent


def execute(c, sql, suppress_errors=True):
    """Run *sql* against cursor/connection *c* and print returned rows."""
    try:
        for row in c.execute(sql):
            print(row)
    except sqlite3.Error as exc:
        tp = type(exc).__name__
        print(f"{tp}: {exc}", file=sys.stderr)
        if not suppress_errors:
            sys.exit(1)


class SqliteInteractiveConsole(InteractiveConsole):
    """A minimal SQLite REPL backed by :class:`InteractiveConsole`."""

    def __init__(self, connection):
        super().__init__()
        self._con = connection
        self._cur = connection.cursor()

    def runsource(self, source, filename="<input>", symbol="single"):
        if source == ".version":
            print(f"{sqlite3.sqlite_version}")
            return False
        if source == ".help":
            print("Enter SQL code and press enter.")
            return False
        if source == ".quit":
            sys.exit(0)
        if not sqlite3.complete_statement(source):
            return True
        execute(self._cur, source)
        return False


def main(*args):
    parser = ArgumentParser(
        description="Python sqlite3 CLI",
        prog="python -m sqlite3",
    )
    parser.add_argument(
        "filename",
        type=str,
        default=":memory:",
        nargs="?",
        help=(
            "SQLite database to open (defaults to ':memory:'). "
            "A new database is created if the file does not previously exist."
        ),
    )
    parser.add_argument(
        "sql",
        type=str,
        nargs="?",
        help="An SQL query to execute. Any returned rows are printed to stdout.",
    )
    parser.add_argument(
        "-v",
        "--version",
        action="version",
        version=f"SQLite version {sqlite3.sqlite_version}",
        help="Print underlying SQLite library version",
    )
    parsed = parser.parse_args(*args)

    db_name = (
        "a transient in-memory database"
        if parsed.filename == ":memory:"
        else repr(parsed.filename)
    )

    eofkey = "CTRL-Z" if sys.platform == "win32" and "idlelib.run" not in sys.modules else "CTRL-D"
    banner = dedent(
        f"""
        sqlite3 shell, running on SQLite version {sqlite3.sqlite_version}
        Connected to {db_name}

        Each command will be run using execute() on the cursor.
        Type ".help" for more information; type ".quit" or {eofkey} to quit.
        """
    ).strip()
    sys.ps1 = "sqlite> "
    sys.ps2 = "    ... "

    con = sqlite3.connect(parsed.filename, isolation_level=None)
    try:
        if parsed.sql:
            execute(con, parsed.sql, suppress_errors=False)
        else:
            console = SqliteInteractiveConsole(con)
            console.interact(banner, exitmsg="")
    finally:
        con.close()

    sys.exit(0)


if __name__ == "__main__":
    main(sys.argv[1:])
