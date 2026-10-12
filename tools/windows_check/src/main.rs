//! Loads every icon and cursor of this executable, of its DLL and of the files the build script wrote,
//! the way Windows does, and compares the pixels with the PNG stored in the resource.
//! Prints one row per image and exits with 1 if any check fails.
//!
//! Checks per image of a module:
//! `lookup`: `LookupIconIdFromDirectoryEx` picks this image for its size from the group directory,
//! `create`: `CreateIconFromResourceEx` decodes the stored image,
//! `load`: `LoadImageW` finds the group and decodes the image, as `LoadIcon` and `LoadCursor` do,
//! `shell`: `PrivateExtractIconsW` extracts the icon from the file the way Explorer does.
//! Files are loaded with `LoadImageW` and `LR_LOADFROMFILE` (`file`).

#![cfg_attr(not(windows), allow(dead_code))]

include!(concat!(env!("OUT_DIR"), "/cases.rs"));

#[cfg(not(windows))]
fn main() {
	eprintln!("The check needs Windows: it tests the image loader of user32");
	std::process::exit(2);
}

#[cfg(windows)]
fn main() {
	std::process::exit(check::run());
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

fn bit_depth(depth: png::BitDepth) -> u8 {
	match depth {
		png::BitDepth::One => 1,
		png::BitDepth::Two => 2,
		png::BitDepth::Four => 4,
		png::BitDepth::Eight => 8,
		png::BitDepth::Sixteen => 16,
	}
}

fn decode_png(data: &[u8]) -> Result<(u32, Vec<u8>, String), String> {
	let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
	decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA | png::Transformations::STRIP_16);
	let mut reader = decoder.read_info().map_err(|err| err.to_string())?;
	let info = reader.info();
	let trns = if info.trns.is_some() { " +tRNS" } else { "" };
	let form = format!("{:?} {}{trns}", info.color_type, bit_depth(info.bit_depth));
	let (width, height) = (info.width, info.height);
	if width != height {
		return Err(format!("{width}x{height} is not square"));
	}
	let mut buffer = vec![0; reader.output_buffer_size().ok_or("the PNG is too large")?];
	let frame = reader.next_frame(&mut buffer).map_err(|err| err.to_string())?;
	buffer.truncate(frame.buffer_size());
	let pixels = match frame.color_type {
		png::ColorType::Rgba => buffer,
		png::ColorType::GrayscaleAlpha => buffer
			.as_chunks::<2>()
			.0
			.iter()
			.flat_map(|pair| [pair[0], pair[0], pair[0], pair[1]])
			.collect(),
		other => return Err(format!("the decoder delivered {other:?}")),
	};
	Ok((width, pixels, form))
}

#[cfg(windows)]
mod check {
	use crate::{ALIAS, CURSOR_CASES, FILES, ICON_CASES, decode_png};
	use std::collections::BTreeSet;
	use std::ffi::OsStr;
	use std::os::windows::ffi::OsStrExt;
	use std::path::Path;
	use std::{fs, mem, ptr};
	use windows_sys::Win32::Foundation::{FreeLibrary, GetLastError, HMODULE};
	use windows_sys::Win32::Globalization::GetACP;
	use windows_sys::Win32::Graphics::Gdi::{
		BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW,
		HBITMAP,
	};
	use windows_sys::Win32::System::LibraryLoader::{
		FindResourceW, GetModuleHandleW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW, LoadResource,
		LockResource, SizeofResource,
	};
	use windows_sys::Win32::UI::WindowsAndMessaging::{
		CreateIconFromResourceEx, DestroyCursor, DestroyIcon, GetIconInfo, HICON, ICONINFO, IMAGE_CURSOR, IMAGE_ICON, IsProcessDPIAware,
		LR_DEFAULTCOLOR, LR_LOADFROMFILE, LoadImageW, LookupIconIdFromDirectoryEx, PrivateExtractIconsW, RT_CURSOR, RT_GROUP_CURSOR,
		RT_GROUP_ICON, RT_ICON,
	};
	use windows_sys::core::PCWSTR;

