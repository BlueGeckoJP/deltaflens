//! Settings, kept in a TOML file that the user may also edit by hand.

use std::{
    fs,
    io::ErrorKind,
    ops::RangeInclusive,
    path::{Path, PathBuf},
};

use eyre::{Result, WrapErr, eyre};
use toml_edit::{DocumentMut, Item, Value};

use crate::platform;

/// The sizes of thumbnails that can be set, in logical pixels.
pub const THUMBNAIL_SIZES: RangeInclusive<u32> = 64..=512;
/// The memory for decoded images that can be set, in MiB.
pub const IMAGE_CACHE_MIBS: RangeInclusive<u32> = 128..=65_536;

const THUMBNAIL_SIZE: &str = "thumbnail_size";
const SHOW_HIDDEN_FILES: &str = "show_hidden_files";
const IMAGE_CACHE_MIB: &str = "image_cache_mib";
const OPEN_COMMAND: &str = "open_command";

/// The settings of the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The longest side of thumbnails in the explorer, in logical pixels.
    pub thumbnail_size: u32,
    /// Whether the explorer and moving between images include hidden files.
    pub show_hidden_files: bool,
    /// The memory for decoded images, in MiB.
    pub image_cache_mib: u32,
    /// The command that opens a file with another application, where
    /// `<path>` stands for the file.
    pub open_command: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            thumbnail_size: 128,
            show_hidden_files: false,
            image_cache_mib: 1024,
            open_command: platform::DEFAULT_OPEN_COMMAND.to_owned(),
        }
    }
}

impl Settings {
    /// The settings file: `config.toml` in the configuration directory of the
    /// application.
    #[must_use]
    pub fn path() -> Option<PathBuf> {
        Some(platform::config_dir()?.join("deltaflens/config.toml"))
    }

    /// Reads the settings from `path`.
    ///
    /// Without the file, the defaults are used. A setting that cannot be read
    /// keeps its default, and why is told in the returned messages, so that a
    /// mistake in the file does not keep the application from starting.
    #[must_use]
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(error) => (
                Self::default(),
                vec![format!("Failed to read {}: {error}", path.display())],
            ),
        }
    }

    /// Reads the settings from the text of a settings file, as [`Self::load`]
    /// does.
    #[must_use]
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut settings = Self::default();
        let mut errors = Vec::new();
        let document = match text.parse::<DocumentMut>() {
            Ok(document) => document,
            Err(error) => {
                errors.push(format!("The settings file is not valid TOML: {error}"));
                return (settings, errors);
            }
        };
        for (key, item) in document.iter() {
            let value = item.as_value();
            let applied =
                match key {
                    THUMBNAIL_SIZE => read_number(value, THUMBNAIL_SIZES)
                        .map(|size| settings.thumbnail_size = size),
                    SHOW_HIDDEN_FILES => value
                        .and_then(Value::as_bool)
                        .map(|show| settings.show_hidden_files = show)
                        .ok_or_else(|| "true or false".to_owned()),
                    IMAGE_CACHE_MIB => read_number(value, IMAGE_CACHE_MIBS)
                        .map(|mib| settings.image_cache_mib = mib),
                    OPEN_COMMAND => value
                        .and_then(Value::as_str)
                        .filter(|command| !command.trim().is_empty())
                        .map(|command| command.clone_into(&mut settings.open_command))
                        .ok_or_else(|| "a command, such as \"xdg-open <path>\"".to_owned()),
                    _ => {
                        errors.push(format!("Unknown setting \"{key}\"."));
                        continue;
                    }
                };
            if let Err(expected) = applied {
                errors.push(format!(
                    "\"{key}\" must be {expected}. The default is used."
                ));
            }
        }
        (settings, errors)
    }

    /// The text of a settings file with these settings, made from `text` by
    /// changing only the settings that differ from it. Comments and the
    /// settings it does not know are kept.
    ///
    /// # Errors
    ///
    /// Returns an error if `text` is not valid TOML.
    pub fn merge_into(&self, text: &str) -> Result<String> {
        let mut document = text
            .parse::<DocumentMut>()
            .wrap_err("the settings file is not valid TOML")?;
        let (current, _) = Self::parse(text);
        if current.thumbnail_size != self.thumbnail_size {
            set(
                &mut document,
                THUMBNAIL_SIZE,
                i64::from(self.thumbnail_size),
            );
        }
        if current.show_hidden_files != self.show_hidden_files {
            set(&mut document, SHOW_HIDDEN_FILES, self.show_hidden_files);
        }
        if current.image_cache_mib != self.image_cache_mib {
            set(
                &mut document,
                IMAGE_CACHE_MIB,
                i64::from(self.image_cache_mib),
            );
        }
        if current.open_command != self.open_command {
            set(&mut document, OPEN_COMMAND, self.open_command.as_str());
        }
        Ok(document.to_string())
    }

    /// Writes these settings to `path` as [`Self::merge_into`] does, creating
    /// the file and its folder if there are none. The file is replaced at
    /// once, so that a reader never sees it half written.
    ///
    /// # Errors
    ///
    /// Returns an error if the file is not valid TOML or cannot be written.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(error).wrap_err_with(|| format!("failed to read {}", path.display()));
            }
        };
        let text = self.merge_into(&text)?;
        let dir = path
            .parent()
            .ok_or_else(|| eyre!("{} has no folder", path.display()))?;
        fs::create_dir_all(dir)?;
        let temporary = dir.join(".config.toml.new");
        fs::write(&temporary, text)?;
        fs::rename(&temporary, path).wrap_err_with(|| format!("failed to write {}", path.display()))
    }
}

