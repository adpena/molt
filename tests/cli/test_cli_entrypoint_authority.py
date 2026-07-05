from __future__ import annotations

import argparse
import inspect

import molt.cli as cli
from molt.cli import entrypoint
from molt.cli import entrypoint_dispatch
from molt.cli import entrypoint_parser
from molt.cli import proof_queue as cli_proof_queue


def test_cli_entrypoint_dispatch_and_parser_authorities_are_single_home() -> None:
    assert callable(entrypoint.main)
    assert callable(entrypoint_dispatch._dispatch_entrypoint_command)
    assert callable(entrypoint_parser._build_entrypoint_parser)
    assert not hasattr(cli, "_dispatch_entrypoint_command")
    assert not hasattr(cli, "_build_entrypoint_parser")

    root_main_source = inspect.getsource(cli.main)
    assert "ArgumentParser" not in root_main_source
    assert "add_parser" not in root_main_source
    assert "_entrypoint.main" in root_main_source

    entrypoint_source = inspect.getsource(entrypoint)
    assert "ArgumentParser(" not in entrypoint_source
    assert ".add_parser(" not in entrypoint_source
    assert "if args.command ==" not in entrypoint_source
    assert "_dispatch_entrypoint_command(" in entrypoint_source
    assert "_build_entrypoint_parser()" in entrypoint_source

    dispatch_source = inspect.getsource(entrypoint_dispatch)
    assert "def _dispatch_entrypoint_command(" in dispatch_source
    assert "if args.command ==" in dispatch_source

    parser_source = inspect.getsource(entrypoint_parser)
    assert "def _build_entrypoint_parser(" in parser_source
    assert "ArgumentParser(" in parser_source
    assert ".add_parser(" in parser_source

    root_module_source = inspect.getsource(cli)
    assert "def build(" in root_module_source
    assert "def main(" in root_module_source
    assert "if args.command ==" not in root_module_source
    assert "ArgumentParser(" not in root_module_source


def test_molt_queue_parser_preserves_proof_queue_argv() -> None:
    parser = entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(["queue", "run", "--queue-size", "2", "--detach"])

    assert args.command == "queue"
    assert args.queue_args == ["run", "--queue-size", "2", "--detach"]

    global_option_args = parser.parse_args(
        [
            "queue",
            "--db",
            "logs/proof_queue/proof_queue.sqlite3",
            "status",
            "--errors-only",
        ]
    )
    assert global_option_args.command == "queue"
    assert global_option_args.queue_args == [
        "--db",
        "logs/proof_queue/proof_queue.sqlite3",
        "status",
        "--errors-only",
    ]

    help_args = parser.parse_args(["queue", "--help"])
    assert help_args.command == "queue"
    assert help_args.queue_args == ["--help"]


def test_molt_queue_handler_delegates_to_proof_queue_main(
    monkeypatch,
) -> None:
    calls: list[list[str]] = []

    class FakeProofQueue:
        @staticmethod
        def main(argv: list[str], *, prog: str | None = None) -> int:
            calls.append([*([] if prog is None else [f"prog={prog}"]), *argv])
            return 17

    def fake_import_module(name: str) -> object:
        assert name == "tools.proof_queue"
        return FakeProofQueue

    monkeypatch.setattr(cli_proof_queue.importlib, "import_module", fake_import_module)

    rc = cli_proof_queue.handle_queue_command(
        argparse.Namespace(queue_args=["run", "--queue-size", "2", "--detach"])
    )

    assert rc == 17
    assert calls == [["prog=molt queue", "run", "--queue-size", "2", "--detach"]]


def test_molt_queue_handler_defaults_to_status(monkeypatch) -> None:
    calls: list[list[str]] = []

    class FakeProofQueue:
        @staticmethod
        def main(argv: list[str], *, prog: str | None = None) -> int:
            calls.append([*([] if prog is None else [f"prog={prog}"]), *argv])
            return 0

    monkeypatch.setattr(
        cli_proof_queue.importlib,
        "import_module",
        lambda name: FakeProofQueue,
    )

    assert cli_proof_queue.handle_queue_command(argparse.Namespace(queue_args=[])) == 0
    assert calls == [["prog=molt queue", "status"]]
