// ui.h - utilidades compartidas de UI (Win32 + GDI+) para OptimizedShell.
// Ventanas con doble buffer (DIB section + repintado parcial), blur acrílico de DWM,
// dibujo vectorial/texto con recursos en caché, iconos del sistema y utilidades
// para reducir el consumo del propio proceso.
#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <windows.h>
#include <windowsx.h>
#include <objidl.h>
#include <gdiplus.h>
#include <shellapi.h>
#include <shlobj.h>
#include <shlwapi.h>
#include <commctrl.h>
#include <commoncontrols.h>
#include <uxtheme.h>
#include <dwmapi.h>
#include <psapi.h>
#include <string>
#include <vector>
#include <unordered_map>
#include <algorithm>
#include <cmath>
#include <cwctype>

#pragma comment(lib, "user32.lib")
#pragma comment(lib, "gdi32.lib")
#pragma comment(lib, "gdiplus.lib")
#pragma comment(lib, "shell32.lib")
#pragma comment(lib, "ole32.lib")
#pragma comment(lib, "shlwapi.lib")
#pragma comment(lib, "dwmapi.lib")
#pragma comment(lib, "advapi32.lib")
#pragma comment(lib, "comctl32.lib")
#pragma comment(lib, "uxtheme.lib")
#pragma comment(lib, "psapi.lib")
// Common Controls v6 (EDIT con tema oscuro, cue banners, tooltips con tema).
#pragma comment(linker, "\"/manifestdependency:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'\"")

