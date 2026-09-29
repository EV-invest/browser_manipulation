use std::{ops::RangeInclusive, time::Duration};

use rand::{Rng, SeedableRng, rngs::StdRng};

/// Shapes every page action: how long to dwell before it, where the pointer lands, how it travels, how text is typed, how the wheel turns.
pub trait Motion {
	/// Actions are dispatched by the driver (actionability-checked, pointer teleports); `aim`/`path`/`keys` are consulted only for `scroll`.
	const NATIVE: bool;
	fn pause(&mut self, act: Act) -> Duration;
	fn aim(&mut self, bbox: Rect) -> Point;
	fn path(&mut self, from: Point, to: Point) -> Vec<(Point, Duration)>;
	fn keys(&mut self, text: &str) -> Keys;
	fn wheel(&mut self, dy: f64) -> Vec<(f64, Duration)>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, derive_more::Display)]
pub enum Act {
	Goto,
	Click,
	Fill,
	Press,
	Check,
	Select,
	Scroll,
	/// Of a held mouse button.
	Release,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
	pub x: f64,
	pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
	pub x: f64,
	pub y: f64,
	pub width: f64,
	pub height: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Keys {
	/// Inserted whole, as a paste would.
	Insert(String),
	/// Each key after its delay.
	Strokes(Vec<(Key, Duration)>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
	Char(char),
	Backspace,
}

/// No noise: the driver's own click/fill, no dwell.
#[derive(Clone, Copy, Debug, Default)]
pub struct Robot;

impl Motion for Robot {
	const NATIVE: bool = true;

	fn pause(&mut self, _: Act) -> Duration {
		Duration::ZERO
	}

	fn aim(&mut self, r: Rect) -> Point {
		Point {
			x: r.x + r.width / 2.,
			y: r.y + r.height / 2.,
		}
	}

	fn path(&mut self, _: Point, to: Point) -> Vec<(Point, Duration)> {
		vec![(to, Duration::ZERO)]
	}

	fn keys(&mut self, text: &str) -> Keys {
		Keys::Insert(text.to_owned())
	}

	fn wheel(&mut self, dy: f64) -> Vec<(f64, Duration)> {
		vec![(dy, Duration::ZERO)]
	}
}

/// Human-shaped: log-normal dwell, bezier travel with overshoot and jitter, typed text with typos, the wheel in notches.
#[derive(Debug, bon::Builder)]
pub struct Noise {
	/// Median dwell before an action.
	dwell: Duration,
	/// σ of `ln(dwell)`.
	dwell_spread: f64,
	/// Median pointer speed, px/s.
	speed: f64,
	/// Chance the pointer flies past its target and comes back.
	overshoot: f64,
	/// σ of per-step pointer offset, px.
	jitter: f64,
	/// Median gap between keystrokes.
	key_gap: Duration,
	/// σ of `ln(key_gap)`.
	key_spread: f64,
	/// Chance a keystroke is a wrong key, erased and retyped.
	typo: f64,
	/// px per wheel notch.
	notch: RangeInclusive<f64>,
	/// Between wheel notches.
	notch_gap: RangeInclusive<Duration>,
	/// Chance a gesture ends with notches back the other way.
	back: f64,
	/// How many notches back, when it does.
	back_notches: RangeInclusive<u32>,
	#[builder(with = |seed: u64| StdRng::seed_from_u64(seed))]
	seed: StdRng,
}

impl Noise {
	/// `median · e^(σ·z)`, z standard normal by Box–Muller.
	fn log_normal(&mut self, median: f64, sigma: f64) -> f64 {
		median * (sigma * self.normal()).exp()
	}

	fn normal(&mut self) -> f64 {
		let u = 1. - self.seed.random::<f64>();
		(-2. * u.ln()).sqrt() * (std::f64::consts::TAU * self.seed.random::<f64>()).cos()
	}

	fn bezier(&mut self, from: Point, to: Point, out: &mut Vec<(Point, Duration)>) {
		let (dx, dy) = (to.x - from.x, to.y - from.y);
		let dist = dx.hypot(dy);
		if dist < 1. {
			out.push((to, Duration::ZERO));
			return;
		}
		let (nx, ny) = (-dy / dist, dx / dist);
		let bend = |s: &mut Self, at: f64| {
			let off = s.normal() * dist * 0.15;
			Point {
				x: from.x + dx * at + nx * off,
				y: from.y + dy * at + ny * off,
			}
		};
		let (c1, c2) = (bend(self, 0.3), bend(self, 0.7));
		let steps = (dist / 12.).clamp(6., 60.) as usize;
		let total = self.log_normal(dist / self.speed, 0.25);
		let step = Duration::from_secs_f64(total / steps as f64);
		for i in 1..=steps {
			let t = i as f64 / steps as f64;
			let t = t * t * (3. - 2. * t);
			let u = 1. - t;
			let at = |a: f64, b: f64, c: f64, d: f64| u * u * u * a + 3. * u * u * t * b + 3. * u * t * t * c + t * t * t * d;
			let mut p = Point {
				x: at(from.x, c1.x, c2.x, to.x),
				y: at(from.y, c1.y, c2.y, to.y),
			};
			if i < steps {
				p.x += self.normal() * self.jitter;
				p.y += self.normal() * self.jitter;
			}
			out.push((p, step));
		}
	}
}

impl Motion for Noise {
	const NATIVE: bool = false;

	fn pause(&mut self, act: Act) -> Duration {
		let (median, spread) = match act {
			Act::Release => (self.key_gap, self.key_spread), // a button is held about as long as a key
			_ => (self.dwell, self.dwell_spread),
		};
		Duration::from_secs_f64(self.log_normal(median.as_secs_f64(), spread))
	}

	fn aim(&mut self, r: Rect) -> Point {
		let mut pick = |start: f64, len: f64| start + len * (0.5 + self.normal() * 0.15).clamp(0.1, 0.9);
		Point {
			x: pick(r.x, r.width),
			y: pick(r.y, r.height),
		}
	}

	fn path(&mut self, from: Point, to: Point) -> Vec<(Point, Duration)> {
		let mut out = Vec::new();
		if self.seed.random_bool(self.overshoot) {
			let past = self.seed.random_range(0.03..0.12);
			let beyond = Point {
				x: to.x + (to.x - from.x) * past,
				y: to.y + (to.y - from.y) * past,
			};
			self.bezier(from, beyond, &mut out);
			self.bezier(beyond, to, &mut out);
		} else {
			self.bezier(from, to, &mut out);
		}
		out
	}

	fn keys(&mut self, text: &str) -> Keys {
		let gap = |s: &mut Self| Duration::from_secs_f64(s.log_normal(s.key_gap.as_secs_f64(), s.key_spread));
		let mut out = Vec::with_capacity(text.len());
		for c in text.chars() {
			if c.is_ascii_alphabetic() && self.seed.random_bool(self.typo) {
				let wrong = (b'a' + self.seed.random_range(0..26)) as char;
				out.push((Key::Char(wrong), gap(self)));
				out.push((Key::Backspace, gap(self) * 2));
			}
			out.push((Key::Char(c), gap(self)));
		}
		Keys::Strokes(out)
	}

	fn wheel(&mut self, dy: f64) -> Vec<(f64, Duration)> {
		let mean = (self.notch.start() + self.notch.end()) / 2.;
		let n = (dy.abs() / mean).round().max(1.) as u32;
		let back = match self.seed.random_bool(self.back) {
			true => self.seed.random_range(self.back_notches.clone()),
			false => 0,
		};
		(0..n + back)
			.map(|i| {
				let notch = self.seed.random_range(self.notch.clone()) * dy.signum();
				let gap = if i == 0 { Duration::ZERO } else { self.seed.random_range(self.notch_gap.clone()) };
				(if i < n { notch } else { -notch }, gap)
			})
			.collect()
	}
}
