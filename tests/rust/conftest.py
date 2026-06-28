"""Pytest configuration for Rust backend tests.

Pre-warms the molt daemon before the test suite starts to avoid first-request
timeout when the daemon binary needs to cold-start.
"""

import os
import sys
import tempfile
from pathlib import Path

from molt.dx import development_artifact_env
from tests.rust.process_guard import run_rust_test_process

MOLT_DIR = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def pytest_configure(config):
    """Pre-warm the molt backend daemon before any tests run."""
    root = Path(MOLT_DIR)
    env = development_artifact_env(
        root,
        os.environ,
        session_prefix="rust-tests",
        session_id=os.environ.get("MOLT_SESSION_ID") or "rust-tests",
        create_dirs=True,
    )
    env.update(
        {
            "MOLT_USE_SCCACHE": "0",
            "RUSTC_WRAPPER": "",
            "PYTHONPATH": os.path.join(MOLT_DIR, "src"),
            "MOLT_DEV_CARGO_PROFILE": os.environ.get(
                "MOLT_DEV_CARGO_PROFILE", "release-fast"
            ),
            "UV_LINK_MODE": os.environ.get("UV_LINK_MODE", "copy"),
            "UV_NO_SYNC": os.environ.get("UV_NO_SYNC", "1"),
        }
    )
    try:
        with tempfile.NamedTemporaryFile(suffix=".py", mode="w", delete=False) as f:
            f.write("print(1)\n")
            warmup_path = f.name
        with tempfile.NamedTemporaryFile(suffix=".rs", delete=False) as f:
            warmup_out = f.name
        # Run a trivial transpilation to ensure daemon is up before tests start.
        # Generous timeout covers daemon cold-start + optional cargo rebuild.
        run_rust_test_process(
            [
                (sys.executable or "python3"),
                "-m",
                "molt.cli",
                "build",
                warmup_path,
                "--target",
                "rust",
                "--profile",
                "dev",
                "--output",
                warmup_out,
            ],
            capture_output=True,
            text=True,
            timeout=900,
            env=env,
            cwd=MOLT_DIR,
        )
    except Exception:
        pass  # Best-effort; individual test failures will surface real issues.
    finally:
        for p in (warmup_path, warmup_out):
            try:
                os.unlink(p)
            except Exception:
                pass