namespace ui {

using namespace Gdiplus;

inline HINSTANCE g_hInst = nullptr;
inline int g_dpi = 96;
inline ULONG_PTR g_gdiToken = 0;

inline Font* fontBody = nullptr;
inline Font* fontBold = nullptr;
inline Font* fontSmall = nullptr;
inline Font* fontSmallBold = nullptr;
inline Font* fontTitle = nullptr;
inline Font* fontBig = nullptr;
inline Font* fontHuge = nullptr;
inline Font* fontAvatar = nullptr;
inline Font* fontIcon = nullptr;

// Recursos GDI para controles EDIT nativos (barra de direcciones, búsqueda, renombrar).
inline HFONT hfontBody = nullptr;
inline HBRUSH hbrEdit = nullptr;
inline const COLORREF kEditBg = RGB(35, 40, 52);
inline const COLORREF kEditFg = RGB(232, 236, 244);

inline int S(int v) { return MulDiv(v, g_dpi, 96); }
inline float SF(float v) { return v * (float)g_dpi / 96.0f; }
inline Color C(BYTE a, BYTE r, BYTE g, BYTE b) { return Color(a, r, g, b); }
inline Color RGBc(BYTE r, BYTE g, BYTE b) { return Color(255, r, g, b); }

// ------------------------------------------------------------ inicialización
inline void InitUi(HINSTANCE inst) {
    g_hInst = inst;
    SetProcessDPIAware();
    HDC dc = GetDC(nullptr);
    if (dc) {
        g_dpi = GetDeviceCaps(dc, LOGPIXELSX);
        ReleaseDC(nullptr, dc);
    }
    if (g_dpi <= 0) g_dpi = 96;

    GdiplusStartupInput gi;
    GdiplusStartup(&g_gdiToken, &gi, nullptr);

    fontBody = new Font(L"Segoe UI", SF(14.0f), FontStyleRegular, UnitPixel);
    fontBold = new Font(L"Segoe UI", SF(14.0f), FontStyleBold, UnitPixel);
    fontSmall = new Font(L"Segoe UI", SF(11.5f), FontStyleRegular, UnitPixel);
    fontSmallBold = new Font(L"Segoe UI", SF(11.5f), FontStyleBold, UnitPixel);
    fontTitle = new Font(L"Segoe UI", SF(19.0f), FontStyleBold, UnitPixel);
    fontBig = new Font(L"Segoe UI", SF(28.0f), FontStyleBold, UnitPixel);
    fontHuge = new Font(L"Segoe UI Light", SF(64.0f), FontStyleRegular, UnitPixel);
    fontAvatar = new Font(L"Segoe UI", SF(34.0f), FontStyleBold, UnitPixel);
    fontIcon = new Font(L"Segoe MDL2 Assets", SF(13.0f), FontStyleRegular, UnitPixel);

    LOGFONTW lf = {};
    lf.lfHeight = -S(14);
    lf.lfWeight = FW_NORMAL;
    lf.lfQuality = CLEARTYPE_QUALITY;
    wcscpy_s(lf.lfFaceName, L"Segoe UI");
    hfontBody = CreateFontIndirectW(&lf);
    hbrEdit = CreateSolidBrush(kEditBg);

    INITCOMMONCONTROLSEX icc = { sizeof(icc), ICC_STANDARD_CLASSES | ICC_BAR_CLASSES };
    InitCommonControlsEx(&icc);

    // Menús contextuales en tema oscuro (Windows 10 1809+).
    HMODULE ux = LoadLibraryW(L"uxtheme.dll");
    if (ux) {
        typedef int(WINAPI* PFN_SetPreferredAppMode)(int);
        auto setMode = (PFN_SetPreferredAppMode)GetProcAddress(ux, MAKEINTRESOURCEA(135));
        if (setMode) setMode(2); // ForceDark
    }
}

inline void ShutdownUi() {
    delete fontBody; fontBody = nullptr;
    delete fontBold; fontBold = nullptr;
    delete fontSmall; fontSmall = nullptr;
    delete fontSmallBold; fontSmallBold = nullptr;
    delete fontTitle; fontTitle = nullptr;
    delete fontBig; fontBig = nullptr;
    delete fontHuge; fontHuge = nullptr;
    delete fontAvatar; fontAvatar = nullptr;
    delete fontIcon; fontIcon = nullptr;
    if (hfontBody) { DeleteObject(hfontBody); hfontBody = nullptr; }
    if (hbrEdit) { DeleteObject(hbrEdit); hbrEdit = nullptr; }
    // Los StringFormat y bitmaps en caché se liberan con el proceso: no se
    // destruyen tras GdiplusShutdown para evitar llamadas sobre GDI+ apagado.
    if (g_gdiToken) {
        GdiplusShutdown(g_gdiToken);
        g_gdiToken = 0;
    }
}

// ------------------------------------------------------------ DWM
typedef enum _ACCENT_STATE {
    ACCENT_DISABLED = 0,
    ACCENT_ENABLE_GRADIENT = 1,
    ACCENT_ENABLE_TRANSPARENTGRADIENT = 2,
    ACCENT_ENABLE_BLURBEHIND = 3,
    ACCENT_ENABLE_ACRYLICBLURBEHIND = 4,
} ACCENT_STATE;

typedef struct _ACCENT_POLICY {
    ACCENT_STATE AccentState;
    DWORD AccentFlags;
    DWORD GradientColor;
    DWORD AnimationId;
} ACCENT_POLICY;

typedef struct _WINDOWCOMPOSITIONATTRIBDATA {
    DWORD Attribute;
    PVOID Data;
    SIZE_T SizeOfData;
} WINDOWCOMPOSITIONATTRIBDATA;

typedef BOOL(WINAPI* PFN_SetWindowCompositionAttribute)(HWND, WINDOWCOMPOSITIONATTRIBDATA*);

// Marco oscuro con esquinas redondeadas, sin blur (ventanas opacas con controles hijos).
inline void EnableDarkFrame(HWND hwnd) {
    BOOL dark = TRUE;
    DwmSetWindowAttribute(hwnd, 20, &dark, sizeof(dark)); // DWMWA_USE_IMMERSIVE_DARK_MODE
    int pref = 2;                                           // DWMWCP_ROUND
    DwmSetWindowAttribute(hwnd, 33, &pref, sizeof(pref));
}

inline void EnableBlur(HWND hwnd, DWORD gradient) {
    static PFN_SetWindowCompositionAttribute setAttr = nullptr;
    if (!setAttr) {
        HMODULE user32 = GetModuleHandleW(L"user32.dll");
        if (user32) setAttr = (PFN_SetWindowCompositionAttribute)GetProcAddress(user32, "SetWindowCompositionAttribute");
    }
    if (setAttr) {
        ACCENT_POLICY policy = {};
        policy.AccentState = ACCENT_ENABLE_ACRYLICBLURBEHIND;
        policy.AccentFlags = 2;
        policy.GradientColor = gradient;
        WINDOWCOMPOSITIONATTRIBDATA data = {};
        data.Attribute = 19; // WCA_ACCENT_POLICY
        data.Data = &policy;
        data.SizeOfData = sizeof(policy);
        setAttr(hwnd, &data);
    }
    EnableDarkFrame(hwnd);
}

// ------------------------------------------------------------ eficiencia del proceso
// EcoQoS: Windows programa el proceso en núcleos/frecuencias de bajo consumo.
inline void SetProcessEco(bool on) {
    PROCESS_POWER_THROTTLING_STATE st = {};
    st.Version = PROCESS_POWER_THROTTLING_CURRENT_VERSION;
    st.ControlMask = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
    st.StateMask = on ? PROCESS_POWER_THROTTLING_EXECUTION_SPEED : 0;
    SetProcessInformation(GetCurrentProcess(), ProcessPowerThrottling, &st, sizeof(st));
}

inline void SetThreadEco(HANDLE thread = GetCurrentThread()) {
    THREAD_POWER_THROTTLING_STATE st = {};
    st.Version = THREAD_POWER_THROTTLING_CURRENT_VERSION;
    st.ControlMask = THREAD_POWER_THROTTLING_EXECUTION_SPEED;
    st.StateMask = THREAD_POWER_THROTTLING_EXECUTION_SPEED;
    SetThreadInformation(thread, ThreadPowerThrottling, &st, sizeof(st));
    SetThreadPriority(thread, THREAD_PRIORITY_BELOW_NORMAL);
}

// Devuelve al sistema las páginas que el proceso no está usando ahora mismo.
inline void TrimWorkingSet() {
    SetProcessWorkingSetSizeEx(GetCurrentProcess(), (SIZE_T)-1, (SIZE_T)-1, 0);
}

// ------------------------------------------------------------ helpers
inline std::wstring RegString(HKEY root, const wchar_t* sub, const wchar_t* name) {
    wchar_t buf[512] = {};
    DWORD size = sizeof(buf);
    if (RegGetValueW(root, sub, name, RRF_RT_REG_SZ, nullptr, buf, &size) == ERROR_SUCCESS) return buf;
    return L"";
}

inline std::wstring GetWallpaperPath() {
    wchar_t buf[MAX_PATH] = {};
    if (SystemParametersInfoW(SPI_GETDESKWALLPAPER, MAX_PATH, buf, 0)) return buf;
    return L"";
}

inline std::wstring GetOwnPath() {
    wchar_t path[MAX_PATH] = {};
    GetModuleFileNameW(nullptr, path, MAX_PATH);
    return path;
}

inline std::wstring GetOwnDir() {
    std::wstring path = GetOwnPath();
    size_t slash = path.find_last_of(L'\\');
    if (slash != std::wstring::npos) path.resize(slash);
    return path;
}

inline void Launch(const std::wstring& target, const std::wstring& args = L"") {
    if (target.empty()) return;
    ShellExecuteW(nullptr, L"open", target.c_str(), args.empty() ? nullptr : args.c_str(), nullptr, SW_SHOWNORMAL);
}

inline void LaunchOwn(const std::wstring& args) {
    Launch(GetOwnPath(), args);
}

inline std::wstring Quote(const std::wstring& s) { return L"\"" + s + L"\""; }

// Ejecuta lo escrito en "Ejecutar"/"Nueva tarea": ruta, URL, comando con argumentos...
inline bool RunCommandLine(const std::wstring& input, HWND owner = nullptr) {
    std::wstring cmd;
    {
        size_t a = input.find_first_not_of(L" \t");
        if (a == std::wstring::npos) return false;
        size_t b = input.find_last_not_of(L" \t");
        cmd = input.substr(a, b - a + 1);
    }
    wchar_t expanded[2048] = {};
    if (ExpandEnvironmentStringsW(cmd.c_str(), expanded, 2048)) cmd = expanded;

    SHELLEXECUTEINFOW sei = { sizeof(sei) };
    sei.fMask = SEE_MASK_FLAG_NO_UI;
    sei.hwnd = owner;
    sei.lpVerb = L"open";
    sei.lpFile = cmd.c_str();
    sei.nShow = SW_SHOWNORMAL;
    if (ShellExecuteExW(&sei)) return true;

    // Separar ejecutable y argumentos: "notepad C:\a.txt", "\"C:\x y\app.exe\" -n".
    std::vector<wchar_t> file(cmd.begin(), cmd.end());
    file.resize(cmd.size() + MAX_PATH, L'\0');
    PathRemoveArgsW(file.data());
    PathUnquoteSpacesW(file.data());
    const wchar_t* args = PathGetArgsW(cmd.c_str());
    SHELLEXECUTEINFOW sei2 = { sizeof(sei2) };
    sei2.hwnd = owner;
    sei2.lpVerb = L"open";
    sei2.lpFile = file.data();
    sei2.lpParameters = (args && *args) ? args : nullptr;
    sei2.nShow = SW_SHOWNORMAL;
    return ShellExecuteExW(&sei2) != FALSE;
}

// Trae una ventana propia al frente aunque otro proceso tenga el foco
// (p. ej. al abrir el menú Inicio con la tecla Windows desde un hook).
inline void ForceForeground(HWND hwnd) {
    HWND fg = GetForegroundWindow();
    DWORD fgThread = fg ? GetWindowThreadProcessId(fg, nullptr) : 0;
    DWORD me = GetCurrentThreadId();
    if (fgThread && fgThread != me && AttachThreadInput(me, fgThread, TRUE)) {
        SetForegroundWindow(hwnd);
        BringWindowToTop(hwnd);
        AttachThreadInput(me, fgThread, FALSE);
    } else {
        SetForegroundWindow(hwnd);
    }
}

// Abre una ruta en el explorador propio (o selecciona un archivo dentro de su carpeta).
inline void OpenInExplorer(const std::wstring& path, bool select = false) {
    LaunchOwn(std::wstring(select ? L"--explorer --select " : L"--explorer ") + Quote(path));
}

inline std::wstring FormatBytes(ULONGLONG bytes) {
    wchar_t buf[64] = {};
    if (bytes >= 1024ull * 1024ull * 1024ull)
        swprintf_s(buf, L"%.1f GB", (double)bytes / (1024.0 * 1024.0 * 1024.0));
    else if (bytes >= 1024ull * 1024ull)
        swprintf_s(buf, L"%.1f MB", (double)bytes / (1024.0 * 1024.0));
    else if (bytes >= 1024ull)
        swprintf_s(buf, L"%.0f KB", (double)bytes / 1024.0);
    else
        swprintf_s(buf, L"%llu B", bytes);
    return buf;
}

inline ULONGLONG FtToU64(const FILETIME& ft) {
    ULARGE_INTEGER u;
    u.LowPart = ft.dwLowDateTime;
    u.HighPart = ft.dwHighDateTime;
    return u.QuadPart;
}

inline std::wstring ToLower(std::wstring s) {
    for (auto& c : s) c = (wchar_t)towlower(c);
    return s;
}

inline std::wstring Trim(const std::wstring& s) {
    size_t a = s.find_first_not_of(L" \t\r\n");
    if (a == std::wstring::npos) return L"";
    size_t b = s.find_last_not_of(L" \t\r\n");
    return s.substr(a, b - a + 1);
}

inline bool ContainsNoCase(const std::wstring& hay, const std::wstring& lowerNeedle) {
    if (lowerNeedle.empty()) return true;
    return ToLower(hay).find(lowerNeedle) != std::wstring::npos;
}

inline bool EndsWithNoCase(const std::wstring& s, const std::wstring& suffix) {
    if (s.size() < suffix.size()) return false;
    return _wcsicmp(s.c_str() + (s.size() - suffix.size()), suffix.c_str()) == 0;
}

inline std::wstring BaseName(const std::wstring& path) {
    size_t slash = path.find_last_of(L"\\/");
    return slash == std::wstring::npos ? path : path.substr(slash + 1);
}

inline std::wstring ParentDir(const std::wstring& path) {
    size_t slash = path.find_last_of(L"\\/");
    if (slash == std::wstring::npos) return L"";
    if (slash == 2 && path.size() > 1 && path[1] == L':') return path.substr(0, 3);
    return path.substr(0, slash);
}

inline std::wstring ExpandEnv(const std::wstring& s) {
    wchar_t buf[2048] = {};
    DWORD n = ExpandEnvironmentStringsW(s.c_str(), buf, 2048);
    return (n && n <= 2048) ? std::wstring(buf) : s;
}

inline bool HasArg(const wchar_t* arg) {
    int argc = 0;
    LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    bool found = false;
    for (int i = 0; argv && i < argc; ++i)
        if (_wcsicmp(argv[i], arg) == 0) { found = true; break; }
    if (argv) LocalFree(argv);
    return found;
}

// Valor que sigue a un argumento (p. ej. --tab services) o cadena vacía.
inline std::wstring ArgValue(const wchar_t* arg) {
    int argc = 0;
    LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    std::wstring out;
    for (int i = 0; argv && i + 1 < argc; ++i)
        if (_wcsicmp(argv[i], arg) == 0) { out = argv[i + 1]; break; }
    if (argv) LocalFree(argv);
    return out;
}

inline bool CopyTextToClipboard(HWND owner, const std::wstring& text) {
    if (!OpenClipboard(owner)) return false;
    EmptyClipboard();
    size_t bytes = (text.size() + 1) * sizeof(wchar_t);
    HGLOBAL hg = GlobalAlloc(GMEM_MOVEABLE, bytes);
    bool ok = false;
    if (hg) {
        void* dst = GlobalLock(hg);
        if (dst) {
            memcpy(dst, text.c_str(), bytes);
            GlobalUnlock(hg);
            ok = SetClipboardData(CF_UNICODETEXT, hg) != nullptr;
        }
        if (!ok) GlobalFree(hg);
    }
    CloseClipboard();
    return ok;
}

inline bool IsElevated() {
    HANDLE token = nullptr;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) return false;
    TOKEN_ELEVATION el = {};
    DWORD sz = 0;
    bool r = GetTokenInformation(token, TokenElevation, &el, sizeof(el), &sz) && el.TokenIsElevated;
    CloseHandle(token);
    return r;
}

