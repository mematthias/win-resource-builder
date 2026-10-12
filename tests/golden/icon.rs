use crate::fixture_path;
use std::num::NonZeroU16;
use win_resource_builder::image::Icon;
use win_resource_builder::resource::{LanguageId, ResourceFile};

const KIND: &str = "icon";

#[must_use]
pub fn id(ordinal: u16) -> NonZeroU16 {
	NonZeroU16::new(ordinal).expect("Fixture ordinals are non-zero")
}

#[must_use]
pub fn icon(fixture: &str) -> Icon {
	Icon::options()
		.create_from_file(fixture_path(KIND, fixture))
		.expect("Fixture icon is valid")
}

#[must_use]
fn build(icons: &[(u16, &str)]) -> ResourceFile {
	let mut file = ResourceFile::new();
	for &(ordinal, fixture) in icons {
		file.insert_icon(id(ordinal), LanguageId::NEUTRAL, &icon(fixture))
			.expect("Fixture icon fits");
	}
	file
}

#[must_use]
fn single() -> ResourceFile {
	build(&[(1, "app.ico")])
}

#[must_use]
fn two_icons() -> ResourceFile {
	build(&[(1, "app.ico"), (2, "second.ico")])
}

golden_cases!("icon"; single, two_icons);

#[test]
fn canonical_fixtures_decode_to_themselves() {
	for fixture in ["app.ico", "second.ico"] {
		assert_eq!(
			icon(fixture).to_ico(),
			crate::read_reference(KIND, fixture, "Icon::to_ico from the files in source/"),
			"{fixture}"
		);
	}
}
