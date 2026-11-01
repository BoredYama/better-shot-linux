//! Scrolling capture: registration and stitching, ported from
//! `Sources/Capture/ScrollFrameAnalyzer.swift` and the merge and session rules
//! in `Sources/Capture/ScrollCaptureController.swift` (both adapted from
//! MacShot, GPLv3; see `Resources/Licenses/MacShot.txt`).
//!
//! macOS measures the shift between frames with Vision's translational
//! registration. Here the shift is found by matching sampled rows of the two
//! frames, which is exact for scrolled screen content. The header and
//! scrollbar detection, the "too small" rule, and the merge that redraws the
//! overlap below a pinned header from the newer frame follow the Swift code.

use image::RgbaImage;

/// Mean difference (sum of R, G, B) above which two samples count as different
/// content rather than compression or antialiasing noise.
pub const DIFFERENCE_THRESHOLD: u64 = 8;

/// `ScrollCaptureController.maxScrollHeight`'s default.
pub const DEFAULT_MAX_HEIGHT: u32 = 30_000;

fn pixel_difference(a: &RgbaImage, b: &RgbaImage, x: u32, y: u32) -> u64 {
    let p = a.get_pixel(x, y);
    let q = b.get_pixel(x, y);
    (0..3).map(|c| u64::from(p[c].abs_diff(q[c]))).sum()
}

/// Width in pixels of the changing strip at the right edge (the scrollbar),
/// which must be excluded from matching. 0 when the edge is static, None when
/// the frames can't be compared.
pub fn scrollbar_width(current: &RgbaImage, previous: &RgbaImage) -> Option<u32> {
    if current.dimensions() != previous.dimensions() || current.width() == 0 {
        return None;
    }
    let (width, height) = current.dimensions();
    let row_start = height * 2 / 10;
    let row_end = height * 8 / 10;
    if row_end <= row_start {
        return Some(0);
    }
    let row_step = ((row_end - row_start) / 40).max(1) as usize;
    let mut strip = 0;
    for offset in 0..50.min(width / 8) {
        let column = width - 1 - offset;
        let rows: Vec<u32> = (row_start..row_end).step_by(row_step).collect();
        let sad: u64 = rows.iter().map(|&row| pixel_difference(current, previous, column, row)).sum();
        if sad / rows.len() as u64 > DIFFERENCE_THRESHOLD {
            strip = offset + 1;
        } else if strip > 0 {
            break;
        }
    }
    Some(strip)
}

/// How many rows at the top are unchanged: a pinned header that must not be
/// stitched in again with every strip. `height` when nothing changed.
pub fn frozen_top_rows(current: &RgbaImage, previous: &RgbaImage, right_margin: u32) -> Option<u32> {
    if current.dimensions() != previous.dimensions() {
        return None;
    }
    let (width, height) = current.dimensions();
    let last_column = width.saturating_sub(right_margin).max(1);
    for row in 0..height {
        let columns: Vec<u32> = (0..last_column.min(width)).step_by(4).collect();
        let average = if columns.is_empty() {
            u64::MAX
        } else {
            columns.iter().map(|&x| pixel_difference(current, previous, x, row)).sum::<u64>() / columns.len() as u64
        };
        if average > DIFFERENCE_THRESHOLD {
            return Some(row);
        }
    }
    Some(height)
}

/// Sampled row signatures: R+G+B at up to 160 evenly spaced columns.
fn signatures(image: &RgbaImage, columns: &[u32], rows: std::ops::Range<u32>) -> Vec<Vec<u16>> {
    rows.map(|y| {
        columns
            .iter()
            .map(|&x| {
                let p = image.get_pixel(x, y);
                u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2])
            })
            .collect()
    })
    .collect()
}

