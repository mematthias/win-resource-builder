//! The checks of every image of the executable, of the DLL and of the written files, and the report.

use crate::decode::{Decoded, decode_png, file_images, group_image_ids, split_cursor};
use crate::win32::{self, IconHandle, Kind, Module, ReadBack};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;
use std::{env, fs};

include!(concat!(env!("OUT_DIR"), "/cases.rs"));

const DLL_NAME: &str = "win_resource_builder_payload.dll";

/// Runs every check, prints the report and returns failure if any check fails.
#[must_use]
pub fn run() -> ExitCode {
	let exe_path = match env::current_exe() {
		Ok(path) => path,
		Err(err) => {
			eprintln!("Cannot find the path of the executable: {err}");
			return ExitCode::FAILURE;
		},
	};
	let dll_path = exe_path.with_file_name(DLL_NAME);
	println!("win-resource-builder Windows check");
	println!("executable: {}", exe_path.display());
	println!(
		"manifest: active code page {} (65001 is the UTF-8 the manifest asks for), DPI aware: {}",
		win32::active_code_page(),
		win32::is_dpi_aware()
	);

	let exe = Module::current();
	let dll = match Module::load_data_file(&dll_path) {
		Ok(dll) => dll,
		Err(error) => {
			eprintln!("Cannot load {} as data file (error {error})", dll_path.display());
			return ExitCode::FAILURE;
		},
	};

	let mut rows = Vec::new();
	for (source, module, path) in [("exe", &exe, exe_path.as_path()), ("dll", &dll, dll_path.as_path())] {
		for (kind, cases) in [(Kind::Icon, ICON_CASES), (Kind::Cursor, CURSOR_CASES)] {
			for &(id, case) in cases {
				rows.extend(check_group(source, module, path, kind, id, case));
			}
			rows.push(check_alias(source, module, kind));
		}
	}
	for (kind, cases, extension) in [(Kind::Icon, ICON_CASES, "ico"), (Kind::Cursor, CURSOR_CASES, "cur")] {
		for &(_, case) in cases {
			for variant in ["input", "optimized"] {
				let path = Path::new(FILES).join(format!("{case}.{variant}.{extension}"));
				rows.extend(check_file(kind, &path, &format!("{case}.{variant}")));
			}
		}
	}
	drop(dll);
	report(&rows)
}

/// A stored image as the check expects to read it back.
struct Expected {
	size: u32,
	form: String,
	pixels: Vec<u8>,
	hotspot: Option<(u32, u32)>,
}

impl Expected {
	#[must_use]
	fn new(decoded: Decoded, hotspot: Option<(u32, u32)>) -> Self {
		let Decoded { size, form, pixels } = decoded;
		Self {
			size,
			form,
			pixels,
			hotspot,
		}
	}

	/// Decodes an image resource: the PNG, after the hotspot for cursors.
	fn from_resource(kind: Kind, data: &[u8]) -> Result<Self, String> {
		let (hotspot, png) = match kind {
			Kind::Icon => (None, data),
			Kind::Cursor => {
				let (hotspot, png) = split_cursor(data).ok_or_else(|| "the cursor image lacks its hotspot".to_owned())?;
				(Some(hotspot), png)
			},
		};
		Ok(Self::new(decode_png(png)?, hotspot))
	}

	/// Returns the edge length as the Win32 functions take it.
	fn edge(&self) -> Result<i32, String> {
		i32::try_from(self.size).map_err(|_| format!("{} pixels exceed the range of Win32 sizes", self.size))
	}
}

/// The results of one image.
struct Row {
	source: &'static str,
	case: String,
	size: u32,
	form: String,
	checks: Vec<(&'static str, Result<String, String>)>,
}

impl Row {
	#[must_use]
	fn new(source: &'static str, case: &str) -> Self {
		Self {
			source,
			case: case.to_owned(),
			size: 0,
			form: String::new(),
			checks: Vec::new(),
		}
	}

	#[must_use]
	fn failed(mut self, check: &'static str, reason: String) -> Self {
		self.checks.push((check, Err(reason)));
		self
	}

	fn describe(&mut self, expected: &Expected) {
		self.size = expected.size;
		<String>::clone_from(&mut self.form, &expected.form);
	}
}

/// The context every image check of one group shares.
struct Group<'a> {
	source: &'static str,
	module: &'a Module,
	path: &'a Path,
	kind: Kind,
	id: u16,
	case: &'a str,
	directory: &'a [u8],
}

