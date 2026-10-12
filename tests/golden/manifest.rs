use crate::read_text_fixture;
use win_resource_builder::manifest::{Manifest, ManifestKind};
use win_resource_builder::resource::ResourceFile;

const KIND: &str = "manifest";

#[must_use]
fn build(kinds: &[ManifestKind]) -> ResourceFile {
	let xml = read_text_fixture(KIND, "manifest.xml");
	let mut file = ResourceFile::new();
	for &kind in kinds {
		let manifest = Manifest::options().kind(kind).create(&xml).expect("Reference manifest is valid");
		file.insert_manifest(&manifest).expect("Reference manifest fits");
	}
	file
}

#[must_use]
fn process() -> ResourceFile {
	build(&[ManifestKind::Process])
}

#[must_use]
fn isolation_aware() -> ResourceFile {
	build(&[ManifestKind::IsolationAware])
}

#[must_use]
fn both() -> ResourceFile {
	// Inserted in reverse to show that the order of the file does not depend on the caller
	build(&[ManifestKind::IsolationAware, ManifestKind::Process])
}

golden_cases!("manifest"; process, isolation_aware, both);
