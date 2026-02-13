"""Differential coverage for Pickler/Unpickler persistent id/load hooks."""

from __future__ import annotations

import io
import pickle


class PersistentPickler(pickle.Pickler):
    def persistent_id(self, obj):
        if isinstance(obj, tuple) and len(obj) == 2 and obj[0] == "ref":
            return obj[1]
        return None


class PersistentUnpickler(pickle.Unpickler):
    def __init__(self, file, table: dict[str, int]) -> None:
        super().__init__(file)
        self._table = table

    def persistent_load(self, pid):
        return self._table[pid]


def main() -> None:
    buf = io.BytesIO()
    payload = [("ref", "a"), ("literal", 2), ("ref", "b")]
    PersistentPickler(buf, protocol=5).dump(payload)
    buf.seek(0)
    out = PersistentUnpickler(buf, {"a": 10, "b": 11}).load()
    print("persistent", out)


if __name__ == "__main__":
    main()