// ------------------------------------------------------------ controles EDIT oscuros
inline HWND CreateDarkEdit(HWND parent, int id, const wchar_t* cue) {
    HWND e = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL,
                             0, 0, 10, 10, parent, (HMENU)(INT_PTR)id, g_hInst, nullptr);
    SendMessageW(e, WM_SETFONT, (WPARAM)hfontBody, TRUE);
    SetWindowTheme(e, L"DarkMode_CFD", nullptr);
    if (cue) SendMessageW(e, EM_SETCUEBANNER, TRUE, (LPARAM)cue);
    SendMessageW(e, EM_SETMARGINS, EC_LEFTMARGIN | EC_RIGHTMARGIN, MAKELPARAM(S(2), S(2)));
    return e;
}

// Responde a WM_CTLCOLOREDIT con colores del tema oscuro.
inline LRESULT CtlColorEdit(WPARAM wp) {
    HDC dc = (HDC)wp;
    SetTextColor(dc, kEditFg);
    SetBkColor(dc, kEditBg);
    return (LRESULT)hbrEdit;
}

// Teclas especiales de un EDIT (flechas, Enter, Esc, RePág/AvPág, Tab) reenviadas al dueño.
struct EditKeySink {
    virtual bool OnEditKey(HWND edit, int vk) = 0; // true = consumida
    virtual ~EditKeySink() = default;
};

