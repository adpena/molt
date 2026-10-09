"""Finite source policy fixtures; never compiler/execution evidence."""

from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parents[1]
EXPECTED_LANES = (
    ("native", "native", "dev", "dev-fast", "release"),
    ("native", "native", "release", "release-output", "release"),
    ("native", "native", "release", "release-size", "release"),
    ("native", "native", "release", "release-fast", "release"),
    ("llvm", "native", "dev", "dev-fast", "release"),
    ("llvm", "native", "release", "release-output", "release"),
    ("llvm", "native", "release", "release-size", "release"),
    ("llvm", "native", "release", "release-fast", "release"),
    ("wasm", "wasm", "dev", "dev-fast", "release"),
    ("wasm", "wasm", "release", "release-output", "release"),
    ("wasm", "wasm", "release", "wasm-release", "release"),
)
LANE_FIELDS = (
    "backend",
    "target",
    "guest_profile",
    "runtime_profile",
    "compiler_profile",
)


def stage_release_lane_authorities(root: Path) -> None:
    """Copy only the real configuration/authority bytes consumed by the reader."""
    config = (ROOT / "config/release_acceptance_matrix.toml").read_bytes()
    declarations = tomllib.loads(config.decode("utf-8"))
    paths = {
        "config/release_acceptance_matrix.toml",
        "config/verified_subset.toml",
        "Cargo.toml",
    }
    paths.update(row["authority"] for row in declarations["lane"])
    paths.update(
        (
            "config/llvm_toolchain_arches.toml",
            "config/llvm_toolchain_releases.toml",
            "runtime/molt-backend-native/Cargo.toml",
            "runtime/molt-backend/Cargo.toml",
            "vendor/llvm/LICENSE.TXT",
        )
    )
    for name in sorted(paths):
        dest = root / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes((ROOT / name).read_bytes())
    policy = tomllib.loads((ROOT / "config/verified_subset.toml").read_text("utf-8"))
    for suite in policy["differential_suites"]:
        dest = root / suite["path"]
        dest.mkdir(parents=True, exist_ok=True)
        # A Git snapshot retains directories through actual source members.
        (dest / "lane_fixture.py").write_bytes(b"# synthetic transport fixture\n")
