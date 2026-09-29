```rust
let browser = Browser::launch(Launch::Owned { profile, executable, headless: true, viewport: None }, Robot, Some(artifacts)).await?;
let mut tab = browser.tab().await?;
tab.goto("https://example.com").await?;
tab.click("text=More").await?; // on failure: Error.capture holds PNG + HTML
```
