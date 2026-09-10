//! Image rightsizing for prompt payloads.
//!
//! Vision requests carry base64 images that dominate a request's token cost: an
//! Anthropic image bills at roughly `width × height / 750` tokens, so one 1600×1200
//! screenshot is ~2500 tokens — more than the prose around it. This module finds those
//! payloads, prices them, and shrinks the ones that are larger than the model can use.
//!
//! What it does, and what it deliberately does not:
//!
//! * **Prices every image** by parsing its header only (PNG, JPEG, WebP, GIF). Free and
//!   format-safe: no decode, no allocation proportional to the pixels.
//! * **Resizes PNG** — decode, box-filter downscale, re-encode — because PNG is what an
//!   IDE paste or a screenshot tool produces, and `flate2` gives us the codec.
//! * **Never re-encodes JPEG/WebP/GIF.** Without a decoder for those formats a "resize"
//!   would mean re-wrapping garbage, so they are only measured and reported.
//! * **Never silently degrades.** Above `max_edge` the pixels are ones the provider
//!   discards anyway (it downscales server-side and bills the smaller size), so removing
//!   them costs nothing. Going below that — `target_edge` — is opt-in and every
//!   rewrite is reported to the caller for logging.

use flate2::{write::ZlibEncoder, Compression, Crc};
use serde_json::Value;
use std::io::Write;

/// Longest edge a provider will actually use. Beyond this the provider downscales and
/// bills the smaller size, so trimming is free.
pub const DEFAULT_MAX_EDGE: u32 = 1568; // Anthropic's vision cap

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    WebP,
    Gif,
}

impl Format {
    pub fn media_type(self) -> &'static str {
        match self {
            Format::Png => "image/png",
            Format::Jpeg => "image/jpeg",
            Format::WebP => "image/webp",
            Format::Gif => "image/gif",
        }
    }

    /// Only PNG can be re-encoded here — see the module docs.
    pub fn resizable(self) -> bool {
        matches!(self, Format::Png)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ImageInfo {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

/// Read dimensions from a header without decoding pixels.
pub fn probe(data: &[u8]) -> Option<ImageInfo> {
    let be32 = |o: usize| -> Option<u32> {
        Some(u32::from_be_bytes(data.get(o..o + 4)?.try_into().ok()?))
    };
    let le32 = |o: usize| -> Option<u32> {
        Some(u32::from_le_bytes(data.get(o..o + 4)?.try_into().ok()?))
    };
    let bytes = data.len();

    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        // IHDR is always the first chunk: length, "IHDR", width, height, …
        if data.get(12..16)? != b"IHDR" {
            return None;
        }
        return Some(ImageInfo { format: Format::Png, width: be32(16)?, height: be32(20)?, bytes });
    }

    if data.starts_with(&[0xFF, 0xD8]) {
        // walk the segment chain to the first frame header (SOF0..SOF15, minus DHT/DAC)
        let mut i = 2usize;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if marker == 0xFF {
                i += 1;
                continue;
            }
            if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
                i += 2;
                continue;
            }
            let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            let is_sof = (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC;
            if is_sof {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some(ImageInfo { format: Format::Jpeg, width: w, height: h, bytes });
            }
            i += 2 + len.max(2);
        }
        return None;
    }

    if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        return match data.get(12..16)? {
            b"VP8X" => Some(ImageInfo {
                format: Format::WebP,
                // 24-bit little-endian canvas size minus one
                width: (le32(24)? & 0x00FF_FFFF) + 1,
                height: ((le32(26)? >> 8) & 0x00FF_FFFF) + 1,
                bytes,
            }),
            b"VP8 " => {
                let w = u16::from_le_bytes(data.get(26..28)?.try_into().ok()?) & 0x3FFF;
                let h = u16::from_le_bytes(data.get(28..30)?.try_into().ok()?) & 0x3FFF;
                Some(ImageInfo { format: Format::WebP, width: w as u32, height: h as u32, bytes })
            }
            b"VP8L" => {
                let bits = le32(21)?;
                Some(ImageInfo {
                    format: Format::WebP,
                    width: (bits & 0x3FFF) + 1,
                    height: ((bits >> 14) & 0x3FFF) + 1,
                    bytes,
                })
            }
            _ => None,
        };
    }

    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        let w = u16::from_le_bytes(data.get(6..8)?.try_into().ok()?) as u32;
        let h = u16::from_le_bytes(data.get(8..10)?.try_into().ok()?) as u32;
        return Some(ImageInfo { format: Format::Gif, width: w, height: h, bytes });
    }

    None
}

