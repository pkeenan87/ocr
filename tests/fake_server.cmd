@echo off
rem Windows wrapper: runs tests\fake_server.py with the arguments given (PLAN 9, Phase 7f).
python "%~dp0fake_server.py" %*
