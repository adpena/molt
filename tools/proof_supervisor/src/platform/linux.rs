use crate::{
    Admission, BackendFailure, CAPABILITY_SCHEMA, Capability, ClosureMode, EventJournal,
    FileIdentity, ImageHashCache, KernelAccounting, ProcessEventKind, Receipt,
    TerminalObservations, ValidatedPolicy, push_bounded_diagnostic,
};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::null_mut;

#[cfg(test)]
pub(super) static TEST_WAIT_CUSTODY: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn capability(mode: ClosureMode) -> Capability {
    let admission = match creation_filter() {
        Ok(_) => ptrace_plan(),
        Err(reason) => Admission::Ineligible { reason },
    };
    Capability {
        schema: CAPABILITY_SCHEMA.to_owned(),
        platform: "linux".to_owned(),
        mode,
        backend: "ptrace-exitkill".to_owned(),
        admission,
        pre_entry_exec_authority: true,
        pre_entry_process_create_authority: true,
        recursive_descendant_authority: true,
        required_environment: super::required_environment(),
    }
}

pub fn run(policy: &ValidatedPolicy, events: &mut EventJournal, capability: Capability) -> Receipt {
    super::run_backend(policy, events, capability, |policy, events| unsafe {
        supervise(policy, events)
    })
}

fn ptrace_plan() -> Admission {
    let scope = match std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope") {
        Ok(value) => match value.trim().parse::<u8>() {
            Ok(scope) => Some(scope),
            Err(error) => {
                return Admission::Ineligible {
                    reason: format!("cannot parse Yama ptrace_scope: {error}"),
                };
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Admission::Ineligible {
                reason: format!("cannot read Yama ptrace_scope: {error}"),
            };
        }
    };
    match scope {
        None | Some(0 | 1) => Admission::Eligible {},
        Some(2) if effective_cap_sys_ptrace() => Admission::Eligible {},
        Some(2) => Admission::Ineligible {
            reason: "Yama ptrace_scope=2 requires effective CAP_SYS_PTRACE".to_owned(),
        },
        Some(3) => Admission::Ineligible {
            reason: "Yama ptrace_scope=3 forbids ptrace".to_owned(),
        },
        Some(value) => Admission::Ineligible {
            reason: format!("unsupported Yama ptrace_scope value {value}"),
        },
    }
}

fn effective_cap_sys_ptrace() -> bool {
    const CAP_SYS_PTRACE: u32 = 19;
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return false;
    };
    let Some(encoded) = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:").map(str::trim))
    else {
        return false;
    };
    u64::from_str_radix(encoded, 16).is_ok_and(|mask| mask & (1_u64 << CAP_SYS_PTRACE) != 0)
}

unsafe fn supervise(
    policy: &ValidatedPolicy,
    events: &mut EventJournal,
) -> Result<crate::NativeCustody, BackendFailure> {
    // The supervisor ABI and every compatible entry ABI must have one audited
    // interpretation before any child exists. This is a syscall restriction,
    // not transparent clone3 or privilege-gaining exec compatibility.
    let filter = creation_filter()?;
    let argv = cstrings(&policy.policy.command, "command")?;
    let environment_strings: Vec<String> = policy
        .policy
        .environment
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let envp = cstrings(&environment_strings, "environment")?;
    let argv_ptrs: Vec<*const libc::c_char> = argv
        .iter()
        .map(|value| value.as_ptr())
        .chain([std::ptr::null()])
        .collect();
    let envp_ptrs: Vec<*const libc::c_char> = envp
        .iter()
        .map(|value| value.as_ptr())
        .chain([std::ptr::null()])
        .collect();
    let executable = argv[0].as_ptr();
    let cwd = CString::new(policy.policy.cwd.as_os_str().as_bytes())
        .map_err(|_| "cwd contains NUL".to_owned())?;
    let (setup_read, setup_write) = setup_pipe()?;
    let setup_result = SetupResult::ready();
    let program = libc::sock_fprog {
        len: filter.len() as libc::c_ushort,
        filter: filter.as_ptr().cast_mut(),
    };
    let parent = unsafe { libc::getpid() };
    let mut trace = TraceTree::prepare()?;

    // clone3 installs the parent's CLOEXEC pidfd before waking the child. All
    // failure exits precede wakeup. Version 0 is exactly eight aligned u64s;
    // no optional extension, shared VM/files, or post-failure launch fallback.
    let mut root_pidfd: libc::c_int = -1;
    let args = CloneArgs {
        flags: libc::CLONE_PIDFD as u64,
        pidfd: (&mut root_pidfd as *mut libc::c_int) as usize as u64,
        exit_signal: libc::SIGCHLD as u64,
        ..CloneArgs::default()
    };
    let root = unsafe {
        libc::syscall(
            libc::SYS_clone3,
            &args as *const CloneArgs,
            std::mem::size_of::<CloneArgs>(),
        )
    };
    if root < 0 {
        return Err(
            os_error("clone3(CLONE_PIDFD): Linux 5.3+ and syscall permission required").into(),
        );
    }
    if root == 0 {
        unsafe {
            // Everything read by this trusted preamble was prepared before
            // clone. No allocator, destructor or general Rust I/O runs here.
            // This closes parent-death races only once the child runs these
            // instructions. It is not atomic pre-admission death containment.
            if libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::SIGKILL as usize,
                0_usize,
                0_usize,
                0_usize,
            ) != 0
            {
                setup_failed(
                    setup_write.as_raw_fd(),
                    setup_result,
                    SETUP_PARENT_DEATH,
                    124,
                );
            }
            if libc::getppid() != parent {
                *libc::__errno_location() = libc::ESRCH;
                setup_failed(
                    setup_write.as_raw_fd(),
                    setup_result,
                    SETUP_PARENT_DEATH,
                    124,
                );
            }
            libc::close(setup_read.as_raw_fd());
            if libc::setpgid(0, 0) != 0 {
                setup_failed(setup_write.as_raw_fd(), setup_result, SETUP_GROUP, 124);
            }
            if libc::ptrace(
                libc::PTRACE_TRACEME,
                0,
                null_mut::<libc::c_void>(),
                null_mut::<libc::c_void>(),
            ) < 0
            {
                setup_failed(setup_write.as_raw_fd(), setup_result, SETUP_TRACE, 124);
            }
            if libc::chdir(cwd.as_ptr()) != 0 {
                setup_failed(setup_write.as_raw_fd(), setup_result, SETUP_CWD, 123);
            }
            if libc::prctl(
                libc::PR_SET_NO_NEW_PRIVS,
                1_usize,
                0_usize,
                0_usize,
                0_usize,
            ) != 0
            {
                setup_failed(
                    setup_write.as_raw_fd(),
                    setup_result,
                    SETUP_NO_NEW_PRIVS,
                    124,
                );
            }
            if libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER as usize,
                &program as *const libc::sock_fprog,
                0_usize,
                0_usize,
            ) != 0
            {
                setup_failed(setup_write.as_raw_fd(), setup_result, SETUP_FILTER, 124);
            }
            publish_setup(setup_write.as_raw_fd(), &setup_result);
            // No subject image can retain the sole writer or forge another
            // result. An earlier externally injected stop has no complete ACK.
            libc::close(setup_write.as_raw_fd());
            if libc::raise(libc::SIGSTOP) != 0 {
                libc::_exit(124);
            }
            libc::execve(executable, argv_ptrs.as_ptr(), envp_ptrs.as_ptr());
            libc::_exit(127);
        }
    }

    let root = root as libc::pid_t;
    // A successful parent return guarantees this actual descriptor. The child
    // did not inherit it: CLONE_FILES is absent and fd installation follows
    // copying the child's file table in copy_process().
    let handle = unsafe { OwnedFd::from_raw_fd(root_pidfd) };
    trace.adopt_root(root, handle);
    drop(setup_write);
    let result = supervise_trace(policy, events, &mut trace, root, setup_read);
    trace.finish(result, events)
}

