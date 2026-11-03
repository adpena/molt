"""The ``molt`` process: the script, ``python -m molt.cli`` and the bootstrap."""

import sys

from molt.host_python import unsupported_frontend_python

# Name an unsupported interpreter before an unrelated import fails (HF-759).
_UNSUPPORTED = unsupported_frontend_python()
if _UNSUPPORTED is not None:
    print(f"molt: error: {_UNSUPPORTED}", file=sys.stderr)
    raise SystemExit(2)

from typing import NoReturn  # noqa: E402

from molt import compiler_process  # noqa: E402
from molt.cli import main  # noqa: E402


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
