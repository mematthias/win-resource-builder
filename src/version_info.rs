//! `RT_VERSION`: the version information Explorer shows on the details page of the file properties.
//!
//! Product name, original file name and version are required.
//! Recommended are also the file description, which Task Manager shows as name of the process,
//! the company name and the copyright.
//! The version is stored as numbers as well, which installers compare before they replace a file.
//! Every string is stored as UTF-16 and must be in Unicode normalization form C.
//!
//! ```no_run
//! use win_resource_builder::resource::ResourceFile;
//! use win_resource_builder::version_info::{Version, VersionInfo};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//! 	let version_info = VersionInfo::options()
//! 		.file_description("Example application")
//! 		.company_name("Example Ltd.")
//! 		.legal_copyright("Copyright 2026 Example Ltd.")
//! 		.create("Example", "example.exe", Version::new(1, 2, 3, 0))?;
//! 	let mut file = ResourceFile::new();
//! 	file.insert_version_info(&version_info)?;
//! 	Ok(())
//! }
//! ```

use crate::resource::{InsertError, LanguageId, MOVEABLE, NewEntry, PURE, RT_VERSION, ResourceFile, ResourceKey};
use crate::{nfc_violation, pad, put_u16, put_u32};
use bitflags::bitflags;
use std::fmt;
use std::num::NonZeroU16;
use std::sync::Arc;
use thiserror::Error;

/// The kind of file, stored as `dwFileType`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FileType {
	/// An executable, the default.
	Application,
	/// A DLL.
	DynamicLibrary,
}

bitflags! {
	/// Flags that describe the build, stored as `dwFileFlags`.
	#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
	#[repr(transparent)]
	pub struct VersionFlags: u32 {
		/// Built with debug information.
		const DEBUG = 0x0000_0001;
		/// A development version, not a release.
		const PRERELEASE = 0x0000_0002;
		/// Changed after the release with the same version number.
		const PATCHED = 0x0000_0004;
	}
}

/// A four-part version `major.minor.patch.build`, each part from 0 to 65535.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Version {
	major: u16,
	minor: u16,
	patch: u16,
	build: u16,
}

impl Version {
	/// Returns the version `major.minor.patch.build`.
	#[must_use]
	pub const fn new(major: u16, minor: u16, patch: u16, build: u16) -> Self {
		Self {
			major,
			minor,
			patch,
			build,
		}
	}

	/// Returns the first part.
	#[must_use]
	pub const fn major(self) -> u16 {
		self.major
	}

	/// Returns the second part.
	#[must_use]
	pub const fn minor(self) -> u16 {
		self.minor
	}

	/// Returns the third part.
	#[must_use]
	pub const fn patch(self) -> u16 {
		self.patch
	}

	/// Returns the fourth part.
	#[must_use]
	pub const fn build(self) -> u16 {
		self.build
	}
}

impl fmt::Display for Version {
	#[inline]
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}.{}.{}.{}", self.major, self.minor, self.patch, self.build)
	}
}

/// Checked and encoded version information, ready to insert.
#[derive(Clone, Eq, PartialEq)]
pub struct VersionInfo {
	file_version: Version,
	product_version: Version,
	language: LanguageId,
	data: Arc<[u8]>,
}

impl VersionInfo {
	/// Returns options with every optional string unset, file type [`Application`](FileType::Application) and neutral language.
	#[must_use]
	pub const fn options() -> VersionInfoOptions {
		VersionInfoOptions {
			file_type: FileType::Application,
			flags: VersionFlags::empty(),
			language: LanguageId::NEUTRAL,
			product_version: None,
			comments: None,
			company_name: None,
			file_description: None,
			internal_name: None,
			legal_copyright: None,
			legal_trademarks: None,
			private_build: None,
			special_build: None,
			allow_bidi_controls: false,
		}
	}

	/// Returns the file version, stored as numbers and as `FileVersion`.
	#[must_use]
	pub const fn file_version(&self) -> Version {
		self.file_version
	}

	/// Returns the product version, stored as numbers and as `ProductVersion`.
	#[must_use]
	pub const fn product_version(&self) -> Version {
		self.product_version
	}