impl Group<'_> {
	/// Checks one image through every path Windows loads it on.
	#[must_use]
	fn check_image(&self, image_id: u16) -> Row {
		let row = Row::new(self.source, self.case);
		let Some(data) = self.module.image(self.kind, image_id) else {
			return row.failed("image", format!("image {image_id} missing"));
		};
		let expected = match Expected::from_resource(self.kind, data) {
			Ok(expected) => expected,
			Err(reason) => return row.failed("decode", reason),
		};
		let mut row = row;
		row.describe(&expected);
		let size = match expected.edge() {
			Ok(size) => size,
			Err(reason) => return row.failed("size", reason),
		};

		let lookup = match win32::lookup_icon_id(self.directory, self.kind, size) {
			Some(picked) if picked == i32::from(image_id) => Ok("ok".to_owned()),
			Some(picked) => Err(format!("picks image {picked} instead of {image_id}")),
			None => Err(format!("picks no image (error {})", win32::last_error())),
		};
		row.checks.push(("lookup", lookup));
		let created = win32::create_from_resource(data, self.kind, size);
		row.checks.push(("create", verify(self.kind, created, &expected)));
		let loaded = self.module.load_image(self.kind, self.id, size);
		row.checks.push(("load", verify(self.kind, loaded, &expected)));
		if self.kind == Kind::Icon {
			// The groups are numbered from 1 without gaps, so the directory order gives the index
			let index = i32::from(self.id).strict_sub(1);
			let shell = match win32::extract_icon(self.path, index, size) {
				Ok(icon) => verify(self.kind, Some(icon), &expected),
				Err(count) => Err(format!("extracts {count} icons")),
			};
			row.checks.push(("shell", shell));
		}
		row
	}
}

#[must_use]
fn check_group(source: &'static str, module: &Module, path: &Path, kind: Kind, id: u16, case: &str) -> Vec<Row> {
	let Some(directory) = module.group(kind, id) else {
		return vec![Row::new(source, case).failed("group", format!("resource missing (error {})", win32::last_error()))];
	};
	let image_ids = match group_image_ids(directory) {
		Ok(image_ids) => image_ids,
		Err(reason) => return vec![Row::new(source, case).failed("group", reason)],
	};
	let group = Group {
		source,
		module,
		path,
		kind,
		id,
		case,
		directory,
	};
	image_ids.into_iter().map(|image_id| group.check_image(image_id)).collect()
}

/// Checks that the alias has the directory of its target and loads its 32-pixel image.
#[must_use]
fn check_alias(source: &'static str, module: &Module, kind: Kind) -> Row {
	let (alias, target) = ALIAS;
	let row = Row::new(source, &format!("alias {alias} of {target}"));
	let (Some(alias_group), Some(target_group)) = (module.group(kind, alias), module.group(kind, target)) else {
		return row.failed("alias", "group missing".to_owned());
	};
	if alias_group != target_group {
		return row.failed("alias", "the group directories differ".to_owned());
	}
	let image_ids = match group_image_ids(target_group) {
		Ok(image_ids) => image_ids,
		Err(reason) => return row.failed("alias", reason),
	};
	let expected = image_ids
		.into_iter()
		.filter_map(|image_id| Expected::from_resource(kind, module.image(kind, image_id)?).ok())
		.find(|image| image.size == 32);
	let Some(expected) = expected else {
		return row.failed("alias", "the target has no 32-pixel image".to_owned());
	};
	let mut row = row;
	row.describe(&expected);
	let loaded = module.load_image(kind, alias, 32);
	row.checks.push(("load", verify(kind, loaded, &expected)));
	row
}

/// Loads every image of an `.ico` or `.cur` file with `LoadImageW` and `LR_LOADFROMFILE`.
#[must_use]
fn check_file(kind: Kind, path: &Path, case: &str) -> Vec<Row> {
	let data = match fs::read(path) {
		Ok(data) => data,
		Err(err) => return vec![Row::new("file", case).failed("file", format!("{}: {err}", path.display()))],
	};
	let images = match file_images(&data) {
		Ok(images) => images,
		Err(reason) => return vec![Row::new("file", case).failed("file", reason)],
	};
	images
		.into_iter()
		.map(|image| {
			let row = Row::new("file", case);
			let decoded = match decode_png(image.data) {
				Ok(decoded) => decoded,
				Err(reason) => return row.failed("decode", reason),
			};
			let expected = Expected::new(decoded, (kind == Kind::Cursor).then_some(image.hotspot));
			let mut row = row;
			row.describe(&expected);
			let result = match expected.edge() {
				Ok(size) => verify(kind, win32::load_file(path, kind, size), &expected),
				Err(reason) => Err(reason),
			};
			row.checks.push(("file", result));
			row
		})
		.collect()
}

