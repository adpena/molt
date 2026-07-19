"""R73.2 — Molt regenerates a Cython extension's C STANDALONE.

Teeth: the real ``scipy/ndimage/src/_ni_label.pyx`` is regenerated through
Molt's standalone-Cython authority and the resulting C must carry ZERO
``scipy._cyutility`` references (and no ``__Pyx_modinit_shared_function_import``
shared-utility helper). The test also proves the assertion has teeth by
generating the ``--shared scipy._cyutility`` variant that Molt must NOT consume
and asserting THAT C *does* carry the ``_cyutility`` import — so a regression
that consumed the ``--shared`` C would fail this test.
"""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from molt.cli import source_extension_cython as cython_authority


ROOT = Path(__file__).resolve().parents[2]


def _find_ni_label_pyx() -> Path | None:
    """Locate the vendored scipy ``_ni_label.pyx`` (bench repo, gitignored)."""
    relative = Path(
        "bench/friends/repos/scipy_off_the_shelf/scipy/ndimage/src/_ni_label.pyx"
    )
    for base in (ROOT, Path.cwd(), *ROOT.parents):
        candidate = base / relative
        if candidate.is_file():
            return candidate.resolve()
    # Fall back to a broad search rooted at the repo checkout.
    for match in ROOT.rglob("_ni_label.pyx"):
        return match.resolve()
    return None


def _find_numpy_source_root() -> Path | None:
    """Locate the package-custody NumPy source root when the bench repo exists."""
    relative = Path("bench/friends/repos/numpy_off_the_shelf")
    for base in (ROOT, Path.cwd(), *ROOT.parents):
        candidate = base / relative
        if (candidate / "numpy" / "__init__.pxd").is_file():
            return candidate.resolve()
    return None


def _cython_available() -> bool:
    return cython_authority._installed_cython_version(sys.executable) is not None


PYX_PATH = _find_ni_label_pyx()
NUMPY_SOURCE_ROOT = _find_numpy_source_root()

requires_ni_label = pytest.mark.skipif(
    PYX_PATH is None or NUMPY_SOURCE_ROOT is None,
    reason=(
        "vendored scipy _ni_label.pyx and numpy package-custody source root "
        "(bench/friends/repos/...) are not present"
    ),
)
requires_cython = pytest.mark.skipif(
    not _cython_available(),
    reason="Cython is not importable by the test interpreter",
)
requires_clang = pytest.mark.skipif(
    shutil.which("clang") is None,
    reason="clang is required for the regenerated-C preprocess proof",
)


def test_parse_cython_build_requirement_reads_version_floor() -> None:
    requirement = cython_authority.parse_cython_build_requirement(
        ["numpy>=1.25", "Cython>=3.0.6,<3.2", "meson-python"]
    )
    assert requirement is not None
    assert requirement.minimum_major() == 3
    assert cython_authority._pip_specifier(requirement) == "Cython>=3.0.6,<3.2"


def test_parse_cython_build_requirement_bare_pins_major() -> None:
    requirement = cython_authority.parse_cython_build_requirement(["Cython"])
    assert requirement is not None
    # A bare requirement pins the 3.x floor scipy needs, never an unbounded pin.
    assert cython_authority._pip_specifier(requirement) == "Cython>=3.0"


def test_cython_build_requirement_from_pyproject() -> None:
    requirement = cython_authority.cython_build_requirement_from_pyproject(
        {"build-system": {"requires": ["Cython==3.1.8", "numpy"]}}
    )
    assert requirement is not None
    assert requirement.raw == "Cython==3.1.8"


