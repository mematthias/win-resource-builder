//! The resource file: collects resources and writes them as `.res` file or as COFF object.
//!
//! Every resource is stored under a type, a name and a language;
//! each kind of resource inserts itself with a function of its own module.
//! The output is deterministic: the same resources give the same bytes in any order of insertion.
//! A failed insertion leaves the file unchanged,
//! and every insertion checks the size of the output, so writing it never fails for the content.

use crate::{ordinal, pad, put_u16, put_u32, usize_from_u32};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::ffi::OsString;
use std::io::{self, BufWriter, Write};
use std::num::NonZeroU16;
use std::sync::Arc;
use std::{env, fmt};
use thiserror::Error;

/// The machine type of a COFF object, which has to match the target of the linked binary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CoffMachine {
	/// 32-bit x86, `IMAGE_FILE_MACHINE_I386`.
	X86,
	/// x86-64, `IMAGE_FILE_MACHINE_AMD64`.
	X64,
	/// 32-bit ARM in Thumb-2 mode, `IMAGE_FILE_MACHINE_ARMNT`.
	Arm,
	/// 64-bit ARM, `IMAGE_FILE_MACHINE_ARM64`.
	Arm64,
	/// ARM64EC, the machine of `arm64ec-pc-windows-msvc`.
	Arm64Ec,
}

impl CoffMachine {
	/// Returns the machine this code is compiled for.
	///
	/// In a build script that is the host, not the target of the build, which [`cargo_target`](Self::cargo_target) reads instead.
	#[cfg(any(
		target_arch = "x86",
		target_arch = "x86_64",
		target_arch = "arm",
		target_arch = "aarch64",
		target_arch = "arm64ec"
	))]
	#[must_use]
	pub const fn host() -> Self {
		if cfg!(target_arch = "x86") {
			Self::X86
		} else if cfg!(target_arch = "x86_64") {
			Self::X64
		} else if cfg!(target_arch = "arm") {
			Self::Arm
		} else if cfg!(target_arch = "arm64ec") {
			Self::Arm64Ec
		} else {
			Self::Arm64
		}
	}

	/// Returns the machine of the target Cargo builds, read from `CARGO_CFG_TARGET_ARCH`.
	///
	/// Build scripts run on the host, so the machine of the resource object has to come from Cargo.
	/// Supported are `x86`, `x86_64`, `arm`, `aarch64` and `arm64ec`.
	pub fn cargo_target() -> Result<Self, CargoTargetError> {
		Self::from_target_arch(env::var_os("CARGO_CFG_TARGET_ARCH"))
	}

	fn from_target_arch(value: Option<OsString>) -> Result<Self, CargoTargetError> {
		let arch = value
			.ok_or(CargoTargetError::NotSet)?
			.into_string()
			.map_err(|_| CargoTargetError::NotUnicode)?;
		match arch.as_str() {
			"x86" => Ok(Self::X86),
			"x86_64" => Ok(Self::X64),
			"arm" => Ok(Self::Arm),
			"aarch64" => Ok(Self::Arm64),
			"arm64ec" => Ok(Self::Arm64Ec),
			_ => Err(CargoTargetError::UnsupportedArch { arch }),
		}
	}

	#[must_use]
	const fn id(self) -> u16 {
		match self {
			Self::X86 => 0x014C,
			Self::X64 => 0x8664,
			Self::Arm => 0x01C4,
			Self::Arm64 => 0xAA64,
			Self::Arm64Ec => 0xA641,
		}
	}

	/// `IMAGE_REL_I386_DIR32NB`, `IMAGE_REL_AMD64_ADDR32NB` and `IMAGE_REL_ARM*_ADDR32NB`.
	#[must_use]
	const fn relocation_type(self) -> u16 {
		match self {
			Self::X86 => 0x0007,
			Self::X64 => 0x0003,
			Self::Arm | Self::Arm64 | Self::Arm64Ec => 0x0002,
		}
	}
}

/// The language of a resource, a Windows `LANGID`.
///
/// Windows picks the resource in the language of the user interface and falls back to neutral,
/// so resources that are not translated are best stored as neutral only.
/// Constants name the common languages, each in its main region;
/// [`from_parts`](Self::from_parts) and [`from_raw`](Self::from_raw) give every other one.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct LanguageId(u16);

impl LanguageId {
	/// Language neutral, the recommended language of resources that are not translated.
	pub const NEUTRAL: Self = Self(0);

	/// English (United States), `en-US`.
	pub const ENGLISH_US: Self = Self(0x0409);
	/// English (United Kingdom), `en-GB`.
	pub const ENGLISH_UK: Self = Self(0x0809);
	/// German (Germany), `de-DE`.
	pub const GERMAN: Self = Self(0x0407);
	/// French (France), `fr-FR`.
	pub const FRENCH: Self = Self(0x040C);
	/// Spanish (Spain), modern sort, `es-ES`.
	pub const SPANISH: Self = Self(0x0C0A);
	/// Italian (Italy), `it-IT`.
	pub const ITALIAN: Self = Self(0x0410);
	/// Portuguese (Brazil), `pt-BR`.
	pub const PORTUGUESE_BRAZIL: Self = Self(0x0416);
	/// Portuguese (Portugal), `pt-PT`.
	pub const PORTUGUESE_PORTUGAL: Self = Self(0x0816);
	/// Dutch (Netherlands), `nl-NL`.
	pub const DUTCH: Self = Self(0x0413);
	/// Polish (Poland), `pl-PL`.
	pub const POLISH: Self = Self(0x0415);
	/// Czech (Czech Republic), `cs-CZ`.
	pub const CZECH: Self = Self(0x0405);
	/// Swedish (Sweden), `sv-SE`.
	pub const SWEDISH: Self = Self(0x041D);
	/// Turkish (Turkey), `tr-TR`.
	pub const TURKISH: Self = Self(0x041F);
	/// Russian (Russia), `ru-RU`.
	pub const RUSSIAN: Self = Self(0x0419);
	/// Ukrainian (Ukraine), `uk-UA`.
	pub const UKRAINIAN: Self = Self(0x0422);
	/// Arabic (Saudi Arabia), `ar-SA`.
	pub const ARABIC: Self = Self(0x0401);
	/// Hindi (India), `hi-IN`.
	pub const HINDI: Self = Self(0x0439);
	/// Japanese (Japan), `ja-JP`.
	pub const JAPANESE: Self = Self(0x0411);
	/// Korean (Korea), `ko-KR`.
	pub const KOREAN: Self = Self(0x0412);
	/// Chinese, simplified (China), `zh-CN`.
	pub const CHINESE_SIMPLIFIED: Self = Self(0x0804);
	/// Chinese, traditional (Taiwan), `zh-TW`.
	pub const CHINESE_TRADITIONAL: Self = Self(0x0404);

