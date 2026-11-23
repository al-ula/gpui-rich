//! Asynchronous [`AssetProvider`] contract and built-in loaders for decoding inline images
//! into GPUI [`RenderImage`] handles painted via `window.paint_image`.
//!
//! # Breaking changes
//!
//! * Asset requests, cache keys, and invalidation now use [`rich_model::ImageAssetId`] instead of
//!   raw `&str` / `String`. The JSON wire format remains a plain string.
//! * Failures now return structured [`ImageAssetError`] values classifying missing assets,
//!   provider failures, and decode failures while retaining their underlying causes.

use gpui_kit::{Image, ImageFormat, RenderImage, SvgRenderer};
use rich_model::ImageAssetId;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

/// Structured failure returned by [`AssetProvider::load_image`] and [`decode_image_asset`].
#[derive(Clone, Debug)]
pub enum ImageAssetError {
    /// The requested asset does not exist in the configured provider.
    NotFound {
        asset_id: ImageAssetId,
        cause: Option<Arc<dyn Error + Send + Sync>>,
    },
    /// The provider encountered an I/O, network, permission, or backend failure.
    Provider {
        asset_id: ImageAssetId,
        message: String,
        cause: Option<Arc<dyn Error + Send + Sync>>,
    },
    /// The fetched asset bytes or format could not be decoded into a GPUI [`RenderImage`].
    Decode {
        asset_id: ImageAssetId,
        message: String,
        cause: Option<Arc<dyn Error + Send + Sync>>,
    },
}

impl ImageAssetError {
    /// Constructs a missing-asset error without an inner cause.
    pub fn not_found(asset_id: ImageAssetId) -> Self {
        Self::NotFound {
            asset_id,
            cause: None,
        }
    }

    /// Constructs a missing-asset error retaining its underlying cause.
    pub fn not_found_with_cause(
        asset_id: ImageAssetId,
        cause: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::NotFound {
            asset_id,
            cause: Some(Arc::new(cause)),
        }
    }

    /// Constructs a provider failure without an inner cause.
    pub fn provider(asset_id: ImageAssetId, message: impl Into<String>) -> Self {
        Self::Provider {
            asset_id,
            message: message.into(),
            cause: None,
        }
    }

    /// Constructs a provider failure retaining its underlying cause.
    pub fn provider_with_cause(
        asset_id: ImageAssetId,
        message: impl Into<String>,
        cause: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::Provider {
            asset_id,
            message: message.into(),
            cause: Some(Arc::new(cause)),
        }
    }

    /// Constructs a decode failure without an inner cause.
    pub fn decode(asset_id: ImageAssetId, message: impl Into<String>) -> Self {
        Self::Decode {
            asset_id,
            message: message.into(),
            cause: None,
        }
    }

    /// Constructs a decode failure retaining its underlying cause.
    pub fn decode_with_cause(
        asset_id: ImageAssetId,
        message: impl Into<String>,
        cause: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::Decode {
            asset_id,
            message: message.into(),
            cause: Some(Arc::new(cause)),
        }
    }

    /// Returns the [`ImageAssetId`] associated with this failure.
    pub fn asset_id(&self) -> &ImageAssetId {
        match self {
            Self::NotFound { asset_id, .. }
            | Self::Provider { asset_id, .. }
            | Self::Decode { asset_id, .. } => asset_id,
        }
    }
}

impl fmt::Display for ImageAssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { asset_id, .. } => write!(formatter, "image asset not found: {asset_id}"),
            Self::Provider {
                asset_id, message, ..
            } => write!(formatter, "failed to load image asset '{asset_id}': {message}"),
            Self::Decode {
                asset_id, message, ..
            } => write!(formatter, "failed to decode image asset '{asset_id}': {message}"),
        }
    }
}

impl Error for ImageAssetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NotFound { cause, .. }
            | Self::Provider { cause, .. }
            | Self::Decode { cause, .. } => cause
                .as_deref()
                .map(|error| error as &(dyn Error + 'static)),
        }
    }
}

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

/// Cached load state for one [`ImageAssetId`].
#[derive(Clone, Debug)]
pub enum LoadedAsset {
    Loading,
    Ready(Arc<RenderImage>),
    Failed(ImageAssetError),
}

