#!/bin/bash
cd "$(dirname "$0")"
echo "=== cargo build $(date) ===" | tee build_output.log
cargo build 2>&1 | tee -a build_output.log
EXIT_CODE=${PIPESTATUS[0]}
echo "=== exit code: $EXIT_CODE ===" | tee -a build_output.log
