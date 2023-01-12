cd ../monitor || return
cargo bpf build --target-dir=../target
cd ../netmod || return
rm src/usage.elf
cp ../target/bpf/programs/usage/usage.elf src/usage.elf