use std::collections::HashSet;
use serde::{Serialize,Deserialize};
use anyhow::{Context, anyhow, Result, Error};

#[derive(Deserialize,Serialize,Debug,Clone)]
pub struct Config {
    // all config options need to be set in default.toml
    pub sensor_sleep: u64,
    pub upload_sleep: u64,

    pub fridge_delay: u64,
    pub fridge_wort_invalid_time: u64,

    pub params_file: String,

    pub sensor_base_dir: String,
    pub fridge_gpio_pin: u32,

    pub ambient_name: String,
    pub fridge_name: String,
    pub wort_name: String,
    pub internal_temperature: String,

    // TODO move this outside
    pub session_secret: String,
    pub allowed_sessions: HashSet<String>,

    // runtime parameters usually from the command line
    // need to be set in default()
    #[serde(skip_serializing)]
    pub debug: bool,

    #[serde(skip_serializing)]
    pub testmode: bool,

    #[serde(skip_serializing)]
    pub dryrun: bool,

    #[serde(skip_serializing)]
    pub nowait: bool,
}

impl Config {
    pub fn example_toml() -> &'static str {
        include_str!("defconfig.toml")
    }

    fn default() -> Result<config::Config> {
        let mut c = config::Config::default();
        c.set_default("debug", false)?;
        c.set_default("testmode", false)?;
        c.set_default("nowait", false)?;
        c.set_default("dryrun", false)?;
        Ok(c)
    }

    pub fn load(conf_file: &str) -> Result<Self> {
        let mut c = Self::default()?;
        c.merge(config::File::with_name(conf_file)).or_else(|e| {
            Err(match e {
                config::ConfigError::NotFound(_) => anyhow!("Missing config {}", conf_file),
                // XXX this is ugly, better way?
                _ => Error::new(e).context(format!("Problem parsing {}", conf_file)),
            })
        })?;
        c.merge(config::Environment::with_prefix("TEMPLOG"))
            .context("Failed loading from TEMPLOG_ environment variables")?;
        Ok(c.try_into().with_context(|| format!("Problem loading config {}", conf_file))?)
    }
}
