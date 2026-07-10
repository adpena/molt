/* native_cext_driver.c — driver for the native CPython-ABI discovery harness.
 *
 * Usage:
 *   native_cext_driver <harness_lib> <extension_so> <module_name>
 *
 * <harness_lib>   path to libmolt_cext_discovery.{dylib,so}
 * <extension_so>  path to a REAL prebuilt CPython 3.12 extension, e.g.
 *                 numpy/core/_multiarray_umath.cpython-312-darwin.so
 * <module_name>   the PyInit_<name> to drive, e.g. _multiarray_umath
 *
 * We dlopen the harness with RTLD_GLOBAL so its exported Py* ABI symbols enter
 * the global (flat) namespace; the extension's `-undefined dynamic_lookup`
 * imports then bind to them. Then we register the REAL molt-runtime hooks and
 * drive PyInit against the molt ABI. This is intentionally a thin C shim so a
 * hard native crash inside numpy is captured cleanly by lldb (no Rust bin
 * unwinding to confuse the backtrace).
 */
#include <dlfcn.h>
#include <stdio.h>

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr,
                "usage: %s <harness_lib> <extension_so> <module_name>\n",
                argv[0]);
        return 64;
    }

    void *h = dlopen(argv[1], RTLD_NOW | RTLD_GLOBAL);
    if (!h) {
        fprintf(stderr, "===MOLT_DISCOVERY_FRONTIER: dlopen(harness) failed: %s\n",
                dlerror());
        return 65;
    }

    void (*init)(void) = (void (*)(void)) dlsym(h, "molt_cext_discovery_init");
    if (!init) {
        fprintf(stderr, "dlsym(molt_cext_discovery_init) failed: %s\n", dlerror());
        return 66;
    }

    int (*load)(const char *, const char *) =
        (int (*)(const char *, const char *)) dlsym(h, "molt_cext_discovery_load");
    if (!load) {
        fprintf(stderr, "dlsym(molt_cext_discovery_load) failed: %s\n", dlerror());
        return 67;
    }

    init();
    int rc = load(argv[2], argv[3]);
    fprintf(stderr, "===MOLT_DISCOVERY_RC=%d\n", rc);
    return rc == 0 ? 0 : 10;
}
