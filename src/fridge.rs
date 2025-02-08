#[allow(unused_imports)]
use {
    anyhow::{anyhow, bail, Context, Result},
    log::{debug, error, info, log, warn},
};

use crate::actzero_pubsub::Subscriber;
use async_std::task::block_on;
use async_trait::async_trait;
use std::time::{Duration, Instant};

use act_zero::runtimes::async_std::spawn_actor;
use act_zero::runtimes::async_std::Timer;
use act_zero::timer::Tick;
use act_zero::*;

use serde::Serialize;
use serde_json::ser::to_string_pretty;

use chrono::{offset::Utc, DateTime};

use super::config::Config;
use crate::params::Params;

use super::sensor;
use super::timeseries::{Seq, TimeSeries};
use super::types::*;

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub params: Params,
    pub on: bool,
    pub temp_wort: Option<f32>,
    pub temp_fridge: Option<f32>,
    pub off_duration: Duration,
    pub fridge_delay: Duration,

    // from config
    pub overshoot_interval: u64,
    pub sensor_interval: u64,

    pub version: &'static str,
    pub uptime: Duration,
}

pub struct Fridge {
    params: Params,
    config: &'static Config,

    on: bool,
    temp_wort: Option<f32>,
    temp_fridge: Option<f32>,
    last_off_time: Instant,
    wort_valid_time: Instant,
    integrator: StepIntegrator,
    output: FridgeOutput,
    started: Instant,

    timer: Timer,

    // avoid printing logs too often
    often_tooearly: NotTooOften,
    often_badfridge: NotTooOften,
    often_badwort: NotTooOften,

    sensor: Option<Addr<dyn Actor>>,
    timeseries: Addr<TimeSeries>,
}

enum FridgeOutput {
    Gpio(gpio_cdev::LineHandle),
    Fake,
}

impl Drop for Fridge {
    fn drop(&mut self) {
        if self.on {
            info!("Fridge turns off at shutdown");
        }
        self.turn_off();

        // make sure timeseries has flushed to disk
        let t = self.timeseries.termination();
        self.timeseries = Addr::detached();
        block_on(t);
    }
}

#[async_trait]
impl Subscriber<Readings> for Fridge {
    async fn notify(&mut self, r: Readings) {
        self.add_readings(r).await;
    }
}

#[async_trait]
impl Tick for Fridge {
    async fn tick(&mut self) -> ActorResult<()> {
        if self.timer.tick() {
            self.update();
        }
        Produces::ok(())
    }
}

#[async_trait]
impl Actor for Fridge {
    async fn started(&mut self, addr: Addr<Self>) -> ActorResult<()> {
        let pp = to_string_pretty(&self.params).expect("Failed serialising params");
        info!("Starting with params: {}", pp);

        if self.config.nowait {
            self.last_off_time -= Duration::new(self.config.fridge_delay, 1);
        }

        if self.config.testmode {
            let sens = sensor::TestSensor::new(self.config, upcast!(addr.downgrade()));
            self.sensor = Some(upcast!(spawn_actor(sens)));
        } else {
            let sens = sensor::OneWireSensor::new(self.config, upcast!(addr.downgrade()));
            self.sensor = Some(upcast!(spawn_actor(sens)));
        };

        // Start the timer going
        self.update();
        // Arbitrary 10 secs, enough to notice invalid wort or fridge delay
        self.timer
            .set_interval_weak(addr.downgrade(), Duration::from_secs(10));
        Produces::ok(())
    }

    async fn error(&mut self, error: ActorError) -> bool {
        warn!("Ignoring error from Fridge actor: {:?}", error);
        false
    }
}

