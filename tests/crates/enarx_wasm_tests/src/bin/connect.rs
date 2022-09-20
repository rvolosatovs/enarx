// SPDX-License-Identifier: Apache-2.0

#![cfg_attr(target_os = "wasi", feature(wasi_ext))]

use enarx_wasm_tests::assert_stream;

use std::fs::{read_dir, File};
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::io::OwnedFd;
#[cfg(target_os = "wasi")]
use std::os::wasi::io::OwnedFd;

use anyhow::{anyhow, ensure, Context};

fn main() -> anyhow::Result<()> {
    let mut con = read_dir("/net/con").context("failed to list connected streams")?;
    let stream_pre = con
        .next()
        .ok_or_else(|| anyhow!("no stream found"))?
        .context("failed to acquire directory entry")?;
    let mut stream_pre = File::options()
        .read(true)
        .write(true)
        .open(stream_pre.path())
        .map(OwnedFd::from)
        .map(TcpStream::from)
        .context("failed to open preconfigured stream")?;
    ensure!(con.next().is_none(), "more than one stream present");

    let mut addr = format!("localhost:");
    BufReader::new(&mut stream_pre)
        .read_line(&mut addr)
        .context("failed to read runtime connection port")?;
    let addr = addr.trim();
    eprintln!("connecting to `{addr}`...");
    let stream_run = File::options()
        .read(true)
        .write(true)
        .create(true)
        .open(format!("/net/con/{addr}"))
        .map(OwnedFd::from)
        .map(TcpStream::from)
        .context("failed to open runtime stream")?;

    assert_stream(stream_pre).context("failed to assert preconfigured stream")?;
    assert_stream(stream_run).context("failed to assert runtime stream")?;
    Ok(())
}
