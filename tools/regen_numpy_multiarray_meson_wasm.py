#!/usr/bin/env python3
"""Regenerate the numpy ``_core._multiarray_umath`` Meson WASM build metadata.

This produces the inputs ``molt extension build --source-plan`` consumes for a
source-recompiled WASM seal of NumPy's C core:

  * a Meson build directory whose custom targets have generated the C sources
    (``arraytypes.c``, ``einsum_sumprod.c``, ``lowlevel_strided_loops.c`` ...)
  * ``intro-targets.json`` (``meson introspect --targets``)
  * ``compile_commands.json`` (Ninja compile database)

The build cross-compiles against the checked-in WASI sysroot (``wasm32-wasip1``)
using host LLVM ``clang`` plus the Rust toolchain's ``compiler_builtins`` archive
(the same ``__multi3``/``__ashlti3`` provider molt's WASM link lane uses). NumPy
is configured with ``-Dblas=none -Dlapack=none`` (WASM has no system BLAS) and
molt's CPython ABI headers stand in for CPython via a ``python3.pc`` pkg-config
file, matching numpy's Emscripten cross recipe
(``tools/ci/emscripten/emscripten.meson.cross``) and the Meson cross-compilation
reference (https://mesonbuild.com/Cross-compilation.html).

Meson is numpy's own vendored copy (``vendored-meson/meson/meson.py``); Ninja is
the host install. Only the generated C sources need to exist for the source
plan; we run ``ninja`` on the ``_multiarray_umath`` C-source generation targets
and tolerate a subsequent object-compile failure (molt owns object compilation
via the source plan, not Meson).

Usage (from the worktree root, with RunContext env exported and
``$MOLT_TARGET_ROOT/toolchains/...`` on PATH)::

    .venv/Scripts/python.exe tools/regen_numpy_multiarray_meson_wasm.py \
        --numpy-root bench/friends/repos/numpy_off_the_shelf \
        --build-root D:/Molt/tmp/numpy_multiarray_meson_wasm_build_20260706 \
        --wasi-sysroot D:/Molt/target-root/toolchains/wasi-sysroot-33.0+m \
        --molt-root .

All paths are emitted absolute so the resulting ``intro-targets.json`` and
``compile_commands.json`` resolve on this host.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path


def _clang() -> str:
    exe = shutil.which("clang") or r"C:\Program Files\LLVM\bin\clang.exe"
    return exe


def _tool(name: str, fallback: str) -> str:
    return shutil.which(name) or fallback


def _rust_compiler_builtins_rlib() -> Path:
    """Locate the active Rust toolchain's wasm32-wasip1 compiler_builtins rlib."""
    home = Path(os.environ.get("USERPROFILE") or os.environ["HOME"])
    rustup = home / ".rustup" / "toolchains"
    candidates: list[Path] = []
    if rustup.is_dir():
        for toolchain in rustup.iterdir():
            libdir = toolchain / "lib" / "rustlib" / "wasm32-wasip1" / "lib"
            if libdir.is_dir():
                candidates.extend(sorted(libdir.glob("libcompiler_builtins-*.rlib")))
    if not candidates:
        raise SystemExit(
            "no wasm32-wasip1 libcompiler_builtins-*.rlib found under "
            f"{rustup}; install the wasm32-wasip1 target for the active toolchain"
        )
    return candidates[0]


def _write_pkgconfig(pkgdir: Path, molt_root: Path) -> None:
    pkgdir.mkdir(parents=True, exist_ok=True)
    # The CPython-ABI tier is ONE self-complete header authority. Only expose
    # ``runtime/molt-cpython-abi/include`` (Python.h, structmember.h, pymem.h,
    # pyerrors.h, ...) via the pkg-config Cflags. Do NOT add the repo-root
    # ``include/`` tier here: it carries Molt's source-compat NumPy overlay
    # (``include/numpy/*``), which — once baked into numpy's Meson
    # compile_commands.json — would shadow numpy's OWN ``numpy/*`` headers and
    # reintroduce the header-custody collision. numpy supplies its own complete
    # ``numpy/_core/include`` through its Meson build's include dirs.
    inc_abi = (molt_root / "runtime" / "molt-cpython-abi" / "include").as_posix()
    (pkgdir / "python3.pc").write_text(
        "prefix={prefix}\n"
        "exec_prefix=${{prefix}}\n"
        "includedir={inc_abi}\n\n"
        "Name: Python\n"
        "Description: Molt Python C API for source-recompiled extensions\n"
        "Version: 3.12\n"
        "Cflags: -I{inc_abi}\n"
        "Libs:\n".format(prefix=molt_root.as_posix(), inc_abi=inc_abi),
        encoding="utf-8",
    )


