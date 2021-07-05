#[allow(unused_imports)]
use log::{debug, info, warn, error};

use anyhow::{Context, Result, anyhow};

use std::str;


use std::fs::File;
use std::io::Read;
use std::path::Path;

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
    const FILENAME: &'static str = "fridgyeast.conf";
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

    fn try_load<P: AsRef<Path>>(path: P) -> Result<Params> {
        let mut s = String::new();
        File::open(path)?.read_to_string(&mut s)?;
        // XXX Ok(...) needed here because of anyhow and serde errors?
        Ok(serde_json::from_str(&s)?)
    }

    pub fn load(config: &Config) -> Result<Params> {
        let params_file = config.params_dir.join(Params::FILENAME);
        Self::try_load(&params_file)
        .or_else(|e| {
            let missing = match e.root_cause().downcast_ref::<std::io::Error>() {
                Some(ioe) => ioe.kind() == std::io::ErrorKind::NotFound,
                None => false,
            };
            if missing {
                warn!("No existing config found, using new default parameters");
            } else {
                error!("Problem reading existing params, will use defaults. {}", e);
            }
            let p = Params::defaults();
            p.save(config).context("writing new default config")?;
            Ok(p)
        })
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        let params_file = config.params_dir.join(Params::FILENAME);
        let af = atomicwrites::AtomicFile::new(&params_file, atomicwrites::AllowOverwrite);
        af.write(|mut f| {
            serde_json::ser::to_writer(&mut f, self)?;
            f.write_all(b"\n")
        }).map_err(|e| anyhow!("Writing params failed: {}", e))
    }

}