inline LRESULT CALLBACK EditKeySubclassProc(HWND h, UINT m, WPARAM wp, LPARAM lp, UINT_PTR, DWORD_PTR ref) {
    auto* sink = (EditKeySink*)ref;
    if (m == WM_KEYDOWN) {
        switch (wp) {
        case VK_UP: case VK_DOWN: case VK_RETURN: case VK_ESCAPE: case VK_PRIOR: case VK_NEXT: case VK_TAB:
            if (sink && sink->OnEditKey(h, (int)wp)) return 0;
            break;
        default: break;
        }
    } else if (m == WM_CHAR && (wp == VK_RETURN || wp == VK_ESCAPE || wp == VK_TAB)) {
        return 0; // evitar el pitido del EDIT
    } else if (m == WM_KILLFOCUS) {
        if (sink) sink->OnEditKey(h, 0); // 0 = pérdida de foco
    } else if (m == WM_NCDESTROY) {
        RemoveWindowSubclass(h, EditKeySubclassProc, 1);
    }
    return DefSubclassProc(h, m, wp, lp);
}

inline void HookEditKeys(HWND edit, EditKeySink* sink) {
    SetWindowSubclass(edit, EditKeySubclassProc, 1, (DWORD_PTR)sink);
}

inline std::wstring GetWindowString(HWND h) {
    int len = GetWindowTextLengthW(h);
    std::wstring s(len + 1, L'\0');
    GetWindowTextW(h, &s[0], len + 1);
    s.resize(len);
    return s;
}

// ------------------------------------------------------------ tooltip
// Un único tooltip por ventana; se reubica sobre el elemento bajo el ratón.
struct HoverTip {
    HWND tip = nullptr;
    HWND owner = nullptr;
    std::wstring text;

    void Create(HWND o) {
        owner = o;
        tip = CreateWindowExW(WS_EX_TOPMOST, TOOLTIPS_CLASSW, nullptr,
                              WS_POPUP | TTS_NOPREFIX | TTS_ALWAYSTIP,
                              CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT,
                              o, nullptr, g_hInst, nullptr);
        if (!tip) return;
        SetWindowTheme(tip, L"DarkMode_Explorer", nullptr);
        TTTOOLINFOW ti = { sizeof(ti) };
        ti.uFlags = TTF_SUBCLASS;
        ti.hwnd = o;
        ti.uId = 1;
        ti.lpszText = (LPWSTR)L"";
        SendMessageW(tip, TTM_ADDTOOLW, 0, (LPARAM)&ti);
        SendMessageW(tip, TTM_SETMAXTIPWIDTH, 0, MulDiv(420, g_dpi, 96));
    }

    void Set(const Gdiplus::RectF& r, const std::wstring& t) {
        if (!tip || t == text) return;
        text = t;
        TTTOOLINFOW ti = { sizeof(ti) };
        ti.hwnd = owner;
        ti.uId = 1;
        if (t.empty()) ti.rect = RECT{ 0, 0, 0, 0 };
        else ti.rect = RECT{ (LONG)r.X, (LONG)r.Y, (LONG)(r.X + r.Width), (LONG)(r.Y + r.Height) };
        ti.lpszText = (LPWSTR)text.c_str();
        SendMessageW(tip, TTM_POP, 0, 0);
        SendMessageW(tip, TTM_NEWTOOLRECTW, 0, (LPARAM)&ti);
        SendMessageW(tip, TTM_UPDATETIPTEXTW, 0, (LPARAM)&ti);
    }
};

// ------------------------------------------------------------ iconos
// Convierte un HICON a un Bitmap PARGB conservando el canal alfa
// (Bitmap::FromHICON lo pierde y deja bordes negros).
inline Bitmap* IconToBitmap(HICON icon, int size) {
    if (!icon || size <= 0) return nullptr;
    BITMAPINFO bi = {};
    bi.bmiHeader.biSize = sizeof(bi.bmiHeader);
    bi.bmiHeader.biWidth = size;
    bi.bmiHeader.biHeight = -size;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    HDC dc = CreateCompatibleDC(nullptr);
    void* bits = nullptr;
    HBITMAP dib = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &bits, nullptr, 0);
    if (!dib) { DeleteDC(dc); return nullptr; }
    HGDIOBJ old = SelectObject(dc, dib);
    DrawIconEx(dc, 0, 0, icon, size, size, 0, nullptr, DI_NORMAL);
    GdiFlush();
    DWORD* px = (DWORD*)bits;
    int n = size * size;
    bool hasAlpha = false;
    for (int i = 0; i < n; ++i) if (px[i] >> 24) { hasAlpha = true; break; }
    if (!hasAlpha) {
        // Icono antiguo sin alfa: la máscara AND indica qué píxeles son transparentes.
        void* mbits = nullptr;
        HDC mdc = CreateCompatibleDC(nullptr);
        HBITMAP mdib = CreateDIBSection(mdc, &bi, DIB_RGB_COLORS, &mbits, nullptr, 0);
        if (mdib) {
            HGDIOBJ mold = SelectObject(mdc, mdib);
            memset(mbits, 0xFF, (size_t)n * 4);
            DrawIconEx(mdc, 0, 0, icon, size, size, 0, nullptr, DI_MASK);
            GdiFlush();
            DWORD* m = (DWORD*)mbits;
            for (int i = 0; i < n; ++i) px[i] = (m[i] & 0xFFFFFF) ? 0 : (px[i] | 0xFF000000);
            SelectObject(mdc, mold);
            DeleteObject(mdib);
        } else {
            for (int i = 0; i < n; ++i) px[i] |= 0xFF000000;
        }
        DeleteDC(mdc);
    }
    Bitmap* bmp = new Bitmap(size, size, PixelFormat32bppPARGB);
    BitmapData bd = {};
    Rect r(0, 0, size, size);
    if (bmp->LockBits(&r, ImageLockModeWrite, PixelFormat32bppPARGB, &bd) == Ok) {
        for (int y = 0; y < size; ++y)
            memcpy((BYTE*)bd.Scan0 + (size_t)y * bd.Stride, px + (size_t)y * size, (size_t)size * 4);
        bmp->UnlockBits(&bd);
    }
    SelectObject(dc, old);
    DeleteObject(dib);
    DeleteDC(dc);
    return bmp;
}