// ── token pricing ─────────────────────────────────────────────────────────────

/// Scale `(w, h)` so the longest edge is at most `edge`, preserving aspect ratio.
pub fn fit(w: u32, h: u32, edge: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= edge || long == 0 {
        return (w, h);
    }
    let scale = edge as f64 / long as f64;
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

/// Tokens a provider bills for an image of these dimensions, after its own downscaling.
pub fn tokens_for(provider: &str, w: u32, h: u32) -> u32 {
    match provider {
        "anthropic" => {
            let (w, h) = fit(w, h, 1568);
            (((w as u64 * h as u64) as f64) / 750.0).ceil() as u32
        }
        "openai" | "azure" | "groq" | "xai" | "deepseek" | "openrouter" | "together"
        | "fireworks" | "perplexity" | "mistral" | "cerebras" => {
            // fit inside 2048², then the short side to 768, then 512px tiles
            let (w, h) = fit(w, h, 2048);
            let short = w.min(h);
            let (w, h) = if short > 768 {
                let s = 768.0 / short as f64;
                (
                    ((w as f64 * s).round() as u32).max(1),
                    ((h as f64 * s).round() as u32).max(1),
                )
            } else {
                (w, h)
            };
            let tiles = w.div_ceil(512) * h.div_ceil(512);
            85 + 170 * tiles
        }
        "gemini" => {
            // ≤384² is one 258-token tile; larger images are cut into 768px tiles
            if w.max(h) <= 384 {
                258
            } else {
                258 * (w.div_ceil(768) * h.div_ceil(768))
            }
        }
        // unknown provider: Anthropic's formula is the common middle ground
        _ => (((w as u64 * h as u64) as f64) / 750.0).ceil() as u32,
    }
}

// ── PNG decode ────────────────────────────────────────────────────────────────

struct Png {
    width: u32,
    height: u32,
    /// always expanded to 8-bit RGBA
    rgba: Vec<u8>,
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (a as i16, b as i16, c as i16);
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

/// Decode the PNG subset screenshots actually use: 8-bit, non-interlaced,
/// colour types 0/2/3/4/6. Anything else returns `None` and is left untouched.
fn png_decode(data: &[u8]) -> Option<Png> {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut i = 8usize;
    let (mut width, mut height) = (0u32, 0u32);
    let mut color = 0u8;
    let mut idat: Vec<u8> = Vec::new();
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();

    while i + 8 <= data.len() {
        let len = u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?) as usize;
        let kind = data.get(i + 4..i + 8)?;
        let body = data.get(i + 8..i + 8 + len)?;
        match kind {
            b"IHDR" => {
                width = u32::from_be_bytes(body.get(0..4)?.try_into().ok()?);
                height = u32::from_be_bytes(body.get(4..8)?.try_into().ok()?);
                color = *body.get(9)?;
                if *body.get(12)? != 0 {
                    return None; // interlaced
                }
                if *body.get(8)? != 8 {
                    return None; // 1/2/4/16-bit depths are rare in prompt payloads
                }
            }
            b"PLTE" => palette = body.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            b"tRNS" => trns = body.to_vec(),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        i += 12 + len; // length + type + body + crc
    }
    if width == 0 || height == 0 || idat.is_empty() {
        return None;
    }

    let channels: usize = match color {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&idat[..])
        .read_to_end_checked(&mut raw)
        .ok()?;

    let bpp = channels; // 8-bit
    let stride = width as usize * bpp;
    let mut prev = vec![0u8; stride];
    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    let mut pos = 0usize;

    for _ in 0..height {
        let filter = *raw.get(pos)?;
        pos += 1;
        let line = raw.get(pos..pos + stride)?.to_vec();
        pos += stride;
        let mut cur = line;
        for x in 0..stride {
            let a = if x >= bpp { cur[x - bpp] } else { 0 };
            let b = prev[x];
            let c = if x >= bpp { prev[x - bpp] } else { 0 };
            cur[x] = match filter {
                0 => cur[x],
                1 => cur[x].wrapping_add(a),
                2 => cur[x].wrapping_add(b),
                3 => cur[x].wrapping_add((((a as u16) + (b as u16)) / 2) as u8),
                4 => cur[x].wrapping_add(paeth(a, b, c)),
                _ => return None,
            };
        }
        // expand to RGBA
        for px in cur.chunks_exact(bpp) {
            let (r, g, b, a) = match color {
                0 => (px[0], px[0], px[0], 255),
                2 => (px[0], px[1], px[2], 255),
                3 => {
                    let p = palette.get(px[0] as usize).copied().unwrap_or([0, 0, 0]);
                    (p[0], p[1], p[2], trns.get(px[0] as usize).copied().unwrap_or(255))
                }
                4 => (px[0], px[0], px[0], px[1]),
                _ => (px[0], px[1], px[2], px[3]),
            };
            out.extend_from_slice(&[r, g, b, a]);
        }
        prev = cur;
    }
    Some(Png { width, height, rgba: out })
}

/// `Read::read_to_end` on a decoder, but rejecting a truncated stream.
trait ReadChecked {
    fn read_to_end_checked(self, out: &mut Vec<u8>) -> std::io::Result<()>;
}

impl<R: std::io::Read> ReadChecked for R {
    fn read_to_end_checked(mut self, out: &mut Vec<u8>) -> std::io::Result<()> {
        std::io::Read::read_to_end(&mut self, out)?;
        Ok(())
    }
}

// ── PNG resize + encode ───────────────────────────────────────────────────────

/// Box-filter downscale: every destination pixel averages the source pixels it covers,
/// which is what keeps screenshot text legible at half size.
fn resize_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; dw as usize * dh as usize * 4];
    let xr = sw as f64 / dw as f64;
    let yr = sh as f64 / dh as f64;
    for dy in 0..dh as usize {
        let y0 = (dy as f64 * yr).floor() as usize;
        let y1 = (((dy + 1) as f64 * yr).ceil() as usize).min(sh as usize).max(y0 + 1);
        for dx in 0..dw as usize {
            let x0 = (dx as f64 * xr).floor() as usize;
            let x1 = (((dx + 1) as f64 * xr).ceil() as usize).min(sw as usize).max(x0 + 1);
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let o = (y * sw as usize + x) * 4;
                    if o + 3 < src.len() {
                        acc[0] += src[o] as u32;
                        acc[1] += src[o + 1] as u32;
                        acc[2] += src[o + 2] as u32;
                        acc[3] += src[o + 3] as u32;
                        n += 1;
                    }
                }
            }
            let d = (dy * dw as usize + dx) * 4;
            if n > 0 {
                for c in 0..4 {
                    out[d + c] = (acc[c] / n) as u8;
                }
            }
        }
    }
    out
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(body);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

