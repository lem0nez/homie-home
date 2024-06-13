use std::{
    io::{self, Read},
    process::ChildStdout,
};

use actix_web::web;
use async_stream::stream;
use futures_core::Stream;

type BytesResult = io::Result<web::Bytes>;

const BUFFER_SIZE: usize = 8 * 1024;

pub struct StdoutReader {
    child_stdout: ChildStdout,
    buffer: [u8; BUFFER_SIZE],
}

impl StdoutReader {
    pub fn new(child_stdout: ChildStdout) -> Self {
        Self {
            child_stdout,
            buffer: [0; BUFFER_SIZE],
        }
    }

    pub fn stream(self) -> impl Stream<Item = BytesResult> {
        stream! {
            for bytes in self {
                yield bytes;
            }
        }
    }
}

impl Iterator for StdoutReader {
    type Item = BytesResult;

    fn next(&mut self) -> Option<Self::Item> {
        match self.child_stdout.read(&mut self.buffer) {
            Ok(len) => {
                if len == 0 {
                    None
                } else {
                    Some(Ok(web::Bytes::copy_from_slice(&self.buffer[..len])))
                }
            }
            Err(e) => Some(Err(e)),
        }
    }
}
