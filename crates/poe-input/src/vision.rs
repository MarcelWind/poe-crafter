//! Screen-capture images and tolerant template matching.
//!
//! References and live crops are converted to grayscale, lightly blurred, and
//! downscaled before a zero-mean normalized cross-correlation (NCC) search
//! over a small translation window and a small set of scales. This tolerates
//! anti-aliasing, minor UI drift, cursor/overlay noise, and small scale
//! changes without requiring a computer-vision dependency.

use std::path::Path;

use image::{GrayImage as ImageGray, ImageBuffer};
use thiserror::Error;

const WORK_WIDTH: f32 = 320.0;
const FIND_WIDTH: f32 = 1280.0;
const EPSILON: f64 = 1e-6;

#[derive(Debug, Error)]
pub enum VisionError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("image: {0}")]
    Image(#[from] image::ImageError),
    #[error("invalid calibration: {0}")]
    InvalidConfig(String),
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8) as u8
}

/// A BGRA image captured from the composed desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl CapturedImage {
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Self {
        Self {
            width,
            height,
            bgra,
        }
    }

    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Crop a sub-rectangle in pixels. Returns `None` when out of bounds.
    pub fn crop(&self, x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 || x < 0 || y < 0 {
            return None;
        }
        let x = x as u32;
        let y = y as u32;
        let width = width as u32;
        let height = height as u32;
        if x + width > self.width || y + height > self.height {
            return None;
        }
        let mut out = Vec::with_capacity(width as usize * height as usize * 4);
        for row in 0..height {
            let start = ((y + row) as usize * self.width as usize + x as usize) * 4;
            out.extend_from_slice(&self.bgra[start..start + width as usize * 4]);
        }
        Some(Self {
            width,
            height,
            bgra: out,
        })
    }

    pub fn to_gray(&self) -> GrayImage {
        let mut data = Vec::with_capacity(self.pixel_count());
        for pixel in self.bgra.chunks_exact(4) {
            data.push(luma(pixel[2], pixel[1], pixel[0]));
        }
        GrayImage {
            width: self.width,
            height: self.height,
            data,
        }
    }

    pub fn to_rgba(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(self.pixel_count() * 4);
        for pixel in self.bgra.chunks_exact(4) {
            data.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
        data
    }

    pub fn save_png(&self, path: &Path) -> Result<(), VisionError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        image::save_buffer(
            path,
            &self.to_rgba(),
            self.width,
            self.height,
            image::ColorType::Rgba8,
        )?;
        Ok(())
    }
}

