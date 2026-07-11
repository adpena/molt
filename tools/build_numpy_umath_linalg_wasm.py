#!/usr/bin/env python3
"""Build + stage NumPy's ``numpy.linalg._umath_linalg`` WASM extension seal.

``numpy.linalg`` is imported unconditionally by the pact Kernel A witness
(``field_solve.py`` calls ``np.linalg.eigh``), and ``numpy/linalg/_linalg.py``
does a module-level ``from numpy.linalg import _umath_linalg`` — so the absence
of the ``_umath_linalg`` C extension blocks the numpy IMPORT, not merely the
compute. This recipe compiles ``_umath_linalg`` (numpy's C++ linalg gufuncs +
its self-contained f2c-translated ``lapack_lite`` LAPACK/BLAS: ``dsyevd``,
``ssyevd``, ``cheevd``, ``zheevd``, ``dgeev``, ``dgesdd``, ``dgetrf``,
``dpotrf`` ...) to a ``wasm32-wasip1`` cpython-abi relocatable object and stages
it beside the ``_multiarray_umath`` seal so ``numpy.linalg`` resolves.

Pipeline (reuses the EXISTING numpy Meson WASM build metadata, never a new
pipeline):

  1. ``tools/regen_numpy_multiarray_meson_wasm.py`` already configures numpy's
     vendored Meson against the checked-in WASI sysroot with
     ``-Dblas=none -Dlapack=none -Dallow-noblas=true`` and materializes numpy's
     generated core headers. Its ``intro-targets.json`` enumerates the
     ``_umath_linalg.cp312-win_amd64`` target (umath_linalg.cpp + python_xerbla.c
     + lapack_lite/*.c) and the ``npymath`` static library; its
     ``compile_commands.json`` owns the per-source compile args. This recipe
     consumes that same build dir — run the multiarray regen first if it is
     absent.

  2. ``molt extension build --target wasm --source-plan <intro-targets.json>
     --source-plan-target _umath_linalg`` compiles each translation unit against
     molt's CPython-ABI headers and wasm-ld ``-r``-merges them into
     ``_umath_linalg.molt.wasm`` with an ``extension_manifest.json`` recording
     the object closure (defined LAPACK symbols + the Python-C-API/libm/
     compiler-rt runtime_symbols + the two required numpy capsules
     ``_ARRAY_API`` / ``_UFUNC_API``).

     CRITICAL — ``--source-plan-exclude-linked-static-library npymath``: numpy's
     Meson links ``libnpymath.a`` into BOTH ``_multiarray_umath`` (PRIMARY) and
     ``_umath_linalg``. In real CPython each is its own ``.so`` so a private
     npymath copy is fine, but molt links every sealed relocatable object of a
     statically-linked package into ONE wasm module, so a second copy of the
     global ``npy_*`` definitions is a fatal ``wasm-ld: duplicate symbol`` at the
     final witness link. Excluding npymath keeps those symbols UNDEFINED here so
     they resolve against ``_multiarray_umath``'s exported npymath at final link
     — the same thin shape ``scipy.ndimage._nd_image`` relies on for numpy's
     C-API. lapack_lite (which ``_umath_linalg`` uniquely owns) is NOT excluded.

  3. The built ``_umath_linalg.molt.wasm`` + its ``*.extension_manifest.json``
     are staged into the witness seal's ``numpy/linalg/`` (disjoint from the
     numpy pure-Python closure and the ``_multiarray_umath`` core). Run
     ``tools/pact_seal_witness_roots.py`` afterwards to ABI-stamp the manifest
     against the current runtime and relativize its object-closure sources.

Usage (from the worktree root, RunContext env exported so ``WASI_SYSROOT`` /
``clang`` / ``wasm-ld`` resolve)::

    .venv/Scripts/python.exe tools/build_numpy_umath_linalg_wasm.py \
        --numpy-root bench/friends/repos/numpy_off_the_shelf \
        --meson-build-root tmp/pact_numpy_multiarray_meson_wasm_build_generated \
        --seal-root tmp/pact_numpy_multiarray_sealed_for_witness

then::

    .venv/Scripts/python.exe tools/pact_seal_witness_roots.py \
        --root tmp/pact_numpy_multiarray_sealed_for_witness
"""

from __future__ import annotations

import argparse
import json
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

_MODULE = "numpy.linalg._umath_linalg"
_ARTIFACT_NAME = "_umath_linalg.molt.wasm"
_MANIFEST_NAME = "_umath_linalg.molt.wasm.extension_manifest.json"
# numpy's ``libnpymath.a`` is statically owned + exported by the PRIMARY
# ``_multiarray_umath`` seal; excluding it keeps ``_umath_linalg`` thin.
_EXCLUDED_STATIC_LIBS = ("npymath",)


def _repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def _abs(base: Path, raw: str | Path) -> Path:
    p = Path(raw)
    return p.resolve() if p.is_absolute() else (base / p).resolve()


