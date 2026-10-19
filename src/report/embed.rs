//! `--embed-artifacts`: the files a report links to, carried inside the page.
//!
//! The artifact on disk is only ever read. What goes into the page is the
//! smaller of the file and its re-encoding, so the cap `--embed-limit` counts
//! on-disk bytes and the page can only come out under it.

use super::model::{Item, Run};
use anyhow::{Result, bail};
use base64::Engine;
use clap::ValueEnum;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Component, Path};

const DEFAULT_LIMIT: u64 = 20_000_000;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The most a WebP bitstream can say for either side.
const WEBP_MAX_SIDE: u32 = 16383;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Images {
    /// 8-bit PNG as lossless WebP, exact pixels
    Lossless,
    /// 8-bit PNG as lossy WebP at --embed-quality
    Lossy,
    /// every file as it is
    Original,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub limit: u64,
    pub images: Images,
    pub quality: u8,
}

impl Options {
    /// The flags as given, each `None` being "not passed". A quality that
    /// would change nothing is refused, the way every option that applies to
    /// nothing is.
    pub fn from_flags(
        limit: Option<u64>,
        images: Option<Images>,
        quality: Option<u8>,
    ) -> Result<Self> {
        let images = images.unwrap_or(Images::Lossless);
        if quality.is_some() && images != Images::Lossy {
            bail!("--embed-quality only applies to --embed-images lossy");
        }
        Ok(Self {
            limit: limit.unwrap_or(DEFAULT_LIMIT),
            images,
            quality: quality.unwrap_or(80),
        })
    }
}

/// `20MB`, `500kb`, `1048576`: whole numbers, decimal units, bytes when bare.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let (number, unit) = text.split_at(digits);
    let factor: u64 = match unit.to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        _ => {
            return Err(format!(
                "`{text}` is not a size: use a whole number and B, KB, MB or GB, e.g. 20MB"
            ));
        }
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
        .ok_or_else(|| {
            format!("`{text}` is not a size: use a whole number and B, KB, MB or GB, e.g. 20MB")
        })
}

/// Decimal, one place: `12.4 MB`. The same units `--embed-limit` reads.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Embedded {
    /// base64
    pub data: String,
    pub media_type: String,
    /// The file's own size on disk.
    pub size: u64,
    /// `original`, `webp-lossless` or `webp-lossy`.
    pub encoding: &'static str,
    #[serde(skip)]
    pub stored: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Skipped {
    pub size: u64,
    /// `limit`, `not found` or `unsafe path`.
    pub reason: &'static str,
}

/// Flattened into the page; both maps absent when empty, so a report made
/// without the flag keeps the bytes it always had.
#[derive(Default, Serialize)]
pub struct Embeds {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub embedded: BTreeMap<String, Embedded>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub not_embedded: BTreeMap<String, Skipped>,
}

impl Embeds {
    /// The clause the summary line adds when embedding was asked for.
    pub fn summary(&self, images: Images) -> String {
        let name = match images {
            Images::Lossless => "lossless",
            Images::Lossy => "lossy",
            Images::Original => "original",
        };
        let before: u64 = self.embedded.values().map(|e| e.size).sum();
        let after: u64 = self.embedded.values().map(|e| e.stored).sum();
        format!(
            ", {} files embedded ({name}, {} → {}), {} left as links",
            self.embedded.len(),
            human(before),
            human(after),
            self.not_embedded.len()
        )
    }
}

struct Reference {
    size: u64,
    media_type: Option<String>,
}

