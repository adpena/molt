#!/usr/bin/env python3
"""Generate a Cloudflare Worker entry point from template."""

from __future__ import annotations
import argparse
import json
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
    capabilities: list[str],
    tmp_quota_mb: int = 32,
    wasm_filename: str | None = None,
    *,
    manifest: Path | None = None,
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
    template = template.replace("{{CALLABLE_TABLE_ABI}}", callable_abi).replace(
        "{{LOADER_BRIDGE}}", bridge
    )
    template = template.replace(
        "{{DEPLOYMENT_MANIFEST}}",
        json.dumps(
            {key: deployment[key] for key in ("version", "mode", "modules")},
            sort_keys=True,
        ),
    )
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
    template = template.replace(
        "{{SYNTHETIC_ERRNO}}", json.dumps(abi["synthetic_errno"])
    )
    template = template.replace(
        "{{WASI_PREVIEW1_ERRNO}}", json.dumps(abi["wasi_preview1_errno"])
    )
    template = template.replace("{{RUNTIME_LIFECYCLE}}", lifetime)
    template = template.replace(
        "{{RUNTIME_LIFETIME_EXPORTS}}", json.dumps(lifetime_exports)
    )
    if not all(c.isalnum() or c in "._-" for c in wasm_filename):
        raise ValueError(f"Invalid wasm_filename: {wasm_filename!r}")
    template = template.replace("{{TMP_QUOTA_MB}}", str(int(tmp_quota_mb)))
    template = template.replace("{{CAPABILITIES}}", json.dumps(capabilities))
    template = template.replace("{{WASM_FILENAME}}", wasm_filename)
    output.write_text(template, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("-o", "--output", type=Path, default=Path("worker.js"))
    parser.add_argument(
        "--capabilities",
        nargs="*",
        default=["fs.bundle.read", "fs.tmp.read", "fs.tmp.write", "http.fetch"],
    )
    parser.add_argument("--tmp-quota-mb", type=int, default=32)
    parser.add_argument("--wasm-filename")
    parser.add_argument("--manifest", type=Path)
    args = parser.parse_args()
    generate_worker(
        args.output,
        args.capabilities,
        args.tmp_quota_mb,
        args.wasm_filename,
        manifest=args.manifest,
    )
    print(f"Generated {args.output}", file=__import__("sys").stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
