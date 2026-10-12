//! PNG: header checks, decoding and validation through the png crate,
//! a chunk walker for the ancillary data the crate never has to decompress, and the two encoders.
//!
//! Every decoder runs with an allocation limit and ignores text and color profile chunks,
//! so a PNG cannot make the decoder inflate metadata; image data is bounded by the checked dimensions.

use super::{Hotspot, ImageError, ImageSize, OxipngPreset, check_dimensions, parse_pair, pixel_under};
use oxipng::{BitDepth as OxiBitDepth, ColorType as OxiColorType, Deflater, Options, RawImage, StripChunks, ZopfliOptions};
use png::{BitDepth, ColorType, Compression, Decoder, Encoder, Limits, Reader, Transformations};
use std::io::Cursor;
use std::iter;
use std::num::{NonZeroU8, NonZeroU64};

pub(super) const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The allocation limit of the decoder: far above the 256 KiB of the largest frame,
/// far below what an untrusted file could request.
const DECODER_LIMIT: usize = 0x0080_0000;

/// Reads the header and checks size and animation, without decoding image data.
pub(super) fn size(data: &[u8], image: usize) -> Result<ImageSize, ImageError> {
	let reader = reader(data, image, Transformations::IDENTITY)?;
	let info = reader.info();
	let size = check_dimensions(image, info.width, info.height)?;
	if info.animation_control.is_some() {
		return Err(ImageError::AnimatedPng { image });
	}
	Ok(size)
}

/// Decodes the image to 8-bit RGBA and clears the color of transparent pixels.
///
/// Reads up to `IEND`, so every chunk is checked, the CRCs of chunks after the image data included.
pub(super) fn decode_rgba(data: &[u8], image: usize, size: ImageSize) -> Result<Vec<u8>, ImageError> {
	let failed = |message: String| ImageError::Decode { image, message };
	let mut reader = reader(
		data,
		image,
		Transformations::EXPAND | Transformations::STRIP_16 | Transformations::ALPHA,
	)?;
	let len = reader
		.output_buffer_size()
		.filter(|&len| len <= DECODER_LIMIT)
		.ok_or_else(|| failed("the image is too large".to_owned()))?;
	let mut buffer = vec![0; len];
	let frame = reader.next_frame(&mut buffer).map_err(|err| failed(err.to_string()))?;
	reader.finish().map_err(|err| failed(err.to_string()))?;
	buffer.truncate(frame.buffer_size());

	let mut pixels = match (frame.color_type, frame.bit_depth) {
		(ColorType::Rgba, BitDepth::Eight) => buffer,
		(ColorType::GrayscaleAlpha, BitDepth::Eight) => {
			let pairs = buffer.as_chunks::<2>().0;
			let mut rgba = Vec::with_capacity(buffer.len().strict_mul(2));
			for &[gray, alpha] in pairs {
				rgba.extend_from_slice(&[gray, gray, gray, alpha]);
			}
			rgba
		},
		(color, depth) => return Err(failed(format!("the decoder returned {color:?} with {depth:?}"))),
	};
	let edge = usize::from(size.get());
	if pixels.len() != edge.strict_mul(edge).strict_mul(4) {
		return Err(failed("the image data does not match the header".to_owned()));
	}
	clear_transparent(&mut pixels);
	Ok(pixels)
}

/// Decodes the image row by row without keeping the pixels, which checks every chunk up to `IEND`.
pub(super) fn validate(data: &[u8], image: usize) -> Result<(), ImageError> {
	let failed = |err: png::DecodingError| ImageError::Decode {
		image,
		message: err.to_string(),
	};
	let mut reader = reader(data, image, Transformations::IDENTITY)?;
	while reader.next_row().map_err(failed)?.is_some() {}
	reader.finish().map_err(failed)
}

/// Appends the PNG without ancillary chunks: `IHDR`, `PLTE`, `tRNS`, `IDAT` and `IEND` stay byte for byte.
pub(super) fn strip_into(png: &[u8], out: &mut Vec<u8>) {
	out.extend_from_slice(png.get(..SIGNATURE.len()).unwrap_or_default());
	for chunk in chunks(png) {
		if matches!(&chunk.kind, b"IHDR" | b"PLTE" | b"tRNS" | b"IDAT" | b"IEND") {
			out.extend_from_slice(chunk.raw);
		}
	}
}