	/// Returns the language of the strings and of the resource.
	#[must_use]
	pub const fn language(&self) -> LanguageId {
		self.language
	}

	/// Returns the encoded `VS_VERSIONINFO` block.
	#[must_use]
	pub fn data(&self) -> &[u8] {
		&self.data
	}
}

/// Options of version information; strings left unset are not written.
#[derive(Clone, Debug)]
pub struct VersionInfoOptions {
	file_type: FileType,
	flags: VersionFlags,
	language: LanguageId,
	product_version: Option<Version>,
	comments: Option<String>,
	company_name: Option<String>,
	file_description: Option<String>,
	internal_name: Option<String>,
	legal_copyright: Option<String>,
	legal_trademarks: Option<String>,
	private_build: Option<String>,
	special_build: Option<String>,
	allow_bidi_controls: bool,
}

impl VersionInfoOptions {
	/// Checks every string and encodes the version information.
	///
	/// Every string must be non-empty, free of control characters and in Unicode normalization form C;
	/// the crate does not normalize, as a normalized `original_filename` could name another file.
	/// Explorer shows `product_name` and `original_filename`;
	/// the original file name also lets tools detect a renamed file.
	/// `file_version` is written as numbers and as text, and so is the product version, which defaults to it.
	pub fn create(self, product_name: &str, original_filename: &str, file_version: Version) -> Result<VersionInfo, VersionInfoError> {
		if !VersionFlags::all().contains(self.flags) {
			return Err(VersionInfoError::UndefinedFlags { bits: self.flags.bits() });
		}
		let mut file_flags = self.flags.bits();
		if self.private_build.is_some() {
			file_flags |= 0x08; // VS_FF_PRIVATEBUILD
		}
		if self.special_build.is_some() {
			file_flags |= 0x20; // VS_FF_SPECIALBUILD
		}

		let product_version = self.product_version.unwrap_or(file_version);
		let file_version_text = file_version.to_string();
		let product_version_text = product_version.to_string();
		// Sorted by key, as rc.exe writes them
		let strings = [
			("Comments", self.comments.as_deref()),
			("CompanyName", self.company_name.as_deref()),
			("FileDescription", self.file_description.as_deref()),
			("FileVersion", Some(file_version_text.as_str())),
			("InternalName", self.internal_name.as_deref()),
			("LegalCopyright", self.legal_copyright.as_deref()),
			("LegalTrademarks", self.legal_trademarks.as_deref()),
			("OriginalFilename", Some(original_filename)),
			("PrivateBuild", self.private_build.as_deref()),
			("ProductName", Some(product_name)),
			("ProductVersion", Some(product_version_text.as_str())),
			("SpecialBuild", self.special_build.as_deref()),
		];
		let present = || strings.iter().filter_map(|&(key, value)| Some((key, value?)));
		for (key, value) in present() {
			check_string(key, value, self.allow_bidi_controls)?;
		}

		// wLength excludes the trailing padding and wType is 1 for every block, as in rc.exe
		let mut out = Vec::with_capacity(1024);
		write_node(&mut out, "VS_VERSION_INFO", 0, 52, |out| {
			put_u32(out, 0xFEEF_04BD); // dwSignature
			put_u32(out, 0x0001_0000); // dwStrucVersion
			write_version(out, file_version);
			write_version(out, product_version);
			put_u32(out, 0x3F); // dwFileFlagsMask: every flag of verrsrc.h
			put_u32(out, file_flags);
			put_u32(out, 0x0004_0004); // VOS_NT_WINDOWS32
			put_u32(out, if self.file_type == FileType::Application { 1 } else { 2 });
			put_u32(out, 0); // dwFileSubtype
			put_u32(out, 0); // dwFileDateMS
			put_u32(out, 0); // dwFileDateLS
			write_node(out, "StringFileInfo", 1, 0, |out| {
				let table_key = format!("{:04X}04B0", self.language.raw());
				write_node(out, &table_key, 1, 0, |out| {
					for (key, value) in present() {
						let units = value
							.encode_utf16()
							.count()
							.checked_add(1)
							.and_then(|units| u16::try_from(units).ok())
							.ok_or(VersionInfoError::TooLarge)?;
						write_node(out, key, 1, units, |out| {
							put_utf16z(out, value);
							Ok(())
						})?;
					}
					Ok(())
				})
			})?;
			write_node(out, "VarFileInfo", 1, 0, |out| {
				write_node(out, "Translation", 0, 4, |out| {
					put_u16(out, self.language.raw());
					put_u16(out, 1200); // UTF-16
					Ok(())
				})
			})
		})?;
		Ok(VersionInfo {
			file_version,
			product_version,
			language: self.language,
			data: Arc::from(out),
		})
	}

