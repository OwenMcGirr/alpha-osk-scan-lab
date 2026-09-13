//! Integration checks against our recording fixture. Never run against a user's keyboard.
use crate::{
    core::{diff, parse_id},
    uia::{Client, Command, Event, Samples, Worker, read_target},
};
use serde_json::{Value, json};
use std::{
    error::Error,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::Win32::{
    System::Com::*,
    UI::{Accessibility::*, WindowsAndMessaging::GetForegroundWindow},
};
type TestResult<T> = std::result::Result<T, Box<dyn Error>>;

fn require(ok: bool, reason: &str) -> TestResult<()> {
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::other(reason).into())
    }
}
struct Fixture {
    work: PathBuf,
    seq: u64,
}
impl Fixture {
    fn command(&mut self, mut data: Value) -> TestResult<()> {
        self.seq += 1;
        data["seq"] = json!(self.seq);
        let temporary = self.work.join("fixture-command.tmp");
        std::fs::write(&temporary, serde_json::to_vec(&data)?)?;
        // The fixture reads a complete JSON file, never partially written commands.
        std::fs::rename(&temporary, self.work.join("fixture-command.json"))?;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Ok(bytes) = std::fs::read(self.work.join("fixture-state.json"))
                && let Ok(state) = serde_json::from_slice::<Value>(&bytes)
                && state["seq"].as_u64() == Some(self.seq)
            {
                require(
                    state["error"].is_null(),
                    &format!("Fixture error: {}", state["error"]),
                )?;
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err(std::io::Error::other("Fixture command timed out").into())
    }
    fn records(&self) -> TestResult<usize> {
        match std::fs::read_to_string(self.work.join("input-records.jsonl")) {
            Ok(records) => Ok(records.lines().count()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error.into()),
        }
    }
}

pub fn run(work: &Path) -> TestResult<()> {
    let state: Value = serde_json::from_slice(&std::fs::read(work.join("fixture-state.json"))?)?;
    require(
        state["live_input"] == false,
        "Refusing tests: fixture must record input, not send it",
    )?;
    let pid = state["pid"].as_i64().ok_or("Fixture PID missing")? as i32;
    let mut fixture = Fixture {
        work: work.to_owned(),
        seq: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
    };
    let commit = state["alpha_commit"]
        .as_str()
        .ok_or("Fixture source commit missing")?;
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let result = tests(&mut fixture, pid, commit);
        CoUninitialize();
        result
    }
}

