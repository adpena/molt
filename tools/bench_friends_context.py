import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = Path(__file__).resolve().parent
_SRC_ROOT = REPO_ROOT / "src"

for _path_root in (TOOLS_ROOT, _SRC_ROOT):
    _path_text = str(_path_root)
    if _path_root.exists() and _path_text not in sys.path:
        sys.path.insert(0, _path_text)
