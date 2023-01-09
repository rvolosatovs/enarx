// SPDX-License-Identifier: Apache-2.0

use super::{check_output, enarx, CRATE, OUT_DIR, TEST_BINS_OUT};

use std::borrow::BorrowMut;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{
    Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6, TcpListener, TcpStream,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread::{self, sleep};
use std::time::Duration;

use anyhow::{ensure, Context};
use drawbridge_client::Url;
use process_control::Output;
use rustls::client::{ServerCertVerified, ServerCertVerifier};
use rustls::version::TLS13;
use rustls::Certificate;
use tempfile::{tempdir, NamedTempFile};

const STDIO_CONFIG: &str = r#"[stdin]
kind = "host"

[stdout]
kind = "host"

[stderr]
kind = "host""#;

#[cfg(enarx_with_shim)]
fn with_signatures(cmd: &mut Command) -> &mut Command {
    let sig = Path::new(OUT_DIR).join("sig.json");
    let outdated = || match sig.metadata().as_ref().map(fs::Metadata::modified) {
        Ok(Ok(modified))
            if modified
                >= fs::metadata(super::KEEP_BIN)
                    .expect("failed to query metadata of keep binary")
                    .modified()
                    .expect("failed to query modification time of keep binary") =>
        {
            // signature is already recent enough, no need to regenerate
            false
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            panic!(
                "failed to query metadata of signature file `{}`: {e}",
                sig.display()
            )
        }
        Ok(Err(e)) => {
            panic!(
                "failed to query modification time of signature file `{}`: {e}",
                sig.display()
            )
        }
        // signature is either missing or outdated
        _ => true,
    };
    let generate = || {
        let out = enarx(
            |cmd| {
                cmd.args([
                    "sign",
                    "--sgx-key",
                    "tests/data/sgx-test.key",
                    "--sev-id-key",
                    "tests/data/sev-id.key",
                    "--sev-id-key-signature",
                    "tests/data/sev-id-key-signature.blob",
                    "--out",
                ])
                .arg(&sig)
            },
            None,
        );
        if !out.status.success() {
            panic!(
                "failed to sign package: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        eprintln!("generated signature file `{}`", sig.display())
    };

    super::PathLock::new_in_out_dir("sig.lock")
        .once_if(outdated, generate, std::time::Duration::from_secs(30))
        .expect("failed to generate signature");
    cmd.arg("--signatures").arg(sig)
}

#[cfg(not(enarx_with_shim))]
fn with_signatures(cmd: &mut Command) -> &mut Command {
    cmd
}

pub fn enarx_run<'a>(
    wasm: &Path,
    conf: Option<&Path>,
    input: impl Into<Option<&'a [u8]>>,
) -> Output {
    enarx(
        |cmd| {
            let cmd = cmd.arg("run").arg(wasm);
            if let Some(conf) = conf {
                cmd.arg("--wasmcfgfile").arg(conf);
            }
            with_signatures(cmd)
        },
        input,
    )
}

pub fn enarx_deploy<'a>(url: &Url, input: impl Into<Option<&'a [u8]>>) -> Output {
    enarx(
        |cmd| {
            let cmd = cmd.arg("deploy").arg(url.as_str());
            with_signatures(cmd)
        },
        input,
    )
}

fn wasm_out() -> PathBuf {
    Path::new(OUT_DIR).join(TEST_BINS_OUT)
}

fn wasm_path(wasm: &str) -> PathBuf {
    wasm_out().join(wasm)
}

fn compile(wasm: &str) -> PathBuf {
    let wasm = wasm_path(wasm);

    let src_path = &Path::new(CRATE).join("tests/crates/enarx_wasm_tests/wat");
    let wat = src_path
        .join(wasm.file_stem().unwrap())
        .with_extension("wat");

    // poor mans `make`
    if wasm.exists() {
        let wasm_meta = wasm.metadata().unwrap();
        let wasm_time = wasm_meta.modified().unwrap();

        let wat_meta = wat.metadata().unwrap();
        let wat_time = wat_meta.modified().unwrap();

        if wasm_meta.len() > 0 && wasm_time > wat_time {
            // skip, if already compiled and newer than original
            return wasm;
        }
    }

    let out_dir = wasm_out();
    match fs::create_dir(&out_dir) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            eprintln!("Can't create {out_dir:#?} : {e:#?}");
            std::process::exit(1);
        }
        Ok(_) => {}
    }
    let bin = wat::parse_file(&wat).unwrap_or_else(|_| panic!("failed to compile {:?}", &wat));
    fs::write(&wasm, bin).unwrap_or_else(|_| panic!("failed to write {:?}", &wasm));
    wasm
}

