// SPDX-License-Identifier: Apache-2.0

//! Workload-related functionality and definitions.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::prelude::FromRawFd;

use anyhow::{anyhow, bail, ensure, Context, Result};
use drawbridge_client::types::digest::ContentDigest;
use drawbridge_client::types::{
    Meta, TagEntry, Tree, TreeDirectory, TreeEntry, TreeName, TreePath,
};
use drawbridge_client::{scope, Client, Entity, Node, Scope};
use enarx_config::Config;
use once_cell::sync::Lazy;
use ureq::serde_json;
use url::Url;
use wiggle::tracing::instrument;

/// Name of package entrypoint file
pub static PACKAGE_ENTRYPOINT: Lazy<TreeName> = Lazy::new(|| "main.wasm".parse().unwrap());

/// Name of package config file
pub static PACKAGE_CONFIG: Lazy<TreeName> = Lazy::new(|| "Enarx.toml".parse().unwrap());

/// Maximum size of WASM module in bytes
const MAX_WASM_SIZE: u64 = 100_000_000;
/// Maximum size of Enarx.toml in bytes
const MAX_CONF_SIZE: u64 = 1_000_000;
/// Maximum directory size in bytes
const MAX_DIR_SIZE: u64 = 1_000_000;

/// Maximum size of top-level response body in bytes
const MAX_TOP_SIZE: u64 = MAX_WASM_SIZE;

const TOML_MEDIA_TYPE: &str = "application/toml";
const WASM_MEDIA_TYPE: &str = "application/wasm";

/// Package spec
#[derive(Clone, Debug)]
#[cfg_attr(unix, derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(unix, serde(deny_unknown_fields, tag = "t", content = "c"))]
pub enum PackageSpec {
    /// URL
    Url(Url),

    /// Slug
    Slug(String),
}

const DEFAULT_HOST: &str = "store.profian.com";

fn parse_user(slug: &str) -> (String, &str) {
    let (host, user) = slug.rsplit_once('/').unwrap_or((DEFAULT_HOST, slug));
    (host.to_string(), user)
}

fn parse_repo(slug: &str) -> anyhow::Result<(String, &str, &str)> {
    let (head, repo) = slug
        .rsplit_once('/')
        .with_context(|| format!("Missing `/` in repository specification: {slug}"))?;
    let (host, user) = parse_user(head);
    Ok((host, user, repo))
}

pub fn parse_tag(slug: &str) -> anyhow::Result<(String, &str, &str, &str)> {
    let (head, tag) = slug
        .rsplit_once(':')
        .with_context(|| format!("Missing `:` in tag specification: {slug}"))?;
    let (host, user, repo) = parse_repo(head)?;
    Ok((host, user, repo, tag))
}

pub fn parse_slug(slug: &str) -> anyhow::Result<Url> {
    use drawbridge_client::API_VERSION;

    let (host, user, repo, tag) = parse_tag(slug)
        .with_context(|| format!("failed to parse `{slug}` as a Drawbridge slug"))?;
    format!("https://{host}/api/v{API_VERSION}/{user}/{repo}/_tag/{tag}")
        .parse()
        .with_context(|| format!("failed to construct a URL from Drawbridge slug `{slug}`"))
}

/// Package to execute
#[derive(Clone, Debug)]
#[cfg_attr(unix, derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(unix, serde(deny_unknown_fields, tag = "t", content = "c"))]
pub enum Package {
    /// Remote URL to fetch package from
    Remote(PackageSpec),

    /// Local package
    #[cfg(unix)]
    Local {
        /// Open WASM module file descriptor
        wasm: std::os::unix::prelude::RawFd,
        /// Optional open config file descriptor
        conf: Option<std::os::unix::prelude::RawFd>,
    },

    /// Local package
    #[cfg(windows)]
    Local {
        /// Open WASM module file
        wasm: File,
        /// Optional open config file
        conf: Option<File>,
    },
}

