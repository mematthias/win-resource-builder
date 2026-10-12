//! Builds the icons, cursors and manifests of the check with win-resource-builder,
//! and links them as one COFF object into the executable and the DLL.
//! Every generated icon is named after the PNG form oxipng should pick for its larger sizes;
//! the check reports the form each stored image really has.

use std::error::Error;
use std::fmt::Write as _;
use std::fs::File;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::{env, fs};
use win_resource_builder::image::{Cursor, Hotspot, Icon, ImageSize, PngOptimization};
use win_resource_builder::manifest::Manifest;
use win_resource_builder::resource::{CoffMachine, LanguageId, ResourceFile};

type BuildResult<T> = Result<T, Box<dyn Error>>;

/// Gives the RGBA color of pixel (x, y) of an image of the given size.
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
	<rect id="hotspot" x="7.5" y="7.5" width="1" height="1" visibility="hidden"/>
	<defs>
		<linearGradient id="fill" x1="0" y1="0" x2="1" y2="1">
			<stop offset="0" stop-color="#2a7fff"/>
			<stop offset="1" stop-color="#ff5aa0" stop-opacity="0.6"/>
		</linearGradient>
	</defs>
	<rect x="4" y="4" width="56" height="56" rx="12" fill="url(#fill)"/>
	<circle cx="36" cy="36" r="14" fill="#ffffff" fill-opacity="0.85"/>
	<path d="M8 8 L30 20 L20 30 Z" fill="#202020"/>
</svg>"##;

fn main() -> BuildResult<()> {
	let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo sets OUT_DIR for build scripts")?);
	let files = out.join("files");
	fs::create_dir_all(&files)?;
	let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
	println!("cargo::rerun-if-changed=build.rs");
	println!("cargo::rerun-if-changed={}", fixtures.display());

	let icons = icon_inputs(&fixtures)?;
	let cursors = cursor_inputs(&fixtures)?;

	let mut file = ResourceFile::new();
	let mut cases = String::new();
	writeln!(cases, "pub const ICON_CASES: &[(u16, &str)] = &[")?;
	for (id, (name, ico)) in (1_u16..).zip(&icons) {
		let icon = Icon::options().create_from_ico(ico).map_err(|err| format!("{name}: {err}"))?;
		fs::write(files.join(format!("{name}.input.ico")), ico)?;
		fs::write(files.join(format!("{name}.optimized.ico")), icon.to_ico())?;
		file.insert_icon(ordinal(id)?, LanguageId::NEUTRAL, &icon)?;
		writeln!(cases, "\t({id}, {name:?}),")?;
	}
	writeln!(cases, "];\npub const CURSOR_CASES: &[(u16, &str)] = &[")?;
	for (id, (name, cur)) in (1_u16..).zip(&cursors) {
		let cursor = Cursor::options().create_from_cur(cur).map_err(|err| format!("{name}: {err}"))?;
		fs::write(files.join(format!("{name}.input.cur")), cur)?;
		fs::write(files.join(format!("{name}.optimized.cur")), cursor.to_cur())?;
		file.insert_cursor(ordinal(id)?, LanguageId::NEUTRAL, &cursor)?;
		writeln!(cases, "\t({id}, {name:?}),")?;
	}
	writeln!(cases, "];")?;
	file.insert_icon_alias(ordinal(ALIAS)?, LanguageId::NEUTRAL, NonZeroU16::MIN)?;
	file.insert_cursor_alias(ordinal(ALIAS)?, LanguageId::NEUTRAL, NonZeroU16::MIN)?;
	writeln!(cases, "pub const ALIAS: (u16, u16) = ({ALIAS}, 1);")?;
	let files_text = files.to_str().ok_or("OUT_DIR is not valid UTF-8")?;
	writeln!(cases, "pub const FILES: &str = {files_text:?};")?;
	fs::write(out.join("cases.rs"), cases)?;

	// The executable only starts if Windows accepts its manifest; the DLL gets one of its own
	file.insert_manifest(&Manifest::standard().create()?)?;
	file.insert_manifest(&Manifest::standard_dll().create()?)?;

	// Every linked target gets the object: the executable and the DLL
	if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
		let object = out.join("resources.obj");
		file.write_coff(File::create(&object)?, CoffMachine::cargo_target()?)?;
		println!("cargo::rustc-link-arg={}", object.display());
	}
	Ok(())
}

