use crate::{
    core::*,
    uia::{Command, Event, Samples, Worker},
};
use std::{
    cell::RefCell,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, Result, w},
};

const START: i32 = 101;
const SELECT: i32 = 102;
const CANCEL: i32 = 103;
const APPLY: i32 = 104;
const EXPORT: i32 = 105;
const CHOOSE: i32 = 106;
const OUTLINES: i32 = 107;
const COL_KEY: COLORREF = COLORREF(0x00ff00ff);
thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }

struct App {
    window: HWND,
    overlay: HWND,
    font: HFONT,
    controls: Vec<(HWND, i32, i32, i32, i32)>,
    status_box: HWND,
    metrics_box: HWND,
    map_box: HWND,
    scratch: HWND,
    mode_box: HWND,
    poll_box: HWND,
    interval: HWND,
    keys: [HWND; 3],
    candidates_box: HWND,
    outline_box: HWND,
    candidate_pids: Vec<i32>,
    worker: Worker,
    settings: Settings,
    scanner: Scanner,
    targets: Vec<Target>,
    groups: Vec<Vec<usize>>,
    revision: String,
    valid: bool,
    status: String,
    action: String,
    changes: Changes,
    beacon: Samples,
    snapshots: Samples,
    paint: Samples,
    awaiting_paint: Option<Instant>,
    last_step: Instant,
    last_info: Instant,
    pending_invoke: bool,
    deferred_activation: Option<(String, Target, Instant)>,
    smoke: Option<Smoke>,
}
struct Smoke {
    stage: u8,
    since: Instant,
    began: Instant,
    checks: Vec<&'static str>,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn send(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    SendMessageW(hwnd, msg, Some(wp), Some(lp))
}
fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
unsafe fn text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd).max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let n = GetWindowTextW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n as usize])
}
unsafe fn set_text(hwnd: HWND, s: &str) {
    let _ = SetWindowTextW(hwnd, PCWSTR(wide(s).as_ptr()));
}
unsafe fn choose(box_: HWND) -> i32 {
    send(box_, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32
}
unsafe fn add(box_: HWND, s: &str) {
    send(
        box_,
        CB_ADDSTRING,
        WPARAM(0),
        LPARAM(wide(s).as_ptr() as isize),
    );
}
unsafe fn selected(box_: HWND, i: usize) {
    send(box_, CB_SETCURSEL, WPARAM(i), LPARAM(0));
}

impl App {
    unsafe fn child(
        &mut self,
        class: PCWSTR,
        label: &str,
        id: i32,
        style: u32,
        rect: (i32, i32, i32, i32),
    ) -> HWND {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(wide(label).as_ptr()),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
            0,
            0,
            0,
            0,
            Some(self.window),
            Some(HMENU(id as usize as *mut _)),
            None,
            None,
        )
        .unwrap();
        send(hwnd, WM_SETFONT, WPARAM(self.font.0 as usize), LPARAM(1));
        self.controls.push((hwnd, rect.0, rect.1, rect.2, rect.3));
        hwnd
    }
    unsafe fn label(&mut self, s: &str, r: (i32, i32, i32, i32)) {
        self.child(w!("STATIC"), s, 0, 0, r);
    }
    unsafe fn setup(&mut self) {
        self.label("ALPHA-OSK / SCAN LAB", (18, 14, 610, 25));
        self.status_box = self.child(
            w!("STATIC"),
            "Waiting for keyboard...",
            0,
            0,
            (18, 44, 622, 32),
        );
        self.child(
            w!("BUTTON"),
            "Start / pause",
            START,
            WS_TABSTOP.0,
            (18, 83, 122, 31),
        );
        self.child(
            w!("BUTTON"),
            "Select",
            SELECT,
            WS_TABSTOP.0,
            (150, 83, 90, 31),
        );
        self.child(
            w!("BUTTON"),
            "Cancel / back",
            CANCEL,
            WS_TABSTOP.0,
            (250, 83, 115, 31),
        );
        self.outline_box = self.child(
            w!("BUTTON"),
            "Outline every target",
            OUTLINES,
            WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
            (390, 86, 230, 25),
        );
        send(
            self.outline_box,
            BM_SETCHECK,
            WPARAM(usize::from(self.settings.outlines)),
            LPARAM(0),
        );
        self.label("Scan mode", (18, 126, 140, 20));
        self.mode_box = self.child(
            w!("COMBOBOX"),
            "",
            0,
            WS_TABSTOP.0 | CBS_DROPDOWNLIST as u32,
            (18, 148, 150, 150),
        );
        add(self.mode_box, "Rows then keys");
        add(self.mode_box, "Linear");
        selected(
            self.mode_box,
            usize::from(self.settings.mode == ScanMode::Linear),
        );
        self.label("Scan interval (ms)", (185, 126, 145, 20));
        self.interval = self.child(
            w!("EDIT"),
            &self.settings.scan_ms.to_string(),
            0,
            WS_TABSTOP.0 | WS_BORDER.0 | ES_NUMBER as u32,
            (185, 148, 145, 26),
        );
        self.label("Watcher poll", (350, 126, 130, 20));
        self.poll_box = self.child(
            w!("COMBOBOX"),
            "",
            0,
            WS_TABSTOP.0 | CBS_DROPDOWNLIST as u32,
            (350, 148, 130, 150),
        );
        for n in [16, 33, 50, 100] {
            add(self.poll_box, &format!("{n} ms"));
        }
        selected(
            self.poll_box,
            [16, 33, 50, 100]
                .iter()
                .position(|&n| n == self.settings.poll_ms)
                .unwrap_or(1),
        );
        for (i, s) in [
            self.settings.start_key.clone(),
            self.settings.select_key.clone(),
            self.settings.cancel_key.clone(),
        ]
        .iter()
        .enumerate()
        {
            self.label(
                ["Global start / pause", "Global select", "Global cancel"][i],
                (18 + i as i32 * 207, 188, 195, 20),
            );
            self.keys[i] = self.child(
                w!("EDIT"),
                s,
                0,
                WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32,
                (18 + i as i32 * 207, 210, 195, 26),
            );
        }
        self.child(
            w!("BUTTON"),
            "Apply settings",
            APPLY,
            WS_TABSTOP.0,
            (18, 246, 125, 29),
        );
        self.child(
            w!("BUTTON"),
            "Export timings",
            EXPORT,
            WS_TABSTOP.0,
            (155, 246, 125, 29),
        );
        self.candidates_box = self.child(
            w!("COMBOBOX"),
            "",
            0,
            WS_TABSTOP.0 | CBS_DROPDOWNLIST as u32,
            (300, 247, 215, 150),
        );
        self.child(
            w!("BUTTON"),
            "Connect",
            CHOOSE,
            WS_TABSTOP.0,
            (530, 246, 102, 29),
        );
        self.metrics_box = self.child(w!("STATIC"), "", 0, 0, (18, 289, 615, 60));
        self.label("Current targets (local display only)", (18, 351, 590, 20));
        self.map_box = self.child(
            w!("EDIT"),
            "",
            0,
            WS_BORDER.0
                | WS_VSCROLL.0
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
            (18, 374, 615, 78),
        );
        self.label(
            "Scratch text: focus here or another app, then use global shortcuts.",
            (18, 463, 615, 20),
        );
        self.scratch = self.child(
            w!("EDIT"),
            "",
            0,
            WS_TABSTOP.0 | WS_BORDER.0 | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32,
            (18, 486, 615, 60),
        );
        self.layout();
    }
    unsafe fn layout(&self) {
        let scale = GetDpiForWindow(self.window) as f64 / 96.0;
        for &(h, x, y, w, hgt) in &self.controls {
            let _ = MoveWindow(
                h,
                (x as f64 * scale).round() as i32,
                (y as f64 * scale).round() as i32,
                (w as f64 * scale).round() as i32,
                (hgt as f64 * scale).round() as i32,
                true,
            );
        }
    }
    unsafe fn register(&self, s: &Settings) -> std::result::Result<(), String> {
        let mut parsed = Vec::new();
        for k in [&s.start_key, &s.select_key, &s.cancel_key] {
            let v = parse_hotkey(k)
                .ok_or_else(|| format!("Invalid shortcut: {k}. Use modifiers plus F1-F24."))?;
            if parsed.contains(&v) {
                return Err("Shortcuts must be distinct".into());
            }
            parsed.push(v);
        }
        for i in 1..=3 {
            let _ = UnregisterHotKey(Some(self.window), i);
        }
        for (i, (m, k)) in parsed.into_iter().enumerate() {
            if let Err(e) = RegisterHotKey(Some(self.window), i as i32 + 1, HOT_KEY_MODIFIERS(m), k)
            {
                for j in 1..=3 {
                    let _ = UnregisterHotKey(Some(self.window), j);
                }
                return Err(format!("Shortcut {} unavailable: {e}", i + 1));
            }
        }
        Ok(())
    }
    unsafe fn apply(&mut self) {
        let scan_ms = text(self.interval)
            .parse::<u64>()
            .ok()
            .filter(|n| (250..=3000).contains(n));
        let Some(scan_ms) = scan_ms else {
            self.action = "Scan interval must be 250-3000 ms".into();
            return;
        };
        let s = Settings {
            scan_ms,
            poll_ms: *[16, 33, 50, 100]
                .get(choose(self.poll_box) as usize)
                .unwrap_or(&33),
            mode: if choose(self.mode_box) == 1 {
                ScanMode::Linear
            } else {
                ScanMode::Rows
            },
            outlines: send(self.outline_box, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1,
            start_key: text(self.keys[0]),
            select_key: text(self.keys[1]),
            cancel_key: text(self.keys[2]),
        };
        if let Err(e) = self.register(&s) {
            let restored = self.register(&self.settings);
            self.action = format!(
                "{e}; old shortcuts {}",
                if restored.is_ok() {
                    "restored"
                } else {
                    "also unavailable"
                }
            );
            return;
        }
        if self.deferred_activation.take().is_some() {
            self.pending_invoke = false;
            let _ = self.worker.tx.send(Command::Refresh);
        }
        self.scanner.mode = s.mode;
        self.scanner.reset();
        self.last_step = Instant::now();
        let _ = self.worker.tx.send(Command::Poll(s.poll_ms));
        self.settings = s;
        match std::fs::write(
            project().join("scan-lab-settings.json"),
            serde_json::to_vec_pretty(&self.settings).unwrap(),
        ) {
            Ok(()) => self.action = "Settings applied".into(),
            Err(e) => self.action = format!("Applied; settings could not be saved: {e}"),
        }
    }
    unsafe fn command(&mut self, cmd: i32, from_button: bool) {
        match cmd {
            START => {
                if self.valid && !self.targets.is_empty() {
                    self.scanner.running = !self.scanner.running;
                    self.last_step = Instant::now();
                } else {
                    self.action = "No current targets; scanning stays paused".into();
                }
                if from_button {
                    let _ = SetFocus(Some(self.scratch));
                }
            }
            SELECT => {
                if self.valid && !self.pending_invoke {
                    if from_button {
                        let _ = SetFocus(Some(self.scratch));
                    }
                    if let Some(i) = self.scanner.select(&self.groups) {
                        self.pending_invoke = true;
                        self.valid = false;
                        if from_button {
                            let _ = self.worker.tx.send(Command::Invoke {
                                revision: self.revision.clone(),
                                target: self.targets[i].clone(),
                            });
                        } else {
                            // Capture the offer at switch-down, but do not send input with
                            // the shortcut's physical Ctrl/Alt still held in the target app.
                            self.deferred_activation = Some((
                                self.revision.clone(),
                                self.targets[i].clone(),
                                Instant::now(),
                            ));
                        }
                    }
                    self.last_step = Instant::now();
                }
            }
            CANCEL => {
                self.scanner.cancel();
                if self.deferred_activation.take().is_some() {
                    self.pending_invoke = false;
                    let _ = self.worker.tx.send(Command::Refresh);
                }
            }
            APPLY => self.apply(),
            EXPORT => self.export(),
            OUTLINES => {
                self.settings.outlines =
                    send(self.outline_box, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1
            }
            CHOOSE => {
                if let Some(&pid) = self
                    .candidate_pids
                    .get(choose(self.candidates_box) as usize)
                {
                    self.valid = false;
                    self.scanner.running = false;
                    let _ = self.worker.tx.send(Command::Choose(pid));
                }
            }
            _ => {}
        }
        self.refresh_overlay();
        self.info();
    }
    unsafe fn export(&mut self) {
        let dir = project().join("measurements");
        let output = serde_json::json!({"schema":1,"alpha_test_revision":"b75ec07125addc58896f0835fbb38879d4811d72",
            "poll_ms":self.settings.poll_ms,"targets":self.targets.len(),"changes":self.changes,
            "beacon_read":self.beacon.summary(),"cached_snapshot":self.snapshots.summary(),"detection_to_paint_submission":self.paint.summary(),
            "notes":"Rolling last 4096 timings; paint submission is not DWM presentation or provider-change latency. No labels, revisions, typing, or target identities exported."});
        let file = dir.join(format!(
            "timings-{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        ));
        match std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(&file, serde_json::to_vec_pretty(&output).unwrap()))
        {
            Ok(()) => {
                self.action = format!("Exported {}", file.file_name().unwrap().to_string_lossy())
            }
            Err(e) => self.action = format!("Export failed: {e}"),
        }
    }
    unsafe fn tick(&mut self) {
        let mut redraw = false;
        while let Ok(event) = self.worker.rx.try_recv() {
            match event {
                Event::Status(s) => {
                    self.status = s;
                    if !self.status.starts_with("Connected") {
                        self.scanner.running = false;
                        self.targets.clear();
                        self.groups.clear();
                        self.valid = false;
                        redraw = true;
                    }
                }
                Event::Candidates(c) => {
                    send(self.candidates_box, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
                    self.candidate_pids.clear();
                    for (pid, name) in c {
                        add(self.candidates_box, &format!("PID {pid}: {name}"));
                        self.candidate_pids.push(pid);
                    }
                    selected(self.candidates_box, 0);
                }
                Event::Invalidated => {
                    self.valid = false;
                    redraw = true;
                }
                Event::Snapshot {
                    revision,
                    targets,
                    detected,
                } => {
                    let change = diff(&self.targets, &targets);
                    self.changes.add(&change);
                    if change.reset_scan() {
                        self.scanner.reset();
                        self.last_step = Instant::now();
                    }
                    self.revision = revision;
                    self.groups = rows(&targets);
                    self.targets = targets;
                    self.valid = true;
                    if self.targets.is_empty() {
                        self.scanner.running = false;
                    }
                    self.awaiting_paint = Some(detected);
                    redraw = true;
                    let lines: Vec<_> = self
                        .targets
                        .iter()
                        .map(|t| {
                            format!(
                                "{} | {} | {},{} {}x{}{}{}",
                                t.id,
                                t.name,
                                t.bounds.left,
                                t.bounds.top,
                                t.bounds.right - t.bounds.left,
                                t.bounds.bottom - t.bounds.top,
                                if t.toggle == Some(1) { " | held" } else { "" },
                                if t.locked { " | locked" } else { "" }
                            )
                        })
                        .collect();
                    set_text(self.map_box, &lines.join("\r\n"));
                }
                Event::Timing {
                    beacon_ms,
                    snapshot_ms,
                } => {
                    self.beacon.record(beacon_ms);
                    if let Some(ms) = snapshot_ms {
                        self.snapshots.record(ms);
                    }
                }
                Event::Invoked {
                    result,
                    foreground_preserved,
                } => {
                    self.pending_invoke = false;
                    self.action = format!(
                        "{result}; foreground {}",
                        if foreground_preserved {
                            "preserved"
                        } else {
                            "CHANGED"
                        }
                    );
                    if !foreground_preserved {
                        self.scanner.running = false;
                    }
                    self.last_step = Instant::now();
                }
            }
        }
        if let Some((_, _, started)) = &self.deferred_activation {
            let select_vk = parse_hotkey(&self.settings.select_key)
                .map(|(_, key)| key as i32)
                .unwrap_or(0);
            let released = [
                VK_CONTROL.0 as i32,
                VK_MENU.0 as i32,
                VK_SHIFT.0 as i32,
                VK_LWIN.0 as i32,
                VK_RWIN.0 as i32,
                select_vk,
            ]
            .iter()
            .all(|key| GetAsyncKeyState(*key) >= 0);
            if started.elapsed() > Duration::from_secs(3) {
                self.deferred_activation = None;
                self.pending_invoke = false;
                let _ = self.worker.tx.send(Command::Refresh);
                self.action = "Selection cancelled: release shortcut keys within 3 seconds".into();
            } else if released {
                let (revision, target, _) = self.deferred_activation.take().unwrap();
                let _ = self.worker.tx.send(Command::Invoke { revision, target });
            }
        }
        if self.valid
            && !self.pending_invoke
            && self.scanner.running
            && self.last_step.elapsed() >= Duration::from_millis(self.settings.scan_ms)
        {
            self.scanner.step(&self.groups);
            self.last_step = Instant::now();
            redraw = true;
        }
        if redraw {
            self.refresh_overlay();
        }
        if self.last_info.elapsed() >= Duration::from_millis(250) {
            self.info();
            self.last_info = Instant::now();
        }
        if let Some(mut smoke) = self.smoke.take() {
            let result = self.smoke_tick(&mut smoke);
            if let Err(error) = result {
                let _ = std::fs::create_dir_all(project().join("measurements"));
                let _ = std::fs::write(
                    project().join("measurements/ui-smoke.json"),
                    serde_json::json!({"passed":false,"error":error}).to_string(),
                );
                PostQuitMessage(1);
            } else if smoke.stage < 7 {
                self.smoke = Some(smoke);
            }
        }
    }
    unsafe fn smoke_tick(&mut self, s: &mut Smoke) -> std::result::Result<(), String> {
        if s.began.elapsed() > Duration::from_secs(15) {
            return Err(format!(
                "UI smoke timed out at stage {}: {}",
                s.stage, self.action
            ));
        }
        match s.stage {
            0 if self.valid && !self.targets.is_empty() => {
                self.scanner.mode = ScanMode::Linear;
                self.settings.scan_ms = 250;
                self.command(START, true);
                s.stage = 1;
                s.since = Instant::now();
            }
            1 if s.since.elapsed() > Duration::from_millis(400) => {
                if !self.scanner.running || self.scanner.item == 0 {
                    return Err("Linear scanning did not advance".into());
                }
                s.checks.push("timer-driven linear scan advances");
                self.command(SELECT, false);
                s.stage = 2;
                s.since = Instant::now();
            }
            2 if !self.pending_invoke && self.action.starts_with("Activated once") => {
                if !self.action.ends_with("preserved") {
                    return Err("Activation changed foreground".into());
                }
                s.checks.push(
                    "shortcut selection waits for release, invokes once, preserves foreground",
                );
                self.scanner.running = false;
                self.scanner.mode = ScanMode::Rows;
                self.scanner.reset();
                s.stage = 3;
                s.since = Instant::now();
            }
            3 if self.valid => {
                self.command(START, true);
                self.command(SELECT, true);
                if !self.scanner.inside {
                    return Err("Row selection did not enter row".into());
                }
                self.command(CANCEL, true);
                if self.scanner.inside || !self.scanner.running {
                    return Err("Cancel did not return to rows".into());
                }
                self.command(CANCEL, true);
                if self.scanner.running {
                    return Err("Second cancel did not pause".into());
                }
                s.checks.push("row selection and cancel/back transitions");
                s.stage = 4;
                s.since = Instant::now();
                self.refresh_overlay();
            }
            4 if s.since.elapsed() > Duration::from_millis(200) => {
                if self.paint.count == 0 {
                    return Err("Overlay never painted a snapshot".into());
                }
                let style = GetWindowLongPtrW(self.overlay, GWL_EXSTYLE) as u32;
                let required =
                    (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST).0;
                if style & required != required {
                    return Err("Overlay window styles incorrect".into());
                }
                if !AreDpiAwarenessContextsEqual(
                    GetWindowDpiAwarenessContext(self.overlay),
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                )
                .as_bool()
                {
                    return Err("Overlay is not Per-Monitor V2 aware".into());
                }
                s.checks.push("overlay paints snapshots and is click-through, topmost, nonactivating, Per-Monitor V2");
                self.export();
                let report = serde_json::json!({"passed":true,"checks":s.checks,"targets":self.targets.len(),
                    "detection_to_paint_submission":self.paint.summary(),"beacon":self.beacon.summary(),"snapshot":self.snapshots.summary(),
                    "note":"Own-app smoke test using a recording Alpha fixture; actual global keystrokes and physical switch hardware are not synthesized."});
                std::fs::write(
                    project().join("measurements/ui-smoke.json"),
                    serde_json::to_vec_pretty(&report).unwrap(),
                )
                .map_err(|e| e.to_string())?;
                s.stage = 7;
                PostQuitMessage(0);
            }
            _ => {}
        }
        Ok(())
    }
    unsafe fn info(&self) {
        set_text(
            self.status_box,
            &format!(
                "{} | {} | {} targets",
                self.status,
                if !self.valid {
                    "updating"
                } else if self.scanner.running {
                    "SCANNING"
                } else {
                    "paused"
                },
                self.targets.len()
            ),
        );
        set_text(
            self.metrics_box,
            &format!(
                "p50 / p95 ms: beacon {:.2}/{:.2}    snapshot {:.2}/{:.2}    paint {:.2}/{:.2}\r\nChanges: +{} -{} moved {} relabeled {} state {} replaced {}\r\n{}",
                self.beacon.percentile(0.5),
                self.beacon.percentile(0.95),
                self.snapshots.percentile(0.5),
                self.snapshots.percentile(0.95),
                self.paint.percentile(0.5),
                self.paint.percentile(0.95),
                self.changes.added,
                self.changes.removed,
                self.changes.moved,
                self.changes.relabeled,
                self.changes.state_changed,
                self.changes.replaced,
                self.action
            ),
        );
    }
    unsafe fn refresh_overlay(&self) {
        let _ = SetWindowPos(
            self.overlay,
            Some(HWND_TOPMOST),
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let _ = InvalidateRect(Some(self.overlay), None, false);
    }
    unsafe fn draw(&mut self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut area = RECT::default();
        let _ = GetClientRect(hwnd, &mut area);
        let bg = CreateSolidBrush(COL_KEY);
        FillRect(dc, &area, bg);
        let _ = DeleteObject(bg.into());
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        if self.valid {
            if self.settings.outlines {
                for t in &self.targets {
                    border(dc, t.bounds, x, y, 1, COLORREF(0x00a58a64));
                }
            }
            let selected = self.scanner.highlighted(&self.groups);
            if let Some(rect) = selected
                .iter()
                .map(|&i| self.targets[i].bounds)
                .reduce(Bounds::union)
            {
                border(dc, rect, x, y, 4, COLORREF(0x0000d5ff));
            }
        }
        SelectObject(dc, old_brush);
        let _ = EndPaint(hwnd, &ps);
        if let Some(detected) = self.awaiting_paint.take() {
            self.paint.record(detected.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
unsafe fn border(dc: HDC, b: Bounds, x: i32, y: i32, width: i32, color: COLORREF) {
    let pen = CreatePen(PS_SOLID, width, color);
    let previous = SelectObject(dc, pen.into());
    let _ = Rectangle(dc, b.left - x, b.top - y, b.right - x, b.bottom - y);
    SelectObject(dc, previous);
    let _ = DeleteObject(pen.into());
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            APP.with(|cell| {
                if let Ok(mut state) = cell.try_borrow_mut()
                    && let Some(app) = state.as_mut()
                {
                    app.draw(hwnd);
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER | WM_COMMAND | WM_HOTKEY | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            APP.with(|cell| {
                let Ok(mut state) = cell.try_borrow_mut() else {
                    return;
                };
                let Some(app) = state.as_mut() else {
                    return;
                };
                match msg {
                    WM_TIMER => app.tick(),
                    WM_COMMAND if (wparam.0 >> 16) == BN_CLICKED as usize => {
                        app.command((wparam.0 & 0xffff) as i32, true)
                    }
                    WM_HOTKEY => app.command(
                        match wparam.0 {
                            1 => START,
                            2 => SELECT,
                            3 => CANCEL,
                            _ => 0,
                        },
                        false,
                    ),
                    WM_DISPLAYCHANGE => app.refresh_overlay(),
                    WM_DPICHANGED => {
                        let r = &*(lparam.0 as *const RECT);
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            r.left,
                            r.top,
                            r.right - r.left,
                            r.bottom - r.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                        app.layout();
                        app.refresh_overlay();
                    }
                    _ => {}
                }
            });
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn run(smoke_fixture: Option<&std::path::Path>) -> Result<()> {
    unsafe {
        let smoke_pid = if let Some(path) = smoke_fixture {
            let bytes = std::fs::read(path.join("fixture-state.json"))
                .map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;
            let state: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;
            if state["live_input"] != false {
                return Err(windows::core::Error::new(
                    E_FAIL,
                    "UI smoke requires recording fixture",
                ));
            }
            Some(
                state["pid"]
                    .as_i64()
                    .ok_or_else(|| windows::core::Error::new(E_FAIL, "Fixture PID missing"))?
                    as i32,
            )
        } else {
            None
        };
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let font = CreateFontW(
            -(16.0 * GetDpiForSystem() as f64 / 96.0).round() as i32,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Segoe UI"),
        );
        let cursor = LoadCursorW(None, IDC_ARROW)?;
        let cls = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            hCursor: cursor,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
            lpszClassName: w!("AlphaScanLab"),
            ..Default::default()
        };
        RegisterClassW(&cls);
        let overlay_class = WNDCLASSW {
            lpfnWndProc: Some(overlay_proc),
            hInstance: instance.into(),
            hCursor: cursor,
            lpszClassName: w!("AlphaScanLabOverlay"),
            ..Default::default()
        };
        RegisterClassW(&overlay_class);
        let scale = GetDpiForSystem() as f64 / 96.0;
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("AlphaScanLab"),
            w!("Alpha-OSK Scan Lab"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            40,
            40,
            (660.0 * scale) as i32,
            (592.0 * scale) as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let overlay = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("AlphaScanLabOverlay"),
            w!("Alpha-OSK Scan Overlay"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        SetLayeredWindowAttributes(overlay, COL_KEY, 255, LWA_COLORKEY)?;
        let mut settings: Settings = std::fs::read(project().join("scan-lab-settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        settings.scan_ms = settings.scan_ms.clamp(250, 3000);
        if ![16, 33, 50, 100].contains(&settings.poll_ms) {
            settings.poll_ms = 33;
        }
        let mut app = App {
            window,
            overlay,
            font,
            controls: Vec::new(),
            status_box: HWND::default(),
            metrics_box: HWND::default(),
            map_box: HWND::default(),
            scratch: HWND::default(),
            mode_box: HWND::default(),
            poll_box: HWND::default(),
            interval: HWND::default(),
            keys: [HWND::default(); 3],
            candidates_box: HWND::default(),
            outline_box: HWND::default(),
            candidate_pids: vec![],
            worker: Worker::start(settings.poll_ms),
            scanner: Scanner {
                mode: settings.mode,
                ..Default::default()
            },
            settings,
            targets: vec![],
            groups: vec![],
            revision: String::new(),
            valid: false,
            status: "Waiting for keyboard".into(),
            action: String::new(),
            changes: Changes::default(),
            beacon: Samples::default(),
            snapshots: Samples::default(),
            paint: Samples::default(),
            awaiting_paint: None,
            last_step: Instant::now(),
            last_info: Instant::now(),
            pending_invoke: false,
            deferred_activation: None,
            smoke: smoke_pid.map(|_| Smoke {
                stage: 0,
                since: Instant::now(),
                began: Instant::now(),
                checks: vec![],
            }),
        };
        if let Some(pid) = smoke_pid {
            let _ = app.worker.tx.send(Command::Choose(pid));
        }
        app.setup();
        if let Err(e) = app.register(&app.settings) {
            app.action = e;
        }
        app.info();
        app.refresh_overlay();
        APP.with(|s| *s.borrow_mut() = Some(app));
        // Consume STARTUPINFO's console-hiding hint, then show the actual UI.
        let _ = ShowWindow(window, SW_SHOWNORMAL);
        let _ = ShowWindow(window, SW_SHOW);
        SetTimer(Some(window), 1, 16, None);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if !IsDialogMessageW(window, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        APP.with(|s| {
            if let Some(app) = s.borrow_mut().take() {
                let _ = KillTimer(Some(window), 1);
                for i in 1..=3 {
                    let _ = UnregisterHotKey(Some(window), i);
                }
                let _ = DestroyWindow(app.overlay);
                let _ = DeleteObject(app.font.into());
            }
        });
        if message.wParam.0 != 0 {
            Err(windows::core::Error::new(
                E_FAIL,
                "UI smoke failed; see measurements/ui-smoke.json",
            ))
        } else {
            Ok(())
        }
    }
}
