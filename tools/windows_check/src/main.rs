//! Loads every icon and cursor of this executable, of its DLL and of the files the build script wrote,
//! the way Windows does, and compares the pixels with the PNG stored in the resource.
//! Prints one row per image and exits with 1 if any check fails, with 2 outside of Windows.
//!
//! Checks per image of a module:
//! `lookup`: `LookupIconIdFromDirectoryEx` picks this image for its size from the group directory,
//! `create`: `CreateIconFromResourceEx` decodes the stored image,
//! `load`: `LoadImageW` finds the group and decodes the image, as `LoadIcon` and `LoadCursor` do,
//! `shell`: `PrivateExtractIconsW` extracts the icon from the file the way Explorer does.
//! Files are loaded with `LoadImageW` and `LR_LOADFROMFILE` (`file`).

#[cfg(windows)]
mod check;
#[cfg(any(windows, test))]
mod decode;
#[cfg(windows)]
mod win32;

use std::process::ExitCode;

#[cfg(windows)]
fn main() -> ExitCode {
	check::run()
}

#[cfg(not(windows))]
fn main() -> ExitCode {
	eprintln!("The check needs Windows: it tests the image loader of user32");
	ExitCode::from(2)
}