#[test]
fn return_1() -> anyhow::Result<()> {
    // This module does, in fact, return 1. But function return values
    // are separate from setting the process exit status code, so
    // we still expect a return code of '0' here.
    let wasm = compile("return_1.wasm");
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(conf, "{STDIO_CONFIG}").context("failed to write config file")?;
    check_output(&enarx_run(&wasm, Some(conf.path()), None), 0, None, None);
    Ok(())
}

#[test]
fn wasi_snapshot1() -> anyhow::Result<()> {
    // This module uses WASI to return the number of commandline args.
    // Since we don't currently do anything with the function return value,
    // we don't get any output here, and we expect '0', as above.
    let wasm = compile("wasi_snapshot1.wasm");
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(conf, "{STDIO_CONFIG}").context("failed to write config file")?;
    check_output(&enarx_run(&wasm, Some(conf.path()), None), 0, None, None);
    Ok(())
}

#[test]
fn hello_wasi_snapshot1() -> anyhow::Result<()> {
    // This module just prints "Hello, world!" to stdout. Hooray!
    let wasm = compile("hello_wasi_snapshot1.wasm");
    const OUTPUT: &[u8] = br#"Hello, world!
"#;
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(conf, "{STDIO_CONFIG}").context("failed to write config file")?;
    check_output(&enarx_run(&wasm, Some(conf.path()), None), 0, OUTPUT, None);
    Ok(())
}

#[test]
fn no_export() -> anyhow::Result<()> {
    // This module has no exported functions, so we get an error.
    let wasm = compile("no_export.wasm");
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(conf, "{STDIO_CONFIG}").context("failed to write config file")?;
    check_output(&enarx_run(&wasm, Some(conf.path()), None), 1, None, None);
    Ok(())
}

#[test]
fn echo() -> anyhow::Result<()> {
    let wasm = wasm_path(env!("CARGO_BIN_FILE_ENARX_WASM_TESTS_echo"));

    let mut input: Vec<_> = Vec::with_capacity(2 * 1024 * 1024);
    for i in 0..input.capacity() {
        input.push(i as _);
    }
    let input = input.as_slice();
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(conf, "{STDIO_CONFIG}").context("failed to write config file")?;
    check_output(&enarx_run(&wasm, Some(conf.path()), input), 0, input, None);
    Ok(())
}

#[test]
fn memspike() -> anyhow::Result<()> {
    let wasm = wasm_path(env!("CARGO_BIN_FILE_ENARX_WASM_TESTS_memspike"));
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(conf, "{STDIO_CONFIG}").context("failed to write config file")?;
    check_output(&enarx_run(&wasm, Some(conf.path()), None), 0, None, None);
    Ok(())
}

#[test]
fn memory_stress_test() -> anyhow::Result<()> {
    let wasm = wasm_path(env!("CARGO_BIN_FILE_ENARX_WASM_TESTS_memory_stress_test"));
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(conf, "{STDIO_CONFIG}").context("failed to write config file")?;
    check_output(&enarx_run(&wasm, Some(conf.path()), None), 0, None, None);
    Ok(())
}

#[test]
fn zerooneone() -> anyhow::Result<()> {
    let wasm = wasm_path(env!("CARGO_BIN_FILE_ENARX_WASM_TESTS_zerooneone"));
    const INPUT: &[u8] = br#"Good morning, that's a nice tnetennba.
0118 999 881 999 119 725 3
"#;
    const OUTPUT: &[u8] = br#"Tbbq zbeavat, gung'f n avpr gargraaon.
0118 999 881 999 119 725 3
"#;

    check_output(&enarx_run(&wasm, None, INPUT), 0, OUTPUT, None);

    let url = Url::from_file_path(&wasm).expect("failed to construct a URL from path");
    check_output(&enarx_deploy(&url, INPUT), 0, OUTPUT, None);

    Ok(())
}

