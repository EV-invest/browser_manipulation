use std::{
	path::{Path, PathBuf},
	time::{Duration, SystemTime},
};

use jiff::Timestamp;

/// Where failure captures go. The directory is ours: anything in it older than `retention` is pruned on each write.
#[derive(Clone, Debug)]
pub struct Artifacts {
	pub dir: PathBuf,
	pub retention: Duration,
}
impl Artifacts {
	pub(crate) fn write(&self, hint: &str, url: &str, png: &[u8], html: &str) -> Result<Capture, String> {
		std::fs::create_dir_all(&self.dir).map_err(|e| format!("creating {}: {e}", self.dir.display()))?;
		self.prune()?;
		let at = Timestamp::now();
		let hint: String = hint.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).take(60).collect();
		let stem = format!("{}-{hint}", at.strftime("%Y%m%dT%H%M%S%.3fZ"));
		let png_path = self.dir.join(format!("{stem}.png"));
		let html_path = self.dir.join(format!("{stem}.html"));
		let png = with_text(
			png,
			&[
				("Creation Time", &at.to_string()),
				("Source", url),
				("Software", concat!("browser_manipulation ", env!("CARGO_PKG_VERSION"))),
			],
		)?;
		create_new(&png_path, &png)?;
		create_new(&html_path, html.as_bytes())?;
		Ok(Capture {
			png: png_path,
			html: html_path,
			url: url.to_owned(),
			at,
		})
	}

	fn prune(&self) -> Result<(), String> {
		let cutoff = SystemTime::now() - self.retention;
		let entries = std::fs::read_dir(&self.dir).map_err(|e| format!("listing {}: {e}", self.dir.display()))?;
		for entry in entries {
			let entry = entry.map_err(|e| format!("listing {}: {e}", self.dir.display()))?;
			let path = entry.path();
			let modified = entry.metadata().and_then(|m| m.modified()).map_err(|e| format!("stat {}: {e}", path.display()))?;
			if modified < cutoff {
				std::fs::remove_file(&path).map_err(|e| format!("pruning {}: {e}", path.display()))?;
			}
		}
		Ok(())
	}
}

/// A page as it was when an action failed.
#[derive(Clone, Debug)]
pub struct Capture {
	pub png: PathBuf,
	pub html: PathBuf,
	pub url: String,
	pub at: Timestamp,
}

fn create_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
	use std::io::Write as _;
	std::fs::File::create_new(path)
		.and_then(|mut f| f.write_all(bytes))
		.map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Inserts one `tEXt` chunk per entry right after IHDR. Values are written as Latin-1, anything outside it as `?`
/// (`iTXt` would carry UTF-8, but `tEXt` is what every viewer shows).
fn with_text(png: &[u8], entries: &[(&str, &str)]) -> Result<Vec<u8>, String> {
	const IHDR_END: usize = 8 + 4 + 4 + 13 + 4;
	if !(png.len() >= IHDR_END && png.starts_with(b"\x89PNG\r\n\x1a\n") && &png[12..16] == b"IHDR") {
		return Err("screenshot is not a PNG".into());
	}
	let mut out = Vec::with_capacity(png.len() + entries.len() * 64);
	out.extend_from_slice(&png[..IHDR_END]);
	for (key, value) in entries {
		assert!((1..=79).contains(&key.len()) && key.bytes().all(|b| (32..=126).contains(&b)), "bad tEXt keyword {key:?}");
		let mut data = Vec::with_capacity(key.len() + 1 + value.len());
		data.extend_from_slice(key.as_bytes());
		data.push(0);
		data.extend(value.chars().map(|c| u8::try_from(u32::from(c)).ok().filter(|&b| b != 0).unwrap_or(b'?')));
		let len = u32::try_from(data.len()).map_err(|_| "tEXt value too long".to_owned())?;
		out.extend_from_slice(&len.to_be_bytes());
		let mut crc = crc32fast::Hasher::new();
		crc.update(b"tEXt");
		crc.update(&data);
		out.extend_from_slice(b"tEXt");
		out.extend_from_slice(&data);
		out.extend_from_slice(&crc.finalize().to_be_bytes());
	}
	out.extend_from_slice(&png[IHDR_END..]);
	Ok(out)
}
