use crate::{
    Capability, ClosureMode, EventJournal, EventKind, FileIdentity, ImageCacheKey, ImageClass,
    ImageHashCache, ProcessEvent, Receipt, RootExitDisposition, SupervisorState, ValidatedPolicy,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::time::Instant;
use windows_sys::Win32::Foundation::{
    CloseHandle, DBG_CONTINUE, DBG_EXCEPTION_NOT_HANDLED, EXCEPTION_BREAKPOINT, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_NAME_NORMALIZED, FileBasicInfo,
    GetFileInformationByHandle, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
    VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CREATE_PROCESS_DEBUG_EVENT, CREATE_THREAD_DEBUG_EVENT, ContinueDebugEvent, DEBUG_EVENT,
    EXCEPTION_DEBUG_EVENT, EXIT_PROCESS_DEBUG_EVENT, LOAD_DLL_DEBUG_EVENT, WaitForDebugEvent,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_ASSOCIATE_COMPLETION_PORT,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectAssociateCompletionPortInformation, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::SystemServices::{
    JOB_OBJECT_MSG_EXIT_PROCESS, JOB_OBJECT_MSG_NEW_PROCESS,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DEBUG_PROCESS,
    PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, WaitForSingleObject,
};

const COMPLETION_KEY: usize = 0x4d4f_4c54;
// The proof queue/memory guard owns wall-clock timeout. An idle debugger wait
// is not a timeout signal: valid proof commands can execute without emitting a
// process event for arbitrarily long periods.
const DEBUG_EVENT_WAIT_MS: u32 = u32::MAX;

pub fn capability(mode: ClosureMode) -> Capability {
    Capability {
        schema: "molt.proof-supervisor-capability.v1".to_owned(),
        platform: "windows".to_owned(),
        mode,
        backend: "debug-process+nested-job".to_owned(),
        available: true,
        pre_entry_exec_authority: true,
        recursive_descendant_authority: true,
        reason: None,
    }
}

struct Handles {
    job: HANDLE,
    port: HANDLE,
    process: HANDLE,
    thread: HANDLE,
}

impl Drop for Handles {
    fn drop(&mut self) {
        unsafe {
            if !self.thread.is_null() {
                CloseHandle(self.thread);
            }
            if !self.process.is_null() {
                CloseHandle(self.process);
            }
            if !self.job.is_null() {
                CloseHandle(self.job);
            }
            if !self.port.is_null() {
                CloseHandle(self.port);
            }
        }
    }
}

pub fn run(policy: &ValidatedPolicy, events: &mut EventJournal) -> Receipt {
    let started = Instant::now();
    let cap = capability(policy.policy.mode);
    let mut receipt = Receipt::running(policy, &cap);
    match unsafe { supervise(policy, &mut receipt, events) } {
        Ok(()) => receipt
            .transition(SupervisorState::Draining)
            .expect("valid drain transition"),
        Err(error) => receipt.record_error(error),
    }
    if receipt.accounting.root_execs == 0 {
        receipt.record_error("root executable never reached an admitted image event");
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

unsafe fn supervise(
    policy: &ValidatedPolicy,
    receipt: &mut Receipt,
    events: &mut EventJournal,
) -> Result<(), String> {
    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
        return Err(last_error("CreateJobObjectW"));
    }
    let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, null_mut(), 0, 1) };
    if port.is_null() {
        unsafe {
            CloseHandle(job);
        }
        return Err(last_error("CreateIoCompletionPort"));
    }
    let mut handles = Handles {
        job,
        port,
        process: null_mut(),
        thread: null_mut(),
    };

    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags =
        windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(last_error("SetInformationJobObject(limits)"));
    }
    let association = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
        CompletionKey: COMPLETION_KEY as _,
        CompletionPort: port,
    };
    if unsafe {
        SetInformationJobObject(
            job,
            JobObjectAssociateCompletionPortInformation,
            &association as *const _ as _,
            size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
        )
    } == 0
    {
        return Err(last_error("SetInformationJobObject(completion port)"));
    }

    let application = wide_nul(OsStr::new(&policy.policy.command[0]));
    let mut command = wide_nul(OsStr::new(&quote_command_line(&policy.policy.command)));
    let cwd = wide_nul(policy.policy.cwd.as_os_str());
    let environment = environment_block(&policy.policy.environment);
    let environment_ptr = environment.as_ptr().cast();
    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let flags = DEBUG_PROCESS | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT;
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            flags,
            environment_ptr,
            cwd.as_ptr(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(last_error("CreateProcessW"));
    }
    handles.process = process.hProcess;
    handles.thread = process.hThread;
    if unsafe { AssignProcessToJobObject(job, process.hProcess) } == 0 {
        unsafe {
            TerminateJobObject(job, 126);
        }
        return Err(last_error("AssignProcessToJobObject"));
    }
    // DEBUG_PROCESS holds the initial thread at CREATE_PROCESS_DEBUG_EVENT
    // before user code. CREATE_SUSPENDED exists only to close the launch/job
    // assignment race, so release that suspension after assignment.
    if unsafe { ResumeThread(process.hThread) } == u32::MAX {
        unsafe {
            TerminateJobObject(job, 125);
        }
        return Err(last_error("ResumeThread"));
    }

    let root_pid = process.dwProcessId;
    let mut active = BTreeSet::new();
    let mut stable_ids = BTreeMap::new();
    let mut pending_initial_breakpoints = BTreeSet::new();
    let mut terminate_after_root = BTreeSet::new();
    let mut hash_cache = ImageHashCache::default();
    let mut sequence = 0_u64;
    let mut violated = false;
    loop {
        let mut event: DEBUG_EVENT = unsafe { zeroed() };
        if unsafe { WaitForDebugEvent(&mut event, DEBUG_EVENT_WAIT_MS) } == 0 {
            unsafe {
                TerminateJobObject(job, 125);
            }
            return Err(last_error("WaitForDebugEvent"));
        }
        sequence += 1;
        let pid = event.dwProcessId;
        let mut continue_status = DBG_CONTINUE;
        match event.dwDebugEventCode {
            CREATE_PROCESS_DEBUG_EVENT => {
                let info = unsafe { event.u.CreateProcessInfo };
                active.insert(pid);
                pending_initial_breakpoints.insert(pid);
                let stable_process_id = format!("windows:{pid}:{sequence}");
                stable_ids.insert(pid, stable_process_id.clone());
                receipt.accounting.observed_process_creates += 1;
                let parent = parent_process_id(pid).filter(|candidate| active.contains(candidate));
                let image = image_identity(policy, info.hFile, &mut hash_cache)?;
                if policy.root_exit_disposition(&image.path) == RootExitDisposition::Terminate {
                    terminate_after_root.insert(pid);
                }
                let mut reason = None;
                if policy.policy.mode == ClosureMode::Leaf && pid != root_pid {
                    reason = Some(format!("leaf closure observed descendant process {pid}"));
                } else if image.class == ImageClass::Unknown
                    && policy.policy.mode != ClosureMode::InventoryTree
                {
                    reason = Some(format!(
                        "unadmitted executable image {} in process {pid}",
                        image.path.display()
                    ));
                }
                receipt.accounting.observed_execs += 1;
                if pid == root_pid {
                    receipt.accounting.root_execs += 1;
                }
                events.record(&ProcessEvent {
                    sequence,
                    kind: EventKind::ProcessCreate,
                    process_id: pid,
                    parent_process_id: parent,
                    stable_process_id,
                    image: Some(image),
                    exit_code: None,
                })?;
                if let Some(reason) = reason {
                    receipt.record_violation(reason);
                    violated = true;
                    unsafe {
                        TerminateJobObject(job, 126);
                    }
                }
                if !info.hProcess.is_null() {
                    unsafe {
                        CloseHandle(info.hProcess);
                    }
                }
                if !info.hThread.is_null() {
                    unsafe {
                        CloseHandle(info.hThread);
                    }
                }
            }
            EXIT_PROCESS_DEBUG_EVENT => {
                let exit_code = unsafe { event.u.ExitProcess.dwExitCode } as i64;
                active.remove(&pid);
                terminate_after_root.remove(&pid);
                pending_initial_breakpoints.remove(&pid);
                receipt.accounting.observed_process_exits += 1;
                if pid == root_pid {
                    receipt.root_exit_code = Some(exit_code);
                }
                events.record(&ProcessEvent {
                    sequence,
                    kind: EventKind::ProcessExit,
                    process_id: pid,
                    parent_process_id: None,
                    stable_process_id: stable_ids
                        .remove(&pid)
                        .unwrap_or_else(|| format!("windows:{pid}:unclassified")),
                    image: None,
                    exit_code: Some(exit_code),
                })?;
                if pid == root_pid && !active.is_empty() {
                    let remaining = active.len() as u64;
                    if active
                        .iter()
                        .all(|child| terminate_after_root.contains(child))
                    {
                        receipt.accounting.root_exit_terminated_processes += remaining;
                        unsafe {
                            TerminateJobObject(job, 0);
                        }
                    } else {
                        receipt.record_violation(format!(
                            "root exited before {} non-auxiliary descendant process(es)",
                            active
                                .iter()
                                .filter(|child| !terminate_after_root.contains(child))
                                .count()
                        ));
                        violated = true;
                        unsafe {
                            TerminateJobObject(job, 126);
                        }
                    }
                }
            }
            LOAD_DLL_DEBUG_EVENT => {
                let file = unsafe { event.u.LoadDll.hFile };
                if !file.is_null() {
                    unsafe {
                        CloseHandle(file);
                    }
                }
            }
            CREATE_THREAD_DEBUG_EVENT => {
                let info = unsafe { event.u.CreateThread };
                events.record(&ProcessEvent {
                    sequence,
                    kind: EventKind::ThreadCreate,
                    process_id: pid,
                    parent_process_id: None,
                    stable_process_id: format!("windows-thread:{pid}:{}", event.dwThreadId),
                    image: None,
                    exit_code: None,
                })?;
                if !info.hThread.is_null() {
                    unsafe {
                        CloseHandle(info.hThread);
                    }
                }
            }
            EXCEPTION_DEBUG_EVENT => {
                let code = unsafe { event.u.Exception.ExceptionRecord.ExceptionCode };
                let initial_loader_breakpoint =
                    code == EXCEPTION_BREAKPOINT && pending_initial_breakpoints.remove(&pid);
                if !initial_loader_breakpoint {
                    continue_status = DBG_EXCEPTION_NOT_HANDLED;
                }
            }
            _ => {}
        }
        if unsafe { ContinueDebugEvent(pid, event.dwThreadId, continue_status) } == 0 {
            unsafe {
                TerminateJobObject(job, 125);
            }
            return Err(last_error("ContinueDebugEvent"));
        }
        if active.is_empty() && (receipt.root_exit_code.is_some() || violated) {
            break;
        }
    }

    if unsafe { WaitForSingleObject(process.hProcess, 5_000) } != WAIT_OBJECT_0 {
        return Err(last_error("WaitForSingleObject(root process drain)"));
    }
    // Debug events establish closure. Job accounting is a bounded independent
    // reconciliation because job completion-port delivery is documented as
    // best-effort. It is never used to discover an executable image.
    let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
    for _ in 0..100 {
        if unsafe {
            QueryInformationJobObject(
                job,
                JobObjectBasicAccountingInformation,
                &mut accounting as *mut _ as _,
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        } == 0
        {
            return Err(last_error("QueryInformationJobObject(accounting)"));
        }
        if accounting.ActiveProcesses == 0 {
            break;
        }
        std::thread::yield_now();
    }
    receipt.accounting.total_processes = accounting.TotalProcesses as u64;
    receipt.accounting.active_processes = accounting.ActiveProcesses as u64;
    let (new_processes, exits) = drain_completion_port(port);
    receipt.accounting.completion_port_new_processes = Some(new_processes);
    receipt.accounting.completion_port_exits = Some(exits);
    Ok(())
}

