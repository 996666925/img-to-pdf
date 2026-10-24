//! 图片解码与缩略图，供界面预览使用。

use std::path::Path;
use std::sync::Arc;

use gpui_kit::RenderImage;

/// 解码后的缩略图（BGRA，GPU 直接可用）。
pub struct Decoded {
    /// 原图尺寸。
    pub src_width: u32,
    pub src_height: u32,
    /// 缩略图尺寸。
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl Decoded {
    /// 构造可渲染的图像。像素已经是 BGRA 顺序，符合 GPUI 的图像缓存要求。
    pub fn into_render_image(self) -> Option<Arc<RenderImage>> {
        let buffer =
            image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(self.width, self.height, self.bgra)?;
        Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
    }
}

/// 读取并解码图片，按最长边缩放。
pub fn decode(path: &Path, max_edge: u32) -> Result<Decoded, String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let source = image::load_from_memory(&bytes).map_err(|err| err.to_string())?;
    let src_width = source.width();
    let src_height = source.height();
    if src_width == 0 || src_height == 0 {
        return Err("图片尺寸无效".to_string());
    }

    let longest = src_width.max(src_height);
    let thumb = if longest > max_edge {
        source.thumbnail(max_edge, max_edge)
    } else {
        source
    };
    let width = thumb.width();
    let height = thumb.height();

    // RGBA -> BGRA。
    let mut rgba = thumb.to_rgba8();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    Ok(Decoded {
        src_width,
        src_height,
        width,
        height,
        bgra: rgba.into_raw(),
    })
}
