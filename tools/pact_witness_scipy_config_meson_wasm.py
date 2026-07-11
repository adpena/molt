#!/usr/bin/env python3
r"""Emit SciPy's build-generated ``scipy/__config__.py`` for the pact witness.

Root cause this closes (E1 witness / scipy-seal closure completeness):

``scipy/__init__.py`` does ``from scipy.__config__ import show as show_config``
and *fails closed* (re-raises ``ImportError`` with the "you cannot import SciPy
while being in the SciPy source directory" message) when that import fails.
``scipy/__config__.py`` exists NOWHERE in SciPy's source tree -- only the
template ``scipy/__config__.py.in``. SciPy's meson build emits the real module
via ``configure_file`` (``scipy/meson.build`` ~L782), substituting the resolved
compiler ids/versions/command arrays + BLAS/LAPACK/pybind11/pythran metadata
into the template. The witness stages SciPy from its *source* tree, so the
module is absent and the source-derived closure drops it.

A full SciPy wasm ``meson setup`` (the deep-correct analogue of the numpy
``_multiarray`` meson-wasm seal, see ``tools/regen_numpy_multiarray_meson_wasm``)
is not feasible in this environment: SciPy's top-level ``meson.build`` hard-errors
on absent git submodules (``array_api_compat``/``array_api_extra``/``cobyqa``/
``unuran``/``xsf``), then requires a Fortran compiler, an openblas/openblas-wasm
BLAS/LAPACK, and configuring every heavy subproject (``boost_math``/``qhull_r``/
``xsf``/``duccfft``/``pyprima``) -- all *before* the ``subdir('scipy')`` that
contains the ``__config__.py`` ``configure_file`` at the very end of setup.

Instead this tool drives meson's OWN ``configure_file`` on SciPy's OWN
``scipy/__config__.py.in`` template, with a ``configuration_data`` populated by a
faithful, line-cited mirror of ``scipy/meson.build`` (v1.18.0, ~L702-788). Every
value is *meson-resolved from the same wasm cross toolchain SciPy would use*
(``molt``'s WASI ``wasm32-wasip1`` clang/clang++/cython, identical cross file to
the numpy seal) -- no hand-typed compiler/BLAS metadata. The Molt wasm build
configuration modelled is::

    -Dblas=none -Dlapack=none -D_without-fortran=true -Duse-pythran=true

which yields (verifiably identical to the real numpy wasm ``__config__.py`` on
disk for the shared toolchain fields):

  * Compilers: clang / ld.lld / <clang version> with the real cross command
    arrays; cython from the venv; fortran mirrors the C compiler with
    ``args``/``linker args`` = ``n/a`` (SciPy's ``_without-fortran`` branch).
  * Machine: host wasm32/wasi, build x86_64/windows, cross-compiled true.
  * BLAS/LAPACK: ``dependency('none')`` -> name ``none``, found ``false``
    (Molt provides no wasm BLAS; matches the numpy wasm config).
  * pybind11 + pythran: real build-machine tool versions (SciPy's own declared
    build deps; their versions are host-independent), resolved by meson.
  * Python: the venv interpreter path + ``3.12``.

The emitted module is written to
``tmp/pact_scipy_config_meson_wasm_build_generated/scipy/__config__.py`` -- the
authoritative source ``tools/pact_witness_scipy_generated_modules.py`` copies
into every witness SciPy package root. If meson setup fails, the tool fails
closed and surfaces the exact meson error (never fabricates the module).

Usage (from the worktree root, RunContext env exported)::

    .venv/Scripts/python.exe tools/pact_witness_scipy_config_meson_wasm.py \
        --scipy-root bench/friends/repos/scipy_off_the_shelf \
        --build-root tmp/pact_scipy_config_meson_wasm_build \
        --wasi-sysroot C:/Molt/target-root/toolchains/wasi-sysroot-33.0+m \
        --molt-root .
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parents[1] / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from molt.scientific_stack_versions import (  # noqa: E402
    resolve_scientific_stack,
    verify_cpython_abi_headers,
    verify_source_checkout,
)


# Canonical location of the emitted, fully-substituted module. Mirrors the
# ``_CONFIG_BUILD_CANDIDATES`` entry in tools/pact_witness_scipy_generated_modules.
_GENERATED_REL = "tmp/pact_scipy_config_meson_wasm_build_generated/scipy/__config__.py"


def _clang() -> str:
    return shutil.which("clang") or r"C:\Program Files\LLVM\bin\clang.exe"


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


def _write_cross_file(
    cross_path: Path, *, wasi_sysroot: Path, builtins_rlib: Path, cython: Path
) -> None:
    """Write the wasm32-wasip1 meson cross file (mirror of the numpy seal's)."""

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
        f"cython = ['{cython_exe}']\n"
        "\n"
        "[built-in options]\n"
        f"c_link_args = ['-nodefaultlibs', '-lc', '{rlib}']\n"
        f"cpp_link_args = ['-nodefaultlibs', '-lc', '-lc++', '-lc++abi', '{rlib}']\n"
        "\n"
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


# Standalone meson project that reproduces SciPy's ``__config__.py`` generation.
# It is a faithful, line-cited mirror of ``scipy/meson.build`` (v1.18.0) lines
# ~702-788 -- the block that builds ``conf_data`` and calls ``configure_file`` on
# ``__config__.py.in``. The ``{{`` / ``}}`` are literal meson braces escaped for
# ``str.format``; ``{scipy_version}`` is the only substituted field. Backslash
# handling in the ``.replace('\\', '/')`` calls matches SciPy's source exactly.
_MESON_BUILD = r"""project('scipy_config_gen', 'c', 'cpp', 'cython',
  version: '{scipy_version}',
  meson_version: '>= 1.5.0',
  default_options: ['buildtype=release'],
)

py3 = import('python').find_installation(pure: false)

# --- Faithful mirror of scipy/meson.build (v{scipy_version}) conf_data
#     population, lines ~702-788, for a Molt wasm cpython-abi cross build:
#       -Dblas=none -Dlapack=none -D_without-fortran=true -Duse-pythran=true
#     All values are meson-resolved from the SAME wasm cross toolchain SciPy
#     would use (no hand-typed compiler/BLAS/pybind11/pythran metadata).

cc = meson.get_compiler('c')
cpp = meson.get_compiler('cpp')

# pybind11 (scipy/meson.build L97): a required SciPy build dependency.
pybind11_dep = dependency('pybind11', version: '>=2.13.2')

# pythran (scipy/meson.build L189-192, L741-745). use-pythran defaults true.
use_pythran = true
if use_pythran
  pythran = find_program('pythran', native: true, version: '>=0.18.1')
  incdir_pythran = run_command(py3,
    ['-c', 'import os, pythran; print(pythran.get_include())'],
    check: true).stdout().strip()
endif

# BLAS/LAPACK: Molt wasm provides no BLAS/LAPACK. Mirror scipy's `-Dblas=none`
# path (scipy/meson.build L224/L300: `blas = dependency(blas_name)` with
# blas_name='none' yields a not-found dependency named 'none').
blas = dependency('none', required: false)
lapack = dependency('none', required: false)
use_ilp64 = false
cython_blas_ilp64 = false

# scipy/meson.build L702-711
compilers = {{
  'C': cc,
  'CPP': cpp,
  'CYTHON': meson.get_compiler('cython'),
}}
# _without-fortran=true: FORTRAN mirrors the C compiler (scipy L707-708).
compilers += {{'FORTRAN': meson.get_compiler('c')}}

machines = {{
  'HOST': host_machine,
  'BUILD': build_machine,
}}

conf_data = configuration_data()

# scipy/meson.build L720-740 (Set compiler information)
foreach name, compiler : compilers
  conf_data.set(name + '_COMP', compiler.get_id())
  conf_data.set(name + '_COMP_LINKER_ID', compiler.get_linker_id())
  conf_data.set(name + '_COMP_VERSION', compiler.version())
  conf_data.set(name + '_COMP_CMD_ARRAY', ', '.join(compiler.cmd_array()))
  if name == 'FORTRAN'
    # scipy L726-728: _without-fortran branch.
    conf_data.set('FORTRAN_COMP_ARGS', 'n/a')
    conf_data.set('FORTRAN_COMP_LINK_ARGS', 'n/a')
  else
    conf_data.set(name + '_COMP_ARGS', ', '.join(
        get_option(name.to_lower() + '_args')).replace('\\', '/'))
    conf_data.set(name + '_COMP_LINK_ARGS', ', '.join(
        get_option(name.to_lower() + '_link_args')).replace('\\', '/'))
  endif
endforeach

# scipy/meson.build L741-745 (pythran information)
if use_pythran
  conf_data.set('PYTHRAN_VERSION', pythran.version())
  conf_data.set('PYTHRAN_INCDIR', incdir_pythran)
endif

# scipy/meson.build L747-753 (Machine CPU and system information)
foreach name, machine : machines
  conf_data.set(name + '_CPU', machine.cpu())
  conf_data.set(name + '_CPU_FAMILY', machine.cpu_family())
  conf_data.set(name + '_CPU_ENDIAN', machine.endian())
  conf_data.set(name + '_CPU_SYSTEM', machine.system())
endforeach

# scipy/meson.build L755
conf_data.set('CROSS_COMPILED', meson.is_cross_build().to_string())

# scipy/meson.build L757-759 (Python information)
conf_data.set('PYTHON_PATH', py3.full_path().replace('\\', '/'))
conf_data.set('PYTHON_VERSION', py3.language_version())

# scipy/meson.build L761-780 (dependency information for __config__.py)
dependency_map = {{
  'BLAS': blas,
  'LAPACK': lapack,
  'PYBIND11': pybind11_dep,
}}
foreach name, dep : dependency_map
  conf_data.set(name + '_NAME', dep.name())
  conf_data.set(name + '_FOUND', dep.found().to_string())
  if dep.found()
    conf_data.set(name + '_VERSION', dep.version())
    conf_data.set(name + '_TYPE_NAME', dep.type_name())
    conf_data.set(name + '_INCLUDEDIR', dep.get_variable('includedir', default_value: 'unknown'))
    conf_data.set(name + '_LIBDIR', dep.get_variable('libdir', default_value: 'unknown'))
    conf_data.set(name + '_OPENBLAS_CONFIG', dep.get_variable('openblas_config', default_value: 'unknown'))
    conf_data.set(name + '_PCFILEDIR', dep.get_variable('pcfiledir', default_value: 'unknown').replace('\\', '/'))
    conf_data.set(name + '_HAS_ILP64', use_ilp64.to_string())
  endif
endforeach
conf_data.set('BLAS_CYTHON_ILP64', cython_blas_ilp64.to_string())

# scipy/meson.build L782-788
configure_file(
  input: '__config__.py.in',
  output: '__config__.py',
  configuration: conf_data,
)
"""


def _scipy_pyproject_version(scipy_root: Path) -> str:
    pyproject = scipy_root / "pyproject.toml"
    for line in pyproject.read_text(encoding="utf-8").splitlines():
        if line.startswith("version ="):
            return line.split("=", 1)[1].strip().strip("\"'")
    raise SystemExit(f"no 'version =' line in {pyproject}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--scipy-root", required=True, type=Path)
    ap.add_argument("--build-root", required=True, type=Path)
    ap.add_argument("--wasi-sysroot", required=True, type=Path)
    ap.add_argument("--molt-root", required=True, type=Path)
    ap.add_argument("--python", default=sys.executable)
    ap.add_argument("--cython", default=None, type=Path)
    args = ap.parse_args()

    stack = resolve_scientific_stack()
    verify_cpython_abi_headers(stack=stack, repo_root=args.molt_root.resolve())

    scipy_root = args.scipy_root.resolve()
    verify_source_checkout("scipy", scipy_root, stack=stack)
    build_root = args.build_root.resolve()
    wasi_sysroot = args.wasi_sysroot.resolve()
    molt_root = args.molt_root.resolve()

    template = scipy_root / "scipy" / "__config__.py.in"
    if not template.is_file():
        raise SystemExit(f"SciPy __config__.py.in template not found: {template}")

    # Meson: reuse numpy's vendored copy (SciPy ships no vendored meson).
    meson_py = (
        molt_root
        / "bench"
        / "friends"
        / "repos"
        / "numpy_off_the_shelf"
        / "vendored-meson"
        / "meson"
        / "meson.py"
    )
    verify_source_checkout("numpy", meson_py.parents[2], stack=stack)
    if not meson_py.is_file():
        raise SystemExit(f"vendored meson not found: {meson_py}")

    py = Path(args.python)
    cython = args.cython
    if cython is None:
        cand = py.parent / "cython.exe"
        cython = cand if cand.is_file() else Path("cython")
    if not Path(cython).is_file():
        raise SystemExit(f"cython executable not found: {cython}")

    scipy_version = _scipy_pyproject_version(scipy_root)
    if scipy_version != stack.scipy:
        raise SystemExit(
            f"SciPy source version {scipy_version} does not match verified "
            f"scientific stack {stack.tuple_label}"
        )

    # Staging: a standalone meson source dir carrying the mirror + the real
    # SciPy template.
    gen_src = build_root.parent / (build_root.name + "_src")
    if gen_src.exists():
        shutil.rmtree(gen_src, ignore_errors=True)
    gen_src.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(template, gen_src / "__config__.py.in")
    (gen_src / "meson.build").write_text(
        _MESON_BUILD.format(scipy_version=scipy_version), encoding="utf-8"
    )

    metadata_dir = build_root.parent / (build_root.name + "_metadata")
    metadata_dir.mkdir(parents=True, exist_ok=True)
    cross_path = metadata_dir / "meson.scipy.cross"
    native_path = metadata_dir / "meson.scipy.native"
    _write_cross_file(
        cross_path,
        wasi_sysroot=wasi_sysroot,
        builtins_rlib=_rust_compiler_builtins_rlib(),
        cython=Path(cython),
    )
    native_path.write_text(
        f"[binaries]\ncython = ['{str(cython).replace(chr(92), chr(92) * 2)}']\n",
        encoding="utf-8",
    )
    print(f"[regen] scipy version: {scipy_version}")
    print(f"[regen] meson source:  {gen_src}")
    print(f"[regen] cross file:     {cross_path}")

    # pybind11-config / pythran-config live in the venv Scripts dir; meson's
    # config-tool dependency resolution must find them on PATH.
    env = dict(os.environ)
    scripts_dir = py.parent
    env["PATH"] = str(scripts_dir) + os.pathsep + env.get("PATH", "")

    if build_root.exists():
        shutil.rmtree(build_root, ignore_errors=True)

    setup_cmd = [
        str(py),
        str(meson_py),
        "setup",
        str(build_root),
        str(gen_src),
        f"--cross-file={cross_path}",
        f"--native-file={native_path}",
        "--buildtype=release",
    ]
    print("[regen] meson setup:\n  " + " ".join(setup_cmd))
    r = subprocess.run(setup_cmd, cwd=str(gen_src), env=env)
    if r.returncode != 0:
        raise SystemExit(f"meson setup failed rc={r.returncode}")

    emitted = build_root / "__config__.py"
    if not emitted.is_file():
        raise SystemExit(
            f"meson setup did not emit {emitted}; configure_file may not have run"
        )
    content = emitted.read_text(encoding="utf-8")
    if (
        "@C_COMP@" in content
        or "@CYTHON_COMP@" in content
        or "@PYBIND11_NAME@" in content
    ):
        raise SystemExit(
            f"{emitted} still has unsubstituted meson @VARS@; refusing to publish"
        )

    dest = (molt_root / _GENERATED_REL).resolve()
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(content, encoding="utf-8")
    print(f"[regen] emitted __config__.py: {emitted}")
    print(f"[regen] published to:          {dest}")
    print("[regen] DONE")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
