use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};

use deltaflens_core::{DecodedImage, LruCache, decode, downscale};
use eyre::{Result, eyre};
use gpui::{AppContext as _, Context, EntityId, EventEmitter, RenderImage, SharedString, Task};

use crate::render_image::to_render_image;

/// How many bytes of decoded images to keep in memory. A 4K image takes about
/// 33 MB.
const CACHE_BYTES: usize = 1024 * 1024 * 1024;
/// At most this many images are decoded ahead of time at once, so that the
/// images asked for to be shown now are not kept waiting.
const MAX_PARALLEL_PREFETCH: usize = 2;

/// Identifies a decoded image, or a downscaled version of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageKey {
    pub path: PathBuf,
    /// Makes a changed file get decoded again.
    pub modified: Option<SystemTime>,
    /// The size in pixels the image is downscaled to, or `None` for the image
    /// as decoded.
    pub downscaled: Option<(u32, u32)>,
}

impl ImageKey {
    pub fn new(path: PathBuf) -> Self {
        let modified = path
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok();
        Self {
            path,
            modified,
            downscaled: None,
        }
    }

    /// The key of the image downscaled to `size`.
    #[must_use]
    pub fn downscaled(&self, size: (u32, u32)) -> Self {
        Self {
            downscaled: Some(size),
            ..self.original()
        }
    }

    /// The key of the image as decoded.
    #[must_use]
    pub fn original(&self) -> Self {
        Self {
            path: self.path.clone(),
            modified: self.modified,
            downscaled: None,
        }
    }
}

/// What a viewer asks for.
#[derive(Debug, Default)]
pub struct Request {
    /// The images it shows, which must not be evicted. They may be of the
    /// previous image while `current` is decoded.
    pub shown: Vec<ImageKey>,
    /// The images to show now: the image, and a downscaled version of it once
    /// it is decoded. They are not evicted either, so that they can be shown
    /// once made.
    pub current: Vec<ImageKey>,
    /// The images it may show next, the most likely first.
    pub prefetch: Vec<ImageKey>,
}

pub enum ImagesEvent {
    /// The image of the key is decoded and can be taken with [`Images::get`].
    Loaded(ImageKey),
    /// The image of the key could not be decoded. [`Images::error`] tells why.
    Failed(ImageKey),
}

/// Decodes images in the background for the viewers of a window and keeps the
/// recently used ones in memory, so that moving between images does not wait
/// for them to be decoded again.
///
/// The viewers do not keep images of their own: the images on screen are
/// pinned in the cache, and a viewer whose tab is hidden asks for nothing.
pub struct Images {
    cache: LruCache<ImageKey, Arc<RenderImage>>,
    /// By the viewer that asks.
    requests: HashMap<EntityId, Request>,
    /// Dropping a task cancels the decoding, unless it has started.
    in_progress: HashMap<ImageKey, Task<()>>,
    /// Not retried while still asked for.
    failed: HashMap<ImageKey, SharedString>,
    /// Decoded ahead of time, and not again while still asked for ahead of
    /// time, even if evicted. Otherwise images that do not all fit in the
    /// cache would evict one another and be decoded over and over.
    prefetched: HashSet<ImageKey>,
    /// The images the viewers show, whose textures are on the GPU.
    shown: HashSet<ImageKey>,
}

impl EventEmitter<ImagesEvent> for Images {}

impl Images {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| {
            for (_, image) in this.cache.drain() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        Self {
            cache: LruCache::new(CACHE_BYTES),
            requests: HashMap::new(),
            in_progress: HashMap::new(),
            failed: HashMap::new(),
            prefetched: HashSet::new(),
            shown: HashSet::new(),
        }
    }

    /// Returns the image if it is decoded, and marks it as recently used.
    pub fn get(&mut self, key: &ImageKey) -> Option<Arc<RenderImage>> {
        self.cache.get(key).cloned()
    }

    #[must_use]
    pub fn contains(&self, key: &ImageKey) -> bool {
        self.cache.contains(key)
    }

    /// Why the image could not be decoded, if it could not.
    #[must_use]
    pub fn error(&self, key: &ImageKey) -> Option<SharedString> {
        self.failed.get(key).cloned()
    }

    /// Replaces what `viewer` asks for. Decoding images that nobody asks for
    /// any longer is canceled.
    pub fn request(&mut self, viewer: EntityId, request: Request, cx: &mut Context<Self>) {
        self.requests.insert(viewer, request);
        self.schedule(cx);
    }

    /// Forgets what `viewer` asks for, as it is gone.
    pub fn forget(&mut self, viewer: EntityId, cx: &mut Context<Self>) {
        self.requests.remove(&viewer);
        self.schedule(cx);
    }

