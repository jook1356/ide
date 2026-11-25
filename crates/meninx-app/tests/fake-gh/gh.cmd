@echo off
rem Fake `gh` for the app script (see gh.js). Node from PATH, else the default install.
setlocal
set "NODE=node"
where node >nul 2>nul || set "NODE=%ProgramFiles%\nodejs\node.exe"
"%NODE%" "%~dp0gh.js" %*
exit /b %ERRORLEVEL%