	/// Returns the language of a raw `LANGID`, such as 0x0409 for English (United States).
	#[must_use]
	pub const fn from_raw(value: u16) -> Self {
		Self(value)
	}

	/// Returns the language of a primary language up to 0x3FF and a sublanguage up to 0x3F,
	/// or [`None`] beyond.
	#[must_use]
	pub const fn from_parts(primary: u16, sub: u16) -> Option<Self> {
		if primary > 0x03FF || sub > 0x003F {
			return None;
		}
		// Both parts are checked, so the shift keeps every bit
		Some(Self(sub.strict_shl(10) | primary))
	}

	/// Returns the raw `LANGID`.
	#[must_use]
	pub const fn raw(self) -> u16 {
		self.0
	}

	/// Returns the primary language, the low 10 bits.
	#[must_use]
	pub const fn primary(self) -> u16 {
		self.0 & 0x03FF
	}

	/// Returns the sublanguage, the high 6 bits.
	#[must_use]
	pub const fn sub(self) -> u16 {
		self.0.strict_shr(10)
	}
}

/// The resources of one executable or DLL.
///
/// Insert the resources, then write the file once, as COFF object for the linker.
/// A failed insertion leaves the file unchanged.
#[derive(Eq, PartialEq)]
pub struct ResourceFile {
	entries: BTreeMap<ResourceKey, Entry>,
	/// The number of languages of every name, at most 65535 as the COFF directory counts them in a WORD.
	languages: BTreeMap<(NonZeroU16, NonZeroU16), u16>,
	/// The number of names of every type.
	names: BTreeMap<NonZeroU16, u16>,
	totals: Totals,
	coff: CoffLayout,
}

impl ResourceFile {
	/// Returns an empty file.
	#[must_use]
	pub const fn new() -> Self {
		Self {
			entries: BTreeMap::new(),
			languages: BTreeMap::new(),
			names: BTreeMap::new(),
			totals: Totals::EMPTY,
			coff: EMPTY_LAYOUT,
		}
	}

	/// Returns the number of resources, counting every image of an icon or cursor and its group.
	#[must_use]
	pub fn len(&self) -> usize {
		self.entries.len()
	}

