from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_sqlite3_connect", globals())
_require_intrinsic("molt_sqlite3_execute", globals())
_require_intrinsic("molt_sqlite3_close", globals())

class Connection:
    def __init__(self, database):
        self._handle = molt_sqlite3_connect(database)
        self.row_factory = None
    
    def execute(self, sql, parameters=None):
        return self.cursor().execute(sql, parameters)
    
    def cursor(self):
        return Cursor(self)
    
    def close(self):
        if self._handle:
            molt_sqlite3_close(self._handle)
            self._handle = None

class Cursor:
    def __init__(self, connection):
        self.connection = connection
        self._rows = []
        self._index = 0
    
    def execute(self, sql, parameters=None):
        self._rows = molt_sqlite3_execute(self.connection._handle, sql)
        self._index = 0
        return self
    
    def fetchall(self):
        rows = self._rows[self._index:]
        self._index = len(self._rows)
        
        if self.connection.row_factory:
            return [self.connection.row_factory(self, row) for row in rows]
        return rows

def connect(database, **kwargs):
    return Connection(database)

class Row:
    def __init__(self, cursor, values):
        self._values = values
    def __getitem__(self, item):
        if isinstance(item, int):
            return self._values[item]
        raise KeyError(item)
