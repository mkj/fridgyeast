#[allow(unused_imports)]
use {
    log::{debug, error, info, warn,log},
    anyhow::{Result,Context,bail,anyhow},
};

use std::{alloc::System, time::{Duration,SystemTime}};
use std::path::Path;

use act_zero::*;
use act_zero::runtimes::async_std::Timer;
use act_zero::timer::Tick;

use async_trait::async_trait;

use async_std::task::{block_on};

use rusqlite::{OptionalExtension,params};

use crate::rusqlmem::RusqlMem;

pub struct TimeSeries {
	quant_secs: u64,
	history: Duration,
	db: RusqlMem,

    prune_timer: Timer,
    flush_timer: Timer,
}

impl TimeSeries {
	pub fn new(p: &Path, quant_secs: u64, history: Duration) -> Result<Self> {
		let ts = TimeSeries {
			quant_secs,
			history,
			db: RusqlMem::new(p, Self::init_schema)?,
			prune_timer: Timer::default(),
			flush_timer: Timer::default(),
		};
		Ok(ts)
	}

	pub fn add(&self, time: SystemTime, value: f32) -> Result<()> {
		let mut conn = self.db.db();
		let t = conn.transaction()?;
		let dif = Self::time_to_int(time)?;
		let quant_time = dif - (dif % self.quant_secs);
		let (oldval, count): (f32, u32) = t.query_row("select value, count from points where time = ?", [quant_time],
			|r| Ok((r.get(0)?, r.get(1)?)))
			.optional()?
			.unwrap_or((0.0, 0));
		// scale by existing accumulated values
		let c = count as f32;
		let v = oldval*c/(c+1.0) + value/(c+1.0);
		if count > 0 {
			t.execute("delete from points where time = ?", [&quant_time])?;
		}
		t.execute("insert into points values (?, ?, ?)", params![quant_time, v, count+1])?;
		t.commit()?;
		Ok(())
	}

	pub fn get(&self) -> Result<Vec<(SystemTime, f32)>> {
		self.db.db()
		.prepare("select time, value from points where time >= ?")?
		.query_map(params![self.earliest()], |r| {
			let t: SystemTime = Self::int_to_time(r.get(0)?);
			let v: f32 = r.get(1)?;
			Ok((t, v))
		})?
		.map(|r| r.context("SQL query"))
		.collect()
	}

	fn time_to_int(time: SystemTime) -> Result<u64> {
		Ok(time.duration_since(SystemTime::UNIX_EPOCH).context("Time before 1970")?.as_secs())
	}

	fn int_to_time(i: u64) -> SystemTime {
		SystemTime::UNIX_EPOCH + Duration::from_secs(i)
	}

	fn prune(&self) -> Result<()> {
		self.db.db().execute("delete from points where time < ?", params![self.earliest()])?;
		Ok(())
	}

	fn earliest(&self) -> u64 {
		Self::time_to_int(SystemTime::now() - self.history)
		.unwrap_or(0)
	}

	fn init_schema(t: &mut rusqlite::Transaction) -> Result<()> {
		t.execute("create table points (time, value, count)", [])?;
		t.execute("create index points_time on points (time)", [])?;
		Ok(())
	}
}

const FLUSH_INTERVAL: Duration = Duration::from_secs(60 * 15);

#[async_trait]
impl Actor for TimeSeries {
    async fn started(&mut self, addr: Addr<Self>) -> ActorResult<()> {
        self.prune_timer.set_interval_weak(addr.downgrade(), self.history * 2);
        self.flush_timer.set_interval_weak(addr.downgrade(), FLUSH_INTERVAL);
        Produces::ok(())
    }

    async fn error(&mut self, error: ActorError) -> bool {
        warn!("Ignoring error from TimeSeries actor: {:?}", error);
        false
    }
}

#[async_trait]
impl Tick for TimeSeries {
    async fn tick(&mut self) -> ActorResult<()> {
        if self.prune_timer.tick() {
            if let Err(e) = self.prune() {
            	warn!("{}", e)
            }
        }
        if self.flush_timer.tick() {
            if let Err(e) = self.db.flush().await {
            	warn!("{}", e)
            }
        }
        Produces::ok(())
    }
}

#[cfg(test)]
mod tests {
use super::*;

#[test]
fn new_timeseries() -> Result<()> {
	let t = TimeSeries::new(Path::new("ff.db"), 3, Duration::from_secs(60*60*24*3))?;
	t.add(SystemTime::now(), 3.2f32)?;
	block_on(t.db.flush())?;
	Ok(())
}

}
