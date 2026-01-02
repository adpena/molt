import argparse
import ast
import json
import subprocess
import sys
from pathlib import Path
from typing import Literal

from molt.frontend import SimpleTIRGenerator

Target = Literal["native", "wasm"]


def build(file_path: str, target: Target = "native") -> int:
    source_path = Path(file_path)
    if not source_path.exists():
        print(f"File not found: {source_path}", file=sys.stderr)
        return 2

    source = source_path.read_text()

    # 1. Frontend: Python -> JSON IR
    tree = ast.parse(source)
    gen = SimpleTIRGenerator()  # type: ignore[no-untyped-call]
    gen.visit(tree)
    ir = gen.to_json()  # type: ignore[no-untyped-call]

    # 2. Backend: JSON IR -> output.o / output.wasm
    cmd = ["cargo", "run", "--quiet", "--package", "molt-backend", "--"]
    if target == "wasm":
        cmd.extend(["--target", "wasm"])

    backend_process = subprocess.run(cmd, input=json.dumps(ir), text=True)
    if backend_process.returncode != 0:
        print("Backend compilation failed", file=sys.stderr)
        return backend_process.returncode or 1

    if target == "wasm":
        print("Successfully built output.wasm")
        return 0

    # 3. Linking: output.o + main.c -> binary
    main_c_content = """
#include <stdio.h>
#include <stdlib.h>
extern void molt_main();
extern long molt_json_parse_int(const char* ptr, long len);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);
extern void* molt_alloc(long size);
extern long molt_block_on(void* task);
extern long molt_async_sleep(void* obj);
extern void molt_spawn(void* task);
extern void* molt_chan_new();
extern long molt_chan_send(void* chan, long val);
extern long molt_chan_recv(void* chan);
void molt_print_int(long i) {
    printf("%ld\\n", i);
}
int main() {
    molt_main();
    return 0;
}
"""
    Path("main_stub.c").write_text(main_c_content)

    output_binary = "hello_molt"
    runtime_lib = Path("target/release/libmolt_runtime.a")
    if not runtime_lib.exists():
        print(
            f"Runtime library not found: {runtime_lib} (run cargo build)",
            file=sys.stderr,
        )
        return 1

    link_process = subprocess.run(
        ["clang", "main_stub.c", "output.o", str(runtime_lib), "-o", output_binary]
    )

    if link_process.returncode == 0:
        print(f"Successfully built {output_binary}")
    else:
        print("Linking failed", file=sys.stderr)

    return link_process.returncode


def main() -> int:
    parser = argparse.ArgumentParser(prog="molt")
    subparsers = parser.add_subparsers(dest="command", required=True)

    build_parser = subparsers.add_parser("build", help="Compile a Python file")
    build_parser.add_argument("file", help="Path to Python source")
    build_parser.add_argument("--target", choices=["native", "wasm"], default="native")

    args = parser.parse_args()

    if args.command == "build":
        return build(args.file, args.target)

    return 2


if __name__ == "__main__":
    raise SystemExit(main())
