//! 把一组图片写成一个 PDF：每张图片一页，支持页面尺寸、方向、适配方式与体积压缩。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use image::{ColorType, DynamicImage};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, TextStr};

/// 页面尺寸。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageSize {
    /// 页面与图片同尺寸同比例（96 DPI 换算），不留白。
    Auto,
    A4,
    A3,
    A5,
    Letter,
}

impl PageSize {
    pub const ALL: [(PageSize, &'static str); 5] = [
        (PageSize::Auto, "原图"),
        (PageSize::A4, "A4"),
        (PageSize::A3, "A3"),
        (PageSize::A5, "A5"),
        (PageSize::Letter, "Letter"),
    ];

    /// 固定尺寸（单位 pt）的宽高，`None` 表示跟随图片。
    fn points(self) -> Option<(f32, f32)> {
        Some(match self {
            PageSize::Auto => return None,
            PageSize::A4 => (595.276, 841.890),
            PageSize::A3 => (841.890, 1190.551),
            PageSize::A5 => (419.528, 595.276),
            PageSize::Letter => (612.0, 792.0),
        })
    }
}

/// 纸张方向。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Orientation {
    Auto,
    Portrait,
    Landscape,
}

impl Orientation {
    pub const ALL: [(Orientation, &'static str); 3] = [
        (Orientation::Auto, "自动"),
        (Orientation::Portrait, "纵向"),
        (Orientation::Landscape, "横向"),
    ];
}

/// 图片在页面内的适配方式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FitMode {
    /// 完整显示，保持比例。
    Contain,
    /// 铺满页面，保持比例，超出部分裁切。
    Cover,
    /// 拉伸铺满，不保持比例。
    Fill,
}

impl FitMode {
    pub const ALL: [(FitMode, &'static str); 3] = [
        (FitMode::Contain, "适应"),
        (FitMode::Cover, "填充"),
        (FitMode::Fill, "拉伸"),
    ];
}

/// 页边距。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Margin {
    None,
    Small,
    Medium,
    Large,
}

impl Margin {
    pub const ALL: [(Margin, &'static str); 4] = [
        (Margin::None, "无"),
        (Margin::Small, "小"),
        (Margin::Medium, "中"),
        (Margin::Large, "大"),
    ];

    fn points(self) -> f32 {
        match self {
            Margin::None => 0.0,
            Margin::Small => 18.0,
            Margin::Medium => 36.0,
            Margin::Large => 54.0,
        }
    }
}

/// 导出参数。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PdfOptions {
    pub page: PageSize,
    pub orientation: Orientation,
    pub fit: FitMode,
    pub margin: Margin,
    /// 把所有图片重编码为 JPEG，体积更小、兼容性更好（透明区域铺白）。
    pub compress: bool,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            page: PageSize::A4,
            orientation: Orientation::Auto,
            fit: FitMode::Contain,
            margin: Margin::Small,
            compress: false,
        }
    }
}

/// 导出结果。
pub struct PdfOutput {
    pub data: Vec<u8>,
    /// 解码失败被跳过的文件说明。
    pub skipped: Vec<String>,
}

