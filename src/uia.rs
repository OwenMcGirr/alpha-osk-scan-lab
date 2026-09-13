use crate::core::{Bounds, Target, parse_id};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, Ole::*, Variant::*},
        UI::{Accessibility::*, WindowsAndMessaging::GetForegroundWindow},
    },
    core::{BSTR, Interface, Result},
};

pub const WINDOW_ID: &str = "alphaOsk.alphaOskKeyboard";
pub const REVISION_ID: &str = "aosk.v1.revision";

#[derive(Default, Clone, Serialize)]
pub struct Samples {
    pub count: u64,
    #[serde(skip)]
    values: VecDeque<f64>,
}
impl Samples {
    pub fn record(&mut self, ms: f64) {
        self.count += 1;
        if self.values.len() == 4096 {
            self.values.pop_front();
        }
        self.values.push_back(ms);
    }
    pub fn percentile(&self, q: f64) -> f64 {
        let mut v: Vec<_> = self.values.iter().copied().collect();
        v.sort_by(f64::total_cmp);
        if v.is_empty() {
            0.0
        } else {
            v[((v.len() - 1) as f64 * q).round() as usize]
        }
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"count":self.count,"window_samples":self.values.len(),"p50_ms":self.percentile(0.5),"p95_ms":self.percentile(0.95)})
    }
}

pub enum Command {
    Refresh,
    Poll(u64),
    Choose(i32),
    Invoke { revision: String, target: Target },
    Stop,
}
pub enum Event {
    Status(String),
    Candidates(Vec<(i32, String)>),
    Invalidated,
    Snapshot {
        revision: String,
        targets: Vec<Target>,
        detected: Instant,
    },
    Timing {
        beacon_ms: f64,
        snapshot_ms: Option<f64>,
    },
    Invoked {
        result: String,
        foreground_preserved: bool,
    },
}
pub struct Worker {
    pub tx: mpsc::Sender<Command>,
    pub rx: mpsc::Receiver<Event>,
}
impl Worker {
    pub fn start(poll_ms: u64) -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        thread::spawn(move || {
            // COM pointers are created, used, and dropped only on this MTA thread.
            unsafe {
                if let Err(e) = CoInitializeEx(None, COINIT_MULTITHREADED).ok() {
                    let _ = events.send(Event::Status(format!("COM initialization failed: {e}")));
                    return;
                }
                if let Err(e) = run(commands, &events, poll_ms) {
                    let _ = events.send(Event::Status(format!("UIA worker failed: {e}")));
                }
                CoUninitialize();
            }
        });
        Self { tx, rx }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
    }
}

pub struct Client {
    pub automation: IUIAutomation,
    cache: IUIAutomationCacheRequest,
}
impl Client {
    pub unsafe fn new() -> Result<Self> {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
        let a2: IUIAutomation2 = automation.cast()?;
        a2.SetConnectionTimeout(500)?;
        a2.SetTransactionTimeout(500)?;
        let cache = automation.CreateCacheRequest()?;
        cache.SetTreeScope(TreeScope_Element)?;
        for p in [
            UIA_AutomationIdPropertyId,
            UIA_NamePropertyId,
            UIA_ControlTypePropertyId,
            UIA_BoundingRectanglePropertyId,
            UIA_IsEnabledPropertyId,
            UIA_IsOffscreenPropertyId,
            UIA_RuntimeIdPropertyId,
            UIA_IsTogglePatternAvailablePropertyId,
            UIA_ToggleToggleStatePropertyId,
            UIA_FullDescriptionPropertyId,
        ] {
            cache.AddProperty(p)?;
        }
        cache.AddPattern(UIA_InvokePatternId)?;
        Ok(Self { automation, cache })
    }
    pub unsafe fn find_id(
        &self,
        root: &IUIAutomationElement,
        id: &str,
    ) -> Result<IUIAutomationElement> {
        let cond = self
            .automation
            .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(id))?;
        root.FindFirst(TreeScope_Descendants, &cond)
    }
    pub unsafe fn windows(&self) -> Result<Vec<IUIAutomationElement>> {
        let root = self.automation.GetRootElement()?;
        let cond = self
            .automation
            .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(WINDOW_ID))?;
        let list = root.FindAll(TreeScope_Children, &cond)?;
        (0..list.Length()?).map(|i| list.GetElement(i)).collect()
    }
    pub unsafe fn snapshot(
        &self,
        root: &IUIAutomationElement,
    ) -> Result<(Vec<Target>, HashMap<String, IUIAutomationElement>)> {
        let cond = self.automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_ButtonControlTypeId.0),
        )?;
        let elements = root.FindAllBuildCache(TreeScope_Descendants, &cond, &self.cache)?;
        let mut targets = Vec::new();
        let mut refs = HashMap::new();
        let mut seen = HashSet::new();
        for i in 0..elements.Length()? {
            let el = elements.GetElement(i)?;
            let id = el.CachedAutomationId()?.to_string();
            if parse_id(&id).is_none() {
                continue;
            }
            if !seen.insert(id.clone()) {
                return Err(windows::core::Error::new(
                    E_FAIL,
                    "Duplicate target IDs; activation suspended",
                ));
            }
            let t = read_target(&el, true)?;
            if t.available() {
                refs.insert(id, el);
                targets.push(t);
            }
        }
        targets.sort_by(|a, b| a.id.cmp(&b.id));
        Ok((targets, refs))
    }
}

