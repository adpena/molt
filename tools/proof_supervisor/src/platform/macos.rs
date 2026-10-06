//! macOS kernel custody without Endpoint Security.
//!
//! Two kernel authorities combine to make leaf closure exact:
//!
//! - Seatbelt (`sandbox_init`) is inherited by every descendant and cannot be
//!   weakened from inside. The profile denies `process-fork` and every
//!   `process-exec*` outside the sealed fixed-image paths, and the kernel kills
//!   the attempting process with `SIGKILL` before the child or the image exists.
//!   `send-signal` honours only `SIGKILL`, and XNU `psignal_internal` releases
//!   the `OS_REASON_SANDBOX` exit reason for a ptrace-traced target before the
//!   signal reaches it, so the stop itself is the attributable event: a
//!   `SIGKILL` the supervisor did not send is recorded as a typed violation
//!   that names both the sealed denial and an external kill as its sources.
//! - ptrace (`PT_TRACE_ME`) on the root process stops it with `SIGTRAP` at the
//!   kernel exec boundary before the first user instruction of every image,
//!   where the mapped text vnode is identified and hashed. XNU kills a traced
//!   process when its tracer exits, so supervisor death tears the root down.
//!
//! Tree closure is refused on this platform. Measured on macOS 26.4 with SIP:
//! `EVFILT_PROC` rejects `NOTE_TRACK` with `ENOTSUP`, `NOTE_FORK` delivers no
//! child pid, ptrace does not follow fork, and `PT_ATTACHEXC` is denied for
//! platform binaries. No unprivileged kernel primitive observes every
//! descendant before entry, so only an Endpoint Security client can honestly
//! advertise recursive descendant authority.

use crate::{
    CAPABILITY_SCHEMA, Capability, ClosureMode, EventJournal, FileIdentity, ImageCacheKey,
    ImageHashCache, KernelAccounting, ProcessEventKind, Receipt, ValidatedPolicy,
};
use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::fs::File;
use std::mem::{size_of, zeroed};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

pub(super) const PLATFORM: &str = "macos";
pub(super) const BACKEND: &str = "seatbelt+ptrace";
pub(super) const TREE_UNAVAILABLE_REASON: &str = "macOS without an Endpoint Security \
     entitlement cannot observe descendant process creation before entry: EVFILT_PROC \
     NOTE_TRACK is unsupported, NOTE_FORK carries no child pid, ptrace does not follow \
     fork, and PT_ATTACHEXC is denied for platform binaries; only leaf closure is \
     kernel-enforced on this host";

#[link(name = "sandbox")]
unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
}

const PROC_PIDREGIONPATHINFO: c_int = 8;
const PROC_PIDUNIQIDENTIFIERINFO: c_int = 17;
const MAX_REGION_WALK: usize = 65_536;

/// `struct proc_uniqidentifierinfo` from XNU `sys/proc_info.h` (56 bytes).
#[repr(C)]
struct ProcUniqueIdentifierInfo {
    p_uuid: [u8; 16],
    p_uniqueid: u64,
    p_puniqueid: u64,
    p_idversion: i32,
    p_reserve2: u32,
    p_reserve3: u64,
    p_reserve4: u64,
}

/// `struct proc_regioninfo` from XNU `sys/proc_info.h` (96 bytes).
#[repr(C)]
struct ProcRegionInfo {
    pri_protection: u32,
    pri_max_protection: u32,
    pri_inheritance: u32,
    pri_flags: u32,
    pri_offset: u64,
    pri_behavior: u32,
    pri_user_wired_count: u32,
    pri_user_tag: u32,
    pri_pages_resident: u32,
    pri_pages_shared_now_private: u32,
    pri_pages_swapped_out: u32,
    pri_pages_dirtied: u32,
    pri_ref_count: u32,
    pri_shadow_depth: u32,
    pri_share_mode: u32,
    pri_private_pages_resident: u32,
    pri_shared_pages_resident: u32,
    pri_obj_id: u32,
    pri_depth: u32,
    pri_address: u64,
    pri_size: u64,
}