/// Reads the hotspot a PNG declares in a `tEXt` chunk `hotspot` with the value `x,y` or `x y`,
/// in pixels from the top left corner.
pub(super) fn text_hotspot(png: &[u8], image: usize, size: ImageSize) -> Result<Option<Hotspot>, ImageError> {
	let invalid = |value: &[u8], reason| ImageError::InvalidPngHotspot {
		image,
		value: String::from_utf8_lossy(value).into_owned(),
		reason,
	};
	let mut values = chunks(png)
		.filter(|chunk| &chunk.kind == b"tEXt")
		.filter_map(|chunk| chunk.data.strip_prefix(b"hotspot\0"));
	let Some(value) = values.next() else {
		return Ok(None);
	};
	if let Some(second) = values.next() {
		return Err(invalid(second, "appears in more than one tEXt chunk \"hotspot\""));
	}
	let text = str::from_utf8(value)
		.ok()
		.filter(|text| text.is_ascii())
		.ok_or_else(|| invalid(value, "is not ASCII text"))?;
	let (x, y) = parse_pair(text).map_err(|reason| invalid(value, reason))?;
	let edge = f64::from(size.get());
	if x < 0.0 || y < 0.0 {
		return Err(invalid(value, "contains a negative coordinate"));
	}
	if x >= edge || y >= edge {
		return Err(invalid(value, "lies outside of the image"));
	}
	Ok(Some(Hotspot::new(pixel_under(x, size), pixel_under(y, size))))
}

/// Appends RGBA pixels as PNG, compressed quickly; for images that are not optimized.
pub(super) fn encode_fast(pixels: &[u8], size: ImageSize, out: &mut Vec<u8>) -> Result<(), ImageError> {
	let failed = |err: png::EncodingError| ImageError::Encode {
		size: size.pixels(),
		message: err.to_string(),
	};
	let mut encoder = Encoder::new(out, size.pixels(), size.pixels());
	encoder.set_color(ColorType::Rgba);
	encoder.set_depth(BitDepth::Eight);
	encoder.set_compression(Compression::Fast);
	let mut writer = encoder.write_header().map_err(failed)?;
	writer.write_image_data(pixels).map_err(failed)?;
	writer.finish().map_err(failed)
}

/// Encodes RGBA pixels as the smallest PNG oxipng finds with `options`.
pub(super) fn optimize(pixels: Vec<u8>, size: ImageSize, options: &Options) -> Result<Vec<u8>, ImageError> {
	let failed = |message: String| ImageError::Encode {
		size: size.pixels(),
		message,
	};
	let raw = RawImage::new(size.pixels(), size.pixels(), OxiColorType::RGBA, OxiBitDepth::Eight, pixels)
		.map_err(|err| failed(err.to_string()))?;
	raw.create_optimized_png(options).map_err(|err| failed(err.to_string()))
}

/// Returns the oxipng options of a preset, with Zopfli if `iterations` is set.
///
/// Every lossless reduction is on, interlacing and ancillary chunks are off,
/// and broken input is an error; the colors of transparent pixels are already cleared.
#[must_use]
pub(super) fn oxipng_options(preset: OxipngPreset, iterations: Option<NonZeroU8>) -> Options {
	let mut options = Options::from_preset(preset.level());
	options.bit_depth_reduction = true;
	options.color_type_reduction = true;
	options.palette_reduction = true;
	options.grayscale_reduction = true;
	options.optimize_alpha = false;
	options.interlace = Some(false);
	options.strip = StripChunks::All;
	options.fix_errors = false;
	options.timeout = None;
	options.max_decompressed_size = Some(DECODER_LIMIT);
	if let Some(iterations) = iterations {
		options.deflater = Deflater::Zopfli(ZopfliOptions {
			iteration_count: NonZeroU64::from(iterations),
			..ZopfliOptions::default()
		});
	}
	options
}

/// Sets the color of every fully transparent pixel to 0, which Windows never shows.
pub(super) fn clear_transparent(pixels: &mut [u8]) {
	for pixel in pixels.as_chunks_mut::<4>().0 {
		if let [.., 0] = *pixel {
			*pixel = [0; 4];
		}
	}
}

fn reader(data: &[u8], image: usize, transformations: Transformations) -> Result<Reader<Cursor<&[u8]>>, ImageError> {
	let mut decoder = Decoder::new_with_limits(Cursor::new(data), Limits { bytes: DECODER_LIMIT });
	decoder.set_transformations(transformations);
	decoder.set_ignore_iccp_chunk(true);
	decoder.set_ignore_text_chunk(true);
	decoder.read_info().map_err(|err| ImageError::Decode {
		image,
		message: err.to_string(),
	})
}

/// One chunk as stored: its type, its data and the whole chunk with length and CRC.
pub(super) struct Chunk<'a> {
	pub(super) kind: [u8; 4],
	pub(super) data: &'a [u8],
	pub(super) raw: &'a [u8],
}

/// Walks the chunks after the signature up to `IEND` and stops at the first truncated one.
///
/// CRCs are not checked here; every PNG that is kept also goes through the decoder, which checks them.
pub(super) fn chunks(png: &[u8]) -> impl Iterator<Item = Chunk<'_>> {
	let mut rest = png.get(SIGNATURE.len()..).unwrap_or_default();
	iter::from_fn(move || {
		let (&length, after_length) = rest.split_first_chunk::<4>()?;
		let (&kind, after_kind) = after_length.split_first_chunk::<4>()?;
		let len = usize::try_from(u32::from_be_bytes(length)).ok()?;
		let data = after_kind.get(..len)?;
		let (raw, tail) = rest.split_at_checked(len.checked_add(12)?)?;
		rest = if &kind == b"IEND" { &[] } else { tail };
		Some(Chunk { kind, data, raw })
	})
}
