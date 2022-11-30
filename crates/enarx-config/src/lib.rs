// SPDX-License-Identifier: Apache-2.0
//! Configuration for a WASI application in an Enarx Keep
//!
#![doc = include_str!("../README.md")]
#![doc = include_str!("../Enarx_toml.md")]
#![deny(missing_docs)]
#![deny(clippy::all)]
#![warn(rust_2018_idioms)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

// TODO: Create a shared Enarx type crate.
// This should be revisited once we address https://github.com/enarx/enarx/issues/2367 and probably
// completely removed.
pub use drawbridge_type::{TreeName as FileName, TreePath as Path};

/// Configuration file template
pub const CONFIG_TEMPLATE: &str = r#"## Configuration for a WASI application in an Enarx Keep

## Arguments
# args = [
#      "--argument1",
#      "--argument2=foo"
# ]

## Steward
# steward = "https://attest.profian.com"

## Environment variables
# [env]
# VAR1 = "var1"
# VAR2 = "var2"

# Standard input file
[stdin]
kind = "host" # or kind = "null"

# Standard output file
[stdout]
kind = "host" # or kind = "null"

# Standard error file
[stderr]
kind = "host" # or kind = "null"

## A listen socket on port 12345
#[listen.12345]
#prot = "tls" # or prot = "tcp"

## An outgoing connected stream
#[connect."localhost:23456"]
#prot = "tls" # or prot = "tcp"
"#;

/// The configuration for an Enarx WASI application
///
/// This struct can be used with any serde deserializer.
///
/// # Examples
///
/// ```
/// extern crate toml;
/// use enarx_config::Config;
/// const CONFIG: &str = r#"
/// [listen.12345]
/// prot = "tls"
/// "#;
///
/// let config: Config = toml::from_str(CONFIG).unwrap();
/// ```
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// An optional Steward URL
    pub steward: Option<Url>,

    /// The arguments to provide to the application
    pub args: Vec<String>,

    /// The environment variables to provide to the application
    pub env: HashMap<String, String>,

    /// Standard input file. Null by default.
    pub stdin: StdioFile,

    /// Standard output file. Null by default.
    pub stdout: StdioFile,

    /// Standard error file. Null by default.
    pub stderr: StdioFile,

    /// Pre-defined listening sockets.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(default)]
    pub listen: HashMap<FileName, ListenFile>,

    /// Pre-defined connected streams.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(default)]
    pub connect: HashMap<FileName, ConnectFile>,

    /// Network policy.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(default)]
    pub network: Network,
}

/// Incoming network connection policy.
///
/// This API is highly experimental and will change significantly in the future.
/// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
/// feature is important for you.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncomingNetwork {
    /// Default incoming network connection policy.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(default)]
    pub default: ListenFile,
}

/// Outgoing network connection policy.
///
/// This API is highly experimental and will change significantly in the future.
/// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
/// feature is important for you.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutgoingNetwork {
    /// Default outgoing network connection policy.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(default)]
    pub default: ConnectFile,
}

/// Network policy.
///
/// This API is highly experimental and will change significantly in the future.
/// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
/// feature is important for you.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    /// Incoming network connection policy.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(default)]
    pub incoming: IncomingNetwork,

    /// Outgoing network connection policy
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(default)]
    pub outgoing: OutgoingNetwork,
}

/// File descriptor of a listen socket.
///
/// This API is highly experimental and will change significantly in the future.
/// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
/// feature is important for you.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "prot", deny_unknown_fields)]
pub enum ListenFile {
    /// TLS listen socket.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(rename = "tls")]
    Tls,

    /// TCP listen socket.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(rename = "tcp")]
    Tcp,
}

impl Default for ListenFile {
    fn default() -> Self {
        Self::Tls
    }
}

/// File descriptor of a stream socket.
///
/// This API is highly experimental and will change significantly in the future.
/// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
/// feature is important for you.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "prot", deny_unknown_fields)]
pub enum ConnectFile {
    /// TLS stream socket.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(rename = "tls")]
    Tls,

    /// TCP stream socket.
    ///
    /// This API is highly experimental and will change significantly in the future.
    /// Please track https://github.com/enarx/enarx/issues/2367 and provide feedback if this
    /// feature is important for you.
    #[serde(rename = "tcp")]
    Tcp,
}

impl Default for ConnectFile {
    fn default() -> Self {
        Self::Tls
    }
}

/// Standard I/O file configuration
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum StdioFile {
    /// Discard standard I/O.
    #[serde(rename = "null")]
    Null,

    /// Forward standard I/O to host.
    #[serde(rename = "host")]
    Host,
}

impl Default for StdioFile {
    fn default() -> Self {
        Self::Null
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn default() {
        let cfg: Config = toml::from_str("").expect("failed to parse config");
        assert_eq!(cfg, Default::default());
    }

    #[test]
    fn all() {
        let cfg: Config = toml::from_str(
            r#"
steward = "https://example.com"

args = [ "first", "2" ]

[env]
TEST = "test"

[stdin]
kind = "host"

[stdout]
kind = "null"

[stderr]
kind = "host"

[listen.9000]
prot = "tcp"

[listen."::9001"]
prot = "tls"

[connect."tls.example.com"]
prot = "tls"

[connect."tcp.example.com"]
prot = "tcp"

[network.incoming.default]
prot = "tcp"

[network.outgoing.default]
prot = "tls"
"#,
        )
        .expect("failed to parse config");
        assert_eq!(
            cfg,
            Config {
                steward: Some("https://example.com".parse().unwrap()),
                args: vec!["first".into(), "2".into()],
                env: vec![("TEST".into(), "test".into())].into_iter().collect(),
                stdin: StdioFile::Host,
                stdout: StdioFile::Null,
                stderr: StdioFile::Host,
                listen: vec![
                    ("9000".parse().unwrap(), ListenFile::Tcp),
                    ("::9001".parse().unwrap(), ListenFile::Tls)
                ]
                .into_iter()
                .collect(),
                connect: vec![
                    ("tls.example.com".parse().unwrap(), ConnectFile::Tls),
                    ("tcp.example.com".parse().unwrap(), ConnectFile::Tcp)
                ]
                .into_iter()
                .collect(),
                network: Network {
                    incoming: IncomingNetwork {
                        default: ListenFile::Tcp,
                    },
                    ..Default::default()
                },
            }
        );
    }

    #[test]
    fn template() {
        let cfg: Config = toml::from_str(CONFIG_TEMPLATE).expect("failed to parse config template");
        let buf = toml::to_string(&cfg).expect("failed to reencode config template");
        assert_eq!(
            toml::from_str::<Config>(&buf).expect("failed to parse reencoded config template"),
            cfg
        );
    }
}
