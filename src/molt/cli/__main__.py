"""The ``molt`` process: the script, ``python -m molt.cli`` and the bootstrap."""

from typing import NoReturn

from molt import compiler_process
from molt.cli import main


def run() -> NoReturn:
    """Run one command in a compiler process, then end that process."""
    compiler_process.configure()
    try:
        code: object = main()
    except SystemExit as exc:
        code = exc.code
    compiler_process.exit_process(code)


if __name__ == "__main__":
    run()
