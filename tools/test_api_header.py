"""Compile the public header/layout assertions as both C17 and C++17 (Windows x64)."""
import os
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main():
    program_files = pathlib.Path(os.environ["ProgramFiles(x86)"])
    vswhere = program_files / "Microsoft Visual Studio/Installer/vswhere.exe"
    found = subprocess.check_output([str(vswhere), "-latest", "-products", "*",
        "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
        "-find", r"VC\Tools\MSVC\**\bin\Hostx64\x64\cl.exe"], text=True).strip().splitlines()
    if not found:
        raise SystemExit("MSVC x64 compiler not found")
    compiler = pathlib.Path(found[-1])
    includes = compiler.parents[3] / "include"
    sdk = sorted((program_files / "Windows Kits/10/Include").glob("*/ucrt"))[-1]
    for language, standard in [("/TC", "/std:c17"), ("/TP", "/std:c++17")]:
        subprocess.run([str(compiler), "/nologo", "/Zs", language, standard,
            "/I" + str(includes), "/I" + str(sdk), str(ROOT / "tools/test_api_header.c")], check=True)
    print("PASS public header: C17/C++17 declarations and ABI layout")


if __name__ == "__main__":
    main()
