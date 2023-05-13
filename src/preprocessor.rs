use std::{
    cmp::{max, min},
    ops::Range,
};

use crate::opt::Opt;
use image::{GrayImage, ImageBuffer, Luma};
use iter_fixed::IntoIteratorFixed;
use log::warn;
use rayon::prelude::*;
use subparse::timetypes::{TimePoint, TimeSpan};

pub struct PreprocessedVobSubtitle {
    pub time_span: TimeSpan,
    pub force: bool,
    pub images: Vec<GrayImage>,
}

pub type Result<T, E = vobsub::Error> = std::result::Result<T, E>;

/// Return a vector of binarized subtitles.
pub fn preprocess_subtitles(opt: &Opt) -> Result<Vec<PreprocessedVobSubtitle>> {
    let idx = vobsub::Index::open(&opt.input)?;
    let subtitles: Vec<vobsub::Subtitle> = idx
        .subtitles()
        .filter_map(|sub| match sub {
            Ok(sub) => Some(sub),
            Err(e) => {
                warn!(
                    "warning: unable to read subtitle: {}. (This can usually be safely ignored.)",
                    e
                );
                None
            }
        })
        .collect();
    let palette = rgb_palette_to_luminance(idx.palette());
    let result = subtitles
        .par_iter()
        .filter_map(|sub| {
            subtitle_to_images(sub, &palette, opt.threshold, opt.border).map(|images| {
                PreprocessedVobSubtitle {
                    time_span: TimeSpan::new(
                        seconds_to_time_point(sub.start_time()),
                        seconds_to_time_point(sub.end_time()),
                    ),
                    force: sub.force(),
                    images,
                }
            })
        })
        .collect();
    Ok(result)
}

/// Represents the left and right boundaries on a scanline.
#[derive(Debug)]
struct ScanlineExtent {
    left: usize,
    right: usize,
}

/// Represents a square subregion of an image, x by y.
#[derive(Debug)]
struct ImageRegion {
    x: Range<usize>,
    y: Range<usize>,
}

fn seconds_to_time_point(seconds: f64) -> TimePoint {
    TimePoint::from_msecs((seconds * 1000.0) as i64)
}

/// Convert an sRGB palette to a luminance palette.
fn rgb_palette_to_luminance(palette: &vobsub::Palette) -> [f32; 16] {
    palette.map(|x| {
        let r = srgb_to_linear(x[0]);
        let g = srgb_to_linear(x[1]);
        let b = srgb_to_linear(x[2]);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    })
}

/// Given a subtitle, binarize, invert, and split the image into multiple lines
/// with borders for direct feeding into Tesseract.
fn subtitle_to_images(
    subtitle: &vobsub::Subtitle,
    palette: &[f32; 16],
    threshold: f32,
    border: u32,
) -> Option<Vec<GrayImage>> {
    let sub_palette_visibility = generate_visibility_palette(subtitle);

    let binarized_palette = binarize_palette(
        palette,
        subtitle.palette(),
        &sub_palette_visibility,
        threshold,
    );

    let scanlines = inventory_scanlines(subtitle, &binarized_palette);
    let scanline_groups = find_contiguous_scanline_groups(&scanlines);
    if scanline_groups.is_empty() {
        // No images found.
        return None;
    }

    let image_regions = scanline_groups_to_image_regions(&scanlines, &scanline_groups);

    let raw_image_width = subtitle.coordinates().width() as u32;

    Some(
        image_regions
            .into_par_iter()
            .map(|region| {
                let x0 = region.x.start as u32;
                let y0 = region.y.start as u32;
                let width = region.x.len() as u32;
                let height = region.y.len() as u32;
                ImageBuffer::from_fn(width + border * 2, height + border * 2, |x, y| {
                    if x < border || x >= width + border || y < border || y >= height + border {
                        Luma([255])
                    } else {
                        let offset = (y0 + (y - border)) * raw_image_width + x0 + (x - border);
                        let sub_palette_ix = subtitle.raw_image()[offset as usize] as usize;
                        if binarized_palette[sub_palette_ix] {
                            Luma([0])
                        } else {
                            Luma([255])
                        }
                    }
                })
            })
            .collect(),
    )
}

/// Find all the palette indices used in this image, and filter out the
/// transparent ones. Checking each and every single pixel in the image like
/// this is probably not strictly necessary, but it could theoretically catch an
/// edge case.
fn generate_visibility_palette(subtitle: &vobsub::Subtitle) -> [bool; 4] {
    let mut sub_palette_visibility = subtitle
        .raw_image()
        .par_iter()
        .fold(
            || [false; 4],
            |mut visible: [bool; 4], &sub_palette_ix| {
                visible[sub_palette_ix as usize] = true;
                visible
            },
        )
        .reduce(
            || [false; 4],
            |mut a: [bool; 4], b: [bool; 4]| {
                a[0] = a[0] || b[0];
                a[1] = a[1] || b[1];
                a[2] = a[2] || b[2];
                a[3] = a[3] || b[3];
                a
            },
        );
    // The alpha palette is reversed.
    for (i, &alpha) in subtitle.alpha().iter().rev().enumerate() {
        if alpha == 0 {
            sub_palette_visibility[i] = false;
        }
    }
    sub_palette_visibility
}