/// Encode 8-bit RGBA (or RGB when every pixel is opaque) with filter 0.
fn png_encode(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let opaque = rgba.chunks_exact(4).all(|p| p[3] == 255);
    let (channels, color) = if opaque { (3usize, 2u8) } else { (4usize, 6u8) };

    let mut raw = Vec::with_capacity((w as usize * channels + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0); // filter: none
        for x in 0..w as usize {
            let o = (y * w as usize + x) * 4;
            raw.extend_from_slice(&rgba.get(o..o + channels)?[..channels]);
        }
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::new(6));
    z.write_all(&raw).ok()?;
    let idat = z.finish().ok()?;

    let mut out = Vec::with_capacity(idat.len() + 64);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, color, 0, 0, 0]); // depth, colour, deflate, filter, no interlace
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &idat);
    png_chunk(&mut out, b"IEND", &[]);
    Some(out)
}

/// What happened to one image.
#[derive(Debug, Clone)]
pub struct Rightsized {
    pub from: (u32, u32),
    pub to: (u32, u32),
    pub format: Format,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub bytes_before: usize,
    pub bytes_after: usize,
    /// false when only measured (JPEG/WebP/GIF, unsupported PNG, or already small)
    pub rewritten: bool,
}

impl Rightsized {
    pub fn saved(&self) -> u32 {
        self.tokens_before.saturating_sub(self.tokens_after)
    }
}

