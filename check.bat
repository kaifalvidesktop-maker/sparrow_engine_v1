@echo off
rem 1) static checker  2) build  3) tests. Send build_errors.txt + INTEGRATION_REPORT.md to the AI.
python tools\integration_check.py . > nul
echo --- INTEGRATION_REPORT.md written
cargo build 2> build_errors.txt
if errorlevel 1 (echo BUILD FAILED - see build_errors.txt & exit /b 1)
cargo test 2> test_errors.txt
if errorlevel 1 (echo TESTS FAILED - see test_errors.txt & exit /b 1)
echo ALL OK