fn image_identity(
    policy: &ValidatedPolicy,
    handle: HANDLE,
    hash_cache: &mut ImageHashCache,
) -> Result<FileIdentity, String> {
    if handle.is_null() {
        return Err("CREATE_PROCESS_DEBUG_EVENT did not provide an image handle".to_owned());
    }
    let mut file = unsafe { File::from_raw_handle(handle as _) };
    let (information, cache_key) = windows_cache_key(&file)?;
    let path = path_from_handle(handle)?;
    let size = ((information.nFileSizeHigh as u64) << 32) | information.nFileSizeLow as u64;
    let file_id = cache_key.stable_file_id().to_owned();
    let digest = hash_cache
        .digest(&cache_key, &mut file, |file| {
            windows_cache_key(file)
                .map(|(_, key)| key)
                .map_err(io::Error::other)
        })
        .map_err(|error| format!("cannot hash executable image: {error}"))?;
    Ok(policy.classify_path(&path, file_id, size, digest))
}

fn windows_cache_key(file: &File) -> Result<(BY_HANDLE_FILE_INFORMATION, ImageCacheKey), String> {
    let handle = file.as_raw_handle() as HANDLE;
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == 0 {
        return Err(last_error("GetFileInformationByHandle"));
    }
    let mut basic: FILE_BASIC_INFO = unsafe { zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            &mut basic as *mut _ as _,
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(last_error("GetFileInformationByHandleEx(FileBasicInfo)"));
    }
    let size = ((information.nFileSizeHigh as u64) << 32) | information.nFileSizeLow as u64;
    Ok((
        information,
        ImageCacheKey::new(
            format!(
                "{:08x}:{:08x}{:08x}",
                information.dwVolumeSerialNumber,
                information.nFileIndexHigh,
                information.nFileIndexLow
            ),
            format!("{size}:{}:{}", basic.LastWriteTime, basic.ChangeTime),
        ),
    ))
}

