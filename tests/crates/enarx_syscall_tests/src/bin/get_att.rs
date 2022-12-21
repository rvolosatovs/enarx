// SPDX-License-Identifier: Apache-2.0

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

enarx_syscall_tests::startup!();

#[cfg(target_vendor = "unknown")]
fn main() -> enarx_syscall_tests::Result<()> {
    use enarx_syscall_tests::*;

    if !is_enarx() {
        Ok(())
    } else {
        get_att(None, None)?;

        Ok(())
    }
}

#[cfg(not(target_vendor = "unknown"))]
fn main() {
    panic!("unsupported on this target")
}