/// `struct proc_regionwithpathinfo` from XNU `sys/proc_info.h` (1272 bytes).
#[repr(C)]
struct ProcRegionWithPathInfo {
    prp_prinfo: ProcRegionInfo,
    prp_vip: libc::vnode_info_path,
}

pub fn capability(mode: ClosureMode) -> Capability {
    let leaf = mode == ClosureMode::Leaf;
    Capability {
        schema: CAPABILITY_SCHEMA.to_owned(),
        platform: PLATFORM.to_owned(),
        mode,
        backend: BACKEND.to_owned(),
        available: leaf,
        pre_entry_exec_authority: leaf,
        pre_entry_process_create_authority: leaf,
        recursive_descendant_authority: leaf,
        required_environment: super::required_environment(),
        reason: if leaf {
            None
        } else {
            Some(TREE_UNAVAILABLE_REASON.to_owned())
        },
    }
}

/// The capability is a pure function of the closure mode: it never probes
/// ambient host state, so a replayed receipt is checked against the exact
/// contract this binary advertises.
pub fn capability_contract_is_valid(recorded: &Capability, mode: ClosureMode) -> bool {
    recorded == &capability(mode)
}

pub fn run(policy: &ValidatedPolicy, events: &mut EventJournal, capability: Capability) -> Receipt {
    super::run_backend(policy, events, capability, |policy, events| unsafe {
        supervise(policy, events)
    })
}

struct MappedVnode {
    dev: u64,
    ino: u64,
}

enum RootEvent {
    Stopped(c_int),
    Exited,
}

/// Owns the traced root until it is reaped; any early return kills it.
struct TracedRoot {
    pid: libc::pid_t,
    reaped: bool,
    /// Set once this supervisor asked the kernel to kill the root, so a later
    /// `SIGKILL` stop is attributed to this receipt and not to the policy.
    terminated: bool,
}

impl TracedRoot {
    /// Kill the root while it is held at a ptrace stop. XNU posts the SIGKILL
    /// and resumes the stopped thread, which delivers it without another stop.
    fn kill_at_stop(&mut self) -> Result<(), String> {
        self.terminated = true;
        if unsafe { libc::ptrace(libc::PT_KILL, self.pid, null_mut(), 0) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!("ptrace(PT_KILL) failed: {error}"));
            }
        }
        Ok(())
    }

    fn reap(&mut self) -> Result<c_int, String> {
        let mut status = 0;
        loop {
            let waited = unsafe { libc::waitpid(self.pid, &mut status, 0) };
            if waited == self.pid {
                if libc::WIFSTOPPED(status) {
                    // A traced root stops even for SIGKILL; release it so the
                    // pending kill is delivered.
                    self.terminated = true;
                    unsafe {
                        libc::ptrace(libc::PT_KILL, self.pid, null_mut(), 0);
                    }
                    continue;
                }
                self.reaped = true;
                return Ok(status);
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINTR) {
                return Err(format!("waitpid(root) failed: {error}"));
            }
        }
    }
}

impl Drop for TracedRoot {
    fn drop(&mut self) {
        if !self.reaped {
            self.terminated = true;
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
                libc::kill(-self.pid, libc::SIGKILL);
            }
            let _ = self.reap();
        }
    }
}

struct Kqueue(c_int);

