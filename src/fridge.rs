#[allow(unused_imports)]
use {
    log::{debug, error, info, warn},
    anyhow::{Result,Context,bail,anyhow},
};

use async_trait::async_trait;
use crate::actzero_pubsub::Subscriber;
use std::time::{Duration,Instant};

use act_zero::*;
use act_zero::runtimes::async_std::spawn_actor;
use act_zero::runtimes::async_std::Timer;
use act_zero::timer::Tick;

use sysfs_gpio::{Direction, Pin};
use serde::Serialize;
use serde_json::ser::to_string_pretty;

use crate::params::Params;
use super::config::Config;

use super::sensor;
use super::types::*;

#[derive(Debug,Clone)]
pub struct GetStatus;

#[derive(Debug,Clone,Serialize)]
pub struct Status {
    pub params: Params,
    pub on: bool,
    pub temp_wort: Option<f32>,
    pub temp_fridge: Option<f32>,
    pub off_duration: Duration,
    pub fridge_delay: Duration,

    // from config
    pub overshoot_interval: u64,
    pub overshoot_factor: f32,
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

    often_tooearly: NotTooOften,

    sensor: Option<Addr<dyn Actor>>,
}

enum FridgeOutput {
    Gpio(Pin),
    Fake,
}

impl Drop for Fridge {
    fn drop(&mut self) {
        debug!("Fridge drop");
        if self.on {
            info!("Fridge turns off at shutdown");
        }
        self.turn_off();
        debug!("Fridge drop complete")
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
        self.timer.set_interval_weak(addr.downgrade(), Duration::from_secs(10));
        Produces::ok(())
    }

    async fn error(&mut self, error: ActorError) -> bool {
        warn!("Ignoring error from Fridge actor: {:?}", error);
        false
    }
}

impl Fridge {
    pub fn try_new(config: &'static Config) -> Result<Self> {
        let output = Self::make_output(&config)?;

        let mut f = Fridge { 
            config,
            params: Params::load(&config),
            on: false,
            temp_wort: None,
            temp_fridge: None,
            last_off_time: Instant::now(),
            wort_valid_time: Instant::now() - Duration::new(config.fridge_wort_invalid_time, 100),
            integrator: StepIntegrator::new(Duration::from_secs(config.overshoot_interval)),
            output,
            often_tooearly: NotTooOften::new(300),
            timer: Timer::default(),
            started: Instant::now(),
            sensor: None,
        };

        // Early check the fridge can turn off
        f.turn(false).context("Initial fridge turn-off")?;

        Ok(f)
    }

    pub async fn add_readings(&mut self, r: Readings) {
        debug!("add_readings {:?}", r);
        self.temp_wort = r.get_temp(&self.config.wort_name);
        self.temp_fridge = r.get_temp(&self.config.fridge_name);

        if self.temp_wort.is_some() {
            self.wort_valid_time = Instant::now();
        }

        self.update();
    }