/// Returns name and `.ico` content of every icon case: PNGs kept as generated, the SVG and the fixtures.
fn icon_inputs(fixtures: &Path) -> BuildResult<Vec<(String, Vec<u8>)>> {
	let retain = PngOptimization::Retain;
	let mut icons = Vec::new();
	for (name, pixel) in ICON_CASES {
		let pngs = ICON_SIZES.iter().map(|&size| png(size, pixel)).collect::<BuildResult<Vec<_>>>()?;
		let icon = Icon::options()
			.png_optimization(retain)
			.create_from_pngs(&pngs)
			.map_err(|err| format!("{name}: {err}"))?;
		icons.push((name.to_owned(), icon.to_ico()));
	}
	let svg = Icon::options()
		.png_optimization(retain)
		.create_from_svg(SVG.as_bytes(), &ImageSize::ICON_SIZES)?;
	icons.push(("svg".to_owned(), svg.to_ico()));
	for fixture in ["app", "second"] {
		icons.push((
			format!("fixture_{fixture}"),
			fs::read(fixtures.join(format!("icon/{fixture}.ico")))?,
		));
	}
	Ok(icons)
}

/// Returns name and `.cur` content of every cursor case, with the hotspot at a quarter and a third of the edge,
/// or where the SVG names it.
fn cursor_inputs(fixtures: &Path) -> BuildResult<Vec<(String, Vec<u8>)>> {
	let retain = PngOptimization::Retain;
	let quarter_third = |size: ImageSize| Hotspot::new(size.get().strict_div(4), size.get().strict_div(3));
	let mut cursors = Vec::new();
	for (name, pixel) in CURSOR_CASES {
		let pngs = CURSOR_SIZES.iter().map(|&size| png(size, pixel)).collect::<BuildResult<Vec<_>>>()?;
		let cursor = Cursor::options()
			.png_optimization(retain)
			.hotspot(|size, _| quarter_third(size))
			.create_from_pngs(&pngs)
			.map_err(|err| format!("{name}: {err}"))?;
		cursors.push((name.to_owned(), cursor.to_cur()));
	}
	let svg = Cursor::options()
		.png_optimization(retain)
		.hotspot(|size, detected| detected.unwrap_or_else(|| quarter_third(size)))
		.create_from_svg(SVG.as_bytes(), &ImageSize::CURSOR_SIZES)?;
	cursors.push(("cursor_svg".to_owned(), svg.to_cur()));
	cursors.push(("fixture_arrow".to_owned(), fs::read(fixtures.join("cursor/arrow.cur"))?));
	Ok(cursors)
}

fn ordinal(value: u16) -> BuildResult<NonZeroU16> {
	NonZeroU16::new(value).ok_or_else(|| "Resource names start at 1".into())
}

/// Encodes an RGBA image of `size` pixels whose colors `pixel` gives.
fn png(size: u32, pixel: Pixel) -> BuildResult<Vec<u8>> {
	let len = size
		.checked_mul(size)
		.and_then(|count| count.checked_mul(4))
		.and_then(|len| usize::try_from(len).ok())
		.ok_or("The image is too large")?;
	let mut rgba = Vec::with_capacity(len);
	for y in 0..size {
		for x in 0..size {
			rgba.extend_from_slice(&pixel(x, y, size));
		}
	}
	let mut data = Vec::new();
	let mut encoder = png::Encoder::new(&mut data, size, size);
	encoder.set_color(png::ColorType::Rgba);
	encoder.set_depth(png::BitDepth::Eight);
	let mut writer = encoder.write_header()?;
	writer.write_image_data(&rgba)?;
	writer.finish()?;
	Ok(data)
}

/// Returns a hash of the position, so that every case gets reproducible noise.
#[must_use]
const fn noise(x: u32, y: u32) -> u32 {
	let mut hash = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77).rotate_left(13);
	hash ^= hash.strict_shr(15);
	hash = hash.wrapping_mul(0x2C1B_3C6D);
	hash ^ hash.strict_shr(12)
}

