use crate::{
    Capability, ClosureMode, EventJournal, EventKind, FileIdentity, ImageCacheKey, ImageClass,
    ImageHashCache, ProcessEvent, Receipt, SupervisorState, ValidatedPolicy,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::time::Instant;

pub fn capability(mode: ClosureMode) -> Capability {
    Capability {
        schema: "molt.proof-supervisor-capability.v1".to_owned(),
        platform: "linux".to_owned(),
        mode,
        backend: "ptrace-exitkill".to_owned(),
        available: ptrace_scope_allows_children(),
        pre_entry_exec_authority: true,
        recursive_descendant_authority: true,
        reason: (!ptrace_scope_allows_children())
            .then(|| "ptrace of direct children is disabled by host policy".to_owned()),
    }
}

pub fn run(policy: &ValidatedPolicy, events: &mut EventJournal) -> Receipt {
    let started = Instant::now();
    let cap = capability(policy.policy.mode);
    if !cap.available {
        return Receipt::rejected(policy, &cap, cap.reason.clone().unwrap());
    }
    let mut receipt = Receipt::running(policy, &cap);
    match unsafe { supervise(policy, &mut receipt, events) } {
        Ok(()) => receipt
            .transition(SupervisorState::Draining)
            .expect("valid drain transition"),
        Err(error) => receipt.record_error(error),
    }
    if receipt.accounting.root_execs == 0 {
        receipt.record_error("root executable never reached an admitted exec stop");
    }
    receipt.elapsed_ns = started.elapsed().as_nanos();
    let complete = receipt.errors.is_empty()
        && receipt.violations.is_empty()
        && receipt.accounting.active_processes == 0
        && receipt.accounting.root_execs >= 1
        && receipt.accounting.observed_process_creates == receipt.accounting.observed_process_exits
        && receipt.accounting.total_processes == receipt.accounting.observed_process_creates;
    receipt.finish(complete);
    receipt
}

fn ptrace_scope_allows_children() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope")
        .map(|value| value.trim() != "3")
        .unwrap_or(true)
}

