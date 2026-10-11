//! Stacks: images that belong together (a burst, a bracket, versions of a motif) shown as one
//! thumbnail, the top image, until the stack is expanded.
//!
//! Membership is stored in the sidecar of each raw file (see `sidecar::Stack`); variants belong
//! to the stack of their file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Result;

use crate::sidecar::{self, Stack};

/// A new, practically unique stack id.
pub fn new_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:04x}", nanos as u64, n & 0xffff)
}

/// Puts the files of `paths` into one new stack with `top` (or the first) as top image.
/// Returns the stack and the files with their new membership.
pub fn create(paths: &[PathBuf], top: Option<&Path>) -> Result<Vec<(PathBuf, Option<Stack>)>> {
    let files = crate::variant::files_of(paths);
    let top = top.map(crate::variant::file_of).filter(|t| files.contains(t)).or_else(|| files.first().cloned());
    let id = new_id();
    let mut changed = Vec::new();
    for f in files {
        let s = Stack { id: id.clone(), top: Some(&f) == top.as_ref() };
        sidecar::set_stack(&f, Some(s.clone()))?;
        changed.push((f, Some(s)));
    }
    Ok(changed)
}

/// Removes the files of `paths` from their stacks. `members` lists all files known to be in
/// the same stacks: a stack that loses its top gets a new one, a stack of one is dissolved.
pub fn remove(paths: &[PathBuf], members: &[(PathBuf, Stack)]) -> Result<Vec<(PathBuf, Option<Stack>)>> {
    let files = crate::variant::files_of(paths);
    let mut changed = Vec::new();
    for f in &files {
        sidecar::set_stack(f, None)?;
        changed.push((f.clone(), None));
    }
    let mut ids: Vec<&str> = members.iter().filter(|(p, _)| files.contains(p)).map(|(_, s)| s.id.as_str()).collect();
    ids.dedup();
    for id in ids {
        let rest: Vec<&(PathBuf, Stack)> = members.iter().filter(|(p, s)| s.id == id && !files.contains(p)).collect();
        if rest.len() == 1 {
            sidecar::set_stack(&rest[0].0, None)?;
            changed.push((rest[0].0.clone(), None));
        } else if !rest.is_empty() && !rest.iter().any(|(_, s)| s.top) {
            let s = Stack { id: id.to_string(), top: true };
            sidecar::set_stack(&rest[0].0, Some(s.clone()))?;
            changed.push((rest[0].0.clone(), Some(s)));
        }
    }
    Ok(changed)
}

/// Makes `top` the top image of its stack (`members`: all files of that stack).
pub fn set_top(top: &Path, members: &[(PathBuf, Stack)]) -> Result<Vec<(PathBuf, Option<Stack>)>> {
    let top = crate::variant::file_of(top);
    let Some(id) = members.iter().find(|(p, _)| *p == top).map(|(_, s)| s.id.clone()) else { return Ok(Vec::new()) };
    let mut changed = Vec::new();
    for (p, s) in members.iter().filter(|(_, s)| s.id == id) {
        let want = *p == top;
        if s.top != want {
            let s = Stack { id: id.clone(), top: want };
            sidecar::set_stack(p, Some(s.clone()))?;
            changed.push((p.clone(), Some(s)));
        }
    }
    Ok(changed)
}

/// Capture time ("2024:05:01 12:30:05", also with '-' or 'T') in seconds since 1970 (no time zone).
pub fn parse_capture_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let digits: Vec<i64> = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).take(6).map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [y, mo, d, h, mi, sec] = <[i64; 6]>::try_from(digits.get(..6)?).ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let (y, m) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec)
}

/// Groups images taken in quick succession: sorted by time, a gap of more than `max_gap`
/// seconds starts a new group. Only groups of two or more are returned; images without a
/// time are left out.
pub fn group_by_time(items: &[(PathBuf, Option<i64>)], max_gap: i64) -> Vec<Vec<PathBuf>> {
    let mut timed: Vec<(&PathBuf, i64)> = items.iter().filter_map(|(p, t)| t.map(|t| (p, t))).collect();
    timed.sort_by_key(|(p, t)| (*t, (*p).clone()));
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    let mut last: Option<i64> = None;
    for (p, t) in timed {
        match last {
            Some(l) if t - l <= max_gap => groups.last_mut().expect("group").push(p.clone()),
            _ => groups.push(vec![p.clone()]),
        }
        last = Some(t);
    }
    groups.retain(|g| g.len() >= 2);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_times() {
        assert_eq!(parse_capture_time("1970:01:01 00:00:00"), Some(0));
        assert_eq!(parse_capture_time("2000:03:01 00:00:01"), Some(951_868_801));
        assert_eq!(parse_capture_time("2024-05-01T12:30:05"), parse_capture_time("2024:05:01 12:30:05"));
        assert_eq!(parse_capture_time("kaputt"), None);
    }

    #[test]
    fn groups_bursts() {
        let p = |n: &str| PathBuf::from(n);
        let items = vec![
            (p("a"), Some(100)),
            (p("c"), Some(102)),
            (p("b"), Some(101)),
            (p("d"), Some(110)),
            (p("e"), Some(200)),
            (p("f"), Some(201)),
            (p("x"), None),
        ];
        assert_eq!(group_by_time(&items, 2), vec![vec![p("a"), p("b"), p("c")], vec![p("e"), p("f")]]);
        assert_eq!(group_by_time(&items, 10).len(), 2);
        assert_eq!(group_by_time(&items, 10)[0].len(), 4);
    }

    #[test]
    fn create_remove_and_top() {
        let dir = std::env::temp_dir().join(format!("sk-stack-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let [a, b, c] = ["a.ARW", "b.ARW", "c.ARW"].map(|n| dir.join(n));
        let made = create(&[a.clone(), b.clone(), c.clone(), crate::variant::path_for(&a, 2)], Some(&b)).unwrap();
        assert_eq!(made.len(), 3);
        let members: Vec<(PathBuf, Stack)> = made.into_iter().map(|(p, s)| (p, s.unwrap())).collect();
        assert!(sidecar::load(&b).stack.unwrap().top);
        assert!(!sidecar::load(&a).stack.unwrap().top);
        // Variants follow their file, never as top.
        assert_eq!(sidecar::load(&crate::variant::path_for(&b, 2)).stack.map(|s| s.top), Some(false));
        // Ordinary saves keep the membership.
        sidecar::save(&a, &sidecar::Sidecar { rating: 3, ..Default::default() }).unwrap();
        assert!(sidecar::load(&a).stack.is_some());
        // Removing the top makes another one the top.
        let changed = remove(&[b.clone()], &members).unwrap();
        assert!(changed.iter().any(|(p, s)| *p == a && s.as_ref().is_some_and(|s| s.top)));
        let members: Vec<(PathBuf, Stack)> = [&a, &c].iter().map(|p| ((*p).clone(), sidecar::load(p).stack.unwrap())).collect();
        set_top(&c, &members).unwrap();
        assert!(sidecar::load(&c).stack.unwrap().top && !sidecar::load(&a).stack.unwrap().top);
        // A stack of one is no stack.
        let members: Vec<(PathBuf, Stack)> = [&a, &c].iter().map(|p| ((*p).clone(), sidecar::load(p).stack.unwrap())).collect();
        remove(&[a.clone()], &members).unwrap();
        assert!(sidecar::load(&c).stack.is_none());
        std::fs::remove_dir_all(dir).ok();
    }
}