/// Sets `key` to `value`, keeping the spaces and the comment around the old
/// value.
fn set(document: &mut DocumentMut, key: &str, value: impl Into<Value>) {
    let mut value = value.into();
    if let Some(old) = document.get(key).and_then(Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    document[key] = Item::Value(value);
}

/// Reads a whole number in `range`, or tells what was expected.
fn read_number(value: Option<&Value>, range: RangeInclusive<u32>) -> Result<u32, String> {
    value
        .and_then(Value::as_integer)
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| range.contains(number))
        .ok_or_else(|| format!("a whole number from {} to {}", range.start(), range.end()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn missing_file_gives_defaults_without_errors() {
        let dir = TempDir::with_files("settings-missing", &[]);
        let (settings, errors) = Settings::load(&dir.path().join("config.toml"));
        assert_eq!(settings, Settings::default());
        assert_eq!(errors, Vec::<String>::new());
    }

    #[test]
    fn settings_are_read() {
        let (settings, errors) = Settings::parse(
            "thumbnail_size = 256\n\
             show_hidden_files = true\n\
             image_cache_mib = 2048\n\
             open_command = \"gimp <path>\"\n",
        );
        assert_eq!(
            settings,
            Settings {
                thumbnail_size: 256,
                show_hidden_files: true,
                image_cache_mib: 2048,
                open_command: "gimp <path>".to_owned(),
            }
        );
        assert_eq!(errors, Vec::<String>::new());
    }

    #[test]
    fn broken_file_gives_defaults_and_an_error() {
        let (settings, errors) = Settings::parse("thumbnail_size = \n[");
        assert_eq!(settings, Settings::default());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("not valid TOML"), "{errors:?}");
    }

    #[test]
    fn bad_value_keeps_its_default_only() {
        let (settings, errors) = Settings::parse(
            "thumbnail_size = 9999\n\
             show_hidden_files = \"yes\"\n\
             image_cache_mib = 512\n\
             open_command = \"  \"\n",
        );
        assert_eq!(
            settings,
            Settings {
                image_cache_mib: 512,
                ..Settings::default()
            }
        );
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].contains("\"thumbnail_size\" must be a whole number from 64 to 512"));
    }

    #[test]
    fn unknown_setting_is_an_error_but_not_fatal() {
        let (settings, errors) = Settings::parse("colour = \"red\"\nthumbnail_size = 96\n");
        assert_eq!(settings.thumbnail_size, 96);
        assert_eq!(errors, ["Unknown setting \"colour\"."]);
    }

    #[test]
    fn merge_keeps_comments_and_unknown_settings() {
        let text = "# My settings\n\
                    thumbnail_size = 96 # small\n\
                    \n\
                    # Kept as is.\n\
                    colour = \"red\"\n";
        let settings = Settings {
            thumbnail_size: 200,
            show_hidden_files: true,
            ..Settings::default()
        };
        assert_eq!(
            settings.merge_into(text).unwrap(),
            "# My settings\n\
             thumbnail_size = 200 # small\n\
             \n\
             # Kept as is.\n\
             colour = \"red\"\n\
             show_hidden_files = true\n"
        );
    }

    #[test]
    fn merge_into_empty_file_writes_only_changes() {
        let settings = Settings {
            image_cache_mib: 4096,
            ..Settings::default()
        };
        assert_eq!(settings.merge_into("").unwrap(), "image_cache_mib = 4096\n");
    }

    #[test]
    fn save_creates_the_folder_and_round_trips() {
        let dir = TempDir::with_files("settings-save", &[]);
        let path = dir.path().join("deltaflens/config.toml");
        let settings = Settings {
            open_command: "viewer --file=<path>".to_owned(),
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), (settings, Vec::new()));
    }
}
