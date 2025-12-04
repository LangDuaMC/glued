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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_ip: Option<String>,
    pub dns_bind: SocketAddr,
    pub cluster_secret: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network_name: "glued_net".into(),
            // Default topic: 32 bytes of 0x42 encoded as hex
            topic_id: "4242424242424242424242424242424242424242424242424242424242424242".into(),
            bootstrap_peers: Vec::new(),
            bind_ip: None,
            dns_bind: "0.0.0.0:53".parse().unwrap(),
            cluster_secret: "default_insecure_secret".into(),
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let mut config: Config = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file("glued.toml"))
            .merge(Json::file("glued.json"))
            .merge(Env::prefixed("GLUED_"))
            .extract()
            .map_err(|e| anyhow::anyhow!("Failed to load configuration: {}", e))?;

        // If bind_ip is set, override the IP part of dns_bind
        if let Some(ref ip) = config.bind_ip {
            let port = config.dns_bind.port();
            config.dns_bind = format!("{}:{}", ip, port)
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid bind_ip: {}", e))?;
        }

        Ok(config)
    }
}