/// The vertical content shift in pixels from `previous` to `current`: how far
/// the content moved up. The header (up to a fifth of the frame) and the
/// scrollbar are left out. None when the frames don't overlap convincingly.
pub fn vertical_offset(
    previous: &RgbaImage,
    current: &RgbaImage,
    excluded_top: u32,
    excluded_right: u32,
) -> Option<u32> {
    if previous.dimensions() != current.dimensions() {
        return None;
    }
    let (width, height) = current.dimensions();
    let top = excluded_top.min(height / 5);
    let usable_width = width.saturating_sub(excluded_right);
    if usable_width <= 20 || height - top <= 20 {
        return None;
    }
    let samples = usable_width.min(160);
    let columns: Vec<u32> = (0..samples)
        .map(|i| (u64::from(i) * u64::from(usable_width - 1) / u64::from(samples.max(2) - 1)) as u32)
        .collect();
    let old = signatures(previous, &columns, top..height);
    let new = signatures(current, &columns, top..height);
    let rows = old.len();
    // Registration needs some overlap with the previous frame.
    let min_overlap = (rows / 8).max(8);

    // Rows that are flat (one color across the samples) can't anchor a match.
    let textured: Vec<bool> = new.iter().map(|row| row.iter().max() != row.iter().min()).collect();
    if !textured.iter().any(|&t| t) {
        return Some(0);
    }

    // Score each shift by how many textured rows match, so a static header or
    // a changing area can't outvote the content that actually moved.
    let row_matches = |y: usize, shift: usize| {
        let diff: u64 = new[y].iter().zip(&old[y + shift]).map(|(a, b)| u64::from(a.abs_diff(*b))).sum();
        diff <= DIFFERENCE_THRESHOLD * new[y].len() as u64
    };
    let mut best: Option<(usize, usize, usize)> = None; // (matched, textured, shift)
    for shift in 0..=rows - min_overlap {
        let overlap = rows - shift;
        let step = (overlap / 240).max(1);
        let (mut matched, mut sampled) = (0, 0);
        for y in (0..overlap).step_by(step).filter(|&y| textured[y]) {
            sampled += 1;
            if row_matches(y, shift) {
                matched += 1;
            }
        }
        if sampled > 0 && best.is_none_or(|(m, t, _)| matched * t > m * sampled) {
            best = Some((matched, sampled, shift));
        }
    }
    let (matched, sampled, shift) = best?;
    // Before the header and scrollbar are known they don't line up, so a good
    // share, not most, of the overlap must match; unrelated frames match ~0%.
    (matched >= 4 && matched * 100 >= sampled * 35).then_some(shift as u32)
}

/// Appends `offset` rows and redraws everything below the pinned header from
/// the newer frame, so content that faded or loaded in late is kept settled.
pub fn merged(existing: &RgbaImage, current: &RgbaImage, offset: u32, header: u32) -> Option<RgbaImage> {
    let (width, height) = current.dimensions();
    if existing.width() != width || header >= height || height > existing.height() + offset {
        return None;
    }
    let total = existing.height() + offset;
    let mut output = RgbaImage::new(width, total);
    image::imageops::replace(&mut output, existing, 0, 0);
    let body = image::imageops::crop_imm(current, 0, header, width, height - header).to_image();
    image::imageops::replace(&mut output, &body, 0, i64::from(total - body.height()));
    Some(output)
}

/// What one frame did to the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Appended,
    /// The page didn't move; the frame becomes the new reference.
    Unchanged,
    /// The page moved less than a tenth of the frame; wait for more.
    TooSmall,
    /// The frame couldn't be joined; the reference stays, so scrolling back recovers.
    Unmatched,
}

/// The stitching state of one scrolling capture.
pub struct Session {
    stitched: RgbaImage,
    reference: RgbaImage,
    strips: usize,
    header: u32,
    header_done: bool,
    right_margin: u32,
    right_margin_done: bool,
    frozen_detection: bool,
    max_height: u32,
}

impl Session {
    pub fn new(first: RgbaImage, frozen_detection: bool, max_height: u32) -> Self {
        Self {
            stitched: first.clone(),
            reference: first,
            strips: 1,
            header: 0,
            header_done: false,
            right_margin: 0,
            right_margin_done: false,
            frozen_detection,
            max_height,
        }
    }