/// Fixed inherited creation policy. The audit tag is checked before syscall
/// numbers. The low word of args[0] is the scalar clone flags on these LE ABIs;
/// clone3's mutable user pointer is never inspected or continued by userspace.
/// Fork/vfork are fixed-flag calls and remain covered by ptrace creation events.
fn creation_filter() -> Result<[libc::sock_filter; 19], String> {
    let (native_arch, compat_arch, native_clone, number_mask) = if cfg!(all(
        target_arch = "x86_64",
        target_pointer_width = "64",
        target_endian = "little"
    )) {
        // AUDIT_ARCH_X86_64 / AUDIT_ARCH_I386, including x32's explicit bit.
        (0xc000_003e, 0x4000_0003, 56, !0x4000_0000_u32)
    } else if cfg!(all(
        target_arch = "aarch64",
        target_pointer_width = "64",
        target_endian = "little"
    )) {
        // AUDIT_ARCH_AARCH64 / AUDIT_ARCH_ARM (AArch32 compatibility).
        (0xc000_00b7, 0x4000_0028, 220, u32::MAX)
    } else {
        return Err("Linux creation admission requires an audited little-endian x86-64 or AArch64 supervisor ABI".to_owned());
    };
    let load = libc::BPF_LD | libc::BPF_W | libc::BPF_ABS;
    let equal = libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K;
    let ret = libc::BPF_RET | libc::BPF_K;
    Ok([
        bpf(load, 0, 0, 4),                             // 0: seccomp_data.arch
        bpf(equal, 2, 0, native_arch),                  // 1: native -> 4
        bpf(equal, 7, 0, compat_arch),                  // 2: compat -> 10
        bpf(ret, 0, 0, libc::SECCOMP_RET_KILL_PROCESS), // 3: unknown ABI
        bpf(load, 0, 0, 0),                             // 4: syscall number
        bpf(
            libc::BPF_ALU | libc::BPF_AND | libc::BPF_K,
            0,
            0,
            number_mask,
        ),
        bpf(equal, 0, 1, 435), // 6: clone3
        bpf(ret, 0, 0, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
        bpf(equal, 6, 0, native_clone),          // 8: clone -> 15
        bpf(ret, 0, 0, libc::SECCOMP_RET_ALLOW), // 9
        bpf(load, 0, 0, 0),                      // 10: compat number
        bpf(equal, 0, 1, 435),                   // 11: compat clone3
        bpf(ret, 0, 0, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
        bpf(equal, 1, 0, 120),                   // 13: compat clone
        bpf(ret, 0, 0, libc::SECCOMP_RET_ALLOW), // 14
        bpf(load, 0, 0, 16),                     // 15: args[0], low32
        bpf(
            libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K,
            0,
            1,
            libc::CLONE_UNTRACED as u32,
        ),
        bpf(ret, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        bpf(ret, 0, 0, libc::SECCOMP_RET_ALLOW),
    ])
}

const fn bpf(code: u32, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

// This protocol exists only between the launch parent and its trusted clone
// preamble. It is not a public receipt or a payload-controlled admission claim.
const SETUP_GROUP: u32 = 1;
const SETUP_TRACE: u32 = 2;
const SETUP_CWD: u32 = 3;
const SETUP_NO_NEW_PRIVS: u32 = 4;
const SETUP_FILTER: u32 = 5;
const SETUP_PARENT_DEATH: u32 = 6;

#[repr(C)]
#[derive(Clone, Copy)]
struct SetupResult {
    magic: u32,
    policy_version: u32,
    stage: u32,
    error: i32,
}

impl SetupResult {
    const fn ready() -> Self {
        Self {
            magic: 0x4d43_5031,
            policy_version: 1,
            stage: 0,
            error: 0,
        }
    }
}

const _: () = assert!(std::mem::size_of::<SetupResult>() == 16);

fn setup_pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut descriptors = [-1; 2];
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(os_error("pipe2 for trusted creation admission"));
    }
    Ok(unsafe {
        (
            OwnedFd::from_raw_fd(descriptors[0]),
            OwnedFd::from_raw_fd(descriptors[1]),
        )
    })
}

unsafe fn publish_setup(fd: libc::c_int, result: &SetupResult) {
    loop {
        let written = unsafe {
            libc::write(
                fd,
                (result as *const SetupResult).cast(),
                std::mem::size_of::<SetupResult>(),
            )
        };
        if written < 0 && unsafe { *libc::__errno_location() } == libc::EINTR {
            continue;
        }
        // A single <= PIPE_BUF frame is atomic. Any failure/short write is
        // rejected by the parent; never try to complete a partial frame.
        return;
    }
}

unsafe fn setup_failed(fd: libc::c_int, mut result: SetupResult, stage: u32, exit: i32) -> ! {
    result.error = unsafe { *libc::__errno_location() };
    result.stage = stage;
    unsafe {
        publish_setup(fd, &result);
        libc::close(fd);
        libc::_exit(exit);
    }
}

fn verify_setup(fd: OwnedFd) -> Result<(), String> {
    let mut bytes = [0_u8; std::mem::size_of::<SetupResult>() + 1];
    let mut used = 0;
    loop {
        if used == bytes.len() {
            return Err("trusted creation setup result has trailing bytes".to_owned());
        }
        let count = unsafe {
            libc::read(
                fd.as_raw_fd(),
                bytes[used..].as_mut_ptr().cast(),
                bytes.len() - used,
            )
        };
        if count == 0 {
            break;
        }
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(format!(
                "trusted creation setup acknowledgment unavailable before first release: {error}"
            ));
        }
        used += count as usize;
    }
    if used != std::mem::size_of::<SetupResult>() {
        return Err(format!(
            "trusted creation setup result is missing or partial: {used} bytes"
        ));
    }
    let result = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<SetupResult>()) };
    let expected = SetupResult::ready();
    if result.magic != expected.magic || result.policy_version != expected.policy_version {
        return Err("trusted creation setup result has an unknown policy identity".to_owned());
    }
    match (result.stage, result.error) {
        (0, 0) => Ok(()),
        (stage, error) if (SETUP_GROUP..=SETUP_PARENT_DEATH).contains(&stage) && error > 0 => {
            let operation = match stage {
                SETUP_GROUP => "setpgid",
                SETUP_TRACE => "PTRACE_TRACEME",
                SETUP_CWD => "chdir",
                SETUP_NO_NEW_PRIVS => "PR_SET_NO_NEW_PRIVS",
                SETUP_FILTER => "inherited seccomp creation filter",
                SETUP_PARENT_DEATH => "PR_SET_PDEATHSIG and original-parent check",
                _ => unreachable!(),
            };
            Err(format!(
                "trusted creation setup failed at {operation}: errno {error} ({})",
                std::io::Error::from_raw_os_error(error)
            ))
        }
        _ => Err("trusted creation setup result has an invalid stage/errno pair".to_owned()),
    }
}

/// Linux UAPI clone_args version 0 (include/uapi/linux/sched.h).
#[repr(C, align(8))]
#[derive(Default)]
struct CloneArgs {
    flags: u64,
    pidfd: u64,
    child_tid: u64,
    parent_tid: u64,
    exit_signal: u64,
    stack: u64,
    stack_size: u64,
    tls: u64,
}

const _: () = assert!(std::mem::size_of::<CloneArgs>() == 64);

/// One process capability per kernel process. Thread IDs are only wait/ptrace
/// bookkeeping: nonleader exec can retire a TID without a terminal wait.
struct TracedProcess {
    handle: OwnedFd,
    stable_id: String,
    image: Option<FileIdentity>,
    recorded: bool,
}

impl TracedProcess {
    /// Only called at a retained descendant creation event,
    /// before either this tracer or the real parent can reap the new process.
    fn acquire(pid: libc::pid_t, generation: u64) -> Result<Self, String> {
        use std::fmt::Write;
        let mut identity = String::new();
        identity
            .try_reserve_exact(crate::BUDGET_STABLE_PROCESS_ID_UTF8_BYTES)
            .map_err(|e| format!("process identity reservation: {e}"))?;
        write!(&mut identity, "linux:{pid}:{generation}").expect("reserved process identity");
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0_u32) };
        if fd < 0 {
            return Err(os_error(
                "pidfd_open at process admission (Linux 5.3+ required)",
            ));
        }
        Ok(Self {
            handle: unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) },
            stable_id: identity,
            image: None,
            recorded: false,
        })
    }

    fn signal(&self, signal: libc::c_int) -> Result<(), String> {
        loop {
            if unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    self.handle.as_raw_fd(),
                    signal,
                    std::ptr::null::<libc::siginfo_t>(),
                    0_u32,
                )
            } == 0
            {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                // This is not a terminal receipt. The wait owner must still
                // consume the kernel's terminal event before closure.
                Some(libc::ESRCH) if signal != 0 => return Ok(()),
                _ => return Err(format!("pidfd_send_signal({signal}) failed: {error}")),
            }
        }
    }
}

#[derive(Default)]
struct TraceTask {
    process: Option<libc::pid_t>,
    stop: Option<libc::c_int>,
    terminal: Option<libc::c_int>,
    resumed: bool,
}

/// The standalone supervisor is the sole tracer/waiter. This replaces the old
/// tracee set and process set; only process records carry signal authority.
struct TraceTree {
    root: libc::pid_t,
    root_exit_code: Option<i64>,
    root_identity: String,
    pending_task: Option<(libc::pid_t, Option<libc::c_int>)>,
    tasks: HashMap<libc::pid_t, TraceTask>,
    processes: HashMap<libc::pid_t, TracedProcess>,
    terminating: bool,
    finalized: bool,
    wait_lost: bool,
}

