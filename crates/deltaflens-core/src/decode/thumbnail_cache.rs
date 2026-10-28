//! The thumbnail cache that file managers such as Dolphin share, as the
//! freedesktop Thumbnail Managing Standard describes:
//! <https://specifications.freedesktop.org/thumbnail-spec/latest/>

use std::{
    fmt::Write as _,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{BufReader, BufWriter},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use eyre::{Result, WrapErr, bail};
use md5::{Digest, Md5};

use super::{DecodedImage, thumbnail};

/// The size directories, smallest first, with the longest side of their
/// thumbnails.
const SIZES: [(&str, u32); 4] = [
    ("normal", 128),
    ("large", 256),
    ("x-large", 512),
    ("xx-large", 1024),
];

const URI_KEY: &str = "Thumb::URI";
const MTIME_KEY: &str = "Thumb::MTime";
const SIZE_KEY: &str = "Thumb::Size";

/// A directory of thumbnails, normally `~/.cache/thumbnails`.
#[derive(Debug, Clone)]
pub struct ThumbnailCache {
    root: PathBuf,
}

/// What a thumbnail records about its image, to tell whether it is up to date.
#[derive(Debug, PartialEq, Eq)]
struct Source {
    uri: String,
    /// Seconds since the Unix epoch.
    mtime: u64,
    /// In bytes.
    size: u64,
}

impl ThumbnailCache {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `$XDG_CACHE_HOME/thumbnails`, or `~/.cache/thumbnails`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            // The XDG Base Directory Specification ignores relative paths.
            .filter(|dir| dir.is_absolute())
            .or_else(|| std::env::home_dir().map(|home| home.join(".cache")))?;
        Some(Self::new(cache.join("thumbnails")))
    }

    /// Returns a thumbnail of `path` that fits in a `size` square, from the
    /// cache if it is up to date, or else made with [`thumbnail`] and saved.
    ///
    /// Thumbnails come in the sizes of the standard, so the one returned may be
    /// larger than `size`, but not larger than 1024 pixels.
    ///
    /// # Errors
    ///
    /// Returns an error if the thumbnail can be neither read nor made. Failing
    /// to save it is only logged.
    pub fn get_or_create(&self, path: &Path, size: u32) -> Result<DecodedImage> {
        let path = std::path::absolute(path)?;
        let (dir, max_size) = size_dir(size);
        let source = match source(&path) {
            Ok(source) => source,
            Err(error) => {
                tracing::debug!("not caching {}: {error:#}", path.display());
                return thumbnail(&path, max_size);
            }
        };
        let cache_path = self
            .root
            .join(dir)
            .join(format!("{}.png", md5_hex(&source.uri)));
        match read_thumbnail(&cache_path, &source) {
            Ok(Some(image)) => return Ok(image),
            Ok(None) => {}
            Err(error) => tracing::debug!("{}: {error:#}", cache_path.display()),
        }

        let image = thumbnail(&path, max_size)?;
        // The standard says not to make thumbnails of thumbnails.
        if !path.starts_with(&self.root)
            && let Err(error) = save_thumbnail(&cache_path, &image, &source)
        {
            tracing::warn!(
                "failed to save the thumbnail of {}: {error:#}",
                path.display()
            );
        }
        Ok(image)
    }
}

/// The directory and the thumbnail size for a thumbnail of at least `size`.
fn size_dir(size: u32) -> (&'static str, u32) {
    SIZES
        .into_iter()
        .find(|&(_, max_size)| size <= max_size)
        .unwrap_or(SIZES[SIZES.len() - 1])
}

fn source(path: &Path) -> Result<Source> {
    let metadata = fs::metadata(path)?;
    Ok(Source {
        uri: file_uri(path),
        mtime: metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs(),
        size: metadata.len(),
    })
}

/// The `file://` URI of an absolute path, percent-encoded as
/// `g_filename_to_uri` does, so that the names of the thumbnails match other
/// applications'.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

fn md5_hex(text: &str) -> String {
    Md5::digest(text)
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Reads the thumbnail at `cache_path`. Returns `None` if there is none, or if
/// it is not of `source` as it is now.
fn read_thumbnail(cache_path: &Path, source: &Source) -> Result<Option<DecodedImage>> {
    let file = match File::open(cache_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let Some(buffer_size) = reader.output_buffer_size() else {
        bail!("the thumbnail is too large");
    };
    let mut samples = vec![0; buffer_size];
    let frame = reader.next_frame(&mut samples)?;
    // Text chunks may also follow the pixels.
    reader.finish()?;
    if !records(reader.info(), source) {
        return Ok(None);
    }

    samples.truncate(frame.buffer_size());
    let pixels = match frame.color_type {
        png::ColorType::Grayscale => samples.iter().flat_map(|&v| [v, v, v, u8::MAX]).collect(),
        png::ColorType::GrayscaleAlpha => samples
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[v, a]| [v, v, v, a])
            .collect(),
        png::ColorType::Rgb => samples
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [b, g, r, u8::MAX])
            .collect(),
        png::ColorType::Rgba => {
            for pixel in samples.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            samples
        }
        png::ColorType::Indexed => bail!("palette left after expansion"),
    };
    Ok(Some(DecodedImage {
        width: frame.width,
        height: frame.height,
        pixels,
    }))
}