fn path_from_handle(handle: HANDLE) -> Result<PathBuf, String> {
    let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
    let required = unsafe { GetFinalPathNameByHandleW(handle, null_mut(), 0, flags) };
    if required == 0 {
        return Err(last_error("GetFinalPathNameByHandleW(size)"));
    }
    let mut buffer = vec![0_u16; required as usize + 1];
    let count = unsafe {
        GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, flags)
    };
    if count == 0 || count as usize >= buffer.len() {
        return Err(last_error("GetFinalPathNameByHandleW"));
    }
    let mut value = String::from_utf16_lossy(&buffer[..count as usize]);
    if let Some(path) = value.strip_prefix(r"\\?\UNC\") {
        value = format!(r"\\{path}");
    } else if let Some(path) = value.strip_prefix(r"\\?\") {
        value = path.to_owned();
    }
    Ok(PathBuf::from(value))
}

fn parent_process_id(pid: u32) -> Option<u32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut found = None;
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        if entry.th32ProcessID == pid {
            found = Some(entry.th32ParentProcessID);
            break;
        }
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    unsafe {
        CloseHandle(snapshot);
    }
    found
}

fn drain_completion_port(port: HANDLE) -> (u64, u64) {
    let mut creates = 0;
    let mut exits = 0;
    loop {
        let mut message = 0_u32;
        let mut key = 0_usize;
        let mut overlapped = null_mut();
        let ok =
            unsafe { GetQueuedCompletionStatus(port, &mut message, &mut key, &mut overlapped, 0) };
        if ok == 0 {
            break;
        }
        if key == COMPLETION_KEY {
            if message == JOB_OBJECT_MSG_NEW_PROCESS {
                creates += 1;
            }
            if message == JOB_OBJECT_MSG_EXIT_PROCESS {
                exits += 1;
            }
        }
    }
    (creates, exits)
}

