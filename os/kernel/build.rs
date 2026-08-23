use std::{env, fs, path::PathBuf};

const MEMORY_SCRIPT_ENV: &str = "ERHINO_MEMORY_SCRIPT";

fn canonicalize(path: PathBuf, name: &str) -> PathBuf {
    fs::canonicalize(&path)
        .unwrap_or_else(|error| panic!("cannot locate {name} {}: {error}", path.display()))
}

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo did not provide CARGO_MANIFEST_DIR"),
    );
    let memory_script = canonicalize(
        env::var_os(MEMORY_SCRIPT_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| manifest_dir.join("../platforms/qemu/virt/memory.x")),
        "platform memory script",
    );
    let linker_script = canonicalize(manifest_dir.join("../platforms/linker.ld"), "kernel linker script");

    println!("cargo::rerun-if-env-changed={MEMORY_SCRIPT_ENV}");
    println!("cargo::rerun-if-changed={}", memory_script.display());
    println!("cargo::rerun-if-changed={}", linker_script.display());

    let memory = fs::read_to_string(&memory_script).expect("cannot read platform memory script");
    let linker = fs::read_to_string(&linker_script).expect("cannot read kernel linker script");
    let generated_script = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo did not provide OUT_DIR"))
        .join("erhino.ld");

    fs::write(&generated_script, format!("{memory}\n{linker}"))
        .expect("cannot generate kernel linker script");
    println!(
        "cargo::rustc-link-arg-bin=erhino_kernel=-T{}",
        generated_script.display()
    );
}