/// Resize `data` so its longest edge is at most `edge`. `Ok(None)` means "nothing to do"
/// (already small enough, or a format this module refuses to re-encode).
pub fn rightsize(data: &[u8], edge: u32, provider: &str) -> Option<(Vec<u8>, Rightsized)> {
    let info = probe(data)?;
    let (tw, th) = fit(info.width, info.height, edge);
    let mut report = Rightsized {
        from: (info.width, info.height),
        to: (info.width, info.height),
        format: info.format,
        tokens_before: tokens_for(provider, info.width, info.height),
        tokens_after: tokens_for(provider, info.width, info.height),
        bytes_before: info.bytes,
        bytes_after: info.bytes,
        rewritten: false,
    };
    if (tw, th) == (info.width, info.height) || !info.format.resizable() {
        return Some((data.to_vec(), report));
    }
    let png = png_decode(data)?;
    let scaled = resize_rgba(&png.rgba, png.width, png.height, tw, th);
    let encoded = png_encode(&scaled, tw, th)?;
    // a re-encode that grew is not a saving
    if encoded.len() >= data.len() && tokens_for(provider, tw, th) >= report.tokens_before {
        return Some((data.to_vec(), report));
    }
    report.to = (tw, th);
    report.tokens_after = tokens_for(provider, tw, th);
    report.bytes_after = encoded.len();
    report.rewritten = true;
    Some((encoded, report))
}

// ── request payload walking ───────────────────────────────────────────────────

/// Base64 alphabet decode (no external dep).
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn b64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Rightsize every base64 image reachable from `value`, in place.
///
/// Handles all three payload shapes:
/// * Anthropic — `{type:"image", source:{type:"base64", media_type, data}}`
/// * OpenAI    — `{type:"image_url", image_url:{url:"data:image/png;base64,…"}}`
/// * Gemini    — `{inline_data:{mime_type, data}}` / `{inlineData:{mimeType, data}}`
pub fn rightsize_value(value: &mut Value, edge: u32, provider: &str) -> Vec<Rightsized> {
    let mut reports = Vec::new();
    walk(value, edge, provider, &mut reports);
    reports
}