#[test]
fn zerooneone_pkg() -> anyhow::Result<()> {
    let wasm = wasm_path(env!("CARGO_BIN_FILE_ENARX_WASM_TESTS_zerooneone"));
    const INPUT: &[u8] = br#"Good morning, that's a nice tnetennba.
0118 999 881 999 119 725 3
"#;
    const OUTPUT: &[u8] = br#"Tbbq zbeavat, gung'f n avpr gargraaon.
0118 999 881 999 119 725 3
"#;

    // TODO: Extract the `enarx deploy` test into a separate test case or use `enarx deploy`
    // in all test cases.
    let pkg = tempdir().context("failed to create temporary package directory")?;
    let pkg_wasm = pkg.path().join("main.wasm");
    let pkg_conf = pkg.path().join("Enarx.toml");

    fs::copy(&wasm, pkg_wasm).context("failed to copy WASM module")?;
    fs::write(&pkg_conf, STDIO_CONFIG).context("failed to write config")?;

    check_output(&enarx_run(&wasm, Some(&pkg_conf), INPUT), 0, OUTPUT, None);

    let url = Url::from_file_path(&pkg).expect("failed to construct a URL from package path");
    check_output(&enarx_deploy(&url, None), 0, None, None);

    // TODO: Test execution from a remote HTTP(S) URL
    // https://github.com/enarx/enarx/issues/1855
    Ok(())
}

fn assert_copy_line(stream: &mut BufReader<impl Read + Write>) -> anyhow::Result<()> {
    writeln!(stream.get_mut(), "test").context("failed to write line")?;
    let mut line = String::new();
    stream.read_line(&mut line).context("failed to read line")?;
    ensure!(line == "test\n");
    ensure!(stream.buffer().is_empty());
    Ok(())
}

fn assert_stream<T: Read + Write>(mut stream: impl BorrowMut<T>) -> anyhow::Result<()> {
    let mut stream = BufReader::new(stream.borrow_mut());
    assert_copy_line(&mut stream).context("failed to copy first line")?;
    assert_copy_line(&mut stream).context("failed to copy second line")?;
    assert_copy_line(&mut stream).context("failed to copy third line")?;
    Ok(())
}

#[test]
fn connect_tcp() -> anyhow::Result<()> {
    let wasm = wasm_path(env!("CARGO_BIN_FILE_ENARX_WASM_TESTS_connect"));

    let listener =
        TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0)).context("failed to start TCP listener")?;
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(
        conf,
        r#"
args = ["{port}"]

{STDIO_CONFIG}

[network.outgoing."{}:{port}"]
prot = "tcp"

[network.outgoing."[{}]"]
prot = "tcp"

[network.outgoing.localhost]
prot = "tcp"
"#,
        Ipv4Addr::LOCALHOST,
        Ipv6Addr::LOCALHOST,
        port = listener
            .local_addr()
            .context("failed to query listener local address")?
            .port()
    )
    .context("failed to write config file")?;

    let server = thread::Builder::new()
        .name("server".into())
        .spawn(move || {
            eprintln!("[host] asserting IPv4 connectivity");
            let (stream, _) = listener
                .accept()
                .expect("failed to accept IPv4 connectivity");
            assert_stream(stream).expect("failed to assert IPv4 connectivity");

            eprintln!("[host] asserting IPv6 connectivity");
            let (stream, _) = listener
                .accept()
                .expect("failed to accept IPv6 connectivity");
            assert_stream(stream).expect("failed to assert IPv6 connectivity");

            eprintln!("[host] asserting localhost connectivity");
            let (stream, _) = listener
                .accept()
                .expect("failed to accept localhost connectivity");
            assert_stream(stream).expect("failed to assert localhost connectivity");
        })
        .expect("failed to start server thread");
    check_output(&enarx_run(&wasm, Some(conf.path()), None), 0, None, None);
    server.join().expect("failed to join server thread");
    Ok(())
}

