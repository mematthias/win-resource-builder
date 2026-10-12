//! The DLL of the check: it only carries the resources the build script links into it.

/// Gives the DLL an export, so that the linker writes it with its resources.
#[allow(unsafe_code, reason = "An exported symbol needs an unmangled name")]
#[unsafe(no_mangle)]
pub extern "C" fn win_resource_builder_payload() -> u32 {
	1
}