fn get_wasm(root: Entity<'_, impl Scope, scope::Node>, entry: &TreeEntry) -> Result<Vec<u8>> {
    ensure!(
        entry.meta.mime.essence_str() == WASM_MEDIA_TYPE,
        "invalid `{}` media type `{}`",
        *PACKAGE_ENTRYPOINT,
        entry.meta.mime.essence_str()
    );
    let (meta, wasm) = Node::new(root, &PACKAGE_ENTRYPOINT.clone().into())
        .get_bytes(MAX_WASM_SIZE)
        .with_context(|| format!("failed to fetch `{}`", *PACKAGE_ENTRYPOINT))?;
    ensure!(
        meta == entry.meta,
        "`{}` metadata does not match directory entry metadata",
        *PACKAGE_ENTRYPOINT,
    );
    Ok(wasm)
}

/// [Workload] content
pub struct WorkloadContent {
    /// Wasm module
    pub webasm: Vec<u8>,

    /// Enarx keep configuration
    pub config: Option<Config>,
}

fn get_package(
    root: Entity<'_, impl Scope, scope::Node>,
    dir: TreeDirectory,
) -> Result<WorkloadContent> {
    let webasm = dir
        .get(&PACKAGE_ENTRYPOINT)
        .ok_or_else(|| anyhow!("directory does not contain `{}`", *PACKAGE_ENTRYPOINT))
        .and_then(|e| get_wasm(root.clone(), e).context("failed to get Wasm"))?;

    let entry = if let Some(entry) = dir.get(&PACKAGE_CONFIG) {
        entry
    } else {
        return Ok(WorkloadContent {
            webasm,
            config: Default::default(),
        });
    };
    ensure!(
        entry.meta.mime.essence_str() == TOML_MEDIA_TYPE,
        "invalid `{}` media type `{}`",
        *PACKAGE_CONFIG,
        entry.meta.mime.essence_str()
    );
    let (meta, config) = Node::new(root, &PACKAGE_CONFIG.clone().into())
        .get_bytes(MAX_CONF_SIZE)
        .with_context(|| format!("failed to fetch `{}`", *PACKAGE_CONFIG))?;
    ensure!(
        meta == entry.meta,
        "`{}` metadata does not match directory entry metadata",
        *PACKAGE_CONFIG,
    );
    let config = toml::from_slice(&config).context("failed to parse config")?;
    Ok(WorkloadContent {
        webasm,
        config: Some(config),
    })
}

/// Acquired workload
pub struct Workload {
    /// Workload content digest
    pub digest: ContentDigest,

    /// Workload content
    pub content: WorkloadContent,
}

impl TryFrom<Package> for Workload {
    type Error = anyhow::Error;

