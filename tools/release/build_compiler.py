"""Build the production compiler, launcher and worker through one authority."""

from .native_build import main


if __name__ == "__main__":
    raise SystemExit(main())
