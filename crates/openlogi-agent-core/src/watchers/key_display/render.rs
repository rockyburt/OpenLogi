//! Rasterize a key face into the JPEG tile a keypad's LCD key displays.
//!
//! Pure CPU work with no device I/O: the action's embedded icon (the same
//! glyph the settings app and the Actions Ring draw) — or, for a key that
//! sends a keyboard shortcut or carries a label, that icon small with the
//! text under it — is rendered white on black, in a tile the size the
//! panel's keys are, and encoded as the baseline JPEG the vendor image write
//! carries.

use std::io::Cursor;
use std::sync::{Arc, LazyLock};

use image::ExtendedColorType;
use image::ImageEncoder as _;
use image::codecs::jpeg::JpegEncoder;
use openlogi_core::binding::ActionRingIcon;
use openlogi_hid::KeyImageJpeg;
use resvg::{tiny_skia, usvg};
use thiserror::Error;

/// What one LCD key shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyFace {
    /// Nothing — the key is unbound, so it does nothing and shows nothing.
    Blank,
    /// The glyph of the action the key is bound to.
    Icon(ActionRingIcon),
    /// Text under a small copy of the action's glyph: what a shortcut key
    /// sends, or the label the user gave the key. Every shortcut shares the
    /// one keyboard glyph, so two keys bound to different shortcuts would
    /// otherwise look identical; the glyph still says what kind of key it is.
    Label {
        /// The glyph of the action the key is bound to, drawn small above
        /// the text.
        icon: ActionRingIcon,
        /// The text.
        text: KeyLabel,
    },
}

/// Text a key face can show: short enough to stay legible on a key, and
/// drawn only from characters the embedded label font covers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyLabel(String);

impl KeyLabel {
    /// Longest label a key shows. Past this the text shrinks below a size
    /// that reads at arm's length, and the action's glyph says more.
    pub const MAX_CHARS: usize = 12;

    /// `text` as a label, or `None` when a key could not show it legibly:
    /// empty or blank, longer than [`Self::MAX_CHARS`], or using a character
    /// outside printable ASCII, which the embedded font does not cover.
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        let printable = text.chars().all(|c| matches!(c, ' '..='~'));
        let visible = text.chars().any(|c| c != ' ');
        (printable && visible && text.chars().count() <= Self::MAX_CHARS)
            .then(|| Self(text.to_owned()))
    }

    /// The label's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a face could not be turned into a tile. Every arm is a defect in the
