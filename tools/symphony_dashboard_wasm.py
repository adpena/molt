from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

try:
    from molt.symphony.paths import default_molt_ext_root
except ModuleNotFoundError:  # pragma: no cover - script execution path.
    _REPO_ROOT = Path(__file__).resolve().parents[1]
    _SRC_ROOT = _REPO_ROOT / "src"
    if str(_SRC_ROOT) not in sys.path:
        sys.path.insert(0, str(_SRC_ROOT))
    from molt.symphony.paths import default_molt_ext_root


def _default_ext_root() -> Path:
    return default_molt_ext_root()


def default_output_path() -> Path:
    return _default_ext_root() / "wasm" / "symphony" / "dashboard_kernel.wasm"


def build_dashboard_wasm_cmd(
    *,
    source: Path,
    output: Path,
    profile: str,
    linked: bool,
) -> list[str]:
    cmd = [
        "uv",
        "run",
        "--python",
        "3.12",
        "python",
        "-m",
        "molt.cli",
        "build",
        "--target",
        "wasm",
        "--profile",
        profile,
    ]
    if linked:
        cmd.extend(
            [
                "--linked",
                "--linked-output",
                str(output),
            ]
        )
    cmd.append(str(source))
    return cmd


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Build Symphony dashboard analytics kernel to WASM via Molt "
            "(Python -> Molt -> wasm)."
        )
    )
    parser.add_argument(
        "--source",
        default="src/molt/symphony/dashboard_kernel.py",
        help="Path to Python kernel source.",
    )
    parser.add_argument(
        "--output",
        default=str(default_output_path()),
        help="Linked WASM output path.",
    )
    parser.add_argument(
        "--profile",
        default="dev",
        choices=("dev", "release"),
        help="Molt profile (dev/release).",
    )
    parser.add_argument(
        "--no-linked",
        action="store_true",
        help="Disable linked output (use relocatable artifact only).",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    source = Path(args.source).expanduser().resolve()
    output = Path(args.output).expanduser().resolve()
    if not source.exists():
        raise FileNotFoundError(f"Source not found: {source}")
    output.parent.mkdir(parents=True, exist_ok=True)
    cmd = build_dashboard_wasm_cmd(
        source=source,
        output=output,
        profile=args.profile,
        linked=not args.no_linked,
    )
    completed = subprocess.run(cmd, check=False)
    return int(completed.returncode)


if __name__ == "__main__":
    raise SystemExit(main())
