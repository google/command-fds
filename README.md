# command-fds

[![crates.io page](https://img.shields.io/crates/v/command-fds.svg)](https://crates.io/crates/command-fds)
[![docs.rs page](https://docs.rs/command-fds/badge.svg)](https://docs.rs/command-fds)

A library for passing arbitrary file descriptors when spawning child processes, and safely taking
ownership of passed file descriptors within such a child process.

## Example

In the parent process:

```rust
use command_fds::{CommandFdExt, FdMapping};
use std::fs::File;
use std::io::stdin;
use std::os::fd::AsFd;
use std::os::unix::io::AsRawFd;
use std::process::Command;

// Open a file.
let file = File::open("Cargo.toml").unwrap();

// Prepare to run `ls -l /proc/self/fd` with some FDs mapped.
let mut command = Command::new("ls");
command.arg("-l").arg("/proc/self/fd");
command
    .fd_mappings(vec![
        // Map `file` as FD 3 in the child process.
        FdMapping {
            parent_fd: file.into(),
            child_fd: 3,
        },
        // Map this process's stdin as FD 5 in the child process.
        FdMapping {
            parent_fd: stdin().as_fd().try_clone_to_owned().unwrap(),
            child_fd: 5,
        },
    ])
    .unwrap();

// Spawn the child process.
let mut child = command.spawn().unwrap();
child.wait().unwrap();
```

In the child process:

```rust
use command_fds::inherited::{init_inherited_fds, take_fd_ownership};

fn main() {
    // SAFETY: This is called before anything else in the program.
    unsafe {
        init_inherited_fds();
    }

    // Get an OwnedFd for the file that was passed as FD 3 by the parent.
    let inherited_file = take_fd_ownership(3).unwrap();

    // Trying to take the same file descriptor again will return an error.
    take_fd_ownership(3).expect_err("Can't take the same FD twice");

    // Trying to take a file descriptor which wasn't passed will also return an error.
    take_fd_ownership(4).expect_err("Can't take an FD which wasn't inherited");
}
```

## License

Licensed under the [Apache License, Version 2.0](http://www.apache.org/licenses/LICENSE-2.0).

## Contributing

If you want to contribute to the project, see details of
[how we accept contributions](CONTRIBUTING.md).