// TODO: Reenable once there's functionality to configure trust anchors in Enarx.toml
// https://github.com/enarx/enarx/issues/2170 (which requires VFS)
//#[test]
//fn connect_tls() -> anyhow::Result<()> {
//    let wasm = wasm_path(env!("CARGO_BIN_FILE_ENARX_WASM_TESTS_connect"));
//
//    let listener =
//        TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).context("failed to start TCP listener")?;
//    let port = listener
//        .local_addr()
//        .context("failed to query listener local address")?
//        .port();
//
//    let mut conf = NamedTempFile::new().context("failed to create config file")?;
//    write!(
//        conf,
//        r#"[[files]]
//kind = "stdin"
//
//[[files]]
//kind = "stdout"
//
//[[files]]
//kind = "stderr"
//
//[[files]]
//kind = "connect"
//prot = "tls"
//host = "localhost"
//port = {port}
//name = "stream"
//ca = {{ custom = [ "test" ] }}
//
//[roots]
//test = """\
//{}\
//""""#,
//        include_str!("../../tests/data/tls/ca.crt")
//    )
//    .context("failed to write config file")?;
//
//    let certs = rustls_pemfile::certs(&mut BufReader::new(
//        include_bytes!("../../tests/data/tls/server.crt").as_slice(),
//    ))
//    .context("failed to read server TLS certificates")?
//    .into_iter()
//    .map(Certificate)
//    .collect();
//
//    let key = match rustls_pemfile::read_one(&mut BufReader::new(
//        include_bytes!("../../tests/data/tls/server.key").as_slice(),
//    ))
//    .context("failed to read server TLS certificate key")?
//    .context("server TLS certificate key missing")?
//    {
//        RSAKey(buf) | PKCS8Key(buf) | ECKey(buf) => PrivateKey(buf),
//        item => bail!("unsupported key type `{:?}`", item),
//    };
//
//    let tls = Arc::new(
//        rustls::ServerConfig::builder()
//            .with_safe_default_cipher_suites()
//            .with_safe_default_kx_groups()
//            .with_protocol_versions(&[&TLS13])
//            .context("failed to select TLS protocol versions")?
//            .with_no_client_auth() // TODO: Validate client cert
//            .with_single_cert(certs, key)
//            .context("invalid server TLS certificate key")?,
//    );
//
//    let server = thread::spawn(move || {
//        let (stream, _) = listener.accept().expect("failed to accept connection");
//        let tls = rustls::ServerConnection::new(tls).expect("failed to create TLS connection");
//        assert_stream(rustls::StreamOwned::new(tls, stream)).expect("failed to assert stream");
//    });
//    check_output(&enarx_run(&wasm, Some(conf.path()), None), 0, None, None);
//    server.join().expect("failed to join server thread");
//    Ok(())
//}

const CONNECTION_TIMEOUT: Duration = Duration::new(10, 0);

fn assert_connect<T: Read + Write>(connect: impl Fn() -> anyhow::Result<T>) -> anyhow::Result<()> {
    const RETRIES: u32 = 10;
    let retry_interval = CONNECTION_TIMEOUT / RETRIES;

    let connect = move || {
        for i in 0..RETRIES {
            eprintln!("[host] attempting to connect to the guest (try {i} out of {RETRIES})",);
            match connect() {
                Ok(ret) => return Ok(ret),
                Err(e) => {
                    eprintln!("[host] connection attempt {i} out of {RETRIES} failed with `{e}`, sleep for `{retry_interval:?}`")
                }
            }
            sleep(retry_interval)
        }
        connect()
    };

    eprintln!("[host] establishing first connection");
    connect()
        .context("failed to establish first connection")
        .and_then(|stream| assert_stream(stream).context("failed to assert first stream"))?;

    eprintln!("[host] establishing second connection");
    connect()
        .context("failed to establish second connection")
        .and_then(|stream| assert_stream(stream).context("failed to assert second stream"))?;

    eprintln!("[host] establishing third connection");
    connect()
        .context("failed to establish third connection")
        .and_then(|stream| assert_stream(stream).context("failed to assert third stream"))?;

    Ok(())
}

fn connect_ipv4(port: u16) -> anyhow::Result<TcpStream> {
    TcpStream::connect_timeout(
        &SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)),
        CONNECTION_TIMEOUT,
    )
    .context("failed to connect to TCP socket via IPv4")
}

fn connect_ipv6(port: u16) -> anyhow::Result<TcpStream> {
    TcpStream::connect_timeout(
        &SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, port, 0, 0)),
        CONNECTION_TIMEOUT,
    )
    .context("failed to connect to TCP socket via IPv6")
}

