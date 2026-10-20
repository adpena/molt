from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import io
import sys
from tests.stdlib_intrinsic_registry import install_registry


install_registry({{
    "molt_token_payload": lambda: {{
        "ENDMARKER": 0, "NAME": 1, "NUMBER": 2, "NEWLINE": 4,
        "PLUS": 14, "EQUAL": 22, "OP": 55, "COMMENT": 64, "NL": 65,
        "ENCODING": 67, "N_TOKENS": 68, "NT_OFFSET": 256,
        "tok_name": {{0: "ENDMARKER", 1: "NAME", 2: "NUMBER", 4: "NEWLINE",
                     14: "PLUS", 22: "EQUAL", 55: "OP", 64: "COMMENT", 65: "NL",
                     67: "ENCODING", 68: "N_TOKENS", 256: "NT_OFFSET"}},
        "EXACT_TOKEN_TYPES": {{"+": 14, "=": 22}},
        "__all__": ["tok_name", "ISTERMINAL", "ISNONTERMINAL", "ISEOF",
                    "EXACT_TOKEN_TYPES", "ENDMARKER", "NAME", "NUMBER", "NEWLINE",
                    "PLUS", "EQUAL", "OP", "COMMENT", "NL", "ENCODING",
                    "N_TOKENS", "NT_OFFSET"],
    }},
    "molt_tokenize_scan": lambda source: [
        (1, "x", (1, 0), (1, 1), source.splitlines()[0]),
        (55, "+", (1, 1), (1, 2), source.splitlines()[0]),
        (4, "\\n", (1, 2), (1, 3), source.splitlines()[0]),
    ],
}})


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


token = _load_module("token", {str(STDLIB_ROOT / "token.py")!r})
tokenize = _load_module("tokenize", {str(STDLIB_ROOT / "tokenize.py")!r})
tokens = list(tokenize.tokenize(io.BytesIO(b"x+\\n").readline))

checks = {{
    "behavior": (
        tokens[0].type == tokenize.ENCODING
        and tokens[1].type == tokenize.NAME
        and tokens[1].string == "x"
        and tokens[1].exact_type == 1
        and tokens[2].type == 55
        and tokens[2].exact_type == 14
        and tokenize.OP == token.OP == 55
        and tokenize.COMMENT == 64 and tokenize.NL == 65
        and tokenize.ENCODING == 67
        and tokenize.TokenInfo(55, "?", (1, 0), (1, 1), "?").exact_type == 55
        and tokenize.TokenInfo(1, "+", (1, 0), (1, 1), "+").exact_type == 1
    ),
    "private_handles_hidden": (
        "_MOLT_TOKENIZE_RUNTIME_READY" not in tokenize.__dict__
        and "_MOLT_TOKENIZE_SCAN" not in tokenize.__dict__
        and "molt_tokenize_runtime_ready" not in tokenize.__dict__
        and "molt_tokenize_scan" not in tokenize.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_tokenize_public_module_hides_bootstrap_handles() -> None:
    proc = run_surface_test_process(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "CHECK":
            checks[rest[0]] = rest[1]
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
