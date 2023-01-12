cd monitor || return
cargo bpf build --target-dir=../target
cd .. || return
if test -f "src/usage.elf"; then
  rm src/usage.elf
fi
cp target/bpf/programs/usage/usage.elf src/usage.elf