/// Reads an icon back and compares it with the stored image; the handle is destroyed afterwards.
fn verify(kind: Kind, handle: Option<IconHandle>, expected: &Expected) -> Result<String, String> {
	let handle = handle.ok_or_else(|| format!("no handle (error {})", win32::last_error()))?;
	compare(kind, expected, &handle.read_back()?)
}

/// Compares size, hotspot and every pixel, and names how Windows returns the colors.
///
/// Alpha has to match exactly and so does the color of every visible pixel, straight or premultiplied.
fn compare(kind: Kind, expected: &Expected, read: &ReadBack) -> Result<String, String> {
	let size = expected.edge()?;
	if (read.width, read.height) != (size, size) {
		return Err(format!("{}x{} instead of {size}x{size}", read.width, read.height));
	}
	if kind == Kind::Cursor && Some(read.hotspot) != expected.hotspot {
		return Err(format!("hotspot {:?} instead of {:?}", read.hotspot, expected.hotspot));
	}
	let columns = usize::try_from(expected.size).map_err(|err| err.to_string())?;
	let (mut straight, mut premultiplied, mut partial) = (None, true, false);
	let pairs = expected.pixels.as_chunks::<4>().0.iter().zip(read.pixels.as_chunks::<4>().0);
	for (index, (&[red, green, blue, alpha], &[read_red, read_green, read_blue, read_alpha])) in pairs.enumerate() {
		let at = (index.checked_rem(columns).unwrap_or(0), index.checked_div(columns).unwrap_or(0));
		if alpha != read_alpha {
			return Err(format!("alpha at {at:?}: {read_alpha} instead of {alpha}"));
		}
		if alpha == 0 {
			continue;
		}
		partial |= alpha < 255;
		let (stored, returned) = ([red, green, blue], [read_red, read_green, read_blue]);
		if stored != returned && straight.is_none() {
			straight = Some(format!("color at {at:?}: {returned:?} instead of {stored:?}"));
		}
		premultiplied &= stored
			.into_iter()
			.zip(returned)
			.all(|(color, read_color)| premultiply(color, alpha).abs_diff(read_color) <= 1);
	}
	match straight {
		None if read.from_mask => Ok("mask".to_owned()),
		None if partial => Ok("straight".to_owned()),
		None => Ok("binary".to_owned()),
		Some(_) if premultiplied => Ok("premultiplied".to_owned()),
		Some(difference) => Err(difference),
	}
}

/// Returns `color` premultiplied with `alpha`, rounded.
#[must_use]
fn premultiply(color: u8, alpha: u8) -> u8 {
	let product = u32::from(color).strict_mul(u32::from(alpha)).strict_add(127).strict_div(255);
	u8::try_from(product).unwrap_or(u8::MAX)
}

/// Prints one row per image and a summary; returns failure if any check failed.
#[must_use]
fn report(rows: &[Row]) -> ExitCode {
	let mut failures = Vec::new();
	let mut forms = BTreeSet::new();
	let mut alpha = BTreeSet::new();
	let mut checks = 0_usize;
	println!();
	println!("{:<6} {:<28} {:>4}  {:<22} checks", "source", "case", "size", "png form");
	for row in rows {
		if !row.form.is_empty() {
			forms.insert(row.form.as_str());
		}
		let mut cells = Vec::with_capacity(row.checks.len());
		for (name, result) in &row.checks {
			checks = checks.strict_add(1);
			match result {
				Ok(label) => {
					if label != "ok" {
						alpha.insert(label.as_str());
					}
					cells.push(format!("{name}:ok"));
				},
				Err(reason) => {
					failures.push(format!("{} {} {} {name}: {reason}", row.source, row.case, row.size));
					cells.push(format!("{name}:FAIL"));
				},
			}
		}
		println!(
			"{:<6} {:<28} {:>4}  {:<22} {}",
			row.source,
			row.case,
			row.size,
			row.form,
			cells.join(" ")
		);
	}
	println!();
	println!("PNG forms: {}", forms.into_iter().collect::<Vec<_>>().join(", "));
	println!("Alpha as read back: {}", alpha.into_iter().collect::<Vec<_>>().join(", "));
	println!("{checks} checks, {} failed", failures.len());
	for failure in &failures {
		println!("FAIL {failure}");
	}
	if failures.is_empty() {
		ExitCode::SUCCESS
	} else {
		ExitCode::FAILURE
	}
}
