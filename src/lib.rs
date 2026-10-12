//! Builds the Win32 resources of Windows executables and DLLs on every platform, without `rc.exe`.
//!
//! A build script collects a manifest, version information, icons and cursors in a [`ResourceFile`](resource::ResourceFile),
//! writes it as COFF object and hands that to the linker.
//! Nothing is parsed from `.rc` text: every resource comes from typed values,
//! which are checked when they are created, so a broken resource fails the build instead of the program.
//! Every text the resources store must be in Unicode normalization form C (NFC);
//! the crate checks it and never normalizes on its own, so no name changes behind the back of its author.
//!
//! What is checked:
//! - `.ico`, `.cur` and PNG input is untrusted: it is parsed with bounded memory and time,
//!   and every image must meet a strict subset of what Windows loads.
//! - Version information and the typed settings of the standard manifest always produce values Windows accepts.
//! - Manifests from text, raw XML and custom windows settings are only checked to be well-formed XML.
//! - SVG documents get basic protection only: a size limit, no DTD, a node limit and no external resources.
//!
//! Recommended for every executable: the standard manifest, version information with a file description,
//! the company name and the copyright, and an icon with every common size as icon 1,
//! which Explorer shows for the file.
//!
//! ```no_run
//! // build.rs
//! use win_resource_builder::manifest::Manifest;
//! use win_resource_builder::resource::{CoffMachine, ResourceFile};
//! use win_resource_builder::version_info::{Version, VersionInfo};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//! 	if std::env::var("CARGO_CFG_TARGET_OS")? != "windows" {
//! 		return Ok(());
//! 	}
//!
//! 	let mut file = ResourceFile::new();
//! 	file.insert_manifest(Manifest::standard().create()?)?;
//!
//! 	let version_info = VersionInfo::options()
//! 		.file_description("Example application")
//! 		.company_name("Example Ltd.")
//! 		.legal_copyright("Copyright 2026 Example Ltd.")
//! 		.create("Example", "example.exe", Version::new(1, 2, 3, 0))?;
//! 	file.insert_version_info(version_info)?;
//!
//! 	let object = std::path::Path::new(&std::env::var("OUT_DIR")?).join("resources.obj");
//! 	file.write_coff(std::fs::File::create(&object)?, CoffMachine::cargo_target()?)?;
//! 	println!("cargo::rustc-link-arg-bins={}", object.display());
//!
//! 	Ok(())
//! }
//! ```
//!
//! # Features
//! - `images` (default): icons and cursors, optimized with oxipng, which brings libdeflate (C).
//!   Without it, the crate builds manifests and version information with a few pure Rust dependencies.
//! - `parallel`: oxipng uses every core through rayon.
//!   rayon ignores the jobserver of Cargo and competes with the other build jobs, so it is off by default.
//! - `svg`: renders SVG documents into icons and cursors with resvg.
//!   Recommended is to render once, keep the `.ico` or `.cur` file and build without this feature.

#[cfg(feature = "images")]
pub mod image;
pub mod manifest;
pub mod resource;
pub mod version_info;

use std::num::NonZeroU16;
use unicode_normalization::{UnicodeNormalization as _, is_nfc};

const _: () = {
	const fn send_sync<T>()
	where
		T: Send + Sync,
	{
	}
	send_sync::<resource::ResourceFile>();
	send_sync::<resource::InsertError>();
	#[cfg(feature = "images")]
	send_sync::<image::Icon>();
	#[cfg(feature = "images")]
	send_sync::<image::Cursor>();
	#[cfg(feature = "images")]
	send_sync::<image::IconOptions>();
	#[cfg(feature = "images")]
	send_sync::<image::ImageError>();
	send_sync::<manifest::Manifest>();
	send_sync::<manifest::ManifestOptions>();
	send_sync::<manifest::StandardManifestOptions>();
	send_sync::<version_info::VersionInfo>();
	send_sync::<version_info::VersionInfoOptions>();
};

#[cfg(not(any(target_pointer_width = "32", target_pointer_width = "64")))]
compile_error!("win-resource-builder needs a pointer width of 32 or 64 bits");

/// Widens a [`u32`] to [`usize`], which holds every value on the supported pointer widths.
#[must_use]
const fn usize_from_u32(value: u32) -> usize {
	let [a, b, c, d] = value.to_le_bytes();
	#[cfg(target_pointer_width = "32")]
	let bytes = [a, b, c, d];
	#[cfg(target_pointer_width = "64")]
	let bytes = [a, b, c, d, 0, 0, 0, 0];
	usize::from_le_bytes(bytes)
}

/// Widens a [`usize`] to [`u64`], which holds every value on the supported pointer widths.
#[cfg(any(test, feature = "images"))]
#[must_use]
const fn u64_from_usize(value: usize) -> u64 {
	#[cfg(target_pointer_width = "32")]
	let bytes = {
		let [a, b, c, d] = value.to_le_bytes();
		[a, b, c, d, 0, 0, 0, 0]
	};
	#[cfg(target_pointer_width = "64")]
	let bytes = value.to_le_bytes();
	u64::from_le_bytes(bytes)
}

/// Returns the resource ordinal `value` for constants; zero saturates to 1.
#[must_use]
const fn ordinal(value: u16) -> NonZeroU16 {
	NonZeroU16::MIN.saturating_add(value.saturating_sub(1))
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
	out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
	out.extend_from_slice(&value.to_le_bytes());
}

/// Returns the byte offset of the first character that Unicode normalization form C changes,
/// or [`None`] for text that is in NFC already.
///
/// ASCII is always in NFC and passes the quick check of `is_nfc` without a lookup.
#[must_use]
fn nfc_violation(text: &str) -> Option<usize> {
	if is_nfc(text) {
		return None;
	}
	let mut normalized = text.nfc();
	let offset = text
		.char_indices()
		.find(|&(_, character)| normalized.next() != Some(character))
		.map_or(text.len(), |(offset, _)| offset);
	Some(offset)
}

/// Appends zeros up to the next multiple of `alignment`, which is a power of two.
fn pad(out: &mut Vec<u8>, alignment: usize) {
	if let Some(end) = out.len().checked_next_multiple_of(alignment) {
		out.resize(end, 0);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn conversions_keep_every_value() {
		for value in [0, 1, 0x1234_5678, u32::MAX] {
			assert_eq!(u64_from_usize(usize_from_u32(value)), u64::from(value));
		}
		assert_eq!(u64_from_usize(usize::MAX), u64::try_from(usize::MAX).unwrap());
		assert_eq!((ordinal(1).get(), ordinal(24).get(), ordinal(0).get()), (1, 24, 1));
	}

	#[test]
	fn nfc_violations_name_the_first_changed_character() {
		for normalized in ["", "ascii", "Caf\u{E9}", "\u{1E9B}\u{323}", "\u{338}", "\u{AC00}"] {
			assert_eq!(nfc_violation(normalized), None, "{normalized:?}");
		}
		assert_eq!(nfc_violation("Cafe\u{301}"), Some(3), "Decomposed e with acute");
		assert_eq!(nfc_violation("xy\u{1100}\u{1161}"), Some(2), "Hangul jamo after a valid prefix");
		assert_eq!(
			nfc_violation(">\u{338}"),
			Some(0),
			"Two NFC strings concatenate to a composable pair"
		);
		assert_eq!(nfc_violation("\u{212B}"), Some(0), "The Angstrom sign maps to A with ring");
	}
}
