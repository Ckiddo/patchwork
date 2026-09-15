@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0local\Stop-LocalTest.ps1" %*
set "test_exit=%errorlevel%"
if not "%test_exit%"=="0" pause
exit /b %test_exit%