/// 生成 PDF 字节流。
pub fn build(paths: &[PathBuf], options: &PdfOptions) -> Result<PdfOutput, String> {
    if paths.is_empty() {
        return Err("没有可导出的图片".to_string());
    }

    let mut pdf = Pdf::new();
    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let mut next_id: i32 = 3;

    let mut page_ids: Vec<Ref> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    for path in paths {
        let prepared = match prepare(path, options.compress) {
            Ok(prepared) => prepared,
            Err(err) => {
                skipped.push(format!("{}（{err}）", display_name(path)));
                continue;
            }
        };
        if prepared.width == 0 || prepared.height == 0 {
            skipped.push(format!("{}（尺寸无效）", display_name(path)));
            continue;
        }

        let (page_width, page_height) = page_size(options, prepared.width, prepared.height);
        let margin = if options.page == PageSize::Auto {
            0.0
        } else {
            options.margin.points()
        };
        let avail_width = (page_width - margin * 2.0).max(1.0);
        let avail_height = (page_height - margin * 2.0).max(1.0);

        let image_width = prepared.width as f32;
        let image_height = prepared.height as f32;
        let (draw_width, draw_height) = match options.fit {
            FitMode::Contain => {
                let scale = (avail_width / image_width).min(avail_height / image_height);
                (image_width * scale, image_height * scale)
            }
            FitMode::Cover => {
                let scale = (avail_width / image_width).max(avail_height / image_height);
                (image_width * scale, image_height * scale)
            }
            FitMode::Fill => (avail_width, avail_height),
        };
        let draw_x = margin + (avail_width - draw_width) / 2.0;
        let draw_y = margin + (avail_height - draw_height) / 2.0;

        let page_id = Ref::new(next_id);
        next_id += 1;
        let content_id = Ref::new(next_id);
        next_id += 1;
        let image_id = Ref::new(next_id);
        next_id += 1;
        let smask_id = prepared.encoded.alpha().map(|_| {
            let id = Ref::new(next_id);
            next_id += 1;
            id
        });

        // 页面。每页只有一张图片，资源名固定为 Im0 即可。
        let image_name = Name(b"Im0");
        let mut page = pdf.page(page_id);
        page.media_box(Rect::new(0.0, 0.0, page_width, page_height));
        page.parent(page_tree_id);
        page.contents(content_id);
        page.resources().x_objects().pair(image_name, image_id);
        page.finish();

        // 图片 XObject。
        let mut image = pdf.image_xobject(image_id, prepared.encoded.data());
        image
            .width(prepared.width as i32)
            .height(prepared.height as i32)
            .bits_per_component(8)
            .interpolate(true);
        if prepared.encoded.gray() {
            image.color_space().device_gray();
        } else {
            image.color_space().device_rgb();
        }
        if let Some(smask_id) = smask_id {
            image.s_mask(smask_id);
        }
        image.filter(prepared.encoded.filter());
        image.finish();

        // 透明通道。
        if let (Some(smask_id), Some(alpha)) = (smask_id, prepared.encoded.alpha()) {
            let mut smask = pdf.image_xobject(smask_id, alpha);
            smask
                .width(prepared.width as i32)
                .height(prepared.height as i32)
                .bits_per_component(8)
                .interpolate(true);
            smask.color_space().device_gray();
            smask.filter(Filter::FlateDecode);
            smask.finish();
        }

        // 内容流：定位 + 绘制。
        let mut content = Content::new();
        content.save_state();
        if options.fit == FitMode::Cover {
            content
                .rect(margin, margin, avail_width, avail_height)
                .clip_nonzero()
                .end_path();
        }
        content.transform([draw_width, 0.0, 0.0, draw_height, draw_x, draw_y]);
        content.x_object(image_name);
        content.restore_state();
        pdf.stream(content_id, &content.finish());

        page_ids.push(page_id);
    }

    if page_ids.is_empty() {
        return Err(match skipped.first() {
            Some(reason) => format!("没有可用的图片：{reason}"),
            None => "没有可用的图片".to_string(),
        });
    }

    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);

    let info_id = Ref::new(next_id);
    pdf.document_info(info_id)
        .title(TextStr("Images"))
        .producer(TextStr("img-to-pdf"));

    Ok(PdfOutput {
        data: pdf.finish(),
        skipped,
    })
}

/// 计算页面尺寸（pt）。
fn page_size(options: &PdfOptions, width_px: u32, height_px: u32) -> (f32, f32) {
    match options.page.points() {
        // 跟随图片：按 96 DPI 换算成 pt，过大的图片整体缩放到可读范围内。
        None => {
            let mut width = width_px as f32 * 0.75;
            let mut height = height_px as f32 * 0.75;
            const MAX_SIDE: f32 = 1400.0;
            let longest = width.max(height);
            if longest > MAX_SIDE {
                let scale = MAX_SIDE / longest;
                width *= scale;
                height *= scale;
            }
            (width.max(1.0), height.max(1.0))
        }
        Some((first, second)) => {
            let landscape = match options.orientation {
                Orientation::Portrait => false,
                Orientation::Landscape => true,
                Orientation::Auto => width_px > height_px,
            };
            let (short, long) = (first.min(second), first.max(second));
            if landscape {
                (long, short)
            } else {
                (short, long)
            }
        }
    }
}

enum Encoded {
    /// JPEG 原样嵌入，不再解码重编码。
    Jpeg { data: Vec<u8>, gray: bool },
    /// 无损 Flate 压缩的像素，可选独立透明通道。
    Flate {
        data: Vec<u8>,
        gray: bool,
        alpha: Option<Vec<u8>>,
    },
}

impl Encoded {
    fn data(&self) -> &[u8] {
        match self {
            Encoded::Jpeg { data, .. } | Encoded::Flate { data, .. } => data,
        }
    }