	/// Returns whether the file holds no resource.
	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}

	/// Returns the file in the `.res` format of `rc.exe`.
	///
	/// `link.exe` and `lld-link` take it as input; other linkers need the COFF object.
	#[must_use]
	pub fn to_res(&self) -> Vec<u8> {
		let mut out = Vec::with_capacity(usize_from_u32(self.totals.res_size));
		match self.emit_res(&mut out) {
			Ok(()) => out,
			Err(never) => match never {},
		}
	}

	/// Writes the file in the `.res` format of `rc.exe` through an internal buffer.
	pub fn write_res(&self, writer: impl Write) -> io::Result<()> {
		let mut sink = BufWriter::new(writer);
		self.emit_res(&mut sink)?;
		sink.flush()
	}

	/// Returns the resources as COFF object for `machine`, which every linker of PE files takes.
	///
	/// The object equals what `llvm-cvtres` makes of the `.res` form, with a zero time stamp;
	/// from 65535 resources on it counts the relocations as the PE format specifies,
	/// where `llvm-cvtres` truncates the count.
	/// It defines no symbol that code could reference,
	/// so a linker only takes it as direct input, not from a static library.
	#[must_use]
	pub fn to_coff(&self, machine: CoffMachine) -> Vec<u8> {
		let mut out = Vec::with_capacity(usize_from_u32(self.coff.size));
		match self.emit_coff(&mut out, machine) {
			Ok(()) => out,
			Err(never) => match never {},
		}
	}

	/// Writes the COFF object for `machine` through an internal buffer,
	/// without holding the whole object in memory.
	///
	/// Recommended in build scripts, with the machine of [`CoffMachine::cargo_target`].
	pub fn write_coff(&self, writer: impl Write, machine: CoffMachine) -> io::Result<()> {
		let mut sink = BufWriter::new(writer);
		self.emit_coff(&mut sink, machine)?;
		sink.flush()
	}

	pub(crate) fn insert_entries(&mut self, new: Vec<NewEntry>) -> Result<(), InsertError> {
		let mut keys: Vec<ResourceKey> = new.iter().map(|entry| entry.key).collect();
		keys.sort_unstable();
		let duplicate = keys
			.windows(2)
			.find_map(|pair| match *pair {
				[first, second] if first == second => Some(first),
				_ => None,
			})
			.or_else(|| keys.iter().copied().find(|key| self.entries.contains_key(key)));
		if let Some(key) = duplicate {
			return Err(InsertError::DuplicateResource {
				resource_type: key.resource_type.get(),
				name: key.name.get(),
				language: key.language.raw(),
			});
		}

		// The new totals are computed first and applied only when everything fits
		let mut totals = self.totals;
		let mut languages = Vec::new();
		for group in keys.chunk_by(|first, second| first.name_key() == second.name_key()) {
			let Some(first) = group.first() else {
				continue;
			};
			let existing = self.languages.get(&first.name_key()).copied();
			let total = u16::try_from(group.len())
				.ok()
				.and_then(|added| existing.unwrap_or(0).checked_add(added))
				.ok_or(InsertError::TooManyLanguages {
					resource_type: first.resource_type.get(),
					name: first.name.get(),
				})?;
			languages.push((first.name_key(), total, existing.is_none()));
		}
		let mut names = Vec::new();
		for group in languages.chunk_by(|first, second| first.0.0 == second.0.0) {
			let Some(&((resource_type, _), ..)) = group.first() else {
				continue;
			};
			let existing = self.names.get(&resource_type).copied();
			// Names are distinct NonZeroU16 values, so a type never has more than 65535
			let added = u16::try_from(group.iter().filter(|&&(_, _, new_name)| new_name).count()).map_err(|_| InsertError::FileTooLarge)?;
			let total = existing.unwrap_or(0).checked_add(added).ok_or(InsertError::FileTooLarge)?;
			names.push((resource_type, total));
			totals.names = totals.names.checked_add(u32::from(added)).ok_or(InsertError::FileTooLarge)?;
			if existing.is_none() {
				totals.types = totals.types.checked_add(1).ok_or(InsertError::FileTooLarge)?;
			}
		}

		let mut entries = Vec::with_capacity(new.len());
		for NewEntry { key, flags, data } in new {
			let len = u32::try_from(data.len()).map_err(|_| InsertError::FileTooLarge)?;
			totals.add_entry(len).ok_or(InsertError::FileTooLarge)?;
			entries.push((key, Entry { flags, len, data }));
		}
		let coff = CoffLayout::of(&totals).ok_or(InsertError::FileTooLarge)?;

		for (key, count, _) in languages {
			self.languages.insert(key, count);
		}
		for (resource_type, count) in names {
			self.names.insert(resource_type, count);
		}
		self.entries.extend(entries);
		self.totals = totals;
		self.coff = coff;
		Ok(())
	}

	#[cfg(any(test, feature = "images"))]
	#[must_use]
	pub(crate) fn max_name(&self, resource_type: NonZeroU16) -> Option<NonZeroU16> {
		self.languages
			.range((resource_type, NonZeroU16::MIN)..=(resource_type, NonZeroU16::MAX))
			.next_back()
			.map(|(&(_, name), _)| name)
	}

	#[cfg(test)]
	#[must_use]
	pub(crate) fn entry(&self, key: ResourceKey) -> Option<(u16, &[u8])> {
		self.entries.get(&key).map(|entry| (entry.flags, &*entry.data))
	}

	/// Returns flags and data of a resource, the data shared with the file.
	#[cfg(feature = "images")]
	#[must_use]
	pub(crate) fn shared_entry(&self, key: ResourceKey) -> Option<(u16, Arc<[u8]>)> {
		self.entries.get(&key).map(|entry| (entry.flags, <Arc<[u8]>>::clone(&entry.data)))
	}

	fn emit_res<S: Sink>(&self, out: &mut S) -> Result<(), S::Error> {
		let mut header = Vec::with_capacity(32);
		write_res_header(&mut header, 0, [0, 0, 0], 0);
		out.put(&header)?;
		for (key, entry) in &self.entries {
			header.clear();
			write_res_header(
				&mut header,
				entry.len,
				[key.resource_type.get(), key.name.get(), key.language.raw()],
				entry.flags,
			);
			out.put(&header)?;
			out.put(&entry.data)?;
			out.put(padding(entry.len, 4))?;
		}
		Ok(())
	}

	// The object is laid out as llvm-cvtres writes it: file header, the sections .rsrc$01 with the
	// resource directory and one relocation per resource and .rsrc$02 with the data, then the symbols
	// @feat.00, both sections with their auxiliary records and $R000000 onwards, one per resource.
	// Every offset below is bounded by the layout that the last insertion checked to fit 32 bits.
	fn emit_coff<S: Sink>(&self, out: &mut S, machine: CoffMachine) -> Result<(), S::Error> {
		let layout = &self.coff;
		let count = self.totals.count;
		let types = u16::try_from(self.names.len()).unwrap_or(u16::MAX);
		let short_relocations = u16::try_from(layout.relocations).unwrap_or(u16::MAX);
		let overflow = layout.relocations > count;

		let mut head = Vec::with_capacity(usize_from_u32(layout.data_start));
		put_u16(&mut head, machine.id());
		put_u16(&mut head, 2); // NumberOfSections
		put_u32(&mut head, 0); // TimeDateStamp
		put_u32(&mut head, layout.symbols_start);
		put_u32(&mut head, count.strict_add(5));
		put_u16(&mut head, 0); // SizeOfOptionalHeader
		put_u16(&mut head, 0x0100); // IMAGE_FILE_32BIT_MACHINE, as cvtres writes for every machine
		// IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ, plus IMAGE_SCN_LNK_NRELOC_OVFL
		let flags = if overflow { 0x4100_0040 } else { 0x4000_0040 };
		let relocations_start = layout.directory.strict_add(100);
		write_section_header(
			&mut head,
			*b".rsrc$01",
			layout.directory,
			100,
			relocations_start,
			short_relocations,
			flags,
		);
		write_section_header(&mut head, *b".rsrc$02", layout.data_size, layout.data_start, 0, 0, 0x4000_0040);

		// Breadth first: the root, the type tables, the name tables and the data entries
		write_table_header(&mut head, types);
		let mut next = table_size(types);
		for (&resource_type, &names) in &self.names {
			put_u32(&mut head, u32::from(resource_type.get()));
			put_u32(&mut head, 0x8000_0000 | next);
			next = next.strict_add(table_size(names));
		}
		for (&resource_type, &names) in &self.names {
			write_table_header(&mut head, names);
			for (&(_, name), &languages) in self
				.languages
				.range((resource_type, NonZeroU16::MIN)..=(resource_type, NonZeroU16::MAX))
			{
				put_u32(&mut head, u32::from(name.get()));
				put_u32(&mut head, 0x8000_0000 | next);
				next = next.strict_add(table_size(languages));
			}
		}
		let data_entries = next;
		for (&(resource_type, name), &languages) in &self.languages {
			write_table_header(&mut head, languages);
			let first = ResourceKey::new(resource_type, name, LanguageId::from_raw(0));
			let last = ResourceKey::new(resource_type, name, LanguageId::from_raw(u16::MAX));
			for key in self.entries.range(first..=last).map(|(key, _)| key) {
				put_u32(&mut head, u32::from(key.language.raw()));
				put_u32(&mut head, next);
				next = next.strict_add(16);
			}
		}
		for entry in self.entries.values() {
			put_u32(&mut head, 0); // OffsetToData, set by the relocation
			put_u32(&mut head, entry.len);
			put_u32(&mut head, 0); // CodePage
			put_u32(&mut head, 0); // Reserved
		}

		if overflow {
			put_u32(&mut head, layout.relocations);
			put_u32(&mut head, 0);
			put_u16(&mut head, 0);
		}
		let mut offset = data_entries;
		for symbol in (5..).take(usize_from_u32(count)) {
			put_u32(&mut head, offset);
			put_u32(&mut head, symbol);
			put_u16(&mut head, machine.relocation_type());
			offset = offset.strict_add(16);
		}
		pad(&mut head, 8);
		out.put(&head)?;
		drop(head);

		for entry in self.entries.values() {
			out.put(&entry.data)?;
			out.put(padding(entry.len, 8))?;
		}

		let mut tail = Vec::with_capacity(usize_from_u32(count.strict_add(5).strict_mul(18).strict_add(4)));
		write_symbol(&mut tail, *b"@feat.00", 0x11, 0xFFFF, 0);
		write_symbol(&mut tail, *b".rsrc$01", 0, 1, 1);
		write_section_aux(&mut tail, layout.directory, short_relocations);
		write_symbol(&mut tail, *b".rsrc$02", 0, 2, 1);
		write_section_aux(&mut tail, layout.data_size, 0);
		let mut data_offset = 0_u32;
		for (index, entry) in self.entries.values().enumerate() {
			write_symbol(&mut tail, data_symbol_name(index), data_offset, 2, 0);
			data_offset = data_offset.strict_add(aligned(entry.len, 8));
		}
		put_u32(&mut tail, 0); // Empty string table; cvtres writes 0, although the size counts itself
		out.put(&tail)
	}
}

