use anyhow::{anyhow, Context, Error, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct Config {
    // all these config options need to be set in default.toml
    pub fridge_delay: u64,
    pub fridge_wort_invalid_time: u64,
    pub overshoot_interval: u64,

    pub sensor_base_dir: String,
    pub fridge_gpio_pin: u32,

    pub fridge_name: String,
    pub wort_name: String,

    pub listen: Vec<String>,
    pub ssl_domain: Vec<String>,
    pub owner_email: String,

    // TODO move this outside
    #[serde(skip_serializing)]
    pub session_secret: String,
    pub allowed_sessions: HashSet<String>,

    // defaulted to "."
    pub params_dir: PathBuf,

    // hardcoded params, set in Config::default()
    pub sensor_interval: u64,

    // runtime parameters usually from the command line
    // need to be set in Config::default()
    pub debug: bool,
    pub testmode: bool,
    pub dryrun: bool,
    pub nowait: bool,
    pub testssl: bool,
}

impl Config {
    pub fn example_toml() -> &'static str {
        include_str!("defconfig.toml")
    }

    pub fn load(conf_file: &str) -> Result<Self> {
        let c = config::Config::builder()
        // defaults for args
        .set_default("debug", false)?
        .set_default("testmode", false)?
        .set_default("nowait", false)?
        .set_default("dryrun", false)?
        .set_default("testssl", false)?
        // hidden config, not in defconfig.toml
        .set_default("sensor_interval", 10)? // 10 seconds
        .set_default("params_dir", ".")?
        .add_source(config::File::with_name(conf_file))
        .add_source(config::Environment::with_prefix("TEMPLOG"))
        .build()
        .map_err(|e| match e {
            config::ConfigError::NotFound(_) => anyhow!("Missing config {}", conf_file),
            _ => Error::new(e).context(format!("Problem parsing {}", conf_file)),
        })?;


        let conf: Self = c
            .try_deserialize()
            .map_err(|e| Error::new(e).context(format!("Problem loading config {}", conf_file)))?;
        Ok(conf)
    }
}
