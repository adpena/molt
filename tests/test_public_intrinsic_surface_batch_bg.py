from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/pstats.py",
    "src/molt/stdlib/pty.py",
    "src/molt/stdlib/pwd.py",
    "src/molt/stdlib/pydoc.py",
    "src/molt/stdlib/pydoc_data/__init__.py",
    "src/molt/stdlib/pyexpat.py",
    "src/molt/stdlib/readline.py",
    "src/molt/stdlib/resource.py",
    "src/molt/stdlib/rlcompleter.py",
    "src/molt/stdlib/sched.py",
    "src/molt/stdlib/shelve.py",
    "src/molt/stdlib/smtplib.py",
    "src/molt/stdlib/sre_compile.py",
    "src/molt/stdlib/sre_constants.py",
    "src/molt/stdlib/symtable.py",
    "src/molt/stdlib/syslog.py",
    "src/molt/stdlib/tabnanny.py",
    "src/molt/stdlib/termios.py",
    "src/molt/stdlib/timeit.py",
    "src/molt/stdlib/tomllib/__init__.py",
]

INTRINSIC_GLOBALS_RE = re.compile(
    r"require_intrinsic\([^)]*,\s*globals\(\)\s*\)", re.DOTALL
)


def test_tranche_files_do_not_bind_intrinsics_via_globals() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        assert INTRINSIC_GLOBALS_RE.search(text) is None, rel_path
