//! Safe wrappers around the Win32 functions of the check; every unsafe operation of the program lives here.
//!
//! Modules, icons, bitmaps and device contexts are owned by types that release them on drop,
//! so no early return of a check leaks a handle.
//! Resource data borrows the module it comes from, so it cannot outlive the mapping.

#![allow(unsafe_code, reason = "The check calls the image loader of user32 through FFI")]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;
use std::{mem, ptr, slice};
use windows_sys::Win32::Foundation::{FreeLibrary, GetLastError, HMODULE};
use windows_sys::Win32::Globalization::GetACP;
use windows_sys::Win32::Graphics::Gdi::{
	BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW,
	HBITMAP, HDC, RGBQUAD,
};
use windows_sys::Win32::System::LibraryLoader::{
	FindResourceW, GetModuleHandleW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW, LoadResource, LockResource,
	SizeofResource,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
	CreateIconFromResourceEx, DestroyCursor, DestroyIcon, GetIconInfo, HICON, ICONINFO, IMAGE_CURSOR, IMAGE_ICON, IsProcessDPIAware,
	LR_DEFAULTCOLOR, LR_LOADFROMFILE, LoadImageW, LookupIconIdFromDirectoryEx, PrivateExtractIconsW,
};
use windows_sys::core::{BOOL, PCWSTR};

/// Icons and cursors, which share the loader but differ in resource types and destroy functions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
	Icon,
	Cursor,
}

impl Kind {
	/// Returns `RT_GROUP_ICON` or `RT_GROUP_CURSOR` as integer resource type.
	#[must_use]
	const fn group_type(self) -> u16 {
		match self {
			Self::Icon => 14,
			Self::Cursor => 12,
		}
	}

	/// Returns `RT_ICON` or `RT_CURSOR` as integer resource type.
	#[must_use]
	const fn image_type(self) -> u16 {
		match self {
			Self::Icon => 3,
			Self::Cursor => 1,
		}
	}

	#[must_use]
	const fn gdi_type(self) -> u32 {
		match self {
			Self::Icon => IMAGE_ICON,
			Self::Cursor => IMAGE_CURSOR,
		}
	}

	#[must_use]
	const fn is_icon(self) -> BOOL {
		match self {
			Self::Icon => 1,
			Self::Cursor => 0,
		}
	}
}

/// What `GetIconInfo` and `GetDIBits` return for an icon or cursor: size, hotspot and RGBA pixels, top-down.
pub struct ReadBack {
	pub width: i32,
	pub height: i32,
	pub hotspot: (u32, u32),
	/// Whether the alpha channel comes from the AND mask, as for images without alpha values.
	pub from_mask: bool,
	pub pixels: Vec<u8>,
}

/// Returns the active code page of the process, 65001 for UTF-8.
#[must_use]
pub fn active_code_page() -> u32 {
	// SAFETY: takes no arguments and reads the state of the process
	unsafe { GetACP() }
}

/// Returns whether the process is DPI aware, as its manifest asks for.
#[must_use]
pub fn is_dpi_aware() -> bool {
	// SAFETY: takes no arguments and reads the state of the process
	unsafe { IsProcessDPIAware() != 0 }
}

/// Returns the last error of the calling thread.
#[must_use]
pub fn last_error() -> u32 {
	// SAFETY: takes no arguments and reads the error of the calling thread
	unsafe { GetLastError() }
}

/// A module whose resources the check reads; a DLL loaded as data file is freed on drop.
pub struct Module {
	handle: HMODULE,
	owned: bool,
}

impl Module {
	/// Returns the executable of the process, which stays loaded while it runs.
	#[must_use]
	pub fn current() -> Self {
		// SAFETY: a null name asks for the module of the process
		let handle = unsafe { GetModuleHandleW(ptr::null()) };
		Self { handle, owned: false }
	}

