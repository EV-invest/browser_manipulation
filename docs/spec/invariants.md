# Invariants
1. Every page action goes through `Motion`.
2. Every failed action carries a capture when `Artifacts` is set (or the reason the capture failed).
3. The driver is `patchright-core` at exactly `playwright_rs::PLAYWRIGHT_VERSION`, asserted at launch. No fallback to the stock driver.
4. No `Runtime.enable` reaches the page.
5. One profile ↔ one process.
