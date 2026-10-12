//! Builds the icons, cursors and manifests of the check with win-resource-builder,
//! and links them as one COFF object into the executable and the DLL.
//! Every generated icon is named after the PNG form oxipng should pick for its larger sizes;
//! the check reports the form each stored image really has.

use std::fmt::Write;
use std::fs::File;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::{env, fs};
use win_resource_builder::image::{Cursor, Hotspot, Icon, ImageSize, PngOptimization};
use win_resource_builder::manifest::Manifest;
use win_resource_builder::resource::{CoffMachine, LanguageId, ResourceFile};

type Pixel = fn(u32, u32, u32) -> [u8; 4];

const ICON_CASES: [(&str, Pixel); 13] = [
	("rgba", rgba),
	("rgb", rgb),
	("rgb_trns", rgb_trns),
	("gray8", gray8),
	("gray8_trns", gray8_trns),
	("gray_alpha", gray_alpha),
	("gray4", gray4),
	("gray2", gray2),
	("gray1", gray1),
	("palette8_alpha", palette8_alpha),
	("palette4", palette4),
	("palette2", palette2),
	("palette1", palette1),
];
const ICON_SIZES: [u32; 7] = [16, 20, 24, 32, 48, 64, 256];

const CURSOR_CASES: [(&str, Pixel); 3] = [("cursor_rgba", rgba), ("cursor_palette1", palette1), ("cursor_gray1", gray1)];
const CURSOR_SIZES: [u32; 6] = [32, 48, 64, 96, 128, 256];

const ALIAS: u16 = 100;

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
	<defs>
		<linearGradient id="fill" x1="0" y1="0" x2="1" y2="1">
			<stop offset="0" stop-color="#2a7fff"/>
			<stop offset="1" stop-color="#ff5aa0" stop-opacity="0.6"/>
		</linearGradient>
	</defs>
	<rect x="4" y="4" width="56" height="56" rx="12" fill="url(#fill)"/>
	<circle cx="36" cy="36" r="14" fill="#ffffff" fill-opacity="0.85"/>
	<path d="M8 8 L30 20 L20 30 Z" fill="#202020"/>
	<rect id="hotspot" x="7.5" y="7.5" width="1" height="1" visibility="hidden"/>
</svg>"##;

