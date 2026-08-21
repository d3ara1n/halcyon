use std::{env, fs, path::PathBuf};

const MEMORY_SCRIPT_ENV: &str = "ERHINO_MEMORY_SCRIPT";

fn canonicalize(path: PathBuf, name: &str) -> PathBuf {
    fs::canonicalize(&path)
        .unwrap_or_else(|error| panic!("无法定位{name} {}：{error}", path.display()))
}

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo 未提供 CARGO_MANIFEST_DIR"),
    );
    let memory_script = canonicalize(
        env::var_os(MEMORY_SCRIPT_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| manifest_dir.join("../platforms/qemu/virt/memory.x")),
        "平台内存脚本",
    );
    let linker_script = canonicalize(manifest_dir.join("../platforms/linker.ld"), "内核链接脚本");

    println!("cargo::rerun-if-env-changed={MEMORY_SCRIPT_ENV}");
    println!("cargo::rerun-if-changed={}", memory_script.display());
    println!("cargo::rerun-if-changed={}", linker_script.display());

    let memory = fs::read_to_string(&memory_script).expect("无法读取平台内存脚本");
    let linker = fs::read_to_string(&linker_script).expect("无法读取内核链接脚本");
    let generated_script = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo 未提供 OUT_DIR"))
        .join("erhino.ld");

    fs::write(&generated_script, format!("{memory}\n{linker}"))
        .expect("无法生成内核链接脚本");
    println!(
        "cargo::rustc-link-arg-bin=erhino_kernel=-T{}",
        generated_script.display()
    );
}