    fn filter(&self) -> Filter {
        match self {
            Encoded::Jpeg { .. } => Filter::DctDecode,
            Encoded::Flate { .. } => Filter::FlateDecode,
        }
    }

    fn gray(&self) -> bool {
        match self {
            Encoded::Jpeg { gray, .. } | Encoded::Flate { gray, .. } => *gray,
        }
    }

    fn alpha(&self) -> Option<&[u8]> {
        match self {
            Encoded::Flate { alpha, .. } => alpha.as_deref(),
            Encoded::Jpeg { .. } => None,
        }
    }
}

struct Prepared {
    width: u32,
    height: u32,
    encoded: Encoded,
}

fn prepare(path: &Path, compress: bool) -> Result<Prepared, String> {
    let bytes = fs::read(path).map_err(|err| format!("无法读取：{err}"))?;

    // 未开启压缩时，基线 JPEG 可以原样嵌入，画质无损且体积最小。
    if !compress {
        if let Some(spec) = jpeg_passthrough(&bytes) {
            return Ok(Prepared {
                width: spec.width,
                height: spec.height,
                encoded: Encoded::Jpeg {
                    data: bytes,
                    gray: spec.gray,
                },
            });
        }
    }

    let dynamic = image::load_from_memory(&bytes).map_err(|err| format!("无法解码：{err}"))?;
    let (width, height) = (dynamic.width(), dynamic.height());

    if compress {
        let rgb = flatten_on_white(&dynamic);
        let mut out = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 86);
        encoder
            .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
            .map_err(|err| format!("无法压缩：{err}"))?;
        return Ok(Prepared {
            width,
            height,
            encoded: Encoded::Jpeg { data: out, gray: false },
        });
    }

    if dynamic.color().has_alpha() {
        let rgba = dynamic.to_rgba8();
        let mut rgb = Vec::with_capacity(rgba.pixels().len() * 3);
        let mut alpha = Vec::with_capacity(rgba.pixels().len());
        for pixel in rgba.pixels() {
            let [r, g, b, a] = pixel.0;
            rgb.extend_from_slice(&[r, g, b]);
            alpha.push(a);
        }
        return Ok(Prepared {
            width,
            height,
            encoded: Encoded::Flate {
                data: zlib(&rgb),
                gray: false,
                alpha: Some(zlib(&alpha)),
            },
        });
    }

    if matches!(dynamic.color(), ColorType::L8) {
        return Ok(Prepared {
            width,
            height,
            encoded: Encoded::Flate {
                data: zlib(dynamic.to_luma8().as_raw()),
                gray: true,
                alpha: None,
            },
        });
    }

    Ok(Prepared {
        width,
        height,
        encoded: Encoded::Flate {
            data: zlib(dynamic.to_rgb8().as_raw()),
            gray: false,
            alpha: None,
        },
    })
}