/// Returns the low byte.
#[must_use]
const fn byte(value: u32) -> u8 {
	let [low, ..] = value.to_le_bytes();
	low
}

/// Returns an alpha value from 32 to 255.
#[must_use]
const fn alpha(value: u32) -> u8 {
	byte(value.strict_rem(224).strict_add(32))
}

/// Returns the color `value` selects from a table, as opaque RGBA.
#[must_use]
fn pick(colors: &[[u8; 3]], value: u32) -> [u8; 4] {
	let index = usize::try_from(value).unwrap_or(0).checked_rem(colors.len()).unwrap_or(0);
	let [red, green, blue] = colors.get(index).copied().unwrap_or_default();
	[red, green, blue, 255]
}

#[must_use]
const fn rgba(x: u32, y: u32, size: u32) -> [u8; 4] {
	let n = noise(x, y);
	[
		byte(x.strict_mul(255).strict_div(size)),
		byte(y.strict_mul(255).strict_div(size)),
		byte(n),
		alpha(n.strict_shr(8)),
	]
}

#[must_use]
const fn rgb(x: u32, y: u32, _: u32) -> [u8; 4] {
	let n = noise(x, y);
	[byte(n), byte(n.strict_shr(8)), byte(n.strict_shr(16)), 255]
}

#[must_use]
const fn rgb_trns(x: u32, y: u32, _: u32) -> [u8; 4] {
	let n = noise(x, y);
	if n.is_multiple_of(7) {
		[0; 4]
	} else {
		[byte(n) | 1, byte(n.strict_shr(8)), byte(n.strict_shr(16)), 255]
	}
}

#[must_use]
const fn gray8(x: u32, y: u32, _: u32) -> [u8; 4] {
	let gray = byte(noise(x, y));
	[gray, gray, gray, 255]
}

#[must_use]
fn gray8_trns(x: u32, y: u32, _: u32) -> [u8; 4] {
	let n = noise(x, y);
	let gray = byte(n).max(1);
	if n.is_multiple_of(7) { [0; 4] } else { [gray, gray, gray, 255] }
}

#[must_use]
const fn gray_alpha(x: u32, y: u32, _: u32) -> [u8; 4] {
	let n = noise(x, y);
	let gray = byte(n);
	[gray, gray, gray, alpha(n.strict_shr(8))]
}

#[must_use]
const fn gray4(x: u32, y: u32, _: u32) -> [u8; 4] {
	let gray = byte(noise(x, y).strict_rem(16).strict_mul(17));
	[gray, gray, gray, 255]
}

#[must_use]
const fn gray2(x: u32, y: u32, _: u32) -> [u8; 4] {
	let gray = byte(noise(x, y).strict_rem(4).strict_mul(85));
	[gray, gray, gray, 255]
}

#[must_use]
const fn gray1(x: u32, y: u32, _: u32) -> [u8; 4] {
	let gray = if x.strict_div(2).strict_add(y.strict_div(2)).is_multiple_of(2) {
		0
	} else {
		255
	};
	[gray, gray, gray, 255]
}

#[must_use]
const fn palette8_alpha(x: u32, y: u32, _: u32) -> [u8; 4] {
	let index = noise(x, y).strict_rem(200);
	[
		byte(index.strict_mul(7)),
		byte(index.strict_mul(13).strict_add(50)),
		byte(index.strict_mul(29).strict_add(100)),
		byte(index.strict_rem(192).strict_add(64)),
	]
}

#[must_use]
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
	pick(&COLORS, noise(x, y))
}

#[must_use]
fn palette2(x: u32, y: u32, _: u32) -> [u8; 4] {
	const COLORS: [[u8; 3]; 4] = [[230, 25, 75], [60, 180, 75], [0, 130, 200], [255, 225, 25]];
	pick(&COLORS, noise(x, y))
}

#[must_use]
const fn palette1(x: u32, y: u32, _: u32) -> [u8; 4] {
	if noise(x, y).is_multiple_of(2) {
		[200, 30, 30, 255]
	} else {
		[30, 30, 200, 255]
	}
}
