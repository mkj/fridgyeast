// TODO:
// - riker
//   - use monotonic clock
//   - timer.rs should use rx.recv_timeout(next_time) instead of rx.try_recv()
//       and then could remove cfg.frequency_millis
use std;

use std::time::{Duration,Instant};
use riker::actors::*;

use sysfs_gpio::{Direction, Pin};

use crate::params::Params;
use super::config::Config;
use super::params;
use super::sensor;
use super::types::*;

#[derive(Debug,Clone)]
pub struct Tick;

#[derive(Debug,Clone)]
pub struct GetOffTime;

#[actor(Params, Tick, Readings, GetOffTime)]
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
                _sender: Sender) {
        self.params = p;
        println!("fridge set_params {:?}", self.params);

        self.tick(ctx);
    }
}

impl Receive<Tick> for Fridge {
    fn receive(&mut self,
                ctx: &Context<Self::Msg>,
                _tick: Tick,
                _sender: Sender) {
        self.tick(ctx);
    }
}

impl Receive<GetOffTime> for Fridge {
    fn receive(&mut self,
                ctx: &Context<Self::Msg>,
                _: GetOffTime,
                sender: Sender) {
        let off_time = Instant::now() - self.last_off_time;
        let tosend = off_time.as_millis() as u64;
        sender.as_ref()
        .unwrap()
        .try_tell(tosend, Some(ctx.myself().into()));
    }
}


enum FridgeOutput {
    Gpio(Pin),
    Fake,
}

impl Drop for Fridge {
    fn drop(&mut self) {
        // safety fridge off 
        self.turn(false);
    }
}

impl ActorFactoryArgs<&'static Config> for Fridge {
    fn create_args(config: &'static Config) -> Self {
        let mut f = Fridge { 
            config,
            params: Params::load(&config),
            on: false,
            temp_wort: None,
            temp_fridge: None,
            last_off_time: Instant::now(),
            wort_valid_time: Instant::now() - Duration::new(config.fridge_wort_invalid_time, 100),
            integrator: StepIntegrator::new(Duration::new(1, 0)),
            output: Self::make_output(&config),
        };

        if config.nowait {
            f.last_off_time -= Duration::new(config.fridge_delay, 1);
        }

        f
    }
}

impl Fridge {
    fn make_output(config: &Config) -> FridgeOutput {
        if config.testmode {
            FridgeOutput::Fake
        } else {
            let pin = Pin::new(config.fridge_gpio_pin.into());
            // XXX better error handling?
            pin.export().expect("Exporting fridge gpio failed");
            // 'Low' is direction=out+value=0
            pin.set_direction(Direction::Low).expect("Fridge gpio direction failed");
            FridgeOutput::Gpio(pin)
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
        self.on = on;
        self.integrator.turn(on)
    }

    // Turns the fridge off and on
    fn compare_temperatures(&mut self) {
        let fridge_min = self.params.fridge_setpoint - self.params.fridge_range_lower;
        let fridge_max = self.params.fridge_setpoint - self.params.fridge_range_upper;
        let wort_max = self.params.fridge_setpoint + self.params.fridge_difference;
        let off_time = Instant::now() - self.last_off_time;

        // Or elsewhere?
        self.integrator.set_limit(Duration::new(self.params.overshoot_delay, 0));

        // Safety to avoid bad things happening to the fridge motor (?)
        // When it turns off don't start up again for at least FRIDGE_DELAY
        if !self.on && off_time < Duration::new(self.config.fridge_delay, 0) {
            info!("fridge skipping, too early");
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
            debug!("fridge is on");
            let on_time = self.integrator.integrate().as_secs() as f32;
            let on_ratio = on_time / self.params.overshoot_delay as f32;

            let overshoot = self.params.overshoot_factor as f32 * on_ratio;
            debug!("on_percent {}, overshoot {}", on_ratio * 100.0, overshoot);

            let mut turn_off = false;
            if self.temp_wort.is_some() && !self.params.nowort {
                let t = self.temp_wort.unwrap();
                // use the wort temperature
                if t - overshoot < self.params.fridge_setpoint {
                    info!("wort has cooled enough, {temp}º (overshoot {overshoot}º = {factor} × {percent}%)",
                         temp = t, overshoot = overshoot,
                         factor = self.params.overshoot_factor,
                         percent = on_ratio*100.0);
                    turn_off = true;
                }
            } else if let Some(t) = self.temp_fridge {
                // use the fridge temperature
                if t < fridge_min {
                    warn!("fridge off fallback, fridge {}, min {}", t, fridge_min);
                    if self.temp_wort.is_none() {
                        warn!("wort has been invalid for {:?}", Instant::now() - self.wort_valid_time);
                    }
                    turn_off = true;
                }
            }
            if turn_off {
                self.turn_off();
            }
        } else {
            debug!("fridge is off. fridge {:?} max {:?}. wort {:?} max {:?}",
                self.temp_fridge, fridge_max, self.temp_wort, wort_max);
            let mut turn_on = false;
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
                    warn!("fridge too hot fallback, fridge {}°, max {}°", t, fridge_max);
                    turn_on = true;
                }
            }

            if turn_on {
                self.turn_on()
            }
        }
    }

    fn next_wakeup(&self) -> Duration {
        let millis = 8000; // XXX fixme
        let dur = Duration::from_millis(millis);
        dur
    }

    /// Must be called after every state change. Turns the fridge on/off as required and
    /// schedules any future wakeups based on the present (new) state
    /// Examples of wakeups events are
    /// 
    ///  * overshoot calculation
    ///  * minimum fridge-off time
    ///  * invalid wort timeout
    /// All specified in next_wakeup()
    fn tick(&mut self,
        ctx: &Context<<Self as Actor>::Msg>) {
        debug!("tick");

        self.compare_temperatures();

        // Sets the next self-wakeup timeout
        let dur = self.next_wakeup();
        ctx.schedule_once(dur, ctx.myself(), None, Tick);
    }
}
