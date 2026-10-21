use std::{
    collections::VecDeque,
    io::{self, Read},
    process::{Child, Command, ExitStatus},
    sync::mpsc::{self, Receiver},
    thread,
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

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

/// Owns a spawned child and its descendants until protocol handling finishes.
/// Dropping the guard tears down the process tree even when a caller returns
/// early through an error path.
pub(super) struct ChildTree {
    child: Child,
    cleaned: bool,
    #[cfg(windows)]
    job: WindowsJob,
}

impl ChildTree {
    pub(super) fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            command.process_group(0);
            Ok(Self {
                child: command.spawn()?,
                cleaned: false,
            })
        }

        #[cfg(windows)]
        {
            Self::spawn_windows(command)
        }

        #[cfg(not(any(unix, windows)))]
        {
            Ok(Self {
                child: command.spawn()?,
                cleaned: false,
            })
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
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }

        #[cfg(windows)]
        {
            self.job.terminate();
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
impl ChildTree {
    fn spawn_windows(command: &mut Command) -> io::Result<Self> {
        let job = WindowsJob::create()?;
        command.creation_flags(windows_job::CREATE_SUSPENDED | windows_job::CREATE_NO_WINDOW);
        let mut child = command.spawn()?;
        if let Err(error) = job.assign(&child) {
            terminate_child(&mut child);
            return Err(error);
        }
        if let Err(error) = windows_job::resume_primary_thread(child.id()) {
            job.terminate();
            terminate_child(&mut child);
            return Err(error);
        }
        Ok(Self {
            child,
            cleaned: false,
            job,
        })
    }
}

#[cfg(windows)]
fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(windows)]
struct WindowsJob(windows_job::Handle);

#[cfg(windows)]
impl WindowsJob {
    fn create() -> io::Result<Self> {
        use std::ptr::{null, null_mut};

        let job = Self(windows_job::Handle::from_raw(unsafe {
            windows_job::CreateJobObjectW(null_mut(), null())
        })?);
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
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;

        let assigned = unsafe {
            windows_job::AssignProcessToJobObject(self.0.as_ptr(), child.as_raw_handle().cast())
        };
        if assigned == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn terminate(&self) {
        unsafe {
            windows_job::TerminateJobObject(self.0.as_ptr(), 1);
        }
    }
}

#[cfg(windows)]
mod windows_job {
    use std::{ffi::c_void, io, ptr::NonNull};

    pub(super) const CREATE_SUSPENDED: u32 = 0x0000_0004;
    pub(super) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const THREAD_SUSPEND_RESUME: u32 = 0x0002;
    const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;
    const INVALID_HANDLE_VALUE: isize = -1;
    pub(super) const EXTENDED_LIMIT_INFORMATION: i32 = 9;
    pub(super) const KILL_ON_JOB_CLOSE: u32 = 0x2000;

    pub(super) struct Handle(NonNull<c_void>);

    impl Handle {
        pub(super) fn from_raw(raw: *mut c_void) -> io::Result<Self> {
            NonNull::new(raw)
                .map(Self)
                .ok_or_else(io::Error::last_os_error)
        }

        pub(super) fn as_ptr(&self) -> *mut c_void {
            self.0.as_ptr()
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.as_ptr());
            }
        }
    }

    // Layout matches Win32 THREADENTRY32: four DWORDs, two LONGs, then DWORD.
    #[repr(C)]
    #[derive(Default)]
    struct ThreadEntry32 {
        size: u32,
        usage_count: u32,
        thread_id: u32,
        owner_process_id: u32,
        base_priority: i32,
        delta_priority: i32,
        flags: u32,
    }

    pub(super) fn resume_primary_thread(process_id: u32) -> io::Result<()> {
        let raw_snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw_snapshot as isize == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Handle::from_raw(raw_snapshot)?;
        let mut entry = ThreadEntry32 {
            size: std::mem::size_of::<ThreadEntry32>() as u32,
            ..ThreadEntry32::default()
        };
        let mut found = None;
        let mut has_entry = unsafe { Thread32First(snapshot.as_ptr(), &mut entry) } != 0;
        while has_entry {
            if entry.owner_process_id == process_id {
                found = Some(entry.thread_id);
                break;
            }
            has_entry = unsafe { Thread32Next(snapshot.as_ptr(), &mut entry) } != 0;
        }
        let thread_id = found.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "suspended process thread was not found",
            )
        })?;
        let thread = Handle::from_raw(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) })?;
        let previous_suspend_count = unsafe { ResumeThread(thread.as_ptr()) };
        if previous_suspend_count == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        if previous_suspend_count != 1 {
            return Err(io::Error::other(
                "suspended process thread had an unexpected suspend count",
            ));
        }
        Ok(())
    }

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
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut c_void;
        fn Thread32First(snapshot: *mut c_void, entry: *mut ThreadEntry32) -> i32;
        fn Thread32Next(snapshot: *mut c_void, entry: *mut ThreadEntry32) -> i32;
        fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> *mut c_void;
        fn ResumeThread(thread: *mut c_void) -> u32;
    }
}
