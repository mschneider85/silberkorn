//! Undo beyond the open image: changes to several images at once (paste, styles, auto
//! adjustment, look matching …) and ratings, labels and keywords. Each such step and each edit
//! of the open image has a sequence number, so that Ctrl+Z always takes back the latest one.

use std::path::PathBuf;

use sk_core::params::Adjustments;
use sk_core::sidecar::ColorLabel;

use crate::app::App;

/// The most steps kept beyond the open image.
const LIMIT: usize = 50;

/// Rating, label and keywords of an image.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub rating: u8,
    pub color_label: ColorLabel,
    pub keywords: Vec<String>,
}

/// What to restore: the state of each image before the change (for undo) or after it (redo).
#[derive(Clone, Debug)]
pub enum Step {
    Adjustments(Vec<(PathBuf, Adjustments)>),
    Meta(Vec<(PathBuf, Meta)>),
}

impl Step {
    fn len(&self) -> usize {
        match self {
            Step::Adjustments(v) => v.len(),
            Step::Meta(v) => v.len(),
        }
    }

    fn describe(&self) -> String {
        let what = match self {
            Step::Adjustments(_) => "Anpassungen",
            Step::Meta(_) => "Bewertung, Label, Keywords",
        };
        format!("{what} ({})", crate::widgets::image_count(self.len()))
    }

    /// Adds the images of `other` this step doesn't hold yet (it keeps their earlier state).
    fn merge(&mut self, other: Step) {
        match (self, other) {
            (Step::Adjustments(a), Step::Adjustments(b)) => {
                for (p, v) in b {
                    if !a.iter().any(|(q, _)| *q == p) {
                        a.push((p, v));
                    }
                }
            }
            (Step::Meta(a), Step::Meta(b)) => {
                for (p, v) in b {
                    if !a.iter().any(|(q, _)| *q == p) {
                        a.push((p, v));
                    }
                }
            }
            _ => {}
        }
    }
}

impl App {
    /// Records a change to several images or to metadata as one undo step. While an auto
    /// adjustment or look match runs, its results (they arrive one image at a time) join the
    /// step of its first result.
    pub(crate) fn record_step(&mut self, step: Step) {
        if step.len() == 0 {
            return;
        }
        let batch = self.auto_progress.is_some() || self.look_progress.is_some();
        if batch
            && matches!(step, Step::Adjustments(_))
            && let Some(seq) = self.batch_step
            && let Some((_, s)) = self.extra_undo.iter_mut().find(|(n, _)| *n == seq)
        {
            s.merge(step);
            return;
        }
        self.next_seq += 1;
        let seq = self.next_seq;
        if batch && matches!(step, Step::Adjustments(_)) {
            self.batch_step = Some(seq);
        }
        self.extra_undo.push((seq, step));
        if self.extra_undo.len() > LIMIT {
            self.extra_undo.remove(0);
        }
        // A new change: what was undone can't be redone any more.
        self.extra_redo.clear();
        self.redo.clear();
        self.redo_seq.clear();
    }

    /// Ctrl+Z: takes back the latest step, an edit of the open image or a step beyond it.
    pub(crate) fn undo(&mut self) {
        self.commit_and_save();
        let image = self.undo_seq.last().copied();
        let extra = self.extra_undo.last().map(|(n, _)| *n);
        if extra > image {
            let (seq, step) = self.extra_undo.pop().expect("checked");
            self.status = format!("Rückgängig: {}", step.describe());
            let inverse = self.apply_step(step);
            self.extra_redo.push((seq, inverse));
        } else {
            self.undo_image();
        }
    }

    /// Ctrl+Y: redoes the step undone last.
    pub(crate) fn redo(&mut self) {
        self.commit_and_save();
        // Undone last = the lowest number of what can be redone.
        let image = self.redo_seq.last().copied();
        let extra = self.extra_redo.last().map(|(n, _)| *n);
        let take_extra = match (image, extra) {
            (Some(i), Some(e)) => e < i,
            (None, Some(_)) => true,
            _ => false,
        };
        if take_extra {
            let (seq, step) = self.extra_redo.pop().expect("checked");
            self.status = format!("Wiederholt: {}", step.describe());
            let inverse = self.apply_step(step);
            self.extra_undo.push((seq, inverse));
        } else {
            self.redo_image();
        }
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty() || !self.extra_redo.is_empty()
    }

    /// Restores what `step` holds and returns the step that restores the state before.
    fn apply_step(&mut self, step: Step) -> Step {
        match step {
            Step::Adjustments(list) => {
                // The strength sliders of an auto adjustment or look match refer to it.
                self.auto_mix = None;
                self.look_mix = None;
                let mut inverse = Vec::new();
                for (path, adj) in list {
                    if let Some(prev) = self.write_adjustments(&path, adj) {
                        inverse.push((path, prev));
                    }
                }
                Step::Adjustments(inverse)
            }
            Step::Meta(list) => {
                let mut inverse = Vec::new();
                for (path, m) in list {
                    if let Some(now) = self.meta_of(&path) {
                        inverse.push((path.clone(), now));
                    }
                    self.update_meta_unrecorded(std::slice::from_ref(&path), |sc| {
                        sc.rating = m.rating;
                        sc.color_label = m.color_label;
                        sc.keywords = m.keywords.clone();
                    });
                }
                Step::Meta(inverse)
            }
        }
    }

    /// Rating, label and keywords of `path` as they are now.
    pub(crate) fn meta_of(&self, path: &PathBuf) -> Option<Meta> {
        if self.current_path().as_ref() == Some(path) {
            let cur = self.current.as_ref()?;
            return Some(Meta { rating: cur.rating, color_label: cur.color_label, keywords: cur.keywords.clone() });
        }
        let sc = self.sidecar_for_update(path).ok()?;
        Some(Meta { rating: sc.rating, color_label: sc.color_label, keywords: sc.keywords })
    }
}
