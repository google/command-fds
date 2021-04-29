// Copyright 2021, The Android Open Source Project
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

//! A library for passing arbitrary file descriptors when spawning child processes.
//!
//! # Example
//!
//! ```rust
//! use command_fds::{CommandFdExt, FdMapping};
//! use std::fs::File;
//! use std::os::unix::io::AsRawFd;
//! use std::process::Command;
//!
//! // Open a file.
//! let file = File::open("Cargo.toml").unwrap();
//!
//! // Prepare to run `ls -l /proc/self/fd` with some FDs mapped.
//! let mut command = Command::new("ls");
//! command.arg("-l").arg("/proc/self/fd");
//! command
//!     .fd_mappings(vec![
//!         // Map `file` as FD 3 in the child process.
//!         FdMapping {
//!             parent_fd: file.as_raw_fd(),
//!             child_fd: 3,
//!         },
//!         // Map this process's stdin as FD 5 in the child process.
//!         FdMapping {
//!             parent_fd: 0,
//!             child_fd: 5,
//!         },
//!     ])
//!     .unwrap();
//!
//! // Spawn the child process.
//! let mut child = command.spawn().unwrap();
//! child.wait().unwrap();
//! ```

use nix::fcntl::{fcntl, FcntlArg};
use nix::unistd::dup2;
use std::cmp::max;
use std::io::{self, ErrorKind};
use std::os::unix::io::RawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;
use thiserror::Error;

/// A mapping from a file descriptor in the parent to a file descriptor in the child, to be applied
/// when spawning a child process.
///
/// The parent_fd must be kept open until after the child is spawned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FdMapping {
    pub parent_fd: RawFd,
    pub child_fd: RawFd,
}

/// Error setting up FD mappings, because there were two or more mappings for the same child FD.
#[derive(Copy, Clone, Debug, Eq, Error, PartialEq)]
#[error("Two or more mappings for the same child FD")]
pub struct FdMappingCollision;

/// Extension to add file descriptor mappings to a [`Command`].
pub trait CommandFdExt {
    /// Adds the given set of file descriptor to the command.
    ///
    /// Calling this more than once on the same command may result in unexpected behaviour.
    fn fd_mappings(&mut self, mappings: Vec<FdMapping>) -> Result<(), FdMappingCollision>;
}

impl CommandFdExt for Command {
    fn fd_mappings(&mut self, mappings: Vec<FdMapping>) -> Result<(), FdMappingCollision> {
        // Validate that there are no conflicting mappings to the same child FD.
        let mut child_fds: Vec<RawFd> = mappings.iter().map(|mapping| mapping.child_fd).collect();
        child_fds.sort_unstable();
        child_fds.dedup();
        if child_fds.len() != mappings.len() {
            return Err(FdMappingCollision);
        }

        // Register the callback to apply the mappings after forking but before execing.
        unsafe {
            self.pre_exec(move || map_fds(&mappings));
        }

        Ok(())
    }
}

fn map_fds(mappings: &[FdMapping]) -> io::Result<()> {
    if mappings.is_empty() {
        // No need to do anything, and finding first_unused_fd would fail.
        return Ok(());
    }

    // Find the first FD which is higher than any parent or child FD in the mapping, so we can
    // safely use it and higher FDs as temporary FDs. There may be other files open with these FDs,
    // so we still need to ensure we don't conflict with them.
    let first_safe_fd = mappings
        .iter()
        .map(|mapping| max(mapping.parent_fd, mapping.child_fd))
        .max()
        .unwrap()
        + 1;

    // If any parent FDs conflict with child FDs, then first duplicate them to a temporary FD which
    // is clear of either range.
    let child_fds: Vec<RawFd> = mappings.iter().map(|mapping| mapping.child_fd).collect();
    let mappings = mappings
        .into_iter()
        .map(|mapping| {
            Ok(if child_fds.contains(&mapping.parent_fd) {
                let temporary_fd =
                    fcntl(mapping.parent_fd, FcntlArg::F_DUPFD_CLOEXEC(first_safe_fd))?;
                FdMapping {
                    parent_fd: temporary_fd,
                    child_fd: mapping.child_fd,
                }
            } else {
                mapping.to_owned()
            })
        })
        .collect::<nix::Result<Vec<_>>>()
        .map_err(nix_to_io_error)?;

    // Now we can actually duplicate FDs to the desired child FDs.
    for mapping in mappings {
        // This closes child_fd if it is already open as something else, and clears the FD_CLOEXEC
        // flag on child_fd.
        dup2(mapping.parent_fd, mapping.child_fd).map_err(nix_to_io_error)?;
    }

    Ok(())
}

/// Convert a [`nix::Error`] to a [`std::io::Error`].
fn nix_to_io_error(error: nix::Error) -> io::Error {
    if let nix::Error::Sys(errno) = error {
        io::Error::from_raw_os_error(errno as i32)
    } else {
        io::Error::new(ErrorKind::Other, error)
    }
}
