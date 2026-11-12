from __future__ import annotations

from molt._wasm_abi_generated import (
    WASM_EXECUTION_MANIFEST_VERSION,
    compiled_execution_symbol,
    RUNTIME_EXECUTION_ABIS,
    ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT,
    ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX,
    ATTACHED_EXECUTION_CONTEXT_ABI_SECTION,
    ATTACHED_CONTEXT_THREAD_RECORD_OFFSET,
)
from molt.capability_manifest import (
    ResolvedRuntimePolicy,
    resolve_runtime_policy_from_env,
)
from molt.wasm_artifact import wasm_execution_manifest_launch

import hashlib
import json
import os
from pathlib import Path


EMPTY_WASM_MODULE = b"\x00asm\x01\x00\x00\x00"


def execution_abi_metadata() -> dict[str, object]:
    return {
        "execution_context_abi": {
            "fingerprint": str(ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT),
            "suffix": ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX,
            "section": ATTACHED_EXECUTION_CONTEXT_ABI_SECTION,
            "thread_record_offset": ATTACHED_CONTEXT_THREAD_RECORD_OFFSET,
        },
        "runtime_execution_abis": {
            logical: {"context": abi.context, "physical_symbol": abi.physical_symbol}
            for logical, abi in RUNTIME_EXECUTION_ABIS.items()
        },
    }


def wasm_execution_launch(
    policy: ResolvedRuntimePolicy | None = None,
) -> dict[str, object]:
    """Fixture transport with explicit grants, defaulting to the deny policy."""
    return wasm_execution_manifest_launch(
        policy if policy is not None else resolve_runtime_policy_from_env({})
    )


def _manifest_payload(
    mode: str,
    modules: dict[str, dict[str, object]],
    policy: ResolvedRuntimePolicy | None = None,
) -> dict[str, object]:
    return {
        "version": WASM_EXECUTION_MANIFEST_VERSION,
        "mode": mode,
        "launch": wasm_execution_launch(policy),
        "modules": modules,
        "entry": {
            "module": "linked" if mode == "linked" else "app",
            "function": compiled_execution_symbol("molt_main"),
        },
        "abi": {"runtime_imports": execution_abi_metadata()},
    }


def split_wasm_execution_manifest(
    app: bytes,
    runtime: bytes,
    *,
    policy: ResolvedRuntimePolicy | None = None,
) -> dict[str, object]:
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
        policy,
    )


def write_wasm_execution_manifest(
    root: Path,
    *,
    linked: Path | None = None,
    app: Path | None = None,
    runtime: Path | None = None,
    policy: ResolvedRuntimePolicy | None = None,
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
            _manifest_payload(mode, modules, policy),
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return manifest
