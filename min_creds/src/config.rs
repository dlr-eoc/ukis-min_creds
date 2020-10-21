use std::collections::HashMap;
use std::fs::File;

use eyre::{Result, WrapErr};
use serde::Deserialize;

fn default_num_concurrent() -> u32 { 1 }

#[derive(Deserialize, Debug)]
pub struct Credential {
    pub user: String,
    pub password: String,

    #[serde(default = "default_num_concurrent")]
    pub num_concurrent: u32,
}

fn default_listen() -> String { "127.0.0.1:9992".to_string() }

fn default_lease_timeout_secs() -> u32 { 60 * 5 }

fn default_web_path() -> String { "/".to_string() }

#[derive(Deserialize, Debug)]
pub struct Service {
    #[serde(default = "default_lease_timeout_secs")]
    pub lease_timeout_secs: u32,
    pub credentials: Vec<Credential>,
}

#[derive(Deserialize, Debug)]
pub struct SSLConfig {
    pub private_key_pem_file: String,
    pub certificate_chain_file: String,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen_on: String,

    #[serde(default = "default_web_path")]
    pub web_path: String,

    pub services: HashMap<String, Service>,

    pub access_tokens: Vec<String>,

    pub ssl: Option<SSLConfig>,

    pub persistent_leases_filename: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_on: default_listen(),
            web_path: default_web_path(),
            services: HashMap::default(),
            access_tokens: vec![],
            ssl: None,
            persistent_leases_filename: None
        }
    }
}

pub fn read_config(filename: String) -> Result<Config> {
    let file = File::open(filename)?;
    serde_yaml::from_reader(file)
        .wrap_err_with(|| "reading configuration failed")
}