    #[instrument]
    fn try_from(mut pkg: Package) -> Result<Self, Self::Error> {
        match pkg {
            Package::Remote(ref pkg) => {
                let url = match pkg {
                    PackageSpec::Url(url) => url.clone(),
                    PackageSpec::Slug(slug) => parse_slug(slug)?,
                };
                let cl = Client::<scope::Unknown>::new_scoped(url.clone())
                    .context("failed to construct client")?;
                let top = Entity::new(&cl);
                let (Meta { size, mime, hash }, mut rdr) = top
                    .get(MAX_TOP_SIZE)
                    .with_context(|| format!("failed to fetch top-level URL `{url}`"))?;
                match mime.essence_str() {
                    WASM_MEDIA_TYPE => {
                        ensure!(
                            size <= MAX_WASM_SIZE,
                            "Wasm size of `{size}` exceeds the limit of `{MAX_WASM_SIZE}`"
                        );
                        let size = size
                            .try_into()
                            .with_context(|| format!("failed to convert `{size}` to usize"))?;
                        let mut webasm = Vec::with_capacity(size);
                        let n = rdr
                            .read_to_end(&mut webasm)
                            .context("failed to fetch workload")?;
                        ensure!(n == size, "invalid amount of Wasm bytes fetched");
                        Ok(Self {
                            digest: hash,
                            content: WorkloadContent {
                                webasm,
                                config: None,
                            },
                        })
                    }
                    TreeDirectory::<()>::TYPE => serde_json::from_reader(rdr)
                        .context("failed to decode response body")
                        .and_then(|dir| {
                            let content = get_package(top.clone().scope(), dir)
                                .context("failed to fetch package")?;
                            Ok(Self {
                                digest: hash,
                                content,
                            })
                        }),
                    typ => {
                        let tag = serde_json::from_reader(rdr).with_context(|| format!("failed to decode top-level entity of type `{typ}` as either Wasm module, Drawbridge directory or a tag"))?;
                        let entry: TreeEntry = match tag {
                            TagEntry::Unsigned(e) => e,
                            TagEntry::Signed(_jws) => {
                                // TODO: Support signed tags
                                // https://github.com/enarx/enarx/issues/2167
                                bail!("signed tags are not currently supported")
                            }
                        };
                        let tree = top.child("tree");
                        let root = Node::new(tree.clone(), &TreePath::ROOT);
                        match entry.meta.mime.essence_str() {
                            WASM_MEDIA_TYPE => get_wasm(tree, &entry)
                                .map(|webasm| Workload {
                                    digest: entry.meta.hash,
                                    content: WorkloadContent {
                                        webasm,
                                        config: None,
                                    },
                                })
                                .context("failed to fetch workload"),
                            TreeDirectory::<()>::TYPE => {
                                let (meta, dir) = root
                                    .get_json::<TreeDirectory>(MAX_DIR_SIZE)
                                    .context("failed to get root directory")?;
                                ensure!(
                                    meta == entry.meta,
                                    "directory metadata does not match tag entry metadata"
                                );
                                let content =
                                    get_package(tree, dir).context("failed to fetch package")?;
                                Ok(Self {
                                    digest: entry.meta.hash,
                                    content,
                                })
                            }
                            typ => bail!("unsupported root type `{typ}`"),
                        }
                    }
                }
            }
            Package::Local {
                ref mut wasm,
                ref mut conf,
            } => {
                let mut webasm = Vec::new();
                // SAFETY: This FD was passed to us by the host and we trust that we have exclusive
                // access to it.
                #[cfg(unix)]
                let mut wasm = unsafe { File::from_raw_fd(*wasm) };

                wasm.read_to_end(&mut webasm)
                    .context("failed to read WASM module")?;
                let wasm =
                    Tree::file_entry_sync(webasm.as_slice(), WASM_MEDIA_TYPE.parse().unwrap())
                        .context("failed to compute Wasm entrypoint entry")?;

                let (config, digest) = if let Some(conf) = conf.as_mut() {
                    // SAFETY: This FD was passed to us by the host and we trust that we have exclusive
                    // access to it.
                    #[cfg(unix)]
                    let mut conf = unsafe { File::from_raw_fd(*conf) };

                    let mut config = vec![];
                    conf.read_to_end(&mut config)
                        .context("failed to read config")?;
                    let conf =
                        Tree::file_entry_sync(config.as_slice(), TOML_MEDIA_TYPE.parse().unwrap())
                            .context("failed to compute Enarx.toml entry")?;
                    let config = toml::from_slice(&config).context("failed to parse config")?;
                    let tree = Tree::try_from(BTreeMap::from([
                        (PACKAGE_ENTRYPOINT.clone(), wasm),
                        (PACKAGE_CONFIG.clone(), conf),
                    ]))
                    .context("failed to compute workload package tree")?;
                    (Some(config), tree.root().meta.hash.clone())
                } else {
                    let tree = Tree::try_from(BTreeMap::from([(PACKAGE_ENTRYPOINT.clone(), wasm)]))
                        .context("failed to workload package tree")?;
                    (None, tree.root().meta.hash.clone())
                };
                Ok(Self {
                    digest,
                    content: WorkloadContent { webasm, config },
                })
            }
        }
    }
}
