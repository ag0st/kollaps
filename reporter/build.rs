use std::process;

fn main() {
    println!("cargo:rerun-if-changed=../monitor/src/usage/");
    println!("cargo:rerun-if-changed=src/usage.elf");
    let res = process::Command::new("sh").arg("build_and_replace_elf.sh")
        .output().expect("cannot launch build script for ELF eBPF");
        if !res.status.success() {
            let output = String::from_utf8_lossy(&*res.stderr);
            panic!("error trying to update elf file, building failed: {}", output)
        }
}