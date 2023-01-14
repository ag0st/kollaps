cd ../monitor || return
cargo bpf build
cd ../reporter || return
rm src/usage.elf
cp ../monitor/target/bpf/programs/usage/usage.elf src/usage.elf