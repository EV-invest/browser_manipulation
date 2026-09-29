use std::{fmt, path::PathBuf};

use crate::Capture;

#[derive(Debug)]
pub struct Error {
	pub kind: Box<ErrorKind>,
	/// `None` without `Artifacts`; `Err` says why the page could not be captured.
	pub capture: Option<Box<Result<Capture, String>>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
	#[error("driver at {cli_js} is {found}, need patchright-core@{}", playwright_rs::PLAYWRIGHT_VERSION)]
	DriverMismatch { cli_js: String, found: String },
	#[error("profile {} is in use by another process", .0.display())]
	ProfileInUse(PathBuf),
	#[error("{what} {}", .path.display())]
	Io {
		what: &'static str,
		path: PathBuf,
		#[source]
		source: std::io::Error,
	},
	#[error("launching the browser")]
	Launch(#[source] playwright_rs::Error),
	#[error("navigating to {url}")]
	Navigation {
		url: String,
		#[source]
		source: playwright_rs::Error,
	},
	#[error("{op} `{target}` timed out")]
	Timeout {
		op: &'static str,
		target: String,
		#[source]
		source: playwright_rs::Error,
	},
	#[error("{op} `{target}`")]
	Selector {
		op: &'static str,
		target: String,
		#[source]
		source: playwright_rs::Error,
	},
	#[error("evaluating `{js}`")]
	Eval {
		js: String,
		#[source]
		source: playwright_rs::Error,
	},
	#[error("{op}")]
	Driver {
		op: &'static str,
		#[source]
		source: playwright_rs::Error,
	},
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.kind.fmt(f)
	}
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		std::error::Error::source(&self.kind)
	}
}

impl From<ErrorKind> for Error {
	fn from(kind: ErrorKind) -> Self {
		Self {
			kind: Box::new(kind),
			capture: None,
		}
	}
}

impl miette::Diagnostic for Error {
	fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
		let code = match *self.kind {
			ErrorKind::DriverMismatch { .. } => "bm::driver_mismatch",
			ErrorKind::ProfileInUse(_) => "bm::profile_in_use",
			ErrorKind::Io { .. } => "bm::io",
			ErrorKind::Launch(_) => "bm::launch",
			ErrorKind::Navigation { .. } => "bm::navigation",
			ErrorKind::Timeout { .. } => "bm::timeout",
			ErrorKind::Selector { .. } => "bm::selector",
			ErrorKind::Eval { .. } => "bm::eval",
			ErrorKind::Driver { .. } => "bm::driver",
		};
		Some(Box::new(code))
	}

	fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
		let hint = match *self.kind {
			ErrorKind::DriverMismatch { .. } => Some("PLAYWRIGHT_CLI_JS and PLAYWRIGHT_NODE_EXE come from this crate's flake (`packages.patchright`)"),
			ErrorKind::ProfileInUse(_) => Some("one profile, one process: stop the other holder first"),
			_ => None,
		};
		let capture = self.capture.as_deref().map(|c| match c {
			Ok(c) => format!("page {} at {}:\n  {}\n  {}", c.url, c.at, c.png.display(), c.html.display()),
			Err(e) => format!("capture failed: {e}"),
		});
		match (hint, capture) {
			(None, None) => None,
			(Some(h), None) => Some(Box::new(h)),
			(None, Some(c)) => Some(Box::new(c)),
			(Some(h), Some(c)) => Some(Box::new(format!("{h}\n{c}"))),
		}
	}
}