unsafe fn variant_string(v: &VARIANT) -> String {
    BSTR::try_from(v).map(|s| s.to_string()).unwrap_or_default()
}
unsafe fn runtime_id(v: &VARIANT) -> Result<Vec<i32>> {
    if v.vt() != VARENUM(VT_ARRAY.0 | VT_I4.0) {
        return Err(windows::core::Error::new(
            E_FAIL,
            "Missing runtime identity",
        ));
    }
    let a = v.Anonymous.Anonymous.Anonymous.parray;
    let lo = SafeArrayGetLBound(a, 1)?;
    let hi = SafeArrayGetUBound(a, 1)?;
    let mut result = Vec::new();
    if hi - lo > 128 {
        return Err(windows::core::Error::new(
            E_FAIL,
            "Invalid runtime identity",
        ));
    }
    for i in lo..=hi {
        let mut n = 0i32;
        SafeArrayGetElement(a, &i, (&mut n as *mut i32).cast())?;
        result.push(n);
    }
    Ok(result)
}
pub unsafe fn read_target(el: &IUIAutomationElement, cached: bool) -> Result<Target> {
    let property = |p| {
        if cached {
            el.GetCachedPropertyValue(p)
        } else {
            el.GetCurrentPropertyValue(p)
        }
    };
    let bounds = if cached {
        el.CachedBoundingRectangle()?
    } else {
        el.CurrentBoundingRectangle()?
    };
    Ok(Target {
        id: variant_string(&property(UIA_AutomationIdPropertyId)?),
        name: variant_string(&property(UIA_NamePropertyId)?),
        runtime: runtime_id(&property(UIA_RuntimeIdPropertyId)?)?,
        bounds: Bounds {
            left: bounds.left,
            top: bounds.top,
            right: bounds.right,
            bottom: bounds.bottom,
        },
        enabled: bool::try_from(&property(UIA_IsEnabledPropertyId)?).unwrap_or(false),
        offscreen: bool::try_from(&property(UIA_IsOffscreenPropertyId)?).unwrap_or(true),
        toggle: if bool::try_from(&property(UIA_IsTogglePatternAvailablePropertyId)?)
            .unwrap_or(false)
        {
            property(UIA_ToggleToggleStatePropertyId)
                .ok()
                .and_then(|v| i32::try_from(&v).ok())
        } else {
            None
        },
        locked: variant_string(&property(UIA_FullDescriptionPropertyId)?)
            .split_whitespace()
            .any(|s| s == "locked"),
    })
}