/// Shows sizes and the length of every resource, not the data, which reaches megabytes with icons.
impl fmt::Debug for ResourceFile {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		struct Lengths<'a>(&'a BTreeMap<ResourceKey, Entry>);
		impl fmt::Debug for Lengths<'_> {
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.debug_map().entries(self.0.iter().map(|(key, entry)| (key, entry.len))).finish()
			}
		}

		f.debug_struct("ResourceFile")
			.field("res_size", &self.totals.res_size)
			.field("coff_size", &self.coff.size)
			.field("resources", &Lengths(&self.entries))
			.finish_non_exhaustive()
	}
}

impl Default for ResourceFile {
	#[inline]
	fn default() -> Self {
		Self::new()
	}
}

/// Why a resource could not be inserted; the file stays unchanged.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum InsertError {
	/// A resource of the same type, name and language exists.
	#[error("Duplicate resource (type {resource_type}, name {name}, language 0x{language:04X})")]
	DuplicateResource { resource_type: u16, name: u16, language: u16 },

	/// A name would exist in more than 65535 languages, which the COFF directory cannot count.
	#[error("Resource (type {resource_type}, name {name}) would exist in more than 65535 languages")]
	TooManyLanguages { resource_type: u16, name: u16 },

	/// The `.res` file or the COFF object would exceed 4 GiB.
	#[error("Resource file would exceed 4 GiB")]
	FileTooLarge,

	/// The images of an icon or cursor need more names than remain below 65536.
	#[error("No ordinals of resource type {resource_type} left for {count} more images")]
	OutOfImageIds { resource_type: u16, count: usize },

	/// The icon or cursor an alias refers to does not exist in its language.
	#[error("Group {name} of resource type {resource_type} does not exist in language 0x{language:04X}")]
	MissingGroup { resource_type: u16, name: u16, language: u16 },
}

/// Why [`CoffMachine::cargo_target`] found no machine.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CargoTargetError {
	/// `CARGO_CFG_TARGET_ARCH` is not set, as outside of build scripts.
	#[error("CARGO_CFG_TARGET_ARCH is not set; Cargo sets it for build scripts only")]
	NotSet,

	/// `CARGO_CFG_TARGET_ARCH` is not valid Unicode.
	#[error("CARGO_CFG_TARGET_ARCH is not valid Unicode")]
	NotUnicode,

	/// The target architecture has no COFF resource machine.
	#[error("Target architecture '{arch}' has no COFF resource machine; supported are x86, x86_64, arm, aarch64 and arm64ec")]
	UnsupportedArch { arch: String },
}

#[cfg(feature = "images")]
pub(crate) const RT_CURSOR: NonZeroU16 = ordinal(1);
#[cfg(feature = "images")]
pub(crate) const RT_ICON: NonZeroU16 = ordinal(3);
#[cfg(feature = "images")]
pub(crate) const RT_GROUP_CURSOR: NonZeroU16 = ordinal(12);
#[cfg(feature = "images")]
pub(crate) const RT_GROUP_ICON: NonZeroU16 = ordinal(14);
pub(crate) const RT_VERSION: NonZeroU16 = ordinal(16);
pub(crate) const RT_MANIFEST: NonZeroU16 = ordinal(24);

