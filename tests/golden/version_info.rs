use win_resource_builder::resource::{LanguageId, ResourceFile};
use win_resource_builder::version_info::{FileType, Version, VersionFlags, VersionInfo};

#[must_use]
fn build(info: VersionInfo) -> ResourceFile {
	let mut file = ResourceFile::new();
	file.insert_version_info(info).expect("Reference version info fits");
	file
}

#[must_use]
fn application() -> ResourceFile {
	let info = VersionInfo::options()
		.file_type(FileType::Application)
		.company_name("Example Company")
		.file_description("Example Application")
		.internal_name("example")
		.legal_copyright("Copyright (C) 2026 Example Company")
		.product_version(Version::new(5, 6, 7, 8))
		.create("Example Product", "example.exe", Version::new(1, 2, 3, 4))
		.expect("Reference version info is valid");
	build(info)
}

#[must_use]
fn library() -> ResourceFile {
	let info = VersionInfo::options()
		.file_type(FileType::DynamicLibrary)
		.flags(VersionFlags::DEBUG | VersionFlags::PRERELEASE | VersionFlags::PATCHED)
		.language(LanguageId::from_parts(0x07, 0x01).expect("German, Germany"))
		.comments("Built for the golden tests")
		.company_name("Example Company")
		.file_description("Example Library")
		.internal_name("example_lib")
		.legal_copyright("Copyright (C) 2026 Example Company")
		.legal_trademarks("Example is a trademark")
		.private_build("Built by CI runner 7")
		.special_build("Instrumented")
		.product_version(Version::new(10, 0, 0, 0))
		.create("Example Product", "example_lib.dll", Version::new(10, 0, 65535, 1))
		.expect("Reference version info is valid");
	build(info)
}

#[must_use]
fn minimal() -> ResourceFile {
	// Only the required entries: product name, original file name and the derived version strings
	build(
		VersionInfo::options()
			.create("Example Product", "example.exe", Version::new(1, 0, 0, 0))
			.expect("Reference version info is valid"),
	)
}

golden_cases!("version_info"; application, library, minimal);
