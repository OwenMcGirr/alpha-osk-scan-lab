# Alpha-OSK Scan Lab

A Windows Rust test application for scanning Alpha-OSK keys and predictions through Windows UI Automation (UIA). It provides a transparent, click-through overlay, linear and row-then-key scanning, global shortcuts, a scratch text field, and timing diagnostics. It is a separate project; Switchify is not modified.

## Run

Requires Windows and an Alpha-OSK build with the UIA target/revision contract from PR #114. The tested source is commit `870f21a34123ad48c6f40336c02606a8ce031b6f`, not the main-branch release. No additional IPC server is implemented here.

Run `target\release\alpha-osk-scan-lab.exe` alongside a compatible keyboard, or run `./launch.ps1` to launch the isolated recording fixture and lab together. The fixture requires Alpha's Python environment with PySide6 and its dependencies. Pass `-AlphaSource` and `-Python` to override local defaults. `-LiveInput` enables real keyboard input; without it, fixture activations are recorded in `work/fixture/input-records.jsonl` and do not type into applications. Do not run two fixtures using the same work directory.

The launcher defaults to a sibling `alpha-osk-uia-870f21a` checkout and `~/repos/alpha-osk/venv/Scripts/python.exe`. To prepare the tested source from an existing Alpha checkout, run:

```powershell
git -C ../alpha-osk fetch origin refs/pull/114/head
git -C ../alpha-osk worktree add --detach ../alpha-osk-uia-870f21a 870f21a34123ad48c6f40336c02606a8ce031b6f
```

The lab starts paused. Select the instance if multiple compatible keyboards are running.

| Action | Default shortcut |
|---|---|
| Start / pause | Ctrl+Alt+F8 |
| Select row / activate key | Ctrl+Alt+F9 |
| Back out of row / cancel | Ctrl+Alt+F10 |
| Minimise / recall keyboard | Ctrl+Alt+F11 |

Set the mode, scan interval (250–3000 ms), polling interval (16/33/50/100 ms), and shortcuts, then click Apply. Shortcut conflicts are reported. Selection waits for shortcut modifiers to be released before invoking a key. Clicking Start or Select focuses the lab scratch field; other buttons operate within the lab; global shortcuts retain the current application's focus. For real input, focus the scratch field or intended text application first.

Blue outlines mark available targets; amber marks the current row or key. Close the lab to remove its overlay and shortcuts. Settings and exported metrics are stored in this build's source project directory. Exported metrics contain counts and timing summaries, not labels, prediction words, revision values, or typing history. The diagnostic target list displays current labels locally. Fixture recordings contain only actions made in that fixture.

Minimise keyboard and Recall keyboard use the keyboard window's standard UIA WindowPattern. Status shows Shown, Minimised, Not running, Unavailable, or a request to choose an instance. UIA failures are not treated as proof that the keyboard exited. The global visibility shortcut works while another application is focused. Existing settings receive its default without losing custom shortcuts.

Minimising clears the overlay, cancels pending selection, and pauses scanning. Recall obtains a fresh map and stays paused until Start. A minimised keyboard remains connected. The client ignores Qt's misleading CanMinimize flag and never uses Close to hide the keyboard. It checks the actual state after each command, allows three seconds for asynchronous completion, and reports failures. It does not repair foreground focus after a visibility command.

## How it works

`src/core.rs` contains target parsing, map comparisons, grouping, and scan state. `src/uia.rs` owns UIA objects on a dedicated MTA COM worker thread. It discovers exact Alpha window IDs, reads the revision beacon at the configured interval, and retrieves cached target properties when the revision changes. It retries connection when the keyboard starts or restarts. Multiple clients can independently read this UIA contract.

A change invalidates the overlay until a consistent snapshot arrives. Geometry-only changes retain the selection; changes to identity, labels, availability, modifier state, or prediction generation reset scanning. Targets use screen-space physical rectangles, including negative desktop coordinates. The overlay spans the virtual desktop and uses Per-Monitor V2 DPI awareness.

Activation checks window visibility, revision, target identity, state, and geometry against the captured selection, then rechecks the revision immediately before UIA Invoke. No mouse click simulation, hold, or repeat is used. Cross-process calls stay off the UI thread. The overlay cannot accept focus or mouse input.

The client discovers the stable `alphaOsk.alphaOskKeyboard` window ID. Older application-class IDs are no longer accepted; use the fixed PR build linked above. The recording fixture uses the upstream application object name while keeping its own settings identity. It accepts only documented `aosk.v1` key and prediction IDs, excluding dialogs and pickers.

## Build and validate

Install Rust with the MSVC Windows build tools, then:

```powershell
cargo fmt --check
cargo clippy --offline --all-targets -- -D warnings
cargo test --offline
cargo build --offline --release
# With the recording fixture running:
cargo run --offline -- --validate-fixture work/fixture
cargo run --offline -- --ui-smoke work/fixture
cargo run --offline -- --probe 10
# Run last: this test quits its recording fixture after checking shutdown.
cargo run --offline -- --validate-visibility work/fixture
```

Omit `--offline` for the first dependency download on another machine. All fixture validators refuse live-input fixtures. Live validation directly invokes retained prediction elements after both identical-word and different-word replacements, without the client revalidation path. A stale Invoke must produce zero recorded synthesis calls and its identity must not change; the precise UIA error is diagnostic only. A current prediction is invoked as a positive control. Provider failures are recorded and cause a nonzero exit. Results go to `measurements/`. The test-only command file changes fixture layout, geometry, predictions, and privacy; it is not a public Alpha IPC protocol.

The visibility validator opens a separate native text receiver and a positive-control window. It monitors foreground and focus WinEvents, checks the focused child with GetGUIThreadInfo, and tests repeated worker and provider visibility changes. It tests held key and prediction elements without client guards, then fresh restored targets, connection while minimised, and shutdown. Receiver focus is established only during setup; no focus repair occurs during measurement. Avoid interacting with other applications while it runs. Its result is in `measurements/visibility-validation.json`. The fixture uses upstream's application subclass and retains its quiet-restore native filter, matching the application startup code.

See `VALIDATION.md` for measured results and remaining limitations. This is an unsigned development prototype, not a production accessibility service.