#[test]
#[cfg_attr(windows, ignore = "listener tests hang on Windows")]
fn listen_tcp() -> anyhow::Result<()> {
    let wasm = wasm_path(env!("CARGO_BIN_FILE_ENARX_WASM_TESTS_listen"));

    let listener =
        TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0)).context("failed to start TCP listener")?;
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(
        conf,
        r#"
args = ["{port}"]

{STDIO_CONFIG}

[network.incoming.default]
prot = "tcp"

[network.outgoing."{}"]
prot = "tcp"

[network.outgoing."[{}]:{port}"]
prot = "tcp"
"#,
        Ipv4Addr::LOCALHOST,
        Ipv6Addr::LOCALHOST,
        port = listener
            .local_addr()
            .context("failed to query listener local address")?
            .port()
    )
    .context("failed to write config file")?;

    let client = thread::Builder::new()
        .name("client".into())
        .spawn(move || {
            eprintln!("[host] waiting for workload to start");
            let (mut stream, _) = listener.accept().expect("failed to accept connection");
            eprintln!("[host] workload has started (ping received)");

            let port = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
                .expect("failed to start TCP listener")
                .local_addr()
                .expect("failed to query listener local address")
                .port();

            eprintln!("[host] sending listening port");
            stream
                .write_all(format!("{port}").as_bytes())
                .expect("failed to send listening port");

            eprintln!("[host] shutting down initial stream");
            stream
                .shutdown(Shutdown::Both)
                .expect("failed to shutdown initial stream");

            eprintln!("[host] asserting IPv4 connectivity");
            assert_connect(|| connect_ipv4(port))
                .expect("failed to assert TCP connection via IPv4");

            eprintln!("[host] waiting for guest listener to be closed");
            _ = listener.accept().expect("failed to accept connection");
            eprintln!("[host] guest listener is closed");

            eprintln!("[host] asserting IPv6 connectivity");
            assert_connect(|| connect_ipv6(port))
                .expect("failed to assert TCP connection via IPv6");
        })
        .expect("failed to start client thread");
    check_output(&enarx_run(&wasm, Some(conf.path()), None), 0, None, None);
    client.join().expect("failed to join client thread");
    Ok(())
}

struct NoopCertVerifier;

impl ServerCertVerifier for NoopCertVerifier {
    fn verify_server_cert(
        &self,
        _: &Certificate,
        _: &[Certificate],
        _: &rustls::ServerName,
        _: &mut dyn Iterator<Item = &[u8]>,
        _: &[u8],
        _: std::time::SystemTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
}

#[test]
#[cfg_attr(windows, ignore = "listener tests hang on Windows")]
fn listen_tls() -> anyhow::Result<()> {
    let wasm = wasm_path(env!("CARGO_BIN_FILE_ENARX_WASM_TESTS_listen"));

    let listener =
        TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0)).context("failed to start TCP listener")?;
    let mut conf = NamedTempFile::new().context("failed to create config file")?;
    write!(
        conf,
        r#"
args = ["{port}"]

{STDIO_CONFIG}

[network.outgoing."{}"]
prot = "tcp"

[network.outgoing."[{}]:{port}"]
prot = "tcp"
"#,
        Ipv4Addr::LOCALHOST,
        Ipv6Addr::LOCALHOST,
        port = listener
            .local_addr()
            .context("failed to query listener local address")?
            .port()
    )
    .context("failed to write config file")?;

    let client = thread::Builder::new()
        .name("client".into())
        .spawn(move || {
            let connect_stream = |stream| {
                let name = "localhost".try_into().unwrap();
                let conf: Arc<_> = rustls::ClientConfig::builder()
                    .with_safe_default_cipher_suites()
                    .with_safe_default_kx_groups()
                    .with_protocol_versions(&[&TLS13])
                    .expect("failed to select TLS protocol versions")
                    .with_custom_certificate_verifier(Arc::new(NoopCertVerifier))
                    .with_no_client_auth()
                    .into();
                let conn = rustls::ClientConnection::new(conf, name)
                    .context("failed to create TLS connection")?;
                Ok(rustls::StreamOwned::new(conn, stream))
            };

            eprintln!("[host] waiting for workload to start");
            let (mut stream, _) = listener.accept().expect("failed to accept connection");
            eprintln!("[host] workload has started (ping received)");

            let port = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
                .expect("failed to start TCP listener")
                .local_addr()
                .expect("failed to query listener local address")
                .port();

            eprintln!("[host] sending listening port");
            stream
                .write_all(format!("{port}").as_bytes())
                .expect("failed to send listening port");

            eprintln!("[host] shutting down initial stream");
            stream
                .shutdown(Shutdown::Both)
                .expect("failed to shutdown initial stream");

            eprintln!("[host] asserting IPv4 connectivity");
            assert_connect(|| connect_ipv4(port).and_then(connect_stream))
                .expect("failed to assert IPv4 connectivity");

            eprintln!("[host] waiting for guest listener to be closed");
            _ = listener.accept().expect("failed to accept connection");
            eprintln!("[host] guest listener is closed");

            eprintln!("[host] asserting IPv6 connectivity");
            assert_connect(|| connect_ipv6(port).and_then(connect_stream))
                .expect("failed to assert IPv6 connectivity");
        })
        .expect("failed to spawn client thread");
    check_output(&enarx_run(&wasm, Some(conf.path()), None), 0, None, None);
    client.join().expect("failed to join client thread");
    Ok(())
}
