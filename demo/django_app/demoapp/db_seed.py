from __future__ import annotations

import argparse
import os
import sqlite3
from pathlib import Path


DEFAULT_USERS = 100
DEFAULT_ITEMS_PER_USER = 500


def seed_db(path: Path, users: int, items_per_user: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(path))
    try:
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA synchronous=NORMAL")
        conn.executescript(
            """
            DROP TABLE IF EXISTS items;
            CREATE TABLE items (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                score REAL NOT NULL,
                unread INTEGER NOT NULL
            );
            CREATE INDEX idx_items_user ON items(user_id);
            CREATE INDEX idx_items_status ON items(status);
            CREATE INDEX idx_items_title ON items(title);
            """
        )
        for user_id in range(1, users + 1):
            rows = []
            base = user_id * 1000
            for idx in range(items_per_user):
                item_id = base + idx
                status = "open" if idx % 2 == 0 else "closed"
                created_at = f"2026-01-{(idx % 28) + 1:02}T00:00:{idx % 60:02}Z"
                title = f"Item {item_id}"
                score = (idx % 100) / 100.0
                unread = 1 if idx % 3 == 0 else 0
                rows.append(
                    (
                        item_id,
                        user_id,
                        created_at,
                        status,
                        title,
                        score,
                        unread,
                    )
                )
            conn.executemany(
                """
                INSERT INTO items (
                    id, user_id, created_at, status, title, score, unread
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                """,
                rows,
            )
        conn.commit()
    finally:
        conn.close()


def main() -> None:
    parser = argparse.ArgumentParser(description="Seed the demo SQLite DB.")
    parser.add_argument(
        "--path",
        default=None,
        help="Path to the SQLite DB file (defaults to MOLT_DEMO_DB_PATH).",
    )
    parser.add_argument(
        "--users",
        type=int,
        default=DEFAULT_USERS,
        help="Number of user_id buckets to seed.",
    )
    parser.add_argument(
        "--items-per-user",
        type=int,
        default=DEFAULT_ITEMS_PER_USER,
        help="Number of items per user.",
    )
    args = parser.parse_args()
    path_str = args.path
    if path_str is None or path_str == "":
        path_str = os.environ.get("MOLT_DEMO_DB_PATH")
    if path_str is None or path_str == "":
        path_str = str(Path(__file__).resolve().parents[1] / "demo_items.sqlite")
    path = Path(path_str)
    seed_db(path, users=max(1, args.users), items_per_user=max(1, args.items_per_user))
    print(f"Seeded demo DB at {path}")


if __name__ == "__main__":
    main()