/// Whether the text chunks of a thumbnail say that it is of `source`. The
/// URI and the modification time are required; the size is checked if given.
fn records(info: &png::Info, source: &Source) -> bool {
    let latin1 = info
        .uncompressed_latin1_text
        .iter()
        .map(|chunk| (chunk.keyword.as_str(), chunk.text.clone()));
    // KIO, which Dolphin uses, writes the URI compressed.
    let compressed = info
        .compressed_latin1_text
        .iter()
        .filter_map(|chunk| Some((chunk.keyword.as_str(), chunk.get_text().ok()?)));
    let utf8 = info
        .utf8_text
        .iter()
        .filter_map(|chunk| Some((chunk.keyword.as_str(), chunk.get_text().ok()?)));
    let (mut uri, mut mtime, mut size) = (None, None, None);
    for (keyword, text) in latin1.chain(compressed).chain(utf8) {
        match keyword {
            URI_KEY => uri = Some(text),
            MTIME_KEY => mtime = text.parse::<u64>().ok(),
            SIZE_KEY => size = text.parse::<u64>().ok(),
            _ => {}
        }
    }
    uri.as_deref() == Some(source.uri.as_str())
        && mtime == Some(source.mtime)
        && size.is_none_or(|size| size == source.size)
}

/// Writes the thumbnail to a temporary file and renames it into place, so that
/// other applications never read a partial one.
fn save_thumbnail(cache_path: &Path, image: &DecodedImage, source: &Source) -> Result<()> {
    let Some(dir) = cache_path.parent() else {
        bail!("no directory in {}", cache_path.display());
    };
    // The standard asks for the directories to be private to the user.
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let temp_path = cache_path.with_extension(format!("png.{}.tmp", std::process::id()));
    let result =
        write_png(&temp_path, image, source).and_then(|()| Ok(fs::rename(&temp_path, cache_path)?));
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.wrap_err_with(|| format!("failed to write {}", cache_path.display()))
}

