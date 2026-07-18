#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include <locale.h>

#ifdef _WIN32
#include <windows.h>
#define PATH_SEP L'\\'
#else
#include <limits.h>
#include <libgen.h>
#include <mach-o/dyld.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#define PATH_SEP '/'
#endif

#include <stdio.h>

static int append_path(wchar_t *destination, size_t capacity, const wchar_t *part) {
    size_t length = wcslen(destination);
    size_t part_length = wcslen(part);
    if (length + part_length + 2 > capacity) return 0;
    if (length && destination[length - 1] != PATH_SEP) destination[length++] = PATH_SEP;
    wmemcpy(destination + length, part, part_length + 1);
    return 1;
}

static int parent_directory(wchar_t *path) {
    wchar_t *separator = wcsrchr(path, PATH_SEP);
    if (!separator) return 0;
    *separator = L'\0';
    return 1;
}

static int executable_path(wchar_t *destination, size_t capacity) {
#ifdef _WIN32
    DWORD written = GetModuleFileNameW(NULL, destination, (DWORD)capacity);
    return written > 0 && written < capacity;
#else
    char narrow[PATH_MAX];
    uint32_t size = sizeof(narrow);
    if (_NSGetExecutablePath(narrow, &size) != 0) return 0;
    char resolved[PATH_MAX];
    if (!realpath(narrow, resolved)) return 0;
    return mbstowcs(destination, resolved, capacity) != (size_t)-1;
#endif
}

#ifdef _WIN32
int wmain(int argc, wchar_t **argv) {
#else
int main(int argc, char **argv) {
#endif
    setlocale(LC_ALL, "");
    wchar_t program[4096];
    if (!executable_path(program, sizeof(program) / sizeof(program[0]))) {
        fputs("lantern-ocr: cannot locate runtime root\n", stderr);
        return 1;
    }
    wchar_t root[4096];
    wcscpy(root, program);
    if (!parent_directory(root)) {
        fputs("lantern-ocr: cannot locate runtime root\n", stderr);
        return 1;
    }
    if (!parent_directory(root)) {
        fputs("lantern-ocr: invalid runtime layout\n", stderr);
        return 1;
    }

    wchar_t home[4096];
    wchar_t script[4096];
    wcscpy(home, root);
    wcscpy(script, root);
    if (!append_path(home, 4096, L"python") || !append_path(script, 4096, L"lib") ||
        !append_path(script, 4096, L"lantern_ocr.py")) {
        fputs("lantern-ocr: runtime path is too long\n", stderr);
        return 1;
    }

    PyConfig config;
    PyConfig_InitPythonConfig(&config);
    config.isolated = 1;
    config.use_environment = 0;
    config.site_import = 1;
    config.parse_argv = 0;
    PyStatus status = PyConfig_SetString(&config, &config.home, home);
    if (PyStatus_Exception(status)) goto fail;
    status = PyConfig_SetString(&config, &config.program_name, program);
    if (PyStatus_Exception(status)) goto fail;

    wchar_t **wide_argv = PyMem_RawCalloc((size_t)argc + 1, sizeof(wchar_t *));
    if (!wide_argv) {
        fputs("lantern-ocr: out of memory\n", stderr);
        PyConfig_Clear(&config);
        return 1;
    }
    wide_argv[0] = script;
    for (int index = 1; index < argc; ++index) {
#ifdef _WIN32
        size_t length = wcslen(argv[index]);
        wide_argv[index] = PyMem_RawMalloc((length + 1) * sizeof(wchar_t));
        if (wide_argv[index]) wmemcpy(wide_argv[index], argv[index], length + 1);
#else
        wide_argv[index] = Py_DecodeLocale(argv[index], NULL);
#endif
        if (!wide_argv[index]) {
            fputs("lantern-ocr: cannot decode argument\n", stderr);
            for (int release = 1; release < index; ++release) PyMem_RawFree(wide_argv[release]);
            PyMem_RawFree(wide_argv);
            PyConfig_Clear(&config);
            return 1;
        }
    }
    status = PyConfig_SetArgv(&config, argc, wide_argv);
    if (!PyStatus_Exception(status)) status = Py_InitializeFromConfig(&config);
    for (int index = 1; index < argc; ++index) PyMem_RawFree(wide_argv[index]);
    PyMem_RawFree(wide_argv);
    PyConfig_Clear(&config);
    if (PyStatus_Exception(status)) {
        Py_ExitStatusException(status);
    }

    char *encoded_script = Py_EncodeLocale(script, NULL);
    if (!encoded_script) {
        fputs("lantern-ocr: cannot encode entrypoint path\n", stderr);
        Py_FinalizeEx();
        return 1;
    }
#ifdef _WIN32
    FILE *file = _wfopen(script, L"rb");
#else
    char narrow_script[4096];
    if (wcstombs(narrow_script, script, sizeof(narrow_script)) == (size_t)-1) {
        fputs("lantern-ocr: cannot encode entrypoint path\n", stderr);
        PyMem_RawFree(encoded_script);
        Py_FinalizeEx();
        return 1;
    }
    FILE *file = fopen(narrow_script, "rb");
#endif
    if (!file) {
        fputs("lantern-ocr: cannot open entrypoint\n", stderr);
        PyMem_RawFree(encoded_script);
        Py_FinalizeEx();
        return 1;
    }
    int result = PyRun_SimpleFileExFlags(file, encoded_script, 1, NULL);
    PyMem_RawFree(encoded_script);
    int finalized = Py_FinalizeEx();
    return result == 0 && finalized == 0 ? 0 : 1;

fail:
    PyConfig_Clear(&config);
    Py_ExitStatusException(status);
}
