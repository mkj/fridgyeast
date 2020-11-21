#[allow(unused_imports)]
use log::{debug, error, info, warn};

use async_trait::async_trait;
use act_zero::*;
use act_zero::runtimes::async_std::Timer;
use act_zero::timer::Tick;

use std::time::Duration;
use std::path::PathBuf;

use async_std::io::BufReader;
use async_std::fs::File;
use async_std::prelude::*;

use async_std::fs::read_to_string;

use std::str::FromStr;


use anyhow::{anyhow,Result,Context};

use super::types::*;
use super::config::Config;
use crate::actzero_pubsub::Subscriber;

pub struct OneWireSensor {
    config: &'static Config,
    target: WeakAddr<dyn Subscriber<Readings>>,
    therm_re: regex::Regex,
    timer: Timer,
}


#[async_trait]
impl Tick for OneWireSensor {
    async fn tick(&mut self) -> ActorResult<()> {
        if self.timer.tick() {
            let r = self.get_readings().await;
            match r {
                Ok(r) => {
                    send!(self.target.notify(r));
                },
                Err(e) => {
                    warn!("Failed reading sensor: {}", e);
                }
            };
        }
        Produces::ok(())
    }
}

#[async_trait]
impl Actor for OneWireSensor {
    async fn started(&mut self, addr: Addr<Self>) -> ActorResult<()> {
        let dur = Duration::new(self.config.sensor_interval,0);
        self.timer.set_interval_weak(addr.downgrade(), dur);
        Produces::ok(())
    }
}

impl OneWireSensor {

    pub fn new(config: &'static Config, target: WeakAddr<dyn Subscriber<Readings>>) -> Self {
        let s = OneWireSensor {
            config,
            target,
            therm_re: regex::Regex::new("(?m).* YES\n.*t=(.*)\n").unwrap(),
            timer: Timer::default(),
        };

        // TODO actor
        // let dur = Duration::new(self.config.sensor_interval, 0);
        // ctx.schedule(Duration::from_millis(0), dur, ctx.myself(), None, SendReading);
        s
    }

    async fn get_readings(&self) -> Result<Readings> {
        let mut r = Readings::new();

        let names = self.sensor_names().await?;
        for n in &names {
            match self.read_sensor(n).await {
                Ok(s) => r.add(n, s),
                Err(e) => debug!("Error reading sensors {}: {}", n, e)
            }
        }

        debug!("sensor step {:?}", r);
        Ok(r)
    }

    // TODO actor
    /*
    fn recv(&mut self,
            _ctx: &Context<Self::Msg>,
            _msg: Self::Msg,
            _sender: Sender) {
        let r = self.get_readings();
        match r {
            Ok(r) => {
                self.notify.tell(Publish{msg: r, topic: "readings".into()}, None);
            },
            Err(e) => {
                warn!("Failed reading sensor: {}", e);
            }
        }
    }
    */



    async fn read_sensor(&self, n: &str) -> Result<f32> {
        let mut path = PathBuf::from(&self.config.sensor_base_dir);
        path.push(n);
        path.push("w1_slave");
        let s = read_to_string(path).await?;
        let caps = self.therm_re.captures(&s).ok_or_else(|| {
                anyhow!("Bad sensor contents match {}", &s)
            })?;
        let v = caps.get(1).ok_or_else(|| {
                anyhow!("Bad sensor contents match {}", &s)
            })?;

        Ok(f32::from_str(v.into()).context("Sensor reading isn't a number")? / 1000.)
    }

    async fn sensor_names(&self) -> Result<Vec<String>> {
        // TODO: needs to handle multiple busses.
        let mut path = PathBuf::from(&self.config.sensor_base_dir);
        path.push("w1_master_slaves");

        let f = BufReader::new(File::open(path).await.context("Failed opening w1 device list")?);
        let mut s = f.lines().collect::<Result<Vec<String>, std::io::Error>>().await
            .context("Failed reading w1 device list")?;
        // limit to ds18b20, family code 28
        s.retain(|n| n.starts_with("28-"));
        Ok(s)
    }
}

pub struct TestSensor {
    config: &'static Config,
    target: WeakAddr<dyn Subscriber<Readings>>,
    timer: Timer,
}

#[async_trait]
impl Tick for TestSensor {
    async fn tick(&mut self) -> ActorResult<()> {
        if self.timer.tick() {
            let r = self.get_readings().await;
            match r {
                Ok(r) => {
                    send!(self.target.notify(r));
                },
                Err(e) => {
                    warn!("Failed reading sensor: {}", e);
                }
            };
        }
        Produces::ok(())
    }
}

#[async_trait]
impl Actor for TestSensor {
    async fn started(&mut self, addr: Addr<Self>) -> ActorResult<()> {
        let dur = Duration::new(self.config.sensor_interval,0);
        self.timer.set_interval_weak(addr.downgrade(), dur);
        Produces::ok(())
    }
}

impl TestSensor {

    pub fn new(config: &'static Config, target: WeakAddr<dyn Subscriber<Readings>>) -> Self {
        TestSensor {
            config,
            target,
            timer: Timer::default(),
        }
    }

    async fn get_readings(&self) -> Result<Readings> {
        let mut r = Readings::new();
        r.add("ambient", 31.2);
        r.add(&self.config.wort_name, Self::try_read("test_wort.txt").await.unwrap_or_else(|_| 18.123));
        r.add(&self.config.fridge_name, Self::try_read("test_fridge.txt").await.unwrap_or_else(|_| 20.233));
        debug!("get_readings {:?}", r);
        Ok(r)
    }

    async fn try_read(filename: &str) -> Result<f32> {
        let s = read_to_string(filename).await?;
        Ok(s.trim().parse::<f32>()?)
    }
}
