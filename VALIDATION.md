# Validation — 12 September 2026

Tested on Windows against the real Alpha QML and bridge from PR #114, commit `0641e0ad8866eaad945518e8d41ed0439a7cf3c2`, using an isolated Python fixture. The fixture reports a 2.5 device pixel ratio. Its input synthesizer records calls instead of sending desktop input.

## Passed

- Rust formatting, Clippy with warnings denied, and all six core unit tests.
- Eleven live UIA checks: discovery and IDs, movement and geometry, optional panels, compact/full layout changes, letter activation, one-shot special-key activation, modifier and lock state, stale-selection cancellation, prediction activation, privacy removal, and hidden-target removal.
- Native UI smoke: timed linear scanning, selection through the shortcut command path, recorded activation with foreground preservation, row selection and back/cancel, overlay painting, click-through/nonactivation/topmost styles, and Per-Monitor V2 awareness.

The live run measured 50 beacon reads (median 0.218 ms; p95 0.469 ms) and 12 full snapshots (median 72.875 ms; p95 100.314 ms). The release UI smoke passed with 81 available targets and measured two detection-to-paint submissions at roughly 80.6 ms. These small samples vary with desktop load; they are diagnostics, not a latency guarantee. Detection-to-paint excludes time before revision detection and does not measure compositor presentation. Raw reports are in `measurements/`.

## Upstream findings

The QApplication launcher exposes `QApplication.alphaOskKeyboard`, while the proposed contract names `QGuiApplication.alphaOskKeyboard`. The client supports both exact IDs.

Replacing prediction words with identical words can retain the same UIA element while its generation changes. Holding an old element and invoking it after replacement succeeded and produced one synthesis call. Replacing with different words produced no input, but did not return the expected element-unavailable response in this environment. The validator preserves both observations in its report instead of treating them as passed provider guarantees.

The lab's revision and identity checks rejected an outdated prediction selection with zero synthesis calls. However, checking state and invoking are separate cross-process operations: another replacement can happen between them. A provider-side lifetime guarantee or atomic activation contract is still needed to close that race. No upstream issue or comment was posted by this implementation.

## Remaining manual coverage

Mixed-DPI multi-monitor movement and negative desktop positions, physical switch/global-key delivery, real text insertion into other applications, a signed installed UIAccess keyboard, and visual alignment during a physical drag still need manual validation. The smoke test calls the same internal command path as a shortcut; it does not synthesize physical key presses. The manifest and coordinate handling support mixed DPI, but that is not evidence of multi-monitor testing.

This is a development test harness. Existing Alpha and Switchify source files were not modified. The fixture uses a separate Alpha worktree and isolated settings. Close both lab and fixture windows after testing.
