"""Produce the stdlib namespace syntax artifact from sealed release source."""

from pathlib import Path
import sys

_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.cli.stdlib_namespace_syntax import stdlib_namespace_syntax_document  # noqa: E402
from molt.exact_json import canonical_json_bytes  # noqa: E402
from molt.file_publication import atomic_write_bytes  # noqa: E402
from molt.source_root import compiler_source_root  # noqa: E402


if __name__ == "__main__":
    (destination,) = sys.argv[1:]
    atomic_write_bytes(
        Path(destination),
        canonical_json_bytes(
            stdlib_namespace_syntax_document(compiler_source_root() / "src/molt/stdlib")
        )
        + b"\n",
        exclusive=True,
    )