fn main() {
	let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
	let files = out.join("files");
	fs::create_dir_all(&files).expect("The file directory can be created");
	let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
	println!("cargo::rerun-if-changed=build.rs");
	println!("cargo::rerun-if-changed={}", fixtures.display());

	// The input files keep every PNG as it was generated; loading them below optimizes them
	let retain = PngOptimization::Retain;
	let mut icons = Vec::new();
	for (name, pixel) in ICON_CASES {
		let pngs: Vec<Vec<u8>> = ICON_SIZES.iter().map(|&size| png(size, pixel)).collect();
		let icon = Icon::options().png_optimization(retain).create_from_pngs(&pngs);
		icons.push((name.to_owned(), icon.unwrap_or_else(|err| panic!("{name}: {err}")).to_ico()));
	}
	let svg_icon = Icon::options()
		.png_optimization(retain)
		.create_from_svg(SVG.as_bytes(), &ImageSize::ICON_SIZES);
	icons.push(("svg".to_owned(), svg_icon.expect("Valid SVG").to_ico()));
	for fixture in ["app", "second"] {
		let ico = fs::read(fixtures.join(format!("icon/{fixture}.ico"))).expect("Icon fixture");
		icons.push((format!("fixture_{fixture}"), ico));
	}

	// Cursors with their hotspot at a quarter and a third of the edge, or where the SVG names it
	let quarter_third = |size: ImageSize| Hotspot::new(edge_part(size, 4), edge_part(size, 3));
	let mut cursors = Vec::new();
	for (name, pixel) in CURSOR_CASES {
		let pngs: Vec<Vec<u8>> = CURSOR_SIZES.iter().map(|&size| png(size, pixel)).collect();
		let cursor = Cursor::options()
			.png_optimization(retain)
			.hotspot(|size, _| quarter_third(size))
			.create_from_pngs(&pngs);
		cursors.push((name.to_owned(), cursor.unwrap_or_else(|err| panic!("{name}: {err}")).to_cur()));
	}
	let svg_cursor = Cursor::options()
		.png_optimization(retain)
		.hotspot(|size, detected| detected.unwrap_or_else(|| quarter_third(size)))
		.create_from_svg(SVG.as_bytes(), &ImageSize::CURSOR_SIZES);
	cursors.push(("cursor_svg".to_owned(), svg_cursor.expect("Valid SVG").to_cur()));
	cursors.push((
		"fixture_arrow".to_owned(),
		fs::read(fixtures.join("cursor/arrow.cur")).expect("Cursor fixture"),
	));

	let mut file = ResourceFile::new();
	let mut cases = String::new();
	let id = |value: usize| NonZeroU16::new(u16::try_from(value).expect("Few cases")).expect("Ids start at 1");
	writeln!(cases, "pub const ICON_CASES: &[(u16, &str)] = &[").unwrap();
	for (index, (name, ico)) in icons.iter().enumerate() {
		let icon = Icon::options().create_from_ico(ico).unwrap_or_else(|err| panic!("{name}: {err}"));
		fs::write(files.join(format!("{name}.input.ico")), ico).expect("Writable OUT_DIR");
		fs::write(files.join(format!("{name}.optimized.ico")), icon.to_ico()).expect("Writable OUT_DIR");
		file.insert_icon(id(index.strict_add(1)), LanguageId::NEUTRAL, icon)
			.expect("Icon fits");
		writeln!(cases, "\t({}, {name:?}),", index.strict_add(1)).unwrap();
	}
	writeln!(cases, "];\npub const CURSOR_CASES: &[(u16, &str)] = &[").unwrap();
	for (index, (name, cur)) in cursors.iter().enumerate() {
		let cursor = Cursor::options().create_from_cur(cur).unwrap_or_else(|err| panic!("{name}: {err}"));
		fs::write(files.join(format!("{name}.input.cur")), cur).expect("Writable OUT_DIR");
		fs::write(files.join(format!("{name}.optimized.cur")), cursor.to_cur()).expect("Writable OUT_DIR");
		file.insert_cursor(id(index.strict_add(1)), LanguageId::NEUTRAL, cursor)
			.expect("Cursor fits");
		writeln!(cases, "\t({}, {name:?}),", index.strict_add(1)).unwrap();
	}
	writeln!(cases, "];").unwrap();
	file.insert_icon_alias(id(ALIAS.into()), LanguageId::NEUTRAL, id(1))
		.expect("Alias fits");
	file.insert_cursor_alias(id(ALIAS.into()), LanguageId::NEUTRAL, id(1))
		.expect("Alias fits");
	writeln!(cases, "pub const ALIAS: (u16, u16) = ({ALIAS}, 1);").unwrap();
	writeln!(cases, "pub const FILES: &str = {:?};", files.to_str().expect("UTF-8 OUT_DIR")).unwrap();
	fs::write(out.join("cases.rs"), cases).expect("Writable OUT_DIR");

	// The executable only starts if Windows accepts its manifest; the DLL gets one of its own
	file.insert_manifest(Manifest::standard().create().expect("Valid manifest"))
		.expect("Manifest fits");
	file.insert_manifest(Manifest::standard_dll().create().expect("Valid manifest"))
		.expect("Manifest fits");

	// Every linked target gets the object: the executable and the DLL
	if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
		let object = out.join("resources.obj");
		let machine = CoffMachine::cargo_target().expect("A COFF machine");
		file.write_coff(File::create(&object).expect("Writable OUT_DIR"), machine)
			.expect("Writable OUT_DIR");
		println!("cargo::rustc-link-arg={}", object.display());
	}
}

fn png(size: u32, pixel: Pixel) -> Vec<u8> {
	let mut rgba = Vec::with_capacity(usize_from_u32(size.strict_mul(size).strict_mul(4)));
	for y in 0..size {
		for x in 0..size {
			rgba.extend_from_slice(&pixel(x, y, size));
		}
	}
	let mut data = Vec::new();
	let mut encoder = png::Encoder::new(&mut data, size, size);
	encoder.set_color(png::ColorType::Rgba);
	encoder.set_depth(png::BitDepth::Eight);
	let mut writer = encoder.write_header().expect("Valid PNG header");
	writer.write_image_data(&rgba).expect("Pixels of the size");
	writer.finish().expect("Complete PNG");
	data
}

fn edge_part(size: ImageSize, divisor: u32) -> u16 {
	u16::try_from(size.pixels().strict_div(divisor)).expect("At most 256 pixels")
}