unsafe fn tests(f: &mut Fixture, pid: i32, commit: &str) -> TestResult<()> {
    let client = Client::new()?;
    let windows = client.windows()?;
    let root = windows
        .into_iter()
        .find(|w| w.CurrentProcessId().ok() == Some(pid))
        .ok_or("Recording fixture window not discovered")?;
    let beacon = client.find_id(&root, crate::uia::REVISION_ID)?;
    let mut checks = Vec::new();
    let mut provider_checks = Vec::new();
    f.command(json!({"op":"privacy","enabled":false}))?;
    f.command(json!({"op":"view","properties":{"compactView":false,"showNavigation":true,"showNumpad":false,"showFunctionRow":false,"showExtraFunctionRow":false}}))?;
    f.command(json!({"op":"geometry","x":40,"y":650,"width":1060}))?;
    let (baseline, _) = client.snapshot(&root)?;
    require(baseline.len() > 50, "Expected full keyboard")?;
    require(
        baseline
            .iter()
            .all(|t| parse_id(&t.id).is_some() && !t.name.trim().is_empty()),
        "All targets need identities and labels",
    )?;
    let toggles = baseline.iter().filter(|t| t.toggle.is_some()).count();
    require(
        toggles > 0 && toggles < baseline.len(),
        "Only modifier/lock controls should expose toggle state",
    )?;
    checks.push("discovery, target IDs, nonempty labels, selective TogglePattern");

    let previous = beacon.CurrentName()?.to_string();
    f.command(json!({"op":"geometry","x":140,"y":570,"width":1060}))?;
    let (moved, _) = client.snapshot(&root)?;
    require(
        beacon.CurrentName()? != previous,
        "Movement must change revision",
    )?;
    let changes = diff(&baseline, &moved);
    require(
        changes.moved > 50 && !changes.reset_scan(),
        "Movement should preserve scan identity",
    )?;
    checks.push("window movement changes bounds and revision while retaining selection identity");

    f.command(json!({"op":"view","properties":{"showFunctionRow":true,"showExtraFunctionRow":true,"showNumpad":true}}))?;
    let (expanded, _) = client.snapshot(&root)?;
    for section in ["fn1", "fn2", "pad"] {
        require(
            expanded
                .iter()
                .any(|t| parse_id(&t.id).unwrap().section == section),
            "Optional section absent",
        )?;
    }
    f.command(json!({"op":"view","properties":{"showFunctionRow":false,"showExtraFunctionRow":false,"showNumpad":false}}))?;
    let (contracted, _) = client.snapshot(&root)?;
    require(
        !contracted
            .iter()
            .any(|t| ["fn1", "fn2", "pad"].contains(&parse_id(&t.id).unwrap().section.as_str())),
        "Hidden panels leaked targets",
    )?;
    checks.push("optional function rows and numpad enter/leave the target map");

    f.command(json!({"op":"view","properties":{"compactView":true}}))?;
    let (compact, _) = client.snapshot(&root)?;
    require(
        diff(&contracted, &compact).reset_scan(),
        "Compact view must invalidate scan ordering",
    )?;
    f.command(json!({"op":"view","properties":{"compactView":false,"showNavigation":true}}))?;
    checks.push("compact/full layout change invalidates scan ordering");

    let (targets, refs) = client.snapshot(&root)?;
    let letter = targets
        .iter()
        .find(|t| t.name == "a")
        .ok_or("Letter a missing")?;
    let element = &refs[&letter.id];
    let before = f.records()?;
    let foreground = GetForegroundWindow();
    let invoke: IUIAutomationInvokePattern = element.GetCurrentPatternAs(UIA_InvokePatternId)?;
    invoke.Invoke()?;
    thread::sleep(Duration::from_millis(150));
    require(f.records()? > before, "Letter did not reach recorder")?;
    require(
        GetForegroundWindow() == foreground,
        "Invoke stole foreground focus",
    )?;
    checks.push("letter Invoke reaches existing synthesis route and preserves foreground");

    let (targets, refs) = client.snapshot(&root)?;
    let back = targets
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case("backspace"))
        .ok_or("Backspace missing")?;
    let before = f.records()?;
    refs[&back.id]
        .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?
        .Invoke()?;
    thread::sleep(Duration::from_millis(150));
    let immediately = f.records()?;
    thread::sleep(Duration::from_millis(1100));
    require(
        immediately > before && f.records()? == immediately,
        "Backspace missing or auto-repeated",
    )?;
    checks.push("special-key Invoke is one-shot without autorepeat");

    let (targets, refs) = client.snapshot(&root)?;
    let shift = targets
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case("shift"))
        .ok_or("Shift missing")?;
    refs[&shift.id]
        .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?
        .Invoke()?;
    thread::sleep(Duration::from_millis(150));
    let current_shift = read_target(&refs[&shift.id], false)?;
    require(current_shift.toggle == Some(1), "Shift did not become held")?;
    refs[&shift.id]
        .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?
        .Invoke()?;
    thread::sleep(Duration::from_millis(150));
    f.command(json!({"op":"lock","modifier":"shift"}))?;
    let (locked, _) = client.snapshot(&root)?;
    require(
        locked
            .iter()
            .any(|t| t.name.eq_ignore_ascii_case("shift") && t.locked),
        "FullDescription lock token missing",
    )?;
    f.command(json!({"op":"lock","modifier":"shift"}))?;
    checks.push("modifier Invoke changes TogglePattern; lock read through FullDescription");

    for same_words in [false, true] {
        f.command(json!({"op":"predictions","words":["hello","help","world"]}))?;
        let (targets, refs) = client.snapshot(&root)?;
        let pill = targets
            .iter()
            .find(|t| t.id.starts_with("aosk.v1.pred."))
            .ok_or("Prediction absent")?;
        let stale = refs[&pill.id].clone();
        let replacement = if same_words {
            vec!["hello", "help", "world"]
        } else {
            vec!["different", "second", "third"]
        };
        f.command(json!({"op":"predictions","words":replacement}))?;
        let (fresh, _) = client.snapshot(&root)?;
        require(
            !fresh.iter().any(|t| t.id == pill.id),
            "Prediction generation reused",
        )?;
        let before = f.records()?;
        let current_before = stale.CurrentAutomationId().ok().map(|v| v.to_string());
        let outcome = stale
            .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            .and_then(|p| p.Invoke());
        thread::sleep(Duration::from_millis(300));
        let calls = f
            .records()?
            .checked_sub(before)
            .ok_or("Fixture recording was truncated during validation")?;
        let identity_unchanged = current_before.as_ref().is_none_or(|id| id == &pill.id);
        let correct = calls == 0 && identity_unchanged;
        provider_checks.push(json!({"test":"unguarded stale prediction Invoke", "same_words":same_words,
                "cached_id":pill.id,"current_id_before_invoke":current_before,"invoke_returned_success":outcome.is_ok(),
                "new_synthesis_calls":calls,"expected":"zero synthesis calls; held identity never changes",
                "hresult":outcome.as_ref().err().map(|e|format!("0x{:08X}",e.code().0 as u32)),
                "passed":correct}));
        if correct {
            checks.push(if same_words {
                "unguarded identical-round stale prediction inserts nothing"
            } else {
                "unguarded different-round stale prediction inserts nothing"
            });
        }
    }
    f.command(json!({"op":"predictions","words":["hello","help","world"]}))?;
    let worker = Worker::start(16);
    worker.tx.send(Command::Choose(pid))?;
    let start = Instant::now();
    let mut captured = None;
    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(Event::Snapshot {
            revision, targets, ..
        }) = worker.rx.recv_timeout(Duration::from_millis(100))
            && let Some(t) = targets.iter().find(|t| t.id.contains(".pred."))
        {
            captured = Some((revision, t.clone()));
            break;
        }
    }
    let (old_revision, old_target) = captured.ok_or("Watcher failed to collect prediction")?;
    f.command(json!({"op":"predictions","words":["different","second","third"]}))?;
    let before = f.records()?;
    worker.tx.send(Command::Invoke {
        revision: old_revision,
        target: old_target,
    })?;
    let start = Instant::now();
    let mut cancelled = false;
    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(Event::Invoked { result, .. }) =
            worker.rx.recv_timeout(Duration::from_millis(100))
        {
            cancelled = result.starts_with("Cancelled:");
            break;
        }
    }
    require(
        cancelled && f.records()? == before,
        "Watcher failed to reject stale selection",
    )?;
    drop(worker);
    checks.push(
        "watcher revision/identity validation cancels stale selection with zero synthesis calls",
    );
    f.command(json!({"op":"predictions","words":["hello","help","world"]}))?;
    let (targets, refs) = client.snapshot(&root)?;
    let pill = targets
        .iter()
        .find(|t| t.id.starts_with("aosk.v1.pred."))
        .unwrap();
    let before = f.records()?;
    refs[&pill.id]
        .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?
        .Invoke()?;
    thread::sleep(Duration::from_millis(150));
    require(f.records()? > before, "Current prediction did not insert")?;
    checks.push("current prediction Invoke inserts through the bridge");

    f.command(json!({"op":"predictions","words":["hello","help","world"]}))?;
    f.command(json!({"op":"privacy","enabled":true}))?;
    let (private, _) = client.snapshot(&root)?;
    require(
        !private.iter().any(|t| t.id.starts_with("aosk.v1.pred.")),
        "Privacy mode leaked predictions",
    )?;
    f.command(json!({"op":"privacy","enabled":false}))?;
    checks.push("privacy mode removes prediction targets");

    f.command(json!({"op":"visible","visible":false}))?;
    let hidden = client
        .snapshot(&root)
        .map(|(t, _)| t.is_empty())
        .unwrap_or(true);
    require(hidden, "Hidden keyboard retained available targets")?;
    f.command(json!({"op":"visible","visible":true}))?;
    checks.push("hidden keyboard has no available scan targets");

    let mut read_times = Samples::default();
    let mut snapshot_times = Samples::default();
    for _ in 0..50 {
        let start = Instant::now();
        let _ = beacon.CurrentName()?;
        read_times.record(start.elapsed().as_secs_f64() * 1000.0);
    }
    for _ in 0..12 {
        let start = Instant::now();
        let _ = client.snapshot(&root)?;
        snapshot_times.record(start.elapsed().as_secs_f64() * 1000.0);
    }
    f.command(json!({"op":"geometry","x":40,"y":570,"width":1060}))?;
    f.command(json!({"op":"predictions","words":["hello","help","world"]}))?;
    let report = json!({"alpha_commit":commit,"checks":checks,
        "beacon":read_times.summary(),"snapshot":snapshot_times.summary(),"fixture_records_only":true,"provider_checks":provider_checks,
        "unverified":["mixed-DPI multi-monitor alignment","signed installed UIAccess boundary","physical switch hardware"]});
    std::fs::create_dir_all("measurements")?;
    std::fs::write(
        "measurements/live-validation.json",
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    require(
        provider_checks.iter().all(|check| check["passed"] == true),
        "Provider stale-prediction regression: see measurements/live-validation.json",
    )
}
