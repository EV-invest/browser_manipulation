#![feature(default_field_values)]
mod browser;
mod capture;
mod error;
mod motion;
mod tab;

pub use browser::{Browser, Launch};
pub use capture::{Artifacts, Capture};
pub use error::{Error, ErrorKind};
pub use motion::{Act, Key, Keys, Motion, Noise, Point, Rect, Robot};
pub use playwright_rs::Cookie;
pub use tab::{Request, Response, Shot, Tab};
