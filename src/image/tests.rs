#![allow(
	clippy::arithmetic_side_effects,
	clippy::as_conversions,
	reason = "Tests run with overflow checks, so every operator is strict there"
)]

use super::container::len32;
use super::*;
use crate::resource::CoffMachine;
use crate::usize_from_u32;
use std::fs;

macro_rules! assert_err {
	($result:expr, $pattern:pat $(if $guard:expr)?) => {
		match $result {
			Err($pattern) $(if $guard)? => {},
			other => panic!("expected {}, got {other:?}", stringify!($pattern)),
		}
	};
}

#[must_use]
fn id(value: u16) -> NonZeroU16 {
	NonZeroU16::new(value).unwrap()
}

#[must_use]
fn size(pixels: u32) -> ImageSize {
	ImageSize::new(pixels).unwrap()
}

#[must_use]
fn key(resource_type: NonZeroU16, name: u16) -> ResourceKey {
	ResourceKey::new(resource_type, id(name), LanguageId::NEUTRAL)
}

#[must_use]
fn fixture(relative: &str) -> Vec<u8> {
	fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(relative)).unwrap()
}

#[must_use]
fn png(edge: u32, rgba: [u8; 4], texts: &[(&str, &str)]) -> Vec<u8> {
	let mut data = Vec::new();
	let mut encoder = png::Encoder::new(&mut data, edge, edge);
	encoder.set_color(png::ColorType::Rgba);
	encoder.set_depth(png::BitDepth::Eight);
	for &(keyword, text) in texts {
		encoder.add_text_chunk(keyword.to_owned(), text.to_owned()).unwrap();
	}
	let mut writer = encoder.write_header().unwrap();
	writer.write_image_data(&rgba.repeat(usize_from_u32(edge * edge))).unwrap();
	writer.finish().unwrap();
	data
}

#[must_use]
fn pixels_of(png: &[u8], edge: u32) -> Vec<u8> {
	codec::decode_rgba(png, 0, size(edge)).unwrap()
}

#[must_use]
fn bitmap_header(edge: u32, bit_count: u16, colors_used: u32) -> Vec<u8> {
	let mut data = Vec::new();
	data.extend_from_slice(&40_u32.to_le_bytes());
	data.extend_from_slice(&edge.to_le_bytes());
	data.extend_from_slice(&(edge * 2).to_le_bytes());
	data.extend_from_slice(&1_u16.to_le_bytes());
	data.extend_from_slice(&bit_count.to_le_bytes());
	data.resize(32, 0);
	data.extend_from_slice(&colors_used.to_le_bytes());
	data.resize(40, 0);
	data
}

fn bit_rows(edge: u32, bit: impl Fn(u32, u32) -> bool) -> Vec<u8> {
	let row_bytes = usize_from_u32(edge.div_ceil(32)) * 4;
	let mut rows = Vec::new();
	for y in (0..edge).rev() {
		let mut row = vec![0_u8; row_bytes];
		for x in (0..edge).filter(|&x| bit(x, y)) {
			row[usize_from_u32(x) / 8] |= 0x80 >> (x % 8);
		}
		rows.extend_from_slice(&row);
	}
	rows
}

/// Returns a bitmap with a color table, XOR rows as given and an AND mask without a set bit.
#[must_use]
fn bitmap(edge: u32, bit_count: u16, colors_used: u32, table: &[u8], xor: &[u8]) -> Vec<u8> {
	[
		bitmap_header(edge, bit_count, colors_used).as_slice(),
		table,
		xor,
		&bit_rows(edge, |_, _| false),
	]
	.concat()
}

fn bitmap_32(edge: u32, bgra: impl Fn(u32, u32) -> [u8; 4], masked: impl Fn(u32, u32) -> bool) -> Vec<u8> {
	let mut data = bitmap_header(edge, 32, 0);
	for y in (0..edge).rev() {
		for x in 0..edge {
			data.extend_from_slice(&bgra(x, y));
		}
	}
	data.extend_from_slice(&bit_rows(edge, masked));
	data
}

fn bitmap_1(edge: u32, white: impl Fn(u32, u32) -> bool, masked: impl Fn(u32, u32) -> bool) -> Vec<u8> {
	let mut data = bitmap_header(edge, 1, 2);
	data.extend_from_slice(&[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0]);
	data.extend_from_slice(&bit_rows(edge, white));
	data.extend_from_slice(&bit_rows(edge, masked));
	data
}

#[must_use]
fn raw_file(file_type: u16, entries: &[(u8, u8, u32, u32)], data: &[u8]) -> Vec<u8> {
	let mut file = Vec::new();
	for word in [0, file_type, u16::try_from(entries.len()).unwrap()] {
		file.extend_from_slice(&word.to_le_bytes());
	}
	for &(width, height, len, offset) in entries {
		// Planes and bit count 0, which a cursor reads as hotspot (0, 0)
		file.extend_from_slice(&[width, height, 0, 0, 0, 0, 0, 0]);
		file.extend_from_slice(&len.to_le_bytes());
		file.extend_from_slice(&offset.to_le_bytes());
	}
	file.extend_from_slice(data);
	file
}

