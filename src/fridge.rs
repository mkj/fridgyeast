use std::time::{Duration,Instant};
use riker::actors::*;

use anyhow::Result;
use anyhow::Context as AHContext;

use sysfs_gpio::{Direction, Pin};
use serde::Serialize;
use serde_json::ser::to_string_pretty;

use crate::params::Params;
use super::config::Config;

use super::sensor;
use super::types::*;

#[derive(Debug,Clone)]
pub struct Tick;

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
}

#[actor(Params, Tick, Readings, GetStatus)]
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

    have_wakeup: bool,

    often_tooearly: NotTooOften,
}

impl Actor for Fridge {
    type Msg = FridgeMsg;
    fn recv(&mut self,
                ctx: &Context<Self::Msg>,
                msg: Self::Msg,
                sender: Sender) {
        // Pass it along to the specific handler
        self.receive(ctx, msg, sender);
    }

    fn pre_start(&mut self, ctx: &Context<Self::Msg>) {
        // Build all the child actors
        let params_chan : ChannelRef<Params> = channel("params", ctx).unwrap();
        params_chan.tell(Subscribe {actor: Box::new(ctx.myself()), topic: "params".into()}, None);

        let sensor_chan : ChannelRef<Readings> = channel("readings", ctx).unwrap();
        sensor_chan.tell(Subscribe {actor: Box::new(ctx.myself()), topic: "readings".into()}, None);
        if self.config.testmode {
            ctx.actor_of_args::<sensor::TestSensor, _>("sensor", (self.config, sensor_chan.clone())).unwrap()
        } else {
            ctx.actor_of_args::<sensor::OneWireSensor, _>("sensor", (self.config, sensor_chan.clone())).unwrap()
        };

        // Start the timer going
        self.tick(ctx);
    }
}

impl Receive<Readings> for Fridge {
    fn receive(&mut self,
                ctx: &Context<Self::Msg>,
                r: Readings,
                _sender: Sender) {
        self.temp_wort = r.get_temp(&self.config.wort_name);
        self.temp_fridge = r.get_temp(&self.config.fridge_name);

        if self.temp_wort.is_some() {
            self.wort_valid_time = Instant::now();
        }

        self.tick(ctx);
    }
}

impl Receive<Params> for Fridge {
    fn receive(&mut self,
                ctx: &Context<Self::Msg>,
                p: Params,
                sender: Sender) {
        self.params = p;
        let pp = to_string_pretty(&self.params).expect("Failed serialising params");
        info!("New params: {}", pp);

        // quickly update the fridge for real world interactivity
        self.tick(ctx);

        let res = self.params.save(self.config);

        if let Err(e) = &res {
            // log it ...
            error!("Failed saving params: {}", e);
        }

        // ... and return it
        let res = res.map_err(|e| e.to_string());
        if let Some(s) = sender {
            s.try_tell(res, None).unwrap_or_else(|_| {
                error!("This shouldn't happen, failed sending params");
            })
        }

    }
}

impl Receive<Tick> for Fridge {
    fn receive(&mut self,
                ctx: &Context<Self::Msg>,
                _tick: Tick,
                _sender: Sender) {
        self.have_wakeup = false;
        self.tick(ctx);
    }
}

impl Receive<GetStatus> for Fridge {
    fn receive(&mut self,
                _ctx: &Context<Self::Msg>,
                _: GetStatus,
                sender: Sender) {
        if let Some(s) = sender {
            let status = Status {
                params: self.params.clone(),
                on: self.on,
                temp_wort: self.temp_wort,
                temp_fridge: self.temp_fridge,
                off_duration: Instant::now() - self.last_off_time,
                fridge_delay: Duration::from_secs(self.config.fridge_delay),
            };
            s.try_tell(status, None).unwrap_or_else(|_| {
                error!("This shouldn't happen, failed sending params");
            })
        }
    }
}


enum FridgeOutput {
    Gpio(Pin),
    Fake,
}

impl Drop for Fridge {
    fn drop(&mut self) {
        if self.on {
            info!("Fridge turns off at shutdown");
        }
        self.turn(false);
    }
}

impl ActorFactoryArgs<&'static Config> for Fridge {
    fn create_args(config: &'static Config) -> Self {
        // XXX how can we handle failing actor creation better?
        let output = Self::make_output(&config).unwrap();

        let mut f = Fridge { 
            config,
            params: Params::load(&config),
            on: false,
            temp_wort: None,
            temp_fridge: None,
            last_off_time: Instant::now(),
            wort_valid_time: Instant::now() - Duration::new(config.fridge_wort_invalid_time, 100),
            integrator: StepIntegrator::new(Duration::new(1, 0)),
            output,
            often_tooearly: NotTooOften::new(300),
            have_wakeup: false,
        };

        let pp = to_string_pretty(&f.params).expect("Failed serialising params");
        info!("Starting with params: {}", pp);

        if config.nowait {
            f.last_off_time -= Duration::new(config.fridge_delay, 1);
        }

        f
    }
}

impl Fridge {
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
        self.turn(false);
    }

    fn turn_on(&mut self) {
        info!("Turning fridge on");
        self.turn(true);
    }

    fn turn(&mut self, on: bool) {
        match self.output {
            FridgeOutput::Gpio(pin) => pin.set_value(on.into()).unwrap_or_else(|e| {
                error!("Couldn't change fridge pin to {}: {}", on, e);
            }),
            FridgeOutput::Fake => debug!("fridge turns {}", if on {"on"} else {"off"}),
        }
        if !on {
            self.last_off_time = Instant::now();
        }
        self.on = on;
        self.integrator.turn(on)
    }

    // Turns the fridge off and on
    fn compare_temperatures(&mut self) {
        let fridge_min = self.params.fridge_setpoint - self.params.fridge_range_lower;
        let fridge_max = self.params.fridge_setpoint - self.params.fridge_range_upper;
        let wort_max = self.params.fridge_setpoint + self.params.fridge_difference;
        let off_duration = Instant::now() - self.last_off_time;

        debug!("off_duration {:?}", off_duration);

        // Or elsewhere?
        self.integrator.set_limit(Duration::from_secs(self.config.overshoot_delay));

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
            let on_ratio = on_time / self.config.overshoot_delay as f32;

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
            } 

            if let Some(t) = self.temp_fridge {
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

    /// Must be called after every state change. 
    /// Turns the fridge on/off as required and schedules a 
    /// future wakeup.
    fn tick(&mut self,
        ctx: &Context<<Self as Actor>::Msg>) {

        self.compare_temperatures();

        if !self.have_wakeup {
            // Arbitrary 10 secs, enough to notice invalid wort or fridge delay
            ctx.schedule_once(Duration::from_secs(10), ctx.myself(), None, Tick);
            self.have_wakeup = true;
        }
    }
}
