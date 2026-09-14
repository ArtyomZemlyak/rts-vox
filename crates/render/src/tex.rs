//! Плитки материалов — в текстуры Bevy.
//!
//! Числа плиток живут в [`crate::look`], а здесь только перекладывание
//! байтов в ассет. Разделение не косметическое: `look` считается
//! headless и покрыт тестами, а этот файл без окна не проверить вовсе.

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::look;

/// Плитка класса — в текстуру.
///
/// Уровни детализации выкладываются **сами**: Bevy их не строит, а без
/// них воксель размером в пиксель кипит муаром на каждом движении
/// камеры (ADR-199).
///
/// `srgb` различает две роли. Цвет лежит в sRGB, потому что это цвет;
/// нормаль — линейно, потому что это вектор, и гамма его искривит.
pub fn tile_image(t: look::Tile, srgb: bool) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: t.size,
            height: t.size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.mip_level_count = t.mips;
    img.data = Some(t.rgba);
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        // Плитка повторяется каждые `TILE_VOXELS` вокселей — иначе она
        // растянулась бы на весь слитый квад, и рисунок получился бы
        // у чанка, а не у материала.
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    img
}
