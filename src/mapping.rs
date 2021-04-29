use nix::fcntl::{fcntl, FcntlArg};
use nix::unistd::dup2;
use std::cmp::max;
use std::io::{self, ErrorKind};
use std::os::unix::io::RawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FdMapping {
    pub old_fd: RawFd,
    pub new_fd: RawFd,
}

fn map_fds(mappings: &[FdMapping]) -> io::Result<()> {
    if mappings.is_empty() {
        // No need to do anything, and finding first_unused_fd would fail.
        return Ok(());
    }

    // Find the first FD which is higher than any old or new FD in the mapping, so we can safely use
    // it and higher FDs as temporary FDs. There may be other files open with these FDs, so we still
    // need to ensure we don't conflict with them.
    let first_safe_fd = mappings
        .iter()
        .map(|mapping| max(mapping.old_fd, mapping.new_fd))
        .max()
        .unwrap()
        + 1;

    // If any old FDs conflict with new FDs, then first duplicate them to a temporary FD which is
    // clear of either range.
    let new_fds: Vec<RawFd> = mappings.iter().map(|mapping| mapping.new_fd).collect();
    let mappings = mappings
        .into_iter()
        .map(|mapping| {
            Ok(if new_fds.contains(&mapping.old_fd) {
                let temporary_fd = fcntl(mapping.old_fd, FcntlArg::F_DUPFD_CLOEXEC(first_safe_fd))?;
                FdMapping {
                    old_fd: temporary_fd,
                    new_fd: mapping.new_fd,
                }
            } else {
                mapping.to_owned()
            })
        })
        .collect::<nix::Result<Vec<_>>>()
        .map_err(nix_to_io_error)?;

    // Now we can actually duplicate FDs to the desired new FDs.
    for mapping in mappings {
        // This closes new_fd if it is already open as something else, and clears the FD_CLOEXEC
        // flag on new_fd.
        dup2(mapping.old_fd, mapping.new_fd).map_err(nix_to_io_error)?;
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

pub fn set_mappings(command: &mut Command, mappings: Vec<FdMapping>) {
    unsafe {
        command.pre_exec(move || {
            map_fds(&mappings)?;
            Ok(())
        });
    }
}
