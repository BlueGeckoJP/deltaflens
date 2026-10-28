use std::{
    collections::{HashSet, VecDeque},
    num::NonZero,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use deltaflens_core::{LruCache, thumbnail};
use eyre::Result;
use gpui::{Context, RenderImage, prelude::*};

use crate::render_image::to_render_image;

/// How many bytes of thumbnails to keep. A 256 x 256 thumbnail takes 256 KiB.
const CACHE_BYTES: usize = 256 * 1024 * 1024;
/// How many requests to remember. Older ones are forgotten, so that scrolling
/// fast through a large folder does not queue every image in it.
const MAX_PENDING: usize = 1024;
/// At most this many thumbnails are made at once. Each one decodes the whole
/// image first, which is about 33 MB for 4K.
const MAX_PARALLEL: usize = 8;

/// Identifies a thumbnail.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbnailKey {
    pub path: PathBuf,
    /// Makes a changed file get a new thumbnail.
    pub modified: Option<SystemTime>,
    /// The longest side in device pixels.
    pub size: u32,
}

/// Makes thumbnails in the background and keeps the recently used ones in
/// memory. Shared by the explorers of a window.
pub struct Thumbnails {
    cache: LruCache<ThumbnailKey, Arc<RenderImage>>,
    /// Requested but not started yet, the latest last.
    pending: VecDeque<ThumbnailKey>,
    /// The same keys as `pending`, for lookups.
    pending_set: HashSet<ThumbnailKey>,
    in_progress: HashSet<ThumbnailKey>,
    /// Not retried, as the image cannot be decoded.
    failed: HashSet<ThumbnailKey>,
    parallel: usize,
}

impl Thumbnails {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| {
            for (_, image) in this.cache.drain() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        // Leave some threads for the viewers and the UI.
        let threads = std::thread::available_parallelism().map_or(2, NonZero::get);
        Self {
            cache: LruCache::new(CACHE_BYTES),
            pending: VecDeque::new(),
            pending_set: HashSet::new(),
            in_progress: HashSet::new(),
            failed: HashSet::new(),
            parallel: (threads / 2).clamp(1, MAX_PARALLEL),
        }
    }

    /// Returns the thumbnail if it is ready, or else requests it. The latest
    /// requests are made first, as they are for what is on screen now.
    pub fn get(&mut self, key: &ThumbnailKey, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        if let Some(image) = self.cache.get(key) {
            return Some(image.clone());
        }
        let known = self.pending_set.contains(key)
            || self.in_progress.contains(key)
            || self.failed.contains(key);
        if !known {
            self.pending.push_back(key.clone());
            self.pending_set.insert(key.clone());
            if self.pending.len() > MAX_PENDING
                && let Some(oldest) = self.pending.pop_front()
            {
                self.pending_set.remove(&oldest);
            }
            self.start_pending(cx);
        }
        None
    }

    /// Forgets the requests for images outside `dir`, which no explorer on
    /// screen shows any longer.
    pub fn keep_requests_in(&mut self, dir: &Path) {
        self.pending.retain(|key| key.path.parent() == Some(dir));
        self.pending_set = self.pending.iter().cloned().collect();
    }

    fn start_pending(&mut self, cx: &mut Context<Self>) {
        while self.in_progress.len() < self.parallel {
            let Some(key) = self.pending.pop_back() else {
                break;
            };
            self.pending_set.remove(&key);
            self.in_progress.insert(key.clone());
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_spawn({
                        let (path, size) = (key.path.clone(), key.size);
                        async move { thumbnail(&path, size).and_then(to_render_image) }
                    })
                    .await;
                // Fails only if the thumbnails are gone.
                let _ = this.update(cx, |this, cx| this.finish(key, result, cx));
            })
            .detach();
        }
    }

    fn finish(
        &mut self,
        key: ThumbnailKey,
        result: Result<Arc<RenderImage>>,
        cx: &mut Context<Self>,
    ) {
        self.in_progress.remove(&key);
        match result {
            Ok(image) => {
                let bytes = image.as_bytes(0).map_or(0, <[u8]>::len);
                for (_, evicted) in self.cache.insert(key, image, bytes) {
                    cx.drop_image(evicted, None);
                }
                cx.notify();
            }
            Err(error) => {
                tracing::warn!("{error:#}");
                self.failed.insert(key);
            }
        }
        self.start_pending(cx);
    }
}
