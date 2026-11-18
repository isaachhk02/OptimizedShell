// startup.h - programas de inicio de Windows (claves Run y carpetas Inicio).
// Compartido por el shell (los ejecuta al iniciar sesión cuando es el shell del
// sistema) y por el Administrador de tareas (pestaña Inicio).
// El estado habilitado/deshabilitado usa las mismas claves StartupApproved que
// Windows, así que es compatible con su Administrador de tareas.
#pragma once
#include "ui.h"

namespace startup {

using namespace ui;

inline const wchar_t* kRunKey = L"Software\\Microsoft\\Windows\\CurrentVersion\\Run";
inline const wchar_t* kRunOnceKey = L"Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce";
inline const wchar_t* kApprovedBase = L"Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\";

struct Entry {
    std::wstring name;        // nombre del valor o del archivo (sin .lnk)
    std::wstring command;     // línea de comandos (registro) o ruta del archivo (carpeta)
    std::wstring location;    // texto para mostrar: "Registro (usuario)", ...
    bool fromFolder = false;
    bool enabled = true;
    HKEY approvedRoot = nullptr;
    std::wstring approvedKey;   // ...\StartupApproved\Run | Run32 | StartupFolder
    std::wstring approvedValue; // nombre del valor dentro de approvedKey
};

inline bool ReadApproved(HKEY root, const std::wstring& key, const std::wstring& value) {
    BYTE data[16] = {};
    DWORD size = sizeof(data);
    LSTATUS st = RegGetValueW(root, key.c_str(), value.c_str(), RRF_RT_REG_BINARY, nullptr, data, &size);
    if (st != ERROR_SUCCESS || size == 0) return true; // sin entrada = habilitado
    return (data[0] & 1) == 0;                        // 02/06 habilitado, 03/07 deshabilitado
}

inline void EnumRegistry(std::vector<Entry>& out, HKEY root, REGSAM view, const wchar_t* approvedSub,
                         const wchar_t* location) {
    HKEY k = nullptr;
    if (RegOpenKeyExW(root, kRunKey, 0, KEY_READ | view, &k) != ERROR_SUCCESS) return;
    for (DWORD i = 0;; ++i) {
        wchar_t name[512];
        DWORD nameLen = 512;
        DWORD type = 0;
        DWORD dataLen = 0;
        LSTATUS st = RegEnumValueW(k, i, name, &nameLen, nullptr, &type, nullptr, &dataLen);
        if (st == ERROR_NO_MORE_ITEMS) break;
        if (st != ERROR_SUCCESS || (type != REG_SZ && type != REG_EXPAND_SZ) || nameLen == 0) continue;
        std::vector<wchar_t> data(dataLen / sizeof(wchar_t) + 2, L'\0');
        DWORD cb = (DWORD)(data.size() * sizeof(wchar_t));
        nameLen = 512;
        if (RegEnumValueW(k, i, name, &nameLen, nullptr, &type, (BYTE*)data.data(), &cb) != ERROR_SUCCESS) continue;
        Entry e;
        e.name = name;
        e.command = data.data();
        e.location = location;
        e.approvedRoot = root;
        e.approvedKey = std::wstring(kApprovedBase) + approvedSub;
        e.approvedValue = name;
        e.enabled = ReadApproved(root, e.approvedKey, e.approvedValue);
        out.push_back(e);
    }
    RegCloseKey(k);
}

inline void EnumFolder(std::vector<Entry>& out, int csidl, HKEY approvedRoot, const wchar_t* location) {
    wchar_t dir[MAX_PATH] = {};
    if (SHGetFolderPathW(nullptr, csidl, nullptr, 0, dir) != S_OK) return;
    WIN32_FIND_DATAW fd;
    HANDLE h = FindFirstFileExW((std::wstring(dir) + L"\\*").c_str(), FindExInfoBasic, &fd,
                                FindExSearchNameMatch, nullptr, 0);
    if (h == INVALID_HANDLE_VALUE) return;
    do {
        if (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) continue;
        if (_wcsicmp(fd.cFileName, L"desktop.ini") == 0) continue;
        Entry e;
        e.name = fd.cFileName;
        if (EndsWithNoCase(e.name, L".lnk")) e.name.resize(e.name.size() - 4);
        e.command = std::wstring(dir) + L"\\" + fd.cFileName;
        e.location = location;
        e.fromFolder = true;
        e.approvedRoot = approvedRoot;
        e.approvedKey = std::wstring(kApprovedBase) + L"StartupFolder";
        e.approvedValue = fd.cFileName;
        e.enabled = ReadApproved(approvedRoot, e.approvedKey, e.approvedValue);
        out.push_back(e);
    } while (FindNextFileW(h, &fd));
    FindClose(h);
}

inline std::vector<Entry> Enumerate() {
    std::vector<Entry> out;
    EnumRegistry(out, HKEY_CURRENT_USER, 0, L"Run", L"Registro (usuario)");
    EnumRegistry(out, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, L"Run", L"Registro (equipo)");
    EnumRegistry(out, HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, L"Run32", L"Registro (equipo, 32 bits)");
    EnumFolder(out, CSIDL_STARTUP, HKEY_CURRENT_USER, L"Carpeta Inicio (usuario)");
    EnumFolder(out, CSIDL_COMMON_STARTUP, HKEY_LOCAL_MACHINE, L"Carpeta Inicio (todos)");
    return out;
}

// Devuelve ERROR_SUCCESS o el código de error (ERROR_ACCESS_DENIED si hace falta administrador).
inline LSTATUS SetEnabled(const Entry& e, bool enable) {
    BYTE data[12] = {};
    data[0] = enable ? 0x02 : 0x03;
    if (!enable) {
        FILETIME ft;
        GetSystemTimeAsFileTime(&ft);
        memcpy(data + 4, &ft, sizeof(ft));
    }
    HKEY k = nullptr;
    LSTATUS st = RegCreateKeyExW(e.approvedRoot, e.approvedKey.c_str(), 0, nullptr, 0,
                                 KEY_SET_VALUE, nullptr, &k, nullptr);
    if (st != ERROR_SUCCESS) return st;
    st = RegSetValueExW(k, e.approvedValue.c_str(), 0, REG_BINARY, data, sizeof(data));
    RegCloseKey(k);
    return st;
}

// Ejecutable al que apunta la entrada (para icono y "Abrir ubicación").
inline std::wstring TargetPath(const Entry& e) {
    if (e.fromFolder) return e.command;
    std::wstring cmd = ExpandEnv(e.command);
    std::vector<wchar_t> buf(cmd.begin(), cmd.end());
    buf.resize(cmd.size() + MAX_PATH, L'\0');
    PathRemoveArgsW(buf.data());
    PathUnquoteSpacesW(buf.data());
    return buf.data();
}

inline void RunCommand(const std::wstring& command) {
    std::wstring cmd = ExpandEnv(command);
    std::vector<wchar_t> line(cmd.begin(), cmd.end());
    line.push_back(L'\0');
    STARTUPINFOW si = { sizeof(si) };
    PROCESS_INFORMATION pi = {};
    if (CreateProcessW(nullptr, line.data(), nullptr, nullptr, FALSE, 0, nullptr, nullptr, &si, &pi)) {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        return;
    }
    RunCommandLine(cmd);
}

// Ejecuta RunOnce del usuario (borrando cada valor antes, como Windows) y luego
// todas las entradas habilitadas. Pensado para un hilo de trabajo con COM.
inline void RunAll() {
    HKEY k = nullptr;
    if (RegOpenKeyExW(HKEY_CURRENT_USER, kRunOnceKey, 0, KEY_READ | KEY_SET_VALUE, &k) == ERROR_SUCCESS) {
        std::vector<std::pair<std::wstring, std::wstring>> once;
        for (DWORD i = 0;; ++i) {
            wchar_t name[512];
            wchar_t data[2048];
            DWORD nameLen = 512, cb = sizeof(data), type = 0;
            LSTATUS st = RegEnumValueW(k, i, name, &nameLen, nullptr, &type, (BYTE*)data, &cb);
            if (st == ERROR_NO_MORE_ITEMS) break;
            if (st == ERROR_SUCCESS && (type == REG_SZ || type == REG_EXPAND_SZ)) once.push_back({ name, data });
        }
        for (auto& o : once) {
            bool keep = !o.first.empty() && o.first[0] == L'!'; // "!" = borrar tras ejecutar
            if (!keep) RegDeleteValueW(k, o.first.c_str());
            RunCommand(o.second);
            if (keep) RegDeleteValueW(k, o.first.c_str());
        }
        RegCloseKey(k);
    }
    for (const Entry& e : Enumerate()) {
        if (!e.enabled) continue;
        if (e.fromFolder) Launch(e.command);
        else RunCommand(e.command);
        Sleep(150); // escalonar para no saturar el disco al iniciar sesión
    }
}

} // namespace startup