/// Asynchronous asset resolver supplied by the host application for `InlineObject::Image`.
///
/// Resolving an asset never mutates [`rich_model::Document`] or changes undo history.
pub trait AssetProvider: Send + Sync + 'static {
    /// Fetches raw or pre-decoded image data for `asset_id`.
    /// Executed on GPUI's background executor so network or disk I/O never blocks painting.
    fn load_image(
        &self,
        asset_id: &ImageAssetId,
    ) -> Pin<Box<dyn Future<Output = Result<ImageAssetData, ImageAssetError>> + Send + 'static>>;
}

/// Decodes [`ImageAssetData`] into a GPUI [`RenderImage`] (safe to run on a background thread).
pub fn decode_image_asset(
    asset_id: &ImageAssetId,
    data: ImageAssetData,
    svg_renderer: SvgRenderer,
) -> Result<Arc<RenderImage>, ImageAssetError> {
    match data {
        ImageAssetData::Decoded(image) => Ok(image),
        ImageAssetData::Encoded { format, bytes } => Image::from_bytes(format, bytes)
            .to_image_data(svg_renderer)
            .map_err(|error| {
                let message = error.to_string();
                let boxed: Box<dyn Error + Send + Sync + 'static> = error.into();
                ImageAssetError::Decode {
                    asset_id: asset_id.clone(),
                    message,
                    cause: Some(Arc::from(boxed)),
                }
            }),
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
    entries: RwLock<HashMap<ImageAssetId, ImageAssetData>>,
}

impl MemoryAssetProvider {
    /// Creates an empty in-memory asset provider.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts encoded image bytes for `asset_id`.
    pub fn insert_encoded(
        &self,
        asset_id: impl Into<ImageAssetId>,
        format: ImageFormat,
        bytes: impl Into<Vec<u8>>,
    ) {
        if let Ok(mut guard) = self.entries.write() {
            guard.insert(asset_id.into(), ImageAssetData::encoded(format, bytes));
        }
    }

    /// Inserts a pre-decoded [`RenderImage`] for `asset_id`.
    pub fn insert_decoded(&self, asset_id: impl Into<ImageAssetId>, image: Arc<RenderImage>) {
        if let Ok(mut guard) = self.entries.write() {
            guard.insert(asset_id.into(), ImageAssetData::decoded(image));
        }
    }

    /// Removes `asset_id` from the in-memory store.
    pub fn remove(&self, asset_id: &ImageAssetId) -> bool {
        self.entries
            .write()
            .map(|mut guard| guard.remove(asset_id).is_some())
            .unwrap_or(false)
    }
}

impl AssetProvider for MemoryAssetProvider {
    fn load_image(
        &self,
        asset_id: &ImageAssetId,
    ) -> Pin<Box<dyn Future<Output = Result<ImageAssetData, ImageAssetError>> + Send + 'static>>
    {
        let requested_id = asset_id.clone();
        let result = self
            .entries
            .read()
            .map_err(|_| {
                ImageAssetError::provider(requested_id.clone(), "memory asset lock poisoned")
            })
            .and_then(|guard| {
                guard
                    .get(&requested_id)
                    .cloned()
                    .ok_or_else(|| ImageAssetError::not_found(requested_id.clone()))
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
        asset_id: &ImageAssetId,
    ) -> Pin<Box<dyn Future<Output = Result<ImageAssetData, ImageAssetError>> + Send + 'static>>
    {
        let requested_id = asset_id.clone();
        let raw_id = asset_id.as_str();
        let stripped = raw_id.strip_prefix("file://").unwrap_or(raw_id);
        let candidate = Path::new(stripped);
        let full_path = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.base_dir.join(candidate)
        };
        let format = infer_image_format(stripped);

        Box::pin(async move {
            let Some(format) = format else {
                return Err(ImageAssetError::decode(
                    requested_id,
                    format!("unsupported image extension: {}", full_path.display()),
                ));
            };
            let bytes = std::fs::read(&full_path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    ImageAssetError::not_found_with_cause(requested_id.clone(), error)
                } else {
                    ImageAssetError::provider_with_cause(
                        requested_id.clone(),
                        format!("failed to read {}", full_path.display()),
                        error,
                    )
                }
            })?;
            Ok(ImageAssetData::Encoded { format, bytes })
        })
    }
}
