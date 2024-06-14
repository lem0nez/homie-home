use anyhow::anyhow;
use std::env;

pub mod rest;

mod endpoint;
mod stdout_reader;

#[derive(Clone)]
pub struct AccessToken(String);

impl AccessToken {
    pub fn from_env() -> anyhow::Result<Self> {
        env::var("ACCESS_TOKEN")
            .map_err(|e| anyhow!("Access token is not set: {e}"))
            .map(Self)
    }
}

impl PartialEq<&str> for AccessToken {
    fn eq(&self, str: &&str) -> bool {
        self.0 == *str
    }
}
