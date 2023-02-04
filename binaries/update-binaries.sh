#!/bin/bash
cargo build --release

if [[ -f binaries/reporter ]]; then
  rm binaries/reporter
fi
if [[ -f binaries/runner ]]; then
  rm binaries/runner
fi
if [[ -f binaries/client ]]; then
  rm binaries/client
fi


cp target/release/reporter binaries/
cp target/release/runner binaries/
cp target/release/client binaries/