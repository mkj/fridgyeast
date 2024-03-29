#[allow(unused_imports)]
use {
    log::{debug, error, info, warn,log},
    anyhow::{Result,Context,bail,anyhow},
};

use std::path::Path;

use act_zero::*;
use act_zero::runtimes::async_std::Timer;
use act_zero::timer::Tick;

use async_trait::async_trait;

use async_std::task::block_on;

use rusqlite::{OptionalExtension,params};

use chrono::{Duration,DateTime,offset::Utc};

use crate::rusqlmem::RusqlMem;

pub struct TimeSeries {
	quantise_secs: u64,
	history: Duration,
	db: RusqlMem,

    prune_timer: Timer,
    flush_timer: Timer,
}

pub type Seq = Vec<(DateTime<Utc>, f32)>;

pub const DEFAULT_SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10*60);

impl TimeSeries {
	pub fn new(p: &Path, quantise_secs: u64, history: Duration) -> Result<Self> {
		let ts = TimeSeries {
			quantise_secs,
			history,
			db: RusqlMem::new(p, Self::init_schema)?,
			prune_timer: Timer::default(),
			flush_timer: Timer::default(),
		};
		Ok(ts)
	}

	/// Inserts a new datapoint. If points exist within the quantised time
	/// window the new point will be accumulated as an average.
	pub async fn add(&self, name: &str, value: f32) -> ActorResult<()> {
		let mut conn = self.db.db();
		let t = conn.transaction()?;
		let dif = Utc::now().timestamp() as u64;
		let quant_time = dif - (dif % self.quantise_secs);
		let (oldval, count): (f32, u32) = t.query_row(
			"select value, count from points where name = ? and time = ?", params![name, quant_time],
			|r| Ok((r.get(0)?, r.get(1)?)))
			.optional()?
			.unwrap_or((0.0, 0));
		// scale by existing accumulated values
		let c = count as f32;
		let v = oldval*c/(c+1.0) + value/(c+1.0);
		if count > 0 {
			t.execute("delete from points where name = ? and time = ?", params![name, quant_time])?;
		}
		t.execute("insert into points values (?, ?, ?, ?)", params![quant_time, name, v, count+1])?;
		t.commit()?;
		Produces::ok(())
	}

	/// Inserts a new datapoint for a stepwise series.
	pub async fn add_step(&self, name: &str, value: f32) -> ActorResult<()> {
		println!("add_step {name} {value}");
		let mut conn = self.db.db();
		let t = conn.transaction()?;
		let now = Utc::now().timestamp() as u64;
		let oldval: Option<f32> = t.query_row(
			"select value from step_points where name = ? and time <= ? order by time desc limit 1", params![name, now],
			|r| r.get(0))
			.optional()?;
		println!("now {now} oldval {oldval:?} value {value}");
		if let Some(ov) = oldval {
			if ov == value {
				return Produces::ok(());
			}
		}
		t.execute("insert into step_points values (?, ?, ?)", params![now, name, value])?;
		t.commit()?;
		Produces::ok(())
	}

	/// Returns points within the history window
	/// _TODO_: also return one point prior the the window?
	pub async fn get(&self, name: String, start: DateTime<Utc>) -> ActorResult<Seq> {
		let r: Result<Seq> = self.db.db()
		.prepare("select time, value from points where name = ? and time >= ? order by time")?
		.query_map(params![name, start.timestamp()], |r| {
			let t = Self::int_to_time(r.get(0)?);
			let v: f32 = r.get(1)?;
			Ok((t, v))
		})?
		.map(|r| r.context("SQL query"))
		.collect();
		Produces::ok(r?)
	}

	/// Returns points within the history window
	pub async fn get_step(&self, name: String, start: DateTime<Utc>) -> ActorResult<Seq> {
		let d = self.db.db();

		// one point prior to the the window
		let mut p = d
			.prepare("select time, value from step_points where name = ? and time < ? order by time desc limit 1")?;
		let r1 = p.query_map(params![name, start.timestamp()], |r| {
			let t = Self::int_to_time(r.get(0)?);
			let v: f32 = r.get(1)?;
			Ok((t, v))
		})?

		.map(|r| r.context("SQL query"));
		let mut p = d
			.prepare("select time, value from step_points where name = ? and time >= ? order by time")?;
		let r = p.query_map(params![name, start.timestamp()], |r| {
			let t = Self::int_to_time(r.get(0)?);
			let v: f32 = r.get(1)?;
			Ok((t, v))
		})?
		.map(|r| r.context("SQL query"));

		let mut prev = None;
		let mut res = vec![];
		for a in r1.chain(r) {
			let a = a?;
			if let Some(p) = prev {
				res.push((a.0, p));
			};
			prev = Some(a.1);
			res.push(a);
		}

		if let Some(prev) = prev {
			res.push((Utc::now(), prev))
		}

		Produces::ok(res)
	}

	pub async fn save(&self) -> ActorResult<()> {
		self.db.flush().await?;
		Produces::ok(())
	}

	fn int_to_time(i: i64) -> DateTime<Utc> {
		DateTime::from_timestamp(i, 0).unwrap()
	}

	// TODO: keep the last point before as well
	fn prune(&self) -> Result<()> {
		let cutoff = self.earliest();
		self.db.db().execute("delete from points where time < ?", params![cutoff])?;
		self.db.db().execute("delete from step_points where time < ?", params![cutoff])?;
		debug!("Pruned memory db prior to {}", cutoff);
		Ok(())
	}

	fn earliest(&self) -> i64 {
		(Utc::now() - self.history).timestamp()
	}

	fn init_schema(t: &mut rusqlite::Transaction) -> Result<()> {
		t.execute("create table points (time, name, value, count)", [])?;
		t.execute("create table step_points (time, name, value)", [])?;
		t.execute("create unique index points_index on points (name, time)", [])?;
		t.execute("create unique index step_index on points (name, time)", [])?;
		Ok(())
	}
}

#[async_trait]
impl Actor for TimeSeries {
    async fn started(&mut self, addr: Addr<Self>) -> ActorResult<()> {
        self.prune_timer.set_interval_weak(addr.downgrade(), (self.history * 2).to_std()?);
        self.flush_timer.set_interval_weak(addr.downgrade(), DEFAULT_SAVE_INTERVAL);
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
            	warn!("Error pruning TimeSeries {:?}", e)
            }
        }
        if self.flush_timer.tick() {
            if let Err(e) = self.db.flush().await {
            	warn!("Error flushing TimeSeries {:?}", e)
            }
        }
        Produces::ok(())
    }
}

impl Drop for TimeSeries {
    fn drop(&mut self) {
        if let Err(e) = block_on(self.db.flush()) {
        	warn!("Error flushing TimeSeries {:?}", e)
        }
    }
}

#[cfg(test)]
mod tests {
use super::*;

#[test]
fn new_timeseries() -> Result<()> {
	let t = TimeSeries::new(Path::new("ff.db"), 3, Duration::from_secs(60*60*24*3))?;
    send!(t.add("wort", 3.2f32));
	block_on(t.db.flush())?;
	Ok(())
}

}