	/// Sets `Comments`, a free text that Explorer does not show.
	#[must_use]
	pub fn comments(mut self, value: impl Into<String>) -> Self {
		self.comments = Some(value.into());
		self
	}

	/// Sets `CompanyName`, the publisher of the file; recommended.
	#[must_use]
	pub fn company_name(mut self, value: impl Into<String>) -> Self {
		self.company_name = Some(value.into());
		self
	}

	/// Sets `FileDescription`, which Explorer shows as description and Task Manager as process name;
	/// recommended.
	#[must_use]
	pub fn file_description(mut self, value: impl Into<String>) -> Self {
		self.file_description = Some(value.into());
		self
	}

	/// Sets the kind of file, an executable by default; DLLs set [`DynamicLibrary`](FileType::DynamicLibrary).
	#[must_use]
	pub const fn file_type(mut self, value: FileType) -> Self {
		self.file_type = value;
		self
	}

	/// Sets the build flags; the flags of private and special builds follow from their strings.
	#[must_use]
	pub const fn flags(mut self, value: VersionFlags) -> Self {
		self.flags = value;
		self
	}

	/// Sets `InternalName`, usually the file name without extension.
	#[must_use]
	pub fn internal_name(mut self, value: impl Into<String>) -> Self {
		self.internal_name = Some(value.into());
		self
	}

	/// Sets the language of the strings, neutral by default.
	#[must_use]
	pub const fn language(mut self, value: LanguageId) -> Self {
		self.language = value;
		self
	}

	/// Sets `LegalCopyright`, which Explorer shows as copyright; recommended.
	#[must_use]
	pub fn legal_copyright(mut self, value: impl Into<String>) -> Self {
		self.legal_copyright = Some(value.into());
		self
	}

	/// Sets `LegalTrademarks`.
	#[must_use]
	pub fn legal_trademarks(mut self, value: impl Into<String>) -> Self {
		self.legal_trademarks = Some(value.into());
		self
	}

	/// Sets `PrivateBuild`, a note on a build made outside the regular process, and its flag.
	#[must_use]
	pub fn private_build(mut self, value: impl Into<String>) -> Self {
		self.private_build = Some(value.into());
		self
	}

	/// Sets the version of the product the file belongs to, which defaults to the file version.
	#[must_use]
	pub const fn product_version(mut self, value: Version) -> Self {
		self.product_version = Some(value);
		self
	}

	/// Sets `SpecialBuild`, a note on how the build differs from the regular one, and its flag.
	#[must_use]
	pub fn special_build(mut self, value: impl Into<String>) -> Self {
		self.special_build = Some(value.into());
		self
	}

	/// Accepts the text direction controls U+202A to U+202E and U+2066 to U+2069 in strings;
	/// off by default.
	///
	/// They make Explorer show a name in another order than it is stored, the way "Trojan Source" hides code,
	/// so only turn it on for right-to-left text that needs them.
	#[must_use]
	pub const fn allow_bidi_controls(mut self, value: bool) -> Self {
		self.allow_bidi_controls = value;
		self
	}
}

/// Shows versions, language and the length of the data, not the data itself.
impl fmt::Debug for VersionInfo {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("VersionInfo")
			.field("file_version", &self.file_version)
			.field("product_version", &self.product_version)
			.field("language", &self.language)
			.field("len", &self.data.len())
			.finish_non_exhaustive()
	}
}

