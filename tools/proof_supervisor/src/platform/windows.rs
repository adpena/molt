use crate::{
    Admission, BackendFailure, CAPABILITY_SCHEMA, Capability, ClosureMode, EventJournal,
    FileIdentity, ImageHashCache, KernelAccounting, ProcessEventKind, Receipt,
    TerminalObservations, ValidatedPolicy, push_bounded_diagnostic,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    CloseHandle, DBG_CONTINUE, DBG_EXCEPTION_NOT_HANDLED, ERROR_SEM_TIMEOUT, EXCEPTION_BREAKPOINT,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, NTSTATUS, WAIT_FAILED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CREATE_PROCESS_DEBUG_EVENT, ContinueDebugEvent, DEBUG_EVENT, EXCEPTION_DEBUG_EVENT,
    EXIT_PROCESS_DEBUG_EVENT, LOAD_DLL_DEBUG_EVENT, WaitForDebugEvent,
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
    GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, TerminateProcess,
    WaitForSingleObject,
};

const COMPLETION_KEY: usize = 0x4d4f_4c54;
// The proof queue/memory guard owns wall-clock timeout. An idle debugger wait
// is not a timeout signal: valid proof commands can execute without emitting a
// process event for arbitrarily long periods.
const DEBUG_EVENT_WAIT_MS: u32 = u32::MAX;

pub(super) fn required_environment() -> BTreeMap<String, String> {
    // DEBUG_PROCESS is required for pre-entry recursive image custody. Windows
    // otherwise enables debugger heap options that prevent the normal LFH.
    // Seal the normal-heap setting in the caller's exact environment; never
    // silently mutate the launched environment after policy capture.
    BTreeMap::from([("_NO_DEBUG_HEAP".to_owned(), "1".to_owned())])
}

