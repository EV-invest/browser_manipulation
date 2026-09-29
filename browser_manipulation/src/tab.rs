use std::{
	sync::{Arc, Mutex},
	time::Duration,
};

use futures::{Stream, StreamExt as _, channel::mpsc};
use playwright_rs::{Cookie, Locator, Page, ScreenshotOptions, WaitForOptions, WaitForState, protocol::ResponseObject};
use serde::{Serialize, de::DeserializeOwned};

use crate::{Act, Browser, Capture, Error, ErrorKind, Key, Keys, Motion, Point, Rect};

/// A hung page must not hold the error it failed with.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

/// URL part → subscriber, fed by the one `on_response` handler a tab registers.
type Listeners = Arc<Mutex<Vec<(String, mpsc::UnboundedSender<ResponseObject>)>>>;
pub enum Shot<'s> {
	Full,
	Element(&'s str),
}

pub struct Response {
	pub url: String,
	pub status: u16,
	pub body: Vec<u8>,
}

pub struct Tab<'b, M: Motion> {
	browser: &'b Browser<M>,
	page: Page,
	mouse: Point,
	listeners: Option<Listeners>,
}

impl<'b, M: Motion> Tab<'b, M> {
	pub(crate) fn new(browser: &'b Browser<M>, page: Page) -> Self {
		Self {
			browser,
			page,
			mouse: Point { x: 0., y: 0. }, // where the driver's mouse starts
			listeners: None,
		}
	}

	pub async fn goto(&mut self, url: &str) -> Result<(), Error> {
		self.pause(Act::Goto).await;
		let r = self.page.goto(url, None).await.map(drop).map_err(|source| ErrorKind::Navigation { url: url.to_owned(), source });
		self.captured(r, "goto").await
	}

	/// For every action and navigation on this tab; the driver's own is 30s.
	pub async fn set_timeout(&self, timeout: Duration) {
		let ms = timeout.as_secs_f64() * 1000.;
		self.page.set_default_timeout(ms).await;
		self.page.set_default_navigation_timeout(ms).await;
	}

	pub fn url(&self) -> String {
		self.page.url()
	}

	pub async fn wait_for_url(&mut self, url_glob: &str) -> Result<(), Error> {
		let r = self.page.wait_for_url(url_glob, None).await.map_err(|source| classify("wait_for_url", url_glob, source));
		self.captured(r, "wait_for_url").await
	}

	/// Index of the first selector to become visible.
	pub async fn wait_for_any(&mut self, selectors: &[&str]) -> Result<usize, Error> {
		assert!(!selectors.is_empty());
		let waits = selectors.iter().enumerate().map(|(i, s)| {
			let l = self.page.locator(*s).first();
			Box::pin(async move { l.wait_for(WaitForOptions::builder().state(WaitForState::Visible).build()).await.map(|()| i) })
		});
		let r = futures::future::select_ok(waits)
			.await
			.map(|(i, _)| i)
			.map_err(|source| classify("wait_for_any", &selectors.join(" | "), source));
		self.captured(r, "wait_for_any").await
	}

	pub async fn click(&mut self, selector: &str) -> Result<(), Error> {
		self.pause(Act::Click).await;
		let r = self.reach_and_click(selector).await;
		self.captured(r, "click").await
	}

	/// Replaces the field's content.
	pub async fn fill(&mut self, selector: &str, text: &str) -> Result<(), Error> {
		self.pause(Act::Fill).await;
		let keys = self.motion().keys(text);
		let r = async {
			match keys {
				Keys::Insert(text) => self.page.locator(selector).fill(&text, None).await.map_err(|s| classify("fill", selector, s)),
				Keys::Strokes(strokes) => {
					self.reach_and_click(selector).await?;
					let kb = self.page.keyboard();
					let fail = |s| classify("fill", selector, s);
					kb.press("ControlOrMeta+A", None).await.map_err(fail)?;
					for (key, gap) in strokes {
						tokio::time::sleep(gap).await;
						match key {
							Key::Char(c) => kb.type_text(c.encode_utf8(&mut [0; 4]), None).await,
							Key::Backspace => kb.press("Backspace", None).await,
						}
						.map_err(fail)?;
					}
					Ok(())
				}
			}
		}
		.await;
		self.captured(r, "fill").await
	}

	/// `key` as Playwright names it: `Enter`, `Control+A`, …
	pub async fn press(&mut self, selector: &str, key: &str) -> Result<(), Error> {
		self.pause(Act::Press).await;
		let r = self.page.locator(selector).press(key, None).await.map_err(|s| classify("press", selector, s));
		self.captured(r, "press").await
	}

	pub async fn check(&mut self, selector: &str) -> Result<(), Error> {
		self.pause(Act::Check).await;
		let r = async {
			let l = self.page.locator(selector);
			match M::NATIVE {
				true => l.check(None).await.map_err(|s| classify("check", selector, s)),
				false => match l.is_checked().await.map_err(|s| classify("check", selector, s))? {
					true => Ok(()),
					false => self.reach_and_click(selector).await,
				},
			}
		}
		.await;
		self.captured(r, "check").await
	}

	/// By option value or label. A native `<select>` popup is not DOM, so no pointer travels to it.
	pub async fn select(&mut self, selector: &str, option: &str) -> Result<(), Error> {
		self.pause(Act::Select).await;
		let r = self
			.page
			.locator(selector)
			.select_option(option, None)
			.await
			.map(drop)
			.map_err(|s| classify("select", selector, s));
		self.captured(r, "select").await
	}

	/// Wheel `dy` px with the pointer over `within`, or over the viewport's middle third.
	pub async fn scroll(&mut self, within: Option<&str>, dy: f64) -> Result<(), Error> {
		self.pause(Act::Scroll).await;
		let target = within.unwrap_or("viewport");
		let r = async {
			let area = match within {
				Some(sel) => self.bbox(sel).await?,
				None => {
					let (w, h): (f64, f64) = self.page.evaluate("[innerWidth, innerHeight]", None::<&()>).await.map_err(|s| classify("scroll", target, s))?;
					Rect {
						x: w / 3.,
						y: h / 3.,
						width: w / 3.,
						height: h / 3.,
					}
				}
			};
			self.travel(area).await.map_err(|s| classify("scroll", target, s))?;
			let mouse = self.page.mouse();
			let notches = self.motion().wheel(dy);
			for (notch, gap) in notches {
				tokio::time::sleep(gap).await;
				mouse.wheel(0., notch).await.map_err(|s| classify("scroll", target, s))?;
			}
			Ok(())
		}
		.await;
		self.captured(r, "scroll").await
	}

	/// `js` is a function expression or an expression; `arg` is its one argument.
	pub async fn eval<T: DeserializeOwned>(&mut self, js: &str, arg: impl Serialize) -> Result<T, Error> {
		let r = self.page.evaluate(js, Some(&arg)).await.map_err(|source| ErrorKind::Eval { js: js.to_owned(), source });
		self.captured(r, "eval").await
	}

	pub async fn content(&mut self) -> Result<String, Error> {
		let r = self.page.content().await.map_err(|source| ErrorKind::Driver { op: "reading page content", source });
		self.captured(r, "content").await
	}

	pub async fn screenshot(&mut self, shot: Shot<'_>) -> Result<Vec<u8>, Error> {
		let r = match shot {
			Shot::Full => self
				.page
				.screenshot(ScreenshotOptions::builder().full_page(true).build())
				.await
				.map_err(|source| ErrorKind::Driver { op: "taking a screenshot", source }),
			Shot::Element(sel) => self.page.locator(sel).screenshot(None).await.map_err(|s| classify("screenshot", sel, s)),
		};
		self.captured(r, "screenshot").await
	}

	pub async fn cookies(&self) -> Result<Vec<Cookie>, Error> {
		let context = self.page.context().map_err(|source| ErrorKind::Driver { op: "reading cookies", source })?;
		Ok(context.cookies(None).await.map_err(|source| ErrorKind::Driver { op: "reading cookies", source })?)
	}

	/// Every response from now on whose URL contains `url_part`, with its body.
	pub async fn responses(&mut self, url_part: &str) -> Result<impl Stream<Item = Result<Response, Error>> + use<M>, Error> {
		let listeners = match &self.listeners {
			Some(l) => l.clone(),
			None => {
				let l = Listeners::default();
				let handler_l = l.clone();
				self.page
					.on_response(move |r| {
						let mut subs = handler_l.lock().expect("never held across a panic");
						subs.retain(|(part, tx)| !r.url().contains(part.as_str()) || tx.unbounded_send(r.clone()).is_ok());
						async { Ok(()) }
					})
					.await
					.map_err(|source| ErrorKind::Driver {
						op: "listening for responses",
						source,
					})?;
				self.listeners.insert(l).clone()
			}
		};
		let (tx, rx) = mpsc::unbounded();
		listeners.lock().expect("never held across a panic").push((url_part.to_owned(), tx));
		Ok(rx.then(|r| async move {
			let body = r.body().await.map_err(|source| ErrorKind::Driver {
				op: "reading a response body",
				source,
			})?;
			Ok(Response {
				url: r.url().to_owned(),
				status: r.status(),
				body,
			})
		}))
	}

