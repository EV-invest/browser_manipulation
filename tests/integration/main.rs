use std::{
	io::{BufRead as _, BufReader, Write as _},
	net::TcpListener,
	path::{Path, PathBuf},
	sync::OnceLock,
	time::Duration,
};

use browser_manipulation::{Artifacts, Browser, ErrorKind, Launch, Motion, Noise, Robot};
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
				let mut line = String::new();
				BufReader::new(&stream).read_line(&mut line).unwrap();
				let Some(path) = line.split(' ').nth(1) else { continue }; // chromium's speculative preconnects close without a request
				let path = path.trim_start_matches('/');
				let (status, body) = match std::fs::read(dir.join(path)) {
					Ok(b) => ("200 OK", b),
					Err(_) => ("404 Not Found", Vec::new()),
				};
				let kind = if path.ends_with(".json") { "application/json" } else { "text/html" };
				write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
				stream.write_all(&body).unwrap();
			}
		});
		base
	})
}

async fn launch<M: Motion>(profile: &Path, motion: M, artifacts: Option<Artifacts>) -> Result<Browser<M>, browser_manipulation::Error> {
	let launch = Launch::Owned {
		profile: profile.to_owned(),
		executable: PathBuf::from(std::env::var("BM_TEST_CHROME").expect("set by the flake's devShell")),
		headless: true,
		viewport: Some((1280, 800)),
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
	tab.goto(&format!("{}/stealth.html", fixtures())).await.unwrap();
	tokio::time::sleep(Duration::from_millis(300)).await;
	let webdriver: bool = tab.eval("navigator.webdriver", ()).await.unwrap();
	let detected: String = tab.eval("document.body.dataset.detected", ()).await.unwrap();
	let main_world: Option<u32> = tab.eval("typeof ace === 'undefined' ? null : ace.marker", ()).await.unwrap();
	assert_eq!((webdriver, detected.as_str(), main_world), (false, "no", Some(42)));
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
