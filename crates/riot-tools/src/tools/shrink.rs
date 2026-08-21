//! 给模型的图片压缩。
//!
//! 截图和读图的原图是给**界面**显示的（落盘后按路径取）；进消息、发给
//! 视觉模型的那份走这里压小。一张 1280×8000 的整页截图原样进上下文能吃掉
//! 小半个窗口，而模型判断布局、找错位并不需要原始分辨率 —— 需要读清小字
//! 的场景该用 BrowserSnapshot（结构化文本）而不是放大图片。
//!
//! 上限取 Anthropic 建议的 ~115 万像素：超过这个数服务方自己也会缩，
//! 白传带宽还多花 token。其它视觉模型的甜点区大致相同。

use base64::Engine as _;

/// 给模型的图的像素上限（宽 × 高）。
pub(crate) const MAX_MODEL_PIXELS: u32 = 1_150_000;

/// 重编码的 JPEG 质量。75 在"看得清布局与大块文字"和"体积"之间够用 ——
/// 这份图不是给人看的，界面显示的是原图。
const JPEG_QUALITY: u8 = 75;

/// 压缩产物。
pub struct Shrunk {
    /// base64 后的 JPEG。
    pub data: String,
    /// 恒为 `image/jpeg`：无损格式缩完再存无损没有意义。
    pub media_type: &'static str,
}

/// 把图压到 [`MAX_MODEL_PIXELS`] 以内。
///
/// 返回 `None` 表示"用原图就好"：已经够小、或者解不开（不支持的格式、
/// 数据损坏）。压缩是优化不是闸门 —— 压不了绝不能让工具失败，上限的
/// 兜底在调用方（`MAX_SHOT_B64` / `MAX_IMAGE_BYTES`）。
pub fn for_model(bytes: &[u8]) -> Option<Shrunk> {
    let img = image::load_from_memory(bytes).ok()?;
    let pixels = img.width().checked_mul(img.height())?;
    if pixels <= MAX_MODEL_PIXELS {
        return None;
    }

    // 等比缩放到像素上限。resize 本身保比例，这里直接给按比例算好的目标。
    // floor 而不是 round —— 两边都向上取整能让乘积恰好越过上限。
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (nw, nh) = {
        let scale = (f64::from(MAX_MODEL_PIXELS) / f64::from(pixels)).sqrt();
        (
            (f64::from(img.width()) * scale).floor().max(1.0) as u32,
            (f64::from(img.height()) * scale).floor().max(1.0) as u32,
        )
    };

    // JPEG 不支持 alpha，先转 RGB（PNG 截图常带 alpha 通道）。
    let rgb = img
        .resize(nw, nh, image::imageops::FilterType::Triangle)
        .to_rgb8();

    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&rgb)
        .ok()?;

    Some(Shrunk {
        data: base64::engine::general_purpose::STANDARD.encode(out),
        media_type: "image/jpeg",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张纯色 PNG。
    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 40, 40, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("编码 PNG");
        out.into_inner()
    }

    #[test]
    fn 大图被压到像素上限以内且保持比例() {
        // 1280×4000 ≈ 5.1M 像素，模拟整页截图的极端长宽比。
        let shrunk = for_model(&png(1280, 4000)).expect("该压");
        assert_eq!(shrunk.media_type, "image/jpeg");

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&shrunk.data)
            .expect("合法 base64");
        let img = image::load_from_memory(&bytes).expect("解得开");
        assert!(
            img.width() * img.height() <= MAX_MODEL_PIXELS,
            "还有 {}×{} 像素",
            img.width(),
            img.height()
        );
        // 比例 1280:4000 = 0.32，缩完该差不多。
        let ratio = f64::from(img.width()) / f64::from(img.height());
        assert!((ratio - 0.32).abs() < 0.01, "比例变了：{ratio}");
    }

    #[test]
    fn 小图不动() {
        assert!(
            for_model(&png(800, 600)).is_none(),
            "48 万像素在上限内，原样用就好"
        );
    }

    #[test]
    fn 解不开的数据不算错() {
        // 压缩是优化不是闸门：垃圾数据（或不支持的格式）就用原样，
        // 让上限兜底去管。
        assert!(for_model(b"not an image").is_none());
    }
}