impl ResourceFile {
	/// Inserts version information as resource 1 of `RT_VERSION` in its language.
	///
	/// One per language; Windows shows the one in the language of the user interface or the neutral one.
	/// The resource file shares the data with the version information, so no byte is copied;
	/// the version information stays usable, after a failed insertion as well, and can go into further files.
	pub fn insert_version_info(&mut self, value: &VersionInfo) -> Result<(), InsertError> {
		let key = ResourceKey::new(RT_VERSION, NonZeroU16::MIN, value.language);
		self.insert_entries(vec![NewEntry::new(key, MOVEABLE | PURE, <Arc<[u8]>>::clone(&value.data))])
	}
}

/// Why version information could not be created.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum VersionInfoError {
	/// A string is empty.
	#[error("Version string '{key}' is empty")]
	EmptyValue { key: &'static str },

	/// A string contains a control character.
	#[error("Version string '{key}' contains {character:?} at byte {offset}")]
	ControlCharacter { key: &'static str, offset: usize, character: char },

	/// A string contains a text direction control, and [`allow_bidi_controls`](VersionInfoOptions::allow_bidi_controls) is off.
	#[error("Version string '{key}' contains the text direction control {character:?} at byte {offset}")]
	BidiControl { key: &'static str, offset: usize, character: char },

	/// A string is not in Unicode normalization form C; `offset` is the first byte that differs.
	#[error("Version string '{key}' is not in Unicode normalization form C from byte {offset} on")]
	NotNormalized { key: &'static str, offset: usize },

	/// The flags contain bits that [`VersionFlags`] does not define.
	#[error("Version flags 0x{bits:08X} contain undefined bits")]
	UndefinedFlags { bits: u32 },

	/// The version information exceeds the 65535 bytes its length fields can describe.
	#[error("Version information exceeds the 65535 bytes its 16-bit length fields can describe")]
	TooLarge,
}

fn check_string(key: &'static str, value: &str, allow_bidi_controls: bool) -> Result<(), VersionInfoError> {
	if value.is_empty() {
		return Err(VersionInfoError::EmptyValue { key });
	}
	for (offset, character) in value.char_indices() {
		if character.is_control() {
			return Err(VersionInfoError::ControlCharacter { key, offset, character });
		}
		if !allow_bidi_controls && matches!(character, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}') {
			return Err(VersionInfoError::BidiControl { key, offset, character });
		}
	}
	// Every string is a node of its own, so no string composes with a neighbor
	match nfc_violation(value) {
		Some(offset) => Err(VersionInfoError::NotNormalized { key, offset }),
		None => Ok(()),
	}
}

/// Writes a node with its key and lets `body` write value and children,
/// then patches the length of the node into its first WORD.
fn write_node(
	out: &mut Vec<u8>,
	key: &str,
	node_type: u16,
	value_length: u16,
	body: impl FnOnce(&mut Vec<u8>) -> Result<(), VersionInfoError>,
) -> Result<(), VersionInfoError> {
	pad(out, 4);
	let start = out.len();
	put_u16(out, 0); // wLength, patched below
	put_u16(out, value_length);
	put_u16(out, node_type);
	put_utf16z(out, key);
	pad(out, 4);
	body(out)?;
	let length = out
		.len()
		.checked_sub(start)
		.and_then(|length| u16::try_from(length).ok())
		.ok_or(VersionInfoError::TooLarge)?;
	if let Some(field) = out.get_mut(start..).and_then(<[u8]>::first_chunk_mut::<2>) {
		*field = length.to_le_bytes();
	}
	Ok(())
}

fn put_utf16z(out: &mut Vec<u8>, text: &str) {
	for unit in text.encode_utf16().chain([0]) {
		put_u16(out, unit);
	}
}

fn write_version(out: &mut Vec<u8>, version: Version) {
	put_u16(out, version.minor);
	put_u16(out, version.major);
	put_u16(out, version.build);
	put_u16(out, version.patch);
}

#[cfg(test)]
#[allow(
	clippy::arithmetic_side_effects,
	reason = "Tests run with overflow checks, so every operator is strict there"
)]
mod tests {
	use super::*;

	#[derive(Debug)]
	struct Node {
		length: usize,
		value_length: u16,
		kind: u16,
		key: String,
		value: Vec<u8>,
		children: Vec<Node>,
	}

