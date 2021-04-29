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

use command_fds::{CommandFdExt, FdMapping};
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::process::Command;

fn main() {
    // Open a file.
    let file = File::open("Cargo.toml").unwrap();

    // Prepare to run `ls -l /proc/self/fd` with some FDs mapped.
    let mut command = Command::new("ls");
    command.arg("-l").arg("/proc/self/fd");
    command
        .fd_mappings(vec![
            // Map `file` as FD 3 in the child process.
            FdMapping {
                parent_fd: file.as_raw_fd(),
                child_fd: 3,
            },
            // Map this process's stdin as FD 5 in the child process.
            FdMapping {
                parent_fd: 0,
                child_fd: 5,
            },
        ])
        .unwrap();

    // Spawn the child process.
    let mut child = command.spawn().unwrap();
    child.wait().unwrap();
}