def test_regenerated_c_compile_profile_is_exact_and_manifested(
    tmp_path: Path,
) -> None:
    regeneration = cython_authority.CythonRegeneration(
        pyx_path=tmp_path / "probe.pyx",
        original_c=tmp_path / "probe.c",
        regenerated_c=tmp_path / "generated" / "probe.c",
        cython_version="test",
        cython_argv=("python", "-m", "cython", "-3"),
    )
    exact_policy = (
        "-DCYTHON_FAST_THREAD_STATE=0",
        "-DCYTHON_USE_UNICODE_INTERNALS=0",
        "-DCYTHON_USE_PYLIST_INTERNALS=0",
        "-DCYTHON_USE_PYLONG_INTERNALS=0",
        "-DCYTHON_USE_PYTYPE_LOOKUP=0",
        "-DCYTHON_ASSUME_SAFE_MACROS=0",
        "-DCYTHON_ASSUME_SAFE_SIZE=0",
        "-DCYTHON_UNPACK_METHODS=0",
        "-DCYTHON_AVOID_BORROWED_REFS=1",
        "-DCYTHON_AVOID_THREAD_UNSAFE_BORROWED_REFS=1",
    )
    assert cython_authority.CYTHON_CPYTHON_ABI_COMPILE_ARGS == exact_policy
    assert regeneration.manifest_payload()["compile_profile"] == (
        "molt-cpython-abi-safe-v1"
    )
    assert regeneration.manifest_payload()["compile_args"] == list(exact_policy)