#[must_use]
fn pixel(pixels: &[u8], edge: u32, x: u32, y: u32) -> [u8; 4] {
	let start = usize_from_u32(y * edge + x) * 4;
	pixels[start..start + 4].try_into().unwrap()
}

fn decoded(data: &[u8]) -> Result<Vec<u8>, ImageError> {
	let (_, bitmap) = Bitmap::parse(data, 0)?;
	bitmap.decode(0)
}

#[must_use]
fn word(bytes: &[u8], offset: usize) -> u16 {
	u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

#[must_use]
fn dword(bytes: &[u8], offset: usize) -> u32 {
	u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[must_use]
fn icon() -> Icon {
	Icon::options()
		.create_from_pngs(&[png(16, [1, 2, 3, 255], &[]), png(256, [4, 5, 6, 128], &[])])
		.unwrap()
}

#[must_use]
fn cursor() -> Cursor {
	Cursor::options()
		.hotspot(|size, _| Hotspot::new(1, size.get() - 1))
		.create_from_pngs(&[png(32, [0, 0, 0, 255], &[]), png(256, [9; 4], &[])])
		.unwrap()
}

#[test]
fn sizes_and_presets_have_their_ranges() {
	assert_eq!(ImageSize::new(0), None);
	assert_eq!(ImageSize::new(257), None);
	assert_eq!(ImageSize::new(256), Some(ImageSize::MAX));
	assert_eq!(
		(ImageSize::MAX.get(), ImageSize::MAX.pixels(), ImageSize::MAX.dimension()),
		(256, 256, 0)
	);
	assert_eq!((size(255).dimension(), ImageSize::MIN.dimension()), (255, 1));
	assert!(ImageSize::ICON_SIZES.is_sorted() && ImageSize::CURSOR_SIZES.is_sorted());
	assert_eq!(ImageSize::ICON_SIZES.map(ImageSize::get), [16, 20, 24, 32, 40, 48, 64, 80, 96, 256]);
	assert_eq!(OxipngPreset::new(7), None);
	assert_eq!(OxipngPreset::new(6), Some(OxipngPreset::MAXIMUM));
}

#[test]
fn pngs_load_losslessly_and_round_trip() {
	let inputs = [png(256, [10, 20, 30, 40], &[]), png(16, [1, 2, 3, 0], &[])];
	let icon = Icon::options().create_from_pngs(&inputs).unwrap();
	assert_eq!(icon.sizes().collect::<Vec<_>>(), [size(16), size(256)]);
	assert_eq!(pixels_of(&icon.images[1].png, 256), [10, 20, 30, 40].repeat(256 * 256));
	assert_eq!(
		pixels_of(&icon.images[0].png, 16),
		[0; 4].repeat(16 * 16),
		"Transparent pixels lose their color"
	);
	assert!(icon.images[1].png.len() < inputs[0].len(), "Loading optimizes the images");
	assert_eq!(Icon::options().create_from_ico(&icon.to_ico()).unwrap(), icon);

	let retained = Icon::options()
		.png_optimization(PngOptimization::Retain)
		.create_from_pngs(&inputs)
		.unwrap();
	assert_eq!(&*retained.images[1].png, inputs[0].as_slice(), "Byte for byte");
	assert_eq!(&*retained.images[0].png, inputs[1].as_slice());
}

#[test]
fn the_chain_reproduces_the_canonical_fixtures() {
	let layers = ["16", "32", "256"].map(|edge| fixture(&format!("icon/source/layer_{edge}.png")));
	assert_eq!(Icon::options().create_from_pngs(&layers).unwrap().to_ico(), fixture("icon/app.ico"));
	let second = Icon::options().create_from_ico(&fixture("icon/source/legacy.ico")).unwrap();
	assert_eq!(second.to_ico(), fixture("icon/second.ico"));
	let arrow = Cursor::options().create_from_cur(&fixture("cursor/source/legacy.cur")).unwrap();
	assert_eq!(arrow.to_cur(), fixture("cursor/arrow.cur"));
}

#[test]
fn the_rules_apply_to_every_source() {
	let (large, small) = (png(256, [0; 4], &[]), png(16, [0; 4], &[]));
	let load = |pngs: &[&Vec<u8>]| Icon::options().create_from_pngs(pngs);
	assert_err!(load(&[]), ImageError::NoImages);
	assert_err!(load(&[&small]), ImageError::Missing256);
	assert_err!(
		load(&[&large, &small, &small]),
		ImageError::DuplicateSize {
			first: 1,
			second: 2,
			size: 16
		}
	);
	let mut wide = Vec::new();
	let mut encoder = png::Encoder::new(&mut wide, 2, 1);
	encoder.set_color(png::ColorType::Grayscale);
	let mut writer = encoder.write_header().unwrap();
	writer.write_image_data(&[0, 0]).unwrap();
	writer.finish().unwrap();
	assert_err!(
		load(&[&wide]),
		ImageError::NotSquare {
			image: 0,
			width: 2,
			height: 1
		}
	);
	assert_err!(load(&[&large, &b"GIF89a".to_vec()]), ImageError::UnknownImageFormat { image: 1 });
	assert_err!(
		Icon::options().min_size(size(32)).create_from_pngs(&[&large, &small]),
		ImageError::SizeOutOfRange {
			image: 1,
			size: 16,
			min_size: 32
		}
	);
	assert_err!(
		Icon::options().create_from_pngs(&[vec![0; MAX_INPUT_SIZE], large]),
		ImageError::InputTooLarge { .. }
	);
}

#[test]
fn the_file_directory_is_validated() {
	let parse = |file: &[u8]| container::parse(file).map(|(kind, records)| (kind, records.len()));
	let image = bitmap_32(2, |_, _| [1, 2, 3, 255], |_, _| false);
	let len = u32::try_from(image.len()).unwrap();
	assert_err!(parse(&[0, 0, 1]), ImageError::Truncated { needed: 6, available: 3 });
	assert_err!(parse(&[1, 0, 1, 0, 1, 0]), ImageError::InvalidHeader { reserved: 1, file_type: 1 });
	assert_err!(parse(&[0, 0, 1, 0, 0, 0]), ImageError::NoImages);
	assert_err!(parse(&[0, 0, 2, 0, 1, 1]), ImageError::TooManyImages { count: 257 });
	assert_err!(
		parse(&[0, 0, 1, 0, 1, 0, 16, 16]),
		ImageError::Truncated { needed: 22, available: 8 }
	);
	assert_err!(parse(&raw_file(1, &[(2, 2, 0, 22)], &image)), ImageError::EmptyImage { image: 0 });
	assert_err!(
		parse(&raw_file(1, &[(2, 2, len, 21)], &image)),
		ImageError::ImageOutOfBounds { image: 0 }
	);
	assert_err!(
		parse(&raw_file(1, &[(2, 2, len + 1, 22)], &image)),
		ImageError::ImageOutOfBounds { image: 0 }
	);
	assert_err!(
		parse(&raw_file(1, &[(2, 2, u32::MAX, u32::MAX)], &image)),
		ImageError::ImageOutOfBounds { image: 0 }
	);
	let twice = [image.as_slice(), &image].concat();
	assert_err!(
		parse(&raw_file(1, &[(2, 2, len, 38), (2, 2, len, 38)], &twice)),
		ImageError::OverlappingImages { first: 0, second: 1 }
	);
	assert_err!(parse(&vec![0; MAX_INPUT_SIZE + 1]), ImageError::InputTooLarge { .. });
	assert!(matches!(parse(&raw_file(2, &[(2, 2, len, 22)], &image)), Ok((Kind::Cursor, 1))));

	let gif = raw_file(1, &[(2, 2, 32, 22)], &[b"GIF89a".as_slice(), &[0; 64]].concat());
	assert_err!(Icon::options().create_from_ico(&gif), ImageError::UnknownImageFormat { image: 0 });
	assert_err!(
		Icon::options().create_from_ico(&raw_file(1, &[(4, 4, len, 22)], &image)),
		ImageError::DimensionMismatch {
			image: 0,
			width: 4,
			height: 4,
			size: 2
		}
	);
	assert_err!(
		Icon::options().create_from_ico(&cursor().to_cur()),
		ImageError::WrongFileKind {
			expected: ".ico",
			found: ".cur"
		}
	);
	assert!(Icon::options().allow_cursor_files(true).create_from_ico(&cursor().to_cur()).is_ok());
	assert_err!(
		Cursor::options().create_from_cur(&icon().to_ico()),
		ImageError::WrongFileKind {
			expected: ".cur",
			found: ".ico"
		}
	);
}

#[test]
fn bitmaps_are_read_the_way_windows_draws_them() {
	// Without any alpha value the AND mask decides, and black under a set bit is transparent
	let masked = bitmap_32(8, |x, _| if x < 2 { [0, 0, 0, 0] } else { [0, 0, 255, 0] }, |x, _| x < 2);
	let pixels = decoded(&masked).unwrap();
	assert_eq!((pixel(&pixels, 8, 0, 0), pixel(&pixels, 8, 5, 5)), ([0, 0, 0, 0], [255, 0, 0, 255]));
	// With alpha values the alpha channel decides, not the mask
	let soft = bitmap_32(
		8,
		|x, y| [10, 20, 30, if (x, y) == (0, 0) { 64 } else { 255 }],
		|x, y| (x, y) == (0, 0),
	);
	assert_eq!(pixel(&decoded(&soft).unwrap(), 8, 0, 0), [30, 20, 10, 64]);
	// White under a set mask bit inverts the screen, like the beam of a classic text cursor
	let beam = bitmap_1(8, |x, y| x == 3 && (1..7).contains(&y), |_, _| true);
	assert_err!(decoded(&beam), ImageError::InvertingPixels { image: 0, x: 3, y: 1 });
	assert_eq!(
		pixel(&decoded(&bitmap_1(8, |x, _| x >= 4, |x, _| x < 4)).unwrap(), 8, 7, 3),
		[255; 4]
	);

	// 4 and 8 bits index the color table from the highest bits on
	let indexed_4 = bitmap(2, 4, 2, &[0, 0, 0, 0, 30, 20, 10, 0], &[0x01, 0, 0, 0, 0x10, 0, 0, 0]);
	let nibbles = decoded(&indexed_4).unwrap();
	assert_eq!([pixel(&nibbles, 2, 0, 0), pixel(&nibbles, 2, 1, 1)], [[10, 20, 30, 255]; 2]);
	let indexed_8 = bitmap(2, 8, 0, &[0; 1024], &[0, 255, 0, 0, 255, 0, 0, 0]);
	assert_eq!(pixel(&decoded(&indexed_8).unwrap(), 2, 0, 1), [0, 0, 0, 255]);

	// 16 bits are X1R5G5B5, 24 bits are padded rows of blue, green and red
	let rgb_555 = bitmap(2, 16, 0, &[], &[0x00, 0x7C, 0xFF, 0x03].repeat(2));
	assert_eq!(pixel(&decoded(&rgb_555).unwrap(), 2, 1, 1), [0, 255, 255, 255]);
	let bgr = bitmap(2, 24, 0, &[], &[1, 2, 3, 4, 5, 6, 0, 0].repeat(2));
	assert_eq!(pixel(&decoded(&bgr).unwrap(), 2, 1, 0), [6, 5, 4, 255]);

	let short_table = bitmap(4, 8, 2, &[0; 8], &[0, 1, 2, 0].repeat(4));
	let reason = |bytes: &[u8]| match decoded(bytes) {
		Err(ImageError::InvalidBitmap { reason, .. }) => reason,
		other => panic!("{other:?}"),
	};
	assert_eq!(reason(&short_table), "a pixel refers to a color behind the color table");
	assert_eq!(reason(&[40, 0, 0, 0, 0]), "the header is truncated");
	let with = |offset: usize, value: &[u8]| {
		let mut header = bitmap_header(8, 8, 0);
		header[offset..offset + value.len()].copy_from_slice(value);
		header
	};
	assert_eq!(reason(&with(0, &41_u32.to_le_bytes())), "the header is not a BITMAPINFOHEADER");
	assert_eq!(reason(&with(4, &(-8_i32).to_le_bytes())), "the width is not positive");
	assert_eq!(reason(&with(8, &(-16_i32).to_le_bytes())), "the height is not positive");
	assert_eq!(
		reason(&with(8, &15_u32.to_le_bytes())),
		"the height does not hold an XOR image and an AND mask of the same height"
	);
	assert_eq!(reason(&with(12, &2_u16.to_le_bytes())), "the plane count is not 1");
	assert_eq!(
		reason(&with(14, &2_u16.to_le_bytes())),
		"the bit count is not 1, 4, 8, 16, 24 or 32"
	);
	assert_eq!(
		reason(&with(16, &3_u32.to_le_bytes())),
		"the bitmap is compressed or uses bit fields"
	);
	assert_eq!(
		reason(&with(32, &257_u32.to_le_bytes())),
		"the color table is larger than the bit count allows"
	);
	let mut true_color = bitmap_header(8, 24, 0);
	true_color[32] = 1;
	assert_eq!(reason(&true_color), "the bitmap declares a color table above 8 bits per pixel");
	assert_err!(
		decoded(&bitmap_header(8, 32, 0)),
		ImageError::BitmapTooSmall {
			image: 0,
			needed: 328,
			available: 40
		}
	);
	assert_err!(
		decoded(&bitmap_header(300, 32, 0)),
		ImageError::SizeOutOfRange {
			image: 0,
			size: 300,
			min_size: 1
		}
	);
}

#[test]
fn optimized_files_load_without_optimizing_again() {
	let app = fixture("icon/app.ico");
	let arrow = fixture("cursor/arrow.cur");
	for mode in [PngOptimization::Retain, PngOptimization::Strip] {
		assert_eq!(
			Icon::options().png_optimization(mode).create_from_ico(&app).unwrap().to_ico(),
			app,
			"Byte for byte"
		);
		assert_eq!(
			Cursor::options().png_optimization(mode).create_from_cur(&arrow).unwrap().to_cur(),
			arrow
		);
	}
	let legacy = Icon::options()
		.png_optimization(PngOptimization::Retain)
		.create_from_ico(&fixture("icon/source/legacy.ico"))
		.unwrap();
	assert_eq!(
		Icon::options().create_from_ico(&legacy.to_ico()).unwrap().to_ico(),
		fixture("icon/second.ico")
	);
}

#[test]
fn strip_drops_ancillary_chunks_but_keeps_the_image() {
	let mut original = Vec::new();
	let mut encoder = png::Encoder::new(&mut original, 256, 256);
	encoder.set_color(png::ColorType::Indexed);
	encoder.set_palette(vec![255, 0, 0, 0, 0, 255]);
	encoder.set_trns(vec![255, 0]);
	encoder.set_source_gamma(png::ScaledFloat::new(0.454_55));
	encoder.add_text_chunk("Comment".to_owned(), "metadata".to_owned()).unwrap();
	let mut writer = encoder.write_header().unwrap();
	let indices: Vec<u8> = (0..256 * 256).map(|index| u8::from(index % 3 == 0)).collect();
	writer.write_image_data(&indices).unwrap();
	writer.finish().unwrap();
	let chunks = |png: &[u8]| {
		codec::chunks(png)
			.map(|chunk| String::from_utf8(chunk.kind.to_vec()).unwrap())
			.collect::<Vec<_>>()
	};
	let load = |mode| Icon::options().png_optimization(mode).create_from_pngs(&[&original]).unwrap();
	let retained = load(PngOptimization::Retain);
	assert_eq!(&*retained.images[0].png, original.as_slice(), "Byte for byte");
	assert_eq!(
		chunks(&retained.images[0].png),
		["IHDR", "gAMA", "PLTE", "tRNS", "tEXt", "IDAT", "IEND"]
	);
	let stripped = load(PngOptimization::Strip);
	assert_eq!(chunks(&stripped.images[0].png), ["IHDR", "PLTE", "tRNS", "IDAT", "IEND"]);
	let optimized = |icon: &Icon| Icon::options().create_from_ico(&icon.to_ico()).unwrap().to_ico();
	assert_eq!(optimized(&stripped), optimized(&retained), "The same pixels");
	assert_eq!(
		chunks(&original[..original.len() - 1]).last().map(String::as_str),
		Some("IDAT"),
		"Truncated"
	);
}

#[test]
fn cheap_checks_run_before_any_pixel_is_decoded() {
	// A broken CRC of the image data only shows when the pixels are decoded
	let corrupt_crc = |png: &mut Vec<u8>| {
		let at = png.len() - 13;
		png[at] ^= 0xFF;
	};
	let len = |data: &[u8]| u32::try_from(data.len()).unwrap();
	let mut broken = png(256, [1, 2, 3, 255], &[]);
	corrupt_crc(&mut broken);
	let intact = png(256, [1, 2, 3, 255], &[]);
	for mode in [DEFAULT_OPTIMIZATION, PngOptimization::Retain] {
		assert_err!(
			Icon::options().png_optimization(mode).create_from_pngs(&[&broken]),
			ImageError::Decode { image: 0, .. }
		);
	}
	let twice = raw_file(
		1,
		&[(0, 0, len(&broken), 38), (0, 0, len(&intact), 38 + len(&broken))],
		&[broken.as_slice(), &intact].concat(),
	);
	assert_err!(
		Icon::options().create_from_ico(&twice),
		ImageError::DuplicateSize {
			first: 0,
			second: 1,
			size: 256
		}
	);

	let mut large = png(256, [0; 4], &[]);
	corrupt_crc(&mut large);
	let small = png(32, [0; 4], &[]);
	let cur = raw_file(
		2,
		&[(32, 32, len(&small), 38), (0, 0, len(&large), 38 + len(&small))],
		&[small.as_slice(), &large].concat(),
	);
	assert_err!(Cursor::options().create_from_cur(&cur), ImageError::Decode { image: 1, .. });
	assert_err!(
		Cursor::options().hotspot(|_, _| Hotspot::new(300, 0)).create_from_cur(&cur),
		ImageError::HotspotOutsideImage {
			image: 0,
			x: 300,
			y: 0,
			size: 32
		}
	);
}

#[test]
fn hotspots_come_from_the_input_or_the_closure() {
	let arrow = fixture("cursor/arrow.cur");
	let (_, records) = container::parse(&arrow).unwrap();
	let from_file: Vec<Hotspot> = records.iter().map(|record| Hotspot::new(record.first, record.second)).collect();
	let loaded = Cursor::options().create_from_cur(&arrow).unwrap();
	assert_eq!(loaded.images().map(|(_, hotspot)| hotspot).collect::<Vec<_>>(), from_file);

	let mut seen = Vec::new();
	let replaced = Cursor::options()
		.hotspot(|size, hotspot| {
			seen.push((size.get(), hotspot));
			Hotspot::new(0, 0)
		})
		.create_from_cur(&arrow)
		.unwrap();
	assert!(replaced.images().all(|(_, hotspot)| hotspot == Hotspot::new(0, 0)));
	assert_eq!(seen.iter().map(|&(_, hotspot)| hotspot.unwrap()).collect::<Vec<_>>(), from_file);
	assert!(seen.windows(2).all(|pair| pair[0].0 < pair[1].0), "Ascending by size");

	let pngs = [png(256, [0; 4], &[]), png(32, [0; 4], &[("hotspot", "31.99 15.5")])];
	assert_err!(Cursor::options().create_from_pngs(&pngs), ImageError::MissingHotspot { image: 0 });
	let mut inputs = Vec::new();
	let cursor = Cursor::options()
		.hotspot(|size, input| {
			inputs.push((size.get(), input));
			input.unwrap_or(Hotspot::new(size.get() - 1, 0))
		})
		.create_from_pngs(&pngs)
		.unwrap();
	assert_eq!(inputs, [(32, Some(Hotspot::new(31, 15))), (256, None)], "Ascending by size");
	assert_eq!(
		cursor.images().collect::<Vec<_>>(),
		[(size(32), Hotspot::new(31, 15)), (ImageSize::MAX, Hotspot::new(255, 0))]
	);
	assert_eq!(
		Cursor::options().create_from_cur(&cursor.to_cur()).unwrap(),
		cursor,
		"The hotspot travels along"
	);

	let reason = |texts: &[(&str, &str)]| match Cursor::options().create_from_pngs(&[png(256, [0; 4], texts)]) {
		Err(ImageError::InvalidPngHotspot { reason, .. }) => reason,
		other => panic!("{other:?}"),
	};
	assert_eq!(reason(&[("hotspot", "4 300")]), "lies outside of the image");
	assert_eq!(reason(&[("hotspot", "-1,2")]), "contains a negative coordinate");
	assert_eq!(reason(&[("hotspot", "4")]), "is not two numbers separated by whitespace or a comma");
	assert_eq!(
		reason(&[("hotspot", "1 2"), ("hotspot", "3 4")]),
		"appears in more than one tEXt chunk \"hotspot\""
	);
	assert_eq!(reason(&[("hotspot", "\u{E9} 2")]), "is not ASCII text");
	assert_err!(
		Cursor::options().hotspot(|_, _| Hotspot::new(256, 0)).create_from_pngs(&[&pngs[0]]),
		ImageError::HotspotOutsideImage {
			image: 0,
			x: 256,
			y: 0,
			size: 256
		}
	);

	assert_eq!(parse_pair("  4 ,\t2.5 "), Ok((4.0, 2.5)));
	assert_eq!(parse_pair("4 2 1"), Err("is not two numbers separated by whitespace or a comma"));
	assert_eq!(parse_pair("4 inf"), Err("contains a number that is not finite"));
	let edge = size(32);
	assert_eq!(
		(
			pixel_under(4.999_999_999_99, edge),
			pixel_under(32.0, edge),
			pixel_under(-1.0, edge)
		),
		(5, 31, 0)
	);
}

#[test]
fn groups_describe_every_image() {
	let mut file = ResourceFile::new();
	file.insert_icon(id(7), LanguageId::NEUTRAL, icon()).unwrap();
	file.insert_cursor(id(7), LanguageId::NEUTRAL, cursor()).unwrap();
	let (_, icon_group) = file.entry(key(RT_GROUP_ICON, 7)).unwrap();
	assert_eq!(icon_group[..6], [0, 0, 1, 0, 2, 0]);
	assert_eq!(
		icon_group[6..14],
		[16, 16, 0, 0, 1, 0, 32, 0],
		"Width, height, colors, reserved, planes and bit count"
	);
	assert_eq!(icon_group[20], 0, "256 pixels are stored as 0");
	assert_eq!((word(icon_group, 18), word(icon_group, 32)), (1, 2), "Image ordinals");
	assert_eq!(dword(icon_group, 14), len32(file.entry(key(RT_ICON, 1)).unwrap().1));

	let (_, cursor_group) = file.entry(key(RT_GROUP_CURSOR, 7)).unwrap();
	assert_eq!(cursor_group[..6], [0, 0, 2, 0, 2, 0]);
	assert_eq!(
		(word(cursor_group, 6), word(cursor_group, 8)),
		(32, 64),
		"WORDs, the height doubled"
	);
	assert_eq!(
		(word(cursor_group, 20), word(cursor_group, 22)),
		(0, 0),
		"256 pixels are stored as 0, as rc.exe writes them"
	);
	let (_, image) = file.entry(key(RT_CURSOR, 1)).unwrap();
	assert_eq!(image[..4], [1, 0, 31, 0], "The hotspot comes first");
	assert_eq!(dword(cursor_group, 14), len32(image), "BytesInRes counts the hotspot");
	assert!(image[4..].starts_with(&codec::SIGNATURE));
}

#[test]
fn image_ordinals_continue_per_type_and_run_out_cleanly() {
	let mut file = ResourceFile::new();
	file.insert_icon(id(1), LanguageId::NEUTRAL, icon()).unwrap();
	file.insert_icon(id(2), LanguageId::NEUTRAL, icon()).unwrap();
	file.insert_cursor(id(1), LanguageId::NEUTRAL, cursor()).unwrap();
	assert_eq!(
		(
			file.max_name(RT_ICON).map(NonZeroU16::get),
			file.max_name(RT_CURSOR).map(NonZeroU16::get)
		),
		(Some(4), Some(2))
	);
	let before = file.to_res();
	assert_eq!(
		file.insert_icon(id(1), LanguageId::NEUTRAL, icon()),
		Err(InsertError::DuplicateResource {
			resource_type: 14,
			name: 1,
			language: 0
		})
	);
	assert_eq!(file.to_res(), before, "A duplicate group inserts no images");

	let mut full = ResourceFile::new();
	full.insert_entries(vec![NewEntry::new(key(RT_ICON, u16::MAX), 0, Box::new([0]))])
		.unwrap();
	let full_before = full.to_res();
	assert_eq!(
		full.insert_icon(id(1), LanguageId::NEUTRAL, icon()),
		Err(InsertError::OutOfImageIds {
			resource_type: 3,
			count: 2
		})
	);
	assert_eq!(full.to_res(), full_before);
}

#[test]
fn aliases_share_the_images_of_their_target() {
	let mut file = ResourceFile::new();
	file.insert_icon(id(1), LanguageId::NEUTRAL, icon()).unwrap();
	file.insert_cursor(id(1), LanguageId::NEUTRAL, cursor()).unwrap();
	let count = file.len();
	file.insert_icon_alias(id(128), LanguageId::NEUTRAL, id(1)).unwrap();
	file.insert_cursor_alias(id(32), LanguageId::NEUTRAL, id(1)).unwrap();
	assert_eq!(file.len(), count + 2, "Only the group directories are added");
	assert_eq!(file.entry(key(RT_GROUP_ICON, 128)), file.entry(key(RT_GROUP_ICON, 1)));
	assert_eq!(file.entry(key(RT_GROUP_CURSOR, 32)), file.entry(key(RT_GROUP_CURSOR, 1)));

	let german = LanguageId::GERMAN;
	assert_eq!(
		file.insert_icon_alias(id(2), german, id(1)),
		Err(InsertError::MissingGroup {
			resource_type: 14,
			name: 1,
			language: 0x0407
		})
	);
	assert_eq!(
		file.insert_icon_alias(id(128), LanguageId::NEUTRAL, id(1)),
		Err(InsertError::DuplicateResource {
			resource_type: 14,
			name: 128,
			language: 0
		})
	);
	assert_eq!(
		file.insert_cursor_alias(id(2), LanguageId::NEUTRAL, id(5)),
		Err(InsertError::MissingGroup {
			resource_type: 12,
			name: 5,
			language: 0
		})
	);
	assert_ne!(file.to_coff(CoffMachine::X64), []);
}

#[cfg(feature = "svg")]
mod svg_source {
	use super::*;

	const RED_SQUARE: &str =
		r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect x="25" y="25" width="50" height="50" fill="red"/></svg>"#;

	#[must_use]
	fn svg_with(attributes: &str, content: &str) -> String {
		format!(r#"<svg xmlns="http://www.w3.org/2000/svg" {attributes}>{content}</svg>"#)
	}

	#[test]
	fn every_size_is_rendered() {
		let document = svg::Document::parse(RED_SQUARE.as_bytes(), false).unwrap();
		let pixels = document.render(ImageSize::MAX, 0).unwrap();
		assert_eq!(
			(pixel(&pixels, 256, 128, 128), pixel(&pixels, 256, 0, 0)),
			([255, 0, 0, 255], [0; 4])
		);

		let icon = Icon::options()
			.create_from_svg(RED_SQUARE.as_bytes(), &ImageSize::ICON_SIZES)
			.unwrap();
		assert_eq!(icon.sizes().collect::<Vec<_>>(), ImageSize::ICON_SIZES);
		let unsorted = Icon::options()
			.create_from_svg(RED_SQUARE.as_bytes(), &[size(256), size(48)])
			.unwrap();
		assert_eq!(unsorted.sizes().collect::<Vec<_>>(), [size(48), size(256)]);
		assert_err!(
			Icon::options().create_from_svg(RED_SQUARE.as_bytes(), &[size(48)]),
			ImageError::Missing256
		);
		// The sizes come before the document
		assert_err!(Icon::options().create_from_svg(b"\xFF", &[size(48)]), ImageError::Missing256);
		assert_err!(
			Icon::options().create_from_svg(RED_SQUARE.as_bytes(), &[size(48), size(48), size(256)]),
			ImageError::DuplicateSize {
				first: 0,
				second: 1,
				size: 48
			}
		);
		let retained = Icon::options()
			.png_optimization(PngOptimization::Retain)
			.create_from_svg(RED_SQUARE.as_bytes(), &[ImageSize::MAX])
			.unwrap();
		assert_eq!(
			pixels_of(&retained.images[0].png, 256),
			pixels,
			"Retain compresses rendered images quickly"
		);
	}

	#[test]
	fn the_hotspot_element_reaches_every_size() {
		let svg = svg_with(
			r#"viewBox="0 0 10 10""#,
			r#"<rect width="10" height="10"/><circle id="hotspot" cx="4" cy="2.14" r="0.5" visibility="hidden"/>"#,
		);
		let mut seen = Vec::new();
		let cursor = Cursor::options()
			.hotspot(|size, hotspot| {
				seen.push(size.get());
				hotspot.unwrap()
			})
			.create_from_svg(svg.as_bytes(), &ImageSize::CURSOR_SIZES)
			.unwrap();
		let scaled: Vec<(u16, u16, u16)> = cursor
			.images()
			.map(|(size, hotspot)| (size.get(), hotspot.x(), hotspot.y()))
			.collect();
		assert_eq!(
			scaled,
			[
				(32, 12, 6),
				(40, 16, 8),
				(48, 19, 10),
				(64, 25, 13),
				(80, 32, 17),
				(96, 38, 20),
				(256, 102, 54)
			]
		);
		assert_eq!(seen, ImageSize::CURSOR_SIZES.map(ImageSize::get));
		let hidden = svg::Document::parse(svg.as_bytes(), false).unwrap().render(size(32), 0).unwrap();
		let plain = svg::Document::parse(
			svg_with(r#"viewBox="0 0 10 10""#, r#"<rect width="10" height="10"/>"#).as_bytes(),
			false,
		)
		.unwrap()
		.render(size(32), 0)
		.unwrap();
		assert_eq!(hidden, plain, "A hidden element is not drawn");

		// Transforms and the viewBox are followed, and the element may be a group
		let centered = svg_with(
			r#"width="20" height="20" viewBox="-5 -5 10 10""#,
			concat!(
				r#"<rect x="-5" y="-5" width="10" height="10"/>"#,
				r#"<g transform="translate(1 1)"><g id="hotspot"><rect x="-1.5" y="-1.5" width="1" height="1" fill="none"/></g></g>"#,
			),
		);
		let centered_cursor = Cursor::options()
			.create_from_svg(centered.as_bytes(), &[size(32), ImageSize::MAX])
			.unwrap();
		assert_eq!(
			centered_cursor.images().map(|(_, hotspot)| hotspot).collect::<Vec<_>>(),
			[Hotspot::new(16, 16), Hotspot::new(128, 128)]
		);

		let outside = svg_with(
			r#"viewBox="0 0 10 10""#,
			r#"<rect id="hotspot" x="11" y="1" width="1" height="1"/>"#,
		);
		assert_err!(
			Cursor::options().create_from_svg(outside.as_bytes(), &[ImageSize::MAX]),
			ImageError::InvalidSvgHotspot { .. }
		);
		assert!(
			Icon::options().create_from_svg(outside.as_bytes(), &[ImageSize::MAX]).is_ok(),
			"Icons ignore the hotspot"
		);
		assert_err!(
			Cursor::options().create_from_svg(RED_SQUARE.as_bytes(), &[ImageSize::MAX]),
			ImageError::MissingHotspot { image: 0 }
		);
	}

	#[test]
	fn the_documented_example_lands_on_its_pixel() {
		const EXAMPLE: &str = concat!(
			r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">"#,
			r#"<rect id="hotspot" x="4" y="7" width="1" height="1" visibility="hidden"/>"#,
			r##"<path d="M4 7 L4 28 L9 23 L13 31 L16 30 L12 22 L19 22 Z" fill="#000" stroke="#fff"/>"##,
			"</svg>"
		);
		let cursor = Cursor::options()
			.create_from_svg(EXAMPLE.as_bytes(), &ImageSize::CURSOR_SIZES)
			.unwrap();
		let hotspots: Vec<(u16, u16, u16)> = cursor
			.images()
			.map(|(size, hotspot)| (size.get(), hotspot.x(), hotspot.y()))
			.collect();
		assert_eq!(
			hotspots,
			[
				(32, 4, 7),
				(40, 5, 9),
				(48, 6, 11),
				(64, 9, 15),
				(80, 11, 18),
				(96, 13, 22),
				(256, 36, 60)
			]
		);

		for hidden in [
			r#"<rect id="hotspot" x="4" y="7" width="1" height="1" display="none"/>"#,
			r#"<defs><rect id="hotspot" x="4" y="7" width="1" height="1"/></defs>"#,
		] {
			let svg = EXAMPLE.replace(
				r#"<rect id="hotspot" x="4" y="7" width="1" height="1" visibility="hidden"/>"#,
				hidden,
			);
			assert_err!(
				Cursor::options().create_from_svg(svg.as_bytes(), &[ImageSize::MAX]),
				ImageError::MissingHotspot { image: 0 }
			);
		}
	}

	#[test]
	fn basic_safeguards_apply() {
		let message = |svg: &[u8]| match Icon::options().create_from_svg(svg, &[ImageSize::MAX]) {
			Err(ImageError::InvalidSvg { message }) => message,
			other => panic!("{other:?}"),
		};
		assert_eq!(
			message(svg_with(r#"viewBox="0 0 20 10""#, "").as_bytes()),
			"the document is 20x10, but icons and cursors must be square"
		);
		assert_eq!(message(&[0xFF, 0xFE]), "the document is not UTF-8");
		assert_err!(
			Icon::options().create_from_svg(&vec![b' '; svg::MAX_SVG_SIZE + 1], &[ImageSize::MAX]),
			ImageError::InputTooLarge { .. }
		);
		let entity = concat!(
			r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "aaaa">]>"#,
			r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><title>&a;</title></svg>"#
		);
		assert!(message(entity.as_bytes()).contains("DTD"), "No DTD");
		let many = svg_with(r#"viewBox="0 0 10 10""#, &"<g/>".repeat(100_001));
		assert!(message(many.as_bytes()).contains("nodes limit"), "At most 100000 nodes");

		let text = svg_with(r#"viewBox="0 0 10 10""#, "<text>A</text>");
		let external = svg_with(
			r#"viewBox="0 0 10 10" xmlns:xlink="http://www.w3.org/1999/xlink""#,
			concat!(
				r#"<image width="10" height="10" xlink:href="file:///etc/passwd"/>"#,
				r#"<image width="10" height="10" href="data:image/svg+xml;utf8,"#,
				r#"&lt;svg xmlns='http://www.w3.org/2000/svg'&gt;&lt;rect width='9' height='9'/&gt;&lt;/svg&gt;"/>"#,
			),
		);
		for svg in [text, external] {
			let pixels = svg::Document::parse(svg.as_bytes(), false)
				.unwrap()
				.render(ImageSize::MAX, 0)
				.unwrap();
			assert!(pixels.iter().all(|&byte| byte == 0), "Neither text nor images are drawn: {svg}");
		}
	}
}