    pub async fn set_params(&mut self, p: Params) -> ActorResult<Result<()>> {
        self.params = p;
        let pp = to_string_pretty(&self.params).expect("Failed serialising params");
        info!("New params: {}", pp);

        // quickly update the fridge for real world interactivity
        self.update();

        let res = self.params.save(self.config);

        if let Err(e) = &res {
            // log it too
            error!("Failed saving params: {}", e);
        }
        // Produces::ok(res)
        Err(anyhow!("failedfake").into())
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
            overshoot_factor: self.config.overshoot_factor,
            sensor_interval: self.config.sensor_interval,
            version: get_hg_version(),
            uptime: Instant::now() - self.started,
        };
        Produces::ok(s)
    }

    fn make_output(config: &Config) -> Result<FridgeOutput> {
        if config.testmode || config.dryrun {
            Ok(FridgeOutput::Fake)
        } else {
            let pin = Pin::new(config.fridge_gpio_pin.into());
            pin.export().context("Exporting fridge GPIO failed")?;
            // Direction::Low is direction=out+value=0
            pin.set_direction(Direction::Low).context("Exporting fridge gpio failed")?;
            Ok(FridgeOutput::Gpio(pin))
        }
    }

    fn turn_off(&mut self) {
        info!("Turning fridge off");
        println!("\nprint turn off\n");
        self.turn(false).unwrap_or_else(|e| error!("Turning off failed: {}", e));
        self.last_off_time = Instant::now();
    }

    fn turn_on(&mut self) {
        info!("Turning fridge on");
        self.turn(true).unwrap_or_else(|e| error!("Turning on failed: {}", e));
    }

    /// Generally use turn_on()/turn_off() instead.
    fn turn(&mut self, on: bool) -> Result<()> {
        match self.output {
            FridgeOutput::Gpio(pin) => pin.set_value(on.into()).context("Couldn't change pin")?,
            FridgeOutput::Fake => debug!("fridge turns {}", if on {"on"} else {"off"}),
        }
        self.on = on;
        self.integrator.turn(on);
        Ok(())
    }

    /// Must be called after every state change. 
    /// Turns the fridge off and on
    fn update(&mut self) {
        let fridge_min = self.params.fridge_setpoint - self.params.fridge_range_lower;
        let fridge_max = self.params.fridge_setpoint - self.params.fridge_range_upper;
        let wort_max = self.params.fridge_setpoint + self.params.fridge_difference;
        let off_duration = Instant::now() - self.last_off_time;

        debug!("off_duration {:?}", off_duration);

        // Safety to avoid bad things happening to the fridge motor (?)
        // When it turns off don't start up again for at least FRIDGE_DELAY
        if !self.on && off_duration < Duration::from_secs(self.config.fridge_delay) {
            self.often_tooearly.and_then(|| info!("Fridge skipping, too early ({} seconds left)",
                self.config.fridge_delay - off_duration.as_secs()));
            return;
        }

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
            warn!("Invalid wort sensor for {:?} secs", invalid_time);
            if invalid_time < Duration::new(self.config.fridge_wort_invalid_time, 0) {
                warn!("Has only been invalid for {:?}, waiting", invalid_time);
                return;
            }
        }

        if self.temp_fridge.is_none() {
            warn!("Invalid fridge sensor");
        }

        if self.on {
            let on_time = self.integrator.integrate().as_secs() as f32;
            let on_ratio = on_time / self.config.overshoot_interval as f32;

            let overshoot = self.config.overshoot_factor as f32 * on_ratio;
            debug!("on_percent {}, overshoot {}", on_ratio * 100.0, overshoot);

            let mut turn_off = false;
            if self.temp_wort.is_some() && !self.params.nowort {
                let t = self.temp_wort.unwrap();
                // use the wort temperature
                if t - overshoot < self.params.fridge_setpoint {
                    info!("Wort has cooled enough, {temp}º (overshoot {overshoot}º = {factor} × {percent}%)",
                         temp = t, overshoot = overshoot,
                         factor = self.config.overshoot_factor,
                         percent = on_ratio*100.0);
                    turn_off = true;
                }
            } else if let Some(t) = self.temp_fridge {
                // use the fridge temperature
                if t < fridge_min {
                    warn!("Fridge off fallback, fridge {}, min {}", t, fridge_min);
                    if self.temp_wort.is_none() {
                        warn!("Wort has been invalid for {:?}", Instant::now() - self.wort_valid_time);
                    }
                    turn_off = true;
                }
            }
            if turn_off {
                self.turn_off();
            }
        } else {
            let mut turn_on = false;
            // TODO can use if let Some(t) = ... && ...
            // once https://github.com/rust-lang/rust/issues/53667 is done
            if self.temp_wort.is_some() && !self.params.nowort {
                // use the wort temperature
                let t = self.temp_wort.unwrap();
                if t >= wort_max {
                    info!("Wort is too hot {}°, max {}°", t, wort_max);
                    turn_on = true;
                }
            } else if let Some(t) = self.temp_fridge {
                if t >= fridge_max {
                    warn!("Fridge too hot fallback, fridge {}°, max {}°", t, fridge_max);
                    turn_on = true;
                }
            }

            if turn_on {
                self.turn_on()
            }
        }
    }
}
