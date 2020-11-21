/// Various helpers

use std::collections::HashMap;
use std::time::{Duration,Instant};

use std::cmp;
use std::cell::Cell;

#[derive(Debug,Clone)]
pub struct Readings {
    pub temps: HashMap<String, f32>,
}

impl Readings {
    pub fn new() -> Readings {
        Readings {
            temps: HashMap::new(),
        }
    }

    pub fn add(&mut self, name: &str, v: f32) {
        self.temps.insert(name.to_string(), v);
    }

    pub fn get_temp(&self, name: &str) -> Option<f32> {
        self.temps.get(name).map(|f| *f)
    }
}

/// Call closures with a rate limit. Useful for log message ratelimiting
pub struct NotTooOften {
    last: Cell<Instant>,
    limit: Duration,
}

impl NotTooOften {
    pub fn new(limit_secs: u64) -> Self {
        NotTooOften {
            limit: Duration::new(limit_secs, 0),
            // XXX why +1?
            last: Cell::new(Instant::now() - Duration::new(limit_secs+1, 0)),
        }
    }

    pub fn and_then<F, U>(&self, op: F) -> Option<U>
        where F: Fn() -> U {
        let now = Instant::now();
        if now - self.last.get() > self.limit {
            self.last.set(now);
            Some(op())
        } else {
            None
        }
    }
}

struct Period {
    start: Instant,
    end: Option<Instant>,
}

pub struct StepIntegrator {
    on_periods: Vec<Period>,
    limit: Duration,
}

impl StepIntegrator {
    pub fn new(limit: Duration) -> Self {
        StepIntegrator {
            on_periods: Vec::new(),
            limit,
        }
    }

    pub fn turn(&mut self, on: bool) {
        self.trim();

        let currently_on = match self.on_periods.last() {
            Some(l) => l.end.is_none(),
            None => {
                self.on_periods.push( Period { start: Instant::now(), end: None });
                return;
            },

        };

        if on == currently_on {
            // state is unchanged
            return;
        }

        if on {
            self.on_periods.push( Period { start: Instant::now(), end: None });
        } else {
            self.on_periods.last_mut().unwrap().end = Some(Instant::now());
        }
    }

    pub fn set_limit(&mut self, limit: Duration) {
        self.limit = limit;
        self.trim();
    }

    pub fn integrate(&self) -> Duration {
        let durs = self.on_periods.iter().map(|p| {
            let end = p.end.unwrap_or_else(|| Instant::now());
            end - p.start
        });
        durs.sum()
    }

    fn trim(&mut self) {
        let begin = Instant::now() - self.limit;

        self.on_periods.retain(|p| {
            // remove expired endtimes
            if let Some(e) = p.end {
                e >= begin && e != p.start
            } else {
                true
            }
        });

        for p in &mut self.on_periods {
            p.start = cmp::max(p.start, begin);
        }
    }
}

pub trait DurationFormat {
    fn as_short_str(&self) -> String;
}

impl DurationFormat for Duration {
    /// Returns a short string representing the [`Duration`]
    /// ```
    /// use std::time::Duration;
    /// use crate::types::DurationFormat;
    /// let d = Duration::from_secs(3600*49+607);
    /// assert_eq!(d.as_short_str(), "2d1h10m7s");
    /// ```
    fn as_short_str(&self) -> String
    {
        let left = self.as_secs();
        let mut parts = vec![];

        // TODO this could be a loop instead?
        let days = left / (60*60*24);
        if days > 0 {
            parts.push(format!("{}d", days));
        }
        let left = left - days*(60*60*24);

        let hours = left / (60*60);
        if hours > 0 || !parts.is_empty() {
            parts.push(format!("{}h", hours));
        }
        let left = left - hours*(60*60);

        let mins = left / 60;
        if mins > 0 || !parts.is_empty() {
            parts.push(format!("{}m", mins));
        }
        let left = left - mins*60;

        parts.push(format!("{}s", left));
        parts.concat()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn duration_format() {
        use std::time::Duration;
        use crate::types::DurationFormat;
        let d = Duration::from_secs(3600*49+607);
        assert_eq!(d.as_short_str(), "2d1h10m7s");
        let d = Duration::from_secs(3660);
        assert_eq!(d.as_short_str(), "1h1m0s");
        let d = Duration::from_secs(5);
        assert_eq!(d.as_short_str(), "5s");
        let d = Duration::from_millis(20);
        assert_eq!(d.as_short_str(), "0s");
    }
}

pub fn get_hg_version() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/hg-revid.txt"))
}
