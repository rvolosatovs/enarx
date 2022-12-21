// SPDX-License-Identifier: Apache-2.0

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

enarx_syscall_tests::startup!();

#[cfg(target_vendor = "unknown")]
fn main() -> enarx_syscall_tests::Result<()> {
    use enarx_syscall_tests::*;

    let out = b"hi\n";
    let len = write(libc::STDOUT_FILENO, out.as_ptr(), out.len())?;
    if len as usize == out.len() {
        Ok(())
    } else {
        Err(1)
    }
}

#[cfg(not(target_vendor = "unknown"))]
fn main() {
    panic!("unsupported on this target")
}
