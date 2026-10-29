#!/usr/bin/env python3
"""Generate a Cloudflare Worker entry point from template."""

from __future__ import annotations
import argparse
import json
import re
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.wasm_artifact import (  # noqa: E402
    require_current_wasm_execution_manifest,
    verify_wasm_execution_manifest_module,
)


TEMPLATE_PATH = Path(__file__).parent / "wasm_worker_template.js"


def generate_worker(
    output: Path,
    *,
    tmp_quota_mb: int = 32,
    wasm_filename: str | None = None,
    manifest: Path | None = None,
    debug_stderr: bool = False,
) -> None:
    manifest_path = (
        manifest if manifest is not None else output.parent / "manifest.json"
    )
    try:
        deployment = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ValueError(
            "worker generation requires a current adjacent Molt deployment manifest; rebuild this deployment"
        ) from exc
    require_current_wasm_execution_manifest(deployment)
    if deployment.get("mode") != "linked":
        raise ValueError("standalone worker requires a linked Molt deployment manifest")
    observed = verify_wasm_execution_manifest_module(
        deployment, manifest_path, "linked"
    )
    admitted_filename = str(deployment["modules"]["linked"]["path"])
    if wasm_filename is not None and wasm_filename != admitted_filename:
        raise ValueError("wasm_filename must match the admitted deployment module")
    wasm_filename = admitted_filename
    if (output.parent / wasm_filename).resolve() != observed.path.resolve():
        raise ValueError(
            "worker output must be adjacent to its admitted deployment module"
        )
    template = TEMPLATE_PATH.read_text(encoding="utf-8")
    wasm_root = TEMPLATE_PATH.parent.parent / "wasm"
    lifetime = (wasm_root / "runtime_lifecycle.js").read_text(encoding="utf-8")
    callable_abi = (wasm_root / "callable_table_abi_generated.js").read_text(
        encoding="utf-8"
    )
    bridge = (wasm_root / "loader_bridge.js").read_text(encoding="utf-8")
    abi = json.loads(
        (wasm_root / "wasm_abi_generated.json").read_text(encoding="utf-8")
    )
    lifetime_exports = {
        key: abi["runtime_export_by_import"][key]
        for key in (
            "runtime_execution_enter",
            "runtime_execution_leave",
            "runtime_shutdown",
        )
    }
    if not all(c.isalnum() or c in "._-" for c in wasm_filename):
        raise ValueError(f"Invalid wasm_filename: {wasm_filename!r}")
    replacements = {
        "{{CALLABLE_TABLE_ABI}}": callable_abi,
        "{{LOADER_BRIDGE}}": bridge,
        "{{RUNTIME_LIFECYCLE}}": lifetime,
        "{{RUNTIME_LIFETIME_EXPORTS}}": json.dumps(lifetime_exports),
        "{{SYNTHETIC_ERRNO}}": json.dumps(abi["synthetic_errno"]),
        "{{WASI_PREVIEW1_ERRNO}}": json.dumps(abi["wasi_preview1_errno"]),
        "{{TMP_QUOTA_MB}}": str(int(tmp_quota_mb)),
        "{{DEBUG_STDERR}}": json.dumps(debug_stderr),
        "{{WASM_FILENAME}}": wasm_filename,
        # Decode JSON rather than a JS object literal: an own __proto__ key is
        # ordinary transported data, not an object-literal prototype setter.
        "{{DEPLOYMENT_MANIFEST}}": (
            f"JSON.parse({json.dumps(json.dumps(deployment, sort_keys=True))})"
        ),
    }
    # Only original template tokens are replaced; inserted data is never rescanned.
    template = re.sub(
        r"\{\{[A-Z0-9_]+\}\}", lambda match: replacements[match[0]], template
    )
    output.write_text(template, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("-o", "--output", type=Path, default=Path("worker.js"))
    parser.add_argument("--debug-stderr", action="store_true")
    parser.add_argument("--tmp-quota-mb", type=int, default=32)
    parser.add_argument("--wasm-filename")
    parser.add_argument("--manifest", type=Path)
    args = parser.parse_args()
    generate_worker(
        args.output,
        tmp_quota_mb=args.tmp_quota_mb,
        wasm_filename=args.wasm_filename,
        manifest=args.manifest,
        debug_stderr=args.debug_stderr,
    )
    print(f"Generated {args.output}", file=__import__("sys").stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
