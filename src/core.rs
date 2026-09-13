use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetId {
    pub section: String,
    pub row: u32,
    pub index: u32,
    pub generation: Option<u64>,
}

pub fn parse_id(id: &str) -> Option<TargetId> {
    let p: Vec<_> = id.split('.').collect();
    if p.len() != 5 || p[0] != "aosk" || p[1] != "v1" {
        return None;
    }
    let (row, index, generation) = if p[2] == "pred" {
        (
            0,
            p[3].parse().ok()?,
            Some(p[4].strip_prefix('g')?.parse().ok()?),
        )
    } else {
        if !["grid", "fn1", "fn2", "num", "nav", "pad"].contains(&p[2]) {
            return None;
        }
        (p[3].parse().ok()?, p[4].parse().ok()?, None)
    };
    Some(TargetId {
        section: p[2].into(),
        row,
        index,
        generation,
    })
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl Bounds {
    pub fn valid(self) -> bool {
        self.right > self.left && self.bottom > self.top
    }
    pub fn union(self, b: Self) -> Self {
        Self {
            left: self.left.min(b.left),
            top: self.top.min(b.top),
            right: self.right.max(b.right),
            bottom: self.bottom.max(b.bottom),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub id: String,
    pub runtime: Vec<i32>,
    pub name: String,
    pub bounds: Bounds,
    pub enabled: bool,
    pub offscreen: bool,
    pub toggle: Option<i32>,
    pub locked: bool,
}
impl Target {
    pub fn available(&self) -> bool {
        self.enabled && !self.offscreen && self.bounds.valid()
    }
    pub fn same_action(&self, other: &Self) -> bool {
        self.id == other.id
            && self.runtime == other.runtime
            && self.name == other.name
            && self.enabled == other.enabled
            && self.offscreen == other.offscreen
            && self.toggle == other.toggle
            && self.locked == other.locked
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Changes {
    pub added: usize,
    pub removed: usize,
    pub moved: usize,
    pub relabeled: usize,
    pub state_changed: usize,
    pub replaced: usize,
}
impl Changes {
    pub fn reset_scan(&self) -> bool {
        self.added + self.removed + self.relabeled + self.state_changed + self.replaced > 0
    }
    pub fn add(&mut self, other: &Self) {
        self.added += other.added;
        self.removed += other.removed;
        self.moved += other.moved;
        self.relabeled += other.relabeled;
        self.state_changed += other.state_changed;
        self.replaced += other.replaced;
    }
}
pub fn diff(old: &[Target], new: &[Target]) -> Changes {
    let before: HashMap<_, _> = old.iter().map(|t| (&t.id, t)).collect();
    let after: HashMap<_, _> = new.iter().map(|t| (&t.id, t)).collect();
    let mut c = Changes::default();
    for t in new {
        if let Some(p) = before.get(&t.id) {
            c.moved += usize::from(t.bounds != p.bounds);
            c.relabeled += usize::from(t.name != p.name);
            c.replaced += usize::from(t.runtime != p.runtime);
            c.state_changed += usize::from(
                t.enabled != p.enabled
                    || t.offscreen != p.offscreen
                    || t.toggle != p.toggle
                    || t.locked != p.locked,
            );
        } else {
            c.added += 1;
        }
    }
    c.removed = old.iter().filter(|t| !after.contains_key(&t.id)).count();
    c
}

pub fn rows(targets: &[Target]) -> Vec<Vec<usize>> {
    let mut map: BTreeMap<(String, u32), Vec<(u32, usize)>> = BTreeMap::new();
    for (i, t) in targets.iter().enumerate().filter(|(_, t)| t.available()) {
        if let Some(id) = parse_id(&t.id) {
            map.entry((id.section, id.row))
                .or_default()
                .push((id.index, i));
        }
    }
    let mut groups: Vec<Vec<usize>> = map
        .into_values()
        .map(|mut v| {
            v.sort_unstable();
            v.into_iter().map(|(_, i)| i).collect()
        })
        .collect();
    groups.sort_by_key(|r| {
        let rect = r
            .iter()
            .map(|&i| targets[i].bounds)
            .reduce(Bounds::union)
            .unwrap();
        (rect.top, rect.left)
    });
    groups
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanMode {
    Linear,
    Rows,
}
#[derive(Debug)]
pub struct Scanner {
    pub mode: ScanMode,
    pub running: bool,
    pub group: usize,
    pub item: usize,
    pub inside: bool,
}
impl Default for Scanner {
    fn default() -> Self {
        Self {
            mode: ScanMode::Rows,
            running: false,
            group: 0,
            item: 0,
            inside: false,
        }
    }
}
impl Scanner {
    pub fn reset(&mut self) {
        self.group = 0;
        self.item = 0;
        self.inside = false;
    }
    pub fn step(&mut self, groups: &[Vec<usize>]) {
        if groups.is_empty() {
            return;
        }
        if self.mode == ScanMode::Linear {
            self.item = (self.item + 1) % groups.iter().map(Vec::len).sum::<usize>();
        } else if self.inside {
            self.item = (self.item + 1) % groups[self.group % groups.len()].len();
        } else {
            self.group = (self.group + 1) % groups.len();
        }
    }
    pub fn highlighted(&self, groups: &[Vec<usize>]) -> Vec<usize> {
        if !self.running || groups.is_empty() {
            return vec![];
        }
        if self.mode == ScanMode::Linear {
            groups
                .iter()
                .flatten()
                .nth(self.item)
                .copied()
                .into_iter()
                .collect()
        } else {
            let row = &groups[self.group % groups.len()];
            if self.inside {
                vec![row[self.item % row.len()]]
            } else {
                row.clone()
            }
        }
    }
    pub fn select(&mut self, groups: &[Vec<usize>]) -> Option<usize> {
        if !self.running || groups.is_empty() {
            return None;
        }
        if self.mode == ScanMode::Rows && !self.inside {
            self.inside = true;
            self.item = 0;
            None
        } else {
            let target = self.highlighted(groups).first().copied();
            self.reset();
            target
        }
    }
    pub fn cancel(&mut self) {
        if self.inside {
            self.inside = false;
            self.item = 0;
        } else {
            self.running = false;
            self.reset();
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub poll_ms: u64,
    pub scan_ms: u64,
    pub mode: ScanMode,
    pub outlines: bool,
    pub start_key: String,
    pub select_key: String,
    pub cancel_key: String,
    pub visibility_key: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_ms: 33,
            scan_ms: 1000,
            mode: ScanMode::Rows,
            outlines: true,
            start_key: "Ctrl+Alt+F8".into(),
            select_key: "Ctrl+Alt+F9".into(),
            cancel_key: "Ctrl+Alt+F10".into(),
            visibility_key: "Ctrl+Alt+F11".into(),
        }
    }
}
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    let mut mods = 0;
    let mut key = None;
    for p in s.split('+').map(str::trim) {
        match p.to_ascii_lowercase().as_str() {
            "ctrl" => mods |= 2,
            "alt" => mods |= 1,
            "shift" => mods |= 4,
            "win" => mods |= 8,
            other => {
                if key.is_some() {
                    return None;
                }
                let n: u32 = other.strip_prefix('f')?.parse().ok()?;
                if !(1..=24).contains(&n) {
                    return None;
                }
                key = Some(0x70 + n - 1);
            }
        }
    }
    Some((mods | 0x4000, key?)) // MOD_NOREPEAT
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(id: &str, x: i32, y: i32) -> Target {
        Target {
            id: id.into(),
            name: "a".into(),
            runtime: vec![1],
            bounds: Bounds {
                left: x,
                top: y,
                right: x + 30,
                bottom: y + 30,
            },
            enabled: true,
            offscreen: false,
            toggle: None,
            locked: false,
        }
    }
    #[test]
    fn old_settings_keep_custom_shortcuts_and_get_visibility_default() {
        let settings: Settings =
            serde_json::from_str(r#"{"start_key":"Shift+F2","scan_ms":725}"#).unwrap();
        assert_eq!(settings.start_key, "Shift+F2");
        assert_eq!(settings.scan_ms, 725);
        assert_eq!(settings.visibility_key, "Ctrl+Alt+F11");
        assert_ne!(
            parse_hotkey(&settings.visibility_key),
            parse_hotkey(&settings.select_key)
        );
    }
    #[test]
    fn ids() {
        assert_eq!(parse_id("aosk.v1.pred.2.g19").unwrap().generation, Some(19));
        for id in [
            "aosk.v1.revision",
            "aosk.v2.grid.0.0",
            "aosk.v1.no.0.0",
            "aosk.v1.grid.-1.0",
            "aosk.v1.pred.0.12",
        ] {
            assert!(parse_id(id).is_none());
        }
    }
    #[test]
    fn geometry_preserves_selection_but_replacements_do_not() {
        let a = t("aosk.v1.pred.0.g1", 0, 0);
        let mut b = a.clone();
        b.bounds.left = -10;
        assert_eq!(diff(std::slice::from_ref(&a), &[b.clone()]).moved, 1);
        assert!(!diff(std::slice::from_ref(&a), &[b.clone()]).reset_scan());
        b.id = "aosk.v1.pred.0.g2".into();
        assert!(diff(std::slice::from_ref(&a), &[b.clone()]).reset_scan());
        assert!(!a.same_action(&b));
    }
    #[test]
    fn state_and_label_invalidate() {
        let a = t("aosk.v1.grid.0.0", 0, 0);
        let mut b = a.clone();
        b.name = "A".into();
        assert!(!a.same_action(&b));
        assert_eq!(diff(std::slice::from_ref(&a), &[b]).relabeled, 1);
        let mut b = a.clone();
        b.locked = true;
        assert_eq!(diff(&[a], &[b]).state_changed, 1);
    }
    #[test]
    fn spatial_rows_and_hidden_targets() {
        let mut hidden = t("aosk.v1.pad.0.0", 99, 0);
        hidden.offscreen = true;
        let targets = vec![
            t("aosk.v1.grid.0.1", 30, 40),
            t("aosk.v1.pred.0.g1", -30, 0),
            t("aosk.v1.grid.0.0", 0, 40),
            hidden,
        ];
        assert_eq!(rows(&targets), vec![vec![1], vec![2, 0]]);
    }
    #[test]
    fn row_and_linear_transitions() {
        let g = vec![vec![0, 1], vec![2]];
        let mut s = Scanner {
            running: true,
            ..Default::default()
        };
        assert_eq!(s.highlighted(&g), vec![0, 1]);
        assert_eq!(s.select(&g), None);
        s.step(&g);
        assert_eq!(s.select(&g), Some(1));
        assert!(!s.inside);
        s.select(&g);
        s.cancel();
        assert!(s.running);
        s.cancel();
        assert!(!s.running);
        s.mode = ScanMode::Linear;
        s.running = true;
        s.step(&g);
        s.step(&g);
        assert_eq!(s.select(&g), Some(2));
        assert_eq!(s.item, 0);
    }
    #[test]
    fn empty_scan_and_shortcuts() {
        let mut s = Scanner::default();
        s.step(&[]);
        assert_eq!(s.select(&[]), None);
        assert_eq!(parse_hotkey("Ctrl+Alt+F8"), Some((0x4003, 0x77)));
        assert!(parse_hotkey("F9+F10").is_none());
        assert!(parse_hotkey("F25").is_none());
    }
}