    pub fn image(&self) -> &RgbaImage {
        &self.stitched
    }

    pub fn into_image(self) -> RgbaImage {
        self.stitched
    }

    pub fn strips(&self) -> usize {
        self.strips
    }

    /// Whether the stitched image reached the size limit.
    pub fn is_full(&self) -> bool {
        self.max_height > 0 && self.stitched.height() >= self.max_height
    }

    /// Adds a frame. `last` is Stop's final frame, which refreshes the page
    /// end even without further scrolling.
    pub fn process(&mut self, frame: RgbaImage, last: bool) -> Step {
        if frame.dimensions() != self.reference.dimensions() {
            return Step::Unmatched;
        }
        if !self.right_margin_done {
            if let Some(width) = scrollbar_width(&frame, &self.reference) {
                self.right_margin_done = true;
                if (3..=40).contains(&width) {
                    self.right_margin = width + 4;
                }
            }
        }
        let excluded_top = if self.header_done { self.header } else { 0 };
        let Some(offset) = vertical_offset(&self.reference, &frame, excluded_top, self.right_margin) else {
            return Step::Unmatched;
        };
        if offset == 0 && !last {
            self.reference = frame;
            return Step::Unchanged;
        }
        if !last && offset < frame.height() / 10 {
            return Step::TooSmall;
        }
        if self.frozen_detection && !self.header_done {
            self.detect_header(&frame, offset);
        }
        let header = if self.header_done { self.header } else { 0 };
        let Some(stitched) = merged(&self.stitched, &frame, offset, header) else { return Step::Unmatched };
        self.stitched = stitched;
        self.reference = frame;
        self.strips += 1;
        Step::Appended
    }

