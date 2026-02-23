# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: validate importlib.resources._functional error semantics."""

import importlib.resources._functional as functional


try:
    functional.read_text("tests.differential.stdlib", "res_pkg", "data.txt")
except Exception as exc:  # noqa: BLE001
    print("encoding_required", type(exc).__name__)
    print("encoding_required_msg", "encoding" in str(exc))

try:
    functional.read_binary(None, "res_pkg", "data.txt")
except Exception as exc:  # noqa: BLE001
    print("anchor_none", type(exc).__name__)
    print("anchor_none_msg", "anchor" in str(exc))

try:
    functional.read_binary("tests.differential.stdlib", "res_pkg")
except Exception as exc:  # noqa: BLE001
    print("dir_read", type(exc).__name__)

try:
    functional.open_binary("tests.differential.stdlib", "res_pkg")
except Exception as exc:  # noqa: BLE001
    print("dir_open", type(exc).__name__)
