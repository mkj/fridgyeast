use anyhow::{Result};

use std::str;

use std::cell::RefCell;
use std::fs::File;
use std::io::Read;

use serde::{Serialize,Deserialize};

use riker::actors::*;

use super::types::*;
use super::config::Config;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Params {
    pub fridge_setpoint: f32,
    pub fridge_difference: f32,
    pub overshoot_delay: u64,
    pub overshoot_factor: f32,
    pub running: bool,
    pub nowort: bool,
    pub fridge_range_lower: f32,
    pub fridge_range_upper: f32,
}

impl Params {
    pub fn defaults() -> Params {
        Params {
            fridge_setpoint: 18.0,
            fridge_difference: 0.2,
            overshoot_delay: 720, // 12 minutes
            overshoot_factor: 1.0,
            running: false,
            nowort: false,
            fridge_range_lower: 3.0,
            fridge_range_upper: 3.0,
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

}