	const DLL_NAME: &str = "win_resource_builder_payload.dll";

	#[derive(Clone, Copy, Eq, PartialEq)]
	enum Kind {
		Icon,
		Cursor,
	}

	impl Kind {
		const fn group(self) -> PCWSTR {
			if matches!(self, Self::Icon) {
				RT_GROUP_ICON
			} else {
				RT_GROUP_CURSOR
			}
		}

		const fn image(self) -> PCWSTR {
			if matches!(self, Self::Icon) { RT_ICON } else { RT_CURSOR }
		}

		const fn image_type(self) -> u32 {
			if matches!(self, Self::Icon) { IMAGE_ICON } else { IMAGE_CURSOR }
		}

		const fn is_icon(self) -> i32 {
			if matches!(self, Self::Icon) { 1 } else { 0 }
		}
	}

	struct Expected {
		size: u32,
		form: String,
		pixels: Vec<u8>,
		hotspot: Option<(u32, u32)>,
	}

	struct ReadBack {
		width: i32,
		height: i32,
		hotspot: (u32, u32),
		from_mask: bool,
		pixels: Vec<u8>,
	}

	struct Row {
		source: &'static str,
		case: String,
		size: u32,
		form: String,
		checks: Vec<(&'static str, Result<String, String>)>,
	}

	impl Row {
		fn new(source: &'static str, case: &str) -> Self {
			Self {
				source,
				case: case.to_owned(),
				size: 0,
				form: String::new(),
				checks: Vec::new(),
			}
		}

		fn failed(mut self, check: &'static str, reason: String) -> Self {
			self.checks.push((check, Err(reason)));
			self
		}
	}

	pub fn run() -> i32 {
		let exe_path = std::env::current_exe().expect("Path of the executable");
		let dll_path = exe_path.with_file_name(DLL_NAME);
		println!("win-resource-builder Windows check");
		println!("executable: {}", exe_path.display());
		// SAFETY: plain queries of the process state
		let (code_page, dpi_aware) = unsafe { (GetACP(), IsProcessDPIAware() != 0) };
		println!("manifest: active code page {code_page} (65001 is the UTF-8 the manifest asks for), DPI aware: {dpi_aware}");

		// SAFETY: a null name is the module of the process
		let exe = unsafe { GetModuleHandleW(ptr::null()) };
		// SAFETY: the path is zero-terminated, and data files run no code
		let dll = unsafe {
			LoadLibraryExW(
				wide(dll_path.as_os_str()).as_ptr(),
				ptr::null_mut(),
				LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
			)
		};
		if dll.is_null() {
			eprintln!("Cannot load {} as data file (error {})", dll_path.display(), last_error());
			return 1;
		}

		let mut rows = Vec::new();
		for (source, module, path) in [("exe", exe, &exe_path), ("dll", dll, &dll_path)] {
			for &(id, case) in ICON_CASES {
				rows.extend(check_group(source, module, path, Kind::Icon, id, case));
			}
			for &(id, case) in CURSOR_CASES {
				rows.extend(check_group(source, module, path, Kind::Cursor, id, case));
			}
			rows.push(check_alias(source, module, Kind::Icon));
			rows.push(check_alias(source, module, Kind::Cursor));
		}
		for (kind, cases, extension) in [(Kind::Icon, ICON_CASES, "ico"), (Kind::Cursor, CURSOR_CASES, "cur")] {
			for &(_, case) in cases {
				for variant in ["input", "optimized"] {
					let path = Path::new(FILES).join(format!("{case}.{variant}.{extension}"));
					rows.extend(check_file(kind, &path, &format!("{case}.{variant}")));
				}
			}
		}
		// SAFETY: no handle into the DLL outlives this point
		unsafe { FreeLibrary(dll) };
		report(&rows)
	}

