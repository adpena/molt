from __future__ import annotations

from molt._wasm_abi_generated import WASM_EXECUTION_MANIFEST_VERSION

import hashlib
import json
import os
from pathlib import Path


EMPTY_WASM_MODULE = b"\x00asm\x01\x00\x00\x00"


def _manifest_payload(
    mode: str, modules: dict[str, dict[str, object]]
) -> dict[str, object]:
    return {
        "version": WASM_EXECUTION_MANIFEST_VERSION,
        "mode": mode,
        "modules": modules,
        "entry": {
            "module": "linked" if mode == "linked" else "app",
            "function": "molt_main",
        },
    }


def split_wasm_execution_manifest(app: bytes, runtime: bytes) -> dict[str, object]:
    """Bind the generator's fixed lexical files to the actual fixture bytes."""
    return _manifest_payload(
        "split-runtime",
        {
            label: {
                "path": filename,
                "size": len(data),
                "sha256": hashlib.sha256(data).hexdigest(),
            }
            for label, filename, data in (
                ("app", "app.wasm", app),
                ("runtime", "molt_runtime.wasm", runtime),
            )
        },
    )


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
            "path": os.path.relpath(module.resolve(), root.resolve()).replace(
                os.sep, "/"
            ),
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
    manifest = root / "manifest.json"
    manifest.write_text(
        json.dumps(
            _manifest_payload(mode, modules),
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return manifest
