// Copyright 2024, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Utilities for safely obtaining `OwnedFd`s for inherited file descriptors.

use nix::{
    fcntl::{F_SETFD, FdFlag, fcntl},
    libc,
};
use std::{
    collections::HashMap,
    fs::{canonicalize, read_dir},
    os::fd::{FromRawFd, OwnedFd, RawFd},
    sync::{Mutex, OnceLock},
};
use thiserror::Error;

static INHERITED_FDS: OnceLock<Mutex<HashMap<RawFd, Option<OwnedFd>>>> = OnceLock::new();

/// Errors that can occur while taking an ownership of `RawFd`
#[derive(Debug, PartialEq, Error)]
pub enum InheritedFdError {
    /// init_inherited_fds() not called
    #[error("init_inherited_fds() not called")]
    NotInitialized,

    /// Ownership already taken
    #[error("Ownership of FD {0} is already taken")]
    OwnershipTaken(RawFd),

    /// Not an inherited file descriptor
    #[error("FD {0} is either invalid file descriptor or not an inherited one")]
    FileDescriptorNotInherited(RawFd),
}

/// Takes ownership of all open file descriptors in this process other than standard
/// input/output/error, so that they can later be obtained by calling [`take_fd_ownership`].
///
/// Sets the `FD_CLOEXEC` flag on all of these file descriptors.
///
/// # Safety
///
/// This must be called very early in the program, before the ownership of any file descriptors
/// (except stdin/out/err) is taken.
pub unsafe fn init_inherited_fds() -> Result<(), std::io::Error> {
    let mut fds = HashMap::new();

    let fd_path = canonicalize("/proc/self/fd")?;

    for entry in read_dir(&fd_path)? {
        let entry = entry?;

        // Files in /prod/self/fd are guaranteed to be numbers. So parsing is always successful.
        let file_name = entry.file_name();
        let raw_fd = file_name.to_str().unwrap().parse::<RawFd>().unwrap();

        // We don't take ownership of the stdio FDs as the Rust runtime owns them.
        if [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO].contains(&raw_fd) {
            continue;
        }

        // Exceptional case: /proc/self/fd/* may be a dir fd created by read_dir just above. Since
        // the file descriptor is owned by read_dir (and thus closed by it), we shouldn't take
        // ownership to it.
        if entry.path().read_link()? == fd_path {
            continue;
        }

        // SAFETY: /proc/self/fd/* are file descriptors that are open. If `init_inherited_fds()` was
        // called at the very beginning of the program execution (as requested by the safety
        // requirement of this function), this is the first time to claim the ownership of these
        // file descriptors.
        let owned_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

        fcntl(&owned_fd, F_SETFD(FdFlag::FD_CLOEXEC))?;
        fds.insert(raw_fd, Some(owned_fd));
    }

    INHERITED_FDS
        .set(Mutex::new(fds))
        .or(Err(std::io::Error::other(
            "Inherited fds were already initialized",
        )))
}