@requires_cython
@requires_clang
def test_regenerated_cython_safe_cpython_profile_is_fail_closed(
    tmp_path: Path,
) -> None:
    """The profile selects public-error CPython branches without unsafe internals."""

    pyx_path = tmp_path / "limited_probe.pyx"
    pyx_path.write_text(
        "def probe(list values, dict mapping, str text, int number):\n"
        "    return values[0], mapping[text], len(text), number + 1\n",
        encoding="utf-8",
    )
    version = cython_authority._installed_cython_version(sys.executable)
    assert version is not None
    regeneration, error = cython_authority.regenerate_cython_c_standalone(
        pyx_path=pyx_path,
        original_c=tmp_path / "limited_probe.c",
        out_dir=tmp_path / "generated",
        include_dirs=(),
        cython_version=version,
        python_exe=sys.executable,
    )
    assert error is None, error
    assert regeneration is not None
    compile_args = cython_authority.CYTHON_CPYTHON_ABI_COMPILE_ARGS
    assert all("Py_LIMITED_API" not in arg for arg in compile_args)

    clang = shutil.which("clang")
    assert clang is not None
    abi_include = ROOT / "runtime" / "molt-cpython-abi" / "include"
    preprocess = run_guarded_test_process(
        [
            clang,
            "-E",
            *compile_args,
            "-I",
            str(abi_include),
            str(regeneration.regenerated_c),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    assert preprocess.returncode == 0, preprocess.stderr
    macro_dump = run_guarded_test_process(
        [
            clang,
            "-dM",
            "-E",
            *compile_args,
            "-I",
            str(abi_include),
            str(regeneration.regenerated_c),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    assert macro_dump.returncode == 0, macro_dump.stderr
    assert re.search(
        r"^#define CYTHON_COMPILING_IN_CPYTHON 1$", macro_dump.stdout, re.MULTILINE
    )
    assert re.search(
        r"^#define CYTHON_COMPILING_IN_LIMITED_API 0$",
        macro_dump.stdout,
        re.MULTILINE,
    )

    for macro in (
        "CYTHON_FAST_THREAD_STATE",
        "CYTHON_USE_UNICODE_INTERNALS",
        "CYTHON_USE_PYLIST_INTERNALS",
        "CYTHON_USE_PYLONG_INTERNALS",
        "CYTHON_USE_PYTYPE_LOOKUP",
        "CYTHON_ASSUME_SAFE_MACROS",
        "CYTHON_ASSUME_SAFE_SIZE",
        "CYTHON_UNPACK_METHODS",
    ):
        assert re.search(
            rf"^#define {macro} 0$", macro_dump.stdout, re.MULTILINE
        ), macro
    assert re.search(
        r"^#define CYTHON_AVOID_BORROWED_REFS 1$",
        macro_dump.stdout,
        re.MULTILINE,
    )

    # Check the disabled internal families after preprocessing. The one
    # unavoidable PyListObject.ob_item construction write is covered by Molt's
    # concrete list layout and is therefore deliberately not forbidden here.
    forbidden_field_access = {
        "Long": r"->\s*(?:long_value|ob_digit)\b",
        "Unicode": (
            r"->\s*(?:wstr_length|utf8|utf8_length)\b"
            r"|->\s*state\s*\.\s*(?:kind|ascii|compact|ready)\b"
        ),
        "Dict": r"->\s*ma_(?:used|keys|values)\b",
    }
    for builtin, pattern in forbidden_field_access.items():
        assert re.search(pattern, preprocess.stdout) is None, (
            f"safe CPython profile still contains direct {builtin} field access"
        )


def test_parse_cimported_packages_derives_from_source(tmp_path: Path) -> None:
    # The cimport include surface is DERIVED FROM SOURCE: every top-level name in a
    # cimport, minus relative cimports and Cython's bundled namespaces. This is what
    # lets `cimport numpy` resolve numpy/__init__.pxd instead of crashing Cython 3.1+.
    pyx = tmp_path / "_ni_label.pyx"
    pyx.write_text(
        "import numpy as np\n"
        "cimport numpy as np\n"
        "cimport scipy.special as sc, pandas._libs as pdlibs\n"
        "from widgetlib.tensor cimport Tensor\n"
        "from libc.stdlib cimport malloc\n"      # bundled -> excluded
        "cimport cython\n"                        # bundled -> excluded
        "from . cimport _helpers\n"               # relative -> excluded
        "from scipy.ndimage cimport _ni_support\n"  # top-level 'scipy'
        "cdef int x = 0\n",
        encoding="utf-8",
    )
    # A sibling .pxd contributes its cimports too.
    (tmp_path / "shared.pxd").write_text(
        "from anotherpkg._headers cimport HeaderThing\n",
        encoding="utf-8",
    )
    pkgs = cython_authority._parse_cimported_packages(pyx)
    assert pkgs == (
        "anotherpkg",
        "numpy",
        "pandas",
        "scipy",
        "widgetlib",
    ), pkgs
    for excluded in ("libc", "cython", "_helpers"):
        assert excluded not in pkgs


def test_cimport_pxd_roots_fail_safe_on_absent_package() -> None:
    # A package that is not installed (or ships no top-level __init__.pxd) is
    # silently skipped — never breaking a build for a pure-C/bundled cimport.
    roots = cython_authority._cimport_pxd_roots(
        sys.executable, ("molt_definitely_absent_pkg_xyz",)
    )
    assert roots == ()
    assert cython_authority._cimport_pxd_roots(sys.executable, ()) == ()


def test_cimport_pxd_roots_resolves_from_source_tree(tmp_path: Path) -> None:
    # A source-recompiled witness builds against the package's OWN source tree, not
    # a pip install: numpy's `.pxd` is at `<numpy-src>/numpy/__init__.pxd` while the
    # build plan only puts `<numpy-src>/numpy/_core/include` on -I. The resolver must
    # walk that include dir's ancestors and add `<numpy-src>` so `cimport numpy`
    # resolves — WITHOUT the package being importable by the build interpreter.
    src = tmp_path / "pkg_off_the_shelf"
    (src / "widget" / "_core" / "include").mkdir(parents=True)
    (src / "widget" / "__init__.pxd").write_text("# widget pxd\n", encoding="utf-8")
    # The plan surfaces only the deep C-header dir; ancestor walk must reach `src`.
    roots = cython_authority._cimport_pxd_roots(
        sys.executable,
        ("widget",),
        search_roots=[src / "widget" / "_core" / "include"],
    )
    assert roots == (src.resolve(),), roots
    assert (roots[0] / "widget" / "__init__.pxd").is_file()


def test_cimport_roots_use_module_roots_package_custody(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    dep_root = tmp_path / "deps" / "widget_off_the_shelf"
    header_root = dep_root / "widget" / "_core" / "include"
    (header_root / "widget").mkdir(parents=True)
    (dep_root / "widget" / "__init__.pxd").write_text(
        "ctypedef unsigned long index_t\n",
        encoding="utf-8",
    )
    (header_root / "widget" / "arrayobject.h").write_text(
        "/* package-owned header */\n",
        encoding="utf-8",
    )
    consumer = tmp_path / "consumer" / "scipy" / "ndimage" / "src"
    consumer.mkdir(parents=True)
    monkeypatch.setenv(
        "MOLT_MODULE_ROOTS",
        os.pathsep.join((f"widget={dep_root}", str(tmp_path / "missing"))),
    )

    roots = cython_authority._cimport_pxd_roots(
        sys.executable,
        ("widget",),
        search_roots=[
            consumer,
            *cython_authority._cimport_package_roots_from_env(),
        ],
    )

    assert roots == (dep_root.resolve(),), roots
    assert cython_authority._cimport_header_include_dirs(
        sys.executable,
        ("widget",),
        cimport_pxd_roots=roots,
    ) == (header_root.resolve(),)


def test_cimport_pxd_roots_resolve_through_build_interpreter(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    site_root = tmp_path / "site"
    pxd_package = site_root / "pxdpackage"
    pxd_package.mkdir(parents=True)
    (pxd_package / "__init__.py").write_text("", encoding="utf-8")
    (pxd_package / "__init__.pxd").write_text(
        "ctypedef unsigned long index_t\n",
        encoding="utf-8",
    )
    header_only = site_root / "headeronly"
    header_only.mkdir()
    (header_only / "__init__.py").write_text("", encoding="utf-8")
    monkeypatch.setenv("PYTHONPATH", str(site_root))

    roots = cython_authority._cimport_pxd_roots(
        sys.executable,
        ("headeronly", "missingpkg", "pxdpackage", "libc"),
    )

    assert roots == (site_root.resolve(),)


def test_cimport_header_include_dirs_resolve_package_get_include(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    site_root = tmp_path / "site"
    include_root = tmp_path / "headers"
    include_root.mkdir()
    pxd_package = site_root / "pxdpackage"
    pxd_package.mkdir(parents=True)
    (pxd_package / "__init__.py").write_text(
        "def get_include():\n"
        f"    return {str(include_root)!r}\n",
        encoding="utf-8",
    )
    no_hook = site_root / "nohook"
    no_hook.mkdir()
    (no_hook / "__init__.py").write_text("", encoding="utf-8")
    monkeypatch.setenv("PYTHONPATH", str(site_root))

    roots = cython_authority._cimport_header_include_dirs(
        sys.executable,
        ("libc", "missingpkg", "nohook", "pxdpackage"),
    )

    assert roots == (include_root.resolve(),)


def test_pair_generated_c_with_pyx_matches_by_stem() -> None:
    pyx = Path("/pkg/src/_ni_label.pyx")
    assert (
        cython_authority.pair_generated_c_with_pyx(
            generated_c=Path("/build/_ni_label.c"),
            pyx_candidates=[pyx],
        )
        is not None
    )
    # No spurious pairing for an unrelated generated C.
    assert (
        cython_authority.pair_generated_c_with_pyx(
            generated_c=Path("/build/nd_image.c"),
            pyx_candidates=[pyx],
        )
        is None
    )
    # A header is never a Cython-generated C.
    assert (
        cython_authority.pair_generated_c_with_pyx(
            generated_c=Path("/build/_ni_label.h"),
            pyx_candidates=[pyx],
        )
        is None
    )
    # Duplicate same-stem inputs are ambiguous; never select by list order.
    assert (
        cython_authority.pair_generated_c_with_pyx(
            generated_c=Path("/build/_ni_label.c"),
            pyx_candidates=[pyx, Path("/other/_ni_label.pyx")],
        )
        is None
    )


def test_regeneration_replays_real_ninja_cython_directives(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source_root = tmp_path / "source"
    build_root = tmp_path / "build"
    pyx = source_root / "pkg" / "probe.pyx"
    original_c = build_root / "pkg" / "probe.c"
    pyx.parent.mkdir(parents=True)
    build_root.mkdir()
    pyx.write_text("def probe():\n    return 1\n", encoding="utf-8")
    (build_root / "build.ninja").write_text("# queried through ninja -t commands\n")
    ninja_command = subprocess.list2cmdline(
        [
            "/tools/cython",
            "-M",
            "-3",
            "--fast-fail",
            "-Xfreethreading_compatible=True",
            "--shared=scipy._cyutility",
            "--include-dir",
            "ignored",
            str(pyx),
            "-o",
            str(original_c),
        ]
    )
    generation_calls: list[list[str]] = []
    query_calls: list[list[str]] = []

    def fake_run(
        argv: list[str],
        **_kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        if "commands" in argv:
            query_calls.append(argv)
            return subprocess.CompletedProcess(argv, 0, ninja_command + "\n", "")
        generation_calls.append(argv)
        output = Path(argv[argv.index("-o") + 1])
        output.write_text("/* generated */\n", encoding="utf-8")
        Path(str(output) + ".dep").write_text(
            f"{output}: {pyx}\n",
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(argv, 0, "", "")

    monkeypatch.setattr(
        cython_authority.process_guard,
        "run_completed_command",
        fake_run,
    )
    regeneration, error = cython_authority.regenerate_cython_c_standalone(
        pyx_path=pyx,
        original_c=original_c,
        out_dir=build_root / "standalone",
        include_dirs=(),
        cython_version="test",
        python_exe=sys.executable,
        package_roots=(source_root, build_root),
        ninja_command=(sys.executable, "-m", "ninja"),
    )

    assert error is None, error
    assert regeneration is not None
    assert generation_calls == [list(regeneration.cython_argv)]
    assert query_calls[0][:3] == [sys.executable, "-m", "ninja"]
    argv = regeneration.cython_argv
    assert argv[:3] == (sys.executable, "-m", "cython")
    assert argv[3:7] == (
        "-M",
        "-3",
        "--fast-fail",
        "-Xfreethreading_compatible=True",
    )
    assert not any(arg == "--shared" or arg.startswith("--shared=") for arg in argv)
    assert "ignored" not in argv
    assert argv.count(str(pyx)) == 1
    assert argv.count(str(regeneration.regenerated_c)) == 1
    assert regeneration.dependencies[0].path == pyx.resolve()
    assert regeneration.manifest_payload()["dependencies"] == [
        {
            "path": str(pyx.resolve()),
            "sha256": regeneration.dependencies[0].sha256,
        }
    ]


def test_ninja_command_strips_separate_shared_and_replaced_paths(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source_root = tmp_path / "source"
    build_root = tmp_path / "build"
    pyx = source_root / "pkg" / "probe.pyx"
    original_c = build_root / "pkg" / "probe.c"
    pyx.parent.mkdir(parents=True)
    build_root.mkdir()
    pyx.write_text("def probe():\n    return 1\n", encoding="utf-8")
    (build_root / "build.ninja").write_text("# queried through ninja -t commands\n")
    command = subprocess.list2cmdline(
        [
            "/tools/cython",
            "-3",
            "--fast-fail",
            "-X",
            "freethreading_compatible=True",
            "--output-file",
            str(original_c),
            "--include-dir",
            ".",
            str(pyx),
            "--shared",
            "scipy._cyutility",
        ]
    )
    monkeypatch.setattr(
        cython_authority.process_guard,
        "run_completed_command",
        lambda argv, **_kwargs: subprocess.CompletedProcess(
            argv, 0, command + "\n", ""
        ),
    )

    args, error = cython_authority._cython_generator_args_from_ninja(
        pyx_path=pyx,
        original_c=original_c,
        package_roots=(source_root, build_root),
        ninja_command=(sys.executable, "-m", "ninja"),
    )

    assert error is None, error
    assert args == ("-3", "--fast-fail", "-X", "freethreading_compatible=True")


def test_ninja_generator_command_ambiguity_fails_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source_root = tmp_path / "source"
    build_root = tmp_path / "build"
    pyx = source_root / "pkg" / "probe.pyx"
    original_c = build_root / "pkg" / "probe.c"
    pyx.parent.mkdir(parents=True)
    build_root.mkdir()
    pyx.write_text("def probe():\n    return 1\n", encoding="utf-8")
    (build_root / "build.ninja").write_text("# queried through ninja -t commands\n")
    command = subprocess.list2cmdline(
        ["/tools/cython", "-3", str(pyx), "-o", str(original_c)]
    )
    monkeypatch.setattr(
        cython_authority.process_guard,
        "run_completed_command",
        lambda argv, **_kwargs: subprocess.CompletedProcess(
            argv, 0, f"{command}\n{command}\n", ""
        ),
    )

    args, error = cython_authority._cython_generator_args_from_ninja(
        pyx_path=pyx,
        original_c=original_c,
        package_roots=(source_root, build_root),
        ninja_command=(sys.executable, "-m", "ninja"),
    )

    assert args is None
    assert error is not None
    assert "ambiguously contains" in error


@requires_ni_label
@requires_cython
def test_molt_regenerates_ni_label_standalone_without_cyutility(
    tmp_path: Path,
) -> None:
    assert PYX_PATH is not None
    assert NUMPY_SOURCE_ROOT is not None
    version = cython_authority._installed_cython_version(sys.executable)
    assert version is not None
    regeneration, error = cython_authority.regenerate_cython_c_standalone(
        pyx_path=PYX_PATH,
        original_c=Path("_ni_label.c"),
        out_dir=tmp_path / "standalone",
        include_dirs=[PYX_PATH.parent],
        cython_version=version,
        python_exe=sys.executable,
        package_roots=[NUMPY_SOURCE_ROOT],
    )
    assert error is None, error
    assert regeneration is not None
    generated = regeneration.regenerated_c.read_text(encoding="utf-8", errors="replace")

    # Teeth: standalone C embeds its utilities; it imports no shared-util module.
    assert "_cyutility" not in generated, (
        "Molt's standalone Cython regeneration must NOT import scipy._cyutility; "
        "found a shared-utility reference in the regenerated C"
    )
    assert "__Pyx_modinit_shared_function_import" not in generated, (
        "standalone regeneration must not emit the shared-function-import helper"
    )
    # It is a real, substantial recompile of scipy's own source, not a stub.
    assert generated.count("\n") > 10000
    assert "PyInit__ni_label" in generated
    # ``-3`` (no ``--shared``) is the standalone contract.
    assert "-3" in regeneration.cython_argv
    assert "--shared" not in regeneration.cython_argv
    assert "numpy" in regeneration.cimport_packages
    assert any(
        (path / "numpy" / "__init__.pxd").is_file()
        for path in regeneration.cimport_pxd_roots
    )
    assert any(
        (path / "numpy" / "arrayobject.h").is_file()
        for path in regeneration.cimport_header_include_dirs
    )


@requires_ni_label
@requires_cython
def test_shared_variant_would_import_cyutility_proving_teeth(
    tmp_path: Path,
) -> None:
    """The ``--shared`` C Molt must NOT consume DOES import scipy._cyutility.

    This is the negative control: it proves the standalone test above would
    fail if Molt regressed to consuming the upstream ``--shared`` C.
    """
    assert PYX_PATH is not None
    assert NUMPY_SOURCE_ROOT is not None
    shared_c = tmp_path / "_ni_label_shared.c"
    cimport_packages = cython_authority._parse_cimported_packages(PYX_PATH)
    cimport_pxd_roots = cython_authority._cimport_pxd_roots(
        sys.executable,
        cimport_packages,
        search_roots=[PYX_PATH.parent, NUMPY_SOURCE_ROOT],
    )
    include_args = [
        arg
        for include_dir in cython_authority._cython_include_dirs(
            pyx_path=PYX_PATH,
            plan_include_dirs=[PYX_PATH.parent],
            cimport_pxd_roots=cimport_pxd_roots,
        )
        for arg in ("-I", str(include_dir))
    ]
    result = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "cython",
            "-3",
            "--shared",
            "scipy._cyutility",
            *include_args,
            str(PYX_PATH),
            "-o",
            str(shared_c),
        ],
        capture_output=True,
        text=True,
        timeout=600,
        check=False,
    )
    assert result.returncode == 0 and shared_c.is_file(), result.stderr or result.stdout
    shared_text = shared_c.read_text(encoding="utf-8", errors="replace")
    # The upstream --shared C carries exactly the blocker Molt bypasses.
    assert "_cyutility" in shared_text, (
        "expected the --shared variant to import scipy._cyutility; if it does "
        "not, the standalone assertion has no teeth"
    )