	fn check_group(source: &'static str, module: HMODULE, path: &Path, kind: Kind, id: u16, case: &str) -> Vec<Row> {
		let Some(group) = resource(module, kind.group(), id) else {
			return vec![Row::new(source, case).failed("group", format!("resource missing (error {})", last_error()))];
		};
		let count = usize::from(u16::from_le_bytes([group[4], group[5]]));
		let mut rows = Vec::with_capacity(count);
		for index in 0..count {
			let entry = &group[index.strict_mul(14).strict_add(6)..index.strict_add(1).strict_mul(14).strict_add(6)];
			let image_id = u16::from_le_bytes([entry[12], entry[13]]);
			let mut row = Row::new(source, case);
			let Some(data) = resource(module, kind.image(), image_id) else {
				rows.push(row.failed("image", format!("image {image_id} missing")));
				continue;
			};
			let expected = match stored_image(kind, &data) {
				Ok(expected) => expected,
				Err(reason) => {
					rows.push(row.failed("decode", reason));
					continue;
				},
			};
			let size = i32::try_from(expected.size).expect("At most 256 pixels");
			(row.size, row.form) = (expected.size, <String>::clone(&expected.form));

			// SAFETY: the group directory comes from the module and stays valid while it is loaded
			let picked = unsafe { LookupIconIdFromDirectoryEx(group.as_ptr(), kind.is_icon(), size, size, LR_DEFAULTCOLOR) };
			let lookup = if picked == i32::from(image_id) {
				Ok("ok".to_owned())
			} else {
				Err(format!("picks image {picked} instead of {image_id}"))
			};
			row.checks.push(("lookup", lookup));

			// SAFETY: the pointer and length describe the copied resource data
			let created = unsafe {
				CreateIconFromResourceEx(
					data.as_ptr(),
					u32::try_from(data.len()).expect("A resource of at most 4 GiB"),
					kind.is_icon(),
					0x0003_0000,
					size,
					size,
					LR_DEFAULTCOLOR,
				)
			};
			row.checks.push(("create", check_handle(kind, created, &expected)));

			// SAFETY: an integer resource name, as MAKEINTRESOURCEW gives it
			let loaded = unsafe { LoadImageW(module, int_resource(id), kind.image_type(), size, size, LR_DEFAULTCOLOR) };
			row.checks.push(("load", check_handle(kind, loaded, &expected)));

			if kind == Kind::Icon {
				let mut extracted: HICON = ptr::null_mut();
				// The groups are numbered from 1 without gaps, so the directory order gives the index
				let index = i32::from(id).strict_sub(1);
				// SAFETY: room for one icon
				let count = unsafe {
					PrivateExtractIconsW(
						wide(path.as_os_str()).as_ptr(),
						index,
						size,
						size,
						&raw mut extracted,
						ptr::null_mut(),
						1,
						LR_DEFAULTCOLOR,
					)
				};
				let shell = if count == 1 {
					check_handle(kind, extracted, &expected)
				} else {
					Err(format!("extracts {count} icons"))
				};
				row.checks.push(("shell", shell));
			}
			rows.push(row);
		}
		rows
	}

	fn check_alias(source: &'static str, module: HMODULE, kind: Kind) -> Row {
		let (alias, target) = ALIAS;
		let case = format!("alias {alias} of {target}");
		let row = Row::new(source, &case);
		let (Some(alias_group), Some(target_group)) = (resource(module, kind.group(), alias), resource(module, kind.group(), target))
		else {
			return row.failed("alias", "group missing".to_owned());
		};
		if alias_group != target_group {
			return row.failed("alias", "the group directories differ".to_owned());
		}
		let count = usize::from(u16::from_le_bytes([target_group[4], target_group[5]]));
		let expected = (0..count)
			.filter_map(|index| {
				let entry = &target_group[index.strict_mul(14).strict_add(6)..index.strict_add(1).strict_mul(14).strict_add(6)];
				let data = resource(module, kind.image(), u16::from_le_bytes([entry[12], entry[13]]))?;
				stored_image(kind, &data).ok()
			})
			.find(|image| image.size == 32);
		let Some(expected) = expected else {
			return row.failed("alias", "the target has no 32-pixel image".to_owned());
		};
		let mut row = row;
		(row.size, row.form) = (expected.size, <String>::clone(&expected.form));
		// SAFETY: an integer resource name
		let loaded = unsafe { LoadImageW(module, int_resource(alias), kind.image_type(), 32, 32, LR_DEFAULTCOLOR) };
		row.checks.push(("load", check_handle(kind, loaded, &expected)));
		row
	}