pub fn capability(mode: ClosureMode) -> Capability {
    Capability {
        schema: CAPABILITY_SCHEMA.to_owned(),
        platform: "windows".to_owned(),
        mode,
        backend: "debug-process+nested-job".to_owned(),
        admission: Admission::Eligible {},
        pre_entry_exec_authority: true,
        pre_entry_process_create_authority: true,
        recursive_descendant_authority: true,
        required_environment: super::required_environment(),
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

pub fn run(policy: &ValidatedPolicy, events: &mut EventJournal, capability: Capability) -> Receipt {
    super::run_backend(policy, events, capability, |policy, events| unsafe {
        supervise(policy, events)
    })
}

unsafe fn supervise(
    policy: &ValidatedPolicy,
    events: &mut EventJournal,
) -> Result<crate::NativeCustody, BackendFailure> {
    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
        return Err(last_error("CreateJobObjectW").into());
    }
    let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, null_mut(), 0, 1) };
    if port.is_null() {
        let error = last_error("CreateIoCompletionPort");
        unsafe {
            CloseHandle(job);
        }
        return Err(error.into());
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
        return Err(last_error("SetInformationJobObject(limits)").into());
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
        return Err(last_error("SetInformationJobObject(completion port)").into());
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
    let mut state = DebugClosure::prepare()?;
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
        return Err(last_error("CreateProcessW").into());
    }
    handles.process = process.hProcess;
    handles.thread = process.hThread;
    let root_in_job = unsafe { AssignProcessToJobObject(job, process.hProcess) } != 0;
    state.root_pid = process.dwProcessId;
    if !root_in_job {
        state.error(last_error("AssignProcessToJobObject"));
    } else if unsafe { ResumeThread(process.hThread) } == u32::MAX {
        state.error(last_error("ResumeThread"));
    }
    // Every post-creation error enters this same event/drain owner. In
    // particular a failed journal write cannot skip event continuation, root
    // wait, or independent Job accounting. A root not assigned to the Job is
    // still held by the actual CreateProcess handle and has never run.
    if state.failure.is_some() {
        state.terminate(&handles, root_in_job, 125, false);
    }
    let mut hash_cache = ImageHashCache::default();
    let mut pending_debug_stop = false;
    loop {
        if state.actuation_failed {
            break; // Do not release an unadmitted image after failed termination.
        }
        let wait_ms = if let Some(deadline) = state.cleanup_deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                state.error(
                    "debug cleanup deadline expired with retained events/processes".to_owned(),
                );
                break;
            }
            remaining.as_millis().min(100) as u32
        } else {
            DEBUG_EVENT_WAIT_MS
        };
        let mut event: DEBUG_EVENT = unsafe { zeroed() };
        if unsafe { WaitForDebugEvent(&mut event, wait_ms) } == 0 {
            // Timeout is expected only in bounded cleanup. Capture the error
            // before another API call changes GetLastError.
            let code = unsafe { GetLastError() };
            if state.cleanup_deadline.is_some() && code == ERROR_SEM_TIMEOUT {
                continue;
            }
            state.error(format!(
                "WaitForDebugEvent failed with Windows error {code}"
            ));
            if state.cleanup_deadline.is_some() {
                break;
            }
            state.terminate(&handles, root_in_job, 125, false);
            continue;
        }
        pending_debug_stop = true;
        let pid = event.dwProcessId;
        // Debug process/thread handles belong to the OS through the actual
        // corresponding EXIT continuation (WaitForDebugEvent contract). Only
        // image/DLL file handles are ours to close; take that ownership before
        // any fallible hashing, generation, or journal operation.
        let image_handle = match event.dwDebugEventCode {
            CREATE_PROCESS_DEBUG_EVENT => unsafe { event.u.CreateProcessInfo.hFile },
            LOAD_DLL_DEBUG_EVENT => unsafe { event.u.LoadDll.hFile },
            _ => null_mut(),
        };
        let mut image_file =
            (!image_handle.is_null()).then(|| unsafe { File::from_raw_handle(image_handle as _) });
        // ContinueDebugEvent takes the NTSTATUS that DBG_CONTINUE and
        // DBG_EXCEPTION_NOT_HANDLED carry.
        let observed = (|| -> Result<(NTSTATUS, Option<u32>), String> {
            let mut continue_status = DBG_CONTINUE;
            let mut terminate_code = None;
            match event.dwDebugEventCode {
                CREATE_PROCESS_DEBUG_EVENT => {
                    state.generation = state
                        .generation
                        .checked_add(1)
                        .ok_or_else(|| "process generation overflow".to_owned())?;
                    // The OS debug stop and retained Job/root handle remain
                    // the cleanup owner if these bounded indexes refuse growth.
                    if state.active.len() >= crate::BUDGET_LIVE_PROCESSES {
                        return Err("live debug-process budget exhausted".to_owned());
                    }
                    state
                        .active
                        .try_reserve(1)
                        .map_err(|e| format!("debug-process reservation: {e}"))?;
                    state
                        .pending_initial_breakpoints
                        .try_reserve(1)
                        .map_err(|e| format!("breakpoint reservation: {e}"))?;
                    use std::fmt::Write;
                    let mut stable_id = String::new();
                    stable_id
                        .try_reserve_exact(crate::BUDGET_STABLE_PROCESS_ID_UTF8_BYTES)
                        .map_err(|e| format!("debug identity reservation: {e}"))?;
                    write!(&mut stable_id, "windows:{pid}:{}", state.generation)
                        .expect("reserved identity");
                    let parent =
                        parent_process_id(pid).filter(|parent| state.active.contains_key(parent));
                    if state.active.contains_key(&pid) {
                        return Err(format!(
                            "debug process {pid} reused a live numeric identity"
                        ));
                    }
                    state.active.insert(
                        pid,
                        DebugProcess {
                            stable_id: stable_id.clone(),
                            recorded: false,
                        },
                    );
                    state.pending_initial_breakpoints.insert(pid);
                    // Genuine lifecycle creation precedes every fallible image
                    // observation. Cleanup can now publish the real EXIT even
                    // if the image handle/hash/admission is refused.
                    let created = events.record(
                        pid,
                        stable_id.clone(),
                        ProcessEventKind::ProcessCreate {
                            parent_process_id: parent,
                        },
                    )?;
                    state
                        .active
                        .get_mut(&pid)
                        .expect("debug process inserted")
                        .recorded = true;
                    if created.must_terminate_closure() {
                        terminate_code = Some(126);
                    } else if state.failure.is_none() {
                        let file = image_file.take().ok_or_else(|| {
                            "CREATE_PROCESS_DEBUG_EVENT did not provide an image handle".to_owned()
                        })?;
                        let image = image_identity(policy, file, &mut hash_cache)?;
                        let outcome = events.record(
                            pid,
                            stable_id,
                            ProcessEventKind::InitialImage { image },
                        )?;
                        if outcome.must_terminate_closure() {
                            terminate_code = Some(126);
                        }
                    }
                    // A newly observed member is a new cleanup obligation,
                    // even if Job termination was already requested. Keep the
                    // original deadline/code and actuate only the retained Job.
                    if let Some(code) = state.termination_code {
                        terminate_code = Some(code);
                    }
                }
                EXIT_PROCESS_DEBUG_EVENT => {
                    let exit_code = unsafe { event.u.ExitProcess.dwExitCode } as i64;
                    state.terminals.observe(pid, exit_code);
                    state.pending_initial_breakpoints.remove(&pid);
                    if pid == state.root_pid {
                        state.root_exit = Some(exit_code);
                    }
                    let process = state.active.remove(&pid).ok_or_else(|| {
                        format!("terminal debug event for unobserved process {pid}: {exit_code}")
                    })?;
                    if process.recorded {
                        let outcome = events.record(
                            pid,
                            process.stable_id,
                            ProcessEventKind::ProcessExit { exit_code },
                        )?;
                        if outcome.must_terminate_closure() {
                            terminate_code = Some(if outcome.has_policy_violation() {
                                126
                            } else {
                                0
                            });
                        }
                    } else {
                        return Err(format!(
                            "unpublished process {pid} terminal debug status {exit_code}"
                        ));
                    }
                }
                EXCEPTION_DEBUG_EVENT => {
                    let code = unsafe { event.u.Exception.ExceptionRecord.ExceptionCode };
                    let initial = code == EXCEPTION_BREAKPOINT
                        && state.pending_initial_breakpoints.remove(&pid);
                    if !initial {
                        continue_status = DBG_EXCEPTION_NOT_HANDLED;
                    }
                }
                _ => {}
            }
            Ok((continue_status, terminate_code))
        })();
        drop(image_file);
        let continue_status = match observed {
            Ok((status, terminate)) => {
                if let Some(code) = terminate {
                    state.terminate(
                        &handles,
                        root_in_job,
                        code,
                        event.dwDebugEventCode == CREATE_PROCESS_DEBUG_EVENT,
                    );
                }
                status
            }
            Err(error) => {
                events.cutoff(crate::CaptureStage::NativeObservation, &error);
                state.error(error);
                state.terminate(
                    &handles,
                    root_in_job,
                    125,
                    event.dwDebugEventCode == CREATE_PROCESS_DEBUG_EVENT,
                );
                DBG_CONTINUE // Only CREATE/EXIT observation can fail above.
            }
        };
        if state.actuation_failed && event.dwDebugEventCode != EXIT_PROCESS_DEBUG_EVENT {
            state.error(format!(
                "debug event {pid}/{} retained after termination failure",
                event.dwThreadId
            ));
            break;
        }
        if unsafe { ContinueDebugEvent(pid, event.dwThreadId, continue_status) } == 0 {
            state.error(last_error("ContinueDebugEvent"));
            state.terminate(&handles, root_in_job, 125, false);
            // The stop remains unresolved. Do not retry or interpret a later
            // numeric PID as authority; still collect independent wait/Job facts.
            break;
        }
        pending_debug_stop = false;
        if state.generation == state.terminals.count && state.root_exit.is_some() {
            break;
        }
    }

    let wait_ms = state.cleanup_deadline.map_or(5_000, |deadline| {
        deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .min(5_000) as u32
    });
    let waited = unsafe { WaitForSingleObject(handles.process, wait_ms) };
    let mut waited_exit = None;
    if waited == WAIT_OBJECT_0 {
        let mut code = 0;
        if unsafe { GetExitCodeProcess(handles.process, &mut code) } != 0 {
            waited_exit = Some(code);
        } else {
            state.error(last_error("GetExitCodeProcess after root wait"));
        }
    } else {
        state.error(if waited == WAIT_FAILED {
            last_error("WaitForSingleObject(root process drain)")
        } else {
            format!("root process drain wait returned {waited:#x}")
        });
    }
    if let (Some(waited), Some(debugged)) = (waited_exit, state.root_exit)
        && i64::from(waited) != debugged
    {
        state.error(format!(
            "root handle exit {waited} disagrees with terminal debug exit {debugged}"
        ));
    }
    let mut kernel_accounting = None;
    for _ in 0..100 {
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
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
            state.error(last_error("QueryInformationJobObject(accounting)"));
            break;
        }
        let active = accounting.ActiveProcesses;
        kernel_accounting = Some(KernelAccounting::WindowsJob {
            total_processes: accounting.TotalProcesses as u64,
            active_processes: active as u64,
            completion_port_new_processes: 0,
            completion_port_exits: 0,
        });
        if active == 0 {
            break;
        }
        std::thread::yield_now();
    }
    let (new_processes, exits) = drain_completion_port(port);
    if let Some(KernelAccounting::WindowsJob {
        active_processes,
        completion_port_new_processes,
        completion_port_exits,
        ..
    }) = &mut kernel_accounting
    {
        *completion_port_new_processes = new_processes;
        *completion_port_exits = exits;
        if *active_processes != 0 {
            state.error(format!(
                "Job drain retains {active_processes} active processes"
            ));
        }
    }
    let remaining_processes = state.generation.saturating_sub(state.terminals.count);
    if remaining_processes != 0 || state.root_exit.is_none() {
        state.error(format!(
            "debug closure unresolved: {} active processes; root exit={:?}",
            state.active.len(),
            state.root_exit
        ));
    }
    let expected_job_creates = state.generation.checked_sub(u64::from(!root_in_job));
    let job_totals_reconciled = matches!(&kernel_accounting, Some(KernelAccounting::WindowsJob {total_processes,active_processes,..})
        if Some(*total_processes)==expected_job_creates && *active_processes==remaining_processes);
    if !job_totals_reconciled {
        state.error("held Job totals do not reconcile with observed debug lifecycle; observation remains incomplete".to_owned());
    }
    if let Some(cause) = state.failure.as_deref() {
        events.cutoff(crate::CaptureStage::NativeObservation, cause);
    }
    let native_custody = crate::NativeCustody::Windows {
        root_in_job,
        root_exit_code: waited_exit.map(i64::from),
        debug_root_exit_code: state.root_exit,
        remaining_processes,
        observed_creates: state.generation,
        observed_exits: state.terminals.count,
        job_totals_reconciled,
        pending_debug_stop,
        job: kernel_accounting,
    };
    if let Some(cause) = state.failure {
        let mut failure = BackendFailure::from(cause);
        failure.native_custody = native_custody;
        failure.retain_cleanup(format!("{}; root debug exit={:?}; root handle wait={waited:#x}; waited exit={waited_exit:?}; active debug processes={}; cleanup errors={}",
            state.terminals.summary("debug events"), state.root_exit, state.active.len(), state.error_count));
        for error in state.errors {
            failure.retain_cleanup(error);
        }
        Err(failure)
    } else {
        Ok(native_custody)
    }
}

