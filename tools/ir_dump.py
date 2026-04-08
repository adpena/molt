#!/usr/bin/env python3
from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC_DIR = ROOT / "src"
if str(SRC_DIR) not in sys.path:
    sys.path.insert(0, str(SRC_DIR))

from molt.debug.ir import (  # noqa: E402
    VALID_STAGES,
    build_parser,
    capture_ir_snapshots,
    main,
    render_ir_json,
    render_ir_text,
)

if __name__ == "__main__":
    raise SystemExit(main())
