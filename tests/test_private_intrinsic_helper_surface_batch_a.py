from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/_aix_support.py",
    "src/molt/stdlib/_android_support.py",
    "src/molt/stdlib/_apple_support.py",
    "src/molt/stdlib/_ast.py",
    "src/molt/stdlib/_ast_unparse.py",
    "src/molt/stdlib/_blake2.py",
    "src/molt/stdlib/_bz2.py",
    "src/molt/stdlib/_codecs.py",
    "src/molt/stdlib/_codecs_cn.py",
    "src/molt/stdlib/_codecs_hk.py",
    "src/molt/stdlib/_codecs_iso2022.py",
    "src/molt/stdlib/_codecs_jp.py",
    "src/molt/stdlib/_codecs_kr.py",
    "src/molt/stdlib/_codecs_tw.py",
    "src/molt/stdlib/_colorize.py",
    "src/molt/stdlib/_compat_pickle.py",
    "src/molt/stdlib/_compression.py",
    "src/molt/stdlib/_contextvars.py",
    "src/molt/stdlib/_crypt.py",
    "src/molt/stdlib/_csv.py",
]


def test_tranche_files_drop_stale_require_intrinsic_helper() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        assert 'globals().pop("_require_intrinsic", None)' in text, rel_path
