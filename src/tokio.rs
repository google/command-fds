use std::os::fd::OwnedFd;
use tokio::process::Command;
use tokio_crate as tokio;

use crate::{map_fds, preserve_fds, validate_child_fds, FdMapping, FdMappingCollision};

/// Extension to add file descriptor mappings to a [`Command`].
pub trait CommandFdAsyncExt {
    /// Adds the given set of file descriptors to the command.
    ///
    /// Warning: Calling this more than once on the same command may result in unexpected behaviour.
    /// In particular, it is not possible to check that two mappings applied separately don't use
    /// the same `child_fd`. If there is such a collision then one will apply and the other will be
    /// lost.
    fn fd_mappings(&mut self, mappings: Vec<FdMapping>) -> Result<&mut Self, FdMappingCollision>;

    /// Adds the given set of file descriptors to be passed on to the child process when the command
    /// is run.
    fn preserved_fds(&mut self, fds: Vec<OwnedFd>) -> &mut Self;
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

    fn preserved_fds(&mut self, fds: Vec<OwnedFd>) -> &mut Self {
        unsafe {
            self.pre_exec(move || preserve_fds(&fds));
        }

        self
    }
}
