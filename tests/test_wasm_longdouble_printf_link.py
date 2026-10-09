"""Execute the selected SDK's binary128 C ABI and real Meson C++ defaults."""

from __future__ import annotations

import os
from pathlib import Path
import sys

import pytest

from molt.cli import wasm_link_inputs
from molt.cli.source_extension_target import resolve_source_extension_target_plan
from molt.cli.source_extension_toolchain import (
    _admit_wasi_source_compiler,
    _meson_cross_text,
)
from molt.cli.wasm_host import resolve_molt_wasm_host_binary
from molt.source_root import compiler_source_root
from tests.process_guard_common import run_guarded_test_process


def _host() -> str:
    host = resolve_molt_wasm_host_binary(
        compiler_source_root(), cargo_profile="dev-fast"
    )
    if host is None:
        pytest.fail("required molt-wasm-host execution capability is unavailable")
    return str(host)


def test_final_link_file_backed_long_double_round_trip(tmp_path: Path) -> None:
    plan = wasm_link_inputs.resolve_wasi_c_abi_plan()
    host = _host()
    source = tmp_path / "long_double_file.c"
    source.write_text(
        r"""#define _GNU_SOURCE
#include <float.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static int format(char *out, size_t n, const char *fmt, ...) {
    va_list args;
    va_start(args, fmt);
    int result = vsnprintf(out, n, fmt, args);
    va_end(args);
    return result;
}
int main(void) {
    /* Exact independently derived decimal rounding of 1 + 2^-100. */
    const char expected[] = "1.00000000000000000000000000000078886";
    long double original = 1.0L + 0x1p-100L;
    if (sizeof(long double) != 16 || LDBL_MANT_DIG != 113 || original == 1.0L) return 10;
    char buffer[256] = {0};
    FILE *file = fmemopen(buffer, sizeof buffer, "w+");
    if (!file) return 11;
    if (fprintf(file, "%.36Lg", original) != (int)strlen(expected) || fflush(file)) return 12;
    if (strcmp(buffer, expected)) return 13;
    rewind(file);
    long double parsed = 0.0L;
    if (fscanf(file, "%Lg", &parsed) != 1 || parsed != original) return 14;
    if (fclose(file)) return 15;
    char *end = NULL;
    if (strtold(expected, &end) != original || !end || *end) return 16;
    const char hexadecimal[] = "0x1.0000000000000000000000001p+0tail";
    if (strtold(hexadecimal, &end) != original
        || end != hexadecimal + sizeof hexadecimal - 5 || strcmp(end, "tail")) return 22;
    if (snprintf(buffer, sizeof buffer, "%.36Lg", original) != (int)strlen(expected)
        || strcmp(buffer, expected)) return 17;
    if (format(buffer, sizeof buffer, "%.36Lg", original) != (int)strlen(expected)
        || strcmp(buffer, expected)) return 18;
    struct { unsigned char before; char text[8]; unsigned char after; } bounded;
    memset(&bounded, 0xa5, sizeof bounded);
    if (snprintf(bounded.text, sizeof bounded.text, "%.36Lg", original) != (int)strlen(expected)
        || bounded.before != 0xa5 || bounded.after != 0xa5
        || memcmp(bounded.text, "1.00000\0", 8)) return 23;
    memset(&bounded, 0xa5, sizeof bounded);
    if (format(bounded.text, sizeof bounded.text, "%.36Lg", original) != (int)strlen(expected)
        || bounded.before != 0xa5 || bounded.after != 0xa5
        || memcmp(bounded.text, "1.00000\0", 8)) return 19;
    char sentinel = 'Q';
    if (snprintf(&sentinel, 0, "%.36Lg", original) != (int)strlen(expected) || sentinel != 'Q') return 20;
    if (format(&sentinel, 0, "%.36Lg", original) != (int)strlen(expected) || sentinel != 'Q') return 24;
    struct { unsigned char before; char text[1]; unsigned char after; } one;
    memset(&one, 0xa5, sizeof one);
    if (snprintf(one.text, 1, "%.36Lg", original) != (int)strlen(expected)
        || one.before != 0xa5 || one.after != 0xa5 || one.text[0]) return 25;
    memset(&one, 0xa5, sizeof one);
    if (format(one.text, 1, "%.36Lg", original) != (int)strlen(expected)
        || one.before != 0xa5 || one.after != 0xa5 || one.text[0]) return 26;
    if (printf("%.36Lg\n", original) != (int)strlen(expected) + 1 || fflush(stdout)) return 21;
    puts("long-double-file-ok");
    return 0;
}
""",
        encoding="utf-8",
    )
    output = tmp_path / "long_double_file.wasm"
    run_guarded_test_process(
        [
            str(plan.driver),
            "--no-default-config",
            "--target=wasm32-wasip1",
            f"--sysroot={plan.sysroot}",
            "-O2",
            str(source),
            "-Wl,--whole-archive",
            str(plan.path("long_double")),
            "-Wl,--no-whole-archive",
            str(plan.path("compiler_rt")),
            "-o",
            str(output),
        ],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=True,
        timeout=120,
    )
    execution = run_guarded_test_process(
        [host, "--wasi-command", str(output)],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=True,
        timeout=30,
    )
    assert execution.stdout.splitlines() == [
        "1.00000000000000000000000000000078886",
        "long-double-file-ok",
    ]


