"""Build the shared probe source against the executing CPython's real C API."""

from __future__ import annotations

import argparse
import shutil
import tomllib
from pathlib import Path

from setuptools import Distribution, Extension
from setuptools.command.build_ext import build_ext


def build(fixture: Path, output_root: Path) -> None:
    with (fixture / "pyproject.toml").open("rb") as source:
        manifest = tomllib.load(source)
    # Molt and CPython consume one module/source declaration. In particular,
    # never compile this oracle against Molt's replacement Python.h.
    extension = manifest["tool"]["molt"]["extension"]
    distribution = Distribution(
        {
            "name": manifest["project"]["name"],
            "version": manifest["project"]["version"],
            "ext_modules": [
                Extension(
                    extension["module"],
                    sources=[str(fixture / path) for path in extension["sources"]],
                )
            ],
        }
    )
    command = build_ext(distribution)
    command.build_lib = str(output_root / "lib")
    command.build_temp = str(output_root / "temp")
    command.ensure_finalized()
    command.run()
    package_name = extension["module"].rsplit(".", 1)[0]
    package_path = Path(*package_name.split("."))
    package = output_root / "lib" / package_path
    shutil.copyfile(fixture / package_path / "__init__.py", package / "__init__.py")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_root", type=Path)
    args = parser.parse_args()
    build(Path(__file__).resolve().parent, args.output_root)


if __name__ == "__main__":
    main()
