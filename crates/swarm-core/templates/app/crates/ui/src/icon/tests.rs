use super::*;

/// The pixel at (`x`, `y`) of an icon `size` wide.
fn at(image: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [image[i], image[i + 1], image[i + 2], image[i + 3]]
}

fn rgb(c: Rgb) -> [u8; 3] {
    [c.r, c.g, c.b]
}

/// The icon is the theme's: corners clear, the tile in its header-to-base gradient, the first row's
/// bar in the series' first colour and its bullet in `text`, all opaque inside.
#[test]
fn the_icon_is_drawn_in_the_themes_palette() {
    let size = 100;
    let image = rgba(size);
    assert_eq!(image.len(), (size * size * 4) as usize);
    assert_eq!(at(&image, size, 0, 0)[3], 0, "a corner is clear");
    let [r, g, b, a] = at(&image, size, 60, 33);
    assert_eq!(([r, g, b], a), (rgb(palette::SERIES.of(Look::Dark).at(0)), 255), "the first bar");
    let [r, g, b, a] = at(&image, size, 23, 33);
    assert_eq!(([r, g, b], a), (rgb(palette::TEXT.of(Look::Dark)), 255), "its bullet");
    let tile = at(&image, size, 50, 90);
    assert_eq!(tile[3], 255);
    let (header, base) = (rgb(palette::HEADER.of(Look::Dark)), rgb(palette::BASE.of(Look::Dark)));
    assert!((0..3).all(|i| tile[i] >= base[i].min(header[i]) && tile[i] <= base[i].max(header[i])), "the tile is the gradient: {tile:?}");
}

/// Any size is drawn, clamped, and the same bytes every time.
#[test]
fn any_size_is_drawn_clamped_and_the_same_each_time() {
    assert_eq!(rgba(0).len(), 4, "0 is 1");
    assert_eq!(rgba(u32::MAX).len(), (MAX_SIZE * MAX_SIZE * 4) as usize);
    assert_eq!(rgba(48), rgba(48));
    let icon = icon_data();
    assert_eq!((icon.width, icon.height, icon.rgba.len()), (WINDOW_SIZE, WINDOW_SIZE, (WINDOW_SIZE * WINDOW_SIZE * 4) as usize));
}