impl TraceTree {
    // All storage for root custody is reserved before clone3 creates a child.
    fn prepare() -> Result<Self, String> {
        let mut tasks = HashMap::new();
        let mut processes = HashMap::new();
        tasks
            .try_reserve(1)
            .map_err(|e| format!("root task reservation: {e}"))?;
        processes
            .try_reserve(1)
            .map_err(|e| format!("root process reservation: {e}"))?;
        let mut root_identity = String::new();
        root_identity
            .try_reserve_exact(crate::BUDGET_STABLE_PROCESS_ID_UTF8_BYTES)
            .map_err(|e| format!("root identity reservation: {e}"))?;
        Ok(Self {
            root: 0,
            root_exit_code: None,
            root_identity,
            pending_task: None,
            tasks,
            processes,
            terminating: false,
            finalized: true,
            wait_lost: false,
        })
    }

    fn adopt_root(&mut self, root: libc::pid_t, handle: OwnedFd) {
        use std::fmt::Write;
        self.root = root;
        write!(&mut self.root_identity, "linux:{root}:1").expect("reserved root identity");
        self.tasks.insert(
            root,
            TraceTask {
                process: Some(root),
                ..TraceTask::default()
            },
        );
        self.processes.insert(
            root,
            TracedProcess {
                handle,
                stable_id: std::mem::take(&mut self.root_identity),
                image: None,
                recorded: false,
            },
        );
        self.finalized = false;
    }

    #[cfg(test)]
    fn new(root: libc::pid_t, handle: OwnedFd) -> Self {
        let mut trace = Self::prepare().expect("test root storage");
        trace.adopt_root(root, handle);
        trace
    }

    fn retain_task(&mut self, pid: libc::pid_t, status: Option<libc::c_int>) -> Result<(), String> {
        if self.tasks.contains_key(&pid) {
            return Ok(());
        }
        // One failed transition frame retains the genuine kernel observation
        // outside a failed index reservation. It is never an alternate registry.
        assert!(
            self.pending_task.is_none(),
            "pending task must be drained before another wait"
        );
        self.pending_task = Some((pid, status));
        if self.tasks.len() >= crate::BUDGET_LIVE_TRACE_TASKS {
            return Err("live trace-task budget exhausted".to_owned());
        }
        self.tasks
            .try_reserve(1)
            .map_err(|e| format!("trace-task reservation: {e}"))?;
        self.tasks.insert(pid, TraceTask::default());
        self.pending_task = None;
        Ok(())
    }

    /// Reap only the single child whose real creation/initial stop was retained
    /// when indexing failed. No numeric kill or unverified pidfd acquisition.
    fn drain_pending(&mut self) -> Result<(), String> {
        let Some((pid, mut status)) = self.pending_task else {
            return Ok(());
        };
        loop {
            let observed = match status.take() {
                Some(status) => status,
                None => loop {
                    let mut next = 0;
                    let result = unsafe { libc::waitpid(pid, &mut next, libc::__WALL) };
                    if result == pid {
                        break next;
                    }
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    return Err(format!("pending admission wait for {pid}: {error}"));
                },
            };
            self.pending_task = Some((pid, Some(observed)));
            if libc::WIFEXITED(observed) || libc::WIFSIGNALED(observed) {
                self.pending_task = None;
                return Ok(());
            }
            if is_initial_stop(observed) {
                if unsafe {
                    libc::ptrace(
                        libc::PTRACE_KILL,
                        pid,
                        null_mut::<libc::c_void>(),
                        null_mut::<libc::c_void>(),
                    )
                } < 0
                {
                    return Err(os_error("pending admission PTRACE_KILL"));
                }
            } else if is_exit_stop(observed) {
                ptrace_continue(pid, 0)?;
            } else {
                return Err(format!(
                    "pending child {pid} has no initial/exit stop authority"
                ));
            }
            self.pending_task = Some((pid, None));
        }
    }

