#!/bin/sh
python3 tools/integration_check.py . >/dev/null && echo "--- INTEGRATION_REPORT.md written"
cargo build 2> build_errors.txt || { echo "BUILD FAILED - see build_errors.txt"; exit 1; }
cargo test 2> test_errors.txt || { echo "TESTS FAILED - see test_errors.txt"; exit 1; }
echo "ALL OK"