def _write_cross_file(
    cross_path: Path,
    *,
    wasi_sysroot: Path,
    pkgdir: Path,
    builtins_rlib: Path,
    cython: Path,
) -> None:
    def esc(p: str) -> str:
        return p.replace("\\", "\\\\")

    clang = esc(_clang())
    clangxx = esc(_tool("clang++", r"C:\Program Files\LLVM\bin\clang++.exe"))
    ar = esc(_tool("llvm-ar", r"C:\Program Files\LLVM\bin\llvm-ar.exe"))
    strip = esc(_tool("llvm-strip", r"C:\Program Files\LLVM\bin\llvm-strip.exe"))
    sysroot = esc(str(wasi_sysroot))
    rlib = esc(str(builtins_rlib))
    cython_exe = esc(str(cython))
    cross_path.write_text(
        "[binaries]\n"
        f"ar = ['{ar}']\n"
        f"c = ['{clang}', '--sysroot', '{sysroot}', '-target', 'wasm32-wasip1']\n"
        f"cpp = ['{clangxx}', '--sysroot', '{sysroot}', '-target', 'wasm32-wasip1']\n"
        f"strip = ['{strip}']\n"
        # Cython runs on the build machine and emits portable C; declare it so
        # Meson does not probe a bare `cython` off PATH (Meson cross-compilation
        # reference: https://mesonbuild.com/Cross-compilation.html#binaries).
        f"cython = ['{cython_exe}']\n"
        "\n"
        "[built-in options]\n"
        f"pkg_config_path = ['{pkgdir.as_posix()}']\n"
        "c_link_args = ['-nodefaultlibs', '-lc', '{rlib}']\n".format(rlib=rlib)
        + "cpp_link_args = ['-nodefaultlibs', '-lc', '-lc++', '-lc++abi', '{rlib}']\n".format(
            rlib=rlib
        )
        + "\n"
        "[host_machine]\n"
        "system = 'wasi'\n"
        "cpu_family = 'wasm32'\n"
        "cpu = 'wasm32'\n"
        "endian = 'little'\n"
        "\n"
        "[properties]\n"
        "longdouble_format = 'IEEE_QUAD_LE'\n"
        "needs_exe_wrapper = true\n"
        "skip_sanity_check = true\n",
        encoding="utf-8",
    )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--numpy-root", required=True, type=Path)
    ap.add_argument("--build-root", required=True, type=Path)
    ap.add_argument("--wasi-sysroot", required=True, type=Path)
    ap.add_argument("--molt-root", required=True, type=Path)
    ap.add_argument(
        "--python",
        default=sys.executable,
        help="Python running vendored meson (needs Cython, meson-python deps).",
    )
    ap.add_argument(
        "--cython",
        default=None,
        type=Path,
        help="Path to cython executable (default: cython.exe beside --python).",
    )
    ap.add_argument(
        "--generate-only",
        action="store_true",
        help="Only ninja the generated C sources for _multiarray_umath, then introspect.",
    )
    args = ap.parse_args()

    numpy_root = args.numpy_root.resolve()
    build_root = args.build_root.resolve()
    wasi_sysroot = args.wasi_sysroot.resolve()
    molt_root = args.molt_root.resolve()

    meson_py = numpy_root / "vendored-meson" / "meson" / "meson.py"
    if not meson_py.is_file():
        raise SystemExit(f"vendored meson not found: {meson_py}")
    # WASI sysroots use a multiarch header layout: clang resolves
    # <errno.h> under include/<triple>/ automatically from -target. Accept
    # either a flat or a per-triple layout.
    _errno_candidates = [
        wasi_sysroot / "include" / "errno.h",
        wasi_sysroot / "include" / "wasm32-wasip1" / "errno.h",
        wasi_sysroot / "include" / "wasm32-wasi" / "errno.h",
    ]
    if not any(p.is_file() for p in _errno_candidates):
        raise SystemExit(
            f"WASI sysroot missing errno.h (checked {[str(p) for p in _errno_candidates]})"
        )

    metadata_dir = build_root.parent / (build_root.name + "_metadata")
    metadata_dir.mkdir(parents=True, exist_ok=True)
    pkgdir = metadata_dir / "pkgconfig"
    cross_path = metadata_dir / "meson.numpy.cross"
    native_path = metadata_dir / "meson.numpy.native"
    cython = args.cython
    if cython is None:
        py = Path(args.python)
        cand = py.parent / "cython.exe"
        cython = cand if cand.is_file() else Path("cython")
    if not Path(cython).is_file():
        raise SystemExit(f"cython executable not found: {cython}")

    _write_pkgconfig(pkgdir, molt_root)
    _write_cross_file(
        cross_path,
        wasi_sysroot=wasi_sysroot,
        pkgdir=pkgdir,
        builtins_rlib=_rust_compiler_builtins_rlib(),
        cython=cython,
    )
    # Cython generates C on the BUILD machine before host cross-compilation, so
    # Meson resolves it from the native file, not the cross file's host
    # [binaries] (Meson cross-compilation reference:
    # https://mesonbuild.com/Cross-compilation.html). Declare it in a native
    # file to avoid Meson probing a bare `cython`/`cython3` off PATH.
    native_path.write_text(
        "[binaries]\n"
        f"cython = ['{str(cython).replace(chr(92), chr(92) * 2)}']\n",
        encoding="utf-8",
    )
    print(f"[regen] cross file:  {cross_path}")
    print(f"[regen] native file: {native_path}")
    print(f"[regen] pkgconfig:   {pkgdir}")

    env = dict(os.environ)
    env["PKG_CONFIG_PATH"] = str(pkgdir)

    if build_root.exists():
        print(f"[regen] wiping existing build root: {build_root}")
        shutil.rmtree(build_root, ignore_errors=True)

    setup_cmd = [
        args.python,
        str(meson_py),
        "setup",
        str(build_root),
        str(numpy_root),
        f"--cross-file={cross_path}",
        f"--native-file={native_path}",
        "-Dblas=none",
        "-Dlapack=none",
        "-Dallow-noblas=true",
        "-Ddisable-optimization=false",
        "--buildtype=release",
    ]
    print("[regen] meson setup:\n  " + " ".join(setup_cmd))
    r = subprocess.run(setup_cmd, cwd=str(numpy_root), env=env)
    if r.returncode != 0:
        raise SystemExit(f"meson setup failed rc={r.returncode}")

    # Introspect targets -> intro-targets.json (the source plan authority).
    intro_path = build_root / "intro-targets.json"
    intro_cmd = [
        args.python,
        str(meson_py),
        "introspect",
        str(build_root),
        "--targets",
    ]
    print("[regen] meson introspect --targets")
    r = subprocess.run(intro_cmd, cwd=str(numpy_root), env=env, capture_output=True, text=True)
    if r.returncode != 0:
        sys.stderr.write(r.stderr)
        raise SystemExit(f"meson introspect failed rc={r.returncode}")
    intro_path.write_text(r.stdout, encoding="utf-8")
    print(f"[regen] wrote {intro_path}")

    # Materialize the C sources AND the GENERATED HEADERS that _multiarray_umath
    # compiles against. molt owns object compilation from the source plan and
    # substitutes its own CPython-ABI include order, so we must NOT ninja-compile
    # objects (which would fail: numpy's Meson embeds the host CPython Include,
    # whose Windows-only <io.h> is absent under the WASI sysroot). Naming only the
    # generated .c/.h OUTPUTS as Ninja targets runs numpy's own custom-command
    # code generators and nothing else.
    #
    # Two distinct generated-output families feed the source plan:
    #
    #   1. Per-target generated .c sources (arraytypes.c, einsum_sumprod.c,
    #      loops.c, ...): declared inline in the extension target's
    #      ``target_sources[].generated_sources`` by intro-targets.
    #
    #   2. Generated HEADERS + #include-only .c API sources
    #      (``__multiarray_api.h``/``.c``, ``__ufunc_api.h``/``.c``,
    #      ``__umath_generated.c``, ``npy_math_internal.h``,
    #      ``_umath_doc_generated.h``): declared by numpy's ``custom_target``s in
    #      ``numpy/_core/meson.build`` and threaded into the extension through the
    #      ``np_core_dep`` ``sources:`` list + include dirs, NOT as the extension
    #      target's own ``generated_sources``. Meson surfaces each as a top-level
    #      ``type == "custom"`` target whose ``filename`` names the generated
    #      output. Without these, molt's compile fails
    #      ``fatal error: '__multiarray_api.h' file not found``. They are numpy's
    #      OWN code-generator outputs (``code_generators/generate_numpy_api.py``
    #      etc.), driven by numpy's meson — package custody, never vendored.
    #
    # We build BOTH families so the meson build dir's include path is complete
    # before molt takes over object compilation.
    targets = json.loads(intro_path.read_text(encoding="utf-8"))
    want = {
        "_multiarray_umath.cp312-win_amd64",
        "_multiarray_umath_mtargets",
    }
    gen_c: set[str] = set()
    for t in targets:
        if t.get("name") in want:
            for src in t.get("target_sources", []):
                for g in src.get("generated_sources", []):
                    if str(g).endswith(".c"):
                        gen_c.add(str(g))
    if not gen_c:
        print("[regen] WARNING: no generated .c sources found for _multiarray_umath")

    # Family 2: every generated header + #include-only API .c source declared by
    # a numpy ``custom_target`` (intro-targets ``type == "custom"``). Discovered
    # from numpy's own meson authority — no hand-written header list.
    gen_hdr: set[str] = set()
    for t in targets:
        if t.get("type") != "custom":
            continue
        for f in t.get("filename", []):
            fp = str(f)
            if fp.endswith(".h") or fp.endswith(
                ("__multiarray_api.c", "__ufunc_api.c", "__umath_generated.c")
            ):
                gen_hdr.add(fp)
    if not gen_hdr:
        print(
            "[regen] WARNING: no custom-target generated headers found "
            "(intro-targets has no type=='custom' .h/.c outputs)"
        )

    ninja = _tool("ninja", "ninja")
    rel_targets = [
        os.path.relpath(g, build_root).replace(os.sep, "/")
        for g in sorted(gen_c | gen_hdr)
    ]
    print(
        f"[regen] ninja generated-output targets "
        f"({len(gen_c)} .c sources + {len(gen_hdr)} generated headers/api-.c "
        f"= {len(rel_targets)}):"
    )
    for r_t in rel_targets:
        print(f"           {r_t}")
    if rel_targets:
        gen_cmd = [ninja, "-C", str(build_root), *rel_targets]
        r = subprocess.run(gen_cmd, cwd=str(build_root), env=env)
        if r.returncode != 0:
            raise SystemExit(
                f"ninja generated-output build failed rc={r.returncode}"
            )

    cc = build_root / "compile_commands.json"
    if not cc.is_file():
        # Meson emits compile_commands.json at setup; if absent, ask ninja.
        with open(cc, "w", encoding="utf-8") as fh:
            subprocess.run([ninja, "-C", str(build_root), "-t", "compdb"], stdout=fh, env=env)
    print(f"[regen] compile_commands.json: {cc} exists={cc.is_file()}")

    # Confirm every generated source AND generated header for the target now
    # resolves. Missing generated .c sources OR generated headers/api-.c means
    # molt's downstream compile would hit a fatal 'file not found'.
    all_gen: set[str] = set(gen_hdr)
    for t in targets:
        if t.get("name") in want:
            for src in t.get("target_sources", []):
                all_gen.update(str(g) for g in src.get("generated_sources", []))
    missing = [g for g in all_gen if not os.path.exists(g)]
    print(
        f"[regen] generated outputs: {len(all_gen)} total "
        f"({len(gen_hdr)} headers/api-.c), {len(missing)} missing"
    )
    for g in missing[:10]:
        print(f"           MISSING: {g}")
    if missing:
        raise SystemExit(
            f"{len(missing)} generated outputs for _multiarray_umath did not materialize"
        )

    print("[regen] DONE")
    print(f"[regen]   build_root      = {build_root}")
    print(f"[regen]   intro-targets   = {intro_path}")
    print(f"[regen]   compile_commands= {cc}")
    print(f"[regen]   cross_file      = {cross_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