pub(crate) const MOVEABLE: u16 = 0x0010;
pub(crate) const PURE: u16 = 0x0020;
#[cfg(feature = "images")]
pub(crate) const DISCARDABLE: u16 = 0x1000;

/// The key of a resource; the field order is the order of the output.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ResourceKey {
	pub(crate) resource_type: NonZeroU16,
	pub(crate) name: NonZeroU16,
	pub(crate) language: LanguageId,
}

impl ResourceKey {
	#[must_use]
	pub(crate) const fn new(resource_type: NonZeroU16, name: NonZeroU16, language: LanguageId) -> Self {
		Self {
			resource_type,
			name,
			language,
		}
	}

	#[must_use]
	const fn name_key(self) -> (NonZeroU16, NonZeroU16) {
		(self.resource_type, self.name)
	}
}

pub(crate) struct NewEntry {
	key: ResourceKey,
	flags: u16,
	data: Arc<[u8]>,
}

impl NewEntry {
	#[must_use]
	pub(crate) const fn new(key: ResourceKey, flags: u16, data: Arc<[u8]>) -> Self {
		Self { key, flags, data }
	}
}

#[derive(Eq, PartialEq)]
struct Entry {
	flags: u16,
	len: u32,
	data: Arc<[u8]>,
}

/// The counts and sizes the layout of both output forms follows from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Totals {
	types: u32,
	names: u32,
	count: u32,
	res_size: u32,
	coff_data: u32,
}

impl Totals {
	const EMPTY: Self = Self {
		types: 0,
		names: 0,
		count: 0,
		res_size: 32,
		coff_data: 0,
	};

	#[must_use]
	fn add_entry(&mut self, len: u32) -> Option<()> {
		self.count = self.count.checked_add(1)?;
		self.res_size = self.res_size.checked_add(32)?.checked_add(len.checked_next_multiple_of(4)?)?;
		self.coff_data = self.coff_data.checked_add(len.checked_next_multiple_of(8)?)?;
		Some(())
	}
}

/// The offsets and sizes of the COFF object, computed whenever the resources change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CoffLayout {
	/// The size of the resource directory with its data entries, the section .rsrc$01.
	directory: u32,
	/// The number of relocation records, with the overflow record from 65535 resources on.
	relocations: u32,
	data_start: u32,
	data_size: u32,
	symbols_start: u32,
	size: u32,
}

macro_rules! some {
	($value:expr) => {
		match $value {
			Some(value) => value,
			None => return None,
		}
	};
}

impl CoffLayout {
	#[must_use]
	const fn of(totals: &Totals) -> Option<Self> {
		let tables = some!(totals.types.checked_add(totals.names));
		let headers = some!(some!(tables.checked_add(1)).checked_mul(16));
		let directory_entries = some!(some!(tables.checked_add(totals.count)).checked_mul(8));
		let data_entries = some!(totals.count.checked_mul(16));
		let directory = some!(some!(headers.checked_add(directory_entries)).checked_add(data_entries));
		let relocations = if totals.count >= 0xFFFF {
			some!(totals.count.checked_add(1))
		} else {
			totals.count
		};
		let relocations_end = some!(some!(directory.checked_add(100)).checked_add(some!(relocations.checked_mul(10))));
		let data_start = some!(relocations_end.checked_next_multiple_of(8));
		let symbols_start = some!(data_start.checked_add(totals.coff_data));
		let symbols = some!(some!(totals.count.checked_add(5)).checked_mul(18));
		let size = some!(some!(symbols_start.checked_add(symbols)).checked_add(4));
		Some(Self {
			directory,
			relocations,
			data_start,
			data_size: totals.coff_data,
			symbols_start,
			size,
		})
	}
}

const EMPTY_LAYOUT: CoffLayout = match CoffLayout::of(&Totals::EMPTY) {
	Some(layout) => layout,
	None => panic!("The empty layout fits"),
};

/// The output of the writers: a vector that cannot fail, or a buffered writer.
trait Sink {
	type Error;

	fn put(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

impl Sink for Vec<u8> {
	type Error = Infallible;

	#[inline]
	fn put(&mut self, bytes: &[u8]) -> Result<(), Infallible> {
		self.extend_from_slice(bytes);
		Ok(())
	}
}

impl<W> Sink for BufWriter<W>
where
	W: Write,
{
	type Error = io::Error;

	#[inline]
	fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
		self.write_all(bytes)
	}
}

/// Returns `len` rounded up to `alignment`; the layout checked that every aligned length fits.
#[must_use]
fn aligned(len: u32, alignment: u32) -> u32 {
	len.checked_next_multiple_of(alignment).unwrap_or(u32::MAX)
}

/// Returns the zeros that pad `len` bytes to `alignment`, which is at most 8.
#[must_use]
fn padding(len: u32, alignment: u32) -> &'static [u8] {
	const ZEROS: [u8; 8] = [0; 8];
	let fill = aligned(len, alignment).saturating_sub(len);
	ZEROS.get(..usize_from_u32(fill)).unwrap_or_default()
}

#[must_use]
fn table_size(entries: u16) -> u32 {
	// At most 65535 entries of 8 bytes after a header of 16 bytes
	u32::from(entries).strict_mul(8).strict_add(16)
}

/// Writes a `RESOURCEHEADER` with ordinal type and name; `key` holds type, name and language.
fn write_res_header(out: &mut Vec<u8>, data_size: u32, key: [u16; 3], flags: u16) {
	let [resource_type, name, language] = key;
	put_u32(out, data_size);
	put_u32(out, 32); // HeaderSize
	put_u16(out, 0xFFFF);
	put_u16(out, resource_type);
	put_u16(out, 0xFFFF);
	put_u16(out, name);
	put_u32(out, 0); // DataVersion
	put_u16(out, flags);
	put_u16(out, language);
	put_u32(out, 0); // Version
	put_u32(out, 0); // Characteristics
}

