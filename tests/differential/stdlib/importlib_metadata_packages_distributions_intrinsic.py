"""Purpose: validate intrinsic-backed importlib.metadata packages_distributions lowering."""

import importlib.metadata
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    site = root / "site"
    site.mkdir()

    dist_one = site / "demo_alpha_pkg-1.0.dist-info"
    dist_two = site / "demo_beta_pkg-2.0.dist-info"
    dist_one.mkdir()
    dist_two.mkdir()

    (dist_one / "METADATA").write_text(
        "Name: demo-alpha-pkg\nVersion: 1.0\n",
        encoding="utf-8",
    )
    (dist_two / "METADATA").write_text(
        "Name: demo-beta-pkg\nVersion: 2.0\n",
        encoding="utf-8",
    )
    (dist_one / "top_level.txt").write_text(
        "pkg_demo_alpha_unique\npkg_demo_shared_unique\n",
        encoding="utf-8",
    )
    (dist_two / "top_level.txt").write_text(
        "pkg_demo_beta_unique\npkg_demo_shared_unique\n",
        encoding="utf-8",
    )

    original = list(sys.path)
    try:
        sys.path.insert(0, str(site))
        mapping = importlib.metadata.packages_distributions()
        alpha = sorted(mapping.get("pkg_demo_alpha_unique", []))
        beta = sorted(mapping.get("pkg_demo_beta_unique", []))
        shared = sorted(mapping.get("pkg_demo_shared_unique", []))
        print("alpha", alpha)
        print("beta", beta)
        print("shared", shared)
        print("shared_has_alpha", "demo-alpha-pkg" in shared)
        print("shared_has_beta", "demo-beta-pkg" in shared)
    finally:
        sys.path[:] = original