unsafe fn supervise(
    policy: &ValidatedPolicy,
    receipt: &mut Receipt,
    events: &mut EventJournal,
) -> Result<(), String> {
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

    let root = unsafe { libc::fork() };
    if root < 0 {
        return Err(os_error("fork"));
    }
    if root == 0 {
        unsafe {
            libc::setpgid(0, 0);
            if libc::ptrace(
                libc::PTRACE_TRACEME,
                0,
                null_mut::<libc::c_void>(),
                null_mut::<libc::c_void>(),
            ) < 0
            {
                libc::_exit(124);
            }
            if libc::chdir(cwd.as_ptr()) != 0 {
                libc::_exit(123);
            }
            libc::raise(libc::SIGSTOP);
            libc::execve(executable, argv_ptrs.as_ptr(), envp_ptrs.as_ptr());
            libc::_exit(127);
        }
    }

    let mut initial = 0;
    if unsafe { libc::waitpid(root, &mut initial, 0) } != root || !libc::WIFSTOPPED(initial) {
        unsafe {
            libc::kill(root, libc::SIGKILL);
        }
        return Err("traced root did not enter its pre-exec stop".to_owned());
    }
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
        unsafe {
            libc::kill(root, libc::SIGKILL);
        }
        return Err(os_error("ptrace(PTRACE_SETOPTIONS)"));
    }

    let mut sequence = 1_u64;
    let root_u32 = root as u32;
    let mut traced = BTreeSet::from([root]);
    let mut processes = BTreeSet::from([root]);
    let mut parents = BTreeMap::from([(root, None)]);
    let root_stable_id = stable_id(root, sequence);
    let mut stable_ids = BTreeMap::from([(root, root_stable_id.clone())]);
    let mut images: BTreeMap<libc::pid_t, FileIdentity> = BTreeMap::new();
    let mut hash_cache = ImageHashCache::default();
    receipt.accounting.total_processes = 1;
    receipt.accounting.observed_process_creates = 1;
    receipt.accounting.active_processes = 1;
    if let Err(error) = events.record(&ProcessEvent {
        sequence,
        kind: EventKind::ProcessCreate,
        process_id: root_u32,
        parent_process_id: None,
        stable_process_id: root_stable_id,
        image: None,
        exit_code: None,
    }) {
        unsafe {
            libc::kill(root, libc::SIGKILL);
        }
        return Err(error);
    }
    if unsafe {
        libc::ptrace(
            libc::PTRACE_CONT,
            root,
            null_mut::<libc::c_void>(),
            null_mut::<libc::c_void>(),
        )
    } < 0
    {
        unsafe {
            libc::kill(root, libc::SIGKILL);
        }
        return Err(os_error("ptrace(PTRACE_CONT root)"));
    }

    let mut violated = false;
    while !traced.is_empty() {
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::__WALL) };
        if pid < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                break;
            }
            terminate_tracees(&traced, root);
            return Err(format!("waitpid(__WALL) failed: {error}"));
        }
        if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
            traced.remove(&pid);
            if processes.remove(&pid) {
                sequence += 1;
                receipt.accounting.observed_process_exits += 1;
                receipt.accounting.active_processes = processes.len() as u64;
                let code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status) as i64
                } else {
                    (128 + libc::WTERMSIG(status)) as i64
                };
                if pid == root {
                    receipt.root_exit_code = Some(code);
                }
                events.record(&ProcessEvent {
                    sequence,
                    kind: EventKind::ProcessExit,
                    process_id: pid as u32,
                    parent_process_id: parents
                        .get(&pid)
                        .copied()
                        .flatten()
                        .map(|value| value as u32),
                    stable_process_id: stable_ids
                        .remove(&pid)
                        .unwrap_or_else(|| format!("linux:{pid}:unclassified")),
                    image: None,
                    exit_code: Some(code),
                })?;
            }
            continue;
        }
        if !libc::WIFSTOPPED(status) {
            continue;
        }
        traced.insert(pid);
        let signal = libc::WSTOPSIG(status);
        let event = (status as u32) >> 16;
        let mut deliver = if signal == libc::SIGTRAP || signal == libc::SIGSTOP {
            0
        } else {
            signal
        };
        match event as libc::c_int {
            libc::PTRACE_EVENT_FORK | libc::PTRACE_EVENT_VFORK | libc::PTRACE_EVENT_CLONE => {
                let mut child = 0_usize;
                if unsafe {
                    libc::ptrace(
                        libc::PTRACE_GETEVENTMSG,
                        pid,
                        null_mut::<libc::c_void>(),
                        &mut child as *mut _ as *mut libc::c_void,
                    )
                } < 0
                {
                    terminate_tracees(&traced, root);
                    return Err(os_error("ptrace(PTRACE_GETEVENTMSG)"));
                }
                let child = child as libc::pid_t;
                traced.insert(child);
                sequence += 1;
                let process = match classify_clone_event(event, child, thread_group_id(child)) {
                    Ok(process) => process,
                    Err(reason) => {
                        events.record(&ProcessEvent {
                            sequence,
                            kind: EventKind::CloneUnclassified,
                            process_id: child as u32,
                            parent_process_id: Some(pid as u32),
                            stable_process_id: stable_id(child, sequence),
                            image: None,
                            exit_code: None,
                        })?;
                        receipt.record_violation(reason);
                        violated = true;
                        false
                    }
                };
                if process && !violated {
                    processes.insert(child);
                    parents.insert(child, Some(pid));
                    receipt.accounting.total_processes += 1;
                    receipt.accounting.observed_process_creates += 1;
                    receipt.accounting.active_processes = processes.len() as u64;
                    let inherited = images.get(&pid).cloned();
                    if let Some(image) = &inherited {
                        images.insert(child, image.clone());
                    }
                    let child_stable_id = stable_id(child, sequence);
                    stable_ids.insert(child, child_stable_id.clone());
                    events.record(&ProcessEvent {
                        sequence,
                        kind: EventKind::Fork,
                        process_id: child as u32,
                        parent_process_id: Some(pid as u32),
                        stable_process_id: child_stable_id,
                        image: inherited,
                        exit_code: None,
                    })?;
                    if policy.policy.mode == ClosureMode::Leaf {
                        receipt.record_violation(format!(
                            "leaf closure observed descendant process {child}"
                        ));
                        violated = true;
                    }
                } else if !violated {
                    events.record(&ProcessEvent {
                        sequence,
                        kind: EventKind::ThreadCreate,
                        process_id: child as u32,
                        parent_process_id: Some(pid as u32),
                        stable_process_id: stable_id(child, sequence),
                        image: None,
                        exit_code: None,
                    })?;
                }
            }
            libc::PTRACE_EVENT_EXEC => {
                let mut former_tid = 0_usize;
                if unsafe {
                    libc::ptrace(
                        libc::PTRACE_GETEVENTMSG,
                        pid,
                        null_mut::<libc::c_void>(),
                        &mut former_tid as *mut _ as *mut libc::c_void,
                    )
                } < 0
                {
                    terminate_tracees(&traced, root);
                    return Err(os_error("ptrace(PTRACE_GETEVENTMSG exec)"));
                }
                let former_tid = former_tid as libc::pid_t;
                reconcile_exec_tid(&mut traced, &mut images, former_tid, pid);
                sequence += 1;
                let image = match proc_image_identity(policy, pid, &mut hash_cache) {
                    Ok(image) => image,
                    Err(error) => {
                        terminate_tracees(&traced, root);
                        return Err(error);
                    }
                };
                let mut reason = None;
                if image.class == ImageClass::Unknown {
                    reason = Some(format!(
                        "unadmitted executable image {} in process {pid}",
                        image.path.display()
                    ));
                }
                images.insert(pid, image.clone());
                receipt.accounting.observed_execs += 1;
                if pid == root {
                    receipt.accounting.root_execs += 1;
                }
                events.record(&ProcessEvent {
                    sequence,
                    kind: EventKind::Exec,
                    process_id: pid as u32,
                    parent_process_id: parents
                        .get(&pid)
                        .copied()
                        .flatten()
                        .map(|value| value as u32),
                    stable_process_id: stable_ids
                        .get(&pid)
                        .cloned()
                        .unwrap_or_else(|| format!("linux:{pid}:unclassified")),
                    image: Some(image),
                    exit_code: None,
                })?;
                if let Some(reason) = reason {
                    receipt.record_violation(reason);
                    violated = true;
                }
            }
            _ => {}
        }
        if violated {
            terminate_tracees(&traced, root);
            deliver = 0;
        }
        if unsafe {
            libc::ptrace(
                libc::PTRACE_CONT,
                pid,
                null_mut::<libc::c_void>(),
                deliver as usize as *mut libc::c_void,
            )
        } < 0
        {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                terminate_tracees(&traced, root);
                return Err(format!("ptrace(PTRACE_CONT) failed: {error}"));
            }
        }
    }
    receipt.accounting.active_processes = processes.len() as u64;
    if !traced.is_empty() {
        receipt.record_error("trace set was not fully drained");
    }
    Ok(())
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
    let cache_key = linux_cache_key(&metadata);
    let file_id = cache_key.stable_file_id().to_owned();
    let mut reader = file;
    let sha256 = hash_cache
        .digest(&cache_key, &mut reader, |file| {
            file.metadata().map(|metadata| linux_cache_key(&metadata))
        })
        .map_err(|error| format!("cannot hash executable: {error}"))?;
    Ok(policy.classify_path(&path, file_id, metadata.size(), sha256))
}

