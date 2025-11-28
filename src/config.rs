use figment::{
    providers::{Env, Format, Json, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub network_name: String,
    pub topic_id: String,
    pub bootstrap_peers: Vec<String>,
    pub dns_bind: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network_name: "glued_net".into(),
            // Default topic: 32 bytes of 0x42 encoded as hex
            topic_id: "4242424242424242424242424242424242424242424242424242424242424242".into(),
            bootstrap_peers: Vec::new(),
            dns_bind: "0.0.0.0:5353".parse().unwrap(),
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file("glued.toml"))
            .merge(Json::file("glued.json"))
            .merge(Env::prefixed("GLUED_"))
            .extract()
            .map_err(|e| anyhow::anyhow!("Failed to load configuration: {}", e))
    }
}
