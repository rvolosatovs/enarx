// SPDX-License-Identifier: Apache-2.0

pub use sallyport::libc;

use super::{syscall, Args, Result};

pub fn exit(status: i32) -> ! {
    syscall(
        libc::SYS_exit,
        Args {
            arg0: status as _,
            ..Default::default()
        },
    );
    unreachable!();
}

pub fn clock_gettime(clk_id: libc::clockid_t, tp: *mut libc::timespec) -> Result<()> {
    let ret = syscall(
        libc::SYS_clock_gettime,
        Args {
            arg0: clk_id as _,
            arg1: tp as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret == 0 {
        Ok(())
    } else {
        Err(-ret as i32)
    }
}

pub fn readv(fd: i32, iov: *const libc::iovec, iovcnt: i32) -> Result<isize> {
    let ret = syscall(
        libc::SYS_readv,
        Args {
            arg0: fd as _,
            arg1: iov as _,
            arg2: iovcnt as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret)
    } else {
        Err(-ret as i32)
    }
}

pub fn read(fd: i32, buf: *mut u8, count: usize) -> Result<isize> {
    let ret = syscall(
        libc::SYS_read,
        Args {
            arg0: fd as _,
            arg1: buf as _,
            arg2: count,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret)
    } else {
        Err(-ret as i32)
    }
}

pub fn write(fd: i32, buf: *const u8, count: usize) -> Result<isize> {
    let ret = syscall(
        libc::SYS_write,
        Args {
            arg0: fd as _,
            arg1: buf as _,
            arg2: count,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret)
    } else {
        Err(-ret as i32)
    }
}

pub fn close(fd: i32) -> Result<()> {
    let ret = syscall(
        libc::SYS_close,
        Args {
            arg0: fd as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret == 0 {
        Ok(())
    } else {
        Err(-ret as i32)
    }
}

pub fn is_enarx() -> bool {
    #[allow(non_upper_case_globals)]
    const SYS_fork: i64 = 57;

    let ret = syscall(SYS_fork, Args::default()).0 as i32;

    match -ret {
        0 => exit(0),
        libc::ENOSYS => true,
        _ => false,
    }
}

#[repr(u64)]
#[non_exhaustive]
#[derive(Debug, Copy, Clone)]
pub enum TeeTech {
    None = 0,
    Sev = 1,
    Sgx = 2,
}

pub struct TryFromIntError(pub(crate) ());

impl TryFrom<u64> for TeeTech {
    type Error = TryFromIntError;

    fn try_from(value: u64) -> core::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Sev),
            2 => Ok(Self::Sgx),
            _ => Err(TryFromIntError(())),
        }
    }
}

pub fn get_att(nonce: Option<&mut [u8]>, buf: Option<&mut [u8]>) -> Result<(usize, TeeTech)> {
    const SYS_GETATT: i64 = 0xEA01;

    let arg1 = if let Some(ref nonce) = nonce {
        nonce.len()
    } else {
        0usize
    };

    let arg0 = if let Some(nonce) = nonce {
        nonce.as_ptr() as usize
    } else {
        0usize
    };

    let arg3 = if let Some(ref buf) = buf {
        buf.len()
    } else {
        0usize
    };

    let arg2 = if let Some(buf) = buf {
        buf.as_mut_ptr() as usize
    } else {
        0usize
    };

    let (rax, rdx) = syscall(
        SYS_GETATT,
        Args {
            arg0,
            arg1,
            arg2,
            arg3,
            ..Default::default()
        },
    );

    let rax: isize = rax as _;

    if rax < 0 {
        return Err(-rax as _);
    }

    let tech = TeeTech::try_from(rdx as u64).map_err(|_| libc::EINVAL)?;

    Ok((rax as _, tech))
}

pub fn getuid() -> Result<libc::uid_t> {
    let ret = syscall(libc::SYS_getuid, Args::default()).0 as isize;
    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn geteuid() -> Result<libc::uid_t> {
    let ret = syscall(libc::SYS_geteuid, Args::default()).0 as isize;
    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn getgid() -> Result<libc::uid_t> {
    let ret = syscall(libc::SYS_getgid, Args::default()).0 as isize;
    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn getegid() -> Result<libc::uid_t> {
    let ret = syscall(libc::SYS_getegid, Args::default()).0 as isize;
    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn uname(buf: *mut libc::utsname) -> Result<()> {
    let ret = syscall(
        libc::SYS_uname,
        Args {
            arg0: buf as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret == 0 {
        Ok(())
    } else {
        Err(-ret as i32)
    }
}

pub fn socket(domain: i32, typ: i32, protocol: i32) -> Result<i32> {
    let ret = syscall(
        libc::SYS_socket,
        Args {
            arg0: domain as _,
            arg1: typ as _,
            arg2: protocol as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn bind(sockfd: i32, addr: *const libc::sockaddr, addrlen: libc::socklen_t) -> Result<i32> {
    let ret = syscall(
        libc::SYS_bind,
        Args {
            arg0: sockfd as _,
            arg1: addr as _,
            arg2: addrlen as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn listen(sockfd: i32, backlog: i32) -> Result<i32> {
    let ret = syscall(
        libc::SYS_listen,
        Args {
            arg0: sockfd as _,
            arg1: backlog as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn accept(
    sockfd: i32,
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
) -> Result<i32> {
    let ret = syscall(
        libc::SYS_accept,
        Args {
            arg0: sockfd as _,
            arg1: addr as _,
            arg2: addrlen as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn accept4(
    sockfd: i32,
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
    flags: i32,
) -> Result<i32> {
    let ret = syscall(
        libc::SYS_accept4,
        Args {
            arg0: sockfd as _,
            arg1: addr as _,
            arg2: addrlen as _,
            arg3: flags as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn connect(sockfd: i32, addr: *const libc::sockaddr, addrlen: libc::socklen_t) -> Result<i32> {
    let ret = syscall(
        libc::SYS_connect,
        Args {
            arg0: sockfd as _,
            arg1: addr as _,
            arg2: addrlen as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn recvfrom(
    sockfd: i32,
    buf: *mut u8,
    len: usize,
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
) -> Result<i32> {
    let ret = syscall(
        libc::SYS_recvfrom,
        Args {
            arg0: sockfd as _,
            arg1: buf as _,
            arg2: len as _,
            arg3: addr as _,
            arg4: addrlen as _,
            ..Default::default()
        },
    )
    .0 as isize;

    if ret >= 0 {
        Ok(ret as _)
    } else {
        Err(-ret as i32)
    }
}

pub fn random() -> u64 {
    let mut r: u64 = 0;

    for _ in 0..1024 {
        if unsafe { core::arch::x86_64::_rdrand64_step(&mut r) } == 1 {
            return r;
        }
    }

    panic!("Could not get random!")
}
