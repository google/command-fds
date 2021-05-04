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
        .iter()
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

#[cfg(test)]
mod tests {
    use super::*;
    use nix::unistd::close;
    use std::collections::HashSet;
    use std::fs::{read_dir, File};
    use std::os::unix::io::AsRawFd;
    use std::process::Output;
    use std::str;
    use std::sync::Once;

    static SETUP: Once = Once::new();

    #[test]
    fn conflicting_mappings() {
        setup();

        let mut command = Command::new("ls");

        // The same mapping can't be included twice.
        assert_eq!(
            command.fd_mappings(vec![
                FdMapping {
                    child_fd: 4,
                    parent_fd: 5,
                },
                FdMapping {
                    child_fd: 4,
                    parent_fd: 5,
                },
            ]),
            Err(FdMappingCollision)
        );

        // Mapping two different FDs to the same FD isn't allowed either.
        assert_eq!(
            command.fd_mappings(vec![
                FdMapping {
                    child_fd: 4,
                    parent_fd: 5,
                },
                FdMapping {
                    child_fd: 4,
                    parent_fd: 6,
                },
            ]),
            Err(FdMappingCollision)
        );
    }

    #[test]
    fn no_mappings() {
        setup();

        let mut command = Command::new("ls");
        command.arg("/proc/self/fd");

        assert_eq!(command.fd_mappings(vec![]), Ok(()));

        let output = command.output().unwrap();
        expect_fds(&output, &[0, 1, 2, 3], 0);
    }

    #[test]
    fn one_mapping() {
        setup();

        let mut command = Command::new("ls");
        command.arg("/proc/self/fd");

        let file = File::open("testdata/file1.txt").unwrap();
        // Map the file an otherwise unused FD.
        assert_eq!(
            command.fd_mappings(vec![FdMapping {
                parent_fd: file.as_raw_fd(),
                child_fd: 5,
            },]),
            Ok(())
        );

        let output = command.output().unwrap();
        expect_fds(&output, &[0, 1, 2, 3, 5], 0);
    }

    #[test]
    fn swap_mappings() {
        setup();

        let mut command = Command::new("ls");
        command.arg("/proc/self/fd");

        let file1 = File::open("testdata/file1.txt").unwrap();
        let file2 = File::open("testdata/file2.txt").unwrap();
        let fd1 = file1.as_raw_fd();
        let fd2 = file2.as_raw_fd();
        // Map files to each other's FDs, to ensure that the temporary FD logic works.
        assert_eq!(
            command.fd_mappings(vec![
                FdMapping {
                    parent_fd: fd1,
                    child_fd: fd2,
                },
                FdMapping {
                    parent_fd: fd2,
                    child_fd: fd1,
                },
            ]),
            Ok(())
        );

        let output = command.output().unwrap();
        // Expect one more Fd for the /proc/self/fd directory. We can't predict what number it will
        // be assigned, because 3 might or might not be taken already by fd1 or fd2.
        expect_fds(&output, &[0, 1, 2, fd1, fd2], 1);
    }

    #[test]
    fn map_stdin() {
        setup();

        let mut command = Command::new("cat");

        let file = File::open("testdata/file1.txt").unwrap();
        // Map the file to stdin.
        assert_eq!(
            command.fd_mappings(vec![FdMapping {
                parent_fd: file.as_raw_fd(),
                child_fd: 0,
            },]),
            Ok(())
        );

        let output = command.output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"test 1");
    }

    /// Parse the output of ls into a set of filenames
    fn parse_ls_output(output: &[u8]) -> HashSet<String> {
        str::from_utf8(output)
            .unwrap()
            .split_terminator("\n")
            .map(str::to_owned)
            .collect()
    }

    /// Check that the output of `ls /proc/self/fd` contains the expected set of FDs, plus exactly
    /// `extra` extra FDs.
    fn expect_fds(output: &Output, expected_fds: &[RawFd], extra: usize) {
        assert!(output.status.success());
        let expected_fds: HashSet<String> = expected_fds.iter().map(RawFd::to_string).collect();
        let fds = parse_ls_output(&output.stdout);
        if extra == 0 {
            assert_eq!(fds, expected_fds);
        } else {
            assert!(expected_fds.is_subset(&fds));
            assert_eq!(fds.len(), expected_fds.len() + extra);
        }
    }

    fn setup() {
        SETUP.call_once(close_excess_fds);
    }

    /// Close all file descriptors apart from stdin, stdout and stderr.
    ///
    /// This is necessary because GitHub Actions opens a bunch of others for some reason.
    fn close_excess_fds() {
        let dir = read_dir("/proc/self/fd").unwrap();
        for entry in dir {
            let entry = entry.unwrap();
            let fd: RawFd = entry.file_name().to_str().unwrap().parse().unwrap();
            if fd > 3 {
                close(fd).unwrap();
            }
        }
    }
}