    fn wait(
        &mut self,
        wanted: libc::pid_t,
        nonblocking: bool,
    ) -> Result<Option<(libc::pid_t, libc::c_int)>, String> {
        loop {
            let mut status = 0;
            // The first root stop can arrive before TRACEME if an outside
            // actor stops the trusted preamble. Observe it and reject the
            // missing setup ACK instead of blocking on an untraced child.
            let flags = libc::__WALL
                | if wanted > 0 { libc::WUNTRACED } else { 0 }
                | if nonblocking { libc::WNOHANG } else { 0 };
            let pid = unsafe { libc::waitpid(wanted, &mut status, flags) };
            if pid > 0 {
                self.retain_task(pid, Some(status))?;
                let task = self.tasks.get_mut(&pid).expect("wait task retained");
                if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                    if pid == self.root {
                        self.root_exit_code = Some(if libc::WIFEXITED(status) {
                            libc::WEXITSTATUS(status) as i64
                        } else {
                            (128 + libc::WTERMSIG(status)) as i64
                        });
                    }
                    task.stop = None;
                    task.terminal = Some(status);
                } else if libc::WIFSTOPPED(status) {
                    task.stop = Some(status);
                    if let Some(expected) = task.process {
                        match thread_group_id(pid) {
                            Some(actual) if actual == expected => {}
                            Some(_) => {
                                // A nonleader exec can retire its former TID
                                // before we receive the EXEC event. A new child
                                // reusing that number must await its own create.
                                task.process = None;
                                task.resumed = false;
                            }
                            None => {
                                return Err(format!(
                                    "cannot validate process owner of stopped trace task {pid}"
                                ));
                            }
                        }
                    }
                } else {
                    task.stop = None;
                    return Err(format!(
                        "waitpid({wanted}) returned unknown status {status:#x} for {pid}"
                    ));
                }
                return Ok(Some((pid, status)));
            }
            if pid == 0 {
                return Ok(None);
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::ECHILD) => {
                    self.wait_lost = true;
                    return Ok(None);
                }
                _ => return Err(format!("waitpid({wanted}, __WALL) failed: {error}")),
            }
        }
    }

    fn admit_process(&mut self, pid: libc::pid_t, generation: u64) -> Result<(), String> {
        self.retain_task(pid, None)?;
        let task = self.tasks.get_mut(&pid).expect("admission task retained");
        if let Some(status) = task.terminal {
            return Err(format!(
                "process {pid} was reaped before admission, status {status:#x}; refusing pidfd acquisition"
            ));
        }
        if task.process.is_some() || self.processes.contains_key(&pid) {
            return Err(format!("duplicate process admission for {pid}"));
        }
        if self.processes.len() >= crate::BUDGET_LIVE_PROCESSES {
            return Err("live process budget exhausted".to_owned());
        }
        self.processes
            .try_reserve(1)
            .map_err(|e| format!("process-owner reservation: {e}"))?;
        let process = TracedProcess::acquire(pid, generation)?;
        self.processes.insert(pid, process);
        task.process = Some(pid);
        // Keep the capability on its owner even when validation fails.
        self.processes
            .get(&pid)
            .expect("process inserted")
            .signal(0)
    }

    fn resume(&mut self, pid: libc::pid_t, signal: libc::c_int) -> Result<(), String> {
        let task = self
            .tasks
            .get_mut(&pid)
            .ok_or_else(|| format!("unowned trace task {pid}"))?;
        if task.terminal.is_some() || task.stop.is_none() || task.process.is_none() {
            return Err(format!(
                "cannot resume unadmitted or non-stopped trace task {pid}"
            ));
        }
        ptrace_continue(pid, signal)?;
        task.stop = None;
        task.resumed = true;
        Ok(())
    }

    fn terminate(&mut self) -> Result<(), String> {
        self.terminating = true;
        let mut errors = Vec::new();
        let mut error_count = 0_u64;
        for (pid, process) in &self.processes {
            // One task's EXIT stop is not process closure. A leader may be
            // exiting while siblings are still live. Terminate the retained
            // process capability, then retire each task's stop independently.
            if let Err(error) = process.signal(libc::SIGKILL) {
                error_count += 1;
                push_bounded_diagnostic(&mut errors, format!("process {pid}: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{error_count} process termination errors; {}",
                errors.join("; ")
            ))
        }
    }

    /// Admission rollback is restricted to the real initial SIGSTOP. Unlike a
    /// numeric kill, ptrace checks our tracer relationship in the kernel. No
    /// payload has run in this child. PTRACE_KILL is deliberately not used on
    /// arbitrary/running tracees: older kernels only inject at delivery stops.
    fn reject_initial_stop(&mut self, pid: libc::pid_t) -> Result<(), String> {
        let task = self
            .tasks
            .get_mut(&pid)
            .ok_or_else(|| format!("unowned admission stop {pid}"))?;
        let status = task
            .stop
            .ok_or_else(|| format!("process {pid} has no admission stop"))?;
        if task.terminal.is_some() || task.resumed || !is_initial_stop(status) {
            return Err(format!(
                "process {pid} has no verified initial signal-delivery stop"
            ));
        }
        if unsafe {
            libc::ptrace(
                libc::PTRACE_KILL,
                pid,
                null_mut::<libc::c_void>(),
                null_mut::<libc::c_void>(),
            )
        } < 0
        {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!("ptrace(PTRACE_KILL admission) failed: {error}"));
            }
        }
        task.stop = None;
        Ok(())
    }

    fn terminal(
        &mut self,
        pid: libc::pid_t,
        status: libc::c_int,
        events: &mut EventJournal,
    ) -> Result<bool, String> {
        let task = self
            .tasks
            .get(&pid)
            .ok_or_else(|| format!("terminal task {pid} has no wait record"))?;
        if task.process.is_none() {
            // Preserve this tombstone until its parent's creation event is
            // reconciled. Reopening the reported PID would reacquire a number
            // which this terminal wait has already released.
            return Err(format!(
                "unadmitted trace task {pid} exited with terminal status {status:#x}"
            ));
        }
        self.tasks.remove(&pid);
        let Some(process) = self.processes.remove(&pid) else {
            return Ok(false);
        };
        let code = exit_code(status);
        if !process.recorded {
            return Err(format!("unpublished process {pid} exited with code {code}"));
        }
        let outcome = events.record(
            pid as u32,
            process.stable_id,
            ProcessEventKind::ProcessExit { exit_code: code },
        ).map_err(|error| format!("process {pid} terminal status {status:#x} (exit code {code}) publication failed: {error}"))?;
        Ok(outcome.must_terminate_closure())
    }

    fn drain(&mut self, events: Option<&mut EventJournal>) -> TraceCleanup {
        let mut events = events;
        let mut cleanup = TraceCleanup::default();
        let mut terminal_statuses = TerminalObservations::default();
        let mut blocked = false;
        if let Err(error) = self.terminate() {
            cleanup.error(error);
            blocked = true;
        }
        // Retire every currently stopped task before waiting. An early error
        // can occur while a process is held at an exec/fork/exit stop.
        if let Err(error) = self.drain_pending() {
            cleanup.error(error);
            blocked = true;
        }
        while !self.wait_lost {
            let Some(pid) = self
                .tasks
                .iter()
                .find_map(|(pid, task)| task.stop.map(|_| *pid))
            else {
                break;
            };
            if let Err(error) = self.retire_stop(pid) {
                cleanup.error(error);
                blocked = true;
                break;
            }
        }
        while !self.wait_lost && self.pending_task.is_none() {
            // After an actuation failure, drain ready events without claiming
            // that a blocked tracee will ever exit. Retain an explicit error.
            let waited = match self.wait(-1, blocked) {
                Ok(Some(waited)) => waited,
                Ok(None) => break,
                Err(error) => {
                    cleanup.error(error);
                    if let Err(error) = self.drain_pending() {
                        cleanup.error(error);
                    }
                    break;
                }
            };
            let (pid, status) = waited;
            if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                terminal_statuses.observe(pid as u32, status as i64);
                self.tasks.remove(&pid);
                if let Some(process) = self.processes.remove(&pid) {
                    let code = exit_code(status);
                    if process.recorded {
                        if let Some(events) = events.as_deref_mut() {
                            if let Err(error) = events.record(
                                pid as u32,
                                process.stable_id,
                                ProcessEventKind::ProcessExit { exit_code: code },
                            ) {
                                cleanup.error(format!("process {pid} exited with code {code}; terminal publication failed: {error}"));
                            }
                        }
                    } else {
                        cleanup.error(format!(
                            "unpublished process {pid} reaped with exit code {code}"
                        ));
                    }
                }
            } else if let Err(error) = self.retire_stop(pid) {
                cleanup.error(error);
                blocked = true;
            }
        }
        if !self.wait_lost || !self.processes.is_empty() {
            cleanup.error(format!("trace cleanup unresolved: {} process capabilities remain; kernel wait exhaustion={}", self.processes.len(), self.wait_lost));
        }
        // A lost wait does not authorize a numerical cleanup attempt. Retained
        // pidfds cannot retarget a replacement even if terminal publication failed.
        cleanup.summary = format!(
            "{}; cleanup errors={}; unresolved process capabilities={}; wait exhaustion={}",
            terminal_statuses.summary("waits"),
            cleanup.error_count,
            self.processes.len(),
            self.wait_lost
        );
        cleanup
    }

    fn retire_stop(&mut self, pid: libc::pid_t) -> Result<(), String> {
        let task = self
            .tasks
            .get(&pid)
            .ok_or_else(|| format!("unowned cleanup stop {pid}"))?;
        if task.stop.is_some_and(is_exit_stop) {
            // This task is already on the kernel exit path. Its stop still
            // needs release after process-level termination; only the later
            // actual terminal wait is authoritative for final exit status.
            ptrace_continue(pid, 0)?;
            self.tasks.get_mut(&pid).expect("task checked").stop = None;
            return Ok(());
        }
        let initial = !task.resumed && task.stop.is_some_and(is_initial_stop);
        let process = self.tasks.get(&pid).and_then(|task| task.process);
        if let Some(process) = process {
            let owner = self
                .processes
                .get(&process)
                .ok_or_else(|| format!("trace task {pid} lost process capability {process}"))?;
            owner.signal(libc::SIGKILL)?;
            if initial {
                // SIGKILL wakes even a pre-TRACEME job-control stop. The
                // retained process handle is sufficient; no ptrace relation
                // is inferred from an arbitrary first SIGSTOP.
                self.tasks.get_mut(&pid).expect("task checked").stop = None;
                Ok(())
            } else {
                self.resume(pid, 0)
            }
        } else {
            self.reject_initial_stop(pid)
        }
    }

    fn custody(&self) -> crate::NativeCustody {
        crate::NativeCustody::Linux {
            remaining_tasks: self.tasks.len() as u64 + u64::from(self.pending_task.is_some()),
            remaining_processes: self.processes.len() as u64,
            wait_exhausted: self.wait_lost,
            root_exit_code: self.root_exit_code,
        }
    }

    fn finish(
        &mut self,
        result: Result<Option<KernelAccounting>, String>,
        events: &mut EventJournal,
    ) -> Result<crate::NativeCustody, BackendFailure> {
        let result = match result {
            Ok(_) => Ok(self.custody()),
            Err(error) => {
                // A setup/admission failure can precede the first recorded
                // create; accepted events cannot represent that native state.
                events.cutoff(crate::CaptureStage::NativeObservation, &error);
                let cleanup = self.drain(Some(events));
                let mut failure = BackendFailure::from(error);
                failure.native_custody = self.custody();
                failure.retain_cleanup(cleanup.summary);
                for error in cleanup.errors {
                    failure.retain_cleanup(error);
                }
                Err(failure)
            }
        };
        self.finalized = true;
        result
    }
}

#[derive(Default)]
struct TraceCleanup {
    summary: String,
    error_count: u64,
    errors: Vec<String>,
}

impl TraceCleanup {
    fn error(&mut self, error: String) {
        self.error_count += 1;
        push_bounded_diagnostic(&mut self.errors, error);
    }
}

impl Drop for TraceTree {
    fn drop(&mut self) {
        if !self.finalized {
            let cleanup = self.drain(None);
            eprintln!(
                "molt-proof-supervisor: trace cleanup during unwind: {}; {}",
                cleanup.summary,
                cleanup.errors.join("; ")
            );
        }
    }
}

fn supervise_trace(
    policy: &ValidatedPolicy,
    events: &mut EventJournal,
    trace: &mut TraceTree,
    root: libc::pid_t,
    setup_read: OwnedFd,
) -> Result<Option<KernelAccounting>, String> {
    let Some((pid, initial)) = trace.wait(root, false)? else {
        return Err("root wait custody was lost before its pre-exec stop".to_owned());
    };
    let setup = verify_setup(setup_read);
    if pid != root
        || !libc::WIFSTOPPED(initial)
        || libc::WSTOPSIG(initial) != libc::SIGSTOP
        || (initial as u32 >> 16) != 0
    {
        // The wait may already have reaped the root. No PID or group signal is
        // permitted after this point; cleanup consults the actual wait state.
        if libc::WIFEXITED(initial) || libc::WIFSIGNALED(initial) {
            trace.tasks.remove(&root);
            trace.processes.remove(&root);
        }
        return Err(format!(
            "traced root did not enter its pre-exec stop: status {initial:#x}; setup: {}",
            setup
                .err()
                .unwrap_or_else(|| "installed creation restriction acknowledged".to_owned())
        ));
    }
    setup?;
    let options = libc::PTRACE_O_EXITKILL
        | libc::PTRACE_O_TRACEFORK
        | libc::PTRACE_O_TRACEVFORK
        | libc::PTRACE_O_TRACECLONE
        | libc::PTRACE_O_TRACEEXEC
        | libc::PTRACE_O_TRACEEXIT;
    if unsafe {
        libc::ptrace(
            libc::PTRACE_SETOPTIONS,
            root,
            null_mut::<libc::c_void>(),
            options as usize as *mut libc::c_void,
        )
    } < 0
    {
        return Err(os_error("ptrace(PTRACE_SETOPTIONS)"));
    }
    let mut generation = 1_u64;
    let process = trace.processes.get_mut(&root).expect("root admitted");
    process.signal(0)?;
    // This is the actual admitted root boundary: retained process capability,
    // authenticated installed restriction and ptrace options all precede it.
    events.record(
        root as u32,
        process.stable_id.clone(),
        ProcessEventKind::ProcessCreate {
            parent_process_id: None,
        },
    )?;
    process.recorded = true;
    trace.resume(root, 0)?;
    let mut hash_cache = ImageHashCache::default();

    // Exhaust the kernel wait domain, not merely the last userspace snapshot.
    // A child creation/stop can still be pending when the root exits.
    while let Some((pid, status)) = trace.wait(-1, false)? {
        handle_trace_event(
            policy,
            events,
            trace,
            pid,
            status,
            &mut generation,
            &mut hash_cache,
        )?;
    }
    if !trace.tasks.is_empty() || !trace.processes.is_empty() {
        return Err(format!(
            "kernel wait custody exhausted with {} unreconciled trace tasks and {} process capabilities",
            trace.tasks.len(),
            trace.processes.len()
        ));
    }
    Ok(None)
}