fn noise(x: u32, y: u32) -> u32 {
	let mut hash = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77).rotate_left(13);
	hash ^= hash >> 15;
	hash = hash.wrapping_mul(0x2C1B_3C6D);
	hash ^ (hash >> 12)
}

#[cfg(not(any(target_pointer_width = "32", target_pointer_width = "64")))]
compile_error!("The Windows check needs a pointer width of 32 or 64 bits");

const fn usize_from_u32(value: u32) -> usize {
	let [a, b, c, d] = value.to_le_bytes();
	#[cfg(target_pointer_width = "32")]
	let bytes = [a, b, c, d];
	#[cfg(target_pointer_width = "64")]
	let bytes = [a, b, c, d, 0, 0, 0, 0];
	usize::from_le_bytes(bytes)
}

const fn byte(value: u32) -> u8 {
	value.to_le_bytes()[0]
}

fn rgba(x: u32, y: u32, size: u32) -> [u8; 4] {
	let n = noise(x, y);
	[
		byte(x.strict_mul(255).strict_div(size)),
		byte(y.strict_mul(255).strict_div(size)),
		byte(n),
		byte(((n >> 8) % 224).strict_add(32)),
	]
}

fn rgb(x: u32, y: u32, _: u32) -> [u8; 4] {
	let n = noise(x, y);
	[byte(n), byte(n >> 8), byte(n >> 16), 255]
}

fn rgb_trns(x: u32, y: u32, _: u32) -> [u8; 4] {
	let n = noise(x, y);
	if n.is_multiple_of(7) {
		[0; 4]
	} else {
		[byte(n) | 1, byte(n >> 8), byte(n >> 16), 255]
	}
}

fn gray8(x: u32, y: u32, _: u32) -> [u8; 4] {
	let gray = byte(noise(x, y));
	[gray, gray, gray, 255]
}

fn gray8_trns(x: u32, y: u32, _: u32) -> [u8; 4] {
	let n = noise(x, y);
	let gray = byte(n).max(1);
	if n.is_multiple_of(7) { [0; 4] } else { [gray, gray, gray, 255] }
}

fn gray_alpha(x: u32, y: u32, _: u32) -> [u8; 4] {
	let n = noise(x, y);
	let gray = byte(n);
	[gray, gray, gray, byte(((n >> 8) % 224).strict_add(32))]
}

fn gray4(x: u32, y: u32, _: u32) -> [u8; 4] {
	let gray = byte((noise(x, y) % 16).strict_mul(17));
	[gray, gray, gray, 255]
}

fn gray2(x: u32, y: u32, _: u32) -> [u8; 4] {
	let gray = byte((noise(x, y) % 4).strict_mul(85));
	[gray, gray, gray, 255]
}

fn gray1(x: u32, y: u32, _: u32) -> [u8; 4] {
	let gray = if (x / 2).strict_add(y / 2).is_multiple_of(2) { 0 } else { 255 };
	[gray, gray, gray, 255]
}

fn palette8_alpha(x: u32, y: u32, _: u32) -> [u8; 4] {
	let index = noise(x, y) % 200;
	[
		byte(index.strict_mul(7)),
		byte(index.strict_mul(13).strict_add(50)),
		byte(index.strict_mul(29).strict_add(100)),
		byte((index % 192).strict_add(64)),
	]
}

fn palette4(x: u32, y: u32, _: u32) -> [u8; 4] {
	const COLORS: [[u8; 3]; 12] = [
		[230, 25, 75],
		[60, 180, 75],
		[255, 225, 25],
		[0, 130, 200],
		[245, 130, 48],
		[145, 30, 180],
		[70, 240, 240],
		[240, 50, 230],
		[210, 245, 60],
		[250, 190, 212],
		[0, 128, 128],
		[170, 110, 40],
	];
	let [red, green, blue] = COLORS[usize_from_u32(noise(x, y) % 12)];
	[red, green, blue, 255]
}

fn palette2(x: u32, y: u32, _: u32) -> [u8; 4] {
	const COLORS: [[u8; 3]; 4] = [[230, 25, 75], [60, 180, 75], [0, 130, 200], [255, 225, 25]];
	let [red, green, blue] = COLORS[usize_from_u32(noise(x, y) % 4)];
	[red, green, blue, 255]
}

fn palette1(x: u32, y: u32, _: u32) -> [u8; 4] {
	if noise(x, y).is_multiple_of(2) {
		[200, 30, 30, 255]
	} else {
		[30, 30, 200, 255]
	}
}
