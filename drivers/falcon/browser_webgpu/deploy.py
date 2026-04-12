from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


DRIVER_DIR = Path(__file__).resolve().parent
DEFAULT_WRANGLER_CONFIG = DRIVER_DIR / "wrangler.jsonc"
DEFAULT_ARTIFACT_SUBDIR = Path("dist") / "browser_split"
DEFAULT_WEIGHTS_SUBDIR = Path("weights")
DEFAULT_WEIGHTS_BUCKET = "falcon-ocr-weights"


def _strip_jsonc(text: str) -> str:
    out: list[str] = []
    i = 0
    in_string = False
    escaped = False
    while i < len(text):
        ch = text[i]
        if in_string:
            out.append(ch)
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
            i += 1
            continue
        if ch == '"':
            in_string = True
            out.append(ch)
            i += 1
            continue
        if ch == "/" and i + 1 < len(text):
            nxt = text[i + 1]
            if nxt == "/":
                i += 2
                while i < len(text) and text[i] not in "\r\n":
                    i += 1
                continue
            if nxt == "*":
                i += 2
                while i + 1 < len(text) and not (
                    text[i] == "*" and text[i + 1] == "/"
                ):
                    i += 1
                i += 2
                continue
        out.append(ch)
        i += 1
    return re.sub(r",(\s*[}\]])", r"\1", "".join(out))


def discover_wrangler_config(explicit: Path | None = None) -> Path:
    candidate = (explicit or DEFAULT_WRANGLER_CONFIG).expanduser().resolve()
    if not candidate.exists():
        raise FileNotFoundError(f"wrangler config not found: {candidate}")
    if not candidate.is_relative_to(DRIVER_DIR):
        raise ValueError(f"wrangler config must live under {DRIVER_DIR}")
    return candidate


def load_wrangler_config(path: Path) -> dict[str, Any]:
    return json.loads(_strip_jsonc(path.read_text(encoding="utf-8")))


def _extract_r2_bucket_names(config: dict[str, Any]) -> list[str]:
    bucket_names: list[str] = []
    for bucket in config.get("r2_buckets", []):
        if not isinstance(bucket, dict):
            continue
        name = bucket.get("bucket_name")
        if isinstance(name, str) and name:
            bucket_names.append(name)
    return bucket_names


def build_deploy_surface(config_path: Path, target_root: Path) -> dict[str, Any]:
    config = load_wrangler_config(config_path)
    entrypoint = (config_path.parent / str(config.get("main", ""))).resolve()
    if not entrypoint.exists():
        raise FileNotFoundError(f"worker entrypoint not found: {entrypoint}")

    artifact_root = target_root / DEFAULT_ARTIFACT_SUBDIR
    app_wasm = artifact_root / "app.wasm"
    runtime_wasm = artifact_root / "molt_runtime.wasm"
    if not app_wasm.exists():
        raise FileNotFoundError(f"missing Falcon app wasm: {app_wasm}")
    if not runtime_wasm.exists():
        raise FileNotFoundError(f"missing Falcon runtime wasm: {runtime_wasm}")
    weights_dir = target_root / DEFAULT_WEIGHTS_SUBDIR
    if not weights_dir.exists():
        raise FileNotFoundError(f"missing Falcon weights dir: {weights_dir}")

    r2_bucket_names = _extract_r2_bucket_names(config)
    if DEFAULT_WEIGHTS_BUCKET not in r2_bucket_names:
        raise ValueError(
            f"{config_path} must bind the {DEFAULT_WEIGHTS_BUCKET!r} R2 bucket"
        )

    return {
        "status": "scaffold",
        "target": "falcon.browser_webgpu",
        "target_root": str(target_root),
        "config_path": str(config_path),
        "worker_entrypoint": str(entrypoint),
        "cloudflare": {
            "name": config.get("name"),
            "compatibility_date": config.get("compatibility_date"),
            "main": config.get("main"),
            "r2_buckets": config.get("r2_buckets", []),
            "observability": config.get("observability", {}),
        },
        "artifacts": {
            "app_wasm": str(app_wasm),
            "runtime_wasm": str(runtime_wasm),
            "weights_dir": str(weights_dir),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Discover the Falcon browser WebGPU Cloudflare deploy surface"
    )
    parser.add_argument(
        "--target-root",
        type=Path,
        required=True,
        help="Falcon application root containing dist/browser_split and weights/",
    )
    parser.add_argument(
        "--wrangler-config",
        type=Path,
        default=None,
        help="Optional target-local wrangler.jsonc override.",
    )
    args = parser.parse_args()

    config_path = discover_wrangler_config(args.wrangler_config)
    surface = build_deploy_surface(config_path, args.target_root.resolve())
    print(json.dumps(surface, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