impl Drop for Kqueue {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

unsafe fn supervise(
    policy: &ValidatedPolicy,
    events: &mut EventJournal,
) -> Result<Option<KernelAccounting>, String> {
    if policy.policy.mode != ClosureMode::Leaf {
        return Err(TREE_UNAVAILABLE_REASON.to_owned());
    }
    let argv = cstrings(&policy.policy.command, "command")?;
    let environment_strings: Vec<String> = policy
        .policy
        .environment
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let envp = cstrings(&environment_strings, "environment")?;
    let argv_ptrs: Vec<*const c_char> = argv
        .iter()
        .map(|value| value.as_ptr())
        .chain([null()])
        .collect();
    let envp_ptrs: Vec<*const c_char> = envp
        .iter()
        .map(|value| value.as_ptr())
        .chain([null()])
        .collect();
    let executable = argv[0].as_ptr();
    let cwd = CString::new(policy.policy.cwd.as_os_str().as_bytes())
        .map_err(|_| "cwd contains NUL".to_owned())?;
    let profile = seatbelt_profile(policy)?;

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(os_error("fork"));
    }
    if pid == 0 {
        // Only async-signal-safe calls plus sandbox_init, which allocates: the
        // supervisor is single-threaded, so the child heap is consistent.
        unsafe {
            libc::setpgid(0, 0);
            if libc::ptrace(libc::PT_TRACE_ME, 0, null_mut(), 0) < 0 {
                libc::_exit(124);
            }
            if libc::chdir(cwd.as_ptr()) != 0 {
                libc::_exit(123);
            }
            libc::raise(libc::SIGSTOP);
            let mut error: *mut c_char = null_mut();
            if sandbox_init(profile.as_ptr(), 0, &mut error) != 0 {
                report_child_failure(b"molt-proof-supervisor: Seatbelt profile rejected: ", error);
                libc::_exit(125);
            }
            libc::execve(executable, argv_ptrs.as_ptr(), envp_ptrs.as_ptr());
            libc::_exit(127);
        }
    }

    let mut root = TracedRoot {
        pid,
        reaped: false,
        terminated: false,
    };
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return Err(os_error("kqueue"));
    }
    let kq = Kqueue(kq);
    watch_root(&kq, pid)?;
    let root_u32 = pid as u32;
    let stable_id = stable_id(pid, 1);
    let mut hash_cache = ImageHashCache::default();
    let mut violated = false;
    let mut pre_exec_stop_seen = false;
    let mut exec_pending = false;
    let mut root_execs = 0_u64;
    events.record(
        root_u32,
        stable_id.clone(),
        ProcessEventKind::ProcessCreate {
            parent_process_id: None,
            image: None,
        },
    )?;
    loop {
        match wait_root(pid)? {
            RootEvent::Stopped(signal) => {
                let notes = drain_notes(&kq)?;
                if notes & libc::NOTE_FORK != 0 {
                    return Err(
                        "kernel reported a fork from the leaf root despite Seatbelt denial"
                            .to_owned(),
                    );
                }
                exec_pending |= notes & libc::NOTE_EXEC != 0;
                if signal == libc::SIGSTOP && !pre_exec_stop_seen {
                    pre_exec_stop_seen = true;
                    continue_root(pid, 0)?;
                    continue;
                }
                if signal == libc::SIGTRAP && exec_pending {
                    exec_pending = false;
                    let image = exec_image_identity(policy, pid, &mut hash_cache)?;
                    let outcome = events.record(
                        root_u32,
                        stable_id.clone(),
                        ProcessEventKind::Exec { image },
                    )?;
                    root_execs += 1;
                    if outcome.must_terminate_closure() {
                        violated |= outcome.has_policy_violation();
                        root.kill_at_stop()?;
                        continue;
                    }
                    continue_root(pid, 0)?;
                    continue;
                }
                if signal == libc::SIGKILL {
                    // XNU holds a traced root at a stop even for SIGKILL and
                    // delivers it on release. A kill this supervisor did not
                    // request is the sealed Seatbelt denial or an external
                    // termination; the kernel keeps no reason for a tracee.
                    if !root.terminated {
                        let outcome = events.record(
                            root_u32,
                            stable_id.clone(),
                            ProcessEventKind::KernelPolicyTermination {
                                reason: format!(
                                    "leaf closure: root process {pid} received SIGKILL under \
                                     the sealed Seatbelt profile, which kills any attempt to \
                                     create a descendant process or execute an unadmitted \
                                     image; XNU keeps no exit reason for a traced target, so \
                                     an external SIGKILL is indistinguishable here"
                                ),
                            },
                        )?;
                        violated |= outcome.has_policy_violation();
                        root.terminated = true;
                    }
                    continue_root(pid, libc::SIGKILL)?;
                    continue;
                }
                // Every other stop is a real signal. SIGSTOP is swallowed like
                // the Linux backend does; all others keep untraced semantics.
                let deliver = if signal == libc::SIGSTOP { 0 } else { signal };
                continue_root(pid, deliver)?;
            }
            RootEvent::Exited => {
                let status = root.reap()?;
                let code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status) as i64
                } else {
                    (128 + libc::WTERMSIG(status)) as i64
                };
                if root_execs == 0 && code == 125 {
                    return Err("root could not enter its Seatbelt profile; see stderr".to_owned());
                }
                events.record(
                    root_u32,
                    stable_id,
                    ProcessEventKind::ProcessExit { exit_code: code },
                )?;
                break;
            }
        }
    }
    let _ = violated;
    Ok(None)
}

