# edgebox/db.py -- BoxDB: a thin wrapper over sqlite3
#
# Uses the ":do:" sentinel convention to separate DDL/DML statements
# in schema files, and provides simple query helpers that return
# lists of dicts.

import json
import sqlite3


# Sentinel used to split multi-statement SQL files.
DO_SENTINEL = ":do:"


class BoxDB:
    """Lightweight SQLite wrapper for edgebox storage.

    Usage:
        db = BoxDB("/path/to/data.db")
        db.execute_schema(open("schema.sql").read())
        rows = db.query("SELECT * FROM events WHERE pr_id = ?", [42])
    """

    def __init__(self, path=":memory:"):
        self._path = path
        self._conn = sqlite3.connect(path)
        self._conn.row_factory = sqlite3.Row

    # -- schema management --------------------------------------------------

    def execute_schema(self, sql_text):
        """Execute a schema file that may contain multiple statements.

        Statements are separated by the `:do:` sentinel on its own line,
        or by standard semicolons. The sentinel allows clean separation
        of DDL blocks without relying on tricky semicolon parsing.
        """
        # Split on sentinel first
        blocks = sql_text.split(DO_SENTINEL)
        idx = 0
        while idx < len(blocks):
            block = blocks[idx].strip()
            idx = idx + 1
            if not block:
                continue
            self._conn.executescript(block)
        self._conn.commit()

    # -- query helpers ------------------------------------------------------

    def query(self, sql, params=None):
        """Run a SELECT and return a list of dicts."""
        if params is None:
            params = []
        cursor = self._conn.execute(sql, params)
        rows = cursor.fetchall()
        result = []
        row_idx = 0
        while row_idx < len(rows):
            row = rows[row_idx]
            row_idx = row_idx + 1
            keys = row.keys()
            d = {}
            key_idx = 0
            while key_idx < len(keys):
                k = keys[key_idx]
                d[k] = row[key_idx]
                key_idx = key_idx + 1
            result.append(d)
        return result

    def execute(self, sql, params=None):
        """Run an INSERT / UPDATE / DELETE and return lastrowid."""
        if params is None:
            params = []
        cursor = self._conn.execute(sql, params)
        self._conn.commit()
        return cursor.lastrowid

    def executemany(self, sql, param_list):
        """Run a statement for each parameter set."""
        self._conn.executemany(sql, param_list)
        self._conn.commit()

    # -- lifecycle ----------------------------------------------------------

    def close(self):
        """Close the underlying connection."""
        self._conn.close()

    # -- serialization helpers ----------------------------------------------

    @staticmethod
    def to_json(value):
        """Serialize a Python value to a JSON string for storage."""
        return json.dumps(value)

    @staticmethod
    def from_json(text):
        """Deserialize a JSON string from storage."""
        if text is None:
            return None
        return json.loads(text)