	impl Node {
		#[must_use]
		fn child(&self, key: &str) -> &Node {
			self.children.iter().find(|child| child.key == key).unwrap()
		}

		#[must_use]
		fn text(&self) -> String {
			let units: Vec<u16> = self
				.value
				.as_chunks::<2>()
				.0
				.iter()
				.map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
				.collect();
			String::from_utf16(units.strip_suffix(&[0]).unwrap()).unwrap()
		}
	}

	#[must_use]
	fn parse(bytes: &[u8]) -> Node {
		let word = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
		let length = usize::from(word(0));
		let (value_length, kind) = (word(2), word(4));
		let mut position = 6;
		let mut key = Vec::new();
		while word(position) != 0 {
			key.push(word(position));
			position += 2;
		}
		position = (position + 2).next_multiple_of(4);
		let value_size = if kind == 1 {
			usize::from(value_length) * 2
		} else {
			usize::from(value_length)
		};
		let value = bytes[position..position + value_size].to_vec();
		position = (position + value_size).next_multiple_of(4);
		let mut children = Vec::new();
		while position < length {
			let child = parse(&bytes[position..]);
			position = (position + child.length).next_multiple_of(4);
			children.push(child);
		}
		Node {
			length,
			value_length,
			kind,
			key: String::from_utf16(&key).unwrap(),
			value,
			children,
		}
	}

	#[must_use]
	fn strings(root: &Node) -> Vec<(String, String)> {
		let table = &root.child("StringFileInfo").children[0];
		table
			.children
			.iter()
			.map(|string| (<String>::clone(&string.key), string.text()))
			.collect()
	}

	#[test]
	fn the_minimal_tree_has_the_documented_layout() {
		let info = VersionInfo::options()
			.create("Example Product", "example.exe", Version::new(1, 2, 3, 4))
			.unwrap();
		let root = parse(info.data());
		assert_eq!((root.length, info.data().len() % 4), (info.data().len(), 0));
		assert_eq!((root.key.as_str(), root.kind, root.value_length), ("VS_VERSION_INFO", 0, 52));
		#[rustfmt::skip]
		let fixed = [
			0xBD, 0x04, 0xEF, 0xFE, 0x00, 0x00, 0x01, 0x00, 0x02, 0x00, 0x01, 0x00, 0x04, 0x00, 0x03, 0x00,
			0x02, 0x00, 0x01, 0x00, 0x04, 0x00, 0x03, 0x00, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
			0x04, 0x00, 0x04, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
			0x00, 0x00, 0x00, 0x00,
		];
		assert_eq!(root.value, fixed);
		assert_eq!(root.child("StringFileInfo").children[0].key, "000004B0");
		assert_eq!(
			strings(&root),
			[
				("FileVersion".to_owned(), "1.2.3.4".to_owned()),
				("OriginalFilename".to_owned(), "example.exe".to_owned()),
				("ProductName".to_owned(), "Example Product".to_owned()),
				("ProductVersion".to_owned(), "1.2.3.4".to_owned()),
			]
		);
		let translation = root.child("VarFileInfo").child("Translation");
		assert_eq!((translation.kind, translation.value.as_slice()), (0, &[0x00, 0x00, 0xB0, 0x04][..]));
	}

	#[test]
	fn options_reach_the_tree() {
		let german = LanguageId::GERMAN;
		let info = VersionInfo::options()
			.language(german)
			.file_type(FileType::DynamicLibrary)
			.flags(VersionFlags::DEBUG)
			.product_version(Version::new(9, 0, 0, 0))
			.comments("C")
			.company_name(String::from("Gr\u{FC}n GmbH"))
			.private_build("P")
			.special_build("S")
			.create("Product", "lib.dll", Version::new(1, 0, 0, 0))
			.unwrap();
		let root = parse(info.data());
		assert_eq!(&root.value[24..28], &[0x3F, 0, 0, 0], "dwFileFlagsMask");
		assert_eq!(&root.value[28..32], &[0x29, 0, 0, 0], "DEBUG, PRIVATEBUILD and SPECIALBUILD");
		assert_eq!(&root.value[36..40], &[2, 0, 0, 0], "VFT_DLL");
		assert_eq!(root.child("StringFileInfo").children[0].key, "040704B0");
		let strings = strings(&root);
		assert!(strings.contains(&("CompanyName".to_owned(), "Gr\u{FC}n GmbH".to_owned())));
		assert!(strings.contains(&("ProductVersion".to_owned(), "9.0.0.0".to_owned())));
		assert!(strings.windows(2).all(|pair| pair[0].0 < pair[1].0), "Sorted by key");
		assert_eq!((info.language(), info.product_version()), (german, Version::new(9, 0, 0, 0)));
	}

