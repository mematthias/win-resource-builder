//! The DLL of the check: it only carries the resources the build script links into it.

#[unsafe(no_mangle)]
pub extern "C" fn win_resource_builder_payload() -> u32 {
	1
}
