//! Silent full-screen frames on X11 sessions: `GetImage` on the root
//! window, which needs no portal, shows no dialog, and flashes nothing. The
//! root image spans every monitor in the X screen's pixel coordinates, the
//! same space GDK's monitor geometry describes (divided by the scale
//! factor), so it drops straight into a `FrozenFrame`.

use image::RgbaImage;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat, ImageOrder};

/// Whether this is an X11 session the grab can read (not XWayland under a
/// Wayland compositor, whose root window shows only X clients).
pub fn available() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some()
}

/// Reads the whole X screen as RGBA.
pub fn grab() -> Result<RgbaImage, String> {
    if !available() {
        return Err("not an X11 session".into());
    }
    let (conn, screen) = crate::pointer::connect().ok_or("The X display could not be opened.")?;
    let root = conn.setup().roots.get(screen).ok_or("The X display has no screen.")?;
    let (width, height) = (root.width_in_pixels, root.height_in_pixels);
    if width == 0 || height == 0 {
        return Err("The X screen is empty.".into());
    }
    if root.root_depth != 24 && root.root_depth != 32 {
        return Err(format!("A {}-bit X screen is not supported.", root.root_depth));
    }
    let little_endian = conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
    // A 4K screen is 33 MB, more than one request may carry, so rows come
    // in slices that fit the server's limit.
    let row_bytes = usize::from(width) * 4;
    let budget = conn.maximum_request_bytes().saturating_sub(256).max(row_bytes);
    let rows_per_request = (budget / row_bytes).clamp(1, usize::from(height)) as u16;
    let mut pixels = Vec::with_capacity(row_bytes * usize::from(height));
    let mut y = 0u16;
    while y < height {
        let rows = rows_per_request.min(height - y);
        let reply = conn
            .get_image(ImageFormat::Z_PIXMAP, root.root, 0, y as i16, width, rows, !0)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let count = usize::from(width) * usize::from(rows);
        let bytes_per_pixel = reply.data.len() / count.max(1);
        match bytes_per_pixel {
            4 => {
                for pixel in reply.data.chunks_exact(4) {
                    let (r, g, b) =
                        if little_endian { (pixel[2], pixel[1], pixel[0]) } else { (pixel[1], pixel[2], pixel[3]) };
                    pixels.extend_from_slice(&[r, g, b, 255]);
                }
            }
            3 => {
                for pixel in reply.data.chunks_exact(3) {
                    let (r, g, b) =
                        if little_endian { (pixel[2], pixel[1], pixel[0]) } else { (pixel[0], pixel[1], pixel[2]) };
                    pixels.extend_from_slice(&[r, g, b, 255]);
                }
            }
            other => return Err(format!("The X server sent {other}-byte pixels.")),
        }
        y += rows;
    }
    RgbaImage::from_raw(u32::from(width), u32::from(height), pixels)
        .ok_or_else(|| "The X server sent a frame of the wrong size.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grabs_the_x_screen_when_one_is_available() {
        if !available() || crate::pointer::connect().is_none() {
            eprintln!("skipped: no X display");
            return;
        }
        let image = grab().expect("the root window can be read");
        let (width, height) = crate::pointer::root_size().unwrap();
        assert_eq!((f64::from(image.width()), f64::from(image.height())), (width, height));
        assert!(image.pixels().all(|p| p.0[3] == 255), "the frame is opaque");
    }
}
