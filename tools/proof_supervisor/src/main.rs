use molt_proof_supervisor::evidence::{
    MAX_RECEIPT_BYTES, OpenedRegularFile, durable_atomic_write, event_artifact_path,
    verify_event_artifact,
};
use molt_proof_supervisor::{
    ClosureMode, EXPORT_EVENT_MAX_BYTES, EXPORT_FOOTER_MAGIC, EXPORT_LENGTH_HEX_DIGITS,
    EXPORT_RECEIPT_MAX_BYTES, EventJournal, MAX_POLICY_BYTES, RECEIPT_SCHEMA, Receipt, platform,
    sha256_bytes, sha256_reader,
};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match dispatch(std::env::args().skip(1).collect()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("molt-proof-supervisor: {error}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(args: Vec<String>) -> Result<u8, String> {
    match args.as_slice() {
        [command, mode] if command == "capability" => {
            let mode = parse_mode(mode)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&platform::capability(mode))
                    .map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        [command, policy_flag, policy, receipt_flag, receipt]
            if command == "run" && policy_flag == "--policy" && receipt_flag == "--receipt" =>
        {
            run_policy(Path::new(policy), Path::new(receipt), false)
        }
        [command, policy_flag, policy, receipt_flag, receipt]
            if command == "run-export" && policy_flag == "--policy" && receipt_flag == "--receipt" =>
        {
            let result = run_policy(Path::new(policy), Path::new(receipt), false)?;
            export_evidence(Path::new(receipt))?;
            Ok(result)
        }
        [command, policy_flag, policy, receipt_flag, receipt]
            if command == "inventory"
                && policy_flag == "--policy"
                && receipt_flag == "--receipt" =>
        {
            run_policy(Path::new(policy), Path::new(receipt), true)
        }
        [command, policy_flag, policy, receipt_flag, receipt]
            if command == "verify" && policy_flag == "--policy" && receipt_flag == "--receipt" =>
        {
            verify_receipt(Path::new(policy), Path::new(receipt), None)
        }
        [command, root_flag, root, policy_flag, policy, receipt_flag, receipt]
            if command == "verify-rooted"
                && root_flag == "--rootfs"
                && policy_flag == "--policy"
                && receipt_flag == "--receipt" =>
        {
            verify_receipt(Path::new(policy), Path::new(receipt), Some(Path::new(root)))
        }
        [command, fixture, code] if command == "fixture-child" && fixture == "exit" => {
            let code: u8 = code
                .parse()
                .map_err(|_| "fixture exit code must be 0..255".to_owned())?;
            Ok(code)
        }
        [command, fixture, marker] if command == "fixture-child" && fixture == "write-marker" => {
            fs::write(marker, b"subject-entered\n").map_err(|error| error.to_string())?;
            Ok(0)
        }
        #[cfg(target_os = "linux")]
        [command, fixture, kind, marker, report]
            if command == "fixture-child" && fixture == "linux-creation-attempt" =>
        {
            linux_creation_attempt(kind, Path::new(marker), Path::new(report))
        }
        #[cfg(target_os = "linux")]
        [command, fixture, kind, marker, report]
            if command == "fixture-child" && fixture == "linux-creation-descendant" =>
        {
            let status = Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                .args(["fixture-child", "linux-creation-attempt", kind, marker, report])
                .status().map_err(|error| error.to_string())?;
            Ok(status.code().unwrap_or(1).clamp(0, 255) as u8)
        }
        #[cfg(target_os = "linux")]
        [command, fixture, operation, policy, receipt]
            if command == "fixture-child" && fixture == "linux-host-denial" =>
        {
            linux_host_denial(operation, policy, receipt)
        }
        #[cfg(target_os = "linux")]
        [command, fixture, report]
            if command == "fixture-child" && fixture == "linux-libc-thread-and-spawn" =>
        {
            linux_libc_thread_and_spawn(Path::new(report))
        }
        #[cfg(target_os = "linux")]
        [command, fixture, report]
            if command == "fixture-child" && fixture == "linux-clone-thread-sigchld" =>
        {
            linux_clone_thread_sigchld(Path::new(report))
        }
        #[cfg(target_os = "linux")]
        [command, fixture, marker]
            if command == "fixture-child" && fixture == "linux-nonleader-exec" =>
        {
            use std::os::unix::process::CommandExt;
            let marker = marker.clone();
            std::thread::spawn(move || {
                Command::new(std::env::current_exe().unwrap())
                    .args(["fixture-child", "write-marker", &marker]).exec()
            }).join().map_err(|_| "nonleader exec thread panicked".to_owned())?;
            Err("nonleader exec returned without replacing the process".to_owned())
        }
        #[cfg(target_os = "linux")]
        [command, fixture, root_marker, ready, owned_marker, owned_finished]
            if command == "fixture-child" && fixture == "linux-root-group-barrier" =>
        {
            Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                .args(["fixture-child", "linux-finite-owned-child", owned_marker, owned_finished])
                .spawn().map_err(|error| error.to_string())?;
            fixture_wait_file(Path::new(owned_marker))?;
            fs::write(root_marker, std::process::id().to_string()).map_err(|error| error.to_string())?;
            fixture_wait_file(Path::new(ready))?;
            Ok(0)
        }
        #[cfg(target_os = "linux")]
        [command, fixture, marker, finished]
            if command == "fixture-child" && fixture == "linux-finite-owned-child" =>
        {
            fs::write(marker, b"owned-entered\n").map_err(|error| error.to_string())?;
            std::thread::sleep(std::time::Duration::from_secs(5));
            fs::write(finished, b"owned-finished\n").map_err(|error| error.to_string())?;
            Ok(0)
        }
        #[cfg(target_os = "linux")]
        [command, fixture, root_marker, ready, release, finished]
            if command == "fixture-child" && fixture == "linux-join-root-group" =>
        {
            fixture_wait_file(Path::new(root_marker))?;
            let group: libc::pid_t = fs::read_to_string(root_marker).map_err(|error| error.to_string())?
                .parse().map_err(|_| "invalid fixture process group".to_owned())?;
            if group <= 1 || unsafe { libc::setpgid(0, group) } != 0 {
                return Err(format!("cannot join fixture root group: {}", std::io::Error::last_os_error()));
            }
            fs::write(ready, b"decoy-joined\n").map_err(|error| error.to_string())?;
            fixture_wait_file(Path::new(release))?;
            fs::write(finished, b"decoy-survived\n").map_err(|error| error.to_string())?;
            Ok(0)
        }
        #[cfg(target_os = "linux")]
        [command, fixture] if command == "fixture-child" && fixture == "application-trap-unhandled" => {
            unsafe { libc::raise(libc::SIGTRAP); }
            Ok(97) // Reaching this branch means the ordinary fatal signal was lost.
        }
        #[cfg(target_os = "linux")]
        [command, fixture, marker] if command == "fixture-child" && fixture == "application-trap-handled" => {
            application_trap_handled_fixture(Path::new(marker))?;
            Ok(0)
        }
        [command, fixture] if command == "fixture-child" && fixture == "spawn-self" => {
            let status = Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                .args(["fixture-child", "exit", "0"])
                .status()
                .map_err(|error| error.to_string())?;
            Ok(status.code().unwrap_or(1).clamp(0, 255) as u8)
        }
        [command, fixture] if command == "fixture-child" && fixture == "export-lookalike" => {
            print!("guest stdout remains unchanged\n");
            eprint!("guest stderr{EXPORT_FOOTER_MAGIC}00000000000000000000000000000000\n");
            Ok(0)
        }
        #[cfg(unix)]
        [command, fixture, image, rest @ ..]
            if command == "fixture-child" && fixture == "exec-image" =>
        {
            use std::os::unix::process::CommandExt;

            // Replace this process image in place; a successful exec never
            // returns, so reaching the error is the only outcome here.
            let error = Command::new(image)
                .arg("fixture-child")
                .args(rest)
                .exec();
            Err(format!("fixture exec-image failed: {error}"))
        }
        [command, fixture, auxiliary]
            if command == "fixture-child" && fixture == "spawn-and-wait" =>
        {
            let status = Command::new(auxiliary)
                .args(["fixture-child", "exit", "0"])
                .status()
                .map_err(|error| error.to_string())?;
            Ok(status.code().unwrap_or(1).clamp(0, 255) as u8)
        }
        [command, fixture, count] if command == "fixture-child" && fixture == "spawn-many" => {
            let count: usize = count
                .parse()
                .map_err(|_| "fixture spawn count must be an integer".to_owned())?;
            let executable = std::env::current_exe().map_err(|error| error.to_string())?;
            for _ in 0..count {
                let status = Command::new(&executable)
                    .args(["fixture-child", "exit", "0"])
                    .status()
                    .map_err(|error| error.to_string())?;
                if !status.success() {
                    return Ok(status.code().unwrap_or(1).clamp(0, 255) as u8);
                }
            }
            Ok(0)
        }
        [command, fixture, auxiliary, marker]
            if command == "fixture-child" && fixture == "spawn-auxiliary" =>
        {
            Command::new(auxiliary)
                .args(["fixture-child", "write-pid-and-sleep-leaf", marker])
                .spawn()
                .map_err(|error| error.to_string())?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !Path::new(marker).is_file() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if !Path::new(marker).is_file() {
                return Err("auxiliary fixture did not reach user code".to_owned());
            }
            Ok(0)
        }
        [command, fixture] if command == "fixture-child" && fixture == "thread-storm" => {
            let threads: Vec<_> = (0..256).map(|_| std::thread::spawn(|| {})).collect();
            for thread in threads {
                thread
                    .join()
                    .map_err(|_| "fixture thread panicked".to_owned())?;
            }
            Ok(0)
        }
        [command, fixture] if command == "fixture-child" && fixture == "application-breakpoint" => {
            application_breakpoint_fixture()
        }
        #[cfg(windows)]
        [command, fixture] if command == "fixture-child" && fixture == "normal-heap-leaf" => {
            normal_heap_fixture()?;
            Ok(0)
        }
        #[cfg(windows)]
        [command, fixture] if command == "fixture-child" && fixture == "normal-heap-tree" => {
            normal_heap_fixture()?;
            let status = Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                .args(["fixture-child", "normal-heap-leaf"])
                .status()
                .map_err(|error| error.to_string())?;
            if !status.success() {
                return Err(format!("normal-heap descendant failed: {status}"));
            }
            Ok(0)
        }
        [command, fixture, marker]
            if command == "fixture-child" && fixture == "write-pid-and-sleep-leaf" =>
        {
            fs::write(marker, std::process::id().to_string())
                .map_err(|error| format!("cannot write fixture marker: {error}"))?;
            std::thread::sleep(std::time::Duration::from_secs(60));
            Ok(0)
        }
        [command, fixture, root_marker, child_marker]
            if command == "fixture-child" && fixture == "write-pid-and-sleep-tree" =>
        {
            fs::write(root_marker, std::process::id().to_string())
                .map_err(|error| format!("cannot write fixture marker: {error}"))?;
            Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                .args(["fixture-child", "write-pid-and-sleep-leaf", child_marker])
                .spawn()
                .map_err(|error| error.to_string())?;
            std::thread::sleep(std::time::Duration::from_secs(60));
            Ok(0)
        }
        _ => Err("usage: capability <leaf|declared-tree|inventory-tree> | run --policy FILE --receipt FILE | run-export --policy FILE --receipt FILE | inventory --policy FILE --receipt FILE | verify --policy FILE --receipt FILE | verify-rooted --rootfs DIR --policy FILE --receipt FILE".to_owned()),
    }
}

#[cfg(target_os = "linux")]
fn fixture_wait_file(path: &Path) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !path.is_file() {
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "finite fixture barrier expired: {}",
                path.display()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn fixture_wait_child(pid: libc::pid_t) -> Result<(), String> {
    let mut status = 0;
    loop {
        let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
        if waited == pid {
            break;
        }
        if waited < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return Err(format!(
            "fixture child wait failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    if !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
        return Err(format!("fixture child terminal status {status:#x}"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_creation_attempt(kind: &str, marker: &Path, report: &Path) -> Result<u8, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let marker = CString::new(marker.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    // Deliberately independent of the supervisor's clone_args structure/filter.
    // The raw UAPI record uses only flags and SIGCHLD. A bypassed child performs
    // bounded async-safe marker I/O and exits; it cannot become an escaped daemon.
    let args = [
        AtomicU64::new(libc::CLONE_UNTRACED as u64),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(libc::SIGCHLD as u64),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ];
    let attempt = || -> Result<i32, String> {
        let mut flags = (libc::CLONE_UNTRACED | libc::SIGCHLD) as libc::c_ulong;
        if kind == "untraced-high-word" {
            flags |= (1_u64 << 32) as libc::c_ulong;
        }
        let result = match kind {
            "untraced" | "untraced-high-word" => unsafe {
                libc::syscall(libc::SYS_clone, flags, 0_usize, 0_usize, 0_usize, 0_usize)
            },
            "clone3" | "clone3-mutating" => unsafe {
                libc::syscall(
                    libc::SYS_clone3,
                    args.as_ptr(),
                    std::mem::size_of_val(&args),
                )
            },
            #[cfg(target_arch = "x86_64")]
            "x32-untraced" => unsafe {
                libc::syscall(
                    0x4000_0038 as libc::c_long,
                    flags,
                    0_usize,
                    0_usize,
                    0_usize,
                    0_usize,
                )
            },
            #[cfg(target_arch = "x86_64")]
            "x32-clone3" => unsafe {
                // Null is sufficient: the declared restriction precedes any
                // pointer validation; without it this is EFAULT, not ENOSYS.
                libc::syscall(0x4000_01b3 as libc::c_long, 0_usize, 64_usize)
            },
            #[cfg(target_arch = "x86_64")]
            "i386-untraced" => unsafe { fixture_int80(120, flags as u32, 0) },
            #[cfg(target_arch = "x86_64")]
            "i386-clone3" => unsafe { fixture_int80(435, 0, 64) },
            _ => return Err(format!("unsupported creation fixture {kind}")),
        };
        if result == 0 {
            unsafe {
                let fd = libc::open(
                    marker.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
                    0o600,
                );
                if fd < 0 {
                    libc::_exit(91);
                }
                let text = b"unexpected-child-entered\n";
                let written = libc::write(fd, text.as_ptr().cast(), text.len());
                libc::close(fd);
                libc::_exit(if written == text.len() as isize {
                    0
                } else {
                    92
                });
            }
        }
        if result < 0 {
            return Ok(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1));
        }
        fixture_wait_child(result as libc::pid_t)?;
        Ok(0)
    };
    let mut errors = Vec::new();
    if kind == "clone3-mutating" {
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| -> Result<(), String> {
            let writer = scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    args[0].store(0, Ordering::Relaxed);
                    args[0].store(libc::CLONE_UNTRACED as u64, Ordering::Relaxed);
                }
            });
            let result = (|| {
                for _ in 0..32 {
                    errors.push(attempt()?);
                }
                Ok(())
            })();
            stop.store(true, Ordering::Relaxed);
            writer
                .join()
                .map_err(|_| "clone_args writer panicked".to_owned())?;
            result
        })?;
    } else {
        errors.push(attempt()?);
    }
    fs::write(
        report,
        serde_json::to_vec(&errors).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(0)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn fixture_int80(number: u32, first: u32, second: u32) -> libc::c_long {
    let result: i32;
    unsafe {
        // Save RBX because LLVM reserves it in some targets. The actual int80
        // entry is used from a 64-bit image; no userspace ABI model substitutes
        // for the kernel. Older kernels may clear R8..R11 on this entry.
        std::arch::asm!(
            "xchg rbx, {saved}", "int 0x80", "xchg rbx, {saved}",
            saved = inout(reg) first as u64 => _,
            inlateout("eax") number => result,
            in("ecx") second, in("edx") 0_u32, in("esi") 0_u32, in("edi") 0_u32,
            lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        );
        if result < 0 {
            *libc::__errno_location() = -result;
            -1
        } else {
            result as libc::c_long
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_libc_thread_and_spawn(report: &Path) -> Result<u8, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    extern "C" fn worker(value: *mut libc::c_void) -> *mut libc::c_void {
        unsafe { &*(value.cast::<AtomicU32>()) }.store(7, Ordering::SeqCst);
        std::ptr::null_mut()
    }
    let observed = AtomicU32::new(0);
    let mut thread = std::mem::MaybeUninit::<libc::pthread_t>::uninit();
    let error = unsafe {
        libc::pthread_create(
            thread.as_mut_ptr(),
            std::ptr::null(),
            worker,
            (&observed as *const AtomicU32).cast_mut().cast(),
        )
    };
    if error != 0 {
        return Err(format!("pthread_create errno {error}"));
    }
    let error = unsafe { libc::pthread_join(thread.assume_init(), std::ptr::null_mut()) };
    if error != 0 || observed.load(Ordering::SeqCst) != 7 {
        return Err(format!("pthread did not complete: errno {error}"));
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let executable =
        CString::new(executable.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    let args = [
        executable,
        CString::new("fixture-child").unwrap(),
        CString::new("exit").unwrap(),
        CString::new("0").unwrap(),
    ];
    let argv: Vec<_> = args
        .iter()
        .map(|value| value.as_ptr().cast_mut())
        .chain([std::ptr::null_mut()])
        .collect();
    let env = [std::ptr::null_mut::<libc::c_char>()];
    let mut pid = 0;
    let error = unsafe {
        libc::posix_spawn(
            &mut pid,
            args[0].as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            argv.as_ptr(),
            env.as_ptr(),
        )
    };
    if error != 0 {
        return Err(format!("posix_spawn errno {error}"));
    }
    fixture_wait_child(pid)?;
    fs::write(report, b"pthread=7;posix_spawn=0\n").map_err(|error| error.to_string())?;
    Ok(0)
}

#[cfg(target_os = "linux")]
fn linux_clone_thread_sigchld(report: &Path) -> Result<u8, String> {
    use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
    extern "C" fn worker(value: *mut libc::c_void) -> libc::c_int {
        // The raw clone callback uses no libc TLS or Rust thread apparatus.
        unsafe { &*(value.cast::<AtomicU32>()) }.store(7, Ordering::SeqCst);
        0
    }
    let observed = AtomicU32::new(0);
    let child_tid = AtomicI32::new(1);
    let mut stack = vec![0_u128; 4096];
    let top = unsafe { stack.as_mut_ptr().add(stack.len()) }.cast::<libc::c_void>();
    let flags = libc::CLONE_THREAD
        | libc::CLONE_VM
        | libc::CLONE_SIGHAND
        | libc::CLONE_CHILD_SETTID
        | libc::CLONE_CHILD_CLEARTID
        | libc::SIGCHLD;
    let tid = unsafe {
        libc::clone(
            worker,
            top,
            flags,
            (&observed as *const AtomicU32).cast_mut().cast(),
            std::ptr::null_mut::<libc::pid_t>(),
            std::ptr::null_mut::<libc::c_void>(),
            child_tid.as_ptr(),
        )
    };
    if tid < 0 {
        return Err(format!(
            "clone thread with SIGCHLD failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    // A callback completion flag alone does not retire its stack. Kernel
    // clear_child_tid provides the independent lifetime boundary.
    while child_tid.load(Ordering::SeqCst) != 0 {
        if std::time::Instant::now() >= deadline {
            // Do not unwind/free a live raw thread stack on failure.
            unsafe {
                libc::_exit(93);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    if observed.load(Ordering::SeqCst) != 7 {
        return Err("raw clone callback did not execute".to_owned());
    }
    fs::write(report, b"clone-thread-sigchld=7\n").map_err(|error| error.to_string())?;
    Ok(0)
}

#[cfg(target_os = "linux")]
fn linux_host_denial(operation: &str, policy: &str, receipt: &str) -> Result<u8, String> {
    use std::os::unix::process::CommandExt;
    let (number, argument) = match operation {
        "clone3" => (libc::SYS_clone3, None),
        "pidfd-open" => (libc::SYS_pidfd_open, None),
        "parent-death" => (libc::SYS_prctl, Some(libc::PR_SET_PDEATHSIG)),
        "no-new-privs" => (libc::SYS_prctl, Some(libc::PR_SET_NO_NEW_PRIVS)),
        "filter" => (libc::SYS_prctl, Some(libc::PR_SET_SECCOMP)),
        "traceme" => (libc::SYS_ptrace, Some(libc::PTRACE_TRACEME as i32)),
        _ => return Err(format!("unknown host denial fixture {operation}")),
    };
    let instruction = |code, jt, jf, k| libc::sock_filter { code, jt, jf, k };
    // A separate, deliberately narrow kernel restriction on the *supervisor*.
    // It is not derived from, nor an evaluator of, its creation filter.
    let mut filter = vec![
        instruction(0x20, 0, 0, 0),
        instruction(
            0x15,
            0,
            if argument.is_some() { 3 } else { 1 },
            number as u32,
        ),
    ];
    if let Some(argument) = argument {
        filter.push(instruction(0x20, 0, 0, 16));
        filter.push(instruction(0x15, 0, 1, argument as u32));
    }
    filter.push(instruction(0x06, 0, 0, 0x0005_0000 | libc::EACCES as u32));
    filter.push(instruction(0x06, 0, 0, 0x7fff_0000));
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    if unsafe {
        libc::prctl(
            libc::PR_SET_NO_NEW_PRIVS,
            1_usize,
            0_usize,
            0_usize,
            0_usize,
        )
    } != 0
        || unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                2_usize,
                &program as *const libc::sock_fprog,
                0_usize,
                0_usize,
            )
        } != 0
    {
        return Err(format!(
            "cannot install independent host restriction: {}",
            std::io::Error::last_os_error()
        ));
    }
    Err(format!(
        "cannot exec restricted supervisor: {}",
        Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
            .args(["run", "--policy", policy, "--receipt", receipt])
            .exec()
    ))
}

#[cfg(target_os = "linux")]
fn application_trap_handled_fixture(marker: &Path) -> Result<(), String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static OBSERVED: AtomicU32 = AtomicU32::new(0);
    extern "C" fn handler(signal: libc::c_int) {
        OBSERVED.store(signal as u32, Ordering::Relaxed);
    }
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler as *const () as usize;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
    }
    if unsafe { libc::sigaction(libc::SIGTRAP, &action, std::ptr::null_mut()) } != 0
        || unsafe { libc::raise(libc::SIGTRAP) } != 0
    {
        return Err(format!(
            "application SIGTRAP fixture failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    if OBSERVED.load(Ordering::Relaxed) != libc::SIGTRAP as u32 {
        return Err("application SIGTRAP handler was not invoked".to_owned());
    }
    fs::write(marker, b"trap-handled\n").map_err(|error| error.to_string())
}

#[cfg(windows)]
fn normal_heap_fixture() -> Result<(), String> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Diagnostics::Debug::IsDebuggerPresent;
    use windows_sys::Win32::System::Memory::{
        HeapCompatibilityInformation, HeapCreate, HeapDestroy, HeapQueryInformation,
        HeapSetInformation,
    };

    struct Heap(HANDLE);
    impl Drop for Heap {
        fn drop(&mut self) {
            unsafe { HeapDestroy(self.0) };
        }
    }

    if unsafe { IsDebuggerPresent() } == 0 {
        return Err("normal-heap fixture requires active debugger custody".to_owned());
    }
    let handle = unsafe { HeapCreate(0, 0, 0) };
    if handle.is_null() {
        return Err(format!(
            "HeapCreate failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let heap = Heap(handle);
    let compatibility = 2_u32;
    if unsafe {
        HeapSetInformation(
            heap.0,
            HeapCompatibilityInformation,
            (&compatibility as *const u32).cast(),
            std::mem::size_of_val(&compatibility),
        )
    } == 0
    {
        return Err(format!(
            "cannot enable LFH under debugger custody: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut observed = 0_u32;
    if unsafe {
        HeapQueryInformation(
            heap.0,
            HeapCompatibilityInformation,
            (&mut observed as *mut u32).cast(),
            std::mem::size_of_val(&observed),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(format!(
            "cannot query LFH: {}",
            std::io::Error::last_os_error()
        ));
    }
    if observed != compatibility || unsafe { IsDebuggerPresent() } == 0 {
        return Err("normal LFH and debugger custody must remain active together".to_owned());
    }
    Ok(())
}

#[cfg(windows)]
fn application_breakpoint_fixture() -> Result<u8, String> {
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::DebugBreak();
    }
    Ok(0)
}

#[cfg(not(windows))]
fn application_breakpoint_fixture() -> Result<u8, String> {
    Err("application-breakpoint fixture is Windows-only".to_owned())
}

fn read_policy(path: &Path) -> Result<Vec<u8>, String> {
    molt_proof_supervisor::evidence::read_bounded_file(path, MAX_POLICY_BYTES)
        .map_err(|error| format!("cannot read policy {}: {error}", path.display()))
}

fn verify_receipt(
    policy_path: &Path,
    receipt_path: &Path,
    rootfs: Option<&Path>,
) -> Result<u8, String> {
    let policy_bytes = read_policy(policy_path)?;
    let raw_policy = molt_proof_supervisor::budget::decode_policy(&policy_bytes)?;
    let policy = match rootfs {
        Some(root) => raw_policy.validate_rooted_linux(root)?,
        None => raw_policy.validate()?,
    };
    let opened_receipt = OpenedRegularFile::open(receipt_path)
        .map_err(|error| format!("cannot open receipt {}: {error}", receipt_path.display()))?;
    let receipt_bytes = opened_receipt.size_bytes();
    if receipt_bytes > MAX_RECEIPT_BYTES as u64 {
        println!(
            "{}",
            serde_json::json!({
                "schema_valid": false,
                "receipt_size_valid": false,
                "receipt_bytes": receipt_bytes,
                "maximum_receipt_bytes": MAX_RECEIPT_BYTES,
            })
        );
        return Ok(79);
    }
    let bytes = opened_receipt
        .read_all(MAX_RECEIPT_BYTES)
        .map_err(|error| format!("cannot read receipt {}: {error}", receipt_path.display()))?;
    let receipt_size_valid = true;
    let receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid receipt: {error}"))?;
    let identity_valid = receipt.identity_is_valid();
    let terminal_consistent = receipt.terminal_is_consistent();
    let lifecycle_valid = receipt.lifecycle_is_valid();
    let schema_valid = receipt.schema == RECEIPT_SCHEMA;
    let capability_valid = match rootfs {
        Some(_) => platform::recorded_linux_capability_contract_is_valid(
            &receipt.capability,
            policy.policy.mode,
        ),
        None => platform::capability_contract_is_valid(&receipt.capability, policy.policy.mode),
    };
    let policy_digest_valid = receipt.policy_sha256 == policy.policy_sha256;
    let nonce_digest_valid = receipt.nonce_sha256 == sha256_bytes(policy.policy.nonce.as_bytes());
    let event_verification = receipt
        .event_log
        .as_ref()
        .ok_or_else(|| "terminal receipt has no event log".to_owned())
        .and_then(|event_log| {
            verify_event_artifact(receipt_path, event_log, &policy, &receipt.capability)
        });
    let event_log_valid = event_verification.is_ok();
    let derived_summary_valid = event_verification
        .as_ref()
        .is_ok_and(|verified| verified.derived_images == receipt.derived_image_summary);
    let accounting_valid = event_verification
        .as_ref()
        .is_ok_and(|verified| verified.accounting == receipt.accounting);
    let root_exit_valid = event_verification
        .as_ref()
        .is_ok_and(|verified| verified.root_exit_code == receipt.root_exit_code);
    let violation_replay_valid = event_verification.as_ref().is_ok_and(|verified| {
        verified.violation_count == receipt.violation_count
            && verified.violations == receipt.violations
    });
    let admission_replay_valid = event_verification
        .as_ref()
        .is_ok_and(|verified| verified.admission == receipt.capability.admission);
    let native_custody_valid = receipt.native_custody_is_valid();
    let custody_closed = receipt.native_custody.is_closed();
    let journal_coverage_valid = receipt.coverage_is_valid();
    println!(
        "{}",
        serde_json::json!({
            "receipt_sha256": sha256_bytes(&bytes),
            "receipt_bytes": bytes.len(),
            "policy_input_sha256": sha256_bytes(&policy_bytes),
            "policy_input_bytes": policy_bytes.len(),
            "capability": receipt.capability,
            "schema": receipt.schema,
            "state": receipt.state,
            "complete": receipt.complete,
            "schema_valid": schema_valid,
            "capability_valid": capability_valid,
            "admission_replay_valid": admission_replay_valid,
            "identity_valid": identity_valid,
            "terminal_consistent": terminal_consistent,
            "lifecycle_valid": lifecycle_valid,
            "policy_digest_valid": policy_digest_valid,
            "nonce_digest_valid": nonce_digest_valid,
            "receipt_size_valid": receipt_size_valid,
            "event_log_valid": event_log_valid,
            "derived_summary_valid": derived_summary_valid,
            "accounting_valid": accounting_valid,
            "root_exit_valid": root_exit_valid,
            "violation_replay_valid": violation_replay_valid,
            "native_custody_valid": native_custody_valid,
            "custody_closed": custody_closed,
            "journal_coverage_valid": journal_coverage_valid,
            "event_log_error": event_verification.err(),
        })
    );
    Ok(
        if schema_valid
            && capability_valid
            && admission_replay_valid
            && identity_valid
            && terminal_consistent
            && lifecycle_valid
            && policy_digest_valid
            && nonce_digest_valid
            && receipt_size_valid
            && event_log_valid
            && derived_summary_valid
            && accounting_valid
            && root_exit_valid
            && violation_replay_valid
            && native_custody_valid
            && journal_coverage_valid
        {
            0
        } else {
            79
        },
    )
}

fn parse_mode(value: &str) -> Result<ClosureMode, String> {
    match value {
        "leaf" => Ok(ClosureMode::Leaf),
        "declared-tree" => Ok(ClosureMode::DeclaredTree),
        "inventory-tree" => Ok(ClosureMode::InventoryTree),
        _ => Err("mode must be leaf, declared-tree, or inventory-tree".to_owned()),
    }
}

fn run_policy(policy_path: &Path, receipt_path: &Path, inventory: bool) -> Result<u8, String> {
    let bytes = read_policy(policy_path)?;
    let raw = molt_proof_supervisor::budget::decode_policy(&bytes)?;
    let policy = raw.validate()?;
    if (policy.policy.mode == ClosureMode::InventoryTree) != inventory {
        return Err(if inventory {
            "inventory command requires inventory-tree policy mode".to_owned()
        } else {
            "inventory-tree policy must use the inventory command".to_owned()
        });
    }
    let capability = platform::capability(policy.policy.mode);
    let mut events = EventJournal::create(receipt_path, &policy, &capability)?;
    let mut receipt = platform::run(&policy, &mut events, capability);
    let publication = (|| {
        let evidence = events.publish()?;
        let event_path = receipt_path.with_file_name(&evidence.event_log.file);
        receipt.attach_evidence(evidence).map_err(|error| {
            format!("{error}; published event artifact {}", event_path.display())
        })?;
        write_receipt_atomic(receipt_path, &receipt)
    })();
    if let Err(error) = publication {
        // Preserve the backend's terminal and cleanup diagnostics even when
        // storage publication fails. The existing stderr transcript carries
        // this snapshot and the independent publication cause; exit remains 2.
        return Err(receipt.publication_failure_diagnostic(&error));
    }
    Ok(if receipt.complete { 0 } else { 78 })
}

/// Export exact retained evidence after the supervised tree is terminal.
/// Only the final fixed-width footer at stderr EOF frames this transport;
/// guest stderr before it is ordinary guest output. Verification is unchanged.
fn export_evidence(receipt_path: &Path) -> Result<(), String> {
    let stderr = std::io::stderr();
    export_evidence_to(receipt_path, &mut stderr.lock())
}

fn export_evidence_to(receipt_path: &Path, stream: &mut impl Write) -> Result<(), String> {
    let opened_receipt = OpenedRegularFile::open(receipt_path)
        .map_err(|error| format!("cannot open export receipt: {error}"))?;
    if opened_receipt.size_bytes() > EXPORT_RECEIPT_MAX_BYTES as u64 {
        return Err("export receipt exceeds its protocol bound".to_owned());
    }
    let receipt_bytes = opened_receipt
        .read_all(EXPORT_RECEIPT_MAX_BYTES)
        .map_err(|error| format!("cannot read export receipt: {error}"))?;
    let receipt: Receipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|error| format!("invalid export receipt: {error}"))?;
    if receipt.schema != RECEIPT_SCHEMA || !receipt.identity_is_valid() {
        return Err("export receipt identity is invalid".to_owned());
    }
    let event = receipt
        .event_log
        .as_ref()
        .ok_or_else(|| "export receipt has no event artifact".to_owned())?;
    if event.bytes > EXPORT_EVENT_MAX_BYTES {
        return Err("export event artifact exceeds its protocol bound".to_owned());
    }
    let path = event_artifact_path(receipt_path, &event.sha256)?;
    if path.file_name().and_then(|name| name.to_str()) != Some(event.file.as_str()) {
        return Err("export event artifact is not the deterministic adjacent file".to_owned());
    }
    let opened = OpenedRegularFile::open(&path)
        .map_err(|error| format!("cannot open export event artifact: {error}"))?;
    if opened.size_bytes() != event.bytes {
        return Err("export event artifact type or size changed".to_owned());
    }
    if sha256_reader(&mut opened.bounded_reader()).map_err(|error| error.to_string())?
        != event.sha256
    {
        return Err("export event artifact digest changed".to_owned());
    }
    opened.verify().map_err(|error| error.to_string())?;
    opened.rewind().map_err(|error| error.to_string())?;
    stream
        .write_all(&receipt_bytes)
        .map_err(|error| error.to_string())?;
    let copied = std::io::copy(&mut opened.bounded_reader(), &mut *stream)
        .map_err(|error| format!("cannot export event artifact: {error}"))?;
    if copied != event.bytes {
        return Err("export event artifact changed while streaming".to_owned());
    }
    opened.verify().map_err(|error| error.to_string())?;
    opened_receipt.verify().map_err(|error| error.to_string())?;
    stream
        .write_all(EXPORT_FOOTER_MAGIC.as_bytes())
        .map_err(|error| error.to_string())?;
    writeln!(
        stream,
        "{:0width$x}{:0width$x}",
        receipt_bytes.len(),
        event.bytes,
        width = EXPORT_LENGTH_HEX_DIGITS
    )
    .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

fn write_receipt_atomic(path: &Path, receipt: &Receipt) -> Result<(), String> {
    let mut bytes = molt_proof_supervisor::budget::encode(receipt, MAX_RECEIPT_BYTES - 1)?;
    bytes
        .try_reserve_exact(1)
        .map_err(|_| "receipt LF reservation refused")?;
    bytes.push(b'\n');
    durable_atomic_write(path, &bytes)
}

#[cfg(test)]
mod export_tests {
    use super::*;
    use molt_proof_supervisor::{
        ArtifactSummary, Capability, FixedImage, Policy, RootExitDisposition,
    };

    #[test]
    fn export_refuses_oversized_retained_evidence_before_emitting_a_footer() {
        let directory = std::env::temp_dir().join(format!(
            "molt-supervisor-export-bound-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        struct RemoveDirectory(std::path::PathBuf);
        impl Drop for RemoveDirectory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        fs::create_dir(&directory).unwrap();
        let _cleanup = RemoveDirectory(directory.clone());
        let receipt_path = directory.join("receipt.json");
        fs::write(&receipt_path, vec![b'x'; EXPORT_RECEIPT_MAX_BYTES + 1]).unwrap();
        assert!(
            export_evidence(&receipt_path)
                .unwrap_err()
                .contains("receipt exceeds")
        );
        let image = std::env::current_exe().unwrap();
        let policy = Policy {
            schema: molt_proof_supervisor::POLICY_SCHEMA.to_owned(),
            nonce: "e".repeat(32),
            mode: ClosureMode::Leaf,
            cwd: directory.clone(),
            command: vec![image.to_string_lossy().into_owned()],
            environment: platform::required_environment(),
            root_role: "fixture".to_owned(),
            fixed_images: vec![FixedImage {
                role: "fixture".to_owned(),
                path: image.clone(),
                sha256: molt_proof_supervisor::sha256_file(&image).unwrap(),
                root_exit_disposition: RootExitDisposition::RequireExit,
            }],
            derived_roots: vec![],
        }
        .validate()
        .unwrap();
        let capability = Capability {
            schema: molt_proof_supervisor::CAPABILITY_SCHEMA.to_owned(),
            platform: "test".to_owned(),
            mode: ClosureMode::Leaf,
            backend: "test".to_owned(),
            admission: molt_proof_supervisor::Admission::Ineligible {
                reason: "fixture".to_owned(),
            },
            pre_entry_exec_authority: false,
            pre_entry_process_create_authority: false,
            recursive_descendant_authority: false,
            required_environment: platform::required_environment(),
        };
        let mut receipt = Receipt::rejected(&policy, &capability, "fixture");
        receipt.event_log = Some(ArtifactSummary {
            schema: molt_proof_supervisor::evidence::EVENT_LOG_SCHEMA.to_owned(),
            file: "not-opened.jsonl".to_owned(),
            count: 1,
            bytes: EXPORT_EVENT_MAX_BYTES + 1,
            sha256: "0".repeat(64),
        });
        receipt.seal();
        fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        assert!(
            export_evidence(&receipt_path)
                .unwrap_err()
                .contains("event artifact exceeds")
        );

        // Transport accepts sealed bytes; event semantics belong to verify.
        let event_bytes = b"sealed event bytes\n";
        let digest = sha256_bytes(event_bytes);
        let event_path = event_artifact_path(&receipt_path, &digest).unwrap();
        fs::write(&event_path, event_bytes).unwrap();
        receipt.event_log = Some(ArtifactSummary {
            schema: molt_proof_supervisor::evidence::EVENT_LOG_SCHEMA.to_owned(),
            file: event_path.file_name().unwrap().to_str().unwrap().to_owned(),
            count: 1,
            bytes: event_bytes.len() as u64,
            sha256: digest,
        });
        receipt.seal();
        fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        let mut accepted = Vec::new();
        export_evidence_to(&receipt_path, &mut accepted).unwrap();
        assert!(
            accepted
                .windows(EXPORT_FOOTER_MAGIC.len())
                .any(|row| row == EXPORT_FOOTER_MAGIC.as_bytes())
        );
        #[cfg(unix)]
        for path in [&receipt_path, &event_path] {
            let saved = path.with_extension("saved");
            fs::rename(path, &saved).unwrap();
            std::os::unix::fs::symlink(&saved, path).unwrap();
            let mut refused = Vec::new();
            assert!(
                export_evidence_to(&receipt_path, &mut refused)
                    .unwrap_err()
                    .contains("direct regular file")
            );
            assert!(
                refused.is_empty(),
                "indirection refusal must precede any export bytes"
            );
            fs::remove_file(path).unwrap();
            fs::rename(&saved, path).unwrap();
        }
        struct MutatingSink {
            event_path: std::path::PathBuf,
            bytes: Vec<u8>,
            mutated: bool,
        }
        impl Write for MutatingSink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if !self.mutated {
                    // Same extent changes between digest and streaming, after
                    // receipt bytes are emitted but before a footer is possible.
                    fs::write(&self.event_path, b"changed event data\n")?;
                    self.mutated = true;
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(event_bytes.len(), b"changed event data\n".len());
        let mut sink = MutatingSink {
            event_path,
            bytes: Vec::new(),
            mutated: false,
        };
        assert!(export_evidence_to(&receipt_path, &mut sink).is_err());
        assert!(sink.mutated);
        assert!(
            !sink
                .bytes
                .windows(EXPORT_FOOTER_MAGIC.len())
                .any(|row| row == EXPORT_FOOTER_MAGIC.as_bytes())
        );
    }
}
