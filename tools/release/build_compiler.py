"""Build the production compiler, launcher and worker through one authority."""

from pathlib import Path
import sys

_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.release.native_build import main  # noqa: E402


if __name__ == "__main__":
    raise SystemExit(main())