	#[test]
	fn invalid_input_is_rejected() {
		let create = |options: VersionInfoOptions, name: &str| options.create(name, "a.exe", Version::default());
		assert_eq!(
			create(VersionInfo::options(), ""),
			Err(VersionInfoError::EmptyValue { key: "ProductName" })
		);
		assert_eq!(
			create(VersionInfo::options().comments("a\nb"), "P"),
			Err(VersionInfoError::ControlCharacter {
				key: "Comments",
				offset: 1,
				character: '\n'
			})
		);
		assert_eq!(
			create(VersionInfo::options().flags(VersionFlags::from_bits_retain(0x100)), "P"),
			Err(VersionInfoError::UndefinedFlags { bits: 0x100 })
		);
		assert_eq!(
			create(VersionInfo::options().comments("x".repeat(40_000)), "P"),
			Err(VersionInfoError::TooLarge)
		);

		// "setup\u{202E}gpj.exe" shows as "setupexe.jpg"
		let spoofed = "setup\u{202E}gpj.exe";
		assert_eq!(
			VersionInfo::options().create("P", spoofed, Version::default()),
			Err(VersionInfoError::BidiControl {
				key: "OriginalFilename",
				offset: 5,
				character: '\u{202E}'
			})
		);
		assert!(
			VersionInfo::options()
				.allow_bidi_controls(true)
				.create("P", spoofed, Version::default())
				.is_ok()
		);
		assert!(
			create(VersionInfo::options().company_name("\u{5D0}\u{200F}Co"), "P").is_ok(),
			"Marks are no controls"
		);

		// "Cafe" with a combining acute accent is the decomposed form of "Caf\u{E9}"
		assert!(create(VersionInfo::options().company_name("Caf\u{E9}"), "P").is_ok());
		assert_eq!(
			create(VersionInfo::options().company_name("Cafe\u{301}"), "P"),
			Err(VersionInfoError::NotNormalized {
				key: "CompanyName",
				offset: 3
			})
		);
		assert_eq!(
			VersionInfo::options().create("P", "\u{212B}.exe", Version::default()),
			Err(VersionInfoError::NotNormalized {
				key: "OriginalFilename",
				offset: 0
			}),
			"The Angstrom sign is not in NFC"
		);
	}

	#[test]
	fn a_second_version_info_needs_another_language() {
		let info = |language| {
			VersionInfo::options()
				.language(language)
				.create("P", "a.exe", Version::default())
				.unwrap()
		};
		let mut file = ResourceFile::new();
		let neutral = info(LanguageId::NEUTRAL);
		let data_at = neutral.data().as_ptr();
		let summary = format!(", len: {}, .. }}", neutral.data().len());
		assert!(format!("{neutral:?}").ends_with(&summary), "No raw bytes: {neutral:?}");
		file.insert_version_info(&neutral).unwrap();
		let key = ResourceKey::new(RT_VERSION, NonZeroU16::MIN, LanguageId::NEUTRAL);
		assert_eq!(
			file.entry(key).map(|(_, data)| data.as_ptr()),
			Some(data_at),
			"The data moves without a copy"
		);
		file.insert_version_info(&info(LanguageId::ENGLISH_US)).unwrap();
		assert_eq!(
			file.insert_version_info(&info(LanguageId::NEUTRAL)),
			Err(InsertError::DuplicateResource {
				resource_type: 16,
				name: 1,
				language: 0
			})
		);
		assert_eq!(file.len(), 2);
		assert_eq!(<VersionInfo>::clone(&neutral).data().as_ptr(), data_at, "Clones share the data");
	}
}
