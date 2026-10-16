#!/usr/bin/env python3
from __future__ import annotations

import argparse
import sys
from pathlib import Path


_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"


from tools import check_stdlib_intrinsics as stdlib_audit  # noqa: E402
from tools.compat import test_policy  # noqa: E402
from molt.stdlib_intrinsic_policy import stdlib_module_intrinsic_facts  # noqa: E402
from molt.compiler_analysis.python_imports import (  # noqa: E402
    normalize_stdlib_module_name,
    UnresolvedStaticImportError,
)
from molt.cli.module_dependencies import _module_dependencies_from_imports  # noqa: E402

DEFAULT_CORE_MANIFEST = ROOT / "tests" / "differential" / "basic" / "CORE_TESTS.txt"


def _collect_manifest_tests(manifest: Path) -> list[Path]:
    if not manifest.is_file():
        raise FileNotFoundError(f"Manifest not found: {manifest}")
    tests: list[Path] = []
    for raw in manifest.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        path = Path(line)
        if not path.is_absolute():
            path = ROOT / path
        if not path.exists():
            raise FileNotFoundError(f"Manifest entry missing: {line} ({manifest})")
        tests.append(path)
    return tests


def _collect_seed_modules(manifest: Path, known_modules: dict[str, Path]) -> set[str]:
    seeds: set[str] = set()
    admitted = 0
    excluded = 0
    paths = known_modules
    for test_path in _collect_manifest_tests(manifest):
        if test_path.suffix != ".py":
            continue
        reason = test_policy.parse_metadata(test_path).python_exclusion_reason(
            sys.version_info[:2]
        )
        if reason is not None:
            excluded += 1
            print(f"[SKIP] {test_path} ({reason})")
            continue
        admitted += 1
        evidence = stdlib_module_intrinsic_facts(
            "__main__",
            test_path,
            target_python=stdlib_audit._DEFAULT_TARGET_PYTHON_VERSION,
        ).import_evidence
        from molt.cli.module_import_scanner import _complete_import_scan
        from molt.cli.module_resolution import _ModuleResolutionCache

        completed = _complete_import_scan(
            evidence.source_scan,
            source_path=test_path,
            roots=None,
            stdlib_root=None,
            stdlib_allowlist=None,
            resolution_cache=_ModuleResolutionCache(),
            target_python=stdlib_audit._DEFAULT_TARGET_PYTHON_VERSION,
            module_graph=paths,
        )
        for site in completed.source_sites:
            owner_plan = site.owner_plan
            if (
                site.request is not None
                and owner_plan.modules
                and not owner_plan.errors
                and not owner_plan.requires_runtime
                and all(
                    not raw.startswith("molt.stdlib.")
                    and (normalize_stdlib_module_name(raw) or raw).split(".", 1)[0]
                    not in known_modules
                    for raw in owner_plan.modules
                )
            ):
                # This catalog cannot complete a project package's exports.
                # Its unknown star topology is not a missing stdlib provider.
                continue
            if site.unresolved:
                raise UnresolvedStaticImportError(
                    f"core-lane source {test_path}:{site.line} requires runtime import custody"
                )
            for raw_owner in site.owner_plan.modules:
                owner = normalize_stdlib_module_name(raw_owner) or raw_owner
                if owner.split(".", 1)[0] in known_modules or raw_owner.startswith(
                    "molt.stdlib."
                ):
                    # Preserve a named absent descendant as an exact inventory
                    # fact. Its runtime ImportError may be intentional Python.
                    seeds.add(owner)
            seeds.update(
                _module_dependencies_from_imports("__main__", paths, site.plan.modules)
            )
    print(f"core-lane source admission: admitted={admitted} excluded={excluded}")
    return seeds


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Inventory core-lane stdlib source candidates and unresolved import custody; "
            "runtime qualification is separate."
        )
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_CORE_MANIFEST,
        help="Path to a differential lane manifest (default: core lane).",
    )
    args = parser.parse_args()

    try:
        inventory = stdlib_audit.stdlib_module_statuses()
        candidates = _collect_seed_modules(
            args.manifest, {name: path for name, (_status, path) in inventory.items()}
        )
    except (OSError, UnicodeError, RuntimeError, UnresolvedStaticImportError) as exc:
        print(f"core-lane source inventory failed: {exc}")
        return 1
    print("core-lane source inventory: ok (runtime qualification is separate)")
    print(f"- manifest: {args.manifest}")
    for name in sorted(candidates):
        status = inventory[name][0] if name in inventory else "absent"
        print(f"- {name}: {status}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
