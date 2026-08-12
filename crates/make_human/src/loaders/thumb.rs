//! .thumb file loader - MakeHuman thumbnail images (PNG format)

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    image::{CompressedImageFormats, Image, ImageFormat, ImageSampler, ImageType},
    prelude::*,
};
use thiserror::Error;

#[derive(Default, TypePath)]
pub struct ThumbLoader;

#[derive(Debug, Error)]
pub enum ThumbLoaderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Image error: {0}")]
    Image(#[from] TextureError),
}

impl AssetLoader for ThumbLoader {
    type Asset = Image;
    type Settings = ();
    type Error = ThumbLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        // .thumb files are PNG images
        let image = Image::from_buffer(
            &bytes,
            ImageType::Format(ImageFormat::Png),
            CompressedImageFormats::NONE,
            true, // is_srgb
            ImageSampler::Default,
            default(),
        )?;

        Ok(image)
    }

    fn extensions(&self) -> &[&str] {
        &["thumb"]
    }
}
