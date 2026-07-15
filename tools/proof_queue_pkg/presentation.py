"""Human CLI quickstart and canonical command templates."""

from __future__ import annotations

import argparse


def _cmd_quickstart(args: argparse.Namespace) -> int:
    del args
    print(
        "molt queue status\n"
        "molt queue run --detach --queue-size 3\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py status\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py cargo "
        '--id focused-cargo-proof --reason "why this proves the Rust contract" '
        '--scope runtime/molt-runtime/src/cpython_abi_hooks.rs --note "change: moved the Rust authority; test: proving the focused invariant" --timeout 900 -- '
        "test -p molt-runtime exact_test_name --lib\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py exec "
        '--id focused-proof --reason "why this proves the changed contract" '
        '--resource-family python --contention-key python:focused --note "change: moved the shared authority; test: proving the focused invariant" --timeout 240 -- '
        "uv run --active --project . --python 3.12 pytest tests/path.py -q"
        "\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py note "
        '<run-id> --kind observation --note "what happened, what it means, and the next bounded action"'
        "\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py diagnose "
        "<run-id> --append-note"
        "\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py link "
        '<child-run-id> --parent <parent-run-id> --kind derives_from --note "why this edge exists"'
    )
    return 0



def _cmd_template(args: argparse.Namespace) -> int:
    del args
    print(
        "[[proof]]\n"
        'id = "focused-proof"\n'
        'reason = "Prove the changed contract, not a broad ritual."\n'
        'resource_family = "python"\n'
        'contention_key = "python:focused"\n'
        'scope = ["src/molt/cli/runtime_features.py"]\n'
        'depends_on = ["previous-run-id-or-logical-id"]\n'
        'note = "change: moved runtime feature authority into the generator-backed path"\n'
        'notes = ["test: targeted pytest proves the generated selector contract"]\n'
        'edge_kind = "derives_from"\n'
        'edge_note = "Narrows the previous failing proof to the generated selector contract."\n'
        'env = { MOLT_EXTERNAL_STATIC_PACKAGES = "numpy scipy" }\n'
        'command = ["uv", "run", "--active", "--project", ".", "--python", "3.12", "pytest", "tests/path.py", "-q"]\n'
    )
    return 0



def _cmd_cargo_template(args: argparse.Namespace) -> int:
    del args
    print(
        "uv run --active --project . --python 3.12 python tools/proof_queue.py cargo \\\n"
        "  --id runtime-focused-proof \\\n"
        '  --reason "Prove the changed Rust runtime contract." \\\n'
        "  --scope runtime/molt-runtime/src/cpython_abi_hooks.rs \\\n"
        '  --note "change: moved static-link Py_mod_exec diagnostics into the C-API authority" \\\n'
        "  --timeout 900 \\\n"
        "  --detach \\\n"
        "  -- test -p molt-runtime exact_test_name --lib"
    )
    return 0