fn write_section_header(out: &mut Vec<u8>, name: [u8; 8], size: u32, data: u32, relocations: u32, relocation_count: u16, flags: u32) {
	out.extend_from_slice(&name);
	put_u32(out, 0); // VirtualSize
	put_u32(out, 0); // VirtualAddress
	put_u32(out, size);
	put_u32(out, data);
	put_u32(out, relocations);
	put_u32(out, 0); // PointerToLinenumbers
	put_u16(out, relocation_count);
	put_u16(out, 0); // NumberOfLinenumbers
	put_u32(out, flags);
}

fn write_table_header(out: &mut Vec<u8>, id_entries: u16) {
	put_u32(out, 0); // Characteristics
	put_u32(out, 0); // TimeDateStamp
	put_u32(out, 0); // MajorVersion and MinorVersion
	put_u16(out, 0); // NumberOfNamedEntries
	put_u16(out, id_entries);
}

fn write_symbol(out: &mut Vec<u8>, name: [u8; 8], value: u32, section: u16, aux_count: u8) {
	out.extend_from_slice(&name);
	put_u32(out, value);
	put_u16(out, section);
	put_u16(out, 0); // Type
	out.extend_from_slice(&[3, aux_count]); // IMAGE_SYM_CLASS_STATIC
}

fn write_section_aux(out: &mut Vec<u8>, size: u32, relocation_count: u16) {
	put_u32(out, size);
	put_u16(out, relocation_count);
	out.extend_from_slice(&[0; 12]);
}

/// Returns `$R` and the low 24 bits of `index` as six uppercase hexadecimal digits.
#[must_use]
fn data_symbol_name(index: usize) -> [u8; 8] {
	const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
	let mut name = *b"$R000000";
	let (_, digits) = name.split_at_mut(2);
	for (digit, shift) in digits.iter_mut().zip([20_u32, 16, 12, 8, 4, 0]) {
		*digit = DIGITS.get(index.strict_shr(shift) & 0xF).copied().unwrap_or(b'0');
	}
	name
}

#[cfg(test)]
#[allow(
	clippy::arithmetic_side_effects,
	clippy::as_conversions,
	reason = "Tests run with overflow checks, so every operator is strict there"
)]
mod tests {
	use super::*;

	#[must_use]
	fn word(bytes: &[u8], offset: usize) -> u16 {
		u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
	}

	#[must_use]
	fn dword(bytes: &[u8], offset: usize) -> u32 {
		u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
	}

	#[must_use]
	fn key(resource_type: u16, name: u16, language: u16) -> ResourceKey {
		ResourceKey::new(
			NonZeroU16::new(resource_type).unwrap(),
			NonZeroU16::new(name).unwrap(),
			LanguageId::from_raw(language),
		)
	}

	fn insert(file: &mut ResourceFile, resource_type: u16, name: u16, language: u16, data: &[u8]) -> Result<(), InsertError> {
		file.insert_entries(vec![NewEntry::new(key(resource_type, name, language), MOVEABLE, Arc::from(data))])
	}

	#[must_use]
	fn resources_of(object: &[u8]) -> Vec<(u16, u16, u16, Vec<u8>)> {
		let section = |index: usize| {
			let header = 20 + 40 * index;
			(
				usize_from_u32(dword(object, header + 20)),
				usize_from_u32(dword(object, header + 24)),
				word(object, header + 32),
				dword(object, header + 36),
			)
		};
		let (directory, relocations_at, relocation_field, flags) = section(0);
		let (data, ..) = section(1);
		let symbols = usize_from_u32(dword(object, 8));

		let (first, count) = if flags & 0x0100_0000 == 0 {
			(0, usize::from(relocation_field))
		} else {
			assert_eq!(relocation_field, 0xFFFF);
			(1, usize_from_u32(dword(object, relocations_at)) - 1)
		};
		let symbol_of_entry: BTreeMap<u32, u32> = (first..first + count)
			.map(|index| {
				let record = relocations_at + 10 * index;
				(dword(object, record), dword(object, record + 4))
			})
			.collect();
		let table = |offset: usize| -> Vec<(u16, u32)> {
			assert_eq!(word(object, directory + offset + 12), 0, "No named entries");
			(0..usize::from(word(object, directory + offset + 14)))
				.map(|index| {
					let entry = directory + offset + 16 + 8 * index;
					(u16::try_from(dword(object, entry)).unwrap(), dword(object, entry + 4))
				})
				.collect()
		};

		let mut resources = Vec::new();
		for (resource_type, types) in table(0) {
			for (name, names) in table(usize_from_u32(types & 0x7FFF_FFFF)) {
				for (language, entry) in table(usize_from_u32(names & 0x7FFF_FFFF)) {
					let at = directory + usize_from_u32(entry);
					assert_eq!(dword(object, at), 0, "OffsetToData is left to the relocation");
					let size = usize_from_u32(dword(object, at + 4));
					let symbol = symbols + 18 * usize_from_u32(symbol_of_entry[&entry]);
					assert_eq!(word(object, symbol + 12), 2, "Data symbols live in .rsrc$02");
					let start = data + usize_from_u32(dword(object, symbol + 8));
					resources.push((resource_type, name, language, object[start..start + size].to_vec()));
				}
			}
		}
		resources
	}

	#[must_use]
	fn sample() -> ResourceFile {
		let mut file = ResourceFile::new();
		insert(&mut file, 10, 2, 0, b"two").unwrap();
		insert(&mut file, 10, 1, 0, b"one!").unwrap();
		insert(&mut file, 10, 1, 0x0407, b"eins").unwrap();
		insert(&mut file, RT_MANIFEST.get(), 1, 0, b"<a/>").unwrap();
		insert(&mut file, 300, 5, 0, b"custom data").unwrap();
		file
	}