// Índice en la lista de imágenes del sistema (se puede llamar desde hilos de trabajo
// con COM inicializado). useAttrs = consulta por extensión sin tocar el disco.
inline int SysIconIndex(const std::wstring& path, DWORD attrs = 0, bool useAttrs = false) {
    SHFILEINFOW sfi = {};
    UINT flags = SHGFI_SYSICONINDEX | SHGFI_SMALLICON | (useAttrs ? SHGFI_USEFILEATTRIBUTES : 0);
    HIMAGELIST h = (HIMAGELIST)SHGetFileInfoW(path.c_str(), attrs, &sfi, sizeof(sfi), flags);
    return h ? sfi.iIcon : -1;
}

// Bitmap (en caché, compartido) del icono del sistema 'index' a 'size' píxeles.
// Iconos idénticos comparten índice, así que la caché crece muy poco. Solo hilo de UI.
inline Bitmap* SysIconBitmap(int index, int size) {
    if (index < 0 || size <= 0) return nullptr;
    static std::unordered_map<long long, Bitmap*> cache;
    long long key = ((long long)size << 32) | (unsigned)index;
    auto it = cache.find(key);
    if (it != cache.end()) return it->second;
    int shil = size <= S(16) ? SHIL_SMALL : (size <= S(32) ? SHIL_LARGE : SHIL_EXTRALARGE);
    IImageList* il = nullptr;
    Bitmap* bmp = nullptr;
    if (SUCCEEDED(SHGetImageList(shil, IID_IImageList, (void**)&il)) && il) {
        HICON hi = nullptr;
        if (SUCCEEDED(il->GetIcon(index, ILD_TRANSPARENT, &hi)) && hi) {
            bmp = IconToBitmap(hi, size);
            DestroyIcon(hi);
        }
        il->Release();
    }
    cache[key] = bmp;
    return bmp;
}

// Conveniencia: icono de una ruta concreta (consulta el archivo).
inline Bitmap* FileIconBitmap(const std::wstring& path, int size) {
    return SysIconBitmap(SysIconIndex(path), size);
}

// ------------------------------------------------------------ dibujo
// Los StringFormat se crean una vez por combinación y se reutilizan.
inline StringFormat* CachedFormat(StringAlignment align, StringAlignment line, bool wrap) {
    static StringFormat* cache[3][3][2] = {};
    StringFormat*& f = cache[(int)align][(int)line][wrap ? 1 : 0];
    if (!f) {
        f = new StringFormat();
        f->SetAlignment(align);
        f->SetLineAlignment(line);
        f->SetTrimming(StringTrimmingEllipsisCharacter);
        if (!wrap) f->SetFormatFlags(StringFormatFlagsNoWrap);
    }
    return f;
}

inline void DrawTextStr(Graphics& g, const std::wstring& s, Font* f, const Color& color,
                        const RectF& rc, StringAlignment align = StringAlignmentNear,
                        bool wrap = false) {
    if (s.empty() || !f) return;
    SolidBrush brush(color);
    g.DrawString(s.c_str(), (INT)s.size(), f, rc, CachedFormat(align, StringAlignmentCenter, wrap), &brush);
}

inline void DrawTextV(Graphics& g, const std::wstring& s, Font* f, const Color& color,
                      const RectF& rc, StringAlignment align = StringAlignmentNear, bool wrap = false) {
    if (s.empty() || !f) return;
    SolidBrush brush(color);
    g.DrawString(s.c_str(), (INT)s.size(), f, rc, CachedFormat(align, StringAlignmentNear, wrap), &brush);
}

inline void DrawRounded(Graphics& g, const RectF& rc, float radius, const Color& fill,
                        const Color& border = Color(0, 0, 0, 0), float bw = 1.0f) {
    if (rc.Width <= 0 || rc.Height <= 0) return;
    GraphicsPath path;
    float d = radius * 2;
    if (d > rc.Width) d = rc.Width;
    if (d > rc.Height) d = rc.Height;
    if (d <= 0.5f) {
        path.AddRectangle(rc);
    } else {
        path.AddArc(rc.X, rc.Y, d, d, 180, 90);
        path.AddArc(rc.X + rc.Width - d, rc.Y, d, d, 270, 90);
        path.AddArc(rc.X + rc.Width - d, rc.Y + rc.Height - d, d, d, 0, 90);
        path.AddArc(rc.X, rc.Y + rc.Height - d, d, d, 90, 90);
        path.CloseFigure();
    }
    if (fill.GetA() > 0) {
        SolidBrush b(fill);
        g.FillPath(&b, &path);
    }
    if (border.GetA() > 0) {
        Pen p(border, bw);
        g.DrawPath(&p, &path);
    }
}

inline void FillRect(Graphics& g, const RectF& rc, const Color& c) {
    SolidBrush b(c);
    g.FillRectangle(&b, rc);
}

inline void HLine(Graphics& g, float x1, float x2, float y, const Color& c) {
    Pen p(c, 1.0f);
    g.DrawLine(&p, x1, y, x2, y);
}

inline void FillCircle(Graphics& g, float cx, float cy, float r, const Color& color) {
    SolidBrush b(color);
    g.FillEllipse(&b, cx - r, cy - r, r * 2, r * 2);
}

inline void DrawCircle(Graphics& g, float cx, float cy, float r, const Color& color, float w = 1.0f) {
    Pen p(color, w);
    g.DrawEllipse(&p, cx - r, cy - r, r * 2, r * 2);
}

inline void DrawCheck(Graphics& g, float cx, float cy, float size, const Color& color, float w) {
    Pen p(color, w);
    p.SetStartCap(LineCapRound);
    p.SetEndCap(LineCapRound);
    PointF a(cx - size * 0.42f, cy + size * 0.02f);
    PointF b(cx - size * 0.10f, cy + size * 0.34f);
    PointF c(cx + size * 0.48f, cy - size * 0.34f);
    g.DrawLine(&p, a, b);
    g.DrawLine(&p, b, c);
}

inline bool InRect(const RectF& r, int x, int y) {
    return x >= r.X && x < r.X + r.Width && y >= r.Y && y < r.Y + r.Height;
}

// Barra de desplazamiento fina (solo indicador).
inline void DrawScrollbar(Graphics& g, const RectF& track, int total, int visible, int offset) {
    if (total <= visible || total <= 0) return;
    DrawRounded(g, track, track.Width / 2, C(25, 255, 255, 255));
    float thumbH = std::max((float)S(26), track.Height * (float)visible / (float)total);
    float thumbY = track.Y + (track.Height - thumbH) * (float)offset / (float)std::max(1, total - visible);
    DrawRounded(g, RectF(track.X, thumbY, track.Width, thumbH), track.Width / 2, C(90, 255, 255, 255));
}