	fn check_file(kind: Kind, path: &Path, case: &str) -> Vec<Row> {
		let data = match fs::read(path) {
			Ok(data) => data,
			Err(err) => return vec![Row::new("file", case).failed("file", format!("{}: {err}", path.display()))],
		};
		let count = usize::from(u16::from_le_bytes([data[4], data[5]]));
		let mut rows = Vec::with_capacity(count);
		for index in 0..count {
			let entry = &data[index.strict_mul(16).strict_add(6)..index.strict_add(1).strict_mul(16).strict_add(6)];
			let dword = |offset: usize| {
				super::usize_from_u32(u32::from_le_bytes([
					entry[offset],
					entry[offset.strict_add(1)],
					entry[offset.strict_add(2)],
					entry[offset.strict_add(3)],
				]))
			};
			let mut row = Row::new("file", case);
			let image = dword(12)
				.checked_add(dword(8))
				.and_then(|end| data.get(dword(12)..end))
				.expect("The image lies inside the file");
			let (size, pixels, form) = match decode_png(image) {
				Ok(decoded) => decoded,
				Err(reason) => {
					rows.push(row.failed("decode", reason));
					continue;
				},
			};
			let hotspot = (kind == Kind::Cursor).then(|| {
				(
					u32::from(u16::from_le_bytes([entry[4], entry[5]])),
					u32::from(u16::from_le_bytes([entry[6], entry[7]])),
				)
			});
			let expected = Expected {
				size,
				form,
				pixels,
				hotspot,
			};
			(row.size, row.form) = (expected.size, <String>::clone(&expected.form));
			// SAFETY: the path is zero-terminated
			let loaded = unsafe {
				LoadImageW(
					ptr::null_mut(),
					wide(path.as_os_str()).as_ptr(),
					kind.image_type(),
					i32::try_from(size).expect("At most 256 pixels"),
					i32::try_from(size).expect("At most 256 pixels"),
					LR_LOADFROMFILE,
				)
			};
			row.checks.push(("file", check_handle(kind, loaded, &expected)));
			rows.push(row);
		}
		rows
	}

	fn stored_image(kind: Kind, data: &[u8]) -> Result<Expected, String> {
		let (hotspot, png) = match kind {
			Kind::Icon => (None, data),
			Kind::Cursor => {
				let x = u32::from(u16::from_le_bytes([data[0], data[1]]));
				let y = u32::from(u16::from_le_bytes([data[2], data[3]]));
				(Some((x, y)), &data[4..])
			},
		};
		let (size, pixels, form) = decode_png(png)?;
		Ok(Expected {
			size,
			form,
			pixels,
			hotspot,
		})
	}

	fn check_handle(kind: Kind, handle: HICON, expected: &Expected) -> Result<String, String> {
		if handle.is_null() {
			return Err(format!("no handle (error {})", last_error()));
		}
		let result = read_back(handle).and_then(|read| compare(kind, expected, &read));
		// SAFETY: the handle came from a create or load function of this check and is no longer used
		unsafe {
			match kind {
				Kind::Icon => DestroyIcon(handle),
				Kind::Cursor => DestroyCursor(handle),
			}
		};
		result
	}

