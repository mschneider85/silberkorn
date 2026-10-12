//! Requests for browser thumbnails. The newest requests come first: when scrolling quickly
//! through a large folder, the cells now on screen are made before those scrolled past, which
//! are still made afterwards. Requests of one frame keep their order (top to bottom).

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// A pending thumbnail: (frame it was requested in, image, session root).
type Request = (u64, PathBuf, Option<PathBuf>);

#[derive(Default)]
struct State {
    pending: Vec<Request>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The app's end of the queue; dropping it ends the workers' [`ThumbSource::next`].
pub struct ThumbQueue(Arc<Shared>);

/// The workers' end of the queue.
#[derive(Clone)]
pub struct ThumbSource(Arc<Shared>);

pub fn queue() -> (ThumbQueue, ThumbSource) {
    let shared = Arc::new(Shared::default());
    (ThumbQueue(shared.clone()), ThumbSource(shared))
}

impl ThumbQueue {
    /// Asks for a thumbnail; asked again, an image moves up to the newer frame.
    pub fn request(&self, frame: u64, path: PathBuf, session: Option<PathBuf>) {
        let mut st = self.0.lock();
        st.pending.retain(|(_, p, _)| *p != path);
        st.pending.push((frame, path, session));
        drop(st);
        self.0.ready.notify_one();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.lock().pending.len()
    }
}

impl Drop for ThumbQueue {
    fn drop(&mut self) {
        self.0.lock().closed = true;
        self.0.ready.notify_all();
    }
}

impl ThumbSource {
    /// The next thumbnail to make: the oldest request of the newest frame. Waits for one;
    /// `None` once the queue is gone.
    pub fn next(&self) -> Option<(PathBuf, Option<PathBuf>)> {
        let mut st = self.0.lock();
        loop {
            if st.closed {
                return None;
            }
            let newest = st.pending.iter().map(|r| r.0).max();
            if let Some(frame) = newest
                && let Some(i) = st.pending.iter().position(|r| r.0 == frame)
            {
                let (_, path, session) = st.pending.remove(i);
                return Some((path, session));
            }
            st = self.0.ready.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_frame_first_in_request_order() {
        let (q, src) = queue();
        let p = |s: &str| PathBuf::from(s);
        for name in ["a", "b", "c"] {
            q.request(1, p(name), None);
        }
        // Scrolled on: the new cells come first, top to bottom, then the older ones.
        q.request(2, p("x"), None);
        q.request(2, p("y"), None);
        // Visible again: moves up without a duplicate.
        q.request(2, p("a"), None);
        assert_eq!(q.len(), 5);
        let order: Vec<PathBuf> = (0..5).map(|_| src.next().unwrap().0).collect();
        assert_eq!(order, ["x", "y", "a", "b", "c"].map(p));
    }

    #[test]
    fn dropping_the_queue_ends_the_workers() {
        let (q, src) = queue();
        let worker = std::thread::spawn(move || src.next());
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(q);
        assert_eq!(worker.join().unwrap(), None);
    }
}