// Fondo desenfocado a partir del wallpaper: se reduce y se vuelve a escalar.
inline Bitmap* CreateBlurredWallpaper(int w, int h, int downscale = 16) {
    std::wstring path = GetWallpaperPath();
    if (path.empty() || w <= 0 || h <= 0) return nullptr;
    Image* img = Image::FromFile(path.c_str());
    if (!img || img->GetLastStatus() != Ok) {
        delete img;
        return nullptr;
    }
    int sw = std::max(1, w / downscale);
    int sh = std::max(1, h / downscale);
    Bitmap* thumb = new Bitmap(sw, sh, PixelFormat32bppPARGB);
    {
        Graphics g(thumb);
        g.SetInterpolationMode(InterpolationModeHighQualityBicubic);
        g.SetPixelOffsetMode(PixelOffsetModeHighQuality);
        g.DrawImage(img, 0, 0, sw, sh);
    }
    delete img;
    return thumb;
}

// ------------------------------------------------------------ estilo propio
// Estilo de ventana inspirado en el Explorador de Windows 11:
// fondo oscuro, esquinas redondeadas, barra de título propia y
// botones de ventana dibujados a mano.
namespace style {

inline const Color windowBg(247, 17, 20, 27);
inline const Color titleBg(255, 23, 27, 36);
inline const Color cardBg(255, 27, 31, 41);
inline const Color cardBorder(40, 255, 255, 255);
inline const Color text(255, 232, 236, 244);
inline const Color textDim(255, 148, 160, 178);
inline const Color textFaint(255, 105, 116, 136);
inline const Color accent(255, 76, 141, 255);
inline const Color accentSolid(255, 59, 130, 246);
inline const Color danger(255, 248, 113, 113);
inline const Color success(255, 74, 222, 128);
inline const Color warning(255, 251, 191, 36);
inline const Color rowHover(255, 34, 39, 51);
inline const Color rowSelected(90, 59, 130, 246);

struct Chrome {
    RectF min, max, close;
};

inline Chrome ChromeRects(int w) {
    Chrome c;
    float titleH = (float)S(42);
    float bw = (float)S(46);
    c.close = RectF((float)w - bw, 0, bw, titleH);
    c.max = RectF((float)w - bw * 2, 0, bw, titleH);
    c.min = RectF((float)w - bw * 3, 0, bw, titleH);
    return c;
}

inline Chrome DrawChrome(Graphics& g, int w, int h, const std::wstring& title,
                         const wchar_t* glyph, bool maximized, int hoverBtn) {
    Chrome c = ChromeRects(w);
    float titleH = (float)S(42);

    g.Clear(C(0, 0, 0, 0));
    DrawRounded(g, RectF(0, 0, (float)w, (float)h), maximized ? 0.0f : SF(10), windowBg, C(55, 255, 255, 255));
    SolidBrush tb(titleBg);
    g.FillRectangle(&tb, 0.0f, 0.0f, (float)w, titleH);
    HLine(g, 0.0f, (float)w, titleH - 1, C(35, 255, 255, 255));

    RectF ic((float)S(14), (float)S(11), (float)S(20), (float)S(20));
    DrawRounded(g, ic, SF(4), accentSolid);
    DrawTextStr(g, glyph, fontIcon, RGBc(255, 255, 255), ic, StringAlignmentCenter);
    DrawTextStr(g, title, fontBold, text,
                RectF((float)S(42), 0, (float)w - S(200), titleH));

    auto button = [&](const RectF& rc, const wchar_t* gl, int id, bool close) {
        bool hov = hoverBtn == id;
        if (hov) {
            SolidBrush b(close ? C(255, 196, 43, 43) : C(255, 44, 50, 64));
            g.FillRectangle(&b, rc);
        }
        DrawTextStr(g, gl, fontIcon, hov && close ? RGBc(255, 255, 255) : textDim,
                    rc, StringAlignmentCenter);
    };
    button(c.min, L"", 1, false);
    button(c.max, maximized ? L"" : L"", 2, false);
    button(c.close, L"", 3, true);
    return c;
}

struct FlyoutChrome {
    RectF close;
};

inline FlyoutChrome DrawFlyoutChrome(Graphics& g, int w, int h, const std::wstring& title,
                                     const wchar_t* glyph, int hoverClose) {
    FlyoutChrome c;
    c.close = RectF((float)w - S(38), (float)S(8), (float)S(28), (float)S(28));

    g.Clear(C(0, 0, 0, 0));
    DrawRounded(g, RectF(0, 0, (float)w, (float)h), SF(12), C(246, 21, 24, 32),
                C(65, 255, 255, 255));

    RectF ic((float)S(16), (float)S(11), (float)S(20), (float)S(20));
    DrawRounded(g, ic, SF(5), accentSolid);
    DrawTextStr(g, glyph, fontIcon, RGBc(255, 255, 255), ic, StringAlignmentCenter);
    DrawTextStr(g, title, fontBold, text,
                RectF((float)S(44), (float)S(6), (float)w - S(90), (float)S(30)));
    if (hoverClose) DrawRounded(g, c.close, SF(5), C(255, 196, 43, 43));
    DrawTextStr(g, L"", fontIcon, hoverClose ? RGBc(255, 255, 255) : textDim,
                c.close, StringAlignmentCenter);
    return c;
}

inline void DrawSwitch(Graphics& g, const RectF& rc, bool on, bool hover) {
    DrawRounded(g, rc, rc.Height / 2.0f,
                on ? accentSolid : C(255, 48, 54, 68),
                on ? Color(0, 0, 0, 0) : C(80, 255, 255, 255));
    float knob = rc.Height - SF(6);
    float x = on ? rc.X + rc.Width - knob - SF(3) : rc.X + SF(3);
    FillCircle(g, x + knob / 2, rc.Y + rc.Height / 2, knob / 2,
               on ? RGBc(255, 255, 255) : C(255, 170, 180, 196));
    (void)hover;
}

inline void DrawLink(Graphics& g, const RectF& rc, const std::wstring& label, bool hover) {
    DrawTextStr(g, label, fontSmall, hover ? accent : textDim, rc, StringAlignmentCenter);
}

inline void DrawProgress(Graphics& g, const RectF& rc, float progress, const Color& color) {
    DrawRounded(g, rc, rc.Height / 2.0f, C(60, 255, 255, 255));
    if (progress < 0) progress = 0;
    if (progress > 1) progress = 1;
    if (progress > 0)
        DrawRounded(g, RectF(rc.X, rc.Y, rc.Width * progress, rc.Height),
                    rc.Height / 2.0f, color);
}

// Botón de barra de herramientas: glifo MDL2 + texto opcional.
inline void DrawToolButton(Graphics& g, const RectF& rc, const wchar_t* glyph, const std::wstring& label,
                           bool hover, bool enabled = true, bool active = false) {
    if (active) DrawRounded(g, rc, SF(5), C(70, 59, 130, 246));
    else if (hover && enabled) DrawRounded(g, rc, SF(5), C(255, 44, 50, 64));
    Color fg = enabled ? (active ? text : textDim) : C(255, 70, 78, 94);
    if (hover && enabled) fg = text;
    if (label.empty()) {
        DrawTextStr(g, glyph, fontIcon, fg, rc, StringAlignmentCenter);
    } else {
        DrawTextStr(g, glyph, fontIcon, fg, RectF(rc.X + S(8), rc.Y, (float)S(18), rc.Height), StringAlignmentCenter);
        DrawTextStr(g, label, fontSmall, fg, RectF(rc.X + S(30), rc.Y, rc.Width - S(34), rc.Height));
    }
}

inline void DrawButton(Graphics& g, const RectF& rc, const std::wstring& label, bool hover,
                       bool primary = false, bool danger_ = false, bool disabled = false) {
    Color fill = hover ? C(255, 44, 50, 64) : C(255, 32, 37, 48);
    Color border = C(45, 255, 255, 255);
    Color fg = text;
    if (primary) { fill = hover ? C(255, 76, 141, 255) : accentSolid; border = C(0, 0, 0, 0); fg = RGBc(255, 255, 255); }
    if (danger_) { fill = hover ? C(255, 86, 34, 40) : C(255, 48, 28, 35); border = C(110, 248, 113, 113); fg = C(255, 252, 165, 165); }
    if (disabled) { fill = C(255, 28, 31, 39); border = C(25, 255, 255, 255); fg = C(255, 84, 94, 112); }
    DrawRounded(g, rc, SF(6), fill, border);
    DrawTextStr(g, label, fontBody, fg, rc, StringAlignmentCenter);
}

} // namespace style

