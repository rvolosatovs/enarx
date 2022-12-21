// SPDX-License-Identifier: Apache-2.0

#![no_std]

#[cfg(target_vendor = "unknown")]
pub mod io;

#[cfg(target_vendor = "unknown")]
mod macros;
#[cfg(target_vendor = "unknown")]
pub use macros::*;

#[cfg(target_vendor = "unknown")]
mod syscalls;
#[cfg(target_vendor = "unknown")]
pub use syscalls::*;

use core::arch::asm;

pub type Result<T> = core::result::Result<T, i32>;

#[macro_export]
macro_rules! startup {
    () => {
        #[cfg(target_os = "none")]
        fn __start_inner() -> ! {
                use $crate::{exit, Termination};
                exit(main().report().to_i32())
        }
        #[cfg(target_os = "none")]
        core::arch::global_asm!(
                ".pushsection .text.startup,\"ax\",@progbits",
                ".global _start",
                "_start:",
                "lea    rdi, [rip + _DYNAMIC]",
                "mov    rsi, rsp",
                "lea    rdx, [rip + {INNER}]",
                "jmp   {RCRT}",

                RCRT = sym rcrt1::rcrt,
                INNER = sym __start_inner,
        );

        #[cfg(target_os = "none")]
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
            use $crate::{eprintln, exit};
            eprintln!("{}\n", info);
            exit(255)
        }
    };
}

/// Termination
pub trait Termination {
    /// Is called to get the representation of the value as status code.
    /// This status code is returned to the operating system.
    fn report(self) -> ExitCode;
}

impl Termination for () {
    #[inline]
    fn report(self) -> ExitCode {
        ExitCode::SUCCESS.report()
    }
}

impl Termination for ExitCode {
    #[inline]
    fn report(self) -> ExitCode {
        self
    }
}

impl<E: core::fmt::Debug> Termination for core::result::Result<(), E> {
    fn report(self) -> ExitCode {
        match self {
            Ok(()) => ().report(),
            Err(err) => {
                #[cfg(target_vendor = "unknown")]
                eprintln!("Error: {:?}", err);
                ExitCode::FAILURE.report()
            }
        }
    }
}

/// The ExitCode
pub struct ExitCode(i32);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);
}

impl ExitCode {
    #[inline]
    pub fn to_i32(self) -> i32 {
        self.0
    }
}

impl From<u8> for ExitCode {
    /// Construct an exit code from an arbitrary u8 value.
    fn from(code: u8) -> Self {
        ExitCode(code as _)
    }
}

#[derive(Default)]
pub struct Args {
    pub arg0: usize,
    pub arg1: usize,
    pub arg2: usize,
    pub arg3: usize,
    pub arg4: usize,
    pub arg5: usize,
}

pub fn syscall(nr: i64, args: Args) -> (usize, usize) {
    let rax: usize;
    let rdx: usize;
    unsafe {
        asm!(
        "syscall",
        inlateout("rax") nr as usize => rax,
        in("rdi") args.arg0,
        in("rsi") args.arg1,
        inlateout("rdx") args.arg2 => rdx,
        in("r10") args.arg3,
        in("r8") args.arg4,
        in("r9") args.arg5,
        lateout("rcx") _, // clobbered
        lateout("r11") _, // clobbered
        );
    }
    (rax, rdx)
}