/// Takes the ownership of the given `RawFd` and returns an `OwnedFd` for it.
///
/// The returned FD will have the `FD_CLOEXEC` flag set.
///
/// An error is returned when the ownership was already taken (by a prior call to this
/// function with the same `RawFd`) or `RawFd` is not an inherited file descriptor.
pub fn take_fd_ownership(raw_fd: RawFd) -> Result<OwnedFd, InheritedFdError> {
    let mut fds = INHERITED_FDS
        .get()
        .ok_or(InheritedFdError::NotInitialized)?
        .lock()
        .unwrap();

    if let Some(value) = fds.get_mut(&raw_fd) {
        if let Some(owned_fd) = value.take() {
            Ok(owned_fd)
        } else {
            Err(InheritedFdError::OwnershipTaken(raw_fd))
        }
    } else {
        Err(InheritedFdError::FileDescriptorNotInherited(raw_fd))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use nix::unistd::close;
    use std::{
        io,
        os::fd::{AsRawFd, IntoRawFd},
    };
    use tempfile::tempfile;

    struct Fixture {
        fds: Vec<RawFd>,
    }

    impl Fixture {
        fn setup(num_fds: usize) -> Result<Self, io::Error> {
            let mut fds = Vec::new();
            for _ in 0..num_fds {
                fds.push(tempfile()?.into_raw_fd());
            }
            Ok(Fixture { fds })
        }

        fn open_new_file(&mut self) -> Result<RawFd, io::Error> {
            let raw_fd = tempfile()?.into_raw_fd();
            self.fds.push(raw_fd);
            Ok(raw_fd)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.fds.iter().for_each(|fd| {
                let _ = close(*fd);
            });
        }
    }

    fn is_fd_opened(raw_fd: RawFd) -> bool {
        unsafe { libc::fcntl(raw_fd, libc::F_GETFD) != -1 }
    }

    #[test]
    fn happy_case() {
        let fixture = Fixture::setup(2).unwrap();
        let f0 = fixture.fds[0];
        let f1 = fixture.fds[1];

        // SAFETY: assume files opened by Fixture are inherited ones
        unsafe {
            init_inherited_fds().unwrap();
        }

        let f0_owned = take_fd_ownership(f0).unwrap();
        let f1_owned = take_fd_ownership(f1).unwrap();
        assert_eq!(f0, f0_owned.as_raw_fd());
        assert_eq!(f1, f1_owned.as_raw_fd());

        drop(f0_owned);
        drop(f1_owned);
        assert!(!is_fd_opened(f0));
        assert!(!is_fd_opened(f1));
    }

    #[test]
    fn access_non_inherited_fd() {
        let mut fixture = Fixture::setup(2).unwrap();

        // SAFETY: assume files opened by Fixture are inherited ones
        unsafe {
            init_inherited_fds().unwrap();
        }

        let f = fixture.open_new_file().unwrap();
        assert_eq!(
            take_fd_ownership(f).err(),
            Some(InheritedFdError::FileDescriptorNotInherited(f))
        );
    }

    #[test]
    fn call_init_inherited_fds_multiple_times() {
        let _ = Fixture::setup(2).unwrap();

        // SAFETY: assume files opened by Fixture are inherited ones
        unsafe {
            init_inherited_fds().unwrap();
        }

        // SAFETY: for testing
        let res = unsafe { init_inherited_fds() };
        assert!(res.is_err());
    }

    #[test]
    fn access_without_init_inherited_fds() {
        let fixture = Fixture::setup(2).unwrap();

        let f = fixture.fds[0];
        assert_eq!(
            take_fd_ownership(f).err(),
            Some(InheritedFdError::NotInitialized)
        );
    }

    #[test]
    fn double_ownership() {
        let fixture = Fixture::setup(2).unwrap();
        let f = fixture.fds[0];

        // SAFETY: assume files opened by Fixture are inherited ones
        unsafe {
            init_inherited_fds().unwrap();
        }

        let f_owned = take_fd_ownership(f).unwrap();
        let f_double_owned = take_fd_ownership(f);
        assert_eq!(
            f_double_owned.err(),
            Some(InheritedFdError::OwnershipTaken(f)),
        );

        // just to highlight that f_owned is kept alive when the second call to take_fd_ownership
        // is made.
        drop(f_owned);
    }

    #[test]
    fn take_drop_retake() {
        let fixture = Fixture::setup(2).unwrap();
        let f = fixture.fds[0];

        // SAFETY: assume files opened by Fixture are inherited ones
        unsafe {
            init_inherited_fds().unwrap();
        }

        let f_owned = take_fd_ownership(f).unwrap();
        drop(f_owned);

        let f_double_owned = take_fd_ownership(f);
        assert_eq!(
            f_double_owned.err(),
            Some(InheritedFdError::OwnershipTaken(f)),
        );
    }

    #[test]
    fn cloexec() {
        let fixture = Fixture::setup(2).unwrap();
        let f = fixture.fds[0];

        let res = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETFD, 0) };
        assert_ne!(res, -1);

        // SAFETY: assume files opened by Fixture are inherited ones
        unsafe {
            init_inherited_fds().unwrap();
        }

        // SAFETY: F_GETFD doesn't need any extra parameters.
        let flags = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags, -1);
        // FD_CLOEXEC should be set by init_inherited_fds
        assert_eq!(flags, FdFlag::FD_CLOEXEC.bits());
    }
}