/// embedded artwork or the encoder, not a device condition.
#[derive(Debug, Error)]
pub enum RenderError {
    /// The icon's asset path names no embedded SVG.
    #[error("no embedded icon at {0}")]
    MissingIcon(&'static str),
    /// The embedded SVG did not parse.
    #[error("icon {path} is not valid SVG: {message}")]
    Svg {
        /// The icon's asset path.
        path: &'static str,
        /// The parser's message.
        message: String,
    },
    /// A label's text could not be laid out as glyphs.
    #[error("label {text:?} could not be laid out: {message}")]
    Label {
        /// The label's text.
        text: String,
        /// What went wrong.
        message: String,
    },
    /// The tile size cannot be rasterized.
    #[error("cannot rasterize a {0}-pixel tile")]
    TileSize(u16),
    /// The JPEG encoder refused the tile, or produced more than the image
    /// write can declare.
    #[error("cannot encode the tile: {0}")]
    Encode(String),
}

/// Share of the tile's side the glyph spans; the rest is margin, so the icon
/// clears the key's rounded bezel.
const GLYPH_SCALE: f32 = 0.55;
/// Encoder quality for a flat two-tone glyph: high enough that the stroke
/// edges stay clean, while a tile stays near 1–2 KiB, one image packet.
const JPEG_QUALITY: u8 = 90;
/// A labelled face's glyph: its side and its top edge, as shares of the
/// tile's side. Small, so the text below it gets most of the key.
const LABEL_ICON_SCALE: f32 = 0.28;
const LABEL_ICON_TOP: f32 = 0.1;
/// The band a labelled face's text is fitted into, as shares of the tile's
/// side: wider than a glyph, since text is read rather than recognised and
/// needs every pixel the bezel allows.
const LABEL_TEXT_TOP: f32 = 0.44;
const LABEL_TEXT_BOTTOM: f32 = 0.9;
const LABEL_TEXT_WIDTH: f32 = 0.84;
/// A label's largest font size, as a share of the tile's side. Short text
/// draws at this size; longer text shrinks to fit the band.
const LABEL_FONT_SCALE: f32 = 0.34;
/// The largest font size for a label of one character. Punctuation such as
/// `` ` `` is a few pixels tall at [`LABEL_FONT_SCALE`], so a lone character
/// draws as large as the band allows.
const LABEL_CHAR_FONT_SCALE: f32 = 0.9;

/// A square-or-not area of the tile, in pixels, that a drawing is fitted
/// into and centred in.
#[derive(Clone, Copy)]
struct Area {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Area {
    /// A `width`-wide area centred across a `side`-pixel tile, from `top` to
    /// `bottom` (all shares of `side`).
    fn band(side: f32, width: f32, top: f32, bottom: f32) -> Self {
        Self {
            x: side * (1.0 - width) / 2.0,
            y: side * top,
            width: side * width,
            height: side * (bottom - top),
        }
    }
}

/// DejaVu Sans Bold, subset to printable ASCII — the exact range
/// [`KeyLabel::new`] admits. Embedded rather than looked up so a label renders
/// the same on every host, including one with no fonts installed.
static LABEL_FONT: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans-Bold-ascii.ttf");
/// The family name the embedded font declares.
const LABEL_FONT_FAMILY: &str = "DejaVu Sans";

/// A font database holding only [`LABEL_FONT`], built on first use.
static LABEL_FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut fonts = usvg::fontdb::Database::new();
    fonts.load_font_data(LABEL_FONT.to_vec());
    Arc::new(fonts)
});

/// Render `face` as a `size`-square tile.
pub fn render_tile(face: &KeyFace, size: u16) -> Result<KeyImageJpeg, RenderError> {
    let mut pixmap = tiny_skia::Pixmap::new(u32::from(size), u32::from(size))
        .ok_or(RenderError::TileSize(size))?;
    pixmap.fill(tiny_skia::Color::BLACK);
    let tile_side = side_of(&pixmap);
    match face {
        KeyFace::Blank => {}
        KeyFace::Icon(icon) => {
            let margin = (1.0 - GLYPH_SCALE) / 2.0;
            let area = Area::band(tile_side, GLYPH_SCALE, margin, 1.0 - margin);
            draw_icon(&mut pixmap, *icon, area)?;
        }
        KeyFace::Label { icon, text } => {
            let icon_area = Area::band(
                tile_side,
                LABEL_ICON_SCALE,
                LABEL_ICON_TOP,
                LABEL_ICON_TOP + LABEL_ICON_SCALE,
            );
            draw_icon(&mut pixmap, *icon, icon_area)?;
            let text_area = Area::band(
                tile_side,
                LABEL_TEXT_WIDTH,
                LABEL_TEXT_TOP,
                LABEL_TEXT_BOTTOM,
            );
            draw_label(&mut pixmap, text, text_area)?;
        }
    }
    encode(&pixmap, size)
}

/// The tile's side in pixels.
fn side_of(pixmap: &tiny_skia::Pixmap) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "tile sides are a few hundred pixels, exact in f32"
    )]
    let side = pixmap.width() as f32;
    side
}

