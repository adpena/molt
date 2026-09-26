/* One C-extension initializer linkage/export authority for both Molt headers. */
#ifndef MOLT_MODULE_INIT_EXPORT_H
#define MOLT_MODULE_INIT_EXPORT_H

#if defined(_WIN32)
#define MOLT_MODULE_INIT_EXPORT __declspec(dllexport)
#elif defined(__GNUC__) || defined(__clang__)
#define MOLT_MODULE_INIT_EXPORT __attribute__((visibility("default")))
#else
#define MOLT_MODULE_INIT_EXPORT
#endif

#ifdef __cplusplus
#define MOLT_PYMODINIT_FUNC extern "C" MOLT_MODULE_INIT_EXPORT PyObject *
#else
#define MOLT_PYMODINIT_FUNC MOLT_MODULE_INIT_EXPORT PyObject *
#endif

#endif /* MOLT_MODULE_INIT_EXPORT_H */
