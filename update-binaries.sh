#!/bin/bash
if [[ -f binaries/reporter ]]; then
  rm binaries/reporter
fi
cp ../kollaps-report/target/release/reporter binaries/