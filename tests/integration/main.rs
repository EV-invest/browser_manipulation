use std::{
	io::{BufRead as _, BufReader, Write as _},
	net::TcpListener,
	path::{Path, PathBuf},
	sync::{Arc, Mutex, OnceLock},
	time::Duration,
};

use browser_manipulation::{Act, Artifacts, Browser, ErrorKind, Keys, Launch, Motion, Noise, Point, Rect, Robot, Shot, Viewport};
use futures::StreamExt as _;

/// Serves `tests/fixtures`; `file://` pages can't `fetch`.
fn fixtures() -> &'static str {
	static BASE: OnceLock<String> = OnceLock::new();
	BASE.get_or_init(|| {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures");
		std::thread::spawn(move || {
			for stream in listener.incoming() {
				let mut stream = stream.unwrap();
				let dir = dir.clone();
				std::thread::spawn(move || {
					// an idle preconnect must not hold up the requests behind it
					let mut head = String::new();
					let mut reader = BufReader::new(&stream);
					while reader.read_line(&mut head).unwrap() > 2 {} // up to the blank line ending the head
					let Some(path) = head.split(' ').nth(1) else { return }; // chromium's speculative preconnects close without a request
					let path = path.trim_start_matches('/');
					let mut extra = "";
					let (status, body) = match (path, std::fs::read(dir.join(path))) {
						("headers", _) => ("200 OK", head.clone().into_bytes()),
						("session", _) => {
							extra = "Set-Cookie: session=1; HttpOnly\r\n";
							("200 OK", Vec::new())
						}
						("echo", _) => {
							let len = head
								.lines()
								.find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|n| n.parse::<usize>().unwrap()))
								.expect("the fixtures POST with a body");
							let mut body = vec![0; len];
							std::io::Read::read_exact(&mut reader, &mut body).unwrap();
							("200 OK", body)
						}
						(_, Ok(b)) => ("200 OK", b),
						(_, Err(_)) => ("404 Not Found", Vec::new()),
					};
					let kind = if path.ends_with(".json") { "application/json" } else { "text/html" };
					write!(
						stream,
						"HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
						body.len()
					)
					.unwrap();
					stream.write_all(&body).unwrap();
				});
			}
		});
		base
	})
}

fn chrome() -> PathBuf {
	PathBuf::from(std::env::var("BM_TEST_CHROME").expect("set by the flake's devShell"))
}

async fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
	let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
	loop {
		if let Some(v) = f() {
			return v;
		}
		assert!(tokio::time::Instant::now() < deadline, "no {what} in 10s");
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
}

async fn launch<M: Motion>(profile: &Path, motion: M, artifacts: Option<Artifacts>) -> Result<Browser<M>, browser_manipulation::Error> {
	launch_at(profile, 1., motion, artifacts).await
}

async fn launch_at<M: Motion>(profile: &Path, device_scale_factor: f64, motion: M, artifacts: Option<Artifacts>) -> Result<Browser<M>, browser_manipulation::Error> {
	let launch = Launch::Owned {
		profile: profile.to_owned(),
		executable: chrome(),
		headless: true,
		viewport: Some(Viewport {
			width: 1280,
			height: 800,
			device_scale_factor,
		}),
	};
	Browser::launch(launch, motion, artifacts).await
}

fn noise() -> Noise {
	Noise::builder()
		.dwell(Duration::from_millis(30))
		.dwell_spread(0.3)
		.speed(1500.)
		.overshoot(0.5)
		.jitter(1.)
		.key_gap(Duration::from_millis(40))
		.key_spread(0.4)
		.typo(0.1)
		.notch(100.0..=120.)
		.notch_gap(Duration::from_millis(8)..=Duration::from_millis(40))
		.back(0.1)
		.back_notches(1..=2)
		.seed(7)
		.build()
}

#[tokio::test]
async fn one_profile_one_process() {
	let dir = tempfile::tempdir().unwrap();
	std::os::unix::fs::symlink("old-host-4242", dir.path().join("SingletonLock")).unwrap();
	let first = launch(dir.path(), Robot, None).await.unwrap();
	let second = launch(dir.path(), Robot, None).await.map(drop).unwrap_err();
	assert!(matches!(*second.kind, ErrorKind::ProfileInUse(_)), "{second:?}");
	first.close().await.unwrap();
	launch(dir.path(), Robot, None).await.unwrap().close().await.unwrap();
}

#[tokio::test]
async fn page_sees_no_automation() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(dir.path(), Robot, None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	for routed in [false, true] {
		if routed {
			tab.route("/", |_| None).await.unwrap();
		}
		tab.goto(&format!("{}/stealth.html", fixtures())).await.unwrap();
		tokio::time::sleep(Duration::from_millis(300)).await;
		let webdriver: bool = tab.eval("navigator.webdriver", ()).await.unwrap();
		let detected: String = tab.eval("document.body.dataset.detected", ()).await.unwrap();
		let main_world: Option<u32> = tab.eval("typeof ace === 'undefined' ? null : ace.marker", ()).await.unwrap();
		assert_eq!((webdriver, detected.as_str(), main_world), (false, "no", Some(42)), "routed: {routed}");
	}
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn route_rewrites_the_page_s_own_request() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(dir.path(), Robot, None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.goto(&format!("{}/post.html", fixtures())).await.unwrap();
	let seen = Arc::<Mutex<Vec<(String, String, Option<Vec<u8>>)>>>::default();
	let seen_in = seen.clone();
	tab.route("/echo", move |r| {
		seen_in.lock().unwrap().push((r.url.clone(), r.method.clone(), r.body.clone()));
		Some(b"rewritten".to_vec())
	})
	.await
	.unwrap();
	let echo = async |tab: &mut browser_manipulation::Tab<'_, Robot>| {
		tab.eval::<()>("void delete document.body.dataset.echo", ()).await.unwrap();
		tab.click("#post").await.unwrap();
		tab.wait_for_any(&["body[data-echo]"]).await.unwrap();
		tab.eval::<String>("document.body.dataset.echo", ()).await.unwrap()
	};

	assert_eq!(echo(&mut tab).await, "rewritten");
	tab.unroute("/echo").await.unwrap();
	assert_eq!(echo(&mut tab).await, "page");
	assert_eq!(*seen.lock().unwrap(), [(format!("{}/echo", fixtures()), "POST".to_owned(), Some(b"page".to_vec()))]);
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn screenshots_are_scaled_by_the_device() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch_at(dir.path(), 2., Robot, None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.goto(&format!("{}/human.html", fixtures())).await.unwrap();
	let png = tab.screenshot(Shot::Full).await.unwrap();
	let width = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap().info().width;
	assert_eq!(width, 2 * 1280);
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn click_waits_for_late_element() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(dir.path(), Robot, None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.goto(&format!("{}/late.html", fixtures())).await.unwrap();
	tab.click("#late").await.unwrap();
	let clicked: Option<String> = tab.eval("document.body.dataset.clicked", ()).await.unwrap();
	assert_eq!(clicked.as_deref(), Some("yes"));
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn failed_action_carries_capture() {
	let dir = tempfile::tempdir().unwrap();
	let artifacts = dir.path().join("artifacts");
	let browser = launch(
		&dir.path().join("profile"),
		Robot,
		Some(Artifacts {
			dir: artifacts,
			retention: Duration::from_secs(3600),
		}),
	)
	.await
	.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.set_timeout(Duration::from_millis(500)).await;
	let url = format!("{}/late.html", fixtures());
	tab.goto(&url).await.unwrap();
	let err = tab.click("#never").await.unwrap_err();
	assert!(matches!(*err.kind, ErrorKind::Timeout { .. }), "{err:?}");
	let capture = err.capture.expect("artifacts set").expect("page is alive");
	assert_eq!(capture.url, url);

	let reader = png::Decoder::new(std::io::Cursor::new(std::fs::read(&capture.png).unwrap())).read_info().unwrap();
	let texts: Vec<(&str, &str)> = reader.info().uncompressed_latin1_text.iter().map(|t| (t.keyword.as_str(), t.text.as_str())).collect();
	assert!(texts.iter().any(|&(k, v)| k == "Source" && v == url), "{texts:?}");
	assert!(texts.iter().any(|&(k, _)| k == "Creation Time"), "{texts:?}");
	assert!(std::fs::read_to_string(&capture.html).unwrap().contains("fixture-late"));
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn noise_moves_and_types_like_a_hand() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(dir.path(), noise(), None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.goto(&format!("{}/human.html", fixtures())).await.unwrap();
	tab.fill("#t", "hello world").await.unwrap();
	tab.click("#b").await.unwrap();
	let (value, trusted, moves, keydowns): (String, String, String, String) = tab
		.eval(
			"[document.getElementById('t').value, document.body.dataset.trusted, document.body.dataset.moves, document.body.dataset.keydowns]",
			(),
		)
		.await
		.unwrap();
	assert_eq!((value.as_str(), trusted.as_str()), ("hello world", "true"));
	assert!(moves.parse::<u32>().unwrap() > 1, "{moves}");
	let keydowns: Vec<f64> = serde_json::from_str(&keydowns).unwrap();
	let gaps: Vec<f64> = keydowns.windows(2).map(|w| w[1] - w[0]).collect();
	assert!(gaps.len() >= 10 && gaps.iter().any(|g| (g - gaps[0]).abs() > 5.), "{gaps:?}");
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn robot_fills_at_once() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(dir.path(), Robot, None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.goto(&format!("{}/human.html", fixtures())).await.unwrap();
	tab.fill("#t", "abc").await.unwrap();
	let keydowns: Option<String> = tab.eval("document.body.dataset.keydowns", ()).await.unwrap();
	let value: String = tab.eval("document.getElementById('t').value", ()).await.unwrap();
	assert_eq!((value.as_str(), keydowns), ("abc", None));
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn responses_yield_bodies() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(dir.path(), Robot, None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.goto(&format!("{}/fetch.html", fixtures())).await.unwrap();
	let mut responses = std::pin::pin!(tab.responses("/data.json").await.unwrap());
	tab.click("#go").await.unwrap();
	let r = tokio::time::timeout(Duration::from_secs(10), responses.next()).await.unwrap().unwrap().unwrap();
	assert_eq!((r.status, String::from_utf8(r.body).unwrap().trim()), (200, r#"{"feed":[1,2,3]}"#));
	drop(tab);
	browser.close().await.unwrap();
}

/// `Noise`, remembering every notch it hands out.
struct Recorded(Noise, Arc<Mutex<Vec<f64>>>);

impl Motion for Recorded {
	const NATIVE: bool = false;

	fn pause(&mut self, act: Act) -> Duration {
		self.0.pause(act)
	}

	fn aim(&mut self, bbox: Rect) -> Point {
		self.0.aim(bbox)
	}

	fn path(&mut self, from: Point, to: Point) -> Vec<(Point, Duration)> {
		self.0.path(from, to)
	}

	fn keys(&mut self, text: &str) -> Keys {
		self.0.keys(text)
	}

	fn wheel(&mut self, dy: f64) -> Vec<(f64, Duration)> {
		let notches = self.0.wheel(dy);
		self.1.lock().unwrap().extend(notches.iter().map(|&(d, _)| d));
		notches
	}
}

#[tokio::test]
async fn scroll_turns_the_wheel_notch_by_notch() {
	let dir = tempfile::tempdir().unwrap();
	let notches = Arc::<Mutex<Vec<f64>>>::default();
	let browser = launch(dir.path(), Recorded(noise(), notches.clone()), None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.goto(&format!("{}/scroll.html", fixtures())).await.unwrap();
	tab.scroll(None, 600.).await.unwrap();
	tab.scroll(None, 600.).await.unwrap();
	let notches = notches.lock().unwrap().clone();
	let mut last = f64::NAN;
	let (wheels, y) = loop {
		tokio::time::sleep(Duration::from_millis(200)).await;
		let (wheels, y): (Option<String>, f64) = tab.eval("[document.body.dataset.wheels ?? null, scrollY]", ()).await.unwrap();
		match y == last {
			true => break (wheels, y), // smooth scrolling has settled
			false => last = y,
		}
	};
	let wheels: Vec<f64> = serde_json::from_str(&wheels.expect("wheel events arrived")).unwrap();
	assert_eq!(wheels.len(), notches.len(), "{wheels:?} vs {notches:?}");
	assert!(wheels.iter().zip(&notches).all(|(w, n)| (w - n).abs() < 1.), "{wheels:?} vs {notches:?}");
	let sum: f64 = notches.iter().sum();
	assert!((y - sum).abs() < notches.len() as f64, "scrollY {y}, notches sum {sum}");
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn headless_passes_for_chrome() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(dir.path(), Robot, None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	let sent_ua = |headers: &str| headers.lines().find_map(|l| l.strip_prefix("User-Agent: ")).expect("chromium sends a UA").to_owned();

	tab.goto(&format!("{}/headers", fixtures())).await.unwrap();
	let page_ua: String = tab.eval("navigator.userAgent", ()).await.unwrap();
	let request_ua = sent_ua(&tab.content().await.unwrap());

	tab.goto(&format!("{}/popup.html", fixtures())).await.unwrap();
	tab.click("#open").await.unwrap();
	let mut popup = until("popup", || browser.tabs().into_iter().find(|t| t.url().ends_with("/headers"))).await;
	let popup_ua = sent_ua(&popup.content().await.unwrap());

	for ua in [&page_ua, &request_ua, &popup_ua] {
		assert!(ua.contains(" Chrome/") && !ua.contains("Headless"), "{ua}");
	}
	drop((tab, popup));
	browser.close().await.unwrap();
}

/// Someone else's Chrome on `profile`, opened at `url`; its CDP endpoint.
async fn debuggable_chrome(profile: &Path, url: &str) -> (std::process::Child, String) {
	let chrome = std::process::Command::new(chrome())
		.args(["--headless=new", "--remote-debugging-port=0", "--no-first-run"])
		.arg(format!("--user-data-dir={}", profile.display()))
		.arg(url)
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.spawn()
		.unwrap();
	let port_file = profile.join("DevToolsActivePort");
	let port = until("DevToolsActivePort", || std::fs::read_to_string(&port_file).ok()?.lines().next().map(str::to_owned)).await;
	(chrome, format!("http://127.0.0.1:{port}"))
}

/// In an attached Chrome, a page of a context we did not make (another profile's) is filed under the default one.
#[tokio::test]
async fn cookies_are_the_tab_s_own() {
	let dir = tempfile::tempdir().unwrap();
	let (mut chrome, cdp) = debuggable_chrome(dir.path(), "about:blank").await;
	let other = playwright_rs::Playwright::launch().await.unwrap();
	let other_browser = other.chromium().connect_over_cdp(&cdp, None).await.unwrap();
	let foreign = other_browser.new_context().await.unwrap().new_page().await.unwrap();
	foreign.goto(&format!("{}/late.html", fixtures()), None).await.unwrap();
	foreign.evaluate::<(), ()>("void (document.cookie = 'who=foreign')", None).await.unwrap();

	let browser = Browser::launch(Launch::Attach { cdp }, Robot, None).await.unwrap();
	let mut ours = browser.tab().await.unwrap();
	ours.goto(&format!("{}/human.html", fixtures())).await.unwrap();
	ours.eval::<()>("void (document.cookie = 'who=default')", ()).await.unwrap();
	let theirs = until("the foreign page", || browser.tabs().into_iter().find(|t| t.url().ends_with("/late.html"))).await;
	let who = async |tab: &browser_manipulation::Tab<'_, Robot>| {
		tab.cookies(&format!("{}/", fixtures()))
			.await
			.unwrap()
			.into_iter()
			.map(|c| format!("{}={}", c.name, c.value))
			.collect::<Vec<_>>()
	};
	assert_eq!((who(&ours).await, who(&theirs).await), (vec!["who=default".to_owned()], vec!["who=foreign".to_owned()]));

	drop(theirs);
	ours.close().await.unwrap();
	browser.close().await.unwrap();
	other_browser.close().await.unwrap();
	other.shutdown().await.unwrap();
	chrome.kill().unwrap();
	chrome.wait().unwrap();
}

#[tokio::test]
async fn attach_leaves_chrome_as_found() {
	let dir = tempfile::tempdir().unwrap();
	let url = format!("{}/late.html", fixtures());
	let (mut chrome, cdp) = debuggable_chrome(dir.path(), &url).await;
	let attach = || Browser::launch(Launch::Attach { cdp: cdp.clone() }, Robot, None);

	let browser = attach().await.unwrap();
	until("the open page", || browser.tabs().iter().any(|t| t.url() == url).then_some(())).await;
	let mut ours = browser.tab().await.unwrap();
	ours.goto(&format!("{}/human.html", fixtures())).await.unwrap();
	ours.close().await.unwrap();
	browser.close().await.unwrap();
	assert!(chrome.try_wait().unwrap().is_none(), "disconnecting killed the attached chrome");

	let again = attach().await.unwrap();
	let urls: Vec<String> = again.tabs().iter().map(|t| t.url()).collect();
	assert_eq!(urls, [url]);
	again.close().await.unwrap();
	chrome.kill().unwrap();
	chrome.wait().unwrap();
}

#[tokio::test]
async fn cookies_are_the_ones_sent_to_the_url() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(dir.path(), Robot, None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	let page = format!("{}/cookies.html", fixtures());
	tab.goto(&page).await.unwrap();
	let names = async |tab: &browser_manipulation::Tab<'_, Robot>, url: &str| tab.cookies(url).await.unwrap().into_iter().map(|c| c.name).collect::<Vec<_>>();
	assert!(names(&tab, &format!("{}/api/x", fixtures())).await.contains(&"scoped".to_owned()));
	assert!(!names(&tab, &page).await.contains(&"scoped".to_owned()));
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn wait_for_cookie_sees_server_and_script_cookies() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(dir.path(), Robot, None).await.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.goto(&format!("{}/cookies.html", fixtures())).await.unwrap();
	let origin = format!("{}/", fixtures());
	let late = tab.wait_for_cookie(&origin, "late").await.unwrap();
	let session = tab.wait_for_cookie(&origin, "session").await.unwrap();
	assert_eq!((late.value.as_str(), session.value.as_str(), session.http_only), ("1", "1", true));
	drop(tab);
	browser.close().await.unwrap();
}

#[tokio::test]
async fn wait_for_cookie_times_out_with_capture() {
	let dir = tempfile::tempdir().unwrap();
	let browser = launch(
		&dir.path().join("profile"),
		Robot,
		Some(Artifacts {
			dir: dir.path().join("artifacts"),
			retention: Duration::from_secs(3600),
		}),
	)
	.await
	.unwrap();
	let mut tab = browser.tab().await.unwrap();
	tab.set_timeout(Duration::from_millis(800)).await;
	tab.goto(&format!("{}/cookies.html", fixtures())).await.unwrap();
	let err = tab.wait_for_cookie(&format!("{}/", fixtures()), "never").await.unwrap_err();
	assert!(matches!(*err.kind, ErrorKind::Timeout { .. }), "{err:?}");
	err.capture.expect("artifacts set").expect("page is alive");
	drop(tab);
	browser.close().await.unwrap();
}
