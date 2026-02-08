# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: intrinsic-backed glob/pathlib character-class parity."""

from __future__ import annotations

import glob
from pathlib import Path
import tempfile


def _touch(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("ok", encoding="utf-8")


def main() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        _touch(root / "src" / "a1.py")
        _touch(root / "src" / "b2.py")
        _touch(root / "src" / "c3.py")
        _touch(root / "src" / "d4.txt")
        _touch(root / "src" / "pkg" / "aa.py")
        _touch(root / "src" / "pkg" / "bb.py")
        _touch(root / "src" / "pkg" / "cc.py")

        glob_hits = sorted(
            Path(item).relative_to(root).as_posix()
            for item in glob.glob(str(root / "src" / "[ab]*.py"))
        )
        print("glob_charclass", glob_hits)

        pathlib_hits = sorted(
            p.relative_to(root).as_posix() for p in (root / "src").glob("[ab]*.py")
        )
        print("pathlib_glob_charclass", pathlib_hits)

        recursive_hits = sorted(
            p.relative_to(root).as_posix()
            for p in (root / "src").glob("**/[!c]*.py")
        )
        print("pathlib_recursive_charclass", recursive_hits)


if __name__ == "__main__":
    main()