fn watch_root(kq: &Kqueue, pid: libc::pid_t) -> Result<(), String> {
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_CLEAR,
        fflags: libc::NOTE_EXEC | libc::NOTE_EXIT | libc::NOTE_FORK | libc::NOTE_EXITSTATUS,
        data: 0,
        udata: null_mut(),
    };
    if unsafe { libc::kevent(kq.0, &change, 1, null_mut(), 0, null()) } < 0 {
        return Err(os_error("kevent(EV_ADD EVFILT_PROC root)"));
    }
    Ok(())
}

/// Accumulate every pending process note for the root without blocking.
fn drain_notes(kq: &Kqueue) -> Result<u32, String> {
    let mut notes = 0;
    let immediate = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    loop {
        let mut event: libc::kevent = unsafe { zeroed() };
        let count = unsafe { libc::kevent(kq.0, null(), 0, &mut event, 1, &immediate) };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(format!("kevent(EVFILT_PROC) failed: {error}"));
        }
        if count == 0 {
            return Ok(notes);
        }
        if event.flags & libc::EV_ERROR != 0 {
            return Err(format!(
                "kevent(EVFILT_PROC) reported error {}",
                std::io::Error::from_raw_os_error(event.data as i32)
            ));
        }
        notes |= event.fflags;
    }
}

fn wait_root(pid: libc::pid_t) -> Result<RootEvent, String> {
    loop {
        let mut info: libc::siginfo_t = unsafe { zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WSTOPPED | libc::WNOWAIT,
            )
        };
        if rc < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(format!("waitid(root) failed: {error}"));
        }
        match info.si_code {
            libc::CLD_EXITED | libc::CLD_KILLED | libc::CLD_DUMPED => return Ok(RootEvent::Exited),
            libc::CLD_STOPPED | libc::CLD_TRAPPED => return Ok(RootEvent::Stopped(info.si_status)),
            libc::CLD_CONTINUED => continue,
            other => return Err(format!("waitid(root) reported unexpected si_code {other}")),
        }
    }
}

fn continue_root(pid: libc::pid_t, signal: c_int) -> Result<(), String> {
    if unsafe {
        libc::ptrace(
            libc::PT_CONTINUE,
            pid,
            std::ptr::without_provenance_mut::<c_char>(1),
            signal,
        )
    } < 0
    {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("ptrace(PT_CONTINUE) failed: {error}"));
        }
    }
    Ok(())
}

