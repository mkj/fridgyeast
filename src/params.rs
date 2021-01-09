use anyhow::{anyhow,Result};

use std::str;


use std::fs::File;
use std::io::Read;

use serde::{Serialize,Deserialize};


use std::io::Write;


use super::config::Config;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Params {
    pub fridge_setpoint: f32,
    pub fridge_difference: f32,
    pub running: bool,
    pub nowort: bool,
    pub fridge_range_lower: f32,
    pub fridge_range_upper: f32,
    pub overshoot_factor: f32,
}

impl Params {
    pub fn defaults() -> Params {
        Params {
            fridge_setpoint: 18.0,
            fridge_difference: 0.2,
            running: false,
            nowort: false,
            fridge_range_lower: 3.0,
            fridge_range_upper: 3.0,
            overshoot_factor: 0.1,
            }
    }

    fn try_load(filename: &str) -> Result<Params> {
        let mut s = String::new();
        File::open(filename)?.read_to_string(&mut s)?;
        // XXX Ok(...) needed here because of anyhow and serde errors?
        Ok(serde_json::from_str(&s)?)
    }

    pub fn load(config: &Config) -> Params {
        Self::try_load(&config.params_file)
            .unwrap_or_else(|_| Params::defaults())
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        let af = atomicwrites::AtomicFile::new(&config.params_file, atomicwrites::AllowOverwrite);
        af.write(|mut f| {
            serde_json::ser::to_writer(&mut f, self)?;
            f.write_all(b"\n")
        }).or_else(|e|
            Err(anyhow!("Writing params failed: {}", e))
        )
    }

}
