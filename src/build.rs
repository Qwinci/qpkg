use std::collections::HashMap;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct General {
	pub name: String,
	pub version: String,
	pub src: Vec<String>,
	#[serde(default)]
	pub src_unpack_dir: String,
	pub workdir: String,
	#[serde(default)]
	pub binary_alternative: String,
	#[serde(default)]
	pub no_auto_patch: bool,
	#[serde(default)]
	pub no_auto_unpack: bool,
	#[serde(default)]
	pub recurse_submodules: bool,
	#[serde(default)]
	pub exports_aclocal: bool,
	#[serde(default)]
	pub depends: Vec<String>,
	#[serde(default)]
	pub host_depends: Vec<String>
}

#[derive(Deserialize, Debug, Default)]
pub struct Step {
	#[serde(default)]
	pub args: Vec<Vec<String>>,
	#[serde(default)]
	pub env: Vec<HashMap<String, String>>
}

#[derive(Deserialize, Debug)]
pub struct Recipe {
	pub general: General,
	#[serde(default)]
	pub prepare: Step,
	#[serde(default)]
	pub configure: Step,
	#[serde(default)]
	pub build: Step,
	#[serde(default)]
	pub install: Step
}
