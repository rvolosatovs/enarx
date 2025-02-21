// SPDX-License-Identifier: Apache-2.0

use clap::Args;

/// Generate a new access token for a repository.
#[derive(Args, Debug)]
pub struct Options {}

impl Options {
    pub fn execute(self) -> anyhow::Result<()> {
        unimplemented!()
    }
}
