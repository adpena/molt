from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/email/__init__.py",
    ROOT / "src/molt/stdlib/email/_encoded_words.py",
    ROOT / "src/molt/stdlib/email/_header_value_parser.py",
    ROOT / "src/molt/stdlib/email/_policybase.py",
    ROOT / "src/molt/stdlib/email/contentmanager.py",
    ROOT / "src/molt/stdlib/email/header.py",
    ROOT / "src/molt/stdlib/email/message.py",
    ROOT / "src/molt/stdlib/email/policy.py",
    ROOT / "src/molt/stdlib/encodings/__init__.py",
    ROOT / "src/molt/stdlib/encodings/mbcs.py",
    ROOT / "src/molt/stdlib/importlib/_bootstrap.py",
    ROOT / "src/molt/stdlib/importlib/_bootstrap_external.py",
    ROOT / "src/molt/stdlib/multiprocessing/util.py",
    ROOT / "src/molt/stdlib/reprlib.py",
    ROOT / "src/molt/stdlib/tkinter/__init__.py",
    ROOT / "src/molt/stdlib/tkinter/__main__.py",
    ROOT / "src/molt/stdlib/tkinter/_support.py",
    ROOT / "src/molt/stdlib/tkinter/constants.py",
    ROOT / "src/molt/stdlib/tkinter/dnd.py",
    ROOT / "src/molt/stdlib/tkinter/ttk.py",
]


def test_residual_public_shim_batch_hides_raw_capability_intrinsic() -> None:
    for path in MODULE_PATHS:
        source = path.read_text()
        assert '_require_intrinsic("molt_capabilities_has", globals())' not in source

    assert (
        '_MOLT_REPRLIB_CAP_HAS = _require_intrinsic("molt_capabilities_has")'
        in (ROOT / "src/molt/stdlib/reprlib.py").read_text()
    )

    for path in [
        ROOT / "src/molt/stdlib/email/__init__.py",
        ROOT / "src/molt/stdlib/email/_encoded_words.py",
        ROOT / "src/molt/stdlib/email/_header_value_parser.py",
        ROOT / "src/molt/stdlib/email/_policybase.py",
        ROOT / "src/molt/stdlib/email/contentmanager.py",
        ROOT / "src/molt/stdlib/email/header.py",
        ROOT / "src/molt/stdlib/email/message.py",
        ROOT / "src/molt/stdlib/email/policy.py",
        ROOT / "src/molt/stdlib/encodings/__init__.py",
        ROOT / "src/molt/stdlib/encodings/mbcs.py",
        ROOT / "src/molt/stdlib/importlib/_bootstrap.py",
        ROOT / "src/molt/stdlib/importlib/_bootstrap_external.py",
        ROOT / "src/molt/stdlib/multiprocessing/util.py",
        ROOT / "src/molt/stdlib/tkinter/__init__.py",
        ROOT / "src/molt/stdlib/tkinter/__main__.py",
        ROOT / "src/molt/stdlib/tkinter/_support.py",
        ROOT / "src/molt/stdlib/tkinter/constants.py",
        ROOT / "src/molt/stdlib/tkinter/dnd.py",
        ROOT / "src/molt/stdlib/tkinter/ttk.py",
    ]:
        assert (
            '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")'
            in path.read_text()
        )