/// Apply one already-owned kernel observation. The same transition handles
/// both legal orders of child stop and parent creation; wait selection is not
/// an admission authority.
fn handle_trace_event(
    policy: &ValidatedPolicy,
    events: &mut EventJournal,
    trace: &mut TraceTree,
    pid: libc::pid_t,
    status: libc::c_int,
    generation: &mut u64,
    hash_cache: &mut ImageHashCache,
) -> Result<(), String> {
    if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
        if trace.terminal(pid, status, events)? {
            trace.terminate()?;
        }
        return Ok(());
    }
    if trace
        .tasks
        .get(&pid)
        .and_then(|task| task.process)
        .is_none()
    {
        // The child's initial stop may arrive before its parent's creation
        // event. Keep it stopped until that real event admits the process.
        if trace.terminating {
            trace.retire_stop(pid)?;
        }
        return Ok(());
    }
    let event = (status as u32) >> 16;
    let deliver = signal_to_deliver(
        pid,
        status,
        trace.tasks.get(&pid).expect("waited task retained"),
    )?;
    match event as libc::c_int {
        libc::PTRACE_EVENT_FORK | libc::PTRACE_EVENT_VFORK | libc::PTRACE_EVENT_CLONE => {
            let child = event_message(pid)?;
            if child <= 0 {
                return Err("creation event returned an invalid child id".to_owned());
            }
            trace.retain_task(child, None)?;
            *generation = generation
                .checked_add(1)
                .ok_or_else(|| "process generation overflow".to_owned())?;
            let parent = trace
                .tasks
                .get(&pid)
                .and_then(|task| task.process)
                .ok_or_else(|| "clone event has no admitted process owner".to_owned())?;
            let is_process = match classify_creation_event(event, child, thread_group_id(child)) {
                Ok(process) => process,
                Err(reason) => {
                    events.record(
                        child as u32,
                        stable_id(child, *generation),
                        ProcessEventKind::CloneUnclassified {
                            parent_process_id: parent as u32,
                            reason: reason.clone(),
                        },
                    )?;
                    return Err(reason);
                }
            };
            if is_process {
                trace.admit_process(child, *generation)?;
                let inherited = trace
                    .processes
                    .get(&parent)
                    .and_then(|process| process.image.clone());
                let process = trace.processes.get_mut(&child).expect("child admitted");
                process.image = inherited.clone();
                let outcome = events.record(
                    child as u32,
                    process.stable_id.clone(),
                    ProcessEventKind::Fork {
                        parent_process_id: parent as u32,
                        image: inherited,
                    },
                )?;
                process.recorded = true;
                if outcome.must_terminate_closure() {
                    trace.terminate()?;
                }
            } else {
                let task = trace.tasks.get_mut(&child).expect("creation task inserted");
                if task.terminal.is_some() || task.process.is_some() {
                    return Err(format!(
                        "thread {child} was reaped or already admitted before its creation event"
                    ));
                }
                task.process = Some(parent);
            }
            if trace.tasks.get(&child).and_then(|task| task.stop).is_some() {
                if trace.terminating {
                    trace.retire_stop(child)?;
                } else {
                    trace.resume(child, 0)?;
                }
            }
        }
        libc::PTRACE_EVENT_EXEC => {
            let former_tid = event_message(pid)?;
            reconcile_exec_tid(&mut trace.tasks, former_tid, pid);
            let image = proc_image_identity(policy, pid, hash_cache)?;
            let process = trace
                .processes
                .get_mut(&pid)
                .ok_or_else(|| format!("exec event {pid} has no admitted process capability"))?;
            process.image = Some(image.clone());
            let outcome = events.record(
                pid as u32,
                process.stable_id.clone(),
                ProcessEventKind::Exec { image },
            )?;
            if outcome.must_terminate_closure() {
                trace.terminate()?;
            }
        }
        _ => {}
    }
    if trace.terminating {
        trace.retire_stop(pid)?;
    } else {
        trace.resume(pid, deliver)?;
    }
    Ok(())
}

fn signal_to_deliver(
    pid: libc::pid_t,
    status: libc::c_int,
    task: &TraceTask,
) -> Result<libc::c_int, String> {
    let event = (status as u32) >> 16;
    if event != 0 {
        return match event as libc::c_int {
            libc::PTRACE_EVENT_FORK
            | libc::PTRACE_EVENT_VFORK
            | libc::PTRACE_EVENT_CLONE
            | libc::PTRACE_EVENT_EXEC
            | libc::PTRACE_EVENT_EXIT => Ok(0),
            _ => Err(format!("unsupported ptrace event {event} for task {pid}")),
        };
    }
    let signal = libc::WSTOPSIG(status);
    if !task.resumed && signal == libc::SIGSTOP {
        return Ok(0); // Actual retained initial child stop, before any user entry.
    }
    if matches!(
        signal,
        libc::SIGSTOP | libc::SIGTSTP | libc::SIGTTIN | libc::SIGTTOU
    ) {
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
        if unsafe {
            libc::ptrace(
                libc::PTRACE_GETSIGINFO,
                pid,
                null_mut::<libc::c_void>(),
                info.as_mut_ptr().cast::<libc::c_void>(),
            )
        } < 0
        {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINVAL) {
                // LISTEN requires SEIZE custody. CONT here would silently undo
                // the application's job-control stop under TRACEME.
                return Err(format!(
                    "unsupported job-control group stop: signal {signal} for task {pid}; TRACEME custody cannot preserve this stop"
                ));
            }
            return Err(format!(
                "cannot classify signal {signal} stop for task {pid}: {error}"
            ));
        }
    }
    // TRACEEXEC was enabled before first release and inherited by every
    // child, so exec traps have an event tag. An event==0 SIGTRAP is the
    // program's signal; injecting zero would cancel it in ptrace_signal().
    Ok(signal)
}

fn event_message(pid: libc::pid_t) -> Result<libc::pid_t, String> {
    let mut value = 0_usize;
    if unsafe {
        libc::ptrace(
            libc::PTRACE_GETEVENTMSG,
            pid,
            null_mut::<libc::c_void>(),
            &mut value as *mut _ as *mut libc::c_void,
        )
    } < 0
    {
        return Err(os_error("ptrace(PTRACE_GETEVENTMSG)"));
    }
    libc::pid_t::try_from(value).map_err(|_| "ptrace event process id overflow".to_owned())
}

fn ptrace_continue(pid: libc::pid_t, signal: libc::c_int) -> Result<(), String> {
    if unsafe {
        libc::ptrace(
            libc::PTRACE_CONT,
            pid,
            null_mut::<libc::c_void>(),
            signal as usize as *mut libc::c_void,
        )
    } < 0
    {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("ptrace(PTRACE_CONT) failed: {error}"));
        }
    }
    Ok(())
}

fn exit_code(status: libc::c_int) -> i64 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status) as i64
    } else {
        (128 + libc::WTERMSIG(status)) as i64
    }
}

fn is_initial_stop(status: libc::c_int) -> bool {
    libc::WIFSTOPPED(status)
        && libc::WSTOPSIG(status) == libc::SIGSTOP
        && (status as u32 >> 16) == 0
}

fn is_exit_stop(status: libc::c_int) -> bool {
    libc::WIFSTOPPED(status) && (status as u32 >> 16) == libc::PTRACE_EVENT_EXIT as u32
}

fn cstrings(values: &[String], label: &str) -> Result<Vec<CString>, String> {
    values
        .iter()
        .map(|value| CString::new(value.as_bytes()).map_err(|_| format!("{label} contains NUL")))
        .collect()
}

