use std::{path::PathBuf, time::Duration, time::SystemTime};

use deltaflens_core::Settings;
use gpui::{Context, SharedString, Task};

/// How often the settings file is checked for changes made by hand.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// The settings of the application, read from the settings file and written
/// back when changed in the settings tab. Changes made to the file by hand are
/// read again. Observe it to follow the settings.
pub struct SettingsStore {
    settings: Settings,
    /// Why settings could not be read or written, if they could not.
    errors: Vec<SharedString>,
    /// `None` if the configuration directory is unknown, so that settings are
    /// neither read nor written.
    path: Option<PathBuf>,
    /// The modification time of the file when it was last read or written.
    modified: Option<SystemTime>,
    _watch: Task<()>,
}

impl SettingsStore {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let path = Settings::path();
        let watch = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_INTERVAL).await;
                if this.update(cx, Self::reload_if_changed).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            settings: Settings::default(),
            errors: Vec::new(),
            path,
            modified: None,
            _watch: watch,
        };
        this.load();
        this
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn errors(&self) -> &[SharedString] {
        &self.errors
    }

    /// Changes the settings and writes them to the settings file.
    pub fn update(&mut self, change: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        change(&mut settings);
        if settings == self.settings {
            return;
        }
        self.settings = settings;
        if let Some(path) = &self.path {
            match self.settings.save(path) {
                Ok(()) => {
                    self.modified = modified(path);
                    // Mistakes in other settings of the file are kept by the
                    // save, so they are still there.
                    let (_, errors) = Settings::load(path);
                    self.errors = errors.into_iter().map(SharedString::from).collect();
                }
                Err(error) => {
                    let message = format!("Failed to save the settings: {error:#}");
                    tracing::error!("{message}");
                    self.errors = vec![message.into()];
                }
            }
        }
        cx.notify();
    }

    fn load(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        self.modified = modified(path);
        let (settings, errors) = Settings::load(path);
        for error in &errors {
            tracing::warn!("{}: {error}", path.display());
        }
        self.settings = settings;
        self.errors = errors.into_iter().map(SharedString::from).collect();
    }

    fn reload_if_changed(&mut self, cx: &mut Context<Self>) {
        let Some(path) = &self.path else {
            return;
        };
        if modified(path) == self.modified {
            return;
        }
        let previous = (self.settings.clone(), self.errors.clone());
        self.load();
        if (self.settings.clone(), self.errors.clone()) != previous {
            cx.notify();
        }
    }
}

/// The modification time of `path`, or `None` if there is no such file.
fn modified(path: &std::path::Path) -> Option<SystemTime> {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
}
