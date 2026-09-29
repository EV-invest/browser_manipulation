use std::{
	fs::{File, OpenOptions, TryLockError},
	path::{Path, PathBuf},
	sync::Mutex,
};

use playwright_rs::{BrowserContext, BrowserContextOptions, Playwright, Viewport};

use crate::{Artifacts, Error, ErrorKind, Motion, Tab};

pub enum Launch {
	/// A browser we start, on a profile only we hold.
	Owned {
		profile: PathBuf,
		executable: PathBuf,
		headless: bool,
		/// `None`: the page takes the window's size.
		viewport: Option<(u32, u32)>,
	},
	/// Someone else's running Chrome, by its CDP endpoint. Closing only disconnects.
	Attach { cdp: String },
}

pub struct Browser<M: Motion> {
	playwright: Playwright,
	context: BrowserContext,
	attached: Option<playwright_rs::Browser>,
	pub(crate) motion: Mutex<M>,
	pub(crate) artifacts: Option<Artifacts>,
	lock: Option<ProfileLock>,
}

impl<M: Motion> Browser<M> {
	pub async fn launch(launch: Launch, motion: M, artifacts: Option<Artifacts>) -> Result<Self, Error> {
		assert_driver()?;
		let lock = match &launch {
			Launch::Owned { profile, .. } => Some(ProfileLock::acquire(profile)?),
			Launch::Attach { .. } => None,
		};
		let playwright = Playwright::launch().await.map_err(ErrorKind::Launch)?;
		let (context, attached) = match launch {
			Launch::Owned {
				profile,
				executable,
				headless,
				viewport,
			} => {
				let options = BrowserContextOptions::builder().headless(headless).executable_path(executable.display().to_string());
				let options = match viewport {
					Some((width, height)) => options.viewport(Viewport { width, height }),
					None => options.no_viewport(true),
				};
				let context = playwright
					.chromium()
					.launch_persistent_context_with_options(profile.display().to_string(), options.build())
					.await
					.map_err(ErrorKind::Launch)?;
				(context, None)
			}
			Launch::Attach { cdp } => {
				let browser = playwright.chromium().connect_over_cdp(&cdp, None).await.map_err(ErrorKind::Launch)?;
				let context = browser.contexts().into_iter().next().expect("a CDP-attached Chrome always has its default context");
				(context, Some(browser))
			}
		};
		Ok(Self {
			playwright,
			context,
			attached,
			motion: Mutex::new(motion),
			artifacts,
			lock,
		})
	}

	pub async fn tab(&self) -> Result<Tab<'_, M>, Error> {
		let page = self.context.new_page().await.map_err(|source| ErrorKind::Driver { op: "opening a tab", source })?;
		Ok(Tab::new(self, page))
	}

	/// Tabs already open, e.g. in an attached Chrome.
	pub fn tabs(&self) -> Vec<Tab<'_, M>> {
		self.context.pages().into_iter().map(|p| Tab::new(self, p)).collect()
	}

	pub async fn close(self) -> Result<(), Error> {
		let closed = match &self.attached {
			Some(browser) => browser.close().await,
			None => self.context.close().await,
		};
		closed.map_err(|source| ErrorKind::Driver { op: "closing the browser", source })?;
		drop(self.lock); // only once Chromium let go of the profile
		self.playwright.shutdown().await.map_err(|source| ErrorKind::Driver { op: "stopping the driver", source })?;
		Ok(())
	}
}

/// playwright-rs takes whichever driver the env names; only patchright's keeps `Runtime.enable` off the page.
fn assert_driver() -> Result<(), Error> {
	let cli_js = std::env::var("PLAYWRIGHT_CLI_JS").unwrap_or_default();
	let found = match (cli_js.is_empty(), std::env::var_os("PLAYWRIGHT_NODE_EXE").is_some()) {
		(true, _) => "unset".to_owned(),
		(false, false) => "without PLAYWRIGHT_NODE_EXE".to_owned(),
		(false, true) => {
			let manifest = Path::new(&cli_js).with_file_name("package.json");
			let raw = std::fs::read(&manifest).map_err(|source| ErrorKind::Io {
				what: "reading",
				path: manifest.clone(),
				source,
			})?;
			let pkg: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| ErrorKind::Io {
				what: "parsing",
				path: manifest,
				source: e.into(),
			})?;
			format!("{}@{}", pkg["name"].as_str().unwrap_or("?"), pkg["version"].as_str().unwrap_or("?"))
		}
	};
	match found == format!("patchright-core@{}", playwright_rs::PLAYWRIGHT_VERSION) {
		true => Ok(()),
		false => Err(ErrorKind::DriverMismatch { cli_js, found }.into()),
	}
}

/// Chromium guards its profile with `Singleton*` files naming the host and pid holding it; after a restart on another host it refuses
/// the profile forever. Our lock says for sure whether anyone still holds it, so what Chromium left behind can go.
struct ProfileLock(File);

impl ProfileLock {
	fn acquire(profile: &Path) -> Result<Self, Error> {
		fn io(what: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> ErrorKind {
			let path = path.to_owned();
			move |source| ErrorKind::Io { what, path, source }
		}
		std::fs::create_dir_all(profile).map_err(io("creating", profile))?;
		let path = profile.join("browser_manipulation.lock");
		let file = OpenOptions::new().create(true).write(true).truncate(false).open(&path).map_err(io("opening", &path))?;
		match file.try_lock() {
			Ok(()) => {}
			Err(TryLockError::WouldBlock) => return Err(ErrorKind::ProfileInUse(profile.to_owned()).into()),
			Err(TryLockError::Error(e)) => return Err(io("locking", &path)(e).into()),
		}
		for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
			let p = profile.join(name);
			match std::fs::remove_file(&p) {
				Ok(()) => {}
				Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
				Err(e) => return Err(io("removing", &p)(e).into()),
			}
		}
		Ok(Self(file))
	}
}

impl Drop for ProfileLock {
	fn drop(&mut self) {
		self.0.unlock().expect("we hold the flock");
	}
}
