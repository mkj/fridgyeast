use std::collections::HashSet;
use serde::Deserialize;
use anyhow::{Context, anyhow, Result, Error};

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
    pub auth_email: String,
    pub cert_file: String,
    pub key_file: String,

    // TODO move this outside
    #[serde(skip_serializing)]
    pub session_secret: String,
    pub allowed_sessions: HashSet<String>,

    // hardcoded params, set in Config::default()
    pub params_file: String,
    pub sensor_interval: u64,


    // runtime parameters usually from the command line
    // need to be set in Config::default()
    pub debug: bool,
    pub testmode: bool,
    pub dryrun: bool,
    pub nowait: bool,
}

impl Config {
    pub fn example_toml() -> &'static str {
        include_str!("defconfig.toml")
    }

    fn default() -> Result<config::Config> {
        let mut c = config::Config::default();
        // defaults for args
        c.set_default("debug", false)?;
        c.set_default("testmode", false)?;
        c.set_default("nowait", false)?;
        c.set_default("dryrun", false)?;

        // hidden config, not in defconfig.toml
        c.set_default("sensor_interval", 10)?; // 10 seconds
        c.set_default("params_file", "fridgyeast.conf".to_string())?;
        Ok(c)
    }

    pub fn load(conf_file: &str) -> Result<Self> {
        let mut c = Self::default()?;
        c.merge(config::File::with_name(conf_file)).map_err(|e| {
            match e {
                config::ConfigError::NotFound(_) => anyhow!("Missing config {}", conf_file),
                _ => Error::new(e).context(format!("Problem parsing {}", conf_file)),
            }
        })?;
        c.merge(config::Environment::with_prefix("TEMPLOG"))
            .context("Failed loading from TEMPLOG_ environment variables")?;
        Ok(c.try_into().with_context(|| format!("Problem loading config {}", conf_file))?)
    }
}