    fn detect_header(&mut self, frame: &RgbaImage, shift: u32) {
        if shift <= 5 {
            return;
        }
        let Some(rows) = frozen_top_rows(frame, &self.reference, self.right_margin) else { return };
        if rows >= frame.height() {
            return;
        }
        if rows >= 10 && rows < frame.height() * 6 / 10 {
            self.header = rows;
            self.header_done = true;
        } else if rows < 10 {
            self.header_done = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// A tall page of distinct text-like rows.
    fn page(width: u32, height: u32) -> RgbaImage {
        let mut seed = 0x2545_f491_u32;
        let mut image = RgbaImage::from_pixel(width, height, Rgba([250, 250, 250, 255]));
        let mut y = 0;
        while y + 14 < height {
            // A "line of text": random dark blocks of varying width.
            let mut x = 8;
            while x + 12 < width - 20 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let w = 3 + seed % 9;
                let shade = (seed >> 8) as u8 % 120;
                for yy in y + 2..y + 12 {
                    for xx in x..(x + w).min(width - 20) {
                        image.put_pixel(xx, yy, Rgba([shade, shade / 2, 40, 255]));
                    }
                }
                x += w + 2 + seed % 5;
            }
            y += 18 + (seed % 7);
        }
        image
    }

    /// The window: a fixed header, the page scrolled by `scroll`, and a
    /// scrollbar thumb that moves with it.
    fn frame(page: &RgbaImage, scroll: u32, height: u32, header: u32) -> RgbaImage {
        let width = page.width();
        let mut out = RgbaImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let pixel = if y < header {
                    Rgba([40, 90, (x % 255) as u8, 255])
                } else if x >= width - 12 {
                    let thumb = header + scroll * (height - header) / page.height();
                    if (thumb..thumb + 40).contains(&y) {
                        Rgba([120, 120, 120, 255])
                    } else {
                        Rgba([230, 230, 230, 255])
                    }
                } else {
                    *page.get_pixel(x, y - header + scroll)
                };
                out.put_pixel(x, y, pixel);
            }
        }
        out
    }

    #[test]
    fn offsets_follow_the_scroll_and_reject_unrelated_frames() {
        let page = page(300, 1600);
        let a = frame(&page, 0, 400, 0);
        let b = frame(&page, 137, 400, 0);
        assert_eq!(vertical_offset(&a, &b, 0, 16), Some(137));
        assert_eq!(vertical_offset(&a, &a, 0, 16), Some(0));
        let far = frame(&page, 1100, 400, 0);
        assert_eq!(vertical_offset(&a, &far, 0, 16), None, "no overlap");
        let up = frame(&page, 300, 400, 0);
        assert_eq!(vertical_offset(&up, &a, 0, 16), None, "scrolling up is not joined");
    }

    #[test]
    fn analyzer_finds_the_scrollbar_and_header() {
        let page = page(300, 1600);
        let a = frame(&page, 0, 400, 50);
        let b = frame(&page, 120, 400, 50);
        let bar = scrollbar_width(&b, &a).unwrap();
        assert!((1..=12).contains(&bar), "{bar}");
        // The page's first rows are blank in both frames, so they read as frozen too.
        assert!(frozen_top_rows(&b, &a, 16).is_some_and(|rows| (50..=56).contains(&rows)));
        assert_eq!(frozen_top_rows(&a, &a, 16), Some(400));
        assert_eq!(scrollbar_width(&a, &RgbaImage::new(10, 10)), None);
    }

    #[test]
    fn merge_redraws_the_overlap_from_the_newer_frame() {
        let existing = RgbaImage::from_pixel(10, 20, Rgba([1, 1, 1, 255]));
        let current = RgbaImage::from_pixel(10, 20, Rgba([2, 2, 2, 255]));
        let out = merged(&existing, &current, 5, 4).unwrap();
        assert_eq!(out.dimensions(), (10, 25));
        assert_eq!(out.get_pixel(0, 8)[0], 1, "above the newer body");
        assert_eq!(out.get_pixel(0, 9)[0], 2, "the body starts below its header");
        assert_eq!(out.get_pixel(0, 24)[0], 2);
        assert!(merged(&existing, &current, 0, 20).is_none());
    }

    #[test]
    fn a_session_rebuilds_the_page_with_one_header() {
        let page = page(300, 1400);
        let (height, header) = (400, 50);
        let mut session = Session::new(frame(&page, 0, height, header), true, DEFAULT_MAX_HEIGHT);
        assert_eq!(session.process(frame(&page, 20, height, header), false), Step::TooSmall);
        assert_eq!(session.process(frame(&page, 210, height, header), false), Step::Appended);
        assert_eq!(session.process(frame(&page, 210, height, header), false), Step::Unchanged);
        // Too far: no overlap, so the reference stays and scrolling back recovers.
        assert_eq!(session.process(frame(&page, 900, height, header), false), Step::Unmatched);
        assert_eq!(session.process(frame(&page, 480, height, header), false), Step::Appended);
        assert_eq!(session.process(frame(&page, 760, height, header), false), Step::Appended);
        assert_eq!(session.process(frame(&page, 760, height, header), true), Step::Appended);
        assert_eq!(session.strips(), 5);

        let image = session.image();
        assert_eq!(image.height(), height + 760);
        // Below the single header, the stitched body is the page itself.
        for y in [header, header + 100, header + 333, header + 700, image.height() - 1] {
            for x in [10, 77, 150, 260] {
                assert_eq!(image.get_pixel(x, y), page.get_pixel(x, y - header), "({x}, {y})");
            }
        }
        assert_eq!(image.get_pixel(5, 10), frame(&page, 0, height, header).get_pixel(5, 10), "the header");
    }

    #[test]
    fn blank_frames_are_unchanged_and_the_limit_is_reported() {
        let blank = RgbaImage::from_pixel(100, 100, Rgba([255, 255, 255, 255]));
        let mut session = Session::new(blank.clone(), true, 100);
        assert_eq!(session.process(blank, false), Step::Unchanged);
        assert!(session.is_full());
    }
}
