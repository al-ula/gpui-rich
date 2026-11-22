//! Asynchronous [`AssetProvider`] contract and built-in loaders for decoding inline images
//! into GPUI [`RenderImage`] handles painted via `window.paint_image`.

use gpui_kit::{Image, ImageFormat, RenderImage, SvgRenderer};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

/// Image payload returned by an [`AssetProvider`].
#[derive(Clone)]
pub enum ImageAssetData {
    /// Encoded image bytes decoded on a background thread via GPUI's native image/SVG decoders.
    Encoded { format: ImageFormat, bytes: Vec<u8> },
    /// Pre-decoded GPUI [`RenderImage`] ready for immediate painting.
    Decoded(Arc<RenderImage>),
}

impl ImageAssetData {
    /// Constructs an encoded image payload from `format` and raw bytes.
    pub fn encoded(format: ImageFormat, bytes: impl Into<Vec<u8>>) -> Self {
        Self::Encoded {
            format,
            bytes: bytes.into(),
        }
    }

    /// Wraps an already-decoded GPUI [`RenderImage`].
    pub fn decoded(image: Arc<RenderImage>) -> Self {
        Self::Decoded(image)
    }
}

/// Cached load state for one `InlineObject::Image::asset_id`.
#[derive(Clone, Debug)]
pub enum LoadedAsset {
    Loading,
    Ready(Arc<RenderImage>),
    Failed(String),
}

/// Asynchronous asset resolver supplied by the host application for `InlineObject::Image`.
pub trait AssetProvider: Send + Sync + 'static {
    /// Fetches raw or pre-decoded image data for `asset_id`.
    /// Executed on GPUI's background executor so network or disk I/O never blocks painting.
    fn load_image(
        &self,
        asset_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ImageAssetData, String>> + Send + 'static>>;
}

/// Decodes [`ImageAssetData`] into a GPUI [`RenderImage`] (safe to run on a background thread).
pub fn decode_image_asset(
    data: ImageAssetData,
    svg_renderer: SvgRenderer,
) -> Result<Arc<RenderImage>, String> {
    match data {
        ImageAssetData::Decoded(image) => Ok(image),
        ImageAssetData::Encoded { format, bytes } => Image::from_bytes(format, bytes)
            .to_image_data(svg_renderer)
            .map_err(|error| error.to_string()),
    }
}

/// Infers a GPUI [`ImageFormat`] from a file extension or asset identifier suffix.
pub fn infer_image_format(path_or_id: &str) -> Option<ImageFormat> {
    let extension = Path::new(path_or_id)
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::Webp),
        "gif" => Some(ImageFormat::Gif),
        "svg" => Some(ImageFormat::Svg),
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "ico" => Some(ImageFormat::Ico),
        "pnm" | "ppm" | "pgm" | "pbm" => Some(ImageFormat::Pnm),
        _ => None,
    }
}

/// Thread-safe in-memory [`AssetProvider`] useful for embedded assets, data bundles, and demos.
#[derive(Default)]
pub struct MemoryAssetProvider {
    entries: RwLock<HashMap<String, ImageAssetData>>,
}

impl MemoryAssetProvider {
    /// Creates an empty in-memory asset provider.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts encoded image bytes for `asset_id`.
    pub fn insert_encoded(
        &self,
        asset_id: impl Into<String>,
        format: ImageFormat,
        bytes: impl Into<Vec<u8>>,
    ) {
        if let Ok(mut guard) = self.entries.write() {
            guard.insert(asset_id.into(), ImageAssetData::encoded(format, bytes));
        }
    }

    /// Inserts a pre-decoded [`RenderImage`] for `asset_id`.
    pub fn insert_decoded(&self, asset_id: impl Into<String>, image: Arc<RenderImage>) {
        if let Ok(mut guard) = self.entries.write() {
            guard.insert(asset_id.into(), ImageAssetData::decoded(image));
        }
    }

    /// Removes `asset_id` from the in-memory store.
    pub fn remove(&self, asset_id: &str) -> bool {
        self.entries
            .write()
            .map(|mut guard| guard.remove(asset_id).is_some())
            .unwrap_or(false)
    }
}

impl AssetProvider for MemoryAssetProvider {
    fn load_image(
        &self,
        asset_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ImageAssetData, String>> + Send + 'static>> {
        let result = self
            .entries
            .read()
            .map_err(|_| "memory asset lock poisoned".to_string())
            .and_then(|guard| {
                guard
                    .get(asset_id)
                    .cloned()
                    .ok_or_else(|| format!("asset not found: {asset_id}"))
            });
        Box::pin(async move { result })
    }
}

/// Local filesystem [`AssetProvider`] resolving `asset_id` relative to a base directory.
pub struct FileAssetProvider {
    base_dir: PathBuf,
}

impl FileAssetProvider {
    /// Creates a filesystem asset loader rooted at `base_dir`.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }
}

impl AssetProvider for FileAssetProvider {
    fn load_image(
        &self,
        asset_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ImageAssetData, String>> + Send + 'static>> {
        let stripped = asset_id.strip_prefix("file://").unwrap_or(asset_id);
        let candidate = Path::new(stripped);
        let full_path = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.base_dir.join(candidate)
        };
        let format = infer_image_format(stripped);

        Box::pin(async move {
            let Some(format) = format else {
                return Err(format!("unsupported image extension: {}", full_path.display()));
            };
            let bytes = std::fs::read(&full_path)
                .map_err(|error| format!("failed to read {}: {error}", full_path.display()))?;
            Ok(ImageAssetData::Encoded { format, bytes })
        })
    }
}
