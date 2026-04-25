"""``sqlite3.dump`` — produce a SQL text dump of a sqlite3 database.

Mirror of CPython 3.12's :mod:`sqlite3.dump` (Author: Paul Kippes).
The implementation only relies on DB-API 2.0 surface methods, so it
works unchanged on top of Molt's intrinsic-backed driver.
"""

# fmt: off
# pylint: disable=all
# ruff: noqa

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")
del _require_intrinsic


def _iterdump(connection):
    """Return an iterator producing the SQL dump of *connection*.

    The output mirrors the sqlite3 shell's ``.dump`` command and is
    suitable for restoring the database via ``executescript``.
    """

    writeable_schema = False
    cu = connection.cursor()
    cu.row_factory = None  # Make sure we get predictable results.
    yield ("BEGIN TRANSACTION;")

    # sqlite_master holds the CREATE statements for every table.
    schema_res = cu.execute(
        """
        SELECT "name", "type", "sql"
        FROM "sqlite_master"
            WHERE "sql" NOT NULL AND
            "type" == 'table'
            ORDER BY "name"
        """
    )
    sqlite_sequence = []
    for table_name, _type, sql in schema_res.fetchall():
        if table_name == "sqlite_sequence":
            rows = cu.execute('SELECT * FROM "sqlite_sequence";').fetchall()
            sqlite_sequence = ['DELETE FROM "sqlite_sequence"']
            sqlite_sequence += [
                f"INSERT INTO \"sqlite_sequence\" VALUES('{row[0]}',{row[1]})"
                for row in rows
            ]
            continue
        if table_name == "sqlite_stat1":
            yield ('ANALYZE "sqlite_master";')
        elif table_name.startswith("sqlite_"):
            continue
        elif sql.startswith("CREATE VIRTUAL TABLE"):
            if not writeable_schema:
                writeable_schema = True
                yield ("PRAGMA writable_schema=ON;")
            yield (
                "INSERT INTO sqlite_master(type,name,tbl_name,rootpage,sql)"
                "VALUES('table','{0}','{0}',0,'{1}');".format(
                    table_name.replace("'", "''"),
                    sql.replace("'", "''"),
                )
            )
        else:
            yield ("{0};".format(sql))

        # Emit INSERT INTO statements for every row of the current table.
        table_name_ident = table_name.replace('"', '""')
        res = cu.execute('PRAGMA table_info("{0}")'.format(table_name_ident))
        column_names = [str(info[1]) for info in res.fetchall()]
        rows_q = """SELECT 'INSERT INTO "{0}" VALUES({1})' FROM "{0}";""".format(
            table_name_ident,
            ",".join(
                """'||quote("{0}")||'""".format(col.replace('"', '""'))
                for col in column_names
            ),
        )
        for row in cu.execute(rows_q):
            yield ("{0};".format(row[0]))

    # Indexes, triggers, views — emit after data so triggers do not fire
    # while rows are being restored.
    schema_res = cu.execute(
        """
        SELECT "name", "type", "sql"
        FROM "sqlite_master"
            WHERE "sql" NOT NULL AND
            "type" IN ('index', 'trigger', 'view')
        """
    )
    for _name, _type, sql in schema_res.fetchall():
        yield ("{0};".format(sql))

    if writeable_schema:
        yield ("PRAGMA writable_schema=OFF;")

    # gh-79009 — emit sqlite_sequence statements at the end of the txn.
    for row in sqlite_sequence:
        yield ("{0};".format(row))

    yield ("COMMIT;")