/// Identify the image at a pre-entry exec stop: the kernel's path for the
/// exec'd vnode, the mapped text vnode identity, and the hashed bytes of the
/// same inode. A path that no longer names the mapped vnode fails closed.
fn exec_image_identity(
    policy: &ValidatedPolicy,
    pid: libc::pid_t,
    hash_cache: &mut ImageHashCache,
) -> Result<FileIdentity, String> {
    let path = proc_pidpath(pid)?;
    let mapped = mapped_text_vnode(pid, &path)?;
    let file = File::open(&path)
        .map_err(|error| format!("cannot open executable {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot stat executable {}: {error}", path.display()))?;
    if metadata.dev() != mapped.dev || metadata.ino() != mapped.ino {
        return Err(format!(
            "executable path {} no longer names the mapped image vnode",
            path.display()
        ));
    }
    let cache_key = macos_cache_key(&metadata);
    let file_id = cache_key.stable_file_id().to_owned();
    let mut reader = file;
    let sha256 = hash_cache
        .digest(&cache_key, &mut reader, |file| {
            file.metadata().map(|metadata| macos_cache_key(&metadata))
        })
        .map_err(|error| format!("cannot hash executable: {error}"))?;
    Ok(policy.classify_path(&path, file_id, metadata.size(), sha256))
}

fn macos_cache_key(metadata: &std::fs::Metadata) -> ImageCacheKey {
    ImageCacheKey::new(
        format!("{:x}:{:x}", metadata.dev(), metadata.ino()),
        format!(
            "{}:{}:{}:{}:{}",
            metadata.size(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        ),
    )
}

fn proc_pidpath(pid: libc::pid_t) -> Result<PathBuf, String> {
    let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length = unsafe {
        libc::proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast::<c_void>(),
            buffer.len() as u32,
        )
    };
    if length <= 0 {
        return Err(os_error("proc_pidpath"));
    }
    Ok(PathBuf::from(OsStr::from_bytes(&buffer[..length as usize])))
}

/// Walk the mapped regions of the stopped process until the region whose
/// backing vnode path equals the kernel's executable path.
fn mapped_text_vnode(pid: libc::pid_t, path: &Path) -> Result<MappedVnode, String> {
    let wanted = path.as_os_str().as_bytes();
    let mut address = 0_u64;
    for _ in 0..MAX_REGION_WALK {
        let mut info: ProcRegionWithPathInfo = unsafe { zeroed() };
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                PROC_PIDREGIONPATHINFO,
                address,
                (&mut info as *mut ProcRegionWithPathInfo).cast::<c_void>(),
                size_of::<ProcRegionWithPathInfo>() as c_int,
            )
        };
        if got <= 0 {
            break;
        }
        let raw: &[c_char] = unsafe {
            std::slice::from_raw_parts(
                info.prp_vip.vip_path.as_ptr().cast::<c_char>(),
                size_of_val(&info.prp_vip.vip_path),
            )
        };
        let region_path = CStr::from_bytes_until_nul(unsafe {
            std::slice::from_raw_parts(raw.as_ptr().cast::<u8>(), raw.len())
        })
        .map(CStr::to_bytes)
        .unwrap_or(&[]);
        if region_path == wanted {
            return Ok(MappedVnode {
                dev: u64::from(info.prp_vip.vip_vi.vi_stat.vst_dev),
                ino: info.prp_vip.vip_vi.vi_stat.vst_ino,
            });
        }
        let next = info
            .prp_prinfo
            .pri_address
            .checked_add(info.prp_prinfo.pri_size)
            .ok_or_else(|| "process region walk overflowed".to_owned())?;
        if next <= address {
            break;
        }
        address = next;
    }
    Err(format!(
        "cannot locate the mapped executable vnode for {}",
        path.display()
    ))
}

fn stable_id(pid: libc::pid_t, generation: u64) -> String {
    let mut info: ProcUniqueIdentifierInfo = unsafe { zeroed() };
    let size = size_of::<ProcUniqueIdentifierInfo>();
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDUNIQIDENTIFIERINFO,
            0,
            (&mut info as *mut ProcUniqueIdentifierInfo).cast::<c_void>(),
            size as c_int,
        )
    };
    let token = if got as usize == size {
        info.p_uniqueid.to_string()
    } else {
        generation.to_string()
    };
    format!("macos:{pid}:{token}")
}

