import sys
import subprocess
import json
from molt.frontend import SimpleTIRGenerator
import ast

def build(file_path, target="native"):
    with open(file_path, "r") as f:
        source = f.read()

    # 1. Frontend: Python -> JSON IR
    tree = ast.parse(source)
    gen = SimpleTIRGenerator()
    gen.visit(tree)
    ir = gen.to_json()

    # 2. Backend: JSON IR -> output.o / output.wasm
    cmd = ["cargo", "run", "--quiet", "--package", "molt-backend", "--"]
    if target == "wasm":
        cmd.extend(["--target", "wasm"])
    
    backend_process = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        text=True
    )
    backend_process.communicate(input=json.dumps(ir))
    
    if backend_process.returncode != 0:
        print("Backend compilation failed")
        return

    if target == "wasm":
        print("Successfully built output.wasm")
        return

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
    with open("main_stub.c", "w") as f:
        f.write(main_c_content)

    output_binary = "hello_molt"
    runtime_lib = "target/release/libmolt_runtime.a"
    link_process = subprocess.run([
        "clang", "main_stub.c", "output.o", runtime_lib, "-o", output_binary
    ])

    if link_process.returncode == 0:
        print(f"Successfully built {output_binary}")
    else:
        print("Linking failed")

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: molt build <file.py> [--target wasm]")
    else:
        command = sys.argv[1]
        if command == "build":
            target = "native"
            file_path = sys.argv[2]
            if len(sys.argv) > 3 and sys.argv[3] == "--target" and sys.argv[4] == "wasm":
                target = "wasm"
            build(file_path, target)
