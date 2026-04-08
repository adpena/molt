#!/usr/bin/env python3
from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC_DIR = ROOT / "src"
if str(SRC_DIR) not in sys.path:
    sys.path.insert(0, str(SRC_DIR))

from molt.debug.verify import (  # noqa: E402
    ALIASES,
    DEFAULT_ALLOWED_MISSING,
    FRONTEND_PATH,
    FRONTEND_SEMANTIC_ASSERTIONS,
    NATIVE_BACKEND_PATH,
    NATIVE_SEMANTIC_ASSERTIONS,
    P0_REQUIRED,
    REQUIRED_BACKEND_KINDS,
    REQUIRED_DIFF_PROBES,
    ROOT,
    SPEC_PATH,
    WASM_BACKEND_PATH,
    WASM_IMPORTS_PATH,
    WASM_SEMANTIC_ASSERTIONS,
    SemanticAssertion,
    VerificationFinding,
    _candidate_kinds,
    _camel_to_upper_snake,
    _default_diff_root,
    _load_rss_metrics,
    _normalize_probe_path,
    _ordered_unique,
    _parse_allow_missing,
    _parse_spec_ops,
    _resolve_probe_run_id,
    _scan_backend_kinds,
    _scan_frontend_emit_kinds,
    _scan_frontend_lower_kinds,
    build_verify_result_payload,
    check_failure_queue_linkage,
    check_required_diff_probes,
    check_required_probe_execution,
    check_semantic_assertions,
    main,
    run_default_verify_checks,
)

if __name__ == "__main__":
    raise SystemExit(main())
