use riker::actors::*;

use std::time::Duration;
use std::io;
use std::fs::File;
use std::io::{Read,BufReader,BufRead};
use std::path::PathBuf;

use std::str::FromStr;

use super::types::*;
use super::config::Config;

pub struct OneWireSensor {
    config: &'static Config,
    notify: ChannelRef<Readings>,
}

// #[derive(Clone)]
pub struct TestSensor {
    config: &'static Config,
    notify: ChannelRef<Readings>,
}

#[derive(Debug,Clone)]
pub struct SendReading;

impl Actor for OneWireSensor {
    type Msg = SendReading;

    fn recv(&mut self,
            _ctx: &Context<Self::Msg>,
            _msg: Self::Msg,
            _sender: Sender) {
        self.notify.tell(Publish{msg: self.get_readings(), topic: "readings".into()}, None);
    }

    fn pre_start(&mut self, ctx: &Context<Self::Msg>) {
        let dur = Duration::new(self.config.sensor_sleep,0);
        ctx.schedule(Duration::from_millis(0), dur, ctx.myself(), None, SendReading);
    }
}

impl ActorFactoryArgs<(&'static Config, ChannelRef<Readings>)> for OneWireSensor {
    fn create_args((config, notify): (&'static Config, ChannelRef<Readings>)) -> Self {
        OneWireSensor {
            config: config,
            notify: notify,
        }
    }
}

impl OneWireSensor {
    fn get_readings(&self) -> Readings {
        let mut r = Readings::new();

        if let Ok(names) = self.sensor_names() {
            for n in &names {
                match self.read_sensor(n) {
                    Ok(s) => r.add(n, s),
                    Err(e) => debug!("Error reading sensors {}: {}", n, e)
                }
            }
        }

        debug!("sensor step {:?}", r);
        r
    }

    fn read_sensor(&self, n: &str) -> Result<f32, TemplogError> {
        lazy_static! {
            // multiline
            static ref THERM_RE: regex::Regex = regex::Regex::new("(?m).* YES\n.*t=(.*)\n").unwrap();
        }
        let mut path = PathBuf::from(&self.config.sensor_base_dir);
        path.push(n);
        path.push("w1_slave");
        let mut s = String::new();
        File::open(path)?.read_to_string(&mut s)?;
        let caps = THERM_RE.captures(&s).ok_or_else(|| {
                TemplogError::new("Bad sensor contents match")
            })?;
        let v = caps.get(1).ok_or_else(|| {
                TemplogError::new("Bad field contents match")
            })?.as_str();

        Ok(f32::from_str(v)?)
    }

    fn sensor_names(&self) -> Result<Vec<String>, TemplogError> {
        // TODO: needs to handle multiple busses.
        let mut path = PathBuf::from(&self.config.sensor_base_dir);
        path.push("w1_master_slaves");

        let f = BufReader::new(File::open(path)?);
        let s = f.lines().collect::<Result<Vec<String>, io::Error>>()?;
        Ok(s)
    }
}

impl Actor for TestSensor {
    type Msg = SendReading;

    fn recv(&mut self,
            _ctx: &Context<Self::Msg>,
            _msg: Self::Msg,
            _sender: Sender) {
        self.notify.tell(Publish{msg: self.get_readings(), topic: "readings".into()}, None);
    }

    fn pre_start(&mut self, ctx: &Context<Self::Msg>) {
        info!("pre_start testsensor readings");
        let dur = Duration::new(self.config.sensor_sleep,0);
        ctx.schedule(Duration::from_millis(0), dur, ctx.myself(), None, SendReading);
    }
}

impl ActorFactoryArgs<(&'static Config, ChannelRef<Readings>)> for TestSensor {
    fn create_args((config, notify): (&'static Config, ChannelRef<Readings>)) -> Self {
        TestSensor {
            config: config,
            notify: notify,
        }
    }
}

impl TestSensor {
    fn get_readings(&self) -> Readings {
        let mut r = Readings::new();
        r.add("ambient", 31.2);
        r.add("wort", Self::try_read("test_wort.txt").unwrap_or_else(|_| 18.0));
        r.add("fridge", Self::try_read("test_fridge.txt").unwrap_or_else(|_| 20.0));
        r
    }

    fn try_read(filename: &str) -> Result<f32, TemplogError> {
        let mut s = String::new();
        File::open(filename)?.read_to_string(&mut s)?;
        Ok(s.trim().parse::<f32>()?)
    }
}