unsafe fn run(
    commands: mpsc::Receiver<Command>,
    events: &mpsc::Sender<Event>,
    mut poll: u64,
) -> Result<()> {
    let client = Client::new()?;
    let mut chosen = None;
    let mut root: Option<IUIAutomationElement> = None;
    let mut beacon: Option<IUIAutomationElement> = None;
    let mut revision = String::new();
    let mut refs: HashMap<String, IUIAutomationElement> = HashMap::new();
    let mut status = String::new();
    let mut last_poll = Instant::now() - Duration::from_secs(1);
    let mut retry = Instant::now();
    let mut valid = false;
    let mut pending_detection = None;
    loop {
        let command = commands.recv_timeout(Duration::from_millis(2));
        match command {
            Ok(Command::Refresh) => {
                valid = false;
                let _ = events.send(Event::Invalidated);
                last_poll = Instant::now() - Duration::from_secs(1);
            }
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Command::Poll(ms)) => poll = ms.clamp(16, 1000),
            Ok(Command::Choose(pid)) => {
                chosen = Some(pid);
                root = None;
                beacon = None;
                valid = false;
                refs.clear();
                retry = Instant::now();
                let _ = events.send(Event::Invalidated);
            }
            Ok(Command::Invoke {
                revision: expected,
                target,
            }) => {
                let foreground = GetForegroundWindow();
                let outcome = (|| -> Result<()> {
                    let b = beacon.as_ref().ok_or_else(|| {
                        windows::core::Error::new(E_FAIL, "Keyboard disconnected")
                    })?;
                    if !valid || revision != expected || b.CurrentName()? != expected {
                        return Err(windows::core::Error::new(
                            E_FAIL,
                            "Selection expired: keyboard changed",
                        ));
                    }
                    let el = refs.get(&target.id).ok_or_else(|| {
                        windows::core::Error::new(
                            windows::core::HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32),
                            "Target removed",
                        )
                    })?;
                    let now = read_target(el, false)?;
                    if !target.same_action(&now) || !now.available() || target.bounds != now.bounds
                    {
                        return Err(windows::core::Error::new(
                            E_FAIL,
                            "Selection expired: target changed",
                        ));
                    }
                    let invoke: IUIAutomationInvokePattern =
                        el.GetCurrentPatternAs(UIA_InvokePatternId)?;
                    if b.CurrentName()? != expected {
                        return Err(windows::core::Error::new(
                            E_FAIL,
                            "Selection expired during validation",
                        ));
                    }
                    invoke.Invoke()
                })();
                let _ = events.send(Event::Invoked {
                    result: match outcome {
                        Ok(()) => "Activated once".into(),
                        Err(e) => format!("Cancelled: {e}"),
                    },
                    foreground_preserved: foreground == GetForegroundWindow(),
                });
                // Even a successful invocation may have changed the map. Never reuse it.
                valid = false;
                let _ = events.send(Event::Invalidated);
                pending_detection = Some(Instant::now());
                last_poll = Instant::now() - Duration::from_secs(1);
            }
            _ => {}
        }
        if root.is_none() {
            if Instant::now() < retry {
                continue;
            }
            retry = Instant::now() + Duration::from_secs(1);
            let windows = match client.windows() {
                Ok(windows) => windows,
                Err(e) => {
                    let next = format!("UIA discovery failed; retrying: {e}");
                    if next != status {
                        status = next;
                        let _ = events.send(Event::Status(status.clone()));
                    }
                    continue;
                }
            };
            let candidates: Vec<_> = windows
                .iter()
                .filter_map(|w| {
                    Some((
                        w.CurrentProcessId().ok()?,
                        w.CurrentName().ok()?.to_string(),
                    ))
                })
                .collect();
            if chosen.is_some_and(|pid| !candidates.iter().any(|(candidate, _)| *candidate == pid))
            {
                chosen = None;
            }
            let selected = if let Some(pid) = chosen {
                windows
                    .iter()
                    .find(|w| w.CurrentProcessId().ok() == Some(pid))
            } else if windows.len() == 1 {
                windows.first()
            } else {
                None
            };
            if let Some(w) = selected {
                root = Some(w.clone());
                beacon = client.find_id(w, REVISION_ID).ok();
                revision.clear();
                valid = false;
                let next = if beacon.is_some() {
                    "Connected; reading targets"
                } else {
                    "Keyboard found but UIA revision is unavailable"
                };
                if status != next {
                    status = next.into();
                    let _ = events.send(Event::Status(status.clone()));
                }
                let _ = events.send(Event::Candidates(candidates));
            } else {
                let next = if windows.len() > 1 {
                    "Multiple keyboards: choose a process"
                } else {
                    "Waiting for Alpha-OSK with PR #114 UIA support"
                };
                if status != next {
                    status = next.into();
                    let _ = events.send(Event::Status(status.clone()));
                    let _ = events.send(Event::Candidates(candidates));
                }
                continue;
            }
        }
        if last_poll.elapsed() < Duration::from_millis(poll) {
            continue;
        }
        last_poll = Instant::now();
        let started = Instant::now();
        let result = (|| -> Result<()> {
            let r = root.as_ref().unwrap();
            // Hidden windows may lose their beacon. Re-discover rather than keeping stale rectangles.
            if beacon.is_none() {
                beacon = Some(client.find_id(r, REVISION_ID)?);
            }
            let b = beacon.as_ref().unwrap();
            let value = b.CurrentName()?.to_string();
            let beacon_ms = started.elapsed().as_secs_f64() * 1000.0;
            if value == revision && valid {
                let _ = events.send(Event::Timing {
                    beacon_ms,
                    snapshot_ms: None,
                });
                return Ok(());
            }
            if valid {
                let _ = events.send(Event::Invalidated);
            }
            valid = false;
            let detected = *pending_detection.get_or_insert_with(Instant::now);
            let snapshot_start = Instant::now();
            let (targets, elements) = client.snapshot(r)?;
            let after = b.CurrentName()?.to_string();
            let _ = events.send(Event::Timing {
                beacon_ms,
                snapshot_ms: Some(snapshot_start.elapsed().as_secs_f64() * 1000.0),
            });
            if after != value {
                return Ok(());
            }
            revision = value;
            refs = elements;
            valid = true;
            pending_detection = None;
            let _ = events.send(Event::Snapshot {
                revision: revision.clone(),
                targets,
                detected,
            });
            if status != "Connected" {
                status = "Connected".into();
                let _ = events.send(Event::Status(status.clone()));
            }
            Ok(())
        })();
        if let Err(e) = result {
            valid = false;
            refs.clear();
            root = None;
            beacon = None;
            pending_detection = None;
            let _ = events.send(Event::Invalidated);
            let next = format!("Disconnected / UIA unavailable: {e}");
            if status != next {
                status = next;
                let _ = events.send(Event::Status(status.clone()));
            }
        }
    }
    Ok(())
}