	/// Loads a DLL as data file, which maps its resources without running its code.
	pub fn load_data_file(path: &Path) -> Result<Self, u32> {
		let name = wide(path.as_os_str());
		// SAFETY: the name is zero-terminated and outlives the call; a data file runs no code
		let handle = unsafe {
			LoadLibraryExW(
				name.as_ptr(),
				ptr::null_mut(),
				LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
			)
		};
		if handle.is_null() {
			Err(last_error())
		} else {
			Ok(Self { handle, owned: true })
		}
	}

	/// Returns the group directory of an icon or cursor.
	#[must_use]
	pub fn group(&self, kind: Kind, id: u16) -> Option<&[u8]> {
		self.resource(kind.group_type(), id)
	}

	/// Returns an image resource of an icon or cursor.
	#[must_use]
	pub fn image(&self, kind: Kind, id: u16) -> Option<&[u8]> {
		self.resource(kind.image_type(), id)
	}

	/// Loads the group `id` with `LoadImageW`, as `LoadIcon` and `LoadCursor` do.
	#[must_use]
	pub fn load_image(&self, kind: Kind, id: u16, size: i32) -> Option<IconHandle> {
		// SAFETY: an integer resource name of this module; without LR_SHARED the handle is the caller's
		let handle = unsafe { LoadImageW(self.handle, int_resource(id), kind.gdi_type(), size, size, LR_DEFAULTCOLOR) };
		IconHandle::new(handle, kind)
	}

	/// Returns the data of the resource of an integer type and name.
	#[must_use]
	fn resource(&self, resource_type: u16, id: u16) -> Option<&[u8]> {
		// SAFETY: integer name and type, which FindResourceW takes like MAKEINTRESOURCE values
		let info = unsafe { FindResourceW(self.handle, int_resource(id), int_resource(resource_type)) };
		if info.is_null() {
			return None;
		}
		// SAFETY: the resource handle comes from this module
		let global = unsafe { LoadResource(self.handle, info) };
		// SAFETY: LockResource takes every handle LoadResource returns, null included
		let data = unsafe { LockResource(global) };
		if data.is_null() {
			return None;
		}
		// SAFETY: the resource handle comes from this module
		let len = usize::try_from(unsafe { SizeofResource(self.handle, info) }).ok()?;
		// SAFETY: the resource is mapped read-only for `len` bytes while the module is loaded,
		// and the returned slice borrows the module, so it cannot outlive the mapping
		Some(unsafe { slice::from_raw_parts(data.cast::<u8>(), len) })
	}
}

impl Drop for Module {
	fn drop(&mut self) {
		if self.owned {
			// SAFETY: the handle came from LoadLibraryExW, and no resource slice borrows the module any more
			unsafe { FreeLibrary(self.handle) };
		}
	}
}

/// An icon or cursor the check created or loaded; destroyed on drop.
pub struct IconHandle {
	handle: HICON,
	kind: Kind,
}

impl IconHandle {
	#[must_use]
	fn new(handle: HICON, kind: Kind) -> Option<Self> {
		(!handle.is_null()).then_some(Self { handle, kind })
	}

