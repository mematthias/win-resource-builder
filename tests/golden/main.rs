#![allow(
	clippy::arithmetic_side_effects,
	clippy::expect_used,
	clippy::indexing_slicing,
	clippy::panic,
	reason = "A test fails loudly"
)]

#[cfg(feature = "images")]
macro_rules! cvtres_cases {
	($kind:literal; $( $case:ident ),* $(,)?) => {
		$(
			mod $case {
				#[test]
				fn coff() {
					crate::assert_matches_cvtres($kind, stringify!($case), &super::$case());
				}
			}
		)*
	};
}

macro_rules! golden_cases {
	($kind:literal; $( $case:ident ),* $(,)?) => {
		$(
			mod $case {
				#[test]
				fn res() {
					crate::assert_matches_rc($kind, stringify!($case), &super::$case());
				}

				#[test]
				fn coff() {
					crate::assert_matches_cvtres($kind, stringify!($case), &super::$case());
				}
			}
		)*
	};
}

#[cfg(feature = "images")]
mod cursor;
#[cfg(feature = "images")]
mod icon;
mod manifest;
mod res_file;
mod version_info;

use std::path::PathBuf;
use std::{env, fs};
use win_resource_builder::resource::{CoffMachine, ResourceFile};

#[must_use]
fn fixture_path(kind: &str, file_name: &str) -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR"))
		.join("tests")
		.join("fixtures")
		.join(kind)
		.join(file_name)
}

#[must_use]
fn read_text_fixture(kind: &str, file_name: &str) -> String {
	let path = fixture_path(kind, file_name);
	fs::read_to_string(&path).unwrap_or_else(|err| panic!("Fixture '{}' is not readable UTF-8: {err}", path.display()))
}

#[must_use]
fn read_reference(kind: &str, file_name: &str, generator: &str) -> Vec<u8> {
	let path = fixture_path(kind, file_name);
	fs::read(&path).unwrap_or_else(|err| panic!("Missing reference '{}' ({err}); generate it with '{generator}'", path.display()))
}

fn dump(kind: &str, case: &str, file: &ResourceFile) {
	if let Some(dir) = env::var_os("WIN_RESOURCE_BUILDER_GOLDEN_DUMP") {
		let dir = PathBuf::from(dir).join(kind);
		fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("Cannot create '{}': {err}", dir.display()));
		let path = dir.join(format!("{case}.res"));
		fs::write(&path, file.to_res()).unwrap_or_else(|err| panic!("Cannot write '{}': {err}", path.display()));
	}
}

fn assert_matches_rc(kind: &str, case: &str, file: &ResourceFile) {
	dump(kind, case, file);
	let reference = read_reference(kind, &format!("{case}.res"), "python tools/generate_golden.py");

	let sorted = |bytes: &[u8]| {
		let mut entries = res_file::read_entries(bytes);
		entries.sort_by_key(|entry| (entry.resource_type, entry.name, entry.language));
		entries
	};
	let (ours, reference) = (sorted(&file.to_res()), sorted(&reference));
	let labels = |entries: &[res_file::Entry]| entries.iter().map(res_file::Entry::label).collect::<Vec<_>>();
	assert_eq!(labels(&ours), labels(&reference), "{kind}/{case}: different entries");

	for (our_entry, reference_entry) in ours.iter().zip(&reference) {
		let label = our_entry.label();
		let header = |entry: &res_file::Entry| (entry.memory_flags, entry.data_version, entry.version, entry.characteristics);
		assert_eq!(
			header(our_entry),
			header(reference_entry),
			"{kind}/{case}, {label}: memory flags, data version, version or characteristics differ"
		);
		assert_bytes_match(&format!("{kind}/{case}, {label}"), "rc.exe", &our_entry.data, &reference_entry.data);
	}
}

fn assert_matches_cvtres(kind: &str, case: &str, file: &ResourceFile) {
	dump(kind, case, file);
	let reference = read_reference(kind, &format!("{case}.x64.obj"), "python tools/generate_coff_golden.py");
	assert_bytes_match(
		&format!("{kind}/{case}.x64.obj"),
		"llvm-cvtres",
		&file.to_coff(CoffMachine::X64),
		&reference,
	);
}

fn assert_bytes_match(label: &str, tool: &str, ours: &[u8], reference: &[u8]) {
	if let Some((offset, (our_byte, reference_byte))) = ours.iter().zip(reference).enumerate().find(|(_, (a, b))| a != b) {
		panic!("{label}: data differs at offset 0x{offset:X}: ours 0x{our_byte:02X}, {tool} 0x{reference_byte:02X}");
	}
	assert_eq!(ours.len(), reference.len(), "{label}: identical prefix, but different sizes");
}
