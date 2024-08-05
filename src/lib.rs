use std::env;

use actix_web_httpauth::extractors::bearer::BearerAuth;
use anyhow::anyhow;

pub mod config;
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

impl PartialEq<BearerAuth> for AccessToken {
    fn eq(&self, auth: &BearerAuth) -> bool {
        self.0 == auth.token()
    }
}
