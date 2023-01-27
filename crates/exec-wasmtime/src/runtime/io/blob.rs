// SPDX-License-Identifier: Apache-2.0

//! A byte blob WasiFile

use std::any::Any;
use std::io::Read;

use wasi_common::file::FileType;
use wasi_common::{Error, ErrorExt, WasiFile};

#[derive(Clone)]
pub struct Blob<T>(std::io::Cursor<T>);

impl<T> From<T> for Blob<T> {
    fn from(r: T) -> Self {
        Self(std::io::Cursor::new(r))
    }
}

#[wiggle::async_trait]
impl<T: AsRef<[u8]> + Send + Sync + 'static> WasiFile for Blob<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&mut self) -> Result<FileType, Error> {
        Ok(FileType::RegularFile)
    }

    async fn read_vectored<'a>(
        &mut self,
        bufs: &mut [std::io::IoSliceMut<'a>],
    ) -> Result<u64, Error> {
        self.0
            .read_vectored(bufs)
            .map(|n| n as _)
            .map_err(|e| Error::io().context(e))
    }

    async fn readable(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn writable(&self) -> Result<(), Error> {
        Ok(())
    }
}