/// Generate a binarized palette where `true` represents a filled text pixel.
fn binarize_palette(
    palette: &[f32; 16],
    sub_palette: &[u8; 4],
    sub_palette_visibility: &[bool; 4],
    threshold: f32,
) -> [bool; 4] {
    // Find the max luminance, so we can scale each luminance value by it.
    // Reminder that the sub palette is reversed.
    let mut max_luminance = 0.0;
    for (&palette_ix, &visible) in sub_palette.iter().rev().zip(sub_palette_visibility) {
        if visible {
            let luminance = palette[palette_ix as usize];
            if luminance > max_luminance {
                max_luminance = luminance;
            }
        }
    }

    // Empty image?
    if max_luminance == 0.0 {
        return [false; 4];
    }

    sub_palette
        .into_iter_fixed()
        .rev()
        .zip(sub_palette_visibility)
        .map(|(&palette_ix, &visible)| {
            if visible {
                let luminance = palette[palette_ix as usize] / max_luminance;
                luminance > threshold
            } else {
                false
            }
        })
        .collect()
}

/// Inventory each scanline of the image, recording if a given scanline has
/// text pixels, and if it does, the left and right extents of the pixels on
/// the scanline.
fn inventory_scanlines(
    subtitle: &vobsub::Subtitle,
    palette: &[bool; 4],
) -> Vec<Option<ScanlineExtent>> {
    let width = subtitle.coordinates().width() as usize;
    let height = subtitle.coordinates().height() as usize;
    (0..height)
        .into_par_iter()
        .map(|y| {
            (0..width)
                .into_par_iter()
                .fold(
                    || None,
                    |scanline: Option<ScanlineExtent>, x| {
                        let offset = y * width + x;
                        let palette_ix = subtitle.raw_image()[offset as usize] as usize;
                        if palette[palette_ix] {
                            match scanline {
                                Some(extent) => Some(ScanlineExtent {
                                    left: min(x, extent.left),
                                    right: max(x, extent.right),
                                }),
                                None => Some(ScanlineExtent { left: x, right: x }),
                            }
                        } else {
                            scanline
                        }
                    },
                )
                .reduce(
                    || None,
                    |a: Option<ScanlineExtent>, b: Option<ScanlineExtent>| match a {
                        Some(extent_a) => match b {
                            Some(extent_b) => Some(ScanlineExtent {
                                left: min(extent_a.left, extent_b.left),
                                right: max(extent_a.right, extent_b.right),
                            }),
                            None => Some(extent_a),
                        },
                        None => b,
                    },
                )
        })
        .collect()
}

/// Find ranges of contiguous, filled scanlines.
fn find_contiguous_scanline_groups(scanlines: &[Option<ScanlineExtent>]) -> Vec<Range<usize>> {
    let mut scanline_groups: Vec<Range<usize>> = Vec::new();
    let mut scanline_ix = 0;
    while scanline_ix < scanlines.len() {
        // Find the start of the next range of contiguous scanlines.
        match scanlines.iter().skip(scanline_ix).position(|x| x.is_some()) {
            Some(start_ix_offset) => {
                // Find the end of this range.
                let end_ix = match scanlines
                    .iter()
                    .skip(scanline_ix + start_ix_offset)
                    .position(|x| x.is_none())
                {
                    Some(end_ix_offset) => scanline_ix + start_ix_offset + end_ix_offset,
                    None => scanlines.len(),
                };
                scanline_groups.push((scanline_ix + start_ix_offset)..end_ix);
                scanline_ix = end_ix;
            }
            None => break,
        }
    }
    scanline_groups
}

/// Given the list of scanlines and a list of contiguous groups, calculate image regions that
/// encompass the extents.
fn scanline_groups_to_image_regions(
    scanlines: &[Option<ScanlineExtent>],
    scanline_groups: &[Range<usize>],
) -> Vec<ImageRegion> {
    scanline_groups
        .iter()
        .map(|y_range| {
            let mut left = usize::MAX;
            let mut right = usize::MIN;
            for y in y_range.clone() {
                // Unwrap here, since we should have filtered out all None
                // scanlines before calling this.
                let x = scanlines[y].as_ref().unwrap();
                if x.left < left {
                    left = x.left;
                }
                if x.right > right {
                    right = x.right;
                }
            }
            ImageRegion {
                x: left..right + 1,
                y: y_range.clone(),
            }
        })
        .collect()
}

/// Convert an sRGB color space channel to linear.
fn srgb_to_linear(channel: u8) -> f32 {
    let value = channel as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