	pub async fn bring_to_front(&mut self) -> Result<(), Error> {
		let r = self.page.bring_to_front().await.map_err(|source| ErrorKind::Driver {
			op: "bringing the tab to front",
			source,
		});
		self.captured(r, "bring_to_front").await
	}

	pub async fn close(self) -> Result<(), Error> {
		Ok(self.page.close().await.map_err(|source| ErrorKind::Driver { op: "closing a tab", source })?)
	}

	/// The page as it is now, into `Artifacts`; `None` without them.
	pub async fn capture(&self, hint: &str) -> Option<Result<Capture, String>> {
		let artifacts = self.browser.artifacts.as_ref()?;
		let shoot = async {
			let png = self
				.page
				.screenshot(ScreenshotOptions::builder().full_page(true).build())
				.await
				.map_err(|e| format!("screenshot: {e}"))?;
			let html = self.page.content().await.map_err(|e| format!("content: {e}"))?;
			Ok::<_, String>((png, html))
		};
		Some(match tokio::time::timeout(CAPTURE_TIMEOUT, shoot).await {
			Err(_) => Err(format!("page did not answer in {CAPTURE_TIMEOUT:?}")),
			Ok(Err(e)) => Err(e),
			Ok(Ok((png, html))) => artifacts.write(hint, &self.page.url(), &png, &html),
		})
	}

	fn motion(&self) -> std::sync::MutexGuard<'_, M> {
		self.browser.motion.lock().expect("never held across a panic")
	}

	async fn pause(&self, act: Act) {
		let d = self.motion().pause(act);
		if !d.is_zero() {
			tokio::time::sleep(d).await;
		}
	}

	async fn captured<T>(&self, r: Result<T, ErrorKind>, hint: &str) -> Result<T, Error> {
		match r {
			Ok(v) => Ok(v),
			Err(kind) => Err(Error {
				kind: Box::new(kind),
				capture: self.capture(hint).await.map(Box::new),
			}),
		}
	}

	async fn bbox(&self, selector: &str) -> Result<Rect, ErrorKind> {
		let l: Locator = self.page.locator(selector).first();
		let fail = |s| classify("locate", selector, s);
		l.wait_for(WaitForOptions::builder().state(WaitForState::Visible).build()).await.map_err(fail)?;
		l.scroll_into_view_if_needed().await.map_err(fail)?;
		let b = l.bounding_box().await.map_err(fail)?.expect("visible, so it has a box");
		Ok(Rect {
			x: b.x,
			y: b.y,
			width: b.width,
			height: b.height,
		})
	}

	async fn travel(&mut self, area: Rect) -> Result<(), playwright_rs::Error> {
		let (to, path) = {
			let mut m = self.motion();
			let to = m.aim(area);
			(to, m.path(self.mouse, to))
		};
		let mouse = self.page.mouse();
		for (p, gap) in path {
			tokio::time::sleep(gap).await;
			mouse.move_to(p.x, p.y, None).await?;
			self.mouse = p;
		}
		assert_eq!(self.mouse, to, "a path ends where it aimed");
		Ok(())
	}

	async fn reach_and_click(&mut self, selector: &str) -> Result<(), ErrorKind> {
		if M::NATIVE {
			return self.page.locator(selector).click(None).await.map_err(|s| classify("click", selector, s));
		}
		let area = self.bbox(selector).await?;
		let fail = |s| classify("click", selector, s);
		self.travel(area).await.map_err(fail)?;
		let mouse = self.page.mouse();
		mouse.down(None).await.map_err(fail)?;
		self.pause(Act::Release).await;
		mouse.up(None).await.map_err(fail)
	}
}

fn classify(op: &'static str, target: &str, source: playwright_rs::Error) -> ErrorKind {
	let target = target.to_owned();
	// ponytail: playwright-rs 0.19 drops the server's `name: "TimeoutError"` into ProtocolError; match on `Error::Timeout` alone once padamson/playwright-rust#159 is released
	let timed_out = match &source {
		playwright_rs::Error::Timeout(_) => true,
		playwright_rs::Error::ProtocolError(m) => m.contains("\n TimeoutError: "),
		_ => false,
	};
	match timed_out {
		true => ErrorKind::Timeout { op, target, source },
		false => ErrorKind::Selector { op, target, source },
	}
}
