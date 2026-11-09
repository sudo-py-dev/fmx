//! Hard memory limits installed before a buffering child can decode input.
use std::io;
use std::process::{Child, Command};

#[cfg(unix)]
pub fn prepare(cmd: &mut Command, bytes: usize) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    // SAFETY: the callback uses only async-signal-safe get/setrlimit calls;
    // it neither allocates nor acquires locks between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            let mut limit = std::mem::zeroed::<libc::rlimit>();
            if libc::getrlimit(libc::RLIMIT_AS, &mut limit) != 0 {
                return Err(io::Error::last_os_error());
            }
            limit.rlim_cur = limit.rlim_cur.min(bytes as libc::rlim_t);
            limit.rlim_max = limit.rlim_max.min(bytes as libc::rlim_t);
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(unix)]
pub fn attach(_: &Child, _: usize) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
pub use windows::{attach, prepare};

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::{io::AsRawHandle, process::CommandExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_JOB_MEMORY,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectExtendedLimitInformation, SetInformationJobObject,
            },
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
            },
        },
    };

    pub struct Budget(HANDLE);
    impl Drop for Budget {
        fn drop(&mut self) {
            // SAFETY: this handle is owned here and closed exactly once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub fn prepare(cmd: &mut Command, _: usize) -> io::Result<()> {
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        Ok(())
    }

    pub fn attach(child: &Child, bytes: usize) -> io::Result<Budget> {
        // SAFETY: all pointers refer to live, correctly sized structures. The
        // child is suspended, so it cannot allocate/decode before assignment.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Budget(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            limits.JobMemoryLimit = bytes;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(job.0, child.as_raw_handle()) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let snapshot = Budget(snapshot);
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut found = Thread32First(snapshot.0, &mut entry);
            while found != 0 {
                if entry.th32OwnerProcessID == child.id() {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if thread.is_null() {
                        return Err(io::Error::last_os_error());
                    }
                    let thread = Budget(thread);
                    if ResumeThread(thread.0) == u32::MAX {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(job);
                }
                found = Thread32Next(snapshot.0, &mut entry);
            }
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "suspended child thread not found",
            ))
        }
    }
}