	fn read_back(handle: HICON) -> Result<ReadBack, String> {
		// SAFETY: ICONINFO is plain data
		let mut info: ICONINFO = unsafe { mem::zeroed() };
		// SAFETY: a valid icon handle and a writable ICONINFO
		if unsafe { GetIconInfo(handle, &raw mut info) } == 0 {
			return Err(format!("GetIconInfo fails (error {})", last_error()));
		}
		let result = read_bitmaps(&info);
		// SAFETY: GetIconInfo hands out copies of both bitmaps, which the caller deletes
		unsafe {
			if !info.hbmColor.is_null() {
				DeleteObject(info.hbmColor);
			}
			if !info.hbmMask.is_null() {
				DeleteObject(info.hbmMask);
			}
		}
		result.map(|(width, height, from_mask, pixels)| ReadBack {
			width,
			height,
			hotspot: (info.xHotspot, info.yHotspot),
			from_mask,
			pixels,
		})
	}

	fn read_bitmaps(info: &ICONINFO) -> Result<(i32, i32, bool, Vec<u8>), String> {
		if info.hbmColor.is_null() {
			return Err("monochrome image without color bitmap".to_owned());
		}
		// SAFETY: BITMAP is plain data
		let mut bitmap: BITMAP = unsafe { mem::zeroed() };
		// SAFETY: a valid bitmap handle and a buffer of the size given
		if unsafe {
			GetObjectW(
				info.hbmColor,
				i32::try_from(mem::size_of::<BITMAP>()).expect("A small struct"),
				(&raw mut bitmap).cast(),
			)
		} == 0
		{
			return Err("GetObjectW fails on the color bitmap".to_owned());
		}
		let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
		let color = dib_bits(info.hbmColor, width, height)?;
		let from_mask = bitmap.bmBitsPixel != 32 || color.as_chunks::<4>().0.iter().all(|bgra| bgra[3] == 0);
		let mask = if from_mask {
			Some(dib_bits(info.hbmMask, width, height)?)
		} else {
			None
		};
		let mut pixels = Vec::with_capacity(color.len());
		for (index, bgra) in color.as_chunks::<4>().0.iter().enumerate() {
			let alpha = match &mask {
				None => bgra[3],
				Some(mask) if mask[index.strict_mul(4)] == 0 => 255,
				Some(_) => 0,
			};
			pixels.extend_from_slice(&[bgra[2], bgra[1], bgra[0], alpha]);
		}
		Ok((width, height, from_mask, pixels))
	}

	fn dib_bits(bitmap: HBITMAP, width: i32, height: i32) -> Result<Vec<u8>, String> {
		// SAFETY: BITMAPINFO is plain data
		let mut header: BITMAPINFO = unsafe { mem::zeroed() };
		header.bmiHeader.biSize = u32::try_from(mem::size_of::<BITMAPINFOHEADER>()).expect("A small struct");
		header.bmiHeader.biWidth = width;
		header.bmiHeader.biHeight = height.strict_neg();
		header.bmiHeader.biPlanes = 1;
		header.bmiHeader.biBitCount = 32;
		header.bmiHeader.biCompression = BI_RGB;
		let mut bits = vec![0_u8; usize::try_from(width.strict_mul(height).strict_mul(4)).expect("A positive size")];
		// SAFETY: a memory DC, a buffer for every line at 32 bits per pixel and a matching header
		let lines = unsafe {
			let dc = CreateCompatibleDC(ptr::null_mut());
			let lines = GetDIBits(
				dc,
				bitmap,
				0,
				u32::try_from(height).expect("A positive height"),
				bits.as_mut_ptr().cast(),
				&raw mut header,
				DIB_RGB_COLORS,
			);
			DeleteDC(dc);
			lines
		};
		if lines == height {
			Ok(bits)
		} else {
			Err(format!("GetDIBits copies {lines} of {height} lines"))
		}
	}

