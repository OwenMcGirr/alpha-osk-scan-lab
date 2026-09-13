# Keyboard visibility validation - 13 September 2026

Tested Alpha-OSK PR #114 at `870f21a34123ad48c6f40336c02606a8ce031b6f`. This latest commit updates documentation; the minimise/restore implementation is from `69d6e56`. The recording fixture uses the upstream application subclass and retains the quiet-restore native filter. Settings are isolated. Device pixel ratio was 2.5, with no desktop text injection.

## Results

- Formatting, Clippy with warnings denied, seven unit tests, and release build passed.
- All 13 existing live UIA checks passed, including stale predictions, privacy, layouts and target activation.
- The expanded native UI smoke passed six groups of checks. It covers both visibility buttons, the toggle command in both directions, shortcut conflict rejection, cancellation of deferred activation, cleared overlay targets while minimised, and recall remaining paused.
- The visibility validator passed eight groups of checks. Its separate native text receiver kept foreground and text-field focus through three worker minimise/recall cycles, a second client recalling an already-minimised keyboard, and direct provider state changes. WinEvent hooks monitored transient foreground and focus changes; GetGUIThreadInfo checked the focused child. A separate control window deliberately took focus to validate the monitor.
- A key and prediction were captured immediately before minimisation and invoked directly while minimised. Both returned success but produced zero recorded synthesis calls. The minimised provider exposed no targets. Fresh keys and predictions activated after recall.
- The watcher detected direct provider state changes through revised state/beacon data, retained the minimised instance, and reported Not running after fixture shutdown.
- Computer control verified the visible controls, automatic connection when Alpha started after the lab, an injected Ctrl+Alt+F11 shortcut, and clicking Recall. The display changed between Shown with 73 targets and Minimised with zero targets, remaining paused.

The live run measured 50 beacon reads with median 0.069 ms and p95 0.396 ms. Twelve snapshots measured median 18.863 ms and p95 21.098 ms. UI smoke timings varied more with desktop load. These small diagnostic samples do not establish latency guarantees.

Raw reports: [live UIA](validation/870f21a-live.json), [UI smoke](validation/870f21a-ui-smoke.json), and [visibility](validation/870f21a-visibility.json). Reports identify the actual source commit. The visibility validator quits its recording fixture after checking shutdown and closes its receiver process on success or failure.

## Earlier failures and remaining limits

During harness development, one run lost foreground to the Alpha keyboard in its first minimise/recall cycle. The test did not yet log which half of the cycle failed. Later complete runs passed, but the trigger has not been isolated. This observation remains unresolved; the passing reports are not a claim that every restore is reliable. No client focus-repair workaround was added.

Other development runs stopped during receiver focus setup. Setup now waits up to three seconds to acquire focus before measurements begin and fails if it cannot. A separate restored-prediction test initially consumed an older queued snapshot; it now waits for the expected beacon revision and reports activation rejection directly. Those harness fixes do not dismiss the earlier keyboard foreground observation.

Signed installed UIAccess, mixed-DPI multi-monitor alignment, physical switch/keyboard delivery, actual taskbar clicks, and real desktop text insertion remain unverified. The computer-control shortcut check uses injected input, not hardware. Direct provider state tests call the same Qt minimise/restore operations, not a physical title-bar or taskbar click. Alpha-OSK and Switchify source were not modified.

# Historical validation at b75ec07 - 13 September 2026

Verified against Alpha-OSK PR #114 at `b75ec07125addc58896f0835fbb38879d4811d72` on Windows. The isolated fixture used the real QML and bridge, the upstream application object name, separate settings, and a recording synthesizer. Device pixel ratio was 2.5. No desktop input was injected.

## Results

- Formatting, Clippy with warnings denied, all six core unit tests, and release build passed.
- All 13 live UIA checks passed, including discovery through `alphaOsk.alphaOskKeyboard`, geometry and panel changes, modifiers, predictions, privacy and visibility.
- Retained prediction elements were invoked directly after both identical-word and different-word replacement rounds, bypassing client revision/identity guards. Both produced **zero synthesis calls**. The old identity could no longer be read and pattern retrieval failed. The Rust UIA binding reported error code `0x00000000`; this is recorded as an observation, not treated as a required error or as successful invocation.
- A current prediction element still inserted through the recording bridge. The normal client's stale-selection check also rejected an outdated selection without synthesis.
- Release UI smoke passed with 81 targets: linear advancement, selection through the shortcut command path, foreground preservation, row/back transitions, and click-through, nonactivating overlay properties.

