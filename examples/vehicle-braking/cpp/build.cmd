@echo off
setlocal
set "SOURCE_DIR=%~dp0"
set "OUTPUT_DIR=%SOURCE_DIR%target"
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"

if not exist "%VSWHERE%" (
    echo Visual Studio Build Tools were not found. Install the Desktop development with C++ workload. 1>&2
    exit /b 1
)
set "VS_INSTALL="
for /f "usebackq delims=" %%I in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VS_INSTALL=%%I"
if not defined VS_INSTALL (
    echo The Visual Studio C++ x64 tools were not found. Install the Desktop development with C++ workload. 1>&2
    exit /b 1
)
if not exist "%VS_INSTALL%\VC\Auxiliary\Build\vcvars64.bat" (
    echo The Visual Studio x64 build environment is missing. Repair the C++ Build Tools installation. 1>&2
    exit /b 1
)
call "%VS_INSTALL%\VC\Auxiliary\Build\vcvars64.bat" >nul
if errorlevel 1 exit /b 1

if not exist "%OUTPUT_DIR%" mkdir "%OUTPUT_DIR%"
if not exist "%OUTPUT_DIR%" exit /b 1

cl /nologo /std:c++17 /permissive- /EHsc /W4 /WX /O2 /MT /Z7 /LD ^
    /I "%SOURCE_DIR%..\..\..\crates\api\include" ^
    /Fo"%OUTPUT_DIR%\main.obj" "%SOURCE_DIR%main.cpp" ^
    /link /MACHINE:X64 /INCREMENTAL:NO /DEBUG /OPT:REF /OPT:ICF ^
    /OUT:"%OUTPUT_DIR%\defiance_example_vehicle_braking_cpp.dll" ^
    /IMPLIB:"%OUTPUT_DIR%\defiance_example_vehicle_braking_cpp.lib" ^
    /PDB:"%OUTPUT_DIR%\defiance_example_vehicle_braking_cpp.pdb"
if errorlevel 1 exit /b 1

copy /Y "%SOURCE_DIR%defiance_example_vehicle_braking_cpp.plugin.json" "%OUTPUT_DIR%\defiance_example_vehicle_braking_cpp.plugin.json" >nul
if errorlevel 1 exit /b 1
echo Built "%OUTPUT_DIR%\defiance_example_vehicle_braking_cpp.dll" and its manifest.
exit /b 0