	#[test]
	fn languages_round_trip() {
		let german = LanguageId::from_parts(0x07, 0x01).unwrap();
		assert_eq!((german.raw(), german.primary(), german.sub()), (0x0407, 0x07, 0x01));
		assert_eq!(LanguageId::from_parts(0x3FF, 0x3F).map(LanguageId::raw), Some(0xFFFF));
		assert_eq!(LanguageId::from_parts(0x400, 0), None);
		assert_eq!(LanguageId::from_parts(0, 0x40), None);
	}

	#[test]
	fn common_languages_have_their_windows_ids() {
		// Checked against the LCID table of Windows: raw value, primary language and sublanguage
		let languages = [
			(LanguageId::ENGLISH_US, 0x0409, 0x09, 0x01),
			(LanguageId::ENGLISH_UK, 0x0809, 0x09, 0x02),
			(LanguageId::GERMAN, 0x0407, 0x07, 0x01),
			(LanguageId::FRENCH, 0x040C, 0x0C, 0x01),
			(LanguageId::SPANISH, 0x0C0A, 0x0A, 0x03),
			(LanguageId::ITALIAN, 0x0410, 0x10, 0x01),
			(LanguageId::PORTUGUESE_BRAZIL, 0x0416, 0x16, 0x01),
			(LanguageId::PORTUGUESE_PORTUGAL, 0x0816, 0x16, 0x02),
			(LanguageId::DUTCH, 0x0413, 0x13, 0x01),
			(LanguageId::POLISH, 0x0415, 0x15, 0x01),
			(LanguageId::CZECH, 0x0405, 0x05, 0x01),
			(LanguageId::SWEDISH, 0x041D, 0x1D, 0x01),
			(LanguageId::TURKISH, 0x041F, 0x1F, 0x01),
			(LanguageId::RUSSIAN, 0x0419, 0x19, 0x01),
			(LanguageId::UKRAINIAN, 0x0422, 0x22, 0x01),
			(LanguageId::ARABIC, 0x0401, 0x01, 0x01),
			(LanguageId::HINDI, 0x0439, 0x39, 0x01),
			(LanguageId::JAPANESE, 0x0411, 0x11, 0x01),
			(LanguageId::KOREAN, 0x0412, 0x12, 0x01),
			(LanguageId::CHINESE_SIMPLIFIED, 0x0804, 0x04, 0x02),
			(LanguageId::CHINESE_TRADITIONAL, 0x0404, 0x04, 0x01),
		];
		for (language, raw, primary, sub) in languages {
			assert_eq!(
				(language.raw(), language.primary(), language.sub()),
				(raw, primary, sub),
				"{language:?}"
			);
			assert_eq!(LanguageId::from_parts(primary, sub), Some(language));
		}
		let mut raws: Vec<u16> = languages.iter().map(|&(language, ..)| language.raw()).collect();
		raws.sort_unstable();
		raws.dedup();
		assert_eq!(raws.len(), languages.len(), "Every constant is distinct");
	}

	#[test]
	fn target_arch_selects_the_machine() {
		let machine = |arch: &str| CoffMachine::from_target_arch(Some(arch.into()));
		assert_eq!(machine("x86"), Ok(CoffMachine::X86));
		assert_eq!(machine("x86_64"), Ok(CoffMachine::X64));
		assert_eq!(machine("arm"), Ok(CoffMachine::Arm));
		assert_eq!(machine("aarch64"), Ok(CoffMachine::Arm64));
		assert_eq!(machine("arm64ec"), Ok(CoffMachine::Arm64Ec));
		assert_eq!(
			machine("riscv64"),
			Err(CargoTargetError::UnsupportedArch {
				arch: "riscv64".to_owned()
			})
		);
		assert_eq!(CoffMachine::from_target_arch(None), Err(CargoTargetError::NotSet));
	}

	#[test]
	fn the_res_form_has_the_exact_layout() {
		let empty = ResourceFile::new().to_res();
		assert_eq!(empty.len(), 32);
		assert_eq!(empty[..16], [0, 0, 0, 0, 0x20, 0, 0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0]);
		let mut file = ResourceFile::new();
		insert(&mut file, 10, 7, 0x0409, b"abcde").unwrap();
		let res = file.to_res();
		assert_eq!(res.len(), 32 + 32 + 8);
		#[rustfmt::skip]
		let header = [
			5, 0, 0, 0, 0x20, 0, 0, 0, 0xFF, 0xFF, 10, 0, 0xFF, 0xFF, 7, 0,
			0, 0, 0, 0, 0x10, 0, 0x09, 0x04, 0, 0, 0, 0, 0, 0, 0, 0,
		];
		assert_eq!(res[32..64], header);
		assert_eq!(res[64..], *b"abcde\0\0\0");
		assert_eq!(res.capacity(), res.len(), "The size is known in advance");
	}

	#[test]
	fn the_writers_stream_the_same_bytes() {
		let file = sample();
		let (mut res, mut coff) = (Vec::new(), Vec::new());
		file.write_res(&mut res).unwrap();
		file.write_coff(&mut coff, CoffMachine::Arm64).unwrap();
		assert_eq!((res, coff), (file.to_res(), file.to_coff(CoffMachine::Arm64)));
	}

	#[test]
	fn duplicates_are_rejected_and_leave_the_file_unchanged() {
		let mut file = sample();
		assert_eq!(
			insert(&mut file, 10, 1, 0x0407, b"x"),
			Err(InsertError::DuplicateResource {
				resource_type: 10,
				name: 1,
				language: 0x0407
			})
		);
		let twice = vec![
			NewEntry::new(key(10, 9, 0), 0, Arc::from([1])),
			NewEntry::new(key(10, 9, 0), 0, Arc::from([2])),
		];
		assert!(matches!(file.insert_entries(twice), Err(InsertError::DuplicateResource { .. })));
		assert_eq!(file, sample(), "The sample is deterministic, so it shows the state before");
	}