fn proc_image_identity(
    policy: &ValidatedPolicy,
    pid: libc::pid_t,
    hash_cache: &mut ImageHashCache,
) -> Result<FileIdentity, String> {
    let proc_path = PathBuf::from(format!("/proc/{pid}/exe"));
    let path = std::fs::read_link(&proc_path)
        .map_err(|error| format!("cannot read {}: {error}", proc_path.display()))?;
    let file = std::fs::File::open(&proc_path)
        .map_err(|error| format!("cannot open {}: {error}", proc_path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot stat executable: {error}"))?;
    let cache_key = crate::image_cache::opened_file_key(&file).map_err(|e| e.to_string())?;
    let file_id = cache_key.stable_file_id().to_owned();
    let mut reader = file;
    let sha256 = hash_cache
        .digest(&cache_key, &mut reader, |file| {
            crate::image_cache::opened_file_key(file)
        })
        .map_err(|error| format!("cannot hash executable: {error}"))?;
    Ok(policy.classify_path(&path, file_id, metadata.len(), sha256))
}

fn classify_creation_event(
    event: u32,
    child: libc::pid_t,
    thread_group_id: Option<libc::pid_t>,
) -> Result<bool, String> {
    // Event choice uses CLONE_VFORK / exit_signal, not CLONE_THREAD. In
    // particular legacy CLONE_THREAD | SIGCHLD can produce EVENT_FORK.
    thread_group_id.map(|tgid| tgid == child).ok_or_else(|| {
        format!(
            "cannot classify ptrace creation event {event} child {child}: /proc thread-group identity unavailable"
        )
    })
}

fn reconcile_exec_tid(
    tasks: &mut HashMap<libc::pid_t, TraceTask>,
    former_tid: libc::pid_t,
    current_pid: libc::pid_t,
) {
    if former_tid != 0
        && former_tid != current_pid
        && tasks
            .get(&former_tid)
            .is_some_and(|task| task.process == Some(current_pid) && task.stop.is_none())
    {
        tasks.remove(&former_tid);
    }
}

fn thread_group_id(pid: libc::pid_t) -> Option<libc::pid_t> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("Tgid:")
            .and_then(|value| value.trim().parse().ok())
    })
}

fn stable_id(pid: libc::pid_t, generation: u64) -> String {
    let start = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|value| value.rsplit(')').next().map(str::to_owned))
        .and_then(|tail| tail.split_whitespace().nth(19).map(str::to_owned))
        .unwrap_or_else(|| generation.to_string());
    format!("linux:{pid}:{start}")
}

