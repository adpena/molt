/* Native errno namespace controls, before every system header.
 * CPython v3.12.13/v3.13.11/v3.14.3/v3.15.0 configure.ac explicitly
 * defines _NETBSD_SOURCE, __BSD_VISIBLE and _DARWIN_C_SOURCE. Its generated
 * AC_USE_SYSTEM_EXTENSIONS block also enables GNU and NetBSD compatibility
 * extensions. This is not a replacement pyconfig.h or an ABI policy.
 * See the owning runtime contract for target-specific configure boundaries.
 */
#ifndef MOLT_ERRNO_FEATURE_VISIBILITY_H
#define MOLT_ERRNO_FEATURE_VISIBILITY_H
#ifndef _WIN32
/* These three are unconditional pyconfig definitions in the pinned releases. */
#undef _NETBSD_SOURCE
#define _NETBSD_SOURCE 1
#undef __BSD_VISIBLE
#define __BSD_VISIBLE 1
#undef _DARWIN_C_SOURCE
#define _DARWIN_C_SOURCE 1
/* Autoconf's generated pyconfig template preserves explicit definitions,
 * including _GNU_SOURCE=0: glibc tests definedness, not truthiness. On Linux
 * GNU, features.h derives POSIX/XOPEN from this macro, so CPython's 3.15
 * requested-level change does not require a second version/profile switch. */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif
#ifndef _OPENBSD_SOURCE
#define _OPENBSD_SOURCE 1
#endif
/* CPython restricts this spelling to OpenBSD: its Linux meaning differs. */
#ifdef __OpenBSD__
#undef _BSD_SOURCE
#define _BSD_SOURCE 1
#endif
#endif
#endif
