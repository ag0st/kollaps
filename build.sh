#!/bin/sh
cd monitor || return
echo "CHANGING RUST VERSION FOR BUILDING ELF FILE"
rustup default 1.59
cargo clean
cargo bpf build --target-dir=../target
echo "FINISHED BUILDING THE ELF FILE, SWITCHING BACK TO RUST 1.66"
rustup default 1.66
cd .. || return

if test -f "./netmod/src/usage.elf"; then
  rm netmod/src/usage.elf
fi

cp target/bpf/programs/usage/usage.elf netmod/src/usage.elf

cargo clean
cargo build --release