"""CPython's bad-syntax tokenizer fixtures stay syntax errors in Molt's stdlib.

``test/tokenizedata/badsyntax_*`` exist so CPython's tokenizer tests can watch
an import fail with a precise ``SyntaxError``. Molt ships them as modules that
raise that error; a rewrite that let one import cleanly would remove what the
tests rely on.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

from tests.stdlib_intrinsic_registry import intrinsic_registry

TOKENIZEDATA = Path(__file__).resolve().parents[1] / "src/molt/stdlib/test/tokenizedata"


@pytest.mark.parametrize(
    ("name", "message"),
    [
        ("badsyntax_3131", "invalid character '€' (U+20AC)"),
        ("badsyntax_pep3120", None),
    ],
)
def test_bad_syntax_fixture_raises_syntax_error(name: str, message: str | None) -> None:
    spec = importlib.util.spec_from_file_location(
        f"molt_test_{name}", TOKENIZEDATA / f"{name}.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    with intrinsic_registry(), pytest.raises(SyntaxError) as excinfo:
        spec.loader.exec_module(module)
    if message is not None:
        assert excinfo.value.msg == message
