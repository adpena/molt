#!/usr/bin/env python3
"""Build + stage SciPy's ``scipy._lib._ccallback_c`` WASM extension seal.

``scipy/__init__.py`` imports ``scipy._lib._ccallback`` unconditionally
(``from scipy._lib._ccallback import LowLevelCallable``), and
``scipy/_lib/_ccallback.py`` does a module-level ``from . import _ccallback_c``
-- so the absence of the ``_ccallback_c`` Cython C extension blocks the SciPy
IMPORT itself (the pact Kernel A witness ``field_solve.py`` calls
``scipy.ndimage.label``, which cannot import until ``scipy`` imports). This
recipe compiles ``_ccallback_c`` (scipy's ccallback LowLevelCallable machinery)
to a ``wasm32-wasip1`` cpython-abi relocatable object and stages it in a sibling
witness seal root so ``scipy._lib._ccallback_c`` resolves.

Pipeline (the SAME cython-standalone -> C -> ``molt extension build`` path the
ecosystem used for ``scipy.ndimage._ni_label``; never a new pipeline):

  1. ``scipy_config.h`` is materialized from scipy's OWN
     ``scipy/scipy_config.h.in`` via meson's ``configure_file`` rule: every
     ``#mesondefine X`` becomes ``/* #undef X */``. For wasm32-wasip1 (a
     single-threaded WASI target) none of the thread-local-storage features are
     configured, so this is scipy's own header at its wasm cross-config -- byte
     identical to a real scipy meson wasm-cross ``configure_file`` output, never
     a Molt-authored overlay. ``scipy/_lib/src/ccallback.h`` (included by the
     Cython module) needs it.

  2. ``molt.cli.source_extension_cython.regenerate_cython_c_standalone``
     regenerates ``_ccallback_c.c`` from scipy's OWN ``_ccallback_c.pyx`` in the
     STANDALONE (non ``--shared``) shape, so the C embeds its utilities and
     imports no shared ``scipy._cyutility`` module (the same standalone contract
     proven for ``_ni_label``).

  3. A minimal source plan (``intro-targets.json`` + ``compile_commands.json``,
     the exact shape ``molt extension build --source-plan`` consumes and the same
     shape used for the ndimage seal) names the single ``_ccallback_c`` target
     and its generated C with scipy's ``_lib`` / ``_lib/src`` include dirs.

  4. ``molt extension build --target wasm --abi-tier cpython-abi`` compiles the
     translation unit against molt's CPython-ABI headers into
     ``_ccallback_c.molt.wasm`` + an ``extension_manifest.json`` recording the
     object closure. ``_ccallback_c`` cimports only header-only ``.ccallback``
     (``ccallback.h``) + cpython/libc and imports ``ctypes`` at runtime -- it
     needs NO numpy capsule, so the seal is thin and self-contained.

  5. The built ``_ccallback_c.molt.wasm`` + manifest land in the sibling seal
     root's ``scipy/_lib/`` (disjoint from the scipy pure-Python closure and the
     ndimage ``_nd_image``/``_ni_label`` seals). The external-native resolver
     unions native artifacts per package across ``MOLT_MODULE_ROOTS``, so the
     sibling root presents ``_ccallback_c`` as ``scipy._lib._ccallback_c``
     alongside them. Run ``tools/pact_seal_witness_roots.py`` afterwards to
     ABI-stamp + relativize the sealed manifest against the current runtime.

Usage (from the worktree root, RunContext env exported so ``WASI_SYSROOT`` /
``clang`` / ``wasm-ld`` resolve)::

    .venv/Scripts/python.exe tools/build_scipy_ccallback_c_wasm.py \
        --scipy-root bench/friends/repos/scipy_off_the_shelf \
        --seal-root tmp/pact_scipy_ccallback_c_molt_ext_wasm_cpython_abi

then::

    .venv/Scripts/python.exe tools/pact_seal_witness_roots.py \
        --root tmp/pact_scipy_ccallback_c_molt_ext_wasm_cpython_abi
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

_MODULE = "scipy._lib._ccallback_c"
_TARGET = "_ccallback_c"
_ARTIFACT_NAME = "_ccallback_c.molt.wasm"
_MANIFEST_NAME = "_ccallback_c.molt.wasm.extension_manifest.json"


def _repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def _abs(base: Path, raw: str | Path) -> Path:
    p = Path(raw)
    return p.resolve() if p.is_absolute() else (base / p).resolve()


def _clang() -> str:
    return shutil.which("clang") or r"C:\Program Files\LLVM\bin\clang.exe"


def _materialize_scipy_config(scipy_pkg: Path, gen_scipy: Path) -> Path:
    """Materialize ``scipy_config.h`` from scipy's own ``scipy_config.h.in``.

    Applies meson's ``configure_file`` rule for the wasm32-wasip1 cross config:
    with no TLS feature configured, every ``#mesondefine X`` -> ``/* #undef X */``
    (identical to a real scipy meson wasm-cross ``configure_file`` output).
    """
    template = scipy_pkg / "scipy_config.h.in"
    if not template.is_file():
        raise SystemExit(f"scipy_config.h.in not found: {template}")
    rendered = re.sub(
        r"#mesondefine (\w+)", r"/* #undef \1 */", template.read_text(encoding="utf-8")
    )
    gen_scipy.mkdir(parents=True, exist_ok=True)
    out = gen_scipy / "scipy_config.h"
    out.write_text(rendered, encoding="utf-8")
    return out


def main(argv: list[str] | None = None) -> int:
    repo_root = _repo_root()
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--scipy-root",
        default="bench/friends/repos/scipy_off_the_shelf",
        help="scipy source checkout (the extension --project).",
    )
    ap.add_argument(
        "--build-root",
        default="tmp/pact_scipy_ccallback_c_meson_wasm_build_generated",
        help="Working dir for the standalone C + synthesized source plan.",
    )
    ap.add_argument(
        "--seal-root",
        default="tmp/pact_scipy_ccallback_c_molt_ext_wasm_cpython_abi",
        help="Witness seal root the built artifact + manifest are staged into.",
    )
    ap.add_argument(
        "--out-dir",
        default=None,
        help="`molt extension build` staging out-dir (default: the seal root).",
    )
    ap.add_argument(
        "--python",
        default=sys.executable,
        help="Python running `molt` + Cython (needs the molt editable install).",
    )
    ap.add_argument(
        "--no-stage",
        action="store_true",
        help="Build only; do not copy into the seal root.",
    )
    args = ap.parse_args(argv)

    scipy_root = _abs(repo_root, args.scipy_root)
    scipy_pkg = scipy_root / "scipy"
    build_root = _abs(repo_root, args.build_root)
    seal_root = _abs(repo_root, args.seal_root)
    out_dir = _abs(repo_root, args.out_dir) if args.out_dir else seal_root

    pyx = scipy_pkg / "_lib" / "_ccallback_c.pyx"
    for label, path in (("scipy root", scipy_root), ("_ccallback_c.pyx", pyx)):
        if not path.exists():
            raise SystemExit(f"{label} not found: {path}")

    # Import molt's GENERIC standalone Cython regenerator.
    sys.path.insert(0, str(repo_root / "src"))
    from molt.cli import source_extension_cython as cython_authority

    if build_root.exists():
        shutil.rmtree(build_root, ignore_errors=True)
    gen_scipy = build_root / "generated" / "scipy"
    gen_lib = gen_scipy / "_lib"
    gen_lib.mkdir(parents=True, exist_ok=True)

    _materialize_scipy_config(scipy_pkg, gen_scipy)

    cython_version = cython_authority._installed_cython_version(args.python)
    if cython_version is None:
        raise SystemExit("Cython is not importable by the build interpreter")
    regeneration, error = cython_authority.regenerate_cython_c_standalone(
        pyx_path=pyx,
        original_c=Path("_ccallback_c.c"),
        out_dir=gen_lib,
        include_dirs=[scipy_pkg / "_lib", scipy_pkg / "_lib" / "src"],
        cython_version=cython_version,
        python_exe=args.python,
        package_roots=[],
    )
    if error is not None or regeneration is None:
        raise SystemExit(f"standalone Cython regeneration failed: {error}")
    generated_c = regeneration.regenerated_c
    print(f"[build-ccallback-c] standalone C: {generated_c}")

    include_dirs = [scipy_pkg / "_lib", scipy_pkg / "_lib" / "src", gen_scipy]
    inc_args: list[str] = []
    for d in include_dirs:
        inc_args += ["-I", str(d)]

    obj_out = build_root / "scipy" / "_lib" / "_ccallback_c.p" / "_ccallback_c.o"
    compile_args = [
        _clang(),
        "-target",
        "wasm32-wasip1",
        "-DNDEBUG",
        *inc_args,
        "-c",
        str(generated_c),
        "-o",
        str(obj_out),
    ]
    (build_root / "compile_commands.json").write_text(
        json.dumps(
            [
                {
                    "directory": str(scipy_root),
                    "file": str(generated_c),
                    "arguments": compile_args,
                }
            ],
            indent=1,
        ),
        encoding="utf-8",
    )
    (build_root / "intro-targets.json").write_text(
        json.dumps(
            [
                {
                    "id": _MODULE,
                    "name": _TARGET,
                    "type": "shared module",
                    "filename": str(build_root / "scipy" / "_lib" / "_ccallback_c.pyd"),
                    "target_sources": [
                        {
                            "language": "c",
                            "machine": "host",
                            "parameters": inc_args,
                            "sources": ["scipy/_lib/_ccallback_c.pyx"],
                            "generated_sources": [
                                str(generated_c.relative_to(build_root)).replace(
                                    "\\", "/"
                                )
                            ],
                        }
                    ],
                    "linker_parameters": [],
                }
            ],
            indent=1,
        ),
        encoding="utf-8",
    )

    if out_dir.exists() and out_dir == seal_root:
        # Preserve a pre-existing seal dir's siblings; only our own outputs are
        # overwritten by the build below.
        pass
    out_dir.mkdir(parents=True, exist_ok=True)

    cmd = [
        args.python,
        "-m",
        "molt",
        "extension",
        "build",
        "--project",
        str(scipy_root),
        "--out-dir",
        str(out_dir),
        "--module",
        _MODULE,
        "--target",
        "wasm",
        "--abi-tier",
        "cpython-abi",
        "--source-plan",
        str(build_root / "intro-targets.json"),
        "--source-plan-target",
        _TARGET,
        "--source-plan-source-root",
        str(scipy_root),
        "--source-plan-build-root",
        str(build_root),
        "--source-plan-compile-commands",
        str(build_root / "compile_commands.json"),
        "--capabilities",
        "fs.read",
        "--python-export",
        "scipy",
        "--no-deterministic",
        "--json",
    ]
    print("[build-ccallback-c] " + " ".join(cmd))
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
    print(
        f"[build-ccallback-c] object_count={data.get('object_count')} "
        f"linked_object_count={data.get('linked_object_count')} "
        f"extension_sha256={data.get('extension_sha256')}"
    )

    built_artifact = out_dir / "scipy" / "_lib" / _ARTIFACT_NAME
    built_manifest = out_dir / "scipy" / "_lib" / _MANIFEST_NAME
    for path in (built_artifact, built_manifest):
        if not path.is_file():
            raise SystemExit(f"expected build output missing: {path}")

    if args.no_stage or out_dir == seal_root:
        print(f"[build-ccallback-c] built + staged in seal root: {built_artifact}")
        return 0

    dest_dir = seal_root / "scipy" / "_lib"
    dest_dir.mkdir(parents=True, exist_ok=True)
    for src in (built_artifact, built_manifest):
        shutil.copy2(src, dest_dir / src.name)
        print(f"[build-ccallback-c] staged {dest_dir / src.name}")
    print(
        "[build-ccallback-c] DONE. Now run tools/pact_seal_witness_roots.py "
        f"--root {seal_root} to ABI-stamp + relativize the sealed manifest."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
