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

use command_fds::{set_mappings, FdMapping};
use std::fs::{read_dir, read_link, File};
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

fn list_fds() {
    let dir = read_dir("/proc/self/fd").unwrap();
    for entry in dir {
        let entry = entry.unwrap();
        let target = read_link(entry.path()).unwrap();
        println!("{:?} {:?}", entry, target);
    }
}

fn main() {
    list_fds();

    let file = File::open("file.txt").unwrap();
    println!("File: {:?}", file);
    list_fds();

    let mut command = Command::new("ls");
    command.arg("-l").arg("/proc/self/fd");
    let mappings = vec![
        FdMapping {
            old_fd: file.as_raw_fd(),
            new_fd: 3,
        },
        FdMapping {
            old_fd: 0,
            new_fd: 5,
        },
    ];
    set_mappings(&mut command, mappings);
    unsafe {
        command.pre_exec(move || {
            let fd = file.as_raw_fd();
            println!("pre_exec, file {:?}, fd {}", file, fd);
            list_fds();
            Ok(())
        });
    }

    println!("Spawning command");
    let mut child = command.spawn().unwrap();
    sleep(Duration::from_millis(100));
    println!("Spawned");
    list_fds();

    println!("Waiting for command");
    println!("{:?}", child.wait().unwrap());
}