fn linux_cache_key(metadata: &std::fs::Metadata) -> ImageCacheKey {
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

fn classify_clone_event(
    event: u32,
    child: libc::pid_t,
    thread_group_id: Option<libc::pid_t>,
) -> Result<bool, String> {
    if event != libc::PTRACE_EVENT_CLONE as u32 {
        return Ok(true);
    }
    thread_group_id.map(|tgid| tgid == child).ok_or_else(|| {
        format!(
            "cannot classify PTRACE_EVENT_CLONE child {child}: /proc thread-group identity unavailable"
        )
    })
}

fn reconcile_exec_tid(
    traced: &mut BTreeSet<libc::pid_t>,
    images: &mut BTreeMap<libc::pid_t, FileIdentity>,
    former_tid: libc::pid_t,
    current_pid: libc::pid_t,
) {
    if former_tid != 0 && former_tid != current_pid {
        traced.remove(&former_tid);
        images.remove(&former_tid);
    }
}

fn thread_group_id(pid: libc::pid_t) -> Option<libc::pid_t> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("Tgid:")
            .and_then(|value| value.trim().parse().ok())
    })
}

fn terminate_tracees(tracees: &BTreeSet<libc::pid_t>, root: libc::pid_t) {
    unsafe {
        libc::kill(-root, libc::SIGKILL);
    }
    for pid in tracees {
        unsafe {
            libc::kill(*pid, libc::SIGKILL);
        }
    }
}

fn stable_id(pid: libc::pid_t, sequence: u64) -> String {
    let start = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|value| value.rsplit(')').next().map(str::to_owned))
        .and_then(|tail| tail.split_whitespace().nth(19).map(str::to_owned))
        .unwrap_or_else(|| sequence.to_string());
    format!("linux:{pid}:{start}")
}

fn os_error(operation: &str) -> String {
    format!("{operation} failed: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ptrace_clone_with_unknown_thread_group_fails_closed() {
        let error = classify_clone_event(libc::PTRACE_EVENT_CLONE as u32, 41, None).unwrap_err();
        assert!(error.contains("cannot classify PTRACE_EVENT_CLONE"));
    }

    #[test]
    fn ptrace_clone_distinguishes_processes_from_threads() {
        assert!(classify_clone_event(libc::PTRACE_EVENT_CLONE as u32, 41, Some(41)).unwrap());
        assert!(!classify_clone_event(libc::PTRACE_EVENT_CLONE as u32, 41, Some(40)).unwrap());
        assert!(classify_clone_event(libc::PTRACE_EVENT_FORK as u32, 41, None).unwrap());
    }

    #[test]
    fn nonleader_exec_removes_the_obsolete_thread_identity() {
        let mut traced = BTreeSet::from([40, 41]);
        let mut images = BTreeMap::new();
        reconcile_exec_tid(&mut traced, &mut images, 41, 40);
        assert_eq!(traced, BTreeSet::from([40]));
    }
}