impl Fridge {
    pub fn try_new(config: &'static Config) -> Result<Self> {
        let output = Self::make_output(config)?;

        let timeseries = spawn_actor(TimeSeries::new(
            std::path::Path::new("fridgyeast.db"),
            300,
            chrono::Duration::days(2),
        )?);

        let mut f = Fridge {
            config,
            params: Params::load(config)?,
            on: false,
            temp_wort: None,
            temp_fridge: None,
            last_off_time: Instant::now(),
            wort_valid_time: Instant::now() - Duration::new(config.fridge_wort_invalid_time, 100),
            integrator: StepIntegrator::new(Duration::from_secs(config.overshoot_interval)),
            output,
            often_tooearly: NotTooOften::new(300),
            often_badwort: NotTooOften::new(100),
            often_badfridge: NotTooOften::new(300),
            timer: Timer::default(),
            started: Instant::now(),
            sensor: None,
            timeseries,
        };

        send!(f.timeseries.add_step("setpoint", f.params.fridge_setpoint));
        send!(f.timeseries.save());

        // Early check the fridge can turn off
        f.turn(false).context("Initial fridge turn-off")?;

        Ok(f)
    }

    pub async fn add_readings(&mut self, r: Readings) {
        debug!("add_readings {r:?}");
        self.temp_wort = r.get_temp(&self.config.wort_name);
        self.temp_fridge = r.get_temp(&self.config.fridge_name);

        if self.temp_wort.is_some() {
            self.wort_valid_time = Instant::now();
        }

        if let Some(t) = self.temp_wort {
            send!(self.timeseries.add("wort", t));
        }

        if let Some(t) = self.temp_fridge {
            send!(self.timeseries.add("fridge", t));
        }

        self.update();
    }

    pub async fn history(&mut self, name: String, start: DateTime<Utc>) -> ActorResult<Seq> {
        Ok(call!(self.timeseries.get(name, start)))
    }

    pub async fn history_step(&mut self, name: String, start: DateTime<Utc>) -> ActorResult<Seq> {
        Ok(call!(self.timeseries.get_step(name, start)))
    }

    pub async fn set_params(&mut self, p: Params) -> ActorResult<Result<()>> {
        self.params = p;
        let pp = to_string_pretty(&self.params).unwrap_or("Failed serialising params".into());
        info!("New params: {pp}");

        // quickly update the fridge for real world interactivity
        self.update();

        send!(self
            .timeseries
            .add_step("setpoint", self.params.fridge_setpoint));
        send!(self.timeseries.save());
        let res = self.params.save(self.config);

        if let Err(e) = &res {
            // log it too
            error!("Failed saving params: {e}");
        }
        Produces::ok(res)
    }

    pub async fn get_status(&mut self) -> ActorResult<Status> {
        let s = Status {
            params: self.params.clone(),
            on: self.on,
            temp_wort: self.temp_wort,
            temp_fridge: self.temp_fridge,
            off_duration: Instant::now() - self.last_off_time,
            fridge_delay: Duration::from_secs(self.config.fridge_delay),
            overshoot_interval: self.config.overshoot_interval,
            sensor_interval: self.config.sensor_interval,
            version: get_vcs_version(),
            uptime: Instant::now() - self.started,
        };
        Produces::ok(s)
    }

    fn make_output(config: &Config) -> Result<FridgeOutput> {
        if config.testmode || config.dryrun {
            Ok(FridgeOutput::Fake)
        } else {
            let mut chip = gpio_cdev::Chip::new("/dev/gpiochip0").context("gpiochip0 failed")?;
            let pin = chip
                .get_line(config.fridge_gpio_pin)
                .with_context(|| format!("gpio line {} failed", config.fridge_gpio_pin))?;
            let output = pin
                .request(gpio_cdev::LineRequestFlags::OUTPUT, 0, "fridge")
                .context("gpio output failed")?;
            Ok(FridgeOutput::Gpio(output))
        }
    }

    fn turn_off(&mut self) {
        info!("Turning fridge off");
        if let Err(e) = self.turn(false) {
            error!("Turning off failed: {e}");
        }
        self.last_off_time = Instant::now();
    }

    fn turn_on(&mut self) {
        info!("Turning fridge on");
        if let Err(e) = self.turn(true) {
            error!("Turning on failed: {e}")
        }
    }