def main(argv: list[str] | None = None) -> int:
    repo_root = _repo_root()
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--numpy-root",
        default="bench/friends/repos/numpy_off_the_shelf",
        help="numpy source checkout (the extension --project).",
    )
    ap.add_argument(
        "--meson-build-root",
        default="tmp/pact_numpy_multiarray_meson_wasm_build_generated",
        help=(
            "Meson WASM build dir produced by "
            "tools/regen_numpy_multiarray_meson_wasm.py (holds intro-targets.json "
            "+ compile_commands.json + numpy's generated core headers)."
        ),
    )
    ap.add_argument(
        "--seal-root",
        default="tmp/pact_numpy_multiarray_sealed_for_witness",
        help="Witness seal root to stage the artifact + manifest into.",
    )
    ap.add_argument(
        "--out-dir",
        default=None,
        help=(
            "Staging out-dir for `molt extension build` "
            "(default: <meson-build-root>/../pact_numpy_umath_linalg_molt_ext_wasm_cpython_abi)."
        ),
    )
    ap.add_argument(
        "--python",
        default=sys.executable,
        help="Python running `molt` (needs the molt editable install).",
    )
    ap.add_argument(
        "--no-stage",
        action="store_true",
        help="Build only; do not copy into the seal root.",
    )
    args = ap.parse_args(argv)

    stack = resolve_scientific_stack()
    verify_cpython_abi_headers(stack=stack, repo_root=repo_root)

    numpy_root = _abs(repo_root, args.numpy_root)
    verify_source_checkout("numpy", numpy_root, stack=stack)
    meson_build_root = _abs(repo_root, args.meson_build_root)
    seal_root = _abs(repo_root, args.seal_root)
    intro_targets = meson_build_root / "intro-targets.json"
    compile_commands = meson_build_root / "compile_commands.json"

    for label, path in (
        ("numpy root", numpy_root),
        ("meson build root", meson_build_root),
        ("intro-targets.json", intro_targets),
        ("compile_commands.json", compile_commands),
    ):
        if not path.exists():
            raise SystemExit(
                f"{label} not found: {path}. Run "
                "tools/regen_numpy_multiarray_meson_wasm.py first to produce the "
                "numpy Meson WASM build metadata."
            )

    out_dir = (
        _abs(repo_root, args.out_dir)
        if args.out_dir
        else meson_build_root.parent
        / "pact_numpy_umath_linalg_molt_ext_wasm_cpython_abi"
    )
    if out_dir.exists():
        shutil.rmtree(out_dir, ignore_errors=True)
    out_dir.mkdir(parents=True, exist_ok=True)

    cmd = [
        args.python,
        "-m",
        "molt",
        "extension",
        "build",
        "--project",
        str(numpy_root),
        "--out-dir",
        str(out_dir),
        "--module",
        _MODULE,
        "--target",
        "wasm",
        "--abi-tier",
        "cpython-abi",
        "--source-plan",
        str(intro_targets),
        "--source-plan-target",
        "_umath_linalg",
        "--source-plan-source-root",
        str(numpy_root),
        "--source-plan-build-root",
        str(meson_build_root),
        "--source-plan-compile-commands",
        str(compile_commands),
    ]
    for lib in _EXCLUDED_STATIC_LIBS:
        cmd += ["--source-plan-exclude-linked-static-library", lib]
    cmd += [
        "--capabilities",
        "fs.read",
        "--python-export",
        _MODULE,
        "--no-deterministic",
        "--json",
    ]

    print("[build-umath-linalg] " + " ".join(cmd))
    result = subprocess.run(cmd, cwd=str(repo_root), capture_output=True, text=True)
    sys.stdout.write(result.stdout)
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(
            f"molt extension build failed rc={result.returncode} for {_MODULE}"
        )
    try:
        payload = json.loads(result.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError) as exc:
        raise SystemExit(f"could not parse extension-build JSON: {exc}")
    if payload.get("status") != "ok":
        raise SystemExit(f"extension build reported non-ok status: {payload}")
    data = payload.get("data", {})
    scan = data  # top-level data carries the summary counts
    print(
        f"[build-umath-linalg] object_count={scan.get('object_count')} "
        f"linked_object_count={scan.get('linked_object_count')} "
        f"extension_sha256={scan.get('extension_sha256')}"
    )

    built_artifact = out_dir / "numpy" / "linalg" / _ARTIFACT_NAME
    built_manifest = out_dir / "numpy" / "linalg" / _MANIFEST_NAME
    for path in (built_artifact, built_manifest):
        if not path.is_file():
            raise SystemExit(f"expected build output missing: {path}")

    if args.no_stage:
        print(f"[build-umath-linalg] built (not staged): {built_artifact}")
        return 0

    dest_dir = seal_root / "numpy" / "linalg"
    dest_dir.mkdir(parents=True, exist_ok=True)
    for src in (built_artifact, built_manifest):
        dst = dest_dir / src.name
        shutil.copy2(src, dst)
        print(f"[build-umath-linalg] staged {dst}")
    print(
        "[build-umath-linalg] DONE. Now run tools/pact_seal_witness_roots.py "
        f"--root {seal_root} to ABI-stamp + relativize the sealed manifest."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
