//! The app's logo (#69): a price tag with the M cut out and the name in
//! Fraunces, kept as SVG in `assets/brand/`. The title bar draws them, the
//! tray renders the app icon from them, and the packaged icons
//! (`assets/icon.png`, `assets/mascate.ico`) are generated from them.

use std::borrow::Cow;
use std::sync::Arc;

use gpui_kit::{
    AssetSource, DevicePixels, Pixels, Result, SharedString, Styled as _, Svg, SvgRenderer,
    SvgSize, size, svg,
};

/// The tag alone, cropped to its outline. One color, so it takes any tint.
const MARK: Brand = Brand {
    path: "brand/mark.svg",
    bytes: include_bytes!("../assets/brand/mark.svg"),
    aspect: 57. / 41.,
};
/// The name outlined from Fraunces (OFL): the letters ship as paths, not as a font.
const WORDMARK: Brand = Brand {
    path: "brand/wordmark.svg",
    bytes: include_bytes!("../assets/brand/wordmark.svg"),
    aspect: 159.57 / 35.41,
};
/// The tag on the app's dark ground: tray, window and installers.
const APP_ICON: Brand = Brand {
    path: "brand/app-icon.svg",
    bytes: include_bytes!("../assets/brand/app-icon.svg"),
    aspect: 1.,
};
const ALL: [Brand; 3] = [MARK, WORDMARK, APP_ICON];

struct Brand {
    path: &'static str,
    bytes: &'static [u8],
    /// Width over height, from the file's `viewBox`.
    aspect: f32,
}

impl Brand {
    fn svg(&self, height: Pixels) -> Svg {
        svg()
            .path(self.path)
            .h(height)
            .w(height * self.aspect)
            .flex_none()
    }
}

/// The tag, `height` tall; tint it with `text_color`.
pub fn mark(height: Pixels) -> Svg {
    MARK.svg(height)
}

/// The name, `height` tall; tint it with `text_color`.
pub fn wordmark(height: Pixels) -> Svg {
    WORDMARK.svg(height)
}

/// gpui-kit's icons plus the logo.
#[derive(Debug, Clone, Copy, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match ALL.iter().find(|brand| brand.path == path) {
            Some(brand) => Ok(Some(Cow::Borrowed(brand.bytes))),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(
            ALL.iter()
                .filter(|brand| brand.path.starts_with(path))
                .map(|brand| brand.path.into()),
        );
        Ok(paths)
    }
}

