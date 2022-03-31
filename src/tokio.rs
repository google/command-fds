use std::os::unix::prelude::RawFd;

use tokio::process::Command;
use tokio_crate as tokio;

use crate::{map_fds, preserve_fds, validate_child_fds, FdMapping, FdMappingCollision};

/// Extension to add file descriptor mappings to a [`Command`].
pub trait CommandFdAsyncExt {
    /// Adds the given set of file descriptors to the command.
    ///
    /// Warning: Calling this more than once on the same command, or attempting to run the same
    /// command more than once after calling this, may result in unexpected behaviour.
    fn fd_mappings(&mut self, mappings: Vec<FdMapping>) -> Result<&mut Self, FdMappingCollision>;

    /// Adds the given set of file descriptors to be passed on to the child process when the command
    /// is run.
    fn preserved_fds(&mut self, fds: Vec<RawFd>) -> &mut Self;
}

impl CommandFdAsyncExt for Command {
    fn fd_mappings(
        &mut self,
        mut mappings: Vec<FdMapping>,
    ) -> Result<&mut Self, FdMappingCollision> {
        let child_fds = validate_child_fds(&mappings)?;

        unsafe {
            self.pre_exec(move || map_fds(&mut mappings, &child_fds));
        }

        Ok(self)
    }

    fn preserved_fds(&mut self, fds: Vec<RawFd>) -> &mut Self {
        unsafe {
            self.pre_exec(move || preserve_fds(&fds));
        }

        self
    }
}
