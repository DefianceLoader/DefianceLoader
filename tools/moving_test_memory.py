"""Executable scratch memory and field writers for the moving-actions host tests.

The fake chassis, weapon and grenade objects the tests build live in
read-write-execute pages, so a plugin's detour can run against them and a
Python callback can stand in for a trampoline. Every page stays in KEEP for
the life of the process.
"""
import ctypes as C
from ctypes import wintypes as W

K = C.WinDLL("kernel32", use_last_error=True)
K.LoadLibraryExW.argtypes = [W.LPCWSTR, C.c_void_p, W.DWORD]
K.LoadLibraryExW.restype = C.c_void_p
K.FreeLibrary.argtypes = [C.c_void_p]
K.VirtualAlloc.argtypes = [C.c_void_p, C.c_size_t, W.DWORD, W.DWORD]
K.VirtualAlloc.restype = C.c_void_p
KEEP = []


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def alloc(size):
    p = K.VirtualAlloc(None, max(size, 8), 0x3000, 0x40)
    if not p:
        raise OSError(C.get_last_error())
    KEEP.append(p)
    C.memset(p, 0, size)
    return p


def p64(address, offset, value):
    C.c_uint64.from_address(address + offset).value = value


def i32(address, offset, value):
    C.c_int32.from_address(address + offset).value = value


def f32(address, offset, value):
    C.c_float.from_address(address + offset).value = value