pub fn probe(seconds: u64) {
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() {
            if let Ok(client) = Client::new()
                && let Ok(root) = client.automation.GetRootElement()
                && let Ok(cond) = client
                    .automation
                    .CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from("Alpha-OSK"))
                && let Ok(list) = root.FindAll(TreeScope_Children, &cond)
            {
                for i in 0..list.Length().unwrap_or(0) {
                    if let Ok(el) = list.GetElement(i) {
                        println!("Alpha window diagnostic: id={:?}", el.CurrentAutomationId());
                    }
                }
            }
            CoUninitialize();
        }
    }
    let worker = Worker::start(33);
    let start = Instant::now();
    let mut beacon = Samples::default();
    let mut snapshot = Samples::default();
    while start.elapsed() < Duration::from_secs(seconds) {
        if let Ok(event) = worker.rx.recv_timeout(Duration::from_millis(100)) {
            match event {
                Event::Status(s) => println!("{s}"),
                Event::Snapshot {
                    revision: _,
                    targets,
                    ..
                } => println!(
                    "snapshot targets={} predictions={} toggles={} locked={}",
                    targets.len(),
                    targets.iter().filter(|t| t.id.contains(".pred.")).count(),
                    targets.iter().filter(|t| t.toggle.is_some()).count(),
                    targets.iter().filter(|t| t.locked).count()
                ),
                Event::Timing {
                    beacon_ms,
                    snapshot_ms,
                } => {
                    beacon.record(beacon_ms);
                    if let Some(ms) = snapshot_ms {
                        snapshot.record(ms);
                    }
                }
                _ => {}
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"beacon":beacon.summary(),"snapshot":snapshot.summary()})
    );
}
