from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/ftplib.py",
    "src/molt/stdlib/genericpath.py",
    "src/molt/stdlib/getpass.py",
    "src/molt/stdlib/imaplib.py",
    "src/molt/stdlib/importlib/metadata/_adapters.py",
    "src/molt/stdlib/logging/handlers.py",
    "src/molt/stdlib/mailbox.py",
    "src/molt/stdlib/marshal.py",
    "src/molt/stdlib/mmap.py",
    "src/molt/stdlib/modulefinder.py",
    "src/molt/stdlib/netrc.py",
    "src/molt/stdlib/opcode.py",
    "src/molt/stdlib/optparse.py",
    "src/molt/stdlib/pdb.py",
    "src/molt/stdlib/pickletools.py",
    "src/molt/stdlib/plistlib.py",
    "src/molt/stdlib/poplib.py",
    "src/molt/stdlib/posix.py",
    "src/molt/stdlib/posixpath.py",
    "src/molt/stdlib/profile.py",
]

INTRINSIC_GLOBALS_RE = re.compile(
    r'require_intrinsic\([^)]*,\s*globals\(\)\s*\)', re.DOTALL
)


def test_tranche_files_do_not_bind_intrinsics_via_globals() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        assert INTRINSIC_GLOBALS_RE.search(text) is None, rel_path
