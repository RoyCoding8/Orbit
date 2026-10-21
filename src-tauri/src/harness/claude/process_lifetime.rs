use std::{
    collections::VecDeque,
    io::{self, Read},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub(super) const OUTPUT_BYTE_LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy)]
pub(super) enum CaptureEnd {
    Beginning,
    End,
}

/// Drain the entire pipe so its writer cannot block, retaining only a bounded
/// prefix or tail for the result shown to the user.
pub(super) fn capture_output<R: Read>(
    mut reader: R,
    limit: usize,
    end: CaptureEnd,
) -> io::Result<String> {
    let mut captured = VecDeque::with_capacity(limit);
    let mut total = 0usize;
    let mut chunk = [0; 8192];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        match end {
            CaptureEnd::Beginning => {
                let available = limit.saturating_sub(captured.len());
                captured.extend(chunk[..read.min(available)].iter().copied());
            }
            CaptureEnd::End => {
                captured.extend(chunk[..read].iter().copied());
                while captured.len() > limit {
                    captured.pop_front();
                }
            }
        }
    }

    let truncated = total > limit;
    let bytes = captured.into_iter().collect::<Vec<_>>();
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        match end {
            CaptureEnd::Beginning => text.push_str("\n[output truncated]"),
            CaptureEnd::End => text.insert_str(0, "[earlier output truncated]\n"),
        }
    }
    Ok(text)
}

pub(super) fn spawn_capture<R: Read + Send + 'static>(
    reader: R,
    limit: usize,
    end: CaptureEnd,
) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let text = capture_output(reader, limit, end).unwrap_or_default();
        let _ = sender.send(text);
    });
    receiver
}

pub(super) fn isolate_process_group(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    let _ = command;
}

/// Owns a spawned child and its descendants until protocol handling finishes.
/// Dropping the guard tears down the process tree even when a caller returns
/// early through an error path.
pub(super) struct ChildTree {
    child: Child,
    cleaned: bool,
    #[cfg(windows)]
    job: Option<WindowsJob>,
}

impl ChildTree {
    pub(super) fn new(child: Child) -> Self {
        #[cfg(windows)]
        let job = WindowsJob::assign(&child);
        Self {
            child,
            cleaned: false,
            #[cfg(windows)]
            job,
        }
    }

    pub(super) fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Stop descendants as well as the direct child, including after the
    /// direct child has exited and left a process group or inherited pipe open.
    pub(super) fn cleanup(&mut self) -> Option<ExitStatus> {
        if self.cleaned {
            return self.child.try_wait().ok().flatten();
        }
        self.cleaned = true;

        #[cfg(unix)]
        {
            let group = format!("-{}", self.child.id());
            let _ = Command::new("kill")
                .args(["-9", &group])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        #[cfg(windows)]
        {
            if let Some(job) = self.job.take() {
                job.terminate();
                drop(job);
            } else {
                kill_windows_tree(&self.child);
            }
        }

        let _ = self.child.kill();
        self.child.wait().ok()
    }
}

impl Drop for ChildTree {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(windows)]
fn kill_windows_tree(child: &Child) {
    let pid = child.id().to_string();
    let _ = Command::new("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(windows)]
struct WindowsJob(std::ptr::NonNull<std::ffi::c_void>);

#[cfg(windows)]
impl WindowsJob {
    fn assign(child: &Child) -> Option<Self> {
        use std::{
            os::windows::io::AsRawHandle,
            ptr::{null, null_mut},
        };

        let job =
            std::ptr::NonNull::new(unsafe { windows_job::CreateJobObjectW(null_mut(), null()) })?;
        let job = Self(job);
        let mut limits = windows_job::ExtendedLimitInformation::default();
        limits.basic.limit_flags = windows_job::KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            windows_job::SetInformationJobObject(
                job.0.as_ptr(),
                windows_job::EXTENDED_LIMIT_INFORMATION,
                (&limits as *const windows_job::ExtendedLimitInformation).cast(),
                std::mem::size_of::<windows_job::ExtendedLimitInformation>() as u32,
            )
        };
        if configured == 0 {
            return None;
        }
        let assigned = unsafe {
            windows_job::AssignProcessToJobObject(job.0.as_ptr(), child.as_raw_handle().cast())
        };
        (assigned != 0).then_some(job)
    }

    fn terminate(&self) {
        unsafe {
            windows_job::TerminateJobObject(self.0.as_ptr(), 1);
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            windows_job::CloseHandle(self.0.as_ptr());
        }
    }
}

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;

    pub(super) const EXTENDED_LIMIT_INFORMATION: i32 = 9;
    pub(super) const KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[repr(C)]
    #[derive(Default)]
    pub(super) struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        pub(super) limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    pub(super) struct ExtendedLimitInformation {
        pub(super) basic: BasicLimitInformation,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub(super) fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> *mut c_void;
        pub(super) fn SetInformationJobObject(
            job: *mut c_void,
            information_class: i32,
            information: *const c_void,
            information_length: u32,
        ) -> i32;
        pub(super) fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        pub(super) fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
        pub(super) fn CloseHandle(handle: *mut c_void) -> i32;
    }
}
