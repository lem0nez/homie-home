use std::io;

use actix_web::web;
use async_stream::stream;
use futures_core::Stream;
use tokio::{io::AsyncReadExt, process::ChildStdout};

type BytesResult = io::Result<web::Bytes>;

const BUFFER_SIZE: usize = 8 * 1024;

pub struct StdoutReader {
    child_stdout: ChildStdout,
    buf: [u8; BUFFER_SIZE],
}

impl StdoutReader {
    pub fn new(child_stdout: ChildStdout) -> Self {
        Self {
            child_stdout,
            buf: [0; BUFFER_SIZE],
        }
    }

    pub async fn stream(mut self) -> impl Stream<Item = BytesResult> {
        stream! {
            loop {
                match self.child_stdout.read(&mut self.buf).await {
                    Ok(len) => {
                        if len == 0 {
                            break
                        } else {
                            let bytes = web::Bytes::copy_from_slice(&self.buf[..len]);
                            yield Ok(bytes)
                        }
                    },
                    Err(e) => yield Err(e),
                }
            }
        }
    }
}
