#![feature(io_error_more)]

mod build;

use std::collections::HashMap;
use std::fs::{create_dir_all, read_to_string, write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{exit, Command};
use aho_corasick::AhoCorasick;
use serde::Deserialize;
use walkdir::WalkDir;
use crate::build::Step;

fn yes() -> bool {
	true
}

#[derive(Deserialize)]
struct GeneralConfig {
	target: String,
	sysroot: String,
	recipes_dir: String,
	host_recipes_dir: String,
	meta_dir: String,
	build_root: String,
	#[serde(default)]
	threads: usize,
	#[serde(default = "yes")]
	prefer_binaries: bool,
	#[serde(flatten)]
	others: HashMap<String, String>
}

#[derive(Deserialize)]
struct BuildConfig {
	cc: String,
	cxx: String,
	#[serde(default)]
	cflags: String,
	#[serde(default)]
	cxxflags: String,
	#[serde(default)]
	ldflags: String,
	#[serde(flatten)]
	others: HashMap<String, String>
}

impl Default for BuildConfig {
	fn default() -> Self {
		Self {
			cc: "cc".to_string(),
			cxx: "c++".to_string(),
			cflags: "".to_string(),
			cxxflags: "".to_string(),
			ldflags: "".to_string(),
			others: HashMap::new()
		}
	}
}

#[derive(Deserialize)]
struct Config {
	general: GeneralConfig,
	#[serde(default)]
	host: BuildConfig,
	target: BuildConfig
}

#[derive(Copy, Clone, PartialEq)]
enum Op {
	Prepare,
	Configure,
	Build,
	Install,
	Sync
}

fn usage() -> ! {
	eprintln!(r"usage: qpkg <ops>... <args>... <names>...
op:
    prepare
    configure
    build
    install
    sync

    rebuild     equivalent to build install sync --force
args:
    --force
    --host
    --env=<name>=<value>
    --config=<path_to_qpkg.toml>");
	exit(1);
}

fn load_config(path: String) -> Config {
	if !path.is_empty() {
		let data = match read_to_string(&path) {
			Ok(data) => data,
			Err(e) => {
				eprintln!("error: failed to read {}: {}", path, e);
				exit(1);
			}
		};

		match toml::from_str::<Config>(&data) {
			Ok(mut config) => {
				if matches!(config.general.build_root.as_str(), "" | ".") {
					let abs = std::path::absolute(&path)
						.expect("failed to get absolute config path");
					config.general.build_root = abs.parent().unwrap().to_str().unwrap().to_string();
				}

				let abs = std::path::absolute(&config.general.build_root)
					.expect("failed to get absolute build root path");
				config.general.build_root = abs.to_str().unwrap().to_string();

				config
			},
			Err(e) => {
				eprintln!("error: failed to parse config: {}", e);
				exit(1);
			}
		}
	} else {
		let list = ["qpkg.toml", "/etc/qpkg.toml"];
		for path in list {
			let data = match read_to_string(path) {
				Ok(data) => data,
				Err(_) => {
					continue;
				}
			};

			return match toml::from_str::<Config>(&data) {
				Ok(mut config) => {
					if matches!(config.general.build_root.as_str(), "" | ".") {
						let abs = std::path::absolute(&path)
							.expect("failed to get absolute config path");
						config.general.build_root = abs.parent().unwrap().to_str().unwrap().to_string();
					}

					let abs = std::path::absolute(&config.general.build_root)
						.expect("failed to get absolute build root path");
					config.general.build_root = abs.to_str().unwrap().to_string();

					config
				},
				Err(e) => {
					eprintln!("error: failed to parse config {}: {}", path, e);
					exit(1);
				}
			};
		}

		eprintln!("error: failed to find qpkg.toml in the current directory or in /etc");
		exit(1);
	}
}

fn load_recipe(config: &Config, name: &str, host: bool) -> build::Recipe {
	let path = if host {
		Path::new(&config.general.host_recipes_dir).join(name).join("build.toml")
	} else {
		Path::new(&config.general.recipes_dir).join(name).join("build.toml")
	};

	let data = match read_to_string(&path) {
		Ok(data) => data,
		Err(e) => {
			eprintln!("error: failed to read recipe {}: {}", path.display(), e);
			exit(1);
		}
	};

	match toml::from_str(&data) {
		Ok(config) => config,
		Err(e) => {
			eprintln!("error: failed to parse recipe: {}", e);
			exit(1);
		}
	}
}

fn finalize_recipe(
	recipe: &mut build::Recipe,
	config: &mut Config,
	root_src_dir: &Path,
	dest_dir: &Path) {
	recipe.general.workdir = recipe.general.workdir.replace("@VERSION@", recipe.general.version.as_str());

	let src_dir = if !recipe.general.src_unpack_dir.is_empty() {
		std::path::absolute(Path::new(&recipe.general.src_unpack_dir).join(&recipe.general.workdir))
	} else {
		std::path::absolute(root_src_dir.join(&recipe.general.workdir))
	}.expect("failed to get absolute srcdir");

	let dest_dir = std::path::absolute(dest_dir)
		.expect("failed to get absolute destdir");

	if config.general.threads == 0 {
		config.general.threads = std::thread::available_parallelism()
			.map(|num| num.get())
			.unwrap_or(1);
	}

	let threads_str = config.general.threads.to_string();

	let build_root_dir = std::path::absolute(&config.general.build_root)
		.expect("failed to make build root absolute");
	let sysroot_dir = std::path::absolute(&config.general.sysroot)
		.expect("failed to make sysroot absolute");

	let mut to_replace = Vec::from([
		"@VERSION@",
		"@BUILDROOT@",
		"@SRCDIR@",
		"@DESTDIR@",
		"@SYSROOT@",
		"@TARGET@",
		"@THREADS@"
	]);
	let mut replaces = Vec::from([
		recipe.general.version.as_str(),
		build_root_dir.to_str().unwrap(),
		src_dir.to_str().unwrap(),
		dest_dir.to_str().unwrap(),
		sysroot_dir.to_str().unwrap(),
		config.general.target.as_str(),
		threads_str.as_str()
	]);

	let mut to_replace_strings = Vec::new();

	for (name, replace) in &config.general.others {
		to_replace_strings.push(format!("@{}@", name.to_uppercase()));
		replaces.push(replace);
	}

	for to_replace_string in &to_replace_strings {
		to_replace.push(to_replace_string);
	}

	let aho = AhoCorasick::new(to_replace).unwrap();

	for src in &mut recipe.general.src {
		*src = aho.replace_all(&src, &replaces);
	}

	for step in [
		&mut recipe.prepare,
		&mut recipe.configure,
		&mut recipe.build,
		&mut recipe.install] {
		for list in &mut step.args {
			for arg in list {
				*arg = aho.replace_all(arg, &replaces);
			}
		}

		for env in &mut step.env {
			for (_, value) in env {
				*value = aho.replace_all(value, &replaces);
			}
		}
	}
}

fn touch_file(path: impl AsRef<Path>) {
	let parent = path.as_ref().parent().unwrap();
	match create_dir_all(parent) {
		Ok(_) => {},
		Err(e) => {
			eprintln!("error: failed to create path {}: {}", parent.display(), e);
			exit(1);
		}
	}
	write(path.as_ref(), "").unwrap();
}

fn remove_file(path: impl AsRef<Path>) {
	match std::fs::remove_file(path.as_ref()) {
		Ok(_) => {},
		Err(e) => {
			if e.kind() != std::io::ErrorKind::NotFound {
				eprintln!("error: failed to remove {}: {}", path.as_ref().display(), e);
				exit(1);
			}
		}
	}
}

fn main() {
	let args: Vec<_> = std::env::args().skip(1).collect();

	if args.len() < 2 {
		usage();
	}

	let mut parsing_ops = true;
	let mut force = false;
	let mut host = false;
	let mut ops = Vec::new();
	let mut names = Vec::new();
	let mut config_path = String::new();

	let mut global_env = Vec::new();

	for arg in args {
		if parsing_ops {
			match arg.as_str() {
				"prepare" => ops.push(Op::Prepare),
				"configure" => ops.push(Op::Configure),
				"build" => ops.push(Op::Build),
				"rebuild" => {
					ops.push(Op::Build);
					ops.push(Op::Install);
					ops.push(Op::Sync);
					force = true;
				},
				"install" => ops.push(Op::Install),
				"sync" => ops.push(Op::Sync),
				"--force" => force = true,
				"--host" => host = true,
				arg if arg.starts_with("--config=") => {
					config_path = arg.strip_prefix("--config=").unwrap().to_string();
				}
				arg if arg.starts_with("--env=") => {
					let (name, value) = arg
						.strip_prefix("--env=")
						.unwrap()
						.split_once('=')
						.unwrap();
					global_env.push((name.to_string(), value.to_string()));
				}
				arg if arg.starts_with("-") => {
					eprintln!("error: unsupported argument {}", arg);
					usage();
				}
				_ => parsing_ops = false
			}
		}

		if !parsing_ops {
			names.push(arg);
		}
	}

	if ops.is_empty() {
		eprintln!("error: no ops specified");
		exit(1);
	}
	if names.is_empty() {
		eprintln!("error: no packages specified");
		exit(1);
	}

	let mut config = load_config(config_path);

	let meta_dir = config.general.meta_dir.clone();
	let meta_dir = Path::new(&meta_dir);

	let abs_host_cc = which::which(&config.host.cc)
		.expect("failed to find host cc in PATH");
	let abs_host_cxx = which::which(&config.host.cxx)
		.expect("failed to find host cxx in PATH");

	let target_cc = config.target.cc.replace("@BUILDROOT@", &config.general.build_root);
	let target_cxx = config.target.cc.replace("@BUILDROOT@", &config.general.build_root);

	global_env.push(("CC".to_string(), target_cc));
	global_env.push(("CXX".to_string(), target_cxx));
	global_env.push(("QPKG_HOST_CC".to_string(), abs_host_cc.to_str().unwrap().to_string()));
	global_env.push(("QPKG_HOST_CXX".to_string(), abs_host_cxx.to_str().unwrap().to_string()));
	if !config.target.cflags.is_empty() {
		global_env.push(("CFLAGS".to_string(), config.target.cflags.clone()));
	}
	if !config.target.cxxflags.is_empty() {
		global_env.push(("CXXFLAGS".to_string(), config.target.cxxflags.clone()));
	}
	if !config.target.ldflags.is_empty() {
		global_env.push(("LDFLAGS".to_string(), config.target.ldflags.clone()));
	}
	for (name, value) in &config.target.others {
		global_env.push((name.clone(), value.clone()));
	}

	let mut global_host_env = Vec::new();
	global_host_env.push(("CC".to_string(), config.host.cc.clone()));
	global_host_env.push(("CXX".to_string(), config.host.cxx.clone()));
	if !config.host.cflags.is_empty() {
		global_host_env.push(("CFLAGS".to_string(), config.host.cflags.clone()));
	}
	if !config.host.cxxflags.is_empty() {
		global_host_env.push(("CXXFLAGS".to_string(), config.host.cxxflags.clone()));
	}
	if !config.host.ldflags.is_empty() {
		global_host_env.push(("LDFLAGS".to_string(), config.host.ldflags.clone()));
	}

	let mut force_prepare = false;
	let mut force_configure = false;
	let mut force_build = false;
	let mut force_install = false;

	let mut do_prepare = false;
	let mut do_configure = false;
	let mut do_build = false;
	let mut do_install = false;
	let mut do_sync = false;

	if force {
		for op in &ops {
			match op {
				Op::Prepare => force_prepare = true,
				Op::Configure => force_configure = true,
				Op::Build => force_build = true,
				Op::Install => force_install = true,
				Op::Sync => {}
			}
		}
	}

	for op in &ops {
		match op {
			Op::Prepare => do_prepare = true,
			Op::Configure => do_configure = true,
			Op::Build => do_build = true,
			Op::Install => do_install = true,
			Op::Sync => do_sync = true
		}
	}

	if do_install {
		do_prepare = true;
		do_configure = true;
		do_build = true;
	} else if do_build {
		do_prepare = true;
		do_configure = true;
	} else if do_configure {
		do_prepare = true;
	}

	struct Entry {
		name: String,
		processed: bool,
		host: bool,
		user_specified: bool
	}

	let mut stack: Vec<_> = names.into_iter()
		.map(|name| Entry { name, processed: false, host, user_specified: true })
		.collect();

	let existing_path = std::env::var("PATH").expect("no PATH set");
	let mut new_path = String::new();
	let mut aclocal = String::new();

	while let Some(entry) = stack.pop() {
		let mut recipe = load_recipe(&config, &entry.name, entry.host);

		if !entry.processed {
			stack.push(Entry { name: entry.name, processed: true, host: entry.host, user_specified: entry.user_specified });
			for dep in &recipe.general.depends {
				stack.push(Entry { name: dep.clone(), processed: false, host: false, user_specified: false });
			}
			for dep in &recipe.general.host_depends {
				stack.push(Entry { name: dep.clone(), processed: false, host: true, user_specified: false });
			}
			continue;
		}

		if config.general.prefer_binaries && !recipe.general.binary_alternative.is_empty() {
			stack.push(Entry {
				name: recipe.general.binary_alternative,
				processed: false,
				host: entry.host,
				user_specified: entry.user_specified
			});
			continue;
		}

		if entry.host {
			let path = std::path::absolute(Path::new(&config.general.build_root)
				.join("host_pkgs")
				.join(&entry.name))
				.expect("failed to get absolute path for host dependency");
			for dir in ["bin", "usr/bin", "usr/local/bin"] {
				if !new_path.ends_with(':') {
					new_path.push(':');
				}

				new_path += path.to_str().unwrap();
				if !new_path.ends_with('/') {
					new_path.push('/');
				}
				new_path += dir;
			}

			if recipe.general.exports_aclocal {
				for dir in ["share", "usr/share", "usr/local/share"] {
					if !aclocal.ends_with(':') {
						aclocal.push(':');
					}

					aclocal += path.to_str().unwrap();
					if !aclocal.ends_with('/') {
						aclocal.push('/');
					}
					aclocal += dir;
					aclocal += "/aclocal";
				}
			}
		}

		let (
			build_dir,
			dest_dir,
			root_src_dir
		) = if entry.host {
			let build_dir = Path::new(&config.general.build_root)
				.join("host_builds")
				.join(&entry.name);
			let dest_dir = Path::new(&config.general.build_root)
				.join("host_pkgs")
				.join(&entry.name);
			let root_src_dir = Path::new(&config.general.build_root)
				.join("host_sources")
				.join(&entry.name);
			(build_dir, dest_dir, root_src_dir)
		} else {
			let build_dir = Path::new(&config.general.build_root)
				.join("pkg_builds")
				.join(&entry.name);
			let dest_dir = Path::new(&config.general.build_root)
				.join("pkgs")
				.join(&entry.name);
			let root_src_dir = Path::new(&config.general.build_root)
				.join("sources")
				.join(&entry.name);
			(build_dir, dest_dir, root_src_dir)
		};

		let archives_dir = Path::new(&config.general.build_root)
			.join("archives");

		match create_dir_all(&root_src_dir) {
			Ok(_) => {},
			Err(e) => {
				eprintln!("error: failed to create directory {}: {}", root_src_dir.display(), e);
				exit(1);
			}
		}

		match create_dir_all(&archives_dir) {
			Ok(_) => {},
			Err(e) => {
				eprintln!("error: failed to create directory {}: {}", archives_dir.display(), e);
				exit(1);
			}
		}

		match create_dir_all(&dest_dir) {
			Ok(_) => {},
			Err(e) => {
				eprintln!("error: failed to create directory {}: {}", dest_dir.display(), e);
				exit(1);
			}
		}

		finalize_recipe(&mut recipe, &mut config, &root_src_dir, &dest_dir);

		for src in &recipe.general.src {
			let name = if let Some((_, name)) = src.rsplit_once('/') {
				if let Some(pos) = name.find(".git") {
					&name[0..pos]
				} else {
					name
				}
			} else {
				src.as_str()
			};

			let path = if !recipe.general.src_unpack_dir.is_empty() {
				Path::new(&recipe.general.src_unpack_dir).to_owned().join(name)
			} else {
				archives_dir.join(name)
			};

			if !path.exists() {
				if let Some(pos) = src.find(".git") {
					let mut full = false;
					let mut branch = "";

					if pos + 4 != src.len() && &src[pos + 4..pos + 5] == ":" {
						let opts = &src[pos + 5..];
						if let Some(pos) = opts.find(",full") {
							branch = &opts[0..pos];
							full = true;
						} else {
							branch = opts;
						}
					}

					println!("info: fetching {} using git", &src[0..pos]);

					let branch_args = ["-b", branch];

					let cmd = Command::new("git")
						.arg("clone")
						.arg(&src[0..pos])
						.args(if !full {
							["--depth=1"].as_slice()
						} else {
							[].as_slice()
						})
						.args(if !branch.is_empty() {
							branch_args.as_slice()
						} else {
							[].as_slice()
						})
						.args(if recipe.general.recurse_submodules {
							["--recurse-submodules"].as_slice()
						} else {
							[].as_slice()
						})
						.arg(path.to_str().unwrap())
						.spawn().expect("failed to spawn git")
						.wait().expect("git failed");
					if !cmd.success() {
						eprintln!("error: git failed with {}", cmd);
						exit(1);
					}
				} else if src.starts_with("http") {
					println!("info: fetching {} using wget", src);

					let cmd = Command::new("wget")
						.arg(src)
						.args(["-O", path.to_str().unwrap()])
						.spawn().expect("failed to spawn wget")
						.wait().expect("wget failed");
					if !cmd.success() {
						eprintln!("error: wget failed with {}", cmd);
						exit(1);
					}
				}
			}
		}

		if !entry.user_specified || do_prepare {
			let prepared_path = root_src_dir.join("qpkg.prepared");

			if entry.user_specified && force_prepare {
				println!("info: forcing prepare for {}", entry.name);
				match std::fs::remove_file(&prepared_path) {
					Ok(_) => {},
					Err(e) => {
						if e.kind() != std::io::ErrorKind::NotFound {
							eprintln!("error: failed to remove {}: {}", prepared_path.display(), e);
							exit(1);
						}
					}
				}
			}

			if !prepared_path.exists() {
				println!("info: preparing source for {}", entry.name);

				std::fs::remove_dir_all(&root_src_dir).expect("failed to remove srcdir");
				create_dir_all(&root_src_dir).expect("failed to create srcdir");

				let work_dir = std::path::absolute(root_src_dir.join(&recipe.general.workdir))
					.expect("failed to get absolute srcdir");

				if !recipe.general.no_auto_unpack {
					for src in &recipe.general.src {
						let name = if let Some((_, name)) = src.rsplit_once('/') {
							if let Some(pos) = name.find(".git") {
								&name[0..pos]
							} else {
								name
							}
						} else {
							src.as_str()
						};

						let path = if !recipe.general.src_unpack_dir.is_empty() {
							Path::new(&recipe.general.src_unpack_dir).to_owned().join(name)
						} else {
							archives_dir.join(name)
						}.canonicalize().expect("failed to canonicalize src path");

						if src.ends_with(".tar.xz") ||
							src.ends_with(".tar.gz") ||
							src.ends_with(".tar.bz2") ||
							src.ends_with(".tar.zst") {
							let cmd = Command::new("tar")
								.arg("-xf")
								.arg(path.to_str().unwrap())
								.current_dir(&root_src_dir)
								.spawn().expect("failed to spawn tar")
								.wait().expect("tar failed");
							if !cmd.success() {
								eprintln!("error: tar failed with {}", cmd);
								exit(1);
							}
						} else if src.contains(".git") {
							if let Err(err) = std::os::unix::fs::symlink(&path, &work_dir) {
								if err.kind() != std::io::ErrorKind::AlreadyExists {
									eprintln!(
										"error: failed to symlink {} -> {}: {}",
										path.display(),
										work_dir.display(),
										err);
									exit(1);
								}
							}
						}
					}
				}

				create_dir_all(&work_dir).ok();

				let recipes_dir = if entry.host {
					Path::new(&config.general.host_recipes_dir)
				} else {
					Path::new(&config.general.recipes_dir)
				};

				let patches_dir = std::path::absolute(recipes_dir
					.join(&entry.name)
					.join("patches"))
					.expect("failed to get absolute patches dir");
				if !recipe.general.no_auto_patch && patches_dir.exists() {
					for file in WalkDir::new(&patches_dir) {
						let file = file.unwrap();
						let path = file.path();

						if let Some(ext) = path.extension() {
							let ext = ext.to_str().unwrap();
							if matches!(ext, "patch" | "diff") {
								println!("info: applying patch {}", file.file_name().to_str().unwrap());

								let cmd = Command::new("patch")
									.arg("-Np1")
									.args(["-i", path.to_str().unwrap()])
									.current_dir(&work_dir)
									.spawn().expect("failed to spawn patch")
									.wait().expect("patch failed");
								if !cmd.success() {
									eprintln!("error: patch failed with {}", cmd);
									exit(1);
								}
							}
						}
					}
				}

				for args in &recipe.prepare.args {
					let value = args.join(" ");

					let global_envs = if entry.host {
						&global_host_env
					} else {
						&global_env
					}.iter().map(|(name, value)| (name.as_str(), value.as_str()));

					let env: Vec<_> = recipe.prepare.env
						.iter()
						.map(|map| map.iter().next().unwrap())
						.collect();

					let real_path = new_path.clone() + ":" + &existing_path;

					let cmd = Command::new("/bin/sh")
						.arg("-c")
						.arg(&value)
						.current_dir(&work_dir)
						.env("LC_ALL", "C")
						.envs(env.iter().map(|(name, value)| (name.as_str(), value.as_str())))
						.envs(global_envs)
						.env("PATH", &real_path)
						.env("ACLOCAL_PATH", &aclocal)
						.spawn().expect("failed to spawn sh")
						.wait().expect("sh failed");
					if !cmd.success() {
						eprintln!("error: command {} failed with status {}", value, cmd);
						exit(1);
					}
				}

				touch_file(prepared_path);
			}
		}

		let execute_step = |step: &Step| {
			create_dir_all(&build_dir).expect("failed to create build dir");

			let env: Vec<_> = step.env
				.iter()
				.map(|map| map.iter().next().unwrap())
				.collect();

			let sysroot_dir = std::path::absolute(&config.general.sysroot)
				.expect("failed to make sysroot absolute");

			for args in &step.args {
				let value = args.join(" ");

				let global_envs = if entry.host {
					&global_host_env
				} else {
					&global_env
				}.iter().map(|(name, value)| (name.as_str(), value.as_str()));

				let real_path = new_path.clone() + ":" + &existing_path;

				let cmd = Command::new("/bin/sh")
					.arg("-c")
					.arg(&value)
					.current_dir(&build_dir)
					.env("LC_ALL", "C")
					.envs(env.iter().map(|(name, value)| (name.as_str(), value.as_str())))
					.envs(global_envs)
					.env("QPKG_SYSROOT_DIR", sysroot_dir.to_str().unwrap())
					.env("PATH", &real_path)
					.env("ACLOCAL_PATH", &aclocal)
					.spawn().expect("failed to spawn sh")
					.wait().expect("sh failed");
				if !cmd.success() {
					eprintln!("error: command {} failed with status {}", value, cmd);
					exit(1);
				}
			}
		};

		if !entry.user_specified || do_configure {
			if entry.user_specified && force_configure {
				println!("info: forcing configure for {}", entry.name);
				std::fs::remove_dir_all(&build_dir).expect("failed to remove build dir");
			}

			if !build_dir.join("qpkg.configured").exists() {
				println!("info: configuring {}", entry.name);
				execute_step(&recipe.configure);
				touch_file(build_dir.join("qpkg.configured"));
			}
		}

		if !entry.user_specified || do_build {
			if entry.user_specified && force_build {
				println!("info: forcing build for {}", entry.name);
				remove_file(build_dir.join("qpkg.built"));
			}

			if !build_dir.join("qpkg.built").exists() {
				println!("info: building {}", entry.name);
				execute_step(&recipe.build);
				touch_file(build_dir.join("qpkg.built"));
			}
		}

		if !entry.user_specified || do_install {
			if entry.user_specified && force_install {
				println!("info: forcing install for {}", entry.name);
				remove_file(build_dir.join("qpkg.installed"));
			}

			if !build_dir.join("qpkg.installed").exists() {
				println!("info: installing {}", entry.name);
				execute_step(&recipe.install);
				touch_file(build_dir.join("qpkg.installed"));
			}
		}

		let pkg_meta_dir = meta_dir.join(&entry.name);
		let installed = read_to_string(pkg_meta_dir.join("FILES")).unwrap_or_default();

		if !entry.user_specified && !installed.trim().is_empty() {
			continue;
		}

		if !entry.host && (!entry.user_specified || do_sync) {
			if !dest_dir.exists() {
				eprintln!("error: dest dir {} doesn't exist", dest_dir.display());
				exit(1);
			}

			let abs_dest_dir = dest_dir.canonicalize().expect("failed to canonizalize dest dir");

			let mut files = String::new();

			let sysroot = Path::new(&config.general.sysroot);

			for file in WalkDir::new(&abs_dest_dir) {
				let file = file.unwrap();
				let path = file.path().strip_prefix(&abs_dest_dir).unwrap();

				let full_path = sysroot.join(path);

				if file.file_type().is_dir() {
					match create_dir_all(&full_path) {
						Ok(_) => {},
						Err(e) => {
							eprintln!("error: failed to create directory {}: {}", full_path.display(), e);
							exit(1);
						}
					}
				} else if file.file_type().is_symlink() {
					let orig = std::fs::read_link(file.path())
						.expect("failed to resolve symlink");
					match std::os::unix::fs::symlink(orig, &full_path) {
						Ok(_) => {},
						Err(e) => {
							if e.kind() != std::io::ErrorKind::AlreadyExists {
								eprintln!("error: failed to create symlink {}: {}", full_path.display(), e);
								exit(1);
							}
						}
					}
				} else {
					if full_path.exists() {
						let mut perms = full_path
							.metadata()
							.expect("failed to query file metadata")
							.permissions();
						// owner/group write
						perms.set_mode(perms.mode() | 0o220);
						std::fs::set_permissions(&full_path, perms)
							.expect("failed to set file permissions");
					}

					match std::fs::copy(file.path(), &full_path) {
						Ok(_) => {},
						Err(e) => {
							eprintln!("error: failed to copy {} to {}: {}", path.display(), full_path.display(), e);
							exit(1);
						}
					}
				}

				files += path.to_str().unwrap();
				files.push('\n');
			}

			for name in installed.lines().rev()
				.filter(|name| files.lines().find(|line| line == name).is_none()) {
				let name = name.trim();
				if name.is_empty() {
					continue;
				}

				let path = sysroot.join(name);
				match std::fs::remove_dir(&path) {
					Ok(_) => {},
					Err(e) => {
						if e.kind() == std::io::ErrorKind::NotADirectory {
							match std::fs::remove_file(&path) {
								Ok(_) => {},
								Err(e) => {
									if e.kind() != std::io::ErrorKind::NotFound {
										eprintln!("error: failed to remove {}: {}", path.display(), e);
										exit(1);
									}
								}
							}
						} else if e.kind() != std::io::ErrorKind::NotFound &&
							e.kind() != std::io::ErrorKind::DirectoryNotEmpty {
							eprintln!("error: failed to remove {}: {}", path.display(), e);
							exit(1);
						}
					}
				}
			}

			match create_dir_all(&pkg_meta_dir) {
				Ok(_) => {},
				Err(e) => {
					eprintln!("error: failed to create directory {}: {}", pkg_meta_dir.display(), e);
					exit(1);
				}
			}

			match write(pkg_meta_dir.join("FILES"), files) {
				Ok(_) => {}
				Err(e) => {
					eprintln!("error: failed to write {}: {}", pkg_meta_dir.join("FILES").display(), e);
					exit(1);
				}
			}
		}
	}
}
