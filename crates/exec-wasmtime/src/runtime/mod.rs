// SPDX-License-Identifier: Apache-2.0

//! The Enarx Wasm runtime and all related functionality

mod identity;
mod io;
mod net;
mod vfs;

use self::io::null::Null;
use super::{Package, Workload};

use std::sync::Arc;

use anyhow::{bail, Context};
use enarx_config::{Config, StdioFile};
use once_cell::sync::Lazy;
use wasi_common::Error;
use wasmtime::{Engine, Linker, Module, Store, Trap, Val};
use wasmtime_vfs_dir::Directory;
use wasmtime_vfs_file::File;
use wasmtime_vfs_ledger::Ledger;
use wasmtime_vfs_memory::Node;
use wasmtime_wasi::WasiCtxBuilder;

pub type WasiResult<T> = Result<T, Error>;

/// Wasmtime config
static WASMTIME_CONFIG: Lazy<wasmtime::Config> = Lazy::new(|| {
    let mut config = wasmtime::Config::new();
    config.wasm_multi_memory(true);
    config.static_memory_maximum_size(0);
    config.static_memory_guard_size(0);
    config.dynamic_memory_guard_size(0);
    config.dynamic_memory_reserved_for_growth(16 * 1024 * 1024);
    config
});

// The Enarx Wasm runtime
pub struct Runtime;

impl Runtime {
    pub async fn execute(package: Package) -> anyhow::Result<Vec<Val>> {
        let (prvkey, crtreq) = identity::generate()?;

        let Workload { webasm, config } = package.try_into()?;
        let Config {
            steward,
            args,
            env,
            stdin,
            stdout,
            stderr,
            listen,
            connect,
        } = config.unwrap_or_default();

        let certs = if let Some(url) = steward {
            identity::steward(&url, crtreq).context("failed to attest to Steward")?
        } else {
            identity::selfsigned(&prvkey).context("failed to generate self-signed certificates")?
        }
        .into_iter()
        .map(rustls::Certificate)
        .collect::<Vec<_>>();

        let engine = Engine::new(&WASMTIME_CONFIG).context("failed to create execution engine")?;
        let module =
            Module::from_binary(&engine, &webasm).context("failed to compile Wasm module")?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::add_to_linker(&mut linker, |s| s).context("failed to link WASI")?;

        let ctx = WasiCtxBuilder::new()
            .envs(&env.into_iter().collect::<Vec<_>>())
            .context("failed to set environment variables")?
            .arg("main.wasm")
            .context("failed to set argv[0]")?
            .args(&args)
            .context("failed to set arguments from config")?;
        let ctx = match stdin {
            StdioFile::Null => ctx.stdin(Box::new(Null)),
            StdioFile::Host => ctx.inherit_stdin(),
        };
        let ctx = match stdout {
            StdioFile::Null => ctx.stdout(Box::new(Null)),
            StdioFile::Host => ctx.inherit_stdout(),
        };
        let ctx = match stderr {
            StdioFile::Null => ctx.stderr(Box::new(Null)),
            StdioFile::Host => ctx.inherit_stderr(),
        };
        let mut ctx = ctx.build();

        let certs = Arc::new(certs);
        let prvkey = Arc::new(prvkey);

        let create_file = Arc::new(File::new);

        let root = Directory::root(Ledger::new(), Some(create_file.clone()));

        // `/key`
        {
            let keyfs = wasmtime_vfs_keyfs::new(root.clone()).await?;
            root.attach("key", keyfs)
                .await
                .context("failed to attach /key")?;
        }

        // `/net`
        {
            let netfs = Directory::new(root.clone(), None);
            let listen =
                vfs::listen::new(netfs.clone(), certs.clone(), prvkey.clone(), listen).await?;
            let connect = vfs::connect::new(netfs.clone(), certs, prvkey, connect).await?;
            netfs
                .attach("lis", listen)
                .await
                .context("failed to attach /net/lis")?;
            netfs
                .attach("con", connect)
                .await
                .context("failed to attach /net/con")?;
            root.attach("net", netfs)
                .await
                .context("failed to attach `/net`")?;
        }

        // `/tmp`
        {
            let tmpfs = Directory::new(root.clone(), Some(create_file));
            root.attach("tmp", tmpfs)
                .await
                .context("failed to attach /tmp")?;
        }

        let root = root
            .open_dir()
            .await
            .context("failed to open root directory")?;

        ctx.push_preopened_dir(root, "/")
            .context("failed to push root directory")?;

        let mut store = Store::new(&engine, ctx);

        linker
            .module(&mut store, "", &module)
            .context("failed to link Wasm module")?;
        let func = linker
            .get_default(&mut store, "")
            .context("failed to get default function")?;

        let mut values = vec![Val::null(); func.ty(&store).results().len()];
        if let Err(e) = func.call(store, Default::default(), &mut values) {
            match e.downcast_ref::<Trap>().map(Trap::i32_exit_status) {
                Some(Some(0)) => {} // function exited with a code of 0, treat as success
                _ => bail!(e.context("failed to execute default function")),
            }
        };
        Ok(values)
    }
}
