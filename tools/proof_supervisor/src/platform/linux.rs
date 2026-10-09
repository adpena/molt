use crate::{
    CAPABILITY_SCHEMA, Capability, ClosureMode, EventJournal, FileIdentity, ImageCacheKey,
    ImageHashCache, KernelAccounting, ProcessEventKind, Receipt, ValidatedPolicy,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::ptr::null_mut;

pub fn capability(mode: ClosureMode) -> Capability {
    let (available, reason) = ptrace_availability();
    Capability {
        schema: CAPABILITY_SCHEMA.to_owned(),
        platform: "linux".to_owned(),
        mode,
        backend: "ptrace-exitkill".to_owned(),
        available,
        pre_entry_exec_authority: true,
        pre_entry_process_create_authority: true,
        recursive_descendant_authority: true,
        required_environment: super::required_environment(),
        reason,
    }
}

pub fn run(policy: &ValidatedPolicy, events: &mut EventJournal, capability: Capability) -> Receipt {
    super::run_backend(policy, events, capability, |policy, events| unsafe {
        supervise(policy, events)
    })
}

fn ptrace_availability() -> (bool, Option<String>) {
    let scope = match std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope") {
        Ok(value) => match value.trim().parse::<u8>() {
            Ok(scope) => Some(scope),
            Err(error) => {
                return (
                    false,
                    Some(format!("cannot parse Yama ptrace_scope: {error}")),
                );
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return (
                false,
                Some(format!("cannot read Yama ptrace_scope: {error}")),
            );
        }
    };
    match scope {
        None | Some(0 | 1) => (true, None),
        Some(2) if effective_cap_sys_ptrace() => (true, None),
        Some(2) => (
            false,
            Some("Yama ptrace_scope=2 requires effective CAP_SYS_PTRACE".to_owned()),
        ),
        Some(3) => (false, Some("Yama ptrace_scope=3 forbids ptrace".to_owned())),
        Some(value) => (
            false,
            Some(format!("unsupported Yama ptrace_scope value {value}")),
        ),
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
) -> Result<Option<KernelAccounting>, String> {
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

    let mut process_generation = 1_u64;
    let root_u32 = root as u32;
    let mut traced = BTreeSet::from([root]);
    let mut processes = BTreeSet::from([root]);
    let root_stable_id = stable_id(root, process_generation);
    let mut stable_ids = BTreeMap::from([(root, root_stable_id.clone())]);
    let mut images: BTreeMap<libc::pid_t, FileIdentity> = BTreeMap::new();
    let mut hash_cache = ImageHashCache::default();
    if let Err(error) = events.record(
        root_u32,
        root_stable_id,
        ProcessEventKind::ProcessCreate {
            parent_process_id: None,
            image: None,
        },
    ) {
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
                let code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status) as i64
                } else {
                    (128 + libc::WTERMSIG(status)) as i64
                };
                let outcome = events.record(
                    pid as u32,
                    stable_ids
                        .remove(&pid)
                        .unwrap_or_else(|| format!("linux:{pid}:unclassified")),
                    ProcessEventKind::ProcessExit { exit_code: code },
                )?;
                images.remove(&pid);
                if outcome.must_terminate_closure() {
                    violated |= outcome.has_policy_violation();
                    terminate_tracees(&traced, root);
                }
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
                process_generation = process_generation
                    .checked_add(1)
                    .ok_or_else(|| "process generation overflow".to_owned())?;
                // ptrace identifies the creating task; the process ledger owns
                // thread groups. A worker thread's fork inherits its process image.
                let parent_process = thread_group_id(pid)
                    .filter(|parent| processes.contains(parent))
                    .ok_or_else(|| "clone event has no live process owner".to_owned())?;
                let process = match classify_clone_event(event, child, thread_group_id(child)) {
                    Ok(process) => process,
                    Err(reason) => {
                        let outcome = events.record(
                            child as u32,
                            stable_id(child, process_generation),
                            ProcessEventKind::CloneUnclassified {
                                parent_process_id: parent_process as u32,
                                reason,
                            },
                        )?;
                        violated |= outcome.has_policy_violation();
                        false
                    }
                };
                if process && !violated {
                    processes.insert(child);
                    let inherited = images.get(&parent_process).cloned();
                    if let Some(image) = &inherited {
                        images.insert(child, image.clone());
                    }
                    let child_stable_id = stable_id(child, process_generation);
                    stable_ids.insert(child, child_stable_id.clone());
                    let outcome = events.record(
                        child as u32,
                        child_stable_id,
                        ProcessEventKind::Fork {
                            parent_process_id: parent_process as u32,
                            image: inherited,
                        },
                    )?;
                    violated |= outcome.has_policy_violation();
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
                let image = match proc_image_identity(policy, pid, &mut hash_cache) {
                    Ok(image) => image,
                    Err(error) => {
                        terminate_tracees(&traced, root);
                        return Err(error);
                    }
                };
                images.insert(pid, image.clone());
                let outcome = events.record(
                    pid as u32,
                    stable_ids
                        .get(&pid)
                        .cloned()
                        .unwrap_or_else(|| format!("linux:{pid}:unclassified")),
                    ProcessEventKind::Exec { image },
                )?;
                violated |= outcome.has_policy_violation();
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
    if !traced.is_empty() {
        return Err("trace set was not fully drained".to_owned());
    }
    Ok(None)
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

pub(super) fn linux_cache_key(metadata: &std::fs::Metadata) -> ImageCacheKey {
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
