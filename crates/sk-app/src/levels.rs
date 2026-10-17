//! The histogram of the levels tool: what goes into the levels (see
//! `sk_core::auto::levels_input_histogram`), measured in the background whenever the image or the
//! settings before the levels change; dragging the levels or the curves measures nothing.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use sk_core::image::LinearImage;
use sk_core::params::Adjustments;
use sk_core::pipeline::Histogram;
use sk_core::raw::RawMeta;

#[derive(Default)]
pub struct LevelsHistogram {
    /// Image and settings of the histogram shown or being measured.
    key: Option<(PathBuf, Adjustments)>,
    hist: Option<Histogram>,
    pending: Option<Receiver<Histogram>>,
}

impl LevelsHistogram {
    /// The histogram for the levels of the image `meta`/`preview` with `adj`: the last one
    /// measured, and a new measurement started if the settings changed since. Called only while
    /// the levels are open.
    pub(crate) fn get(&mut self, meta: &Arc<RawMeta>, preview: &Arc<LinearImage>, adj: &Adjustments, ctx: &egui::Context) -> Option<Histogram> {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(h) => (self.hist, self.pending) = (Some(h), None),
                Err(TryRecvError::Disconnected) => self.pending = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let key = (meta.path.clone(), sk_core::auto::levels_input_settings(adj));
        if self.pending.is_none() && self.key.as_ref() != Some(&key) {
            if self.key.as_ref().is_none_or(|(p, _)| *p != key.0) {
                // Not the histogram of another image meanwhile.
                self.hist = None;
            }
            let (meta, preview, settings) = (meta.clone(), preview.clone(), key.1.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(sk_core::auto::levels_input_histogram(&meta, &settings, &preview));
                ctx.request_repaint();
            });
            (self.key, self.pending) = (Some(key), Some(rx));
        }
        self.hist.clone()
    }
}