fn os_error(operation: &str) -> String {
    format!("{operation} failed: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    // These controls exercise the real kernel wait/ptrace boundary. Every
    // native child in this library test binary belongs to this serialized
    // fixture; integration-test children have separate process wait domains.

    #[derive(Clone, Copy)]
    enum KernelFixtureKind {
        ForkExec,
        LeaderExitWithLiveThread,
        GroupStop(libc::c_int),
        Exit42,
    }

    struct WorkerPipes {
        ready: libc::c_int,
        completion: libc::c_int,
    }

    extern "C" fn finite_kernel_worker(raw: *mut libc::c_void) -> libc::c_int {
        // Raw clone shares no Rust thread/runtime state. The callback uses
        // only syscall wrappers and returns through libc's SYS_exit wrapper.
        unsafe {
            let pipes = &*raw.cast::<WorkerPipes>();
            let byte = 1_u8;
            if libc::write(pipes.ready, (&byte as *const u8).cast(), 1) != 1 {
                return 81;
            }
            let mut delay = libc::timespec {
                tv_sec: 5,
                tv_nsec: 0,
            };
            loop {
                let mut remaining = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                };
                if libc::nanosleep(&delay, &mut remaining) == 0 {
                    break;
                }
                if *libc::__errno_location() != libc::EINTR {
                    return 82;
                }
                delay = remaining;
            }
            if libc::write(pipes.completion, (&byte as *const u8).cast(), 1) != 1 {
                return 83;
            }
            0
        }
    }

    struct KernelFixture {
        trace: TraceTree,
        root: libc::pid_t,
        completion: OwnedFd,
    }

    impl KernelFixture {
        fn launch(kind: KernelFixtureKind) -> Self {
            // Prepare all memory and descriptors before fork-like clone3.
            let mut stack = vec![0_u128; 4096];
            let top = unsafe { stack.as_mut_ptr().add(stack.len()).cast::<libc::c_void>() };
            let (completion, writer) = setup_pipe().unwrap();
            let mut ready = [-1; 2];
            assert_eq!(
                unsafe { libc::pipe2(ready.as_mut_ptr(), libc::O_CLOEXEC) },
                0
            );
            let ready_read = unsafe { OwnedFd::from_raw_fd(ready[0]) };
            let ready_write = unsafe { OwnedFd::from_raw_fd(ready[1]) };
            let mut pipes = WorkerPipes {
                ready: ready_write.as_raw_fd(),
                completion: writer.as_raw_fd(),
            };
            let mut default_action: libc::sigaction = unsafe { std::mem::zeroed() };
            default_action.sa_sigaction = libc::SIG_DFL;
            let mut unblocked: libc::sigset_t = unsafe { std::mem::zeroed() };
            assert_eq!(unsafe { libc::sigemptyset(&mut default_action.sa_mask) }, 0);
            assert_eq!(unsafe { libc::sigemptyset(&mut unblocked) }, 0);
            if let KernelFixtureKind::GroupStop(signal) = kind {
                assert_eq!(unsafe { libc::sigaddset(&mut unblocked, signal) }, 0);
            }
            let exec_path = CString::new("/bin/echo").unwrap();
            let exec_argument = CString::new([1_u8]).unwrap();
            let exec_argv = [exec_path.as_ptr(), exec_argument.as_ptr(), std::ptr::null()];
            let exec_environment: [*const libc::c_char; 1] = [std::ptr::null()];
            let parent = unsafe { libc::getpid() };
            let mut root_pidfd = -1;
            let args = CloneArgs {
                flags: libc::CLONE_PIDFD as u64,
                pidfd: (&mut root_pidfd as *mut libc::c_int) as usize as u64,
                exit_signal: libc::SIGCHLD as u64,
                ..CloneArgs::default()
            };
            let root =
                unsafe { libc::syscall(libc::SYS_clone3, &args, std::mem::size_of::<CloneArgs>()) };
            assert!(
                root >= 0,
                "native fixture clone3: {}",
                std::io::Error::last_os_error()
            );
            if root == 0 {
                unsafe {
                    // This prelude has the same explicitly non-atomic
                    // pre-admission parent-death boundary as production.
                    if libc::prctl(
                        libc::PR_SET_PDEATHSIG,
                        libc::SIGKILL as usize,
                        0_usize,
                        0_usize,
                        0_usize,
                    ) != 0
                        || libc::getppid() != parent
                        || libc::setpgid(0, 0) != 0
                        || libc::ptrace(
                            libc::PTRACE_TRACEME,
                            0,
                            null_mut::<libc::c_void>(),
                            null_mut::<libc::c_void>(),
                        ) < 0
                    {
                        libc::_exit(84);
                    }
                    if let KernelFixtureKind::GroupStop(signal) = kind {
                        // A separate process group with a live parent in this
                        // session is not orphaned. TSTP/TTIN/TTOU therefore
                        // exercise real job control instead of being ignored.
                        if (signal != libc::SIGSTOP
                            && libc::sigaction(signal, &default_action, null_mut()) != 0)
                            || libc::sigprocmask(libc::SIG_UNBLOCK, &unblocked, null_mut()) != 0
                        {
                            libc::_exit(90);
                        }
                    }
                    libc::close(completion.as_raw_fd());
                    if libc::raise(libc::SIGSTOP) != 0 {
                        libc::_exit(85);
                    }
                    match kind {
                        KernelFixtureKind::LeaderExitWithLiveThread => {
                            if libc::clone(
                                finite_kernel_worker,
                                top,
                                libc::CLONE_THREAD | libc::CLONE_VM | libc::CLONE_SIGHAND,
                                (&mut pipes as *mut WorkerPipes).cast::<libc::c_void>(),
                            ) < 0
                            {
                                libc::_exit(86);
                            }
                            libc::close(ready_write.as_raw_fd());
                            let mut byte = 0_u8;
                            loop {
                                let count = libc::read(
                                    ready_read.as_raw_fd(),
                                    (&mut byte as *mut u8).cast(),
                                    1,
                                );
                                if count == 1 {
                                    break;
                                }
                                if count < 0 && *libc::__errno_location() == libc::EINTR {
                                    continue;
                                }
                                libc::_exit(87);
                            }
                            // Exit this task only, leaving the actual worker
                            // alive. libc::_exit would terminate the group.
                            libc::syscall(libc::SYS_exit, 0);
                            libc::_exit(88);
                        }
                        KernelFixtureKind::GroupStop(signal) => {
                            if libc::raise(signal) != 0 {
                                libc::_exit(89);
                            }
                            let byte = 1_u8;
                            libc::write(writer.as_raw_fd(), (&byte as *const u8).cast(), 1);
                            libc::_exit(0);
                        }
                        KernelFixtureKind::ForkExec => {
                            let child = libc::fork();
                            if child < 0 {
                                libc::_exit(91);
                            }
                            if child == 0 {
                                if libc::dup2(writer.as_raw_fd(), libc::STDOUT_FILENO) < 0 {
                                    libc::_exit(92);
                                }
                                libc::execve(
                                    exec_path.as_ptr(),
                                    exec_argv.as_ptr(),
                                    exec_environment.as_ptr(),
                                );
                                libc::_exit(93);
                            }
                            let mut status = 0;
                            if libc::waitpid(child, &mut status, 0) != child {
                                libc::_exit(94);
                            }
                            libc::_exit(if libc::WIFEXITED(status) {
                                libc::WEXITSTATUS(status)
                            } else {
                                95
                            });
                        }
                        KernelFixtureKind::Exit42 => libc::_exit(42),
                    }
                }
            }
            assert!(root_pidfd >= 0);
            let mut trace = TraceTree::new(root as libc::pid_t, unsafe {
                OwnedFd::from_raw_fd(root_pidfd)
            });
            drop(writer);
            drop(ready_read);
            drop(ready_write);
            let (pid, status) = trace.wait(root as libc::pid_t, false).unwrap().unwrap();
            assert_eq!(pid, root as libc::pid_t);
            assert!(is_initial_stop(status), "bootstrap status {status:#x}");
            let options = libc::PTRACE_O_EXITKILL
                | libc::PTRACE_O_TRACECLONE
                | libc::PTRACE_O_TRACEFORK
                | libc::PTRACE_O_TRACEEXEC
                | libc::PTRACE_O_TRACEEXIT;
            assert_eq!(
                unsafe {
                    libc::ptrace(
                        libc::PTRACE_SETOPTIONS,
                        pid,
                        null_mut::<libc::c_void>(),
                        options as usize as *mut libc::c_void,
                    )
                },
                0
            );
            trace.resume(pid, 0).unwrap();
            Self {
                trace,
                root: pid,
                completion,
            }
        }

        fn hold_leader_exit(&mut self) {
            loop {
                let (pid, status) = self.trace.wait(-1, false).unwrap().unwrap();
                assert!(
                    libc::WIFSTOPPED(status),
                    "unexpected fixture status {status:#x}"
                );
                if pid == self.root && is_exit_stop(status) {
                    return;
                }
                if self.trace.tasks[&pid].process.is_none() {
                    continue; // Child stop can precede its parent's event.
                }
                match (status as u32 >> 16) as libc::c_int {
                    libc::PTRACE_EVENT_CLONE | libc::PTRACE_EVENT_FORK => {
                        let child = event_message(pid).unwrap();
                        assert_eq!(thread_group_id(child), Some(self.root));
                        let task = self.trace.tasks.entry(child).or_default();
                        task.process = Some(self.root);
                        if task.stop.is_some() {
                            self.trace.resume(child, 0).unwrap();
                        }
                    }
                    _ => {}
                }
                let signal = signal_to_deliver(pid, status, &self.trace.tasks[&pid]).unwrap();
                self.trace.resume(pid, signal).unwrap();
            }
        }

        fn completion_is_pending(&self) {
            let mut byte = 0_u8;
            assert_eq!(
                unsafe {
                    libc::read(
                        self.completion.as_raw_fd(),
                        (&mut byte as *mut u8).cast(),
                        1,
                    )
                },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EAGAIN)
            );
        }

        fn assert_completion(&self, expected_count: isize) {
            let mut byte = 0_u8;
            assert_eq!(
                unsafe {
                    libc::read(
                        self.completion.as_raw_fd(),
                        (&mut byte as *mut u8).cast(),
                        1,
                    )
                },
                expected_count
            );
            if expected_count == 1 {
                assert_eq!(byte, 1);
            }
        }

        fn drain(&mut self) -> String {
            let result = self.trace.drain(None);
            assert!(
                self.trace.wait_lost
                    && self.trace.tasks.is_empty()
                    && self.trace.processes.is_empty(),
                "{}; {:?}",
                result.summary,
                result.errors
            );
            self.trace.finalized = true;
            // No fabricated policy/journal is installed for these raw kernel
            // controls. The actual unrecorded terminal status remains an error.
            assert!(
                result.error_count > 0,
                "raw fixture cannot claim journal publication"
            );
            format!("{}; {}", result.summary, result.errors.join("; "))
        }
    }

    #[test]
    fn leader_exit_stop_cleanup_terminates_live_siblings_through_the_process_handle() {
        let _serial = TEST_WAIT_CUSTODY.lock().unwrap();
        // Causal positive control: the real worker reaches its marker after
        // the leader exits when process cleanup is not requested.
        let mut baseline = KernelFixture::launch(KernelFixtureKind::LeaderExitWithLiveThread);
        baseline.hold_leader_exit();
        baseline.completion_is_pending();
        baseline.trace.resume(baseline.root, 0).unwrap();
        while let Some((pid, status)) = baseline.trace.wait(-1, false).unwrap() {
            if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                baseline.trace.tasks.remove(&pid);
                baseline.trace.processes.remove(&pid);
            } else {
                let signal = signal_to_deliver(pid, status, &baseline.trace.tasks[&pid]).unwrap();
                baseline.trace.resume(pid, signal).unwrap();
            }
        }
        assert!(baseline.trace.tasks.is_empty() && baseline.trace.processes.is_empty());
        baseline.trace.finalized = true;
        baseline.assert_completion(1);

        let mut fixture = KernelFixture::launch(KernelFixtureKind::LeaderExitWithLiveThread);
        fixture.hold_leader_exit();
        fixture.completion_is_pending();
        // Invoke the real error cleanup while the leader is at EXIT. The
        // worker completion byte distinguishes a skipped whole-process kill
        // from cleanup; elapsed time and a modeled task set are not the oracle.
        let report = fixture.drain();
        assert!(report.contains("cleanup terminal waits ["));
        fixture.assert_completion(0);
    }

    #[test]
    fn cleanup_of_single_task_exit_stop_retains_actual_exit_status() {
        let _serial = TEST_WAIT_CUSTODY.lock().unwrap();
        let mut fixture = KernelFixture::launch(KernelFixtureKind::Exit42);
        fixture.hold_leader_exit();
        let report = fixture.drain();
        assert!(
            report.contains(&format!("{}:0x2a00", fixture.root)),
            "{report}"
        );
        fixture.assert_completion(0);
    }

    #[test]
    fn application_stop_is_delivered_then_unsupported_group_stop_is_refused() {
        let _serial = TEST_WAIT_CUSTODY.lock().unwrap();
        for signal in [libc::SIGSTOP, libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
            let mut fixture = KernelFixture::launch(KernelFixtureKind::GroupStop(signal));
            let (pid, delivery) = fixture.trace.wait(fixture.root, false).unwrap().unwrap();
            assert!(libc::WIFSTOPPED(delivery));
            assert_eq!(libc::WSTOPSIG(delivery), signal);
            assert_eq!(delivery as u32 >> 16, 0);
            let forwarded = signal_to_deliver(pid, delivery, &fixture.trace.tasks[&pid]).unwrap();
            assert_eq!(forwarded, signal);
            fixture.trace.resume(pid, forwarded).unwrap();
            let (pid, group_stop) = fixture.trace.wait(fixture.root, false).unwrap().unwrap();
            let refusal =
                signal_to_deliver(pid, group_stop, &fixture.trace.tasks[&pid]).unwrap_err();
            assert!(
                refusal.contains("unsupported job-control group stop"),
                "{refusal}"
            );
            fixture.completion_is_pending();
            fixture.drain();
            fixture.assert_completion(0); // Code following the stop never ran.
        }
    }

    #[test]
    fn setup_ack_requires_exact_framing_and_closed_writer() {
        use std::io::Write;
        let valid: Vec<u8> = [0x4d43_5031_u32, 1, 0, 0]
            .into_iter()
            .flat_map(u32::to_ne_bytes)
            .collect();
        for length in [0, 7, 16, 17] {
            let (reader, writer) = setup_pipe().unwrap();
            let mut writer = std::fs::File::from(writer);
            let bytes: Vec<_> = valid.iter().copied().chain([0]).take(length).collect();
            writer.write_all(&bytes).unwrap();
            drop(writer);
            assert_eq!(verify_setup(reader).is_ok(), length == 16);
        }
        let (reader, writer) = setup_pipe().unwrap();
        let mut writer = std::fs::File::from(writer);
        writer.write_all(&valid).unwrap();
        // A correct frame alone is insufficient: the trusted child must have
        // closed the sole writer before the parent can release any subject.
        assert!(
            verify_setup(reader)
                .unwrap_err()
                .contains("before first release")
        );
        drop(writer);
    }

    #[test]
    fn setup_pipe_reader_is_nonblocking_and_both_ends_close_on_exec() {
        let (reader, writer) = setup_pipe().unwrap();
        for fd in [reader.as_raw_fd(), writer.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert!(flags >= 0);
            assert_ne!(flags & libc::FD_CLOEXEC, 0);
        }
        let flags = unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::O_NONBLOCK, 0);
    }

    #[test]
    fn ptrace_creation_with_unknown_thread_group_fails_closed() {
        for event in [
            libc::PTRACE_EVENT_CLONE,
            libc::PTRACE_EVENT_FORK,
            libc::PTRACE_EVENT_VFORK,
        ] {
            let error = classify_creation_event(event as u32, 41, None).unwrap_err();
            assert!(error.contains("thread-group identity unavailable"));
        }
    }

    #[test]
    fn ptrace_creation_kind_does_not_substitute_for_thread_group_identity() {
        for event in [
            libc::PTRACE_EVENT_CLONE,
            libc::PTRACE_EVENT_FORK,
            libc::PTRACE_EVENT_VFORK,
        ] {
            assert!(classify_creation_event(event as u32, 41, Some(41)).unwrap());
            assert!(!classify_creation_event(event as u32, 41, Some(40)).unwrap());
        }
    }

    #[test]
    fn actual_child_stop_before_parent_creation_never_releases_unadmitted_code() {
        let _serial = TEST_WAIT_CUSTODY.lock().unwrap();
        for child_first in [false, true] {
            let root_image = std::env::current_exe().unwrap();
            let child_image = std::fs::canonicalize("/bin/echo").unwrap();
            let directory = std::env::temp_dir().join(format!(
                "molt-kernel-event-order-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                child_first
            ));
            std::fs::create_dir(&directory).unwrap();
            let receipt = directory.join("receipt.json");
            let policy = crate::Policy {
                schema: crate::POLICY_SCHEMA.to_owned(),
                nonce: "a".repeat(32),
                mode: ClosureMode::DeclaredTree,
                cwd: std::env::current_dir().unwrap(),
                command: vec![root_image.to_str().unwrap().to_owned()],
                environment: super::super::required_environment(),
                root_role: "root".to_owned(),
                fixed_images: [("root", root_image), ("child", child_image)]
                    .into_iter()
                    .map(|(role, path)| crate::FixedImage {
                        role: role.to_owned(),
                        sha256: crate::sha256_file(&path).unwrap(),
                        path,
                        root_exit_disposition: crate::RootExitDisposition::RequireExit,
                    })
                    .collect(),
                derived_roots: vec![],
            }
            .validate()
            .unwrap();
            let cap = capability(policy.policy.mode);
            assert_eq!(cap.admission, Admission::Eligible {});
            let mut journal = EventJournal::create(&receipt, &policy, &cap).unwrap();
            let mut fixture = KernelFixture::launch(KernelFixtureKind::ForkExec);
            let (parent, parent_status) = fixture.trace.wait(fixture.root, false).unwrap().unwrap();
            assert_eq!(parent, fixture.root);
            assert_eq!(parent_status as u32 >> 16, libc::PTRACE_EVENT_FORK as u32);
            let child = event_message(parent).unwrap();
            let root_id = fixture.trace.processes[&parent].stable_id.clone();
            let mut cache = ImageHashCache::default();
            let image = proc_image_identity(&policy, parent, &mut cache).unwrap();
            journal
                .record(
                    parent as u32,
                    root_id.clone(),
                    ProcessEventKind::ProcessCreate {
                        parent_process_id: None,
                    },
                )
                .unwrap();
            journal
                .record(
                    parent as u32,
                    root_id,
                    ProcessEventKind::Exec {
                        image: image.clone(),
                    },
                )
                .unwrap();
            let root = fixture.trace.processes.get_mut(&parent).unwrap();
            root.recorded = true;
            root.image = Some(image);
            let mut generation = 1;
            if !child_first {
                handle_trace_event(
                    &policy,
                    &mut journal,
                    &mut fixture.trace,
                    parent,
                    parent_status,
                    &mut generation,
                    &mut cache,
                )
                .unwrap();
            }
            let (observed_child, child_status) = fixture.trace.wait(child, false).unwrap().unwrap();
            assert_eq!(observed_child, child);
            assert!(is_initial_stop(child_status));
            handle_trace_event(
                &policy,
                &mut journal,
                &mut fixture.trace,
                child,
                child_status,
                &mut generation,
                &mut cache,
            )
            .unwrap();
            if child_first {
                // Independent kernel oracle: PTRACE_CONT destroys this actual
                // delivery stop. A premature resume yields ESRCH or the later
                // EXEC/SIGTRAP stop, never this still-owned initial SIGSTOP.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                assert_eq!(
                    unsafe {
                        libc::ptrace(
                            libc::PTRACE_GETSIGINFO,
                            child,
                            null_mut::<libc::c_void>(),
                            (&mut info as *mut libc::siginfo_t).cast::<libc::c_void>(),
                        )
                    },
                    0
                );
                assert_eq!(info.si_signo, libc::SIGSTOP);
                fixture.completion_is_pending();
                handle_trace_event(
                    &policy,
                    &mut journal,
                    &mut fixture.trace,
                    parent,
                    parent_status,
                    &mut generation,
                    &mut cache,
                )
                .unwrap();
            }
            while let Some((pid, status)) = fixture.trace.wait(-1, false).unwrap() {
                handle_trace_event(
                    &policy,
                    &mut journal,
                    &mut fixture.trace,
                    pid,
                    status,
                    &mut generation,
                    &mut cache,
                )
                .unwrap();
            }
            assert!(fixture.trace.custody().is_closed());
            fixture.trace.finalized = true;
            let mut bytes = [0_u8; 2];
            assert_eq!(
                unsafe { libc::read(fixture.completion.as_raw_fd(), bytes.as_mut_ptr().cast(), 2) },
                2
            );
            assert_eq!(bytes, [1, b'\n']);
            let evidence = journal.publish().unwrap();
            assert_eq!(evidence.verified.accounting.process_creates, 2);
            assert_eq!(evidence.verified.accounting.process_exits, 2);
            assert_eq!(evidence.verified.accounting.active_processes, 0);
            let rows = std::fs::read_to_string(directory.join(&evidence.event_log.file)).unwrap();
            let events: Vec<crate::ProcessEvent> = rows
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let child_events: Vec<_> = events
                .iter()
                .filter(|e| e.process_id == child as u32)
                .collect();
            assert!(matches!(
                child_events[0].event,
                ProcessEventKind::Fork { .. }
            ));
            assert!(matches!(
                child_events[1].event,
                ProcessEventKind::Exec { .. }
            ));
            assert!(matches!(
                child_events[2].event,
                ProcessEventKind::ProcessExit { exit_code: 0 }
            ));
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn nonleader_exec_removes_the_obsolete_thread_identity() {
        let mut tasks = HashMap::from([
            (
                40,
                TraceTask {
                    process: Some(40),
                    ..TraceTask::default()
                },
            ),
            (
                41,
                TraceTask {
                    process: Some(40),
                    ..TraceTask::default()
                },
            ),
        ]);
        reconcile_exec_tid(&mut tasks, 41, 40);
        assert_eq!(tasks.keys().copied().collect::<Vec<_>>(), vec![40]);
    }

    #[test]
    fn nonleader_exec_preserves_a_reused_tid_waiting_for_its_creation_event() {
        // The old worker 41 became leader 40. Before that EXEC event reached
        // the tracer, the kernel reused 41 for a new child whose initial stop
        // has already been observed. Erasing it would lose the only stop and
        // leave the new child hung after its creation event is admitted.
        let stop = (libc::SIGSTOP << 8) | 0x7f;
        let mut tasks = HashMap::from([
            (
                40,
                TraceTask {
                    process: Some(40),
                    ..TraceTask::default()
                },
            ),
            (
                41,
                TraceTask {
                    stop: Some(stop),
                    ..TraceTask::default()
                },
            ),
        ]);
        reconcile_exec_tid(&mut tasks, 41, 40);
        let replacement = tasks.get(&41).expect("new child's observed stop retained");
        assert_eq!(replacement.stop, Some(stop));
        assert_eq!(replacement.process, None);
    }

    #[test]
    fn nonleader_exec_preserves_a_reused_tid_owned_by_another_process() {
        let mut tasks = HashMap::from([
            (
                40,
                TraceTask {
                    process: Some(40),
                    ..TraceTask::default()
                },
            ),
            (
                41,
                TraceTask {
                    process: Some(80),
                    resumed: true,
                    ..TraceTask::default()
                },
            ),
        ]);
        reconcile_exec_tid(&mut tasks, 41, 40);
        assert_eq!(
            tasks
                .get(&41)
                .expect("other process's task retained")
                .process,
            Some(80)
        );
    }
}