    /// Pins the images shown or to be shown, cancels the decoding of images
    /// nobody asks for, and starts decoding the ones asked for: all images to
    /// be shown now, then images to be shown next, a few at a time.
    fn schedule(&mut self, cx: &mut Context<Self>) {
        let shown: HashSet<ImageKey> = self
            .requests
            .values()
            .flat_map(|request| request.shown.iter().cloned())
            .collect();
        // The images no longer shown stay in memory, but their textures are
        // freed, as they would double the memory used for them. Showing them
        // again uploads them again.
        let hidden: Vec<Arc<RenderImage>> = self
            .shown
            .difference(&shown)
            .filter_map(|key| self.cache.peek(key).cloned())
            .collect();
        Self::release(hidden, cx);
        let pinned = self
            .requests
            .values()
            .flat_map(|request| request.current.iter().cloned())
            .chain(shown.iter().cloned())
            .collect();
        self.shown = shown;
        let evicted = self.cache.set_pinned(pinned);
        Self::release(evicted.into_iter().map(|(_, image)| image).collect(), cx);

        let current: Vec<ImageKey> = self
            .requests
            .values()
            .flat_map(|request| request.current.iter().cloned())
            .collect();
        let prefetch: Vec<ImageKey> = self
            .requests
            .values()
            .flat_map(|request| request.prefetch.iter().cloned())
            .filter(|key| !current.contains(key))
            .collect();
        let wanted = |key: &ImageKey| current.contains(key) || prefetch.contains(key);
        self.in_progress.retain(|key, _| wanted(key));
        self.failed.retain(|key, _| wanted(key));
        self.prefetched.retain(|key| prefetch.contains(key));

        for key in &current {
            self.start(key, cx);
        }
        for key in &prefetch {
            if self.prefetched.contains(key) {
                continue;
            }
            let prefetching = self
                .in_progress
                .keys()
                .filter(|key| !current.contains(key))
                .count();
            if prefetching >= MAX_PARALLEL_PREFETCH {
                break;
            }
            self.start(key, cx);
        }
    }

    /// Starts decoding the image of `key`, or downscaling it once it is
    /// decoded, unless that is done, being done, or failed.
    fn start(&mut self, key: &ImageKey, cx: &mut Context<Self>) {
        if self.cache.contains(key)
            || self.in_progress.contains_key(key)
            || self.failed.contains_key(key)
        {
            return;
        }
        let source = match key.downscaled {
            Some(_) => match self.cache.peek(&key.original()) {
                Some(image) => Some(image.clone()),
                // Started once the image is decoded.
                None => return,
            },
            None => None,
        };
        let task = cx.spawn({
            let key = key.clone();
            async move |this, cx| {
                let result = cx
                    .background_spawn({
                        let key = key.clone();
                        async move {
                            match (source, key.downscaled) {
                                (Some(source), Some(size)) => downscale_image(&source, size),
                                _ => decode(&key.path),
                            }
                            .and_then(to_render_image)
                        }
                    })
                    .await;
                // Fails only if the images are gone.
                let _ = this.update(cx, |this, cx| this.finish(key, result, cx));
            }
        });
        self.in_progress.insert(key.clone(), task);
    }

    fn finish(&mut self, key: ImageKey, result: Result<Arc<RenderImage>>, cx: &mut Context<Self>) {
        self.in_progress.remove(&key);
        match result {
            Ok(image) => {
                // Forgotten by `schedule` unless it is asked for ahead of time.
                self.prefetched.insert(key.clone());
                let bytes = image.as_bytes(0).map_or(0, <[u8]>::len);
                let evicted = self.cache.insert(key.clone(), image, bytes);
                Self::release(evicted.into_iter().map(|(_, image)| image).collect(), cx);
                cx.emit(ImagesEvent::Loaded(key));
            }
            Err(error) => {
                tracing::warn!("{error:#}");
                self.failed.insert(key.clone(), format!("{error:#}").into());
                cx.emit(ImagesEvent::Failed(key));
            }
        }
        self.schedule(cx);
    }

    /// Frees the textures of `images`, if they have any.
    fn release(images: Vec<Arc<RenderImage>>, cx: &mut Context<Self>) {
        if images.is_empty() {
            return;
        }
        // Wait until no window is being updated, as `drop_image` would miss it.
        cx.defer(move |cx| {
            for image in images {
                cx.drop_image(image, None);
            }
        });
    }
}

/// Downscales `image` to `size` with a Lanczos3 filter.
fn downscale_image(image: &RenderImage, size: (u32, u32)) -> Result<DecodedImage> {
    let source = image.size(0);
    let bytes = image
        .as_bytes(0)
        .ok_or_else(|| eyre!("the image has no pixels"))?;
    // Sizes of images are never negative.
    #[expect(clippy::cast_sign_loss)]
    let source = (source.width.0 as u32, source.height.0 as u32);
    downscale(bytes, source, size)
}
