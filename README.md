# browser_manipulation
![Minimum Supported Rust Version](https://img.shields.io/badge/nightly-1.100+-ab6000.svg)
[<img alt="crates.io" src="https://img.shields.io/crates/v/browser_manipulation.svg?color=fc8d62&logo=rust" height="20" style=flat-square>](https://crates.io/crates/browser_manipulation)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs&style=flat-square" height="20">](https://docs.rs/browser_manipulation)
![Lines Of Code](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/browser_manipulation-loc.json)
<br>
[<img alt="ci errors" src="https://img.shields.io/github/actions/workflow/status/valeratrades/browser_manipulation/errors.yml?branch=main&style=for-the-badge&style=flat-square&label=errors&labelColor=420d09" height="20">](https://github.com/valeratrades/browser_manipulation/actions?query=branch%3Amain) <!--NB: Won't find it if repo is private-->
[<img alt="ci warnings" src="https://img.shields.io/github/actions/workflow/status/valeratrades/browser_manipulation/warnings.yml?branch=main&style=for-the-badge&style=flat-square&label=warnings&labelColor=d16002" height="20">](https://github.com/valeratrades/browser_manipulation/actions?query=branch%3Amain) <!--NB: Won't find it if repo is private-->

One Playwright-protocol browser framework for every consumer: patchright driver (no `Runtime.enable`), human-shaped `Motion`, capture-on-failure.
<!-- markdownlint-disable -->
<details>
<summary>
<h2>Installation</h2>
</summary>

Needs the `patchright-core` driver at `playwright_rs::PLAYWRIGHT_VERSION`: consume this flake's `packages.patchright` and set `PLAYWRIGHT_CLI_JS=${patchright}/package/cli.js`, `PLAYWRIGHT_NODE_EXE=${nodejs}/bin/node`, `PLAYWRIGHT_SKIP_DRIVER_DOWNLOAD=1`.

</details>
<!-- markdownlint-restore -->

## Usage
```rust
let browser = Browser::launch(Launch::Owned { profile, executable, headless: true, viewport: None }, Robot, Some(artifacts)).await?;
let mut tab = browser.tab().await?;
tab.goto("https://example.com").await?;
tab.click("text=More").await?; // on failure: Error.capture holds PNG + HTML
```



<br>

<sup>
	This repository follows <a href="https://github.com/valeratrades/.github/tree/master/best_practices">my best practices</a> and <a href="https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md">Tiger Style</a> (except "proper capitalization for acronyms": (VsrState, not VSRState) and formatting). For project's architecture, see <a href="./docs/ARCHITECTURE.md">ARCHITECTURE.md</a>.
</sup>

#### License

<sup>
	Licensed under <a href="LICENSE">Blue Oak 1.0.0</a>
</sup>

<br>

<sub>
	Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be licensed as above, without any additional terms or conditions.
</sub>