/// 把透明像素铺到白底上，供 JPEG 编码使用。
fn flatten_on_white(image: &DynamicImage) -> Vec<u8> {
    let rgba = image.to_rgba8();
    let mut rgb = Vec::with_capacity(rgba.pixels().len() * 3);
    for pixel in rgba.pixels() {
        let [r, g, b, a] = pixel.0;
        if a == 255 {
            rgb.extend_from_slice(&[r, g, b]);
        } else {
            let alpha = a as u32;
            let blend = |channel: u8| -> u8 {
                ((channel as u32 * alpha + 255 * (255 - alpha)) / 255) as u8
            };
            rgb.extend_from_slice(&[blend(r), blend(g), blend(b)]);
        }
    }
    rgb
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // 写入内存缓冲区不会失败。
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

struct JpegSpec {
    width: u32,
    height: u32,
    gray: bool,
}

/// 判断 JPEG 能否直接以 DCTDecode 嵌入（基线 / 扩展顺序、灰度或 RGB）。
fn jpeg_passthrough(bytes: &[u8]) -> Option<JpegSpec> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }

    let mut i = 2usize;
    while i + 1 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        match marker {
            // 填充字节
            0xFF => {
                i += 1;
                continue;
            }
            // 无长度字段的标记
            0xD8 | 0x01 | 0xD0..=0xD7 => {
                i += 2;
                continue;
            }
            // 到了扫描数据或文件结尾仍未遇到 SOF
            0xDA | 0xD9 => return None,
            _ => {}
        }

        if i + 4 > bytes.len() {
            return None;
        }
        let length = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if length < 2 {
            return None;
        }
        let segment_end = i + 2 + length;
        if segment_end > bytes.len() {
            return None;
        }

        // 渐进式、算术编码等变体不直接透传，交给解码路径处理。
        if matches!(marker, 0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            return None;
        }

        if matches!(marker, 0xC0 | 0xC1) {
            if i + 10 > bytes.len() {
                return None;
            }
            let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            let components = bytes[i + 9];
            let gray = match components {
                1 => true,
                3 => false,
                _ => return None,
            };
            if width == 0 || height == 0 {
                return None;
            }
            return Some(JpegSpec {
                width,
                height,
                gray,
            });
        }

        i = segment_end;
    }

    None
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("img-to-pdf-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn gradient(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let alpha = if x < 4 { 120 } else { 255 };
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, alpha])
        })
    }

    #[test]
    fn one_page_per_supported_format() {
        let dir = TempDir::new("mixed");
        let png = dir.path("alpha.png");
        gradient(120, 90).save(&png).unwrap();
        let jpg = dir.path("photo.jpg");
        image::DynamicImage::ImageRgba8(gradient(240, 160))
            .to_rgb8()
            .save(&jpg)
            .unwrap();
        let webp = dir.path("shot.webp");
        gradient(64, 48).save(&webp).unwrap();

        let output = build(&[png, jpg, webp], &PdfOptions::default()).expect("build pdf");
        assert!(output.data.starts_with(b"%PDF"));
        assert!(output.skipped.is_empty(), "skipped: {:?}", output.skipped);

        let document = lopdf::Document::load_mem(&output.data).expect("parsable pdf");
        assert_eq!(document.get_pages().len(), 3);
    }

    #[test]
    fn compressed_export_is_smaller() {
        let dir = TempDir::new("compress");
        let png = dir.path("big.png");
        gradient(300, 300).save(&png).unwrap();

        let mut options = PdfOptions::default();
        let lossless = build(&[png.clone()], &options).unwrap();
        options.compress = true;
        let compressed = build(&[png], &options).unwrap();

        let document = lopdf::Document::load_mem(&compressed.data).expect("parsable pdf");
        assert_eq!(document.get_pages().len(), 1);
        assert!(compressed.data.len() < lossless.data.len());
    }

    #[test]
    fn page_geometry_follows_options() {
        let mut options = PdfOptions::default();
        options.page = PageSize::A4;

        let (width, height) = page_size(&options, 400, 200);
        assert!(width > height, "自动方向应按图片选择横向");

        options.orientation = Orientation::Portrait;
        let (width, height) = page_size(&options, 400, 200);
        assert!(width < height, "强制纵向");

        options.orientation = Orientation::Landscape;
        let (width, height) = page_size(&options, 200, 400);
        assert!(width > height, "强制横向");

        let mut auto = options;
        auto.page = PageSize::Auto;
        let (width, height) = page_size(&auto, 300, 150);
        assert!((width / height - 2.0).abs() < 0.01, "原图模式保持比例");
    }

    #[test]
    fn baseline_jpeg_is_embedded_without_reencoding() {
        let dir = TempDir::new("jpeg");
        let jpg = dir.path("photo.jpg");
        image::DynamicImage::ImageRgba8(gradient(240, 160))
            .to_rgb8()
            .save(&jpg)
            .unwrap();

        let bytes = fs::read(&jpg).unwrap();
        let spec = jpeg_passthrough(&bytes).expect("识别基线 JPEG");
        assert_eq!((spec.width, spec.height), (240, 160));
        assert!(!spec.gray);

        let output = build(&[jpg], &PdfOptions::default()).unwrap();
        let document = lopdf::Document::load_mem(&output.data).unwrap();
        let objects = document.objects.values();
        let has_dct = objects.clone().any(|object| match object {
            lopdf::Object::Stream(stream) => stream
                .dict
                .get(b"Filter")
                .map(|filter| format!("{filter:?}").contains("DCTDecode"))
                .unwrap_or(false),
            _ => false,
        });
        assert!(has_dct, "应使用 DCTDecode 直接嵌入 JPEG");
    }

    #[test]
    fn unreadable_files_are_skipped() {
        let dir = TempDir::new("broken");
        let broken = dir.path("broken.png");
        fs::write(&broken, b"not an image").unwrap();

        let output = build(&[broken], &PdfOptions::default());
        let error = output.err().expect("没有可用图片时应报错");
        assert!(error.contains("broken.png"), "错误信息应包含文件名");
    }
}