The live run measured 50 beacon reads with median 0.069 ms and p95 0.350 ms, and 12 snapshots with median 20.995 ms and p95 22.167 ms. UI smoke recorded two detection-to-paint submissions at about 101.4 ms. These are small diagnostic samples, not latency guarantees or compositor presentation measurements.

Reports are committed in [live validation](validation/b75ec07-live.json) and [UI smoke](validation/b75ec07-ui-smoke.json). The validator records the fixture's actual source commit and exits unsuccessfully if a stale element produces synthesis or changes identity. It does not require a particular HRESULT. Recorder read errors also fail validation.

The first attempt stopped because a Windows file-sharing conflict prevented a fixture status acknowledgement. The fixture now retries atomic replacement without blocking the Qt event loop. The results above are from the subsequent successful run.

## Limits

This verifies Owen's two reported fixes in the recording fixture. Mixed-DPI multi-monitor alignment, signed installed UIAccess behavior, physical switches/global-key delivery and real desktop text insertion remain unverified. Minimise/recall controls and visibility-status integration were not added. Switchify PC and Alpha-OSK source were not changed.

# Historical validation — 12 September 2026

Tested on Windows against the real Alpha QML and bridge from PR #114, commit `0641e0ad8866eaad945518e8d41ed0439a7cf3c2`, using an isolated Python fixture. The fixture reports a 2.5 device pixel ratio. Its input synthesizer records calls instead of sending desktop input.

### Passed

- Rust formatting, Clippy with warnings denied, and all six core unit tests.
- Eleven live UIA checks: discovery and IDs, movement and geometry, optional panels, compact/full layout changes, letter activation, one-shot special-key activation, modifier and lock state, stale-selection cancellation, prediction activation, privacy removal, and hidden-target removal.
- Native UI smoke: timed linear scanning, selection through the shortcut command path, recorded activation with foreground preservation, row selection and back/cancel, overlay painting, click-through/nonactivation/topmost styles, and Per-Monitor V2 awareness.

The live run measured 50 beacon reads (median 0.218 ms; p95 0.469 ms) and 12 full snapshots (median 72.875 ms; p95 100.314 ms). The release UI smoke passed with 81 available targets and measured two detection-to-paint submissions at roughly 80.6 ms. These small samples vary with desktop load; they are diagnostics, not a latency guarantee. Detection-to-paint excludes time before revision detection and does not measure compositor presentation. Raw reports are in `measurements/`.

### Upstream findings

The QApplication launcher exposes `QApplication.alphaOskKeyboard`, while the proposed contract names `QGuiApplication.alphaOskKeyboard`. The client supports both exact IDs.

Replacing prediction words with identical words can retain the same UIA element while its generation changes. Holding an old element and invoking it after replacement succeeded and produced one synthesis call. Replacing with different words produced no input, but did not return the expected element-unavailable response in this environment. The validator preserves both observations in its report instead of treating them as passed provider guarantees.

The lab's revision and identity checks rejected an outdated prediction selection with zero synthesis calls. However, checking state and invoking are separate cross-process operations: another replacement can happen between them. A provider-side lifetime guarantee or atomic activation contract is still needed to close that race. These findings were subsequently reported on Alpha-OSK PR #114 and fixed in b75ec07; the current verification is above.

### Remaining manual coverage

Mixed-DPI multi-monitor movement and negative desktop positions, physical switch/global-key delivery, real text insertion into other applications, a signed installed UIAccess keyboard, and visual alignment during a physical drag still need manual validation. The smoke test calls the same internal command path as a shortcut; it does not synthesize physical key presses. The manifest and coordinate handling support mixed DPI, but that is not evidence of multi-monitor testing.

This is a development test harness. Existing Alpha and Switchify source files were not modified. The fixture uses a separate Alpha worktree and isolated settings. Close both lab and fixture windows after testing.
