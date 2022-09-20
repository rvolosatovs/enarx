// SPDX-License-Identifier: Apache-2.0

#![cfg_attr(target_os = "wasi", feature(wasi_ext))]

use enarx_wasm_tests::assert_stream;

use std::borrow::BorrowMut;
use std::fs::{read_dir, File};
use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::io::OwnedFd;
#[cfg(target_os = "wasi")]
use std::os::wasi::io::OwnedFd;

use anyhow::{anyhow, bail, ensure, Context};

fn assert_listener(mut listener: impl BorrowMut<TcpListener>) -> anyhow::Result<()> {
    let listener = listener.borrow_mut();

    let (stream, _) = listener
        .accept()
        .context("failed to accept first connection")?;
    assert_stream(stream).context("failed to assert default stream")?;

    listener
        .set_nonblocking(false)
        .context("failed to unset NONBLOCK")?;
    let (stream, _) = listener
        .accept()
        .context("failed to accept second connection")?;
    assert_stream(stream).context("failed to assert blocking stream")?;

    listener
        .set_nonblocking(true)
        .context("failed to set NONBLOCK")?;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                assert_stream(stream).context("failed to assert non-blocking stream")?;
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => bail!(anyhow!(e).context("failed to accept third connection")),
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut con = read_dir("/net/con").context("failed to list connected streams")?;
    let stream = con
        .next()
        .ok_or_else(|| anyhow!("no stream found"))?
        .context("failed to acquire directory entry")?;
    ensure!(con.next().is_none(), "more than one stream present");

    let mut lis = read_dir("/net/lis").context("failed to list listening sockets")?;
    let listener_pre = lis
        .next()
        .ok_or_else(|| anyhow!("no listener found"))?
        .context("failed to acquire directory entry")?;
    let listener_pre = File::options()
        .read(true)
        .write(true)
        .open(listener_pre.path())
        .map(OwnedFd::from)
        .map(TcpListener::from)
        .context("failed to open preconfigured listener")?;
    ensure!(lis.next().is_none(), "more than one listener present");

    let mut addr = String::new();
    _ = File::options()
        .read(true)
        .write(true)
        .open(stream.path())
        .map(OwnedFd::from)
        .map(TcpStream::from)
        .context("failed to open stream")?
        .read_to_string(&mut addr)
        .context("failed to read runtime listening address")?;
    let addr = addr.trim();
    eprintln!("listening on `{addr}`...");
    let listener_run = File::options()
        .read(true)
        .write(true)
        .create(true)
        .open(format!("/net/lis/{addr}"))
        .map(OwnedFd::from)
        .map(TcpListener::from)
        .context("failed to open runtime listener")?;

    assert_listener(listener_pre).context("failed to assert preconfigured listener")?;
    assert_listener(listener_run).context("failed to assert runtime listener")?;
    Ok(())
}