	/// Reads size, hotspot and pixels back through `GetIconInfo` and `GetDIBits`.
	pub fn read_back(&self) -> Result<ReadBack, String> {
		let mut info = ICONINFO {
			fIcon: 0,
			xHotspot: 0,
			yHotspot: 0,
			hbmMask: ptr::null_mut(),
			hbmColor: ptr::null_mut(),
		};
		// SAFETY: a valid icon handle and a writable ICONINFO
		if unsafe { GetIconInfo(self.handle, &raw mut info) } == 0 {
			return Err(format!("GetIconInfo fails (error {})", last_error()));
		}
		// GetIconInfo hands out copies of both bitmaps, which the caller deletes
		let (color, mask) = (Bitmap(info.hbmColor), Bitmap(info.hbmMask));
		if color.0.is_null() {
			return Err("monochrome image without color bitmap".to_owned());
		}
		let mut bitmap = BITMAP {
			bmType: 0,
			bmWidth: 0,
			bmHeight: 0,
			bmWidthBytes: 0,
			bmPlanes: 0,
			bmBitsPixel: 0,
			bmBits: ptr::null_mut(),
		};
		let bitmap_size = i32::try_from(mem::size_of::<BITMAP>()).map_err(|err| err.to_string())?;
		// SAFETY: a valid bitmap handle and a buffer of exactly the size given
		if unsafe { GetObjectW(color.0, bitmap_size, (&raw mut bitmap).cast()) } == 0 {
			return Err("GetObjectW fails on the color bitmap".to_owned());
		}
		let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
		let colors = color.bits(width, height)?;
		let bgra = colors.as_chunks::<4>().0;
		let from_mask = bitmap.bmBitsPixel != 32 || bgra.iter().all(|&[.., alpha]| alpha == 0);
		let pixels = if from_mask {
			let mask_bits = mask.bits(width, height)?;
			bgra.iter()
				.zip(mask_bits.as_chunks::<4>().0)
				.flat_map(|(&[blue, green, red, _], &[masked, ..])| [red, green, blue, if masked == 0 { 255 } else { 0 }])
				.collect()
		} else {
			bgra.iter()
				.flat_map(|&[blue, green, red, alpha]| [red, green, blue, alpha])
				.collect()
		};
		Ok(ReadBack {
			width,
			height,
			hotspot: (info.xHotspot, info.yHotspot),
			from_mask,
			pixels,
		})
	}
}

impl Drop for IconHandle {
	fn drop(&mut self) {
		match self.kind {
			// SAFETY: the handle came from a create or load function of this check and is no longer used
			Kind::Icon => unsafe { DestroyIcon(self.handle) },
			// SAFETY: the handle came from a create or load function of this check and is no longer used
			Kind::Cursor => unsafe { DestroyCursor(self.handle) },
		};
	}
}

/// Returns the image name `LookupIconIdFromDirectoryEx` picks from a group directory for `size`.
#[must_use]
pub fn lookup_icon_id(directory: &[u8], kind: Kind, size: i32) -> Option<i32> {
	// The function reads as many 14-byte records as the directory announces
	let &[_, _, _, _, c0, c1] = directory.first_chunk::<6>()?;
	let needed = usize::from(u16::from_le_bytes([c0, c1])).checked_mul(14)?.checked_add(6)?;
	if directory.len() < needed {
		return None;
	}
	// SAFETY: the directory holds every record it announces
	let id = unsafe { LookupIconIdFromDirectoryEx(directory.as_ptr(), kind.is_icon(), size, size, LR_DEFAULTCOLOR) };
	(id != 0).then_some(id)
}

/// Decodes an image resource with `CreateIconFromResourceEx`.
#[must_use]
pub fn create_from_resource(data: &[u8], kind: Kind, size: i32) -> Option<IconHandle> {
	let len = u32::try_from(data.len()).ok()?;
	// SAFETY: the pointer and the length describe the resource data
	let handle = unsafe { CreateIconFromResourceEx(data.as_ptr(), len, kind.is_icon(), 0x0003_0000, size, size, LR_DEFAULTCOLOR) };
	IconHandle::new(handle, kind)
}

/// Loads an `.ico` or `.cur` file with `LoadImageW` and `LR_LOADFROMFILE`.
#[must_use]
pub fn load_file(path: &Path, kind: Kind, size: i32) -> Option<IconHandle> {
	let name = wide(path.as_os_str());
	// SAFETY: the name is zero-terminated and outlives the call
	let handle = unsafe { LoadImageW(ptr::null_mut(), name.as_ptr(), kind.gdi_type(), size, size, LR_LOADFROMFILE) };
	IconHandle::new(handle, kind)
}

