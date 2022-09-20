// SPDX-License-Identifier: Apache-2.0

//! A file system containing listening networking sockets.

use super::super::net::tls;
use super::super::WasiResult;

use std::any::Any;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::ops::Deref;
use std::path::MAIN_SEPARATOR;
use std::sync::Arc;

use anyhow::Context;
use cap_std::net::TcpListener;
use enarx_config::{FileName, ListenFile};
use rustls::{Certificate, PrivateKey};
use wasi_common::file::{FdFlags, FileType};
use wasi_common::{Error, ErrorExt, WasiDir, WasiFile};
use wasmtime_vfs_dir::Directory;
use wasmtime_vfs_ledger::InodeId;
use wasmtime_vfs_memory::{Data, Inode, Link, Node};
use wiggle::async_trait;
use zeroize::Zeroizing;

enum Socket {
    Tcp(Link<()>),
    Tls(Link<Arc<rustls::ServerConfig>>),
}

#[async_trait]
impl Node for Socket {
    fn to_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn parent(&self) -> Option<Arc<dyn Node>> {
        match self {
            Self::Tcp(Link { parent, .. }) | Self::Tls(Link { parent, .. }) => parent.upgrade(),
        }
    }

    fn filetype(&self) -> FileType {
        FileType::SocketStream
    }

    fn id(&self) -> Arc<InodeId> {
        match self {
            Self::Tcp(Link { inode, .. }) => inode.id.clone(),
            Self::Tls(Link { inode, .. }) => inode.id.clone(),
        }
    }

    async fn open_dir(self: Arc<Self>) -> WasiResult<Box<dyn WasiDir>> {
        Err(Error::not_dir())
    }

    async fn open_file(
        self: Arc<Self>,
        path: &str,
        dir: bool,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> WasiResult<Box<dyn WasiFile>> {
        if dir {
            return Err(Error::not_dir());
        }

        if !read || !write {
            return Err(Error::perm()); // FIXME(@npmccallum): errno
        }

        let addr = path
            .rsplit_terminator(MAIN_SEPARATOR)
            .next()
            .ok_or_else(|| Error::invalid_argument().context("failed to parse file name"))?;
        let (host, port) = addr
            .split_once(':')
            .map(|(host, addr)| (Some(host), addr))
            .unwrap_or((None, addr));
        let port = port
            .parse()
            .map_err(|e| Error::invalid_argument().context(e))
            .context("failed to parse port `{port}`")?;
        let tcp = match (host, port) {
            (Some("localhost" | "127.0.0.1"), port) => std::net::TcpListener::bind(SocketAddr::V4(
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            )),
            (None | Some("0.0.0.0"), port) => std::net::TcpListener::bind(SocketAddr::V4(
                SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port),
            )),
            (Some(host), port) => std::net::TcpListener::bind((host, port)),
        }
        .map(TcpListener::from_std)
        .map_err(|e| Error::io().context(e).context("failed to bind to socket"))?;

        if flags == FdFlags::NONBLOCK {
            tcp.set_nonblocking(true)
                .context("failed to enable NONBLOCK")?;
        } else if flags.is_empty() {
            tcp.set_nonblocking(false)
                .context("failed to disable NONBLOCK")?;
        } else {
            return Err(
                Error::invalid_argument().context("cannot set anything other than NONBLOCK")
            );
        }
        match self.as_ref() {
            Self::Tcp(..) => Ok(wasmtime_wasi::net::Socket::from(tcp).into()),
            Self::Tls(Link { inode, .. }) => {
                Ok(tls::Listener::new(tcp, inode.data.read().await.content.clone()).into())
            }
        }
    }
}
pub async fn new(
    parent: Arc<dyn Node>,
    certs: Arc<Vec<Certificate>>,
    key: Arc<Zeroizing<Vec<u8>>>,
    sockets: impl IntoIterator<Item = (FileName, ListenFile)>,
) -> anyhow::Result<Arc<dyn Node>> {
    let certs = certs.deref().clone();
    let key = PrivateKey(key.deref().deref().clone());
    let tls_config = rustls::ServerConfig::builder()
        .with_cipher_suites(tls::DEFAULT_CIPHER_SUITES.deref())
        .with_kx_groups(tls::DEFAULT_KX_GROUPS.deref())
        .with_protocol_versions(tls::DEFAULT_PROTOCOL_VERSIONS.deref())?
        .with_no_client_auth() // TODO: https://github.com/enarx/enarx/issues/1547
        .with_single_cert(certs, key)
        .map(Arc::new)
        .context("failed to construct TLS config")?;
    let dir = Directory::device(parent, {
        let tls_config = tls_config.clone();
        Some(Arc::new(move |parent: Arc<dyn Node>| {
            let id = parent.id().device().create_inode();
            let parent = Arc::downgrade(&parent);
            let data = Data::from(tls_config.clone()).into();
            let inode = Inode { data, id }.into();
            Arc::new(Socket::Tls(Link { parent, inode }))
        }))
    });
    for (name, file) in sockets {
        let parent = Arc::downgrade(&(dir.clone() as Arc<dyn Node>));
        let id = dir.id().device().create_inode();
        let file = match file {
            ListenFile::Tcp => {
                let data = Data::from(()).into();
                let inode = Inode { data, id }.into();
                Arc::new(Socket::Tcp(Link { parent, inode }))
            }
            ListenFile::Tls => {
                let data = Data::from(tls_config.clone()).into();
                let inode = Inode { data, id }.into();
                Arc::new(Socket::Tls(Link { parent, inode }))
            }
        };
        dir.attach(name.as_str(), file)
            .await
            .context("failed to attach socket to directory")?;
    }
    Ok(dir)
}
