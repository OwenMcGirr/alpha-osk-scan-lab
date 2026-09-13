//! Recording-only integration tests with a separate native text receiver.
use crate::{
    live_tests::Fixture,
    uia::{self, Client, Command, Event, Visibility, VisibilityAction, Worker},
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    error::Error,
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command as ProcessCommand},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, LibraryLoader::GetModuleHandleW},
        UI::{Accessibility::*, Input::KeyboardAndMouse::SetFocus, WindowsAndMessaging::*},
    },
    core::w,
};
type TestResult<T> = Result<T, Box<dyn Error>>;
fn require(ok: bool, reason: &str) -> TestResult<()> {
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::other(reason).into())
    }
}

thread_local! {
    static FOCUS_EVENTS: RefCell<Vec<(u32, usize)>> = const { RefCell::new(Vec::new()) };
}
unsafe extern "system" fn focus_event(
    _: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    FOCUS_EVENTS.with(|events| events.borrow_mut().push((event, window.0 as usize)));
}
unsafe fn pump() {
    let mut msg = MSG::default();
    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}
unsafe fn settle(ms: u64) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(ms) {
        pump();
        thread::sleep(Duration::from_millis(5));
    }
}
struct Monitor(Vec<HWINEVENTHOOK>);
impl Monitor {
    unsafe fn new() -> TestResult<Self> {
        let mut hooks = Self(Vec::new());
        for event in [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_FOCUS] {
            let hook = SetWinEventHook(
                event,
                event,
                None,
                Some(focus_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            require(!hook.0.is_null(), "Could not install focus monitor")?;
            hooks.0.push(hook);
        }
        Ok(hooks)
    }
    unsafe fn reset(&self) {
        pump();
        FOCUS_EVENTS.with(|events| events.borrow_mut().clear());
    }
    unsafe fn check(&self, window: HWND, edit: HWND) -> TestResult<()> {
        pump();
        require(
            GetForegroundWindow() == window,
            &format!(
                "Receiver lost foreground: expected {:?}, actual {:?}",
                window,
                GetForegroundWindow()
            ),
        )?;
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(GetWindowThreadProcessId(window, None), &mut info)?;
        require(info.hwndFocus == edit, "Receiver text field lost focus")?;
        let changes = FOCUS_EVENTS.with(|events| {
            events
                .borrow()
                .iter()
                .filter(|(event, hwnd)| {
                    *hwnd
                        != if *event == EVENT_SYSTEM_FOREGROUND {
                            window.0 as usize
                        } else {
                            edit.0 as usize
                        }
                })
                .count()
        });
        require(
            changes == 0,
            "Focus monitor caught a transient focus change",
        )
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe {
            for hook in &self.0 {
                let _ = UnhookWinEvent(*hook);
            }
        }
    }
}
struct Receiver(Child);
impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

unsafe fn wait_event(worker: &Worker, predicate: impl Fn(&Event) -> bool) -> TestResult<Event> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(8) {
        pump();
        while let Ok(event) = worker.rx.try_recv() {
            if predicate(&event) {
                return Ok(event);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    Err("Timed out waiting for worker event".into())
}
unsafe fn snapshot(worker: &Worker, empty: bool) -> TestResult<Event> {
    wait_event(
        worker,
        |event| matches!(event, Event::Snapshot { targets, .. } if targets.is_empty() == empty),
    )
}
unsafe fn change(worker: &Worker, action: VisibilityAction, state: Visibility) -> TestResult<()> {
    while worker.rx.try_recv().is_ok() {}
    worker.tx.send(Command::Visibility(action))?;
    wait_event(worker, |event| matches!(event, Event::VisibilityResult(_))).and_then(|event| {
        if let Event::VisibilityResult(result) = event {
            require(
                result == format!("Keyboard {}", state.label().to_lowercase()),
                &result,
            )
        } else {
            unreachable!()
        }
    })?;
    snapshot(worker, state == Visibility::Minimised)?;
    Ok(())
}

pub fn run(work: &Path) -> TestResult<()> {
    let state: Value = serde_json::from_slice(&std::fs::read(work.join("fixture-state.json"))?)?;
    require(
        state["live_input"] == false,
        "Visibility tests require a recording fixture",
    )?;
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let result = tests(work, &state);
        CoUninitialize();
        if let Err(error) = &result {
            std::fs::create_dir_all("measurements")?;
            std::fs::write(
                "measurements/visibility-validation.json",
                serde_json::to_vec_pretty(
                    &json!({"passed":false,"alpha_commit":state["alpha_commit"],"error":error.to_string()}),
                )?,
            )?;
        }
        result
    }
}
unsafe fn tests(work: &Path, state: &Value) -> TestResult<()> {
    let pid = state["pid"].as_i64().ok_or("Fixture PID missing")? as i32;
    let mut fixture = Fixture {
        work: work.to_owned(),
        seq: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
    };
    fixture.command(json!({"op":"visible","visible":true}))?;
    fixture.command(json!({"op":"privacy","enabled":false}))?;
    fixture.command(json!({"op":"predictions","words":["hello","help","world"]}))?;
    let client = Client::new()?;
    let root = client
        .windows()?
        .into_iter()
        .find(|w| w.CurrentProcessId().ok() == Some(pid))
        .ok_or("Fixture not discovered")?;
    println!("Keyboard HWND: {:?}", root.CurrentNativeWindowHandle()?);
    let beacon = client.find_id(&root, uia::REVISION_ID)?;
    let worker = Worker::start(33);
    worker.tx.send(Command::Choose(pid))?;
    snapshot(&worker, false)?;

    let receiver_file = work.join(format!("receiver-{}.json", fixture.seq));
    let _receiver = Receiver(
        ProcessCommand::new(std::env::current_exe()?)
            .arg("--focus-receiver")
            .arg(&receiver_file)
            .creation_flags(0x08000000)
            .spawn()?,
    );
    let start = Instant::now();
    let receiver: Value = loop {
        if let Ok(bytes) = std::fs::read(&receiver_file)
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            break value;
        }
        require(
            start.elapsed() < Duration::from_secs(8),
            "Receiver startup timed out",
        )?;
        settle(20);
    };
    let handle = |key: &str| -> TestResult<HWND> {
        Ok(HWND(
            receiver[key].as_u64().ok_or("Receiver handle missing")? as usize as *mut _,
        ))
    };
    let window = handle("window")?;
    let edit = handle("edit")?;
    let other = handle("other")?;
    let monitor = Monitor::new()?;
    let focus_setup = Instant::now();
    loop {
        client.automation.ElementFromHandle(edit)?.SetFocus()?;
        PostMessageW(Some(window), WM_APP + 2, WPARAM(0), LPARAM(0))?;
        settle(250);
        monitor.reset();
        if monitor.check(window, edit).is_ok() {
            break;
        }
        require(
            focus_setup.elapsed() < Duration::from_secs(3),
            "Receiver could not acquire foreground during setup; avoid interacting with other applications",
        )?;
    }
    // Positive control: activate another receiver window, then restore the text
    // field once during setup. Never repair focus during the measured operations.
    PostMessageW(Some(window), WM_APP + 1, WPARAM(0), LPARAM(0))?;
    settle(250);
    require(
        GetForegroundWindow() == other,
        "Positive control did not take foreground",
    )?;
    require(
        monitor.check(window, edit).is_err(),
        "Monitor missed deliberate focus change",
    )?;
    require(
        FOCUS_EVENTS.with(|events| {
            events
                .borrow()
                .iter()
                .any(|(e, _)| *e == EVENT_SYSTEM_FOREGROUND)
                && events
                    .borrow()
                    .iter()
                    .any(|(e, _)| *e == EVENT_OBJECT_FOCUS)
        }),
        "Positive control missing foreground or focus events",
    )?;
    PostMessageW(Some(window), WM_APP + 2, WPARAM(0), LPARAM(0))?;
    settle(250);
    monitor.reset();
    println!("Checking receiver focus after positive control");
    monitor.check(window, edit)?;
    let mut checks =
        vec!["separate text receiver and foreground/focus event monitor positive control"];

    println!("Focus setup and positive control passed");
    for cycle in 0..3 {
        println!("Focus cycle {}", cycle + 1);
        change(&worker, VisibilityAction::Minimise, Visibility::Minimised)?;
        settle(200);
        println!(
            "Checking focus, keyboard state {:?}",
            uia::visibility(&root)?
        );
        monitor.check(window, edit)?;
        require(
            client.snapshot(&root)?.0.is_empty(),
            "Minimised provider still exposes targets",
        )?;
        require(
            client
                .windows()?
                .iter()
                .any(|w| w.CurrentProcessId().ok() == Some(pid)),
            "Minimised window disappeared",
        )?;
        change(&worker, VisibilityAction::Recall, Visibility::Shown)?;
        settle(200);
        println!(
            "Checking focus, keyboard state {:?}",
            uia::visibility(&root)?
        );
        monitor.check(window, edit)?;
    }
    checks.push("three worker minimise/recall cycles preserve foreground and text-field focus, including transient event monitoring");
    fixture.command(json!({"op":"predictions","words":["fresh","freshly","refresh"]}))?;
    let (targets, refs) = client.snapshot(&root)?;
    let letter = targets
        .iter()
        .find(|t| t.name == "a")
        .ok_or("Letter missing")?;
    let prediction = targets
        .iter()
        .find(|t| t.id.starts_with("aosk.v1.pred."))
        .ok_or("Prediction missing")?;
    let held = [refs[&letter.id].clone(), refs[&prediction.id].clone()];
    change(&worker, VisibilityAction::Minimise, Visibility::Minimised)?;
    let before = fixture.records()?;
    let mut held_results = Vec::new();
    for element in held {
        let result = element
            .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            .and_then(|pattern| pattern.Invoke());
        held_results.push(format!("{result:?}"));
    }
    settle(300);
    require(
        fixture.records()? == before,
        "Held element inserted while minimised",
    )?;
    checks.push("held key and prediction Invoke insert nothing while minimised");
    let late = Worker::start(33);
    late.tx.send(Command::Choose(pid))?;
    wait_event(&late, |event| {
        matches!(event, Event::Visibility(Visibility::Minimised))
    })?;
    snapshot(&late, true)?;
    change(&late, VisibilityAction::Recall, Visibility::Shown)?;
    monitor.check(window, edit)?;
    drop(late);
    worker.tx.send(Command::Refresh)?;
    snapshot(&worker, false)?;
    checks.push("client connecting to a minimised keyboard discovers it and recalls it");

    for shown in [false, true] {
        let previous = beacon.CurrentName()?.to_string();
        while worker.rx.try_recv().is_ok() {}
        fixture.command(json!({"op":"visible","visible":shown}))?;
        require(
            beacon.CurrentName()? != previous,
            "Direct provider state change did not move beacon",
        )?;
        wait_event(
            &worker,
            |event| matches!(event, Event::Visibility(v) if *v == if shown { Visibility::Shown } else { Visibility::Minimised }),
        )?;
        snapshot(&worker, !shown)?;
        settle(200);
        println!(
            "Checking focus, keyboard state {:?}",
            uia::visibility(&root)?
        );
        monitor.check(window, edit)?;
    }
    checks.push(
        "direct provider minimise/restore changes beacon and watcher state without stealing focus",
    );
    let expected_revision = beacon.CurrentName()?.to_string();
    worker.tx.send(Command::Refresh)?;
    let Event::Snapshot {
        revision, targets, ..
    } = wait_event(
        &worker,
        |event| matches!(event, Event::Snapshot { revision, targets, .. } if *revision == expected_revision && !targets.is_empty()),
    )?
    else {
        unreachable!()
    };
    let current = targets
        .into_iter()
        .find(|t| t.name == "a")
        .ok_or("Restored letter missing")?;
    let before = fixture.records()?;
    worker.tx.send(Command::Invoke {
        revision,
        target: current,
    })?;
    let outcome = wait_event(&worker, |event| matches!(event, Event::Invoked { .. }))?;
    if let Event::Invoked {
        result,
        foreground_preserved,
    } = outcome
    {
        require(
            result == "Activated once" && foreground_preserved,
            &format!("Restored activation: {result}, foreground preserved={foreground_preserved}"),
        )?;
    }
    settle(200);
    require(
        fixture.records()? > before,
        "Fresh restored key failed to insert",
    )?;
    monitor.check(window, edit)?;
    checks.push("fresh restored key activates after another client disconnects");
    fixture.command(json!({"op":"predictions","words":["restored","restore","rest"]}))?;
    let expected_revision = beacon.CurrentName()?.to_string();
    worker.tx.send(Command::Refresh)?;
    let Event::Snapshot {
        revision, targets, ..
    } = wait_event(
        &worker,
        |event| matches!(event, Event::Snapshot { revision, targets, .. } if *revision == expected_revision && !targets.is_empty()),
    )?
    else {
        unreachable!()
    };
    let current = targets
        .into_iter()
        .find(|t| t.id.starts_with("aosk.v1.pred."))
        .ok_or("Restored prediction missing")?;
    let before = fixture.records()?;
    worker.tx.send(Command::Invoke {
        revision,
        target: current,
    })?;
    let outcome = wait_event(&worker, |event| matches!(event, Event::Invoked { .. }))?;
    if let Event::Invoked {
        result,
        foreground_preserved,
    } = outcome
    {
        require(
            result == "Activated once" && foreground_preserved,
            &format!("Restored activation: {result}, foreground preserved={foreground_preserved}"),
        )?;
    }
    settle(200);
    require(
        fixture.records()? > before,
        "Fresh restored prediction failed to insert",
    )?;
    monitor.check(window, edit)?;
    checks.push("fresh restored prediction activates through recording bridge");

    // The fixture owns its process. Request a real quit, then verify discovery.
    let quit = json!({"op":"quit","seq":fixture.seq + 1});
    std::fs::write(
        work.join("fixture-command.json"),
        serde_json::to_vec(&quit)?,
    )?;
    wait_event(&worker, |event| {
        matches!(event, Event::Visibility(Visibility::NotRunning))
    })?;
    checks.push("fixture shutdown becomes Not running");
    let report = json!({"passed":true,"alpha_commit":state["alpha_commit"],"checks":checks,
        "fixture_records_only":true,"held_invoke_results":held_results,"focus_cycles":3,
        "focus_monitor":"WinEvent foreground and object-focus hooks plus GetGUIThreadInfo; no focus repair during measured operations",
        "unverified":["signed installed UIAccess", "mixed-DPI multi-monitor alignment", "physical global shortcut delivery", "actual taskbar click", "desktop text insertion"]});
    std::fs::create_dir_all("measurements")?;
    std::fs::write(
        "measurements/visibility-validation.json",
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

// Separate process with two ordinary windows, used only by --validate-visibility.
thread_local! { static RECEIVER: RefCell<Option<(HWND, HWND, HWND)>> = const { RefCell::new(None) }; }
unsafe extern "system" fn receiver_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_APP + 1 || msg == WM_APP + 2 {
        RECEIVER.with(|state| {
            if let Some((main, edit, other)) = *state.borrow() {
                let target = if msg == WM_APP + 1 { other } else { main };
                let _ = SetForegroundWindow(target);
                let _ = SetFocus(Some(if msg == WM_APP + 1 { other } else { edit }));
            }
        });
        return LRESULT(0);
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
pub fn receiver(path: &Path) -> TestResult<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(receiver_proc),
            hInstance: instance.into(),
            lpszClassName: w!("ScanLabFocusReceiver"),
            ..Default::default()
        };
        require(RegisterClassW(&class) != 0, "Receiver class failed")?;
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class.lpszClassName,
            w!("Scan Lab focus test receiver"),
            WS_OVERLAPPEDWINDOW,
            80,
            80,
            650,
            230,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP,
            20,
            20,
            550,
            80,
            Some(window),
            None,
            Some(instance.into()),
            None,
        )?;
        let other = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class.lpszClassName,
            w!("Scan Lab focus positive control"),
            WS_OVERLAPPEDWINDOW,
            100,
            100,
            400,
            150,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        RECEIVER.with(|state| *state.borrow_mut() = Some((window, edit, other)));
        let _ = ShowWindow(other, SW_SHOWNOACTIVATE);
        let _ = ShowWindow(window, SW_SHOWNORMAL);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);
        let _ = SetFocus(Some(edit));
        std::fs::write(
            path,
            serde_json::to_vec(
                &json!({"window":window.0 as usize,"edit":edit.0 as usize,"other":other.0 as usize}),
            )?,
        )?;
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Ok(())
    }
}