struct DebugProcess {
    stable_id: String,
    recorded: bool,
}

struct DebugClosure {
    root_pid: u32,
    active: HashMap<u32, DebugProcess>,
    pending_initial_breakpoints: HashSet<u32>,
    generation: u64,
    root_exit: Option<i64>,
    terminals: TerminalObservations,
    failure: Option<String>,
    errors: Vec<String>,
    error_count: u64,
    cleanup_deadline: Option<std::time::Instant>,
    actuation_failed: bool,
    termination_code: Option<u32>,
}

impl DebugClosure {
    fn prepare() -> Result<Self, String> {
        let mut active = HashMap::new();
        let mut pending_initial_breakpoints = HashSet::new();
        active
            .try_reserve(1)
            .map_err(|e| format!("root debug reservation: {e}"))?;
        pending_initial_breakpoints
            .try_reserve(1)
            .map_err(|e| format!("root breakpoint reservation: {e}"))?;
        Ok(Self {
            root_pid: 0,
            active,
            pending_initial_breakpoints,
            generation: 0,
            root_exit: None,
            terminals: TerminalObservations::default(),
            failure: None,
            errors: Vec::new(),
            error_count: 0,
            cleanup_deadline: None,
            actuation_failed: false,
            termination_code: None,
        })
    }