fn write_png(path: &Path, image: &DecodedImage, source: &Source) -> Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.add_text_chunk(URI_KEY.to_owned(), source.uri.clone())?;
    encoder.add_text_chunk(MTIME_KEY.to_owned(), source.mtime.to_string())?;
    encoder.add_text_chunk(SIZE_KEY.to_owned(), source.size.to_string())?;
    encoder.add_text_chunk("Software".to_owned(), "deltaflens".to_owned())?;
    let mut writer = encoder.write_header()?;
    let mut rgba = image.pixels.clone();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    writer.write_image_data(&rgba)?;
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, time::Duration};

    use image::{Rgba, RgbaImage};

    use super::*;
    use crate::test_util::TempDir;

    const RED: [u8; 4] = [0, 0, 255, 255];
    const BLUE: [u8; 4] = [255, 0, 0, 255];

    /// Writes a 3x2 PNG of one color, given in RGBA.
    fn write_image(path: &Path, rgba: [u8; 4]) {
        RgbaImage::from_pixel(3, 2, Rgba(rgba)).save(path).unwrap();
    }

    fn set_mtime(path: &Path, secs: u64) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    fn cached_files(cache: &ThumbnailCache, dir: &str) -> Vec<PathBuf> {
        fs::read_dir(cache.root.join(dir)).map_or_else(
            |_| Vec::new(),
            |entries| entries.map(|entry| entry.unwrap().path()).collect(),
        )
    }

    #[test]
    fn md5_matches_the_example_of_the_standard() {
        assert_eq!(
            md5_hex("file:///home/jens/photos/me.png"),
            "c6ee772d9e49320e97ec29a7eb5b1697"
        );
    }

    #[test]
    fn file_uri_percent_encodes_like_glib() {
        assert_eq!(
            file_uri(Path::new("/home/a b/日本#1%?.png")),
            "file:///home/a%20b/%E6%97%A5%E6%9C%AC%231%25%3F.png"
        );
        assert_eq!(
            file_uri(Path::new("/a/(x)+y,z=1;@:~'!$&*.jxl")),
            "file:///a/(x)+y,z=1;@:~'!$&*.jxl"
        );
        assert_eq!(
            file_uri(Path::new(OsStr::from_bytes(b"/\xff.png"))),
            "file:///%FF.png"
        );
    }

    #[test]
    fn size_dir_picks_the_smallest_that_fits() {
        assert_eq!(size_dir(100), ("normal", 128));
        assert_eq!(size_dir(128), ("normal", 128));
        assert_eq!(size_dir(129), ("large", 256));
        assert_eq!(size_dir(256), ("large", 256));
        assert_eq!(size_dir(300), ("x-large", 512));
        assert_eq!(size_dir(5000), ("xx-large", 1024));
    }

    #[test]
    fn thumbnail_is_saved_with_its_source() {
        let dir = TempDir::with_files("cache-save", &[]);
        let image_path = dir.path().join("a b.png");
        write_image(&image_path, [255, 0, 0, 255]);
        let cache = ThumbnailCache::new(dir.path().join("thumbnails"));

        let image = cache.get_or_create(&image_path, 200).unwrap();
        assert_eq!(image.into_pixels(), RED.repeat(6));
        let files = cached_files(&cache, "large");
        let uri = file_uri(&image_path);
        assert_eq!(
            files,
            [cache
                .root
                .join("large")
                .join(format!("{}.png", md5_hex(&uri)))]
        );

        let decoder = png::Decoder::new(BufReader::new(File::open(&files[0]).unwrap()));
        let info = decoder.read_info().unwrap().info().clone();
        assert!(records(&info, &source(&image_path).unwrap()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&files[0]).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn up_to_date_thumbnail_is_read_from_the_cache() {
        let dir = TempDir::with_files("cache-read", &[]);
        let image_path = dir.path().join("a.png");
        write_image(&image_path, [255, 0, 0, 255]);
        set_mtime(&image_path, 1_000_000);
        let cache = ThumbnailCache::new(dir.path().join("thumbnails"));
        cache.get_or_create(&image_path, 128).unwrap();

        // Same size and modification time, but blue: the cached red is used.
        write_image(&image_path, [0, 0, 255, 255]);
        set_mtime(&image_path, 1_000_000);
        let image = cache.get_or_create(&image_path, 128).unwrap();
        assert_eq!(image.into_pixels(), RED.repeat(6));
    }

    #[test]
    fn thumbnail_with_compressed_uri_is_used() {
        let dir = TempDir::with_files("cache-ztxt", &[]);
        let image_path = dir.path().join("a.png");
        write_image(&image_path, [255, 0, 0, 255]);
        let cache = ThumbnailCache::new(dir.path().join("thumbnails"));
        let source = source(&image_path).unwrap();

        // A blue thumbnail written like KIO does, with the URI in a zTXt chunk.
        let cache_path = cache
            .root
            .join("normal")
            .join(format!("{}.png", md5_hex(&source.uri)));
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let file = BufWriter::new(File::create(&cache_path).unwrap());
        let mut encoder = png::Encoder::new(file, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .add_ztxt_chunk(URI_KEY.to_owned(), source.uri.clone())
            .unwrap();
        encoder
            .add_text_chunk(MTIME_KEY.to_owned(), source.mtime.to_string())
            .unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0, 0, 255, 255]).unwrap();
        writer.finish().unwrap();

        let image = cache.get_or_create(&image_path, 128).unwrap();
        assert_eq!(image.into_pixels(), BLUE);
    }

    #[test]
    fn modified_image_gets_a_new_thumbnail() {
        let dir = TempDir::with_files("cache-modified", &[]);
        let image_path = dir.path().join("a.png");
        write_image(&image_path, [255, 0, 0, 255]);
        set_mtime(&image_path, 1_000_000);
        let cache = ThumbnailCache::new(dir.path().join("thumbnails"));
        cache.get_or_create(&image_path, 128).unwrap();

        write_image(&image_path, [0, 0, 255, 255]);
        set_mtime(&image_path, 2_000_000);
        let image = cache.get_or_create(&image_path, 128).unwrap();
        assert_eq!(image.into_pixels(), BLUE.repeat(6));
        assert_eq!(cached_files(&cache, "normal").len(), 1);
    }

    #[test]
    fn images_in_the_cache_are_not_cached() {
        let dir = TempDir::with_files("cache-nested", &[]);
        let cache = ThumbnailCache::new(dir.path().join("thumbnails"));
        fs::create_dir_all(cache.root.join("other")).unwrap();
        let image_path = cache.root.join("other/a.png");
        write_image(&image_path, [255, 0, 0, 255]);
        cache.get_or_create(&image_path, 128).unwrap();
        assert_eq!(cached_files(&cache, "normal"), Vec::<PathBuf>::new());
    }

    #[test]
    fn corrupt_cached_thumbnail_is_replaced() {
        let dir = TempDir::with_files("cache-corrupt", &[]);
        let image_path = dir.path().join("a.png");
        write_image(&image_path, [255, 0, 0, 255]);
        let cache = ThumbnailCache::new(dir.path().join("thumbnails"));
        cache.get_or_create(&image_path, 128).unwrap();
        let cached = &cached_files(&cache, "normal")[0];
        fs::write(cached, b"not a png").unwrap();

        let image = cache.get_or_create(&image_path, 128).unwrap();
        assert_eq!(image.into_pixels(), RED.repeat(6));
        assert!(fs::read(cached).unwrap().starts_with(b"\x89PNG"));
    }
}