// ------------------------------------------------------------ ventana base
// Doble buffer sobre una DIB section persistente: solo se redibuja y se vuelca
// a pantalla la región inválida (ps.rcPaint). Los buffers de ventanas ocultas
// se pueden liberar con ReleaseBuffers() para ahorrar memoria.
class WindowBase {
public:
    virtual ~WindowBase() {
        if (hwnd && IsWindow(hwnd)) DestroyWindow(hwnd);
        ReleaseBuffers();
    }

    HWND Hwnd() const { return hwnd; }

    bool Create(const wchar_t* cls, DWORD exStyle, DWORD style, int x, int y, int w, int h,
                const wchar_t* title = L"", UINT classStyle = CS_DBLCLKS) {
        WNDCLASSEXW wc = {};
        wc.cbSize = sizeof(wc);
        wc.style = classStyle;
        wc.lpfnWndProc = StaticProc;
        wc.hInstance = g_hInst;
        wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
        wc.lpszClassName = cls;
        RegisterClassExW(&wc);
        hwnd = CreateWindowExW(exStyle, cls, title, style, x, y, w, h, nullptr, nullptr, g_hInst, this);
        return hwnd != nullptr;
    }

    void Close() {
        if (hwnd && IsWindow(hwnd)) DestroyWindow(hwnd);
    }

    void Invalidate() {
        if (hwnd) InvalidateRect(hwnd, nullptr, FALSE);
    }

    void InvalidateRectF(const RectF& r) {
        if (!hwnd) return;
        RECT rc = { (LONG)std::floor(r.X) - 1, (LONG)std::floor(r.Y) - 1,
                    (LONG)std::ceil(r.X + r.Width) + 1, (LONG)std::ceil(r.Y + r.Height) + 1 };
        InvalidateRect(hwnd, &rc, FALSE);
    }

    void ReleaseBuffers() {
        delete gbmp;
        gbmp = nullptr;
        if (memDC) { SelectObject(memDC, oldBmp); DeleteDC(memDC); memDC = nullptr; }
        if (dib) { DeleteObject(dib); dib = nullptr; }
        bits = nullptr;
        bufW = bufH = 0;
    }

protected:
    HWND hwnd = nullptr;
    bool trackingMouse = false;
    // Marco propio sobre WS_OVERLAPPEDWINDOW: todo es área de cliente, pero se
    // conservan maximizar real, Aero Snap, Win+flechas y la animación de minimizar.
    bool customFrame = false;

    // Bordes de redimensión + barra de título (excepto los rectángulos indicados).
    LRESULT FrameHitTest(int x, int y, int captionH, std::initializer_list<RectF> exclude) {
        RECT rc;
        GetClientRect(hwnd, &rc);
        if (!IsZoomed(hwnd)) {
            int b = S(6);
            bool l = x < b, r = x >= rc.right - b, t = y < b, btm = y >= rc.bottom - b;
            if (t && l) return HTTOPLEFT;
            if (t && r) return HTTOPRIGHT;
            if (btm && l) return HTBOTTOMLEFT;
            if (btm && r) return HTBOTTOMRIGHT;
            if (t) return HTTOP;
            if (btm) return HTBOTTOM;
            if (l) return HTLEFT;
            if (r) return HTRIGHT;
        }
        if (y < captionH) {
            for (const RectF& e : exclude)
                if (InRect(e, x, y)) return HTCLIENT;
            return HTCAPTION;
        }
        return 0;
    }

    virtual void OnPaint(Graphics& g, int w, int h) = 0;
    // Pintado GDI previo (p. ej. PaintDesktop) sobre la misma DIB, en coordenadas de cliente.
    virtual void OnPaintBackground(HDC, const RECT&) {}
    virtual void OnCreate() {}
    virtual void OnDestroy() {}
    virtual void OnTimer(UINT_PTR) {}
    virtual void OnMouseDown(int, int) {}
    virtual void OnMouseUp(int, int) {}
    virtual void OnMouseMove(int, int) {}
    virtual void OnMouseLeave() { OnMouseMove(-30000, -30000); }
    virtual void OnDoubleClick(int, int) {}
    virtual void OnWheel(int) {}
    virtual void OnKeyDown(int) {}
    virtual void OnChar(wchar_t) {}
    virtual void OnActivate(bool) {}
    virtual void OnCommand(int, int) {}
    virtual void OnSettingsChange() {}
    virtual void OnSize(UINT, int, int) {}
    virtual bool CanClose() { return true; }
    virtual LRESULT OnHitTest(int, int) { return 0; } // 0 => DefWindowProc
    virtual LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }

