"""Rustc TargetInfo protocol fixtures captured from cross-target print queries."""

from __future__ import annotations

from pathlib import Path


def rustc_target_metadata_output(
    sysroot: Path,
    cfg: str = "unix\nselected\n",
    *,
    target: str | None = None,
) -> tuple[str, str]:
    if target is not None and target.startswith("wasm32"):
        filenames = ("___.wasm", "lib___.rlib", "___.wasm", "lib___.a")
        unsupported = ("dylib", "proc-macro")
        split_debuginfo = ("off",)
    elif target is not None and "windows-msvc" in target:
        filenames = (
            "___.exe",
            "lib___.rlib",
            "___.dll",
            "___.dll",
            "___.lib",
            "___.dll",
        )
        unsupported = ()
        split_debuginfo = ("packed",)
    elif target is not None and "apple-darwin" in target:
        filenames = (
            "___",
            "lib___.rlib",
            "lib___.dylib",
            "lib___.dylib",
            "lib___.a",
            "lib___.dylib",
        )
        unsupported = ()
        split_debuginfo = ("off", "packed", "unpacked")
    else:
        filenames = (
            "___",
            "lib___.rlib",
            "lib___.so",
            "lib___.so",
            "lib___.a",
            "lib___.so",
        )
        unsupported = ()
        split_debuginfo = ("off", "packed", "unpacked")
    stdout = "\n".join((*filenames, str(sysroot), *split_debuginfo, "___", cfg))
    stderr = "\n".join(
        f"warning: dropping unsupported crate type `{kind}` for target `{target}`"
        for kind in unsupported
    )
    return stdout, stderr


def rustc_target_metadata_stdout(sysroot: Path, cfg: str = "unix\nselected\n") -> str:
    return rustc_target_metadata_output(sysroot, cfg)[0]