	#[test]
	fn output_does_not_depend_on_insertion_order() {
		let mut reversed = ResourceFile::new();
		insert(&mut reversed, 300, 5, 0, b"custom data").unwrap();
		insert(&mut reversed, RT_MANIFEST.get(), 1, 0, b"<a/>").unwrap();
		insert(&mut reversed, 10, 1, 0x0407, b"eins").unwrap();
		insert(&mut reversed, 10, 1, 0, b"one!").unwrap();
		insert(&mut reversed, 10, 2, 0, b"two").unwrap();
		assert_eq!(reversed.to_res(), sample().to_res());
		assert_eq!(reversed.to_coff(CoffMachine::X64), sample().to_coff(CoffMachine::X64));
		assert_eq!(reversed, sample());
	}

	#[test]
	fn the_coff_object_holds_every_resource_with_its_predicted_size() {
		let file = sample();
		let object = file.to_coff(CoffMachine::X64);
		assert_eq!(object.len(), usize_from_u32(file.coff.size));
		assert_eq!(object.capacity(), object.len(), "The size is known in advance");
		assert_eq!((file.totals.types, file.totals.names, file.totals.count), (3, 4, 5));
		assert_eq!(
			resources_of(&object),
			[
				(10, 1, 0, b"one!".to_vec()),
				(10, 1, 0x0407, b"eins".to_vec()),
				(10, 2, 0, b"two".to_vec()),
				(RT_MANIFEST.get(), 1, 0, b"<a/>".to_vec()),
				(300, 5, 0, b"custom data".to_vec()),
			]
		);
		let empty = ResourceFile::new().to_coff(CoffMachine::X64);
		assert_eq!(empty.len(), 214);
		assert_eq!(resources_of(&empty), []);
	}

	#[test]
	fn data_symbols_are_named_like_llvm_cvtres_names_them() {
		for index in [0, 9, 10, 0xFFFF, 0x12_3456, 0xFF_FFFF, 0x100_0001] {
			assert_eq!(
				data_symbol_name(index).as_slice(),
				format!("$R{:06X}", index & 0xFF_FFFF).as_bytes()
			);
		}
	}

	#[test]
	fn machines_change_only_the_machine_field_and_the_relocation_type() {
		let x64 = sample().to_coff(CoffMachine::X64);
		for (machine, id, relocation) in [
			(CoffMachine::X86, 0x014C, 7),
			(CoffMachine::Arm, 0x01C4, 2),
			(CoffMachine::Arm64, 0xAA64, 2),
			(CoffMachine::Arm64Ec, 0xA641, 2),
		] {
			let object = sample().to_coff(machine);
			assert_eq!(word(&object, 0), id);
			let differences: Vec<usize> = (0..object.len()).filter(|&index| object[index] != x64[index]).collect();
			assert!(
				differences.iter().all(|&index| index < 2 || object[index] == relocation),
				"{machine:?}"
			);
		}
	}

	#[test]
	fn size_limits_leave_the_file_unchanged() {
		// The sample is deterministic, so a second one shows the state before the insertion
		let with_totals = |edit: fn(&mut Totals)| {
			let mut file = sample();
			edit(&mut file.totals);
			file
		};
		let fill_res: fn(&mut Totals) = |totals| totals.res_size = u32::MAX - 32;
		let mut res_full = with_totals(fill_res);
		assert_eq!(insert(&mut res_full, 10, 3, 0, &[0; 4]), Err(InsertError::FileTooLarge));
		assert_eq!(res_full, with_totals(fill_res));

		let fill_coff: fn(&mut Totals) = |totals| totals.coff_data = u32::MAX - 512;
		let mut coff_full = with_totals(fill_coff);
		assert_eq!(insert(&mut coff_full, 10, 3, 0, &[0; 4]), Err(InsertError::FileTooLarge));
		assert_eq!(coff_full, with_totals(fill_coff));
	}

	#[test]
	fn languages_per_name_are_limited_and_relocations_overflow() {
		// 65535 languages fill the table of one name; with a second name 65536 relocations need the overflow record
		let mut file = ResourceFile::new();
		let entries = (0..=0xFFFE)
			.map(|language| NewEntry::new(key(10, 1, language), 0, Arc::from([0xAB])))
			.collect();
		file.insert_entries(entries).unwrap();
		let before = (file.to_res(), file.totals, file.coff);
		assert_eq!(
			insert(&mut file, 10, 1, 0xFFFF, &[0xAB]),
			Err(InsertError::TooManyLanguages {
				resource_type: 10,
				name: 1
			})
		);
		assert_eq!((file.to_res(), file.totals, file.coff), before);
		insert(&mut file, 10, 2, 0xFFFF, &[0xCD]).unwrap();

		let object = file.to_coff(CoffMachine::Arm64);
		assert_eq!(object.len(), usize_from_u32(file.coff.size));
		assert_eq!(word(&object, 20 + 32), 0xFFFF);
		assert_eq!(
			dword(&object, usize_from_u32(dword(&object, 20 + 24))),
			65537,
			"The overflow record counts itself"
		);
		let resources = resources_of(&object);
		assert_eq!(resources.len(), 65536);
		assert_eq!(resources.last(), Some(&(10, 2, 0xFFFF, vec![0xCD])));
	}

	#[test]
	fn the_highest_name_is_tracked_per_type() {
		let file = sample();
		let highest = |resource_type: u16| file.max_name(NonZeroU16::new(resource_type).unwrap()).map(NonZeroU16::get);
		assert_eq!((highest(10), highest(300), highest(7)), (Some(2), Some(5), None));
	}
}