/// The app icon as straight RGBA pixels, `side` × `side`.
pub fn app_icon_rgba(side: u32) -> Vec<u8> {
    let renderer = SvgRenderer::new(Arc::new(Assets));
    let svg = renderer
        .parse_svg(APP_ICON.bytes)
        .expect("the app icon is valid SVG");
    let side = DevicePixels(i32::try_from(side).expect("an icon side fits in i32"));
    let image = renderer
        .render_parsed(&svg, SvgSize::ExactSize(size(side, side)))
        .expect("the app icon renders");
    let mut pixels = image.as_bytes(0).expect("one frame").to_vec();
    // GPUI hands images out in BGRA, the order its renderers upload.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    pixels
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The sizes `mascate.ico` carries: Windows picks one per DPI and place.
    const ICO_SIDES: [u32; 7] = [16, 20, 24, 32, 48, 64, 256];
    /// The side of `icon.png`, the installers' icon.
    const PNG_SIDE: u32 = 256;

    fn asset(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(name)
    }

    fn alpha(pixels: &[u8], side: u32, x: u32, y: u32) -> u8 {
        pixels[((y * side + x) * 4 + 3) as usize]
    }

    /// The largest difference in any channel; rasterizers round a little
    /// differently across platforms, so the generated icons get slack.
    fn max_difference(a: &[u8], b: &[u8]) -> u8 {
        assert_eq!(a.len(), b.len(), "same size");
        a.iter()
            .zip(b)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn the_logo_ships_with_the_app_and_gpui_kits_icons_still_do() {
        for brand in ALL {
            assert!(
                matches!(Assets.load(brand.path), Ok(Some(_))),
                "{} is not shipped",
                brand.path
            );
        }
        assert!(matches!(
            Assets.load(&gpui_kit::assets::IconName::Close.path()),
            Ok(Some(_))
        ));
        assert_eq!(
            Assets.list("brand/").expect("listing"),
            ALL.map(|brand| SharedString::from(brand.path))
        );
    }

    #[test]
    fn each_drawn_size_keeps_its_files_proportions() {
        let renderer = SvgRenderer::new(Arc::new(Assets));
        for brand in ALL {
            let svg = renderer.parse_svg(brand.bytes).expect("valid SVG");
            let image = renderer
                .render_parsed(&svg, SvgSize::ScaleFactor(8.))
                .expect("renders");
            let rendered = image.size(0);
            let aspect = i32::from(rendered.width) as f32 / i32::from(rendered.height) as f32;
            assert!(
                (aspect - brand.aspect).abs() < 0.01,
                "{}: drawn at {}, the file is {aspect}",
                brand.path,
                brand.aspect
            );
        }
    }

    #[test]
    fn the_tray_icon_is_the_tile_with_transparent_corners() {
        let side = 32;
        let pixels = app_icon_rgba(side);
        assert_eq!(pixels.len(), (side * side * 4) as usize);
        assert_eq!(alpha(&pixels, side, 0, 0), 0, "rounded corner");
        assert_eq!(
            alpha(&pixels, side, side - 1, side - 1),
            0,
            "rounded corner"
        );
        assert_eq!(alpha(&pixels, side, side / 2, 2), 255, "the tile");
        let center = ((side / 2 * side + side / 2) * 4) as usize;
        assert_ne!(
            &pixels[center..center + 3],
            &[0x17, 0x18, 0x1a],
            "the tag sits in the middle"
        );
    }

    #[test]
    fn the_packaged_icons_match_the_logo() {
        let png = image::open(asset("icon.png")).expect("icon.png").to_rgba8();
        assert_eq!(png.dimensions(), (PNG_SIDE, PNG_SIDE));
        assert!(
            max_difference(png.as_raw(), &app_icon_rgba(PNG_SIDE)) <= 8,
            "icon.png is stale: run `cargo test -p mascate -- --ignored write_app_icons`"
        );

        let ico = std::fs::read(asset("mascate.ico")).expect("mascate.ico");
        let decoder =
            image::codecs::ico::IcoDecoder::new(std::io::Cursor::new(ico)).expect("a valid .ico");
        let largest = image::DynamicImage::from_decoder(decoder).expect("decodes");
        assert_eq!(largest.width(), *ICO_SIDES.last().expect("sizes"));
        assert!(
            max_difference(largest.to_rgba8().as_raw(), &app_icon_rgba(largest.width())) <= 8,
            "mascate.ico is stale: run `cargo test -p mascate -- --ignored write_app_icons`"
        );
    }

    /// Regenerates the packaged icons after the logo changes.
    #[test]
    #[ignore = "writes assets/icon.png and assets/mascate.ico"]
    fn write_app_icons() {
        use image::ImageEncoder as _;
        use image::codecs::ico::{IcoEncoder, IcoFrame};
        use image::codecs::png::PngEncoder;

        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(
                &app_icon_rgba(PNG_SIDE),
                PNG_SIDE,
                PNG_SIDE,
                image::ExtendedColorType::Rgba8,
            )
            .expect("encodes the png");
        std::fs::write(asset("icon.png"), png).expect("writes icon.png");

        let frames: Vec<_> = ICO_SIDES
            .iter()
            .map(|&side| {
                IcoFrame::as_png(
                    &app_icon_rgba(side),
                    side,
                    side,
                    image::ExtendedColorType::Rgba8,
                )
                .expect("encodes a frame")
            })
            .collect();
        let mut ico = Vec::new();
        IcoEncoder::new(&mut ico)
            .encode_images(&frames)
            .expect("encodes the ico");
        std::fs::write(asset("mascate.ico"), ico).expect("writes mascate.ico");
    }
}