    fn error(&mut self, error: String) {
        self.error_count += 1;
        if self.failure.is_none() {
            self.failure = Some(error);
        } else {
            push_bounded_diagnostic(&mut self.errors, error);
        }
    }

    fn terminate(
        &mut self,
        handles: &Handles,
        root_in_job: bool,
        exit_code: u32,
        new_member: bool,
    ) {
        if self.cleanup_deadline.is_some() && !(root_in_job && new_member) {
            return;
        }
        self.cleanup_deadline
            .get_or_insert_with(|| std::time::Instant::now() + std::time::Duration::from_secs(5));
        let exit_code = *self.termination_code.get_or_insert(exit_code);
        let result = unsafe {
            if root_in_job {
                TerminateJobObject(handles.job, exit_code)
            } else {
                TerminateProcess(handles.process, exit_code)
            }
        };
        if result == 0 {
            self.error(last_error(if root_in_job {
                "TerminateJobObject"
            } else {
                "TerminateProcess(unassigned root)"
            }));
            self.actuation_failed = true;
        }
    }
}

fn image_identity(
    policy: &ValidatedPolicy,
    mut file: File,
    hash_cache: &mut ImageHashCache,
) -> Result<FileIdentity, String> {
    let handle = file.as_raw_handle() as HANDLE;
    let cache_key = crate::image_cache::opened_file_key(&file).map_err(|e| e.to_string())?;
    let path = path_from_handle(handle)?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let file_id = cache_key.stable_file_id().to_owned();
    let digest = hash_cache
        .digest(&cache_key, &mut file, |file| {
            crate::image_cache::opened_file_key(file)
        })
        .map_err(|error| format!("cannot hash executable image: {error}"))?;
    Ok(policy.classify_path(&path, file_id, size, digest))
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
    let value = OsString::from_wide(&buffer[..count as usize]);
    let path = PathBuf::from(value);
    Ok(dunce::simplified(&path).to_path_buf())
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
    use std::collections::BTreeSet;
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle, SetFileTime,
    };

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
        let mut before_information: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
        assert_ne!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut before_information) },
            0
        );
        let before_key = crate::image_cache::opened_file_key(&file).unwrap();
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
        let mut after_information: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
        assert_ne!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut after_information) },
            0
        );
        let after_key = crate::image_cache::opened_file_key(&file).unwrap();
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