/// An 8-bit grayscale image used by the matcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl GrayImage {
    pub fn load_png(path: &Path) -> Result<Self, VisionError> {
        let image = image::open(path)?.to_rgba8();
        let mut data = Vec::with_capacity((image.width() * image.height()) as usize);
        for pixel in image.pixels() {
            data.push(luma(pixel[0], pixel[1], pixel[2]));
        }
        Ok(Self {
            width: image.width(),
            height: image.height(),
            data,
        })
    }

    pub fn save_png(&self, path: &Path) -> Result<(), VisionError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        image::save_buffer(
            path,
            &self.data,
            self.width,
            self.height,
            image::ColorType::L8,
        )?;
        Ok(())
    }

    /// Light blur applied to both references and live crops before comparing.
    /// The matcher performs the shared downscale so the candidate and template
    /// always use the same working scale.
    pub fn prepared(&self) -> Self {
        self.box_blur()
    }

    fn box_blur(&self) -> Self {
        if self.width < 3 || self.height < 3 {
            return self.clone();
        }
        let width = self.width as usize;
        let height = self.height as usize;
        let mut horizontal = vec![0u16; self.data.len()];
        for y in 0..height {
            for x in 0..width {
                let left = self.data[y * width + x.saturating_sub(1)] as u16;
                let center = self.data[y * width + x] as u16;
                let right = self.data[y * width + (x + 1).min(width - 1)] as u16;
                horizontal[y * width + x] = left + center + right;
            }
        }
        let mut out = Vec::with_capacity(self.data.len());
        for y in 0..height {
            for x in 0..width {
                let top = horizontal[y.saturating_sub(1) * width + x] as u32;
                let center = horizontal[y * width + x] as u32;
                let bottom = horizontal[(y + 1).min(height - 1) * width + x] as u32;
                out.push(((top + center + bottom) / 9) as u8);
            }
        }
        Self {
            width: self.width,
            height: self.height,
            data: out,
        }
    }

    pub fn resize(&self, width: u32, height: u32) -> Self {
        if width == 0 || height == 0 {
            return self.clone();
        }
        if width == self.width && height == self.height {
            return self.clone();
        }
        let buffer: ImageGray = ImageBuffer::from_raw(self.width, self.height, self.data.clone())
            .expect("gray buffer dimensions match");
        let resized = image::imageops::resize(
            &buffer,
            width,
            height,
            image::imageops::FilterType::Triangle,
        );
        Self {
            width,
            height,
            data: resized.into_raw(),
        }
    }

    fn mean_std(&self) -> (f64, f64) {
        let count = self.data.len() as f64;
        if count == 0.0 {
            return (0.0, 0.0);
        }
        let sum: f64 = self.data.iter().map(|value| *value as f64).sum();
        let mean = sum / count;
        let variance = self
            .data
            .iter()
            .map(|value| {
                let delta = *value as f64 - mean;
                delta * delta
            })
            .sum::<f64>()
            / count;
        (mean, variance.max(0.0).sqrt())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchParams {
    pub threshold: f32,
    pub margin: f32,
    pub search_px: u32,
    pub scales: Vec<f32>,
}

impl Default for MatchParams {
    fn default() -> Self {
        Self {
            threshold: 0.90,
            margin: 0.05,
            search_px: 4,
            scales: vec![0.98, 1.0, 1.02],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchResult {
    pub score: f32,
    pub runner_up: Option<f32>,
    pub scale: f32,
    /// Matched template center in native candidate pixels.
    pub center_native: (f32, f32),
}

impl MatchResult {
    pub fn passes(&self, params: &MatchParams) -> bool {
        self.score >= params.threshold
            && self
                .runner_up
                .is_none_or(|runner_up| self.score - runner_up >= params.margin)
    }
}

struct PreparedTemplate {
    image: GrayImage,
    mean: f64,
    std: f64,
}

fn clamped_scales(params: &MatchParams) -> Vec<f32> {
    let scales: Vec<f32> = params
        .scales
        .iter()
        .copied()
        .map(|scale| scale.clamp(0.80, 1.20))
        .filter(|scale| *scale > 0.0)
        .collect();
    if scales.is_empty() {
        vec![1.0]
    } else {
        scales
    }
}

fn prepare_template(template: &GrayImage, work: f32) -> Option<PreparedTemplate> {
    if template.width == 0 || template.height == 0 {
        return None;
    }
    let image = template.resize(
        ((template.width as f32 * work).round() as u32).max(1),
        ((template.height as f32 * work).round() as u32).max(1),
    );
    let (mean, std) = image.mean_std();
    (std >= EPSILON).then_some(PreparedTemplate { image, mean, std })
}

fn search_prepared(
    candidate_work: &GrayImage,
    template: &PreparedTemplate,
    scale: f32,
    factor: f32,
    params: &MatchParams,
    full_search: bool,
) -> Option<MatchResult> {
    if candidate_work.width < template.image.width || candidate_work.height < template.image.height
    {
        return None;
    }
    let max_dx = candidate_work.width.saturating_sub(template.image.width) as i32;
    let max_dy = candidate_work.height.saturating_sub(template.image.height) as i32;
    let search = ((params.search_px as f32 * factor).ceil() as i32).max(0);
    let (x_start, x_end, y_start, y_end) = if full_search {
        (0, max_dx, 0, max_dy)
    } else {
        let center_x = max_dx / 2;
        let center_y = max_dy / 2;
        (
            (center_x - search).max(0),
            (center_x + search).min(max_dx),
            (center_y - search).max(0),
            (center_y + search).min(max_dy),
        )
    };
    let mut best: Option<MatchResult> = None;
    for oy in y_start..=y_end {
        for ox in x_start..=x_end {
            let Some(score) = ncc_at(
                candidate_work,
                &template.image,
                ox as u32,
                oy as u32,
                template.mean,
                template.std,
            ) else {
                continue;
            };
            let result = MatchResult {
                score: score as f32,
                runner_up: None,
                scale,
                center_native: (
                    (ox as f32 + template.image.width as f32 / 2.0) / factor,
                    (oy as f32 + template.image.height as f32 / 2.0) / factor,
                ),
            };
            if best.is_none_or(|current| result.score > current.score) {
                best = Some(result);
            }
            if !full_search {
                if let Some(current) = best {
                    if current.score >= params.threshold + 0.05 {
                        return Some(current);
                    }
                }
            }
        }
    }
    best
}

/// Match a prepared candidate against one prepared template, searching a small
/// window around the candidate center. Used for the panel guard, where the
/// captured region is padded around the reference.
pub fn best_match(
    candidate: &GrayImage,
    template: &GrayImage,
    params: &MatchParams,
) -> Option<MatchResult> {
    let work = (WORK_WIDTH / template.width.max(1) as f32).min(1.0);
    match_impl(candidate, template, params, work, false)
}

/// Match a prepared candidate against one prepared template across the whole
/// candidate region. Used to locate a tab inside a wide tab strip.
pub fn find_match_in_region(
    candidate: &GrayImage,
    template: &GrayImage,
    params: &MatchParams,
) -> Option<MatchResult> {
    let work = (FIND_WIDTH / candidate.width.max(1) as f32).min(1.0);
    match_impl(candidate, template, params, work, true)
}

fn match_impl(
    candidate: &GrayImage,
    template: &GrayImage,
    params: &MatchParams,
    work: f32,
    full_search: bool,
) -> Option<MatchResult> {
    if template.width == 0
        || template.height == 0
        || candidate.width < template.width
        || candidate.height < template.height
    {
        return None;
    }
    let prepared = prepare_template(template, work)?;
    let mut best: Option<MatchResult> = None;
    for scale in clamped_scales(params) {
        let factor = (work * scale).max(0.0001);
        let candidate_work = candidate.resize(
            ((candidate.width as f32 * factor).round() as u32).max(1),
            ((candidate.height as f32 * factor).round() as u32).max(1),
        );
        let Some(result) = search_prepared(
            &candidate_work,
            &prepared,
            scale,
            factor,
            params,
            full_search,
        ) else {
            continue;
        };
        if best.is_none_or(|current| result.score > current.score) {
            best = Some(result);
        }
        if !full_search && result.score >= params.threshold + 0.05 {
            return Some(result);
        }
    }
    best
}

fn ncc_at(
    candidate: &GrayImage,
    template: &GrayImage,
    ox: u32,
    oy: u32,
    template_mean: f64,
    template_std: f64,
) -> Option<f64> {
    let tw = template.width as usize;
    let th = template.height as usize;
    let cw = candidate.width as usize;
    let count = (tw * th) as f64;
    if count == 0.0 {
        return None;
    }
    let mut sum = 0.0;
    let mut sum_sq = 0.0;
    let mut cross = 0.0;
    for y in 0..th {
        let c_row = &candidate.data[(oy as usize + y) * cw + ox as usize..][..tw];
        let t_row = &template.data[y * tw..][..tw];
        for x in 0..tw {
            let c = c_row[x] as f64;
            let t = t_row[x] as f64;
            sum += c;
            sum_sq += c * c;
            cross += c * t;
        }
    }
    let mean = sum / count;
    let variance = (sum_sq / count - mean * mean).max(0.0);
    let std = variance.sqrt();
    if std < EPSILON {
        return None;
    }
    let covariance = cross / count - mean * template_mean;
    Some(covariance / (std * template_std))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemplateMatch {
    pub index: usize,
    pub result: MatchResult,
}

/// Match a candidate against several templates, returning the best and the
/// runner-up. The runner-up populates `MatchResult::runner_up` on the best.
pub fn match_best_of(
    candidate: &GrayImage,
    templates: &[&GrayImage],
    params: &MatchParams,
) -> Option<(TemplateMatch, Option<TemplateMatch>)> {
    let work = (FIND_WIDTH / candidate.width.max(1) as f32).min(1.0);
    let prepared_templates: Vec<Option<PreparedTemplate>> = templates
        .iter()
        .map(|template| prepare_template(template, work))
        .collect();

    // The working candidate is identical for every template at a given scale,
    // so build each scaled candidate once and reuse it across tab templates.
    let mut scaled_candidates: Vec<(f32, f32, GrayImage)> = Vec::new();
    for scale in clamped_scales(params) {
        let factor = (work * scale).max(0.0001);
        let image = candidate.resize(
            ((candidate.width as f32 * factor).round() as u32).max(1),
            ((candidate.height as f32 * factor).round() as u32).max(1),
        );
        scaled_candidates.push((scale, factor, image));
    }

    let mut matches: Vec<TemplateMatch> = Vec::new();
    for (index, prepared) in prepared_templates.iter().enumerate() {
        let Some(prepared) = prepared else {
            continue;
        };
        let mut best: Option<MatchResult> = None;
        for (scale, factor, candidate_work) in &scaled_candidates {
            let Some(result) =
                search_prepared(candidate_work, prepared, *scale, *factor, params, true)
            else {
                continue;
            };
            if best.is_none_or(|current| result.score > current.score) {
                best = Some(result);
            }
        }
        if let Some(result) = best {
            matches.push(TemplateMatch { index, result });
        }
    }
    if matches.is_empty() {
        return None;
    }
    matches.sort_by(|left, right| right.result.score.total_cmp(&left.result.score));
    let mut best = matches[0];
    let runner_up = matches.get(1).copied();
    best.result.runner_up = runner_up.map(|entry| entry.result.score);
    Some((best, runner_up))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> GrayImage {
        let mut data = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                data.push(((x * 13 + y * 7) % 256) as u8);
            }
        }
        GrayImage {
            width,
            height,
            data,
        }
    }

    fn with_shift(base: &GrayImage, dx: u32, dy: u32) -> GrayImage {
        let width = base.width + dx * 2;
        let height = base.height + dy * 2;
        let mut data = vec![128u8; (width * height) as usize];
        for y in 0..base.height {
            for x in 0..base.width {
                data[((y + dy) * width + x + dx) as usize] =
                    base.data[(y * base.width + x) as usize];
            }
        }
        GrayImage {
            width,
            height,
            data,
        }
    }

    #[test]
    fn identical_images_match_perfectly() {
        let template = gradient(80, 40).prepared();
        let candidate = with_shift(&gradient(80, 40), 0, 0).prepared();
        let result = best_match(&candidate, &template, &MatchParams::default()).unwrap();
        assert!(result.score > 0.99, "score {}", result.score);
    }

    #[test]
    fn small_translation_is_found() {
        let template = gradient(80, 40).prepared();
        let candidate = with_shift(&gradient(80, 40), 2, 1).prepared();
        let result = best_match(&candidate, &template, &MatchParams::default()).unwrap();
        assert!(result.score > 0.95, "score {}", result.score);
    }

    #[test]
    fn brightness_offset_still_matches() {
        let template = gradient(80, 40).prepared();
        let mut shifted = with_shift(&gradient(80, 40), 1, 1);
        for value in &mut shifted.data {
            *value = value.saturating_add(20);
        }
        let result = best_match(&shifted.prepared(), &template, &MatchParams::default()).unwrap();
        assert!(result.score > 0.95, "score {}", result.score);
    }

    #[test]
    fn unrelated_images_score_low() {
        let template = gradient(80, 40).prepared();
        let mut other = gradient(80, 40);
        for (index, value) in other.data.iter_mut().enumerate() {
            *value = ((index * 31) % 256) as u8;
        }
        let result = best_match(&other.prepared(), &template, &MatchParams::default()).unwrap();
        assert!(result.score < 0.90, "score {}", result.score);
    }

    #[test]
    fn best_of_reports_runner_up() {
        let template = gradient(80, 40).prepared();
        let mut other = template.clone();
        other.data.reverse();
        let candidate = with_shift(&gradient(80, 40), 0, 0).prepared();
        let (best, runner_up) =
            match_best_of(&candidate, &[&template, &other], &MatchParams::default()).unwrap();
        assert_eq!(best.index, 0);
        assert!(best.result.runner_up.is_some());
        assert!(runner_up.is_some());
    }

    #[test]
    fn finds_tab_anywhere_in_a_wide_strip() {
        let template = gradient(40, 20);
        let mut strip = GrayImage {
            width: 400,
            height: 60,
            data: vec![128u8; 400 * 60],
        };
        for y in 0..20 {
            for x in 0..40 {
                strip.data[((y + 15) * 400 + x + 300) as usize] =
                    template.data[(y * 40 + x) as usize];
            }
        }
        let result = find_match_in_region(
            &strip.prepared(),
            &template.prepared(),
            &MatchParams::default(),
        )
        .unwrap();
        assert!(result.score > 0.85, "score {}", result.score);
        assert!((result.center_native.0 - 320.0).abs() < 6.0);
        assert!((result.center_native.1 - 25.0).abs() < 6.0);
    }

    #[test]
    fn gray_png_round_trip() {
        let dir = std::env::temp_dir().join("poe-crafter-vision-png-test");
        let path = dir.join("template.png");
        let image = gradient(32, 16);
        image.save_png(&path).unwrap();
        let loaded = GrayImage::load_png(&path).unwrap();
        assert_eq!(loaded.width, 32);
        assert_eq!(loaded.height, 16);
        assert_eq!(loaded.data, image.data);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crop_and_save_round_trip() {
        let image = CapturedImage::new(
            2,
            2,
            vec![
                0, 0, 255, 255, // red
                0, 255, 0, 255, // green
                255, 0, 0, 255, // blue
                0, 0, 0, 255,
            ],
        );
        let cropped = image.crop(1, 0, 1, 1).unwrap();
        assert_eq!(cropped.width, 1);
        assert_eq!(cropped.bgra, vec![0, 255, 0, 255]);
    }
}