private:
    HDC memDC = nullptr;
    HBITMAP dib = nullptr;
    HGDIOBJ oldBmp = nullptr;
    void* bits = nullptr;
    Bitmap* gbmp = nullptr;
    int bufW = 0, bufH = 0;

    bool EnsureBuffer(int w, int h) {
        if (gbmp && bufW == w && bufH == h) return false;
        ReleaseBuffers();
        BITMAPINFO bi = {};
        bi.bmiHeader.biSize = sizeof(bi.bmiHeader);
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        memDC = CreateCompatibleDC(nullptr);
        dib = CreateDIBSection(memDC, &bi, DIB_RGB_COLORS, &bits, nullptr, 0);
        if (!dib) { DeleteDC(memDC); memDC = nullptr; return true; }
        oldBmp = SelectObject(memDC, dib);
        gbmp = new Bitmap(w, h, w * 4, PixelFormat32bppPARGB, (BYTE*)bits);
        bufW = w;
        bufH = h;
        return true;
    }

protected:
    void PaintDoubleBuffered() {
        PAINTSTRUCT ps;
        HDC hdc = BeginPaint(hwnd, &ps);
        RECT rc;
        GetClientRect(hwnd, &rc);
        int w = rc.right - rc.left;
        int h = rc.bottom - rc.top;
        if (w <= 0 || h <= 0) {
            EndPaint(hwnd, &ps);
            return;
        }
        bool fresh = EnsureBuffer(w, h);
        if (!gbmp) { EndPaint(hwnd, &ps); return; }
        RECT paint = fresh ? rc : ps.rcPaint;
        IntersectRect(&paint, &paint, &rc);
        if (IsRectEmpty(&paint)) { EndPaint(hwnd, &ps); return; }

        OnPaintBackground(memDC, paint);
        GdiFlush();
        {
            Graphics g(gbmp);
            g.SetClip(Rect(paint.left, paint.top, paint.right - paint.left, paint.bottom - paint.top));
            g.SetSmoothingMode(SmoothingModeAntiAlias);
            g.SetTextRenderingHint(TextRenderingHintClearTypeGridFit);
            g.SetPixelOffsetMode(PixelOffsetModeHalf);
            OnPaint(g, w, h);
            g.Flush(FlushIntentionSync);
        }
        BitBlt(hdc, paint.left, paint.top, paint.right - paint.left, paint.bottom - paint.top,
               memDC, paint.left, paint.top, SRCCOPY);
        EndPaint(hwnd, &ps);
    }

    static LRESULT CALLBACK StaticProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
        WindowBase* self = nullptr;
        if (msg == WM_NCCREATE) {
            CREATESTRUCTW* cs = (CREATESTRUCTW*)lp;
            self = (WindowBase*)cs->lpCreateParams;
            self->hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (LONG_PTR)self);
        } else {
            self = (WindowBase*)GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        }
        if (self) return self->Handle(msg, wp, lp);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }

    LRESULT Handle(UINT msg, WPARAM wp, LPARAM lp) {
        switch (msg) {
        case WM_ERASEBKGND:
            return 1;
        case WM_PAINT:
            PaintDoubleBuffered();
            return 0;
        case WM_SIZE:
            OnSize((UINT)wp, LOWORD(lp), HIWORD(lp));
            InvalidateRect(hwnd, nullptr, FALSE);
            return 0;
        case WM_CREATE:
            OnCreate();
            return 0;
        case WM_DESTROY:
            OnDestroy();
            return 0;
        case WM_CLOSE:
            if (CanClose()) DestroyWindow(hwnd);
            return 0;
        case WM_COMMAND:
            OnCommand(LOWORD(wp), HIWORD(wp));
            return 0;
        case WM_ACTIVATE:
            OnActivate(LOWORD(wp) != WA_INACTIVE);
            return 0;
        case WM_LBUTTONDOWN:
            SetFocus(hwnd);
            OnMouseDown(GET_X_LPARAM(lp), GET_Y_LPARAM(lp));
            return 0;
        case WM_LBUTTONUP:
            OnMouseUp(GET_X_LPARAM(lp), GET_Y_LPARAM(lp));
            return 0;
        case WM_LBUTTONDBLCLK:
            OnDoubleClick(GET_X_LPARAM(lp), GET_Y_LPARAM(lp));
            return 0;
        case WM_MOUSEMOVE:
            if (!trackingMouse) {
                TRACKMOUSEEVENT tme = { sizeof(tme), TME_LEAVE, hwnd, 0 };
                trackingMouse = TrackMouseEvent(&tme) != FALSE;
            }
            OnMouseMove(GET_X_LPARAM(lp), GET_Y_LPARAM(lp));
            return 0;
        case WM_MOUSELEAVE:
            trackingMouse = false;
            OnMouseLeave();
            return 0;
        case WM_MOUSEWHEEL:
            OnWheel(GET_WHEEL_DELTA_WPARAM(wp));
            return 0;
        case WM_KEYDOWN:
            OnKeyDown((int)wp);
            return 0;
        case WM_CHAR:
            OnChar((wchar_t)wp);
            return 0;
        case WM_TIMER:
            OnTimer((UINT_PTR)wp);
            return 0;
        case WM_SETTINGCHANGE:
            OnSettingsChange();
            return 0;
        case WM_CTLCOLOREDIT:
            return CtlColorEdit(wp);
        case WM_NCCALCSIZE:
            if (customFrame && wp) {
                if (IsZoomed(hwnd)) {
                    // Maximizada, Windows la extiende el grosor del marco fuera del monitor.
                    auto* p = (NCCALCSIZE_PARAMS*)lp;
                    int fx = GetSystemMetrics(SM_CXFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                    int fy = GetSystemMetrics(SM_CYFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                    p->rgrc[0].left += fx;
                    p->rgrc[0].right -= fx;
                    p->rgrc[0].top += fy;
                    p->rgrc[0].bottom -= fy;
                }
                return 0;
            }
            break;
        case WM_NCHITTEST: {
            POINT pt = { GET_X_LPARAM(lp), GET_Y_LPARAM(lp) };
            ScreenToClient(hwnd, &pt);
            LRESULT r = OnHitTest(pt.x, pt.y);
            if (r) return r;
            break;
        }
        default:
            break;
        }
        return OnMessage(msg, wp, lp);
    }
};

// Bucle de mensajes estándar.
inline int RunMessageLoop() {
    MSG msg;
    while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    return (int)msg.wParam;
}

} // namespace ui