/// Draw `label` white, centred in `area`, at [`LABEL_FONT_SCALE`] (or
/// [`LABEL_CHAR_FONT_SCALE`] for one character) or smaller, so it fits.
fn draw_label(
    pixmap: &mut tiny_skia::Pixmap,
    label: &KeyLabel,
    area: Area,
) -> Result<(), RenderError> {
    let side = side_of(pixmap);
    let font_scale = if label.as_str().chars().count() == 1 {
        LABEL_CHAR_FONT_SCALE
    } else {
        LABEL_FONT_SCALE
    };
    let font_size = side * font_scale;
    // Laid out at the origin and placed afterwards by its measured bounds,
    // which centres the glyphs themselves rather than the font's line box.
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}"><text x="0" y="{font_size}" font-family="{LABEL_FONT_FAMILY}" font-weight="bold" font-size="{font_size}" fill="#ffffff" xml:space="preserve">{}</text></svg>"##,
        escape_xml(label.as_str())
    );
    let options = usvg::Options {
        fontdb: Arc::clone(&LABEL_FONTS),
        font_family: LABEL_FONT_FAMILY.to_owned(),
        ..usvg::Options::default()
    };
    let laid_out = |message: String| RenderError::Label {
        text: label.as_str().to_owned(),
        message,
    };
    let tree = usvg::Tree::from_str(&svg, &options).map_err(|error| laid_out(error.to_string()))?;
    // The glyph outlines' bounds, not the text element's: SVG's text box is
    // the font's line box, which would park a high glyph such as `` ` `` at
    // the top of the key.
    let bounds = tree
        .root()
        .children()
        .iter()
        .find_map(|node| match node {
            usvg::Node::Text(text) => Some(text.flattened().abs_bounding_box()),
            _ => None,
        })
        .filter(|bounds| bounds.width() > 0.0 && bounds.height() > 0.0)
        .ok_or_else(|| laid_out("no glyphs were drawn".to_owned()))?;
    let scale = (area.width / bounds.width())
        .min(area.height / bounds.height())
        .min(1.0);
    let offset_x = area.x + (area.width - bounds.width() * scale) / 2.0 - bounds.x() * scale;
    let offset_y = area.y + (area.height - bounds.height() * scale) / 2.0 - bounds.y() * scale;
    let transform =
        tiny_skia::Transform::from_scale(scale, scale).post_translate(offset_x, offset_y);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(())
}

/// `text` with the characters XML gives meaning to escaped.
fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// Draw `icon` white, as large as fits `area` and centred in it.
fn draw_icon(
    pixmap: &mut tiny_skia::Pixmap,
    icon: ActionRingIcon,
    area: Area,
) -> Result<(), RenderError> {
    let path = icon.asset_path();
    let svg =
        openlogi_ui::action_icons::embedded_icon(path).ok_or(RenderError::MissingIcon(path))?;
    // The glyphs stroke with `currentColor`, which a renderer outside a
    // styled document resolves to black — invisible on the panel.
    let svg = String::from_utf8_lossy(svg).replace("currentColor", "#ffffff");
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).map_err(|error| {
        RenderError::Svg {
            path,
            message: error.to_string(),
        }
    })?;
    let glyph = tree.size();
    let scale = (area.width / glyph.width()).min(area.height / glyph.height());
    let offset_x = area.x + (area.width - glyph.width() * scale) / 2.0;
    let offset_y = area.y + (area.height - glyph.height() * scale) / 2.0;
    let transform =
        tiny_skia::Transform::from_scale(scale, scale).post_translate(offset_x, offset_y);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(())
}

fn encode(pixmap: &tiny_skia::Pixmap, size: u16) -> Result<KeyImageJpeg, RenderError> {
    // The tile was filled opaque before drawing, so the premultiplied pixels
    // are already the composited colours: drop alpha.
    let rgb: Vec<u8> = pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, _]| [r, g, b])
        .collect();
    let mut jpeg = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .write_image(
            &rgb,
            u32::from(size),
            u32::from(size),
            ExtendedColorType::Rgb8,
        )
        .map_err(|error| RenderError::Encode(error.to_string()))?;
    let bytes = jpeg.into_inner();
    let len = bytes.len();
    KeyImageJpeg::new(bytes)
        .ok_or_else(|| RenderError::Encode(format!("{len} bytes do not fit the image write")))
}

#[cfg(test)]
mod tests;
