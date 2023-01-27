// SPDX-License-Identifier: Apache-2.0

//! Networking functionality for keeps

pub mod tls;

use super::identity::Peer;

use std::net::SocketAddr;

use serde::Serialize;
use url::Host;

#[derive(Serialize, Clone, Debug, Eq, PartialEq)]
pub struct ConnectMetadata {
    pub host: Host,
    pub port: u16,
    pub peer: Peer,
}

#[derive(Serialize, Clone, Debug, Eq, PartialEq)]
pub struct AcceptMetadata {
    pub addr: SocketAddr,
    pub peer: Peer,
}