/// Extracts icon `index` of a file with `PrivateExtractIconsW`, the way Explorer does;
/// the error is the number of icons it extracted instead of one.
pub fn extract_icon(path: &Path, index: i32, size: i32) -> Result<IconHandle, u32> {
	let name = wide(path.as_os_str());
	let mut handle: HICON = ptr::null_mut();
	// SAFETY: a zero-terminated name and room for exactly one icon
	let count = unsafe {
		PrivateExtractIconsW(
			name.as_ptr(),
			index,
			size,
			size,
			&raw mut handle,
			ptr::null_mut(),
			1,
			LR_DEFAULTCOLOR,
		)
	};
	match IconHandle::new(handle, Kind::Icon) {
		Some(icon) if count == 1 => Ok(icon),
		_ => Err(count),
	}
}

/// A bitmap `GetIconInfo` handed out; deleted on drop.
struct Bitmap(HBITMAP);

impl Bitmap {
	/// Returns the pixels as 32-bit BGRA, top-down.
	fn bits(&self, width: i32, height: i32) -> Result<Vec<u8>, String> {
		let invalid = || format!("{width}x{height} is no valid bitmap size");
		let rows = u32::try_from(height).ok().filter(|&rows| rows > 0).ok_or_else(invalid)?;
		let len = u32::try_from(width)
			.ok()
			.filter(|&columns| columns > 0)
			.and_then(|columns| columns.checked_mul(rows)?.checked_mul(4))
			.and_then(|len| usize::try_from(len).ok())
			.ok_or_else(invalid)?;
		let mut header = BITMAPINFO {
			bmiHeader: BITMAPINFOHEADER {
				biSize: u32::try_from(mem::size_of::<BITMAPINFOHEADER>()).map_err(|err| err.to_string())?,
				biWidth: width,
				// A negative height asks for the rows top-down
				biHeight: height.checked_neg().ok_or_else(invalid)?,
				biPlanes: 1,
				biBitCount: 32,
				biCompression: BI_RGB,
				biSizeImage: 0,
				biXPelsPerMeter: 0,
				biYPelsPerMeter: 0,
				biClrUsed: 0,
				biClrImportant: 0,
			},
			bmiColors: [RGBQUAD {
				rgbBlue: 0,
				rgbGreen: 0,
				rgbRed: 0,
				rgbReserved: 0,
			}],
		};
		let mut bits = vec![0_u8; len];
		let dc = MemoryDc::new().ok_or_else(|| format!("CreateCompatibleDC fails (error {})", last_error()))?;
		// SAFETY: a memory DC, a bitmap no DC has selected, a buffer for `rows` lines of 32-bit pixels,
		// and a header that describes exactly that buffer
		let lines = unsafe { GetDIBits(dc.0, self.0, 0, rows, bits.as_mut_ptr().cast(), &raw mut header, DIB_RGB_COLORS) };
		if lines == height {
			Ok(bits)
		} else {
			Err(format!("GetDIBits copies {lines} of {height} lines"))
		}
	}
}

impl Drop for Bitmap {
	fn drop(&mut self) {
		if !self.0.is_null() {
			// SAFETY: GetIconInfo created the bitmap for the caller, and no other handle refers to it
			unsafe { DeleteObject(self.0) };
		}
	}
}

/// A memory device context for `GetDIBits`; deleted on drop.
struct MemoryDc(HDC);

impl MemoryDc {
	#[must_use]
	fn new() -> Option<Self> {
		// SAFETY: a null DC asks for a memory DC compatible with the screen
		let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
		(!dc.is_null()).then_some(Self(dc))
	}
}

impl Drop for MemoryDc {
	fn drop(&mut self) {
		// SAFETY: the DC came from CreateCompatibleDC and has no object selected
		unsafe { DeleteDC(self.0) };
	}
}

/// Returns an integer resource name or type, as `MAKEINTRESOURCEW` gives it.
#[must_use]
fn int_resource(id: u16) -> PCWSTR {
	ptr::without_provenance(usize::from(id))
}

/// Returns `text` as zero-terminated UTF-16.
#[must_use]
fn wide(text: &OsStr) -> Vec<u16> {
	text.encode_wide().chain([0]).collect()
}
