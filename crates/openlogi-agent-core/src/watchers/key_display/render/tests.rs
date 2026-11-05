use image::{ImageFormat, Rgb, RgbImage};

use super::*;

const KEY: u16 = 118;

fn decode(tile: &KeyImageJpeg) -> RgbImage {
    image::load_from_memory_with_format(tile.as_bytes(), ImageFormat::Jpeg)
        .expect("a rendered tile is a valid JPEG")
        .to_rgb8()
}

fn luma(pixel: Rgb<u8>) -> u16 {
    pixel.0.iter().copied().map(u16::from).sum::<u16>() / 3
}

#[test]
fn every_ring_icon_renders_to_a_key_sized_jpeg() {
    for icon in ActionRingIcon::ALL {
        let tile = render_tile(&KeyFace::Icon(icon), KEY)
            .unwrap_or_else(|error| panic!("{icon:?}: {error}"));
        assert_eq!(decode(&tile).dimensions(), (118, 118), "{icon:?}");
    }
}

#[test]
fn a_blank_face_is_dark_everywhere() {
    let image = decode(&render_tile(&KeyFace::Blank, KEY).unwrap());
    assert!(image.pixels().all(|pixel| luma(*pixel) < 16));
}

/// White glyph strokes in the middle, untouched black margins at the edges:
/// a `currentColor` left unresolved would draw black on black and fail the
/// first half, a mis-scaled glyph would bleed into the second.
#[test]
fn an_icon_is_drawn_bright_and_centred_inside_dark_margins() {
    let image = decode(&render_tile(&KeyFace::Icon(ActionRingIcon::Copy), KEY).unwrap());
    let margin = u32::from(KEY) / 8;
    let (width, height) = image.dimensions();

    let bright_centre = image
        .enumerate_pixels()
        .filter(|(x, y, _)| {
            (margin..width - margin).contains(x) && (margin..height - margin).contains(y)
        })
        .filter(|(_, _, pixel)| luma(**pixel) > 200)
        .count();
    assert!(bright_centre > 100, "only {bright_centre} bright pixels");

    let edges_dark = image
        .enumerate_pixels()
        .filter(|(x, y, _)| {
            *x < margin || *y < margin || *x >= width - margin || *y >= height - margin
        })
        .all(|(_, _, pixel)| luma(*pixel) < 24);
    assert!(edges_dark, "glyph reaches into the margin");
}

#[test]
fn a_zero_sized_tile_is_refused() {
    assert!(matches!(
        render_tile(&KeyFace::Blank, 0),
        Err(RenderError::TileSize(0))
    ));
}

fn label(text: &str) -> KeyFace {
    KeyFace::Label {
        icon: ActionRingIcon::Keyboard,
        text: KeyLabel::new(text).unwrap_or_else(|| panic!("{text:?} fits a key")),
    }
}

/// Bright pixels in the rows `top..bottom`.
fn bright_rows(image: &RgbImage, top: u32, bottom: u32) -> usize {
    image
        .enumerate_pixels()
        .filter(|(_, y, pixel)| (top..bottom).contains(y) && luma(**pixel) > 200)
        .count()
}

/// Bright pixels anywhere, and how many lie within `margin` of an edge.
fn bright_pixels(image: &RgbImage, margin: u32) -> (usize, usize) {
    let (width, height) = image.dimensions();
    let bright = || {
        image
            .enumerate_pixels()
            .filter(|(_, _, pixel)| luma(**pixel) > 200)
    };
    let in_margin = bright()
        .filter(|(x, y, _)| {
            *x < margin || *y < margin || *x >= width - margin || *y >= height - margin
        })
        .count();
    (bright().count(), in_margin)
}

/// A label is drawn white and inside the bezel margin, from the shortest
/// label to the longest one admitted — the long one by shrinking, not by
/// running off the key.
#[test]
fn a_label_is_drawn_bright_inside_dark_margins() {
    let longest = "M".repeat(KeyLabel::MAX_CHARS);
    for text in ["`", "~", "Shift+`", "Ctrl+Shift+P", longest.as_str()] {
        let image = decode(&render_tile(&label(text), KEY).unwrap());
        let (bright, in_margin) = bright_pixels(&image, u32::from(KEY) / 16);
        assert!(bright > 10, "{text:?}: only {bright} bright pixels");
        assert_eq!(in_margin, 0, "{text:?} reaches into the margin");
    }
}

/// The point of a label: two shortcut keys, and a shortcut key and the
/// shared keyboard glyph, must not look the same.
#[test]
fn labels_tell_shortcut_keys_apart() {
    let tile = |face: &KeyFace| decode(&render_tile(face, KEY).unwrap()).into_raw();
    let shifted = tile(&label("Shift+`"));
    let plain = tile(&label("`"));
    let glyph = tile(&KeyFace::Icon(ActionRingIcon::Keyboard));
    assert_ne!(shifted, plain);
    assert_ne!(shifted, glyph);
    assert_ne!(plain, glyph);
}

/// Every character [`KeyLabel::new`] admits is in the embedded font: a
/// character missing from it would lay out as nothing and blank the key.
#[test]
fn every_admitted_character_renders() {
    for c in ('!'..='~').map(String::from) {
        let image = decode(&render_tile(&label(&c), KEY).unwrap());
        let (bright, _) = bright_pixels(&image, 0);
        assert!(bright > 0, "{c:?} drew nothing");
    }
}

#[test]
fn a_label_is_refused_when_a_key_could_not_show_it() {
    assert_eq!(KeyLabel::new(""), None, "empty");
    assert_eq!(KeyLabel::new("   "), None, "blank");
    assert_eq!(KeyLabel::new("é"), None, "outside the font");
    assert_eq!(KeyLabel::new("tab\there"), None, "a control character");
    let too_long = "x".repeat(KeyLabel::MAX_CHARS + 1);
    assert_eq!(KeyLabel::new(&too_long), None, "too long");
    let longest = "x".repeat(KeyLabel::MAX_CHARS);
    assert_eq!(
        KeyLabel::new(&longest).map(|label| label.as_str().len()),
        Some(KeyLabel::MAX_CHARS)
    );
}

/// The XML characters reach the panel as themselves, not as markup.
#[test]
fn a_label_with_markup_characters_renders() {
    for text in ["<", "&", "\"'", "a<b>&c"] {
        render_tile(&label(text), KEY).unwrap_or_else(|error| panic!("{text:?}: {error}"));
    }
}

/// A labelled face keeps the action's glyph, small, above its text: the
/// glyph alone fills the top band, the text alone the bottom one, and the
/// glyph is what tells two labels for different actions apart.
#[test]
fn a_label_sits_under_a_small_copy_of_the_glyph() {
    let side = u32::from(KEY);
    let image = decode(&render_tile(&label("~"), KEY).unwrap());
    let icon_bottom = side * 38 / 100;
    let text_top = side * 44 / 100;
    assert!(
        bright_rows(&image, 0, icon_bottom) > 20,
        "no glyph above the text"
    );
    assert!(
        bright_rows(&image, text_top, side) > 20,
        "no text below the glyph"
    );
    assert_eq!(
        bright_rows(&image, icon_bottom + 2, text_top - 2),
        0,
        "glyph and text run together"
    );

    let copy = KeyFace::Label {
        icon: ActionRingIcon::Copy,
        text: KeyLabel::new("~").unwrap(),
    };
    assert_ne!(
        decode(&render_tile(&copy, KEY).unwrap()).into_raw(),
        image.into_raw()
    );
}