fn environment_block(values: &BTreeMap<String, String>) -> Vec<u16> {
    let mut block = Vec::new();
    for (key, value) in values {
        block.extend(OsStr::new(&format!("{key}={value}")).encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

fn wide_nul(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

fn quote_command_line(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| quote_argument(argument))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_argument(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .bytes()
            .any(|byte| matches!(byte, b' ' | b'\t' | b'\"'))
    {
        return argument.to_owned();
    }
    let mut output = String::from("\"");
    let mut slashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            slashes += 1;
            continue;
        }
        if character == '"' {
            output.push_str(&"\\".repeat(slashes * 2 + 1));
            output.push('"');
        } else {
            output.push_str(&"\\".repeat(slashes));
            output.push(character);
        }
        slashes = 0;
    }
    output.push_str(&"\\".repeat(slashes * 2));
    output.push('"');
    output
}

fn last_error(operation: &str) -> String {
    let code = unsafe { GetLastError() };
    format!(
        "{operation} failed with Windows error {code}: {}",
        io::Error::from_raw_os_error(code as i32)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows_sys::Win32::Storage::FileSystem::SetFileTime;

    #[test]
    fn debugger_idle_wait_defers_timeout_to_outer_guard() {
        assert_eq!(DEBUG_EVENT_WAIT_MS, u32::MAX);
    }

    #[test]
    fn only_the_initial_loader_breakpoint_is_debugger_handled() {
        let mut pending = BTreeSet::from([41]);
        let initial = EXCEPTION_BREAKPOINT;
        let initial_loader_breakpoint = initial == EXCEPTION_BREAKPOINT && pending.remove(&41);
        assert!(initial_loader_breakpoint);

        let application_breakpoint = initial == EXCEPTION_BREAKPOINT && pending.remove(&41);
        assert!(!application_breakpoint);
    }

    #[test]
    fn same_size_rewrite_with_restored_last_write_time_changes_cache_token() {
        let path = std::env::temp_dir().join(format!(
            "molt-proof-supervisor-cache-token-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"before").unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let (before_information, before_key) = windows_cache_key(&file).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"after!").unwrap();
        file.sync_all().unwrap();
        assert_ne!(
            unsafe {
                SetFileTime(
                    file.as_raw_handle() as HANDLE,
                    null(),
                    null(),
                    &before_information.ftLastWriteTime,
                )
            },
            0
        );
        file.sync_all().unwrap();
        drop(file);

        let file = File::open(&path).unwrap();
        let (after_information, after_key) = windows_cache_key(&file).unwrap();
        assert_eq!(
            before_information.ftLastWriteTime.dwHighDateTime,
            after_information.ftLastWriteTime.dwHighDateTime
        );
        assert_eq!(
            before_information.ftLastWriteTime.dwLowDateTime,
            after_information.ftLastWriteTime.dwLowDateTime
        );
        assert_eq!(before_key.stable_file_id(), after_key.stable_file_id());
        assert_ne!(before_key.mutation_token(), after_key.mutation_token());
        let _ = std::fs::remove_file(path);
    }
}
