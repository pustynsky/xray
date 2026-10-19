use interprocess::local_socket::{ListenerOptions, Stream, traits::StreamCommon};
use std::{io, process::Command};

#[cfg(windows)]
pub(super) fn restrict_listener(options: ListenerOptions<'_>) -> io::Result<ListenerOptions<'_>> {
    use interprocess::os::windows::{
        local_socket::ListenerOptionsExt,
        security_descriptor::SecurityDescriptor,
    };
    let sid = process_user(std::process::id())?;
    let sddl = widestring::U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    Ok(options.security_descriptor(SecurityDescriptor::deserialize(&sddl)?))
}

#[cfg(unix)]
pub(super) fn restrict_listener(options: ListenerOptions<'_>) -> io::Result<ListenerOptions<'_>> {
    use interprocess::os::unix::local_socket::ListenerOptionsExt;
    Ok(options.mode(0o600))
}

pub(super) fn verify_peer(stream: &Stream) -> io::Result<()> {
    let peer = stream.peer_creds()?;
    #[cfg(windows)]
    let same_user = {
        let pid = peer.pid().ok_or_else(|| io::Error::other("IPC peer PID unavailable"))?;
        process_user(pid)? == process_user(std::process::id())?
    };
    #[cfg(unix)]
    let same_user = peer.euid() == Some(unsafe { libc::geteuid() });
    if !same_user {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "IPC peer is not the current user"));
    }
    Ok(())
}

#[cfg(windows)]
fn process_user(pid: u32) -> io::Result<String> {
    use std::{os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle}, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser},
        System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // Owned handles keep the process identity alive throughout the token lookup.
    unsafe {
        let raw_process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if raw_process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let process = OwnedHandle::from_raw_handle(raw_process);
        let mut raw_token = ptr::null_mut();
        if OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &mut raw_token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle::from_raw_handle(raw_token);
        let mut length = 0;
        GetTokenInformation(token.as_raw_handle(), TokenUser, ptr::null_mut(), 0, &mut length);
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        if GetTokenInformation(token.as_raw_handle(), TokenUser, storage.as_mut_ptr().cast(), length, &mut length) == 0 {
            return Err(io::Error::last_os_error());
        }
        let user = &*storage.as_ptr().cast::<TOKEN_USER>();
        let mut raw_sid = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut raw_sid) == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = widestring::U16CStr::from_ptr_str(raw_sid).to_string();
        LocalFree(raw_sid.cast());
        sid.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

pub(super) fn read_handshake(stream: &mut Stream, bytes: &mut [u8]) -> io::Result<usize> {
    use std::io::Read;
    #[cfg(windows)]
    {
        use std::{os::windows::io::{AsHandle, AsRawHandle}, ptr};
        use windows_sys::Win32::System::Pipes::PeekNamedPipe;
        let Stream::NamedPipe(pipe) = stream;
        let mut available = 0;
        if unsafe { PeekNamedPipe(pipe.as_handle().as_raw_handle(), ptr::null_mut(), 0, ptr::null_mut(), &mut available, ptr::null_mut()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // The transport maps an empty PIPE_NOWAIT read to EOF. Peek first to
        // distinguish a live, empty pipe from a disconnected peer.
        if available == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
    }
    stream.read(bytes)
}

#[cfg(windows)]
pub(super) fn detach(command: &mut Command) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation},
        System::{
            Console::{GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE},
            Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS},
        },
    };
    // Command installs separate null/log handles for the backend, but Windows
    // also inherits any other inheritable handles. Keep the host's pipes private
    // to this proxy so its exit closes stdout/stderr even while the backend lives.
    for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { GetStdHandle(kind) };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        if !handle.is_null() && unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // Fail explicitly when a host job prohibits breakaway; never silently attach
    // the shared backend's lifetime to the first MCP client's job.
    command.creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    Ok(())
}

#[cfg(unix)]
pub(super) fn detach(command: &mut Command) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}
