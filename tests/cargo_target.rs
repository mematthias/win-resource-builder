#![allow(clippy::expect_used, reason = "A test fails loudly")]

use std::env;
use std::process::Command;
use win_resource_builder::resource::{CargoTargetError, CoffMachine};

const CHILD: &str = "WIN_RESOURCE_BUILDER_CARGO_TARGET_CHILD";
const RESULT: &str = "CARGO_TARGET_RESULT=";

#[must_use]
fn describe(result: Result<CoffMachine, CargoTargetError>) -> String {
	match result {
		Ok(CoffMachine::X86) => "x86".to_owned(),
		Ok(CoffMachine::X64) => "x64".to_owned(),
		Ok(CoffMachine::Arm) => "arm".to_owned(),
		Ok(CoffMachine::Arm64) => "arm64".to_owned(),
		Ok(CoffMachine::Arm64Ec) => "arm64ec".to_owned(),
		Err(err) => err.to_string(),
	}
}

#[test]
fn child() {
	if env::var_os(CHILD).is_some() {
		println!("{RESULT}{}", describe(CoffMachine::cargo_target()));
	}
}

#[must_use]
fn run(arch: Option<&str>) -> String {
	let mut command = Command::new(env::current_exe().expect("Path of the test binary"));
	command
		.args(["child", "--exact", "--nocapture", "--test-threads=1"])
		.env(CHILD, "1");
	match arch {
		Some(arch) => command.env("CARGO_CFG_TARGET_ARCH", arch),
		None => command.env_remove("CARGO_CFG_TARGET_ARCH"),
	};
	let output = command.output().expect("The test binary runs as child");
	assert!(output.status.success(), "Child failed: {}", String::from_utf8_lossy(&output.stderr));
	String::from_utf8_lossy(&output.stdout)
		.lines()
		.find_map(|line| line.split_once(RESULT).map(|(_, result)| result))
		.expect("The child reports its result")
		.to_owned()
}

#[test]
fn cargo_target_reads_the_variable() {
	assert_eq!(run(Some("x86")), describe(Ok(CoffMachine::X86)));
	assert_eq!(run(Some("x86_64")), describe(Ok(CoffMachine::X64)));
	assert_eq!(run(Some("arm")), describe(Ok(CoffMachine::Arm)));
	assert_eq!(run(Some("aarch64")), describe(Ok(CoffMachine::Arm64)));
	assert_eq!(run(Some("arm64ec")), describe(Ok(CoffMachine::Arm64Ec)));
	assert_eq!(
		run(Some("riscv64")),
		describe(Err(CargoTargetError::UnsupportedArch {
			arch: "riscv64".to_owned()
		}))
	);
	assert_eq!(run(None), describe(Err(CargoTargetError::NotSet)));
}