/// Seatbelt profile sealing the leaf closure: no process creation, and no
/// exec outside the canonical fixed-image paths. Every denial kills the
/// attempting process in the kernel.
fn seatbelt_profile(policy: &ValidatedPolicy) -> Result<CString, String> {
    let mut profile = String::from(
        "(version 1)\n(allow default)\n(deny process-fork (with send-signal SIGKILL))\n(deny process-exec* (with send-signal SIGKILL))\n(allow process-exec*",
    );
    for path in policy.fixed.keys() {
        profile.push_str(" (literal \"");
        profile.push_str(&seatbelt_literal(path)?);
        profile.push_str("\")");
    }
    profile.push_str(")\n");
    CString::new(profile).map_err(|_| "Seatbelt profile contains NUL".to_owned())
}

fn seatbelt_literal(path: &Path) -> Result<String, String> {
    let text = std::str::from_utf8(path.as_os_str().as_bytes()).map_err(|_| {
        format!(
            "fixed image path is not UTF-8 and cannot enter a Seatbelt profile: {}",
            path.display()
        )
    })?;
    if text.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return Err(format!(
            "fixed image path contains control characters and cannot enter a Seatbelt profile: {}",
            path.display()
        ));
    }
    Ok(text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Child-side diagnostic before `_exit`: raw writes only, no allocation.
unsafe fn report_child_failure(prefix: &[u8], error: *const c_char) {
    unsafe {
        libc::write(2, prefix.as_ptr().cast::<c_void>(), prefix.len());
        if !error.is_null() {
            let message = CStr::from_ptr(error).to_bytes();
            libc::write(2, message.as_ptr().cast::<c_void>(), message.len());
        }
        libc::write(2, b"\n".as_ptr().cast::<c_void>(), 1);
    }
}

fn cstrings(values: &[String], label: &str) -> Result<Vec<CString>, String> {
    values
        .iter()
        .map(|value| CString::new(value.as_bytes()).map_err(|_| format!("{label} contains NUL")))
        .collect()
}

fn os_error(operation: &str) -> String {
    format!("{operation} failed: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seatbelt_literals_escape_quotes_and_backslashes_only() {
        assert_eq!(
            seatbelt_literal(Path::new(r#"/tmp/a"b\c"#)).unwrap(),
            r#"/tmp/a\"b\\c"#
        );
        let error = seatbelt_literal(Path::new("/tmp/a\nb")).unwrap_err();
        assert!(error.contains("control characters"));
        let error = seatbelt_literal(Path::new(OsStr::from_bytes(b"/tmp/\xff"))).unwrap_err();
        assert!(error.contains("not UTF-8"));
    }

    #[test]
    fn private_libproc_layouts_match_the_kernel_sizes() {
        assert_eq!(size_of::<ProcUniqueIdentifierInfo>(), 56);
        assert_eq!(size_of::<ProcRegionInfo>(), 96);
        assert_eq!(size_of::<ProcRegionWithPathInfo>(), 1272);
    }

    #[test]
    fn tree_modes_are_unavailable_and_leaf_is_complete() {
        for mode in [ClosureMode::DeclaredTree, ClosureMode::InventoryTree] {
            let capability = capability(mode);
            assert!(!capability.available);
            assert!(!capability.recursive_descendant_authority);
            assert_eq!(capability.reason.as_deref(), Some(TREE_UNAVAILABLE_REASON));
            assert!(capability_contract_is_valid(&capability, mode));
        }
        let leaf = capability(ClosureMode::Leaf);
        assert!(leaf.available && leaf.reason.is_none());
        assert!(leaf.pre_entry_exec_authority);
        assert!(leaf.pre_entry_process_create_authority);
        assert!(leaf.recursive_descendant_authority);
        assert!(!capability_contract_is_valid(
            &leaf,
            ClosureMode::DeclaredTree
        ));
    }
}