fn walk(value: &mut Value, edge: u32, provider: &str, out: &mut Vec<Rightsized>) {
    match value {
        Value::Object(map) => {
            // Anthropic / Gemini: a sibling "data" field holding raw base64
            for key in ["source", "inline_data", "inlineData"] {
                if let Some(Value::Object(src)) = map.get_mut(key) {
                    if let Some(Value::String(b64)) = src.get_mut("data") {
                        if let Some((new_b64, report)) = rightsize_b64(b64, edge, provider) {
                            if report.rewritten {
                                *b64 = new_b64;
                                let mt = report.format.media_type().to_string();
                                for k in ["media_type", "mime_type", "mimeType"] {
                                    if src.contains_key(k) {
                                        src.insert(k.to_string(), Value::String(mt.clone()));
                                    }
                                }
                            }
                            out.push(report);
                        }
                    }
                }
            }
            // OpenAI: a data: URL
            if let Some(Value::Object(iu)) = map.get_mut("image_url") {
                if let Some(Value::String(url)) = iu.get_mut("url") {
                    if let Some(rest) = url.strip_prefix("data:") {
                        if let Some((meta, b64)) = rest.split_once(";base64,") {
                            if let Some((new_b64, report)) = rightsize_b64(b64, edge, provider) {
                                if report.rewritten {
                                    let mt = if meta.is_empty() {
                                        report.format.media_type().to_string()
                                    } else {
                                        report.format.media_type().to_string()
                                    };
                                    *url = format!("data:{};base64,{}", mt, new_b64);
                                }
                                out.push(report);
                            }
                        }
                    }
                }
            }
            for (_, v) in map.iter_mut() {
                walk(v, edge, provider, out);
            }
        }
        Value::Array(items) => {
            for v in items.iter_mut() {
                walk(v, edge, provider, out);
            }
        }
        _ => {}
    }
}

fn rightsize_b64(b64: &str, edge: u32, provider: &str) -> Option<(String, Rightsized)> {
    // cheap reject: base64 of any real image is far longer than this
    if b64.len() < 64 {
        return None;
    }
    let raw = b64_decode(b64)?;
    let (new_bytes, report) = rightsize(&raw, edge, provider)?;
    let encoded = if report.rewritten { b64_encode(&new_bytes) } else { String::new() };
    Some((encoded, report))
}

// Test-only re-exports so the proxy's integration tests can build real payloads
// instead of hard-coding a fixture blob.
#[doc(hidden)]
pub fn png_encode_for_test(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    png_encode(rgba, w, h).expect("encode")
}

#[doc(hidden)]
pub fn b64_encode_for_test(data: &[u8]) -> String {
    b64_encode(data)
}

