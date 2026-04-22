from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _run_compute_engine_script(script: str) -> dict[str, object]:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser compute-engine tests")
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(run.stdout)


def test_compute_engine_rejects_unknown_forced_backend() -> None:
    result = _run_compute_engine_script(
        """
        const { ComputeEngine } = await import('./deploy/browser/compute-engine.js');
        try {
          const engine = await ComputeEngine.create({ forceBackend: 'bogus-backend' });
          process.stdout.write(JSON.stringify({
            ok: true,
            backend: engine.backendName,
          }));
        } catch (err) {
          process.stdout.write(JSON.stringify({
            ok: false,
            message: err.message,
          }));
        }
        """
    )

    assert result["ok"] is False
    assert "bogus-backend" in result["message"]
    assert "requested" in result["message"]
