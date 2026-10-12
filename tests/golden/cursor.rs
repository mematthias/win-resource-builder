use crate::icon::{icon, id};
use crate::{fixture_path, res_file};
use win_resource_builder::image::Cursor;
use win_resource_builder::resource::{LanguageId, ResourceFile};

const KIND: &str = "cursor";

const CURSOR: u16 = 1;
const ICON: u16 = 3;
const GROUP_CURSOR: u16 = 12;
const GROUP_ICON: u16 = 14;

#[must_use]
fn cursor(fixture: &str) -> Cursor {
	Cursor::options()
		.create_from_file(fixture_path(KIND, fixture))
		.expect("Fixture cursor is valid")
}

#[must_use]
fn single() -> ResourceFile {
	let mut file = ResourceFile::new();
	file.insert_cursor(id(1), LanguageId::NEUTRAL, cursor("arrow.cur"))
		.expect("Fixture cursor fits");
	file
}

#[must_use]
fn mixed() -> ResourceFile {
	// Icons and cursors are namespaces of their own: both groups and both image sets start at 1
	let mut file = ResourceFile::new();
	file.insert_icon(id(1), LanguageId::NEUTRAL, icon("app.ico"))
		.expect("Fixture icon fits");
	file.insert_cursor(id(1), LanguageId::NEUTRAL, cursor("arrow.cur"))
		.expect("Fixture cursor fits");
	file
}

cvtres_cases!("cursor"; single, mixed);

#[test]
fn canonical_fixture_decodes_to_itself() {
	let reference = crate::read_reference(KIND, "arrow.cur", "Cursor::to_cur from the files in source/");
	assert_eq!(cursor("arrow.cur").to_cur(), reference);
}

#[test]
fn mixed_numbers_icon_and_cursor_images_independently() {
	let entries = res_file::read_entries(&mixed().to_res());
	let names = |resource_type| {
		let mut names: Vec<u16> = entries
			.iter()
			.filter(|entry| entry.resource_type == resource_type)
			.map(|entry| entry.name)
			.collect();
		names.sort_unstable();
		names
	};
	assert_eq!(names(ICON), [1, 2, 3]);
	assert_eq!(names(CURSOR), [1, 2]);
	assert_eq!(names(GROUP_ICON), [1]);
	assert_eq!(names(GROUP_CURSOR), [1]);
}