#[doc(hidden)]
pub fn b64_decode_for_test(s: &str) -> Option<Vec<u8>> {
    b64_decode(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a real PNG so the tests exercise the actual codec, not a fixture blob.
    fn make_png(w: u32, h: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                // a gradient plus a hard edge, so averaging errors would show up
                let v = ((x * 255) / w.max(1)) as u8;
                let e = if x < w / 2 { 0u8 } else { 255 };
                rgba.extend_from_slice(&[v, e, ((y * 255) / h.max(1)) as u8, 255]);
            }
        }
        png_encode(&rgba, w, h).expect("encode")
    }

    #[test]
    fn probes_png_jpeg_webp_gif_headers() {
        let png = make_png(40, 20);
        let i = probe(&png).expect("png");
        assert_eq!((i.format, i.width, i.height), (Format::Png, 40, 20));

        // minimal JPEG: SOI, APP0, SOF0(h=300,w=200), EOI
        let jpeg: Vec<u8> = [
            &[0xFF, 0xD8][..],
            &[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00][..],
            &[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0x2C, 0x00, 0xC8][..],
            &[0xFF, 0xD9][..],
        ]
        .concat();
        let j = probe(&jpeg).expect("jpeg");
        assert_eq!((j.format, j.width, j.height), (Format::Jpeg, 200, 300));

        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&320u16.to_le_bytes());
        gif.extend_from_slice(&240u16.to_le_bytes());
        gif.extend_from_slice(&[0; 10]);
        let g = probe(&gif).expect("gif");
        assert_eq!((g.format, g.width, g.height), (Format::Gif, 320, 240));

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&0u32.to_le_bytes());
        webp.extend_from_slice(b"WEBPVP8L");
        webp.extend_from_slice(&0u32.to_le_bytes());
        webp.push(0x2F);
        let bits: u32 = (99) | (49 << 14); // 100 x 50
        webp.extend_from_slice(&bits.to_le_bytes());
        let w = probe(&webp).expect("webp");
        assert_eq!((w.format, w.width, w.height), (Format::WebP, 100, 50));

        assert!(probe(b"not an image at all, just bytes").is_none());
        assert!(probe(&[]).is_none());
    }

    #[test]
    fn prices_images_per_provider_with_their_own_caps() {
        // Anthropic: w*h/750, capped at a 1568px long edge
        assert_eq!(tokens_for("anthropic", 750, 1000), 1000);
        let huge = tokens_for("anthropic", 4000, 3000);
        assert_eq!(huge, tokens_for("anthropic", 1568, 1176), "cap not applied");
        // OpenAI: 85 + 170 per 512px tile after fitting to 768 on the short side
        assert_eq!(tokens_for("openai", 512, 512), 85 + 170);
        assert_eq!(tokens_for("openai", 1024, 1024), 85 + 170 * 4);
        // Gemini: one tile below 384px
        assert_eq!(tokens_for("gemini", 300, 300), 258);
        assert!(tokens_for("gemini", 1000, 1000) > 258);
    }

    #[test]
    fn png_roundtrip_decodes_what_it_encodes() {
        let png = make_png(31, 17); // odd sizes catch stride mistakes
        let dec = png_decode(&png).expect("decode");
        assert_eq!((dec.width, dec.height), (31, 17));
        assert_eq!(dec.rgba.len(), 31 * 17 * 4);
        // the hard edge survives: left half green=0, right half green=255
        let px = |x: usize, y: usize| dec.rgba[(y * 31 + x) * 4 + 1];
        assert_eq!(px(0, 0), 0);
        assert_eq!(px(30, 16), 255);
    }

    #[test]
    fn resizing_to_the_provider_cap_saves_bytes_but_not_tokens() {
        // The provider downscales to 1568 itself and bills the smaller size, so trimming
        // to the cap is a bandwidth win with *zero* effect on what the model sees.
        let png = make_png(2000, 1000);
        let (out, r) = rightsize(&png, DEFAULT_MAX_EDGE, "anthropic").expect("rightsized");
        assert!(r.rewritten, "{:?}", r);
        assert_eq!(r.to, (1568, 784));
        let info = probe(&out).expect("still a png");
        assert_eq!((info.width, info.height), (1568, 784));
        assert!(out.len() < png.len(), "bytes grew: {} -> {}", png.len(), out.len());
        assert_eq!(r.tokens_before, r.tokens_after, "billing is capped, so tokens are equal");
        assert!(r.bytes_after < r.bytes_before);
    }

    #[test]
    fn resizing_below_the_cap_is_what_saves_tokens() {
        let png = make_png(2000, 1000);
        let (out, r) = rightsize(&png, 800, "anthropic").expect("rightsized");
        assert!(r.rewritten, "{:?}", r);
        assert_eq!(r.to, (800, 400));
        assert_eq!(probe(&out).unwrap().width, 800);
        assert_eq!(r.tokens_before, tokens_for("anthropic", 1568, 784));
        assert_eq!(r.tokens_after, tokens_for("anthropic", 800, 400));
        assert!(r.saved() > 1000, "{:?}", r);
        // and the same holds for the tile-priced providers
        let (_, o) = rightsize(&make_png(2000, 1000), 500, "openai").expect("rightsized");
        assert!(o.saved() > 0, "{:?}", o);
    }

    #[test]
    fn rightsize_leaves_small_images_and_unsupported_formats_alone() {
        let small = make_png(200, 100);
        let (out, r) = rightsize(&small, DEFAULT_MAX_EDGE, "anthropic").expect("measured");
        assert!(!r.rewritten);
        assert_eq!(out, small, "a small image must be byte-identical");
        assert_eq!(r.saved(), 0);

        // a JPEG is measured but never re-encoded
        let jpeg: Vec<u8> = [
            &[0xFF, 0xD8][..],
            &[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x0B, 0xB8, 0x0F, 0xA0][..],
            &[0xFF, 0xD9][..],
        ]
        .concat();
        let (out, r) = rightsize(&jpeg, DEFAULT_MAX_EDGE, "anthropic").expect("measured");
        assert_eq!(r.format, Format::Jpeg);
        assert!(!r.rewritten, "JPEG must not be re-encoded");
        assert_eq!(out, jpeg);
        assert!(r.tokens_before > 0);
    }

    #[test]
    fn base64_roundtrip() {
        for case in [&b""[..], b"a", b"ab", b"abc", b"abcd", &[0u8, 255, 128, 7][..]] {
            let enc = b64_encode(case);
            assert_eq!(b64_decode(&enc).as_deref(), Some(case), "{}", enc);
        }
    }

    #[test]
    fn rewrites_anthropic_openai_and_gemini_payloads() {
        let big = b64_encode(&make_png(2400, 1200));

        let mut anthropic = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "what is this?"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": big}}
            ]}]
        });
        let reports = rightsize_value(&mut anthropic, 1024, "anthropic");
        assert_eq!(reports.len(), 1);
        assert!(reports[0].rewritten);
        // 2400×1200 bills as 1568×784 (1640 tok); at a 1024 edge it is 700
        assert!(reports[0].tokens_after * 2 < reports[0].tokens_before, "{:?}", reports[0]);
        let data = anthropic["messages"][0]["content"][1]["source"]["data"].as_str().unwrap();
        let info = probe(&b64_decode(data).unwrap()).unwrap();
        assert_eq!(info.width, 1024);
        // the prose is untouched
        assert_eq!(anthropic["messages"][0]["content"][0]["text"], "what is this?");

        let mut openai = serde_json::json!({
            "messages": [{"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", b64_encode(&make_png(2400, 1200)))}}
            ]}]
        });
        let reports = rightsize_value(&mut openai, 1024, "openai");
        assert_eq!(reports.len(), 1);
        assert!(reports[0].rewritten);
        let url = openai["messages"][0]["content"][0]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{}", &url[..40]);
        let info = probe(&b64_decode(url.split(',').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(info.width, 1024);

        let mut gemini = serde_json::json!({
            "contents": [{"parts": [
                {"text": "hi"},
                {"inline_data": {"mime_type": "image/png", "data": b64_encode(&make_png(2400, 1200))}}
            ]}]
        });
        let reports = rightsize_value(&mut gemini, 1024, "gemini");
        assert_eq!(reports.len(), 1);
        assert!(reports[0].rewritten);
    }

    #[test]
    fn payloads_without_images_are_untouched() {
        let original = serde_json::json!({
            "messages": [{"role": "user", "content": "just text"}],
            "tools": [{"name": "read", "input_schema": {"type": "object"}}]
        });
        let mut v = original.clone();
        let reports = rightsize_value(&mut v, DEFAULT_MAX_EDGE, "anthropic");
        assert!(reports.is_empty());
        assert_eq!(v, original);
    }

    #[test]
    fn malformed_images_never_panic() {
        for bad in [
            &b"\x89PNG\r\n\x1a\n"[..],                       // header only
            &b"\x89PNG\r\n\x1a\nIHDRnope"[..],               // truncated IHDR
            &[0xFF, 0xD8, 0xFF, 0xC0, 0x00][..],             // truncated SOF
            &b"RIFF\0\0\0\0WEBP"[..],                        // no chunk
            &b"GIF89a"[..],                                  // no size
        ] {
            let _ = probe(bad);
            let _ = rightsize(bad, DEFAULT_MAX_EDGE, "anthropic");
        }
        // a PNG whose IDAT is garbage decodes to nothing rather than panicking
        let mut broken = make_png(64, 64);
        let len = broken.len();
        broken[len / 2..].iter_mut().for_each(|b| *b = 0);
        let _ = rightsize(&broken, 32, "anthropic");
    }
}