    /// Generally use turn_on()/turn_off() instead.
    fn turn(&mut self, on: bool) -> Result<()> {
        match &self.output {
            FridgeOutput::Gpio(pin) => pin.set_value(on.into()).context("Couldn't change pin")?,
            FridgeOutput::Fake => debug!("fridge turns {}", if on { "on" } else { "off" }),
        }
        self.on = on;
        self.integrator.turn(on);
        Ok(())
    }

    /// Must be called after every state change.
    /// Turns the fridge off and on
    fn update(&mut self) {
        let fridge_min = self.params.fridge_setpoint - self.params.fridge_range_lower;
        let fridge_max = self.params.fridge_setpoint + self.params.fridge_range_upper;
        let wort_max = self.params.fridge_setpoint + self.params.fridge_difference;
        let off_duration = Instant::now() - self.last_off_time;

        debug!("off_duration {:?}", off_duration);

        if !self.params.running {
            if self.on {
                info!("Disabled, turning fridge off");
                self.turn_off();
            }
            return;
        }

        // handle broken wort sensor
        if self.temp_wort.is_none() {
            let invalid_time = Instant::now() - self.wort_valid_time;
            let skip = invalid_time < Duration::new(self.config.fridge_wort_invalid_time, 0);
            self.often_badwort.and_then(|| {
                if skip {
                    warn!("Has only been invalid for {:?}, waiting", invalid_time);
                } else {
                    warn!("Invalid wort sensor for {:?} secs", invalid_time);
                }
            });
            if skip {
                return;
            }
        }

        if self.temp_fridge.is_none() {
            self.often_badfridge
                .and_then(|| warn!("Invalid fridge sensor"));
        }

        // The main decision
        if self.on {
            let on_time = self.integrator.integrate().as_secs() as f32;
            let on_ratio = on_time / self.config.overshoot_interval as f32;
            let overshoot = self.params.overshoot_factor * on_ratio;
            debug!("on_percent {}, overshoot {}", on_ratio * 100.0, overshoot);

            match (self.temp_wort, self.temp_fridge) {
                (Some(t), _) if self.params.use_wort => {
                    if t - overshoot < self.params.fridge_setpoint {
                        info!("Wort has cooled enough, {t}° (overshoot {overshoot}°)");
                        self.turn_off();
                    }
                }
                (_, Some(t)) => {
                    if t < fridge_min {
                        warn!("Fridge off fallback, fridge {t}°, min {fridge_min}°");
                        if self.temp_wort.is_none() {
                            warn!(
                                "Wort has been invalid for {:?}",
                                Instant::now() - self.wort_valid_time
                            );
                        }
                        self.turn_off();
                    }
                }
                _ => (),
            }
        } else {
            // Also is the flag to turn it on.
            let mut turn_on_reason = None;

            match (self.temp_wort, self.temp_fridge) {
                (Some(t), _) if self.params.use_wort => {
                    if t >= wort_max {
                        turn_on_reason = Some((
                            format!("Wort is too hot {t}°, max {wort_max}°"),
                            log::Level::Info,
                        ));
                    }
                }
                (_, Some(t)) => {
                    if t >= fridge_max {
                        turn_on_reason = Some((
                            format!("Fridge too hot fallback, fridge {t}°, max {fridge_max}°"),
                            log::Level::Warn,
                        ));
                    }
                }
                _ => (),
            }

            if let Some((reason, loglevel)) = turn_on_reason {
                // The fridge should turn on

                // To avoid bad things happening to the fridge motor (?)
                // When it turns off don't start up again for at least FRIDGE_DELAY
                if off_duration < Duration::from_secs(self.config.fridge_delay) {
                    self.often_tooearly.and_then(|| {
                        log!(
                            loglevel,
                            "{}, but fridge skipping, too early ({} seconds left)",
                            reason,
                            self.config.fridge_delay - off_duration.as_secs()
                        )
                    });
                } else {
                    // Really turn on.
                    log!(loglevel, "{reason}");
                    self.turn_on();
                }
            }
        }
    }
}