/// Every object with a string `path` and a numeric `size`, anywhere in an
/// evidence or attempt value: the set the viewer passes to `link()`.
fn references(run: &Run) -> BTreeMap<String, Reference> {
    fn walk(value: &Value, out: &mut BTreeMap<String, Reference>) {
        match value {
            Value::Object(map) => {
                if let (Some(Value::String(path)), Some(size)) =
                    (map.get("path"), map.get("size").and_then(Value::as_u64))
                {
                    let entry = out.entry(path.clone()).or_insert(Reference {
                        size,
                        media_type: None,
                    });
                    if entry.media_type.is_none() {
                        entry.media_type = map
                            .get("media_type")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                map.values().for_each(|v| walk(v, out));
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    fn items(list: &[Item], out: &mut BTreeMap<String, Reference>) {
        for item in list {
            item.evidence
                .iter()
                .chain(&item.attempts)
                .for_each(|v| walk(v, out));
            items(&item.children, out);
        }
    }
    let mut out = BTreeMap::new();
    for file in &run.files {
        for scenario in &file.scenarios {
            items(&scenario.items, &mut out);
        }
    }
    out
}

/// Relative, and every component a plain name: nothing a crafted stream could
/// use to pull a file from outside the artifact root into a page that is mailed.
fn safe(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && path.split('/').all(|s| !s.is_empty())
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// What `plan` decided: the files to read, smallest first, and the rest.
pub struct Plan {
    chosen: Vec<(String, Option<String>)>,
    not_embedded: BTreeMap<String, Skipped>,
}

impl Plan {
    pub fn files(&self) -> usize {
        self.chosen.len()
    }
}

/// Chooses by on-disk size, smallest first, ties by path, and ends at the
/// first file that does not fit.
pub fn plan(run: &Run, root: Option<&Path>, limit: u64) -> Plan {
    let mut not_embedded = BTreeMap::new();
    let mut candidates: Vec<(u64, String, Option<String>)> = Vec::new();
    let canonical_root = root.and_then(|r| r.canonicalize().ok());
    for (path, reference) in references(run) {
        let skip = |reason| Skipped {
            size: reference.size,
            reason,
        };
        if !safe(&path) {
            not_embedded.insert(path, skip("unsafe path"));
            continue;
        }
        // Resolved, then required to stay under the resolved root: a link
        // anywhere in the path, not only at its end, must not lead out of it.
        let on_disk = canonical_root
            .as_ref()
            .and_then(|root| {
                let full = root.join(&path).canonicalize().ok()?;
                full.starts_with(root).then_some(full)
            })
            .and_then(|full| std::fs::metadata(full).ok())
            .filter(|m| m.is_file())
            .map(|m| m.len());
        match on_disk {
            Some(size) => candidates.push((size, path, reference.media_type)),
            None => {
                not_embedded.insert(path, skip("not found"));
            }
        }
    }
    candidates.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let mut total = 0u64;
    let mut chosen = Vec::new();
    let mut full = false;
    for (size, path, media_type) in candidates {
        if !full && limit > 0 && total.saturating_add(size) <= limit {
            total += size;
            chosen.push((path, media_type));
        } else {
            full = true;
            not_embedded.insert(
                path,
                Skipped {
                    size,
                    reason: "limit",
                },
            );
        }
    }
    Plan {
        chosen,
        not_embedded,
    }
}

/// 8-bit pixels of a PNG, grey and palette already expanded.
struct Pixels {
    data: Vec<u8>,
    width: u32,
    height: u32,
    color: png::ColorType,
}

/// `None` for anything that must stay as it is: not a PNG, 16 bits, animated,
/// or not decodable.
fn decode_png(bytes: &[u8]) -> Option<Pixels> {
    if !bytes.starts_with(PNG_SIGNATURE) {
        return None;
    }
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    // A WebP of the first frame would silently freeze an APNG.
    if reader.info().animation_control.is_some() {
        return None;
    }
    let mut data = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut data).ok()?;
    if frame.bit_depth != png::BitDepth::Eight || frame.color_type == png::ColorType::Indexed {
        return None;
    }
    data.truncate(frame.buffer_size());
    Some(Pixels {
        data,
        width: frame.width,
        height: frame.height,
        color: frame.color_type,
    })
}

fn lossless(px: &Pixels) -> Option<Vec<u8>> {
    use image_webp::ColorType::{L8, La8, Rgb8, Rgba8};
    let color = match px.color {
        png::ColorType::Grayscale => L8,
        png::ColorType::GrayscaleAlpha => La8,
        png::ColorType::Rgb => Rgb8,
        _ => Rgba8,
    };
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(&mut out)
        .encode(&px.data, px.width, px.height, color)
        .ok()?;
    Some(out)
}

fn lossy(px: &Pixels, quality: u8) -> Option<Vec<u8>> {
    if px.width > WEBP_MAX_SIDE || px.height > WEBP_MAX_SIDE {
        return None;
    }
    let quality = f32::from(quality);
    let encoded = match px.color {
        png::ColorType::Rgb => {
            webp::Encoder::from_rgb(&px.data, px.width, px.height).encode(quality)
        }
        png::ColorType::Rgba => {
            webp::Encoder::from_rgba(&px.data, px.width, px.height).encode(quality)
        }
        grey => {
            let step = if grey == png::ColorType::Grayscale {
                1
            } else {
                2
            };
            let mut rgba = Vec::with_capacity(px.data.len() / step * 4);
            for p in px.data.chunks_exact(step) {
                rgba.extend_from_slice(&[p[0], p[0], p[0], if step == 2 { p[1] } else { 255 }]);
            }
            webp::Encoder::from_rgba(&rgba, px.width, px.height).encode(quality)
        }
    };
    (!encoded.is_empty()).then(|| encoded.to_vec())
}

/// The re-encoding and its label, only when it is smaller than the file.
fn reencode(bytes: &[u8], opts: &Options) -> Option<(Vec<u8>, &'static str)> {
    if opts.images == Images::Original {
        return None;
    }
    let px = decode_png(bytes)?;
    let (out, label) = match opts.images {
        Images::Lossy => (lossy(&px, opts.quality)?, "webp-lossy"),
        _ => (lossless(&px)?, "webp-lossless"),
    };
    (out.len() < bytes.len()).then_some((out, label))
}

fn embed_one(
    root: &Path,
    path: &str,
    media_type: Option<&str>,
    opts: &Options,
) -> Option<Embedded> {
    let bytes = std::fs::read(root.join(path)).ok()?;
    let declared = media_type.map_or_else(
        || {
            mime_guess::from_path(path)
                .first_or_octet_stream()
                .essence_str()
                .to_owned()
        },
        str::to_owned,
    );
    let size = bytes.len() as u64;
    let (stored, media_type, encoding) = match reencode(&bytes, opts) {
        Some((out, label)) => (out, "image/webp".to_owned(), label),
        None => (bytes, declared, "original"),
    };
    Some(Embedded {
        data: base64::engine::general_purpose::STANDARD.encode(&stored),
        media_type,
        size,
        encoding,
        stored: stored.len() as u64,
    })
}

/// Reads and encodes the chosen files on `threads` threads. Workers claim
/// indices from one counter and the result is assembled in plan order, so the
/// page is the same bytes however many threads there were. `tick` is called
/// once per finished file.
pub fn encode(
    plan: Plan,
    root: Option<&Path>,
    opts: &Options,
    threads: usize,
    tick: &(dyn Fn() + Sync),
) -> Embeds {
    let Plan {
        chosen,
        mut not_embedded,
    } = plan;
    let mut embedded = BTreeMap::new();
    let Some(root) = root else {
        return Embeds {
            embedded,
            not_embedded,
        };
    };
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut slots: Vec<(usize, Option<Embedded>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.clamp(1, chosen.len().max(1)))
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((path, media_type)) = chosen.get(i) else {
                            break done;
                        };
                        done.push((i, embed_one(root, path, media_type.as_deref(), opts)));
                        tick();
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("an encoder thread panicked"))
            .collect()
    });
    slots.sort_by_key(|(i, _)| *i);
    for (i, result) in slots {
        let path = chosen[i].0.clone();
        match result {
            Some(e) => {
                embedded.insert(path, e);
            }
            // Read failed after the stat: the file went away under us.
            None => {
                not_embedded.insert(
                    path,
                    Skipped {
                        size: 0,
                        reason: "not found",
                    },
                );
            }
        }
    }
    Embeds {
        embedded,
        not_embedded,
    }
}

pub fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::model;

    fn run_naming(events: &[Value]) -> Run {
        let mut stream = String::from(
            "{\"type\":\"run_started\",\"run_id\":\"r1\",\"bddkit\":\"0.3.2\"}\n\
             {\"type\":\"file_started\",\"file\":\"a.feature\",\"name\":\"A\",\"tags\":[]}\n\
             {\"type\":\"scenario_started\",\"file\":\"a.feature\",\"scenario\":0,\"name\":\"s\",\"line\":1,\"tags\":[]}\n\
             {\"type\":\"step_started\",\"file\":\"a.feature\",\"scenario\":0,\"step\":0,\"keyword\":\"Given \",\"text\":\"x\",\"line\":2}\n",
        );
        for e in events {
            stream.push_str(&e.to_string());
            stream.push('\n');
        }
        model::build(stream.as_bytes()).expect("a stream")
    }

    fn shot(path: &str, size: u64) -> Value {
        serde_json::json!({"type":"evidence","file":"a.feature","scenario":0,"step":0,
            "kind":"screenshot","path":path,"size":size,"media_type":"image/png"})
    }

    /// Blocks of flat colour, the shape of a screenshot.
    fn gradient(color: png::ColorType, depth: png::BitDepth, w: u32, h: u32) -> Vec<u8> {
        let channels = color.samples();
        let per = if depth == png::BitDepth::Sixteen {
            2
        } else {
            1
        };
        let mut data = Vec::new();
        for y in 0..h as usize {
            for x in 0..w as usize {
                for c in 0..channels {
                    let v = ((x / 8 + y / 8 * 3 + c * 50) % 256) as u8;
                    data.extend(std::iter::repeat_n(v, per));
                }
            }
        }
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        enc.write_header()
            .expect("header")
            .write_image_data(&data)
            .expect("data");
        out
    }

    /// Noise no encoder can shrink.
    fn noise(w: u32, h: u32) -> Vec<u8> {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let data: Vec<u8> = (0..w * h * 3)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .expect("header")
            .write_image_data(&data)
            .expect("data");
        out
    }

    fn opts(images: Images) -> Options {
        Options::from_flags(None, Some(images), None).expect("opts")
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bddkit-embed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn decode_webp(bytes: &[u8]) -> (Vec<u8>, u32, u32, bool) {
        let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).expect("webp");
        let mut buf = vec![0; d.output_buffer_size().expect("size")];
        d.read_image(&mut buf).expect("pixels");
        let (w, h) = d.dimensions();
        (buf, w, h, d.has_alpha())
    }

    #[test]
    fn sizes_parse_as_the_spec_says() {
        for (text, bytes) in [
            ("20MB", 20_000_000),
            ("20mb", 20_000_000),
            ("500KB", 500_000),
            ("1048576", 1_048_576),
            ("1GB", 1_000_000_000),
            ("0", 0),
        ] {
            assert_eq!(parse_size(text), Ok(bytes), "{text}");
        }
        for bad in ["-1", "1.5MB", "20MiB", "MB", "", "99999999999GB"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_quality_without_lossy_is_refused() {
        assert!(Options::from_flags(None, None, Some(70)).is_err());
        assert!(Options::from_flags(None, Some(Images::Original), Some(70)).is_err());
        assert!(Options::from_flags(None, Some(Images::Lossy), Some(70)).is_ok());
    }

    #[test]
    fn paths_that_leave_the_root_are_not_safe() {
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "a/",
            "./a",
            "a\\b",
            "C:/x",
        ] {
            assert!(!safe(bad), "{bad}");
        }
        assert!(safe("000001/shot.png"));
    }

    #[test]
    fn a_path_named_twice_is_one_candidate_found_in_nested_content() {
        let nested = serde_json::json!({"type":"evidence","file":"a.feature","scenario":0,"step":0,
            "kind":"http","content":{"response":{"body":{"path":"b.bin","size":3}}}});
        let run = run_naming(&[shot("a.png", 5), shot("a.png", 5), nested]);
        let refs = references(&run);
        assert_eq!(refs.keys().collect::<Vec<_>>(), ["a.png", "b.bin"]);
    }

    #[test]
    fn the_cap_takes_smallest_first_and_zero_takes_nothing() {
        let d = scratch("cap");
        for (name, n) in [("a.bin", 30), ("b.bin", 10), ("c.bin", 20), ("d.bin", 10)] {
            std::fs::write(d.join(name), vec![1u8; n]).expect("file");
        }
        let run = run_naming(&["a.bin", "b.bin", "c.bin", "d.bin"].map(|p| shot(p, 1)));
        let p = plan(&run, Some(&d), 40);
        let names: Vec<_> = p.chosen.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["b.bin", "d.bin", "c.bin"]);
        assert_eq!(
            p.not_embedded["a.bin"],
            Skipped {
                size: 30,
                reason: "limit"
            }
        );
        let none = plan(&run, Some(&d), 0);
        assert_eq!(none.files(), 0);
        assert_eq!(none.not_embedded.len(), 4);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_anywhere_in_the_path_cannot_lead_out_of_the_root() {
        let d = scratch("links");
        std::fs::create_dir_all(d.join("art")).expect("art");
        std::fs::create_dir_all(d.join("outside")).expect("outside");
        std::fs::write(d.join("outside/secret"), b"s").expect("secret");
        std::os::unix::fs::symlink(d.join("outside"), d.join("art/1")).expect("dir link");
        std::os::unix::fs::symlink(d.join("outside/secret"), d.join("art/f")).expect("file link");
        let run = run_naming(&[shot("1/secret", 1), shot("f", 1)]);
        let p = plan(&run, Some(&d.join("art")), 1000);
        assert_eq!(p.files(), 0);
        assert_eq!(p.not_embedded.len(), 2);
    }

    #[test]
    fn zero_means_nothing_even_for_an_empty_file() {
        let d = scratch("empty");
        std::fs::write(d.join("e.bin"), b"").expect("file");
        let run = run_naming(&[shot("e.bin", 0)]);
        assert_eq!(plan(&run, Some(&d), 0).files(), 0);
    }

    #[test]
    fn the_first_file_that_does_not_fit_ends_the_selection() {
        let d = scratch("end");
        for (name, n) in [("a.bin", 10), ("b.bin", 30), ("c.bin", 31)] {
            std::fs::write(d.join(name), vec![1u8; n]).expect("file");
        }
        let run = run_naming(&["a.bin", "b.bin", "c.bin"].map(|p| shot(p, 1)));
        // 10 + 30 > 35, and 31 would not fit either; nothing after b is taken.
        let p = plan(&run, Some(&d), 35);
        assert_eq!(p.files(), 1);
        assert_eq!(p.not_embedded.len(), 2);
    }

    #[test]
    fn an_unsafe_path_and_a_missing_file_are_marked_and_the_rest_embeds() {
        let d = scratch("marks");
        std::fs::write(d.join("ok.bin"), b"hello").expect("file");
        let run = run_naming(&[shot("ok.bin", 5), shot("../x", 1), shot("gone.png", 9)]);
        let e = encode(
            plan(&run, Some(&d), 1000),
            Some(&d),
            &opts(Images::Original),
            1,
            &|| {},
        );
        assert_eq!(e.embedded["ok.bin"].encoding, "original");
        assert_eq!(e.not_embedded["../x"].reason, "unsafe path");
        assert_eq!(
            e.not_embedded["gone.png"],
            Skipped {
                size: 9,
                reason: "not found"
            }
        );
    }

    #[test]
    fn original_round_trips_exact_bytes_and_leaves_the_file_alone() {
        let d = scratch("orig");
        let png = gradient(png::ColorType::Rgba, png::BitDepth::Eight, 32, 32);
        std::fs::write(d.join("a.png"), &png).expect("file");
        let run = run_naming(&[shot("a.png", png.len() as u64)]);
        let e = encode(
            plan(&run, Some(&d), 1 << 30),
            Some(&d),
            &opts(Images::Original),
            1,
            &|| {},
        );
        let got = base64::engine::general_purpose::STANDARD
            .decode(&e.embedded["a.png"].data)
            .expect("base64");
        assert_eq!(got, png);
        assert_eq!(e.embedded["a.png"].media_type, "image/png");
        assert_eq!(std::fs::read(d.join("a.png")).expect("read"), png);
    }

    #[test]
    fn lossless_webp_decodes_to_the_exact_pixels_of_each_colour_type() {
        for color in [
            png::ColorType::Rgba,
            png::ColorType::Rgb,
            png::ColorType::Grayscale,
            png::ColorType::GrayscaleAlpha,
        ] {
            let source = gradient(color, png::BitDepth::Eight, 400, 300);
            let px = decode_png(&source).expect("decodes");
            let (out, label) = reencode(&source, &opts(Images::Lossless)).expect("smaller");
            assert_eq!(label, "webp-lossless", "{color:?}");
            let (pixels, w, h, alpha) = decode_webp(&out);
            assert_eq!((w, h), (400, 300));
            let mut want = Vec::new();
            for p in px.data.chunks_exact(color.samples()) {
                match color {
                    png::ColorType::Rgba => want.extend_from_slice(p),
                    png::ColorType::Rgb => want.extend_from_slice(p),
                    png::ColorType::Grayscale => want.extend_from_slice(&[p[0]; 3]),
                    _ => want.extend_from_slice(&[p[0], p[0], p[0], p[1]]),
                }
            }
            assert_eq!(
                alpha,
                matches!(color, png::ColorType::Rgba | png::ColorType::GrayscaleAlpha)
            );
            assert_eq!(pixels, want, "{color:?}");
        }
    }

    #[test]
    fn a_palette_png_is_expanded_and_encoded() {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 300, 300);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_palette(vec![255, 0, 0, 0, 255, 0, 0, 0, 255]);
        let data: Vec<u8> = (0..90_000).map(|i| (i / 300 / 7 % 3) as u8).collect();
        enc.write_header()
            .expect("header")
            .write_image_data(&data)
            .expect("data");
        let (webp, label) = reencode(&out, &opts(Images::Lossless)).expect("smaller");
        assert_eq!(label, "webp-lossless");
        let (pixels, ..) = decode_webp(&webp);
        assert_eq!(&pixels[..3], &[255, 0, 0]);
        assert_eq!(&pixels[7 * 300 * 3..7 * 300 * 3 + 3], &[0, 255, 0]);
    }

    #[test]
    fn a_webp_that_is_not_smaller_leaves_the_original() {
        assert!(reencode(&noise(32, 32), &opts(Images::Lossless)).is_none());
    }

    #[test]
    fn sixteen_bit_corrupt_and_animated_input_stay_original() {
        let deep = gradient(png::ColorType::Rgba, png::BitDepth::Sixteen, 64, 64);
        assert!(reencode(&deep, &opts(Images::Lossless)).is_none());
        let mut corrupt = gradient(png::ColorType::Rgba, png::BitDepth::Eight, 64, 64);
        let cut = corrupt.len() / 2;
        corrupt.truncate(cut);
        assert!(reencode(&corrupt, &opts(Images::Lossless)).is_none());
        assert!(reencode(b"\x89PNG\r\n\x1a\nnot really", &opts(Images::Lossy)).is_none());

        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 64, 64);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_animated(2, 0).expect("apng");
        let mut w = enc.write_header().expect("header");
        for _ in 0..2 {
            w.write_image_data(&vec![7u8; 64 * 64 * 3]).expect("frame");
        }
        w.finish().expect("done");
        assert!(reencode(&out, &opts(Images::Lossless)).is_none());
    }

    #[test]
    fn lossy_is_smaller_decodes_to_the_right_size_and_a_jpeg_stays_original() {
        let source = gradient(png::ColorType::Rgb, png::BitDepth::Eight, 96, 64);
        let o = Options::from_flags(None, Some(Images::Lossy), Some(50)).expect("opts");
        let (out, label) = reencode(&source, &o).expect("smaller");
        assert_eq!(label, "webp-lossy");
        assert!(out.len() < source.len());
        let (_, w, h, _) = decode_webp(&out);
        assert_eq!((w, h), (96, 64));
        assert!(reencode(b"\xff\xd8\xff\xe0 jpeg-ish", &o).is_none());
    }

    #[test]
    fn one_thread_and_many_give_the_same_embeds() {
        let d = scratch("threads");
        let names: Vec<String> = (0..20).map(|i| format!("{i:02}.png")).collect();
        for (i, name) in names.iter().enumerate() {
            let png = gradient(
                png::ColorType::Rgba,
                png::BitDepth::Eight,
                20 + i as u32,
                30,
            );
            std::fs::write(d.join(name), png).expect("file");
        }
        let run = run_naming(&names.iter().map(|n| shot(n, 1)).collect::<Vec<_>>());
        let one = encode(
            plan(&run, Some(&d), 1 << 30),
            Some(&d),
            &opts(Images::Lossless),
            1,
            &|| {},
        );
        let many = encode(
            plan(&run, Some(&d), 1 << 30),
            Some(&d),
            &opts(Images::Lossless),
            8,
            &|| {},
        );
        assert_eq!(one.embedded, many.embedded);
        assert_eq!(one.embedded.len(), 20);
    }

    #[test]
    fn the_summary_clause_counts_and_sizes_decimal() {
        assert_eq!(human(999), "999 B");
        assert_eq!(human(12_400_000), "12.4 MB");
        assert_eq!(
            Embeds::default().summary(Images::Lossless),
            ", 0 files embedded (lossless, 0 B → 0 B), 0 left as links"
        );
    }
}
