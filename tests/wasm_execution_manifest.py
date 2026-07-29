from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path


def write_wasm_execution_manifest(
    root: Path,
    *,
    linked: Path | None = None,
    app: Path | None = None,
    runtime: Path | None = None,
) -> Path:
    """Write the production execution-manifest shape for synthetic WASM fixtures."""
    if linked is not None:
        if app is not None or runtime is not None:
            raise ValueError("linked and split-runtime modules are mutually exclusive")
        mode = "linked"
        selected = {"linked": linked}
    else:
        if app is None or runtime is None:
            raise ValueError("split-runtime fixtures require app and runtime modules")
        mode = "split-runtime"
        selected = {"app": app, "runtime": runtime}
    modules: dict[str, dict[str, object]] = {}
    for label, module in selected.items():
        data = module.read_bytes()
        modules[label] = {
            "path": os.path.relpath(module.resolve(), root.resolve()).replace(os.sep, "/"),
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
    manifest = root / "manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "version": 2,
                "mode": mode,
                "modules": modules,
                "entry": {
                    "module": "linked" if linked is not None else "app",
                    "function": "molt_main",
                },
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return manifest