	fn compare(kind: Kind, expected: &Expected, read: &ReadBack) -> Result<String, String> {
		let size = i32::try_from(expected.size).expect("At most 256 pixels");
		if (read.width, read.height) != (size, size) {
			return Err(format!("{}x{} instead of {size}x{size}", read.width, read.height));
		}
		if kind == Kind::Cursor && Some(read.hotspot) != expected.hotspot {
			return Err(format!("hotspot {:?} instead of {:?}", read.hotspot, expected.hotspot));
		}
		let premultiply =
			|color: u8, alpha: u8| i32::try_from(u32::from(color).strict_mul(u32::from(alpha)).strict_add(127) / 255).expect("At most 255");
		let (mut straight, mut premultiplied, mut partial) = (None, true, false);
		for (index, (stored, read)) in expected
			.pixels
			.as_chunks::<4>()
			.0
			.iter()
			.zip(read.pixels.as_chunks::<4>().0)
			.enumerate()
		{
			let position = i32::try_from(index).expect("At most 65536 pixels");
			let at = (position.strict_rem(size), position.strict_div(size));
			if stored[3] != read[3] {
				return Err(format!("alpha at {at:?}: {} instead of {}", read[3], stored[3]));
			}
			if stored[3] == 0 {
				continue;
			}
			partial |= stored[3] < 255;
			if stored[..3] != read[..3] && straight.is_none() {
				straight = Some(format!("color at {at:?}: {:?} instead of {:?}", &read[..3], &stored[..3]));
			}
			premultiplied &= (0..3).all(|channel| premultiply(stored[channel], stored[3]).strict_sub(i32::from(read[channel])).abs() <= 1);
		}
		match straight {
			None if read.from_mask => Ok("mask".to_owned()),
			None if partial => Ok("straight".to_owned()),
			None => Ok("binary".to_owned()),
			Some(_) if premultiplied => Ok("premultiplied".to_owned()),
			Some(difference) => Err(difference),
		}
	}

	fn report(rows: &[Row]) -> i32 {
		let mut failures = Vec::new();
		let mut forms = BTreeSet::new();
		let mut alpha = BTreeSet::new();
		let mut checks = 0_usize;
		println!();
		println!("{:<6} {:<28} {:>4}  {:<22} checks", "source", "case", "size", "png form");
		for row in rows {
			forms.insert(<String>::clone(&row.form));
			let cells: Vec<String> = row
				.checks
				.iter()
				.map(|(name, result)| {
					checks = checks.strict_add(1);
					match result {
						Ok(label) => {
							if label != "ok" {
								alpha.insert(<String>::clone(label));
							}
							format!("{name}:ok")
						},
						Err(reason) => {
							failures.push(format!("{} {} {} {name}: {reason}", row.source, row.case, row.size));
							format!("{name}:FAIL")
						},
					}
				})
				.collect();
			println!(
				"{:<6} {:<28} {:>4}  {:<22} {}",
				row.source,
				row.case,
				row.size,
				row.form,
				cells.join(" ")
			);
		}
		forms.remove("");
		println!();
		println!("PNG forms: {}", forms.into_iter().collect::<Vec<_>>().join(", "));
		println!("Alpha as read back: {}", alpha.into_iter().collect::<Vec<_>>().join(", "));
		println!("{checks} checks, {} failed", failures.len());
		for failure in &failures {
			println!("FAIL {failure}");
		}
		i32::from(!failures.is_empty())
	}

	fn resource(module: HMODULE, kind: PCWSTR, id: u16) -> Option<Vec<u8>> {
		// SAFETY: integer names and types, and the resource stays mapped while the module is loaded
		unsafe {
			let info = FindResourceW(module, int_resource(id), kind);
			if info.is_null() {
				return None;
			}
			let data = LockResource(LoadResource(module, info));
			if data.is_null() {
				return None;
			}
			Some(std::slice::from_raw_parts(data.cast::<u8>(), super::usize_from_u32(SizeofResource(module, info))).to_vec())
		}
	}

	fn int_resource(id: u16) -> PCWSTR {
		ptr::without_provenance(usize::from(id))
	}

	fn wide(text: &OsStr) -> Vec<u16> {
		text.encode_wide().chain([0]).collect()
	}

	fn last_error() -> u32 {
		// SAFETY: reads the error of the calling thread
		unsafe { GetLastError() }
	}
}
