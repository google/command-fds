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

use nix::fcntl::{fcntl, FcntlArg};
use nix::unistd::dup2;
use std::cmp::max;
use std::io::{self, ErrorKind};
use std::os::unix::io::RawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FdMapping {
    pub parent_fd: RawFd,
    pub child_fd: RawFd,
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

fn nix_to_io_error(error: nix::Error) -> io::Error {
    if let nix::Error::Sys(errno) = error {
        io::Error::from_raw_os_error(errno as i32)
    } else {
        io::Error::new(ErrorKind::Other, error)
    }
}

pub trait CommandFdExt {
    fn fd_mappings(&mut self, mappings: Vec<FdMapping>);
}

impl CommandFdExt for Command {
    fn fd_mappings(&mut self, mappings: Vec<FdMapping>) {
        unsafe {
            self.pre_exec(move || map_fds(&mappings));
        }
    }
}
