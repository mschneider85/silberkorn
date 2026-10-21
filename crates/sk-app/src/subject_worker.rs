//! Motiv auswählen in the background. BiRefNet takes 10–30 s on the processor, so its masks are
//! computed on a thread of their own (Objekt, Himmel and the magic brush don't wait behind it; on
//! Windows the network also has a worker process of its own, see `sk_ai::worker::Lane`), kept per
//! image, and computed ahead, without notice, when an image stays in the Ebenen tab. A click
//! while that runs waits for it instead of computing again.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use sk_ai::MaskImage;

use crate::workers::Event;

/// Who waits for a mask.
pub struct Deliver {
    pub generation: u64,
    pub layer: u32,
}

pub struct SubjectJob {
    pub key: u64,
    pub guide: sk_ai::Guide,
    /// `None`: computed ahead, only kept.
    pub deliver: Option<Deliver>,
}

/// The exact masks of the last few images (shared with the mask thread, which answers from it).
#[derive(Clone, Default)]
pub struct SubjectCache(Arc<Mutex<Vec<(u64, MaskImage)>>>);

/// Masks kept (each about 1 MB).
const KEEP: usize = 8;

impl SubjectCache {
    pub fn get(&self, key: u64) -> Option<MaskImage> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key).map(|(_, m)| m.clone())
    }

    fn put(&self, key: u64, mask: MaskImage) {
        let mut c = self.0.lock().unwrap_or_else(|e| e.into_inner());
        c.retain(|(k, _)| *k != key);
        if c.len() >= KEEP {
            c.remove(0);
        }
        c.push((key, mask));
    }
}

/// An image's key: the subject doesn't change with the edits (the guide only has white balance
/// and exposure), only with another preview.
pub fn key(path: &Path, width: usize, height: usize) -> u64 {
    sk_core::cache::fnv1a(format!("{}|{width}x{height}", path.display()).as_bytes())
}

/// The thread computing exact subject masks.
pub fn spawn(rx: Receiver<SubjectJob>, cache: SubjectCache, ev_tx: std::sync::mpsc::Sender<Event>, ctx: egui::Context) {
    crate::guard::spawn("Motiv", move || {
        let mut net: Option<(sk_ai::subject::SubjectNet, u64)> = None;
        while let Ok(first) = rx.recv() {
            // Clicks first; of the images computed ahead only the newest (the others were left).
            let (mut jobs, ahead): (Vec<_>, Vec<_>) = std::iter::once(first).chain(rx.try_iter()).partition(|j| j.deliver.is_some());
            jobs.extend(ahead.into_iter().last());
            for job in jobs {
                let result = match cache.get(job.key) {
                    Some(m) => Ok(m),
                    None => crate::guard::catch(|| {
                        let generation = sk_ai::runtime::config_generation();
                        if net.as_ref().is_none_or(|(_, g)| *g != generation) && sk_ai::subject::available() {
                            net = sk_ai::subject::SubjectNet::load().map_err(|e| log::warn!("Motiv: {e:#}")).ok().map(|n| (n, generation));
                        }
                        let Some((n, _)) = &net else { return Err("MODEL_MISSING".to_string()) };
                        let m = n.mask(&job.guide).map_err(|e| format!("{e:#}"))?;
                        cache.put(job.key, m.clone());
                        Ok(m)
                    })
                    .and_then(|r| r),
                };
                let Some(d) = job.deliver else {
                    if let Err(e) = result {
                        log::warn!("Motiv vorab: {e}");
                    }
                    continue;
                };
                let _ = ev_tx.send(Event::AiMask { generation: d.generation, layer: d.layer, result });
                ctx.request_repaint();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_masks() {
        let cache = SubjectCache::default();
        let mask = |v: u8| MaskImage { width: 1, height: 1, data: vec![v] };
        for k in 0..KEEP as u64 + 2 {
            cache.put(k, mask(k as u8));
        }
        assert!(cache.get(0).is_none() && cache.get(1).is_none());
        assert_eq!(cache.get(KEEP as u64 + 1).unwrap().data, vec![KEEP as u8 + 1]);
        // The same image again replaces its mask.
        cache.put(5, mask(99));
        assert_eq!(cache.get(5).unwrap().data, vec![99]);
        assert_ne!(key(Path::new("a.arw"), 10, 10), key(Path::new("a.arw"), 20, 10));
    }
}
