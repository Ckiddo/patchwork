//! Lock guard lives until all HTTP workers have stopped.
use std::io;

#[cfg(windows)]
pub struct InstanceGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl InstanceGuard {
    pub fn acquire() -> io::Result<Self> {
        Self::named("Global\\PatchworkBackend")
    }
    fn named(name: &str) -> io::Result<Self> {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError},
            System::Threading::CreateMutexW,
        };
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: NUL-terminated UTF-16 name, default security, handle owned below.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: immediately follows CreateMutexW; valid owned handle.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle);
            }
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Patchwork backend is already running",
            ));
        }
        Ok(Self(handle))
    }
}
#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the successful CreateMutexW handle.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
pub struct InstanceGuard(std::fs::File);
#[cfg(not(windows))]
impl InstanceGuard {
    pub fn acquire() -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(std::env::temp_dir().join("patchwork-backend.lock"))?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Patchwork backend is already running",
            )
        })?;
        Ok(Self(file))
    }
}
#[cfg(not(windows))]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn duplicate_instance_is_rejected_and_exit_releases_handle() {
        let name = format!("Local\\PatchworkTest-{}", uuid::Uuid::new_v4());
        let guard = InstanceGuard::named(&name).unwrap();
        assert!(
            matches!(InstanceGuard::named(&name), Err(e) if e.kind() == io::ErrorKind::AlreadyExists)
        );
        drop(guard);
        assert!(InstanceGuard::named(&name).is_ok());
    }
}
