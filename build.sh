#!/bin/sh
if [[ ! -f "binaries/reporter" ]]; then
  cd ..
  git clone https://github.com/ag0st/kollaps-report.git kollaps-report
  cd kollaps-report || return
  echo "CHANGING RUST VERSION FOR BUILDING ELF FILE"
  rustup default 1.59
  cargo clean
  cargo build --release
  echo "FINISHED BUILDING THE KOLLAPS-REPORT FILE, SWITCHING BACK TO RUST 1.66"
  rustup default 1.66
  cd ../kollaps || return
  cargo clean
  cargo build --release
  echo "COPYING KOLLAPS-REPORT EXECUTABLE HERE"
  cp ../kollaps-report/target/release/reporter binaries/
fi
