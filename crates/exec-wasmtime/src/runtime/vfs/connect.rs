// SPDX-License-Identifier: Apache-2.0

//! A file system providing outgoing network connectivity.

use super::super::net::tls;
use super::super::WasiResult;

use std::any::Any;
use std::fmt::Debug;
use std::io::{IoSlice, IoSliceMut, SeekFrom};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use cap_std::net::TcpStream;
use enarx_config::OutgoingNetwork;
use rustls::{Certificate, PrivateKey, RootCertStore};
use url::Host;
use wasi_common::dir::{ReaddirCursor, ReaddirEntity};
use wasi_common::file::{Advice, FdFlags, FileType, Filestat, OFlags};
use wasi_common::{Error, ErrorExt, SystemTimeSpec, WasiDir, WasiFile};
use wasmtime_vfs_ledger::InodeId;
use wasmtime_vfs_memory::{Data, Inode, Link, Node, Open, State};
use wiggle::async_trait;
use wiggle::tracing::instrument;
use zeroize::Zeroizing;

// NOTE: Directory-specific functionality is duplicated from `wasmtime_vfs_dir::Directory`
// implementation. There should be a better API provided to handle this.

#[derive(Debug)]
pub struct ConnectState {
    policy: OutgoingNetwork,
    tls_config: Arc<rustls::ClientConfig>,
}

pub struct Connect(Link<ConnectState>);

impl Connect {
    #[instrument(skip(parent))]
    pub async fn new(
        parent: Arc<dyn Node>,
        certs: Arc<Vec<Certificate>>,
        key: Arc<Zeroizing<Vec<u8>>>,
        policy: OutgoingNetwork,
    ) -> anyhow::Result<Arc<dyn Node>> {
        let certs = certs.deref().clone();
        let key = PrivateKey(key.deref().deref().clone());
        let mut server_roots = RootCertStore::empty();
        server_roots.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
            rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                ta.subject,
                ta.spki,
                ta.name_constraints,
            )
        }));
        let tls_config = rustls::ClientConfig::builder()
            .with_cipher_suites(tls::DEFAULT_CIPHER_SUITES.deref())
            .with_kx_groups(tls::DEFAULT_KX_GROUPS.deref())
            .with_protocol_versions(tls::DEFAULT_PROTOCOL_VERSIONS.deref())?
            .with_root_certificates(server_roots)
            .with_single_cert(certs, key)
            .map(Arc::new)
            .context("failed to construct TLS config")?;
        let id = parent.id().device().create_inode();
        let parent = Arc::downgrade(&parent);
        let data = Data::from(ConnectState { tls_config, policy }).into();
        let inode = Inode { data, id }.into();
        Ok(Arc::new(Self(Link { parent, inode })))
    }

    fn prev(self: &Arc<Self>) -> Arc<dyn Node> {
        match self.parent.upgrade() {
            Some(parent) => parent,
            None => self.clone(),
        }
    }
}

impl Deref for Connect {
    type Target = Link<ConnectState>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Connect {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Debug for Connect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Connect").finish()
    }
}

#[async_trait]
impl Node for Connect {
    fn to_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn parent(&self) -> Option<Arc<dyn Node>> {
        self.parent.upgrade()
    }

    fn filetype(&self) -> FileType {
        FileType::Directory
    }

    fn id(&self) -> Arc<InodeId> {
        self.inode.id.clone()
    }

    #[instrument]
    async fn open_dir(self: Arc<Self>) -> WasiResult<Box<dyn WasiDir>> {
        Ok(Box::new(OpenConnect(Open {
            root: self.root(),
            link: self,
            state: State::default().into(),
            write: false,
            read: false,
        })))
    }

    #[instrument]
    async fn open_file(
        self: Arc<Self>,
        _path: &str,
        _dir: bool,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> WasiResult<Box<dyn WasiFile>> {
        Ok(Box::new(OpenConnect(Open {
            root: self.root(),
            link: self,
            state: State::from(flags).into(),
            write,
            read,
        })))
    }
}

struct OpenConnect(Open<Connect>);

impl Deref for OpenConnect {
    type Target = Open<Connect>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Debug for OpenConnect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OpenConnect").finish()
    }
}

#[async_trait]
impl WasiDir for OpenConnect {
    fn as_any(&self) -> &dyn Any {
        self
    }

    #[instrument]
    async fn open_file(
        &self,
        follow: bool,
        path: &str,
        oflags: OFlags,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> WasiResult<Box<dyn WasiFile>> {
        const VALID_OFLAGS: &[u32] = &[
            OFlags::empty().bits(),
            OFlags::CREATE.bits(),
            OFlags::DIRECTORY.bits(),
            OFlags::TRUNCATE.bits(),
            OFlags::CREATE.bits() | OFlags::DIRECTORY.bits(),
            OFlags::CREATE.bits() | OFlags::EXCLUSIVE.bits(),
            OFlags::CREATE.bits() | OFlags::TRUNCATE.bits(),
            OFlags::CREATE.bits() | OFlags::DIRECTORY.bits() | OFlags::EXCLUSIVE.bits(),
        ];

        // Descend into the path.
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(follow, lhs).await?;
            return child
                .open_file(follow, rhs, oflags, read, write, flags)
                .await;
        }

        // Check the validity of the flags.
        if !VALID_OFLAGS.contains(&oflags.bits()) {
            return Err(Error::invalid_argument());
        }

        // Truncate can only be used with write.
        if oflags.contains(OFlags::TRUNCATE) && !write {
            return Err(Error::invalid_argument()); // FIXME
        }

        let odir = oflags.contains(OFlags::DIRECTORY);

        match path {
            "." if oflags.contains(OFlags::EXCLUSIVE) => Err(Error::exist()),
            "." if oflags.contains(OFlags::TRUNCATE) => Err(Error::io()), // FIXME
            "." | "" => {
                let link = self.link.clone();
                link.open_file(path, odir, read, write, flags).await
            }

            ".." if oflags.contains(OFlags::EXCLUSIVE) => Err(Error::exist()),
            ".." if oflags.contains(OFlags::TRUNCATE) => Err(Error::io()), // FIXME
            ".." => {
                let link = self.link.prev();
                link.open_file(path, odir, read, write, flags).await
            }

            addr => {
                let addr = addr
                    .parse()
                    .map_err(|e| Error::invalid_argument().context(e))
                    .context("failed to parse address")?;
                let state = &self.link.inode.data.read().await.content;
                let policy = state.policy.get(&addr);

                use enarx_config::Outgoing::{Tcp, Tls};

                let port = match (addr.port, policy) {
                    (Some(port), _) => port.into(),
                    (None, Tcp) => 80,
                    (None, Tls) => 443,
                };
                // TODO: Handle DNS in the keep
                // https://github.com/enarx/enarx/issues/1511
                let tcp = match addr.host {
                    Host::Domain(ref domain) if domain == "localhost" => {
                        std::net::TcpStream::connect(
                            [
                                SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
                                SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                            ]
                            .as_slice(),
                        )
                    }
                    Host::Domain(ref domain) => {
                        std::net::TcpStream::connect((domain.as_str(), port))
                    }
                    Host::Ipv4(addr) => std::net::TcpStream::connect((addr, port)),
                    Host::Ipv6(addr) => std::net::TcpStream::connect((addr, port)),
                }
                .map(TcpStream::from_std)
                .map_err(|e| Error::io().context(e))
                .with_context(|| format!("failed to connect to `{addr}`"))?;
                if flags == FdFlags::NONBLOCK {
                    tcp.set_nonblocking(true)
                        .context("failed to enable NONBLOCK")?;
                } else if flags.is_empty() {
                    tcp.set_nonblocking(false)
                        .context("failed to disable NONBLOCK")?;
                } else {
                    return Err(Error::invalid_argument()
                        .context("cannot set anything other than NONBLOCK"));
                }
                match policy {
                    Tcp => Ok(wasmtime_wasi::net::Socket::from(tcp).into()),
                    Tls => tls::Stream::connect(tcp, addr.to_string(), state.tls_config.clone())
                        .map(Into::into),
                }
            }
        }
    }

    async fn open_dir(&self, follow: bool, path: &str) -> WasiResult<Box<dyn WasiDir>> {
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(follow, lhs).await?;
            return child.open_dir(follow, rhs).await;
        }

        match path {
            "" => Err(Error::invalid_argument()),
            "." => self.link.clone().open_dir().await,
            ".." => self.link.prev().open_dir().await,
            _ => Err(Error::not_supported()),
        }
    }

    async fn create_dir(&self, path: &str) -> WasiResult<()> {
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(true, lhs).await?;
            return child.create_dir(rhs).await;
        }

        match path {
            "" | "." | ".." => Err(Error::invalid_argument()),
            _ => Err(Error::not_supported()),
        }
    }

    async fn readdir(
        &self,
        _cursor: ReaddirCursor,
    ) -> WasiResult<Box<dyn Iterator<Item = WasiResult<ReaddirEntity>> + Send>> {
        Err(Error::not_supported())
    }

    async fn symlink(&self, old_path: &str, new_path: &str) -> WasiResult<()> {
        if let Some((lhs, rhs)) = new_path.split_once('/') {
            let child = self.open_dir(true, lhs).await?;
            return child.symlink(old_path, rhs).await;
        }

        Err(Error::not_supported())
    }

    async fn remove_dir(&self, path: &str) -> WasiResult<()> {
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(true, lhs).await?;
            return child.remove_dir(rhs).await;
        }

        match path {
            "" | "." | ".." => Err(Error::invalid_argument()),
            _ => Err(Error::not_supported()),
        }
    }

    async fn unlink_file(&self, path: &str) -> WasiResult<()> {
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(true, lhs).await?;
            return child.unlink_file(rhs).await;
        }

        match path {
            "" | "." | ".." => Err(Error::invalid_argument()),
            _ => Err(Error::not_supported()),
        }
    }

    async fn read_link(&self, path: &str) -> WasiResult<PathBuf> {
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(true, lhs).await?;
            return child.read_link(rhs).await;
        }

        Err(Error::not_supported())
    }

    async fn get_filestat(&self) -> WasiResult<Filestat> {
        let ilock = self.link.inode.data.read().await;

        Ok(Filestat {
            device_id: **self.link.inode.id.device(),
            inode: **self.link.inode.id,
            filetype: FileType::Directory,
            nlink: Arc::strong_count(&self.link.inode) as u64 * 2,
            size: 0, // FIXME
            atim: Some(ilock.access),
            mtim: Some(ilock.modify),
            ctim: Some(ilock.create),
        })
    }

    async fn get_path_filestat(&self, path: &str, follow: bool) -> WasiResult<Filestat> {
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(true, lhs).await?;
            return child.get_path_filestat(rhs, follow).await;
        }

        match path {
            "." | "" => self.get_filestat().await,
            ".." => self.open_dir(true, "..").await?.get_filestat().await,
            _ => Err(Error::not_supported()),
        }
    }

    async fn rename(&self, path: &str, dest_dir: &dyn WasiDir, dest_path: &str) -> WasiResult<()> {
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(true, lhs).await?;
            return child.rename(rhs, dest_dir, dest_path).await;
        }

        Err(Error::not_supported())
    }

    async fn hard_link(
        &self,
        path: &str,
        target_dir: &dyn WasiDir,
        target_path: &str,
    ) -> WasiResult<()> {
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(true, lhs).await?;
            return child.hard_link(rhs, target_dir, target_path).await;
        }

        Err(Error::not_supported())
    }

    async fn set_times(
        &self,
        path: &str,
        atime: Option<SystemTimeSpec>,
        mtime: Option<SystemTimeSpec>,
        follow: bool,
    ) -> WasiResult<()> {
        if let Some((lhs, rhs)) = path.split_once('/') {
            let child = self.open_dir(true, lhs).await?;
            return child.set_times(rhs, atime, mtime, follow).await;
        }

        match path {
            "." | "" => self.link.inode.data.write().await.set_times(atime, mtime),
            ".." => {
                let dir = self.open_dir(true, "..").await?;
                dir.set_times(".", atime, mtime, follow).await
            }
            _ => Err(Error::not_supported()),
        }
    }
}

#[async_trait]
impl WasiFile for OpenConnect {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&mut self) -> WasiResult<FileType> {
        Ok(FileType::Directory)
    }

    async fn datasync(&mut self) -> WasiResult<()> {
        Ok(())
    }

    async fn sync(&mut self) -> WasiResult<()> {
        Ok(())
    }

    async fn get_fdflags(&mut self) -> WasiResult<FdFlags> {
        Err(Error::not_supported())
    }

    async fn set_fdflags(&mut self, _flags: FdFlags) -> WasiResult<()> {
        Err(Error::not_supported())
    }

    async fn get_filestat(&mut self) -> WasiResult<Filestat> {
        Err(Error::not_supported())
    }

    async fn set_filestat_size(&mut self, _size: u64) -> WasiResult<()> {
        Err(Error::not_supported())
    }

    async fn advise(&mut self, _offset: u64, _len: u64, _advice: Advice) -> WasiResult<()> {
        Err(Error::not_supported())
    }

    async fn allocate(&mut self, _offset: u64, _len: u64) -> WasiResult<()> {
        Err(Error::not_supported())
    }

    async fn set_times(
        &mut self,
        atime: Option<SystemTimeSpec>,
        mtime: Option<SystemTimeSpec>,
    ) -> WasiResult<()> {
        self.link.inode.data.write().await.set_times(atime, mtime)
    }

    async fn read_vectored<'a>(&mut self, _bufs: &mut [IoSliceMut<'a>]) -> WasiResult<u64> {
        Err(Error::not_supported())
    }

    async fn read_vectored_at<'a>(
        &mut self,
        _bufs: &mut [IoSliceMut<'a>],
        _offset: u64,
    ) -> WasiResult<u64> {
        Err(Error::not_supported())
    }

    async fn write_vectored<'a>(&mut self, _bufs: &[IoSlice<'a>]) -> WasiResult<u64> {
        Err(Error::not_supported())
    }

    async fn write_vectored_at<'a>(
        &mut self,
        _bufs: &[IoSlice<'a>],
        _offset: u64,
    ) -> WasiResult<u64> {
        Err(Error::not_supported())
    }

    async fn seek(&mut self, _pos: SeekFrom) -> WasiResult<u64> {
        Err(Error::not_supported())
    }

    async fn peek(&mut self, _buf: &mut [u8]) -> WasiResult<u64> {
        Err(Error::not_supported())
    }

    async fn num_ready_bytes(&self) -> WasiResult<u64> {
        Err(Error::not_supported())
    }

    async fn readable(&self) -> WasiResult<()> {
        Err(Error::not_supported())
    }

    async fn writable(&self) -> WasiResult<()> {
        Err(Error::not_supported())
    }
}