def test_meson_cpp_driver_links_standard_library_and_abi(tmp_path: Path) -> None:
    plan = wasm_link_inputs.resolve_wasi_c_abi_plan()
    host = _host()
    # Exercise the supported shared-image entrypoint with an explicit C++ mode.
    cpp = _admit_wasi_source_compiler(
        (
            str(plan.driver),
            "--driver-mode=g++",
            "--target=wasm32-wasip1",
            "-fno-exceptions",
        ),
        role="cpp",
        plan=plan,
        environment=os.environ,
    )
    c = _admit_wasi_source_compiler(
        (str(plan.driver), "--target=wasm32-wasip1"),
        role="c",
        plan=plan,
        environment=os.environ,
    )
    source = tmp_path / "project"
    source.mkdir()
    (source / "meson.build").write_text(
        "project('wasi_cpp_role', 'cpp')\nexecutable('cpp_abi', 'main.cpp', name_suffix: 'wasm')\n",
        encoding="utf-8",
    )
    (source / "main.cpp").write_text(
        r"""#include <cstdio>
#include <string>
#include <vector>
struct Base { virtual ~Base() {} };
struct Derived : Base { std::string value = "sdk-cpp-ok"; };
int main() {
    Base *base = new Derived;
    Derived *value = dynamic_cast<Derived *>(base);
    std::vector<std::string> words = {"sdk", "cpp", "ok"};
    std::string expected = words[0] + "-" + words[1] + "-" + words[2];
    if (!value || value->value != expected) return 1;
    std::puts(value->value.c_str());
    delete base;
    return 0;
}
""",
        encoding="utf-8",
    )
    # Linker detection may transiently overwrite and remove a.out. Preserve an
    # authored file of that name, not merely the final directory's membership.
    (source / "a.out").write_bytes(b"authored source content")
    source_before = {path.name: path.read_bytes() for path in source.iterdir()}
    cross = tmp_path / "cross.ini"
    cross.write_text(
        _meson_cross_text(
            target_plan=resolve_source_extension_target_plan("wasm"),
            pkg_config_dir=tmp_path / "pkgconfig",
            commands={
                "c": c,
                "cpp": cpp,
                "ar": (str(plan.driver.with_name("llvm-ar" + plan.driver.suffix)),),
            },
            compiler_rt=plan.path("compiler_rt"),
            include_dirs=(),
        ),
        encoding="utf-8",
    )
    build = tmp_path / "build"
    build.mkdir()
    meson = [sys.executable, "-m", "mesonbuild.mesonmain"]
    run_guarded_test_process(
        [*meson, "setup", str(build), str(source), "--cross-file", str(cross)],
        cwd=build,
        capture_output=True,
        text=True,
        check=True,
        timeout=120,
    )
    run_guarded_test_process(
        [*meson, "compile", "-C", str(build)],
        cwd=build,
        capture_output=True,
        text=True,
        check=True,
        timeout=120,
    )
    execution = run_guarded_test_process(
        [host, "--wasi-command", str(build / "cpp_abi.wasm")],
        cwd=build,
        capture_output=True,
        text=True,
        check=True,
        timeout=30,
    )
    assert {path.name: path.read_bytes() for path in source.iterdir()} == source_before
    assert execution.stdout.strip() == "sdk-cpp-ok"
