use std::collections::HashMap;
use serde::Deserialize;
use toml::Value;

#[derive(Deserialize, Debug)]
pub struct Template {
	#[serde(default)]
	pub opt_args: Vec<String>,

	#[serde(default)]
	pub depends: Vec<String>,
	#[serde(default)]
	pub host_depends: Vec<String>,

	#[serde(default)]
	pub add_prepare: Vec<String>,
	#[serde(default)]
	pub add_configure: Vec<String>,
	#[serde(default)]
	pub add_build: Vec<String>,
	#[serde(default)]
	pub add_install: Vec<String>,

	#[serde(default)]
	pub prepare_env: Vec<HashMap<String, String>>,
	#[serde(default)]
	pub configure_env: Vec<HashMap<String, String>>,
	#[serde(default)]
	pub build_env: Vec<HashMap<String, String>>,
	#[serde(default)]
	pub install_env: Vec<HashMap<String, String>>,

	#[serde(default)]
	pub default_prepare: String,
	#[serde(default)]
	pub default_configure: String,
	#[serde(default)]
	pub default_build: String,
	#[serde(default)]
	pub default_install: String,

	#[serde(flatten)]
	pub others: HashMap<String, Value>
}

#[derive(Deserialize, Debug)]
pub struct Templates {
	#[serde(flatten)]
	pub templates: HashMap<String, Template>
}
