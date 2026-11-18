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
#include <memory>
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

inline int S(int v) { return MulDiv(v, g_dpi, 96); }
inline float SF(float v) { return v * (float)g_dpi / 96.0f; }
inline Color C(BYTE a, BYTE r, BYTE g, BYTE b) { return Color(a, r, g, b); }
inline Color RGBc(BYTE r, BYTE g, BYTE b) { return Color(255, r, g, b); }

// ------------------------------------------------------------ tema
// Todos los colores de la interfaz salen de estos tokens. ApplyTheme() los
// rellena a partir de la configuración (modo claro/oscuro, acento, transparencia),
// así que shell, explorador, Administrador de tareas, menús y diálogos comparten tema.
namespace style {
inline bool dark = true;
inline bool transparency = true;
inline Color windowBg, titleBg, cardBg, cardBorder, text, textDim, textFaint;
inline Color accent, accentSolid, accentHover, onAccent, rowSelected, focusRing;
inline Color danger, success, warning, closeHover;
inline Color rowHover, controlBg, controlHover, controlBorder, separator, navBg, dialogBg;
inline Color menuBg, menuHover, menuBorder;
inline Color taskbarBg, taskButton, taskButtonHover, taskButtonActive, flyoutBg, flyoutBorder;
inline Color graphBg, gridLine, track;
inline COLORREF editBg = RGB(35, 40, 52), editFg = RGB(232, 236, 244);
inline DWORD taskbarTint = 0, flyoutTint = 0; // AABBGGRR para el acrílico de DWM
} // namespace style

struct ThemeSettings {
    bool dark = true;
    COLORREF accent = RGB(59, 130, 246);
    bool transparency = true;
    bool operator==(const ThemeSettings& o) const {
        return dark == o.dark && accent == o.accent && transparency == o.transparency;
    }
};

struct AccentPreset {
    COLORREF color;
    const wchar_t* name;
};

inline const AccentPreset kAccentPresets[] = {
    { RGB(59, 130, 246), L"Azul" },    { RGB(14, 165, 233), L"Cielo" },   { RGB(20, 184, 166), L"Turquesa" },
    { RGB(34, 197, 94), L"Verde" },    { RGB(132, 204, 22), L"Lima" },    { RGB(234, 179, 8), L"Ámbar" },
    { RGB(249, 115, 22), L"Naranja" }, { RGB(239, 68, 68), L"Rojo" },     { RGB(236, 72, 153), L"Rosa" },
    { RGB(139, 92, 246), L"Violeta" }, { RGB(100, 116, 139), L"Pizarra" },
};

inline ThemeSettings g_theme;

inline std::wstring ConfigDir() {
    wchar_t buf[MAX_PATH] = {};
    SHGetFolderPathW(nullptr, CSIDL_APPDATA, nullptr, 0, buf);
    std::wstring dir = std::wstring(buf) + L"\\OptimizedShell";
    CreateDirectoryW(dir.c_str(), nullptr);
    return dir;
}

inline std::wstring ThemeFile() { return ConfigDir() + L"\\theme.ini"; }

inline ThemeSettings LoadThemeSettings() {
    ThemeSettings t;
    std::wstring file = ThemeFile();
    wchar_t buf[64] = {};
    GetPrivateProfileStringW(L"Tema", L"Modo", L"oscuro", buf, 64, file.c_str());
    t.dark = _wcsicmp(buf, L"claro") != 0;
    GetPrivateProfileStringW(L"Tema", L"Acento", L"3B82F6", buf, 64, file.c_str());
    unsigned long rgb = wcstoul(buf, nullptr, 16);
    t.accent = RGB((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
    t.transparency = GetPrivateProfileIntW(L"Tema", L"Transparencia", 1, file.c_str()) != 0;
    return t;
}

inline void SaveThemeSettings(const ThemeSettings& t) {
    std::wstring file = ThemeFile();
    wchar_t hex[16];
    swprintf_s(hex, L"%02X%02X%02X", GetRValue(t.accent), GetGValue(t.accent), GetBValue(t.accent));
    WritePrivateProfileStringW(L"Tema", L"Modo", t.dark ? L"oscuro" : L"claro", file.c_str());
    WritePrivateProfileStringW(L"Tema", L"Acento", hex, file.c_str());
    WritePrivateProfileStringW(L"Tema", L"Transparencia", t.transparency ? L"1" : L"0", file.c_str());
}

inline Color Mix(const Color& a, const Color& b, float t, BYTE alpha = 255) {
    auto m = [t](BYTE x, BYTE y) { return (BYTE)(x + (y - x) * t + 0.5f); };
    return Color(alpha, m(a.GetR(), b.GetR()), m(a.GetG(), b.GetG()), m(a.GetB(), b.GetB()));
}

inline Color WithAlpha(const Color& c, BYTE a) { return Color(a, c.GetR(), c.GetG(), c.GetB()); }

inline Color FromRef(COLORREF c, BYTE a = 255) { return Color(a, GetRValue(c), GetGValue(c), GetBValue(c)); }

// Color de acento actual de Windows (para "Usar el color de Windows").
inline COLORREF WindowsAccent() {
    DWORD argb = 0;
    BOOL opaque = FALSE;
    if (SUCCEEDED(DwmGetColorizationColor(&argb, &opaque)))
        return RGB((argb >> 16) & 0xFF, (argb >> 8) & 0xFF, argb & 0xFF);
    return RGB(59, 130, 246);
}

// Tooltips registrados en este proceso (se recolorean al cambiar el tema).
inline std::vector<HWND>& ThemedTips() {
    static std::vector<HWND> tips;
    return tips;
}

inline void ThemeTooltip(HWND tip) {
    SetWindowTheme(tip, L"", L""); // sin estilo visual: así respeta los colores propios
    SendMessageW(tip, TTM_SETTIPBKCOLOR, RGB(style::menuBg.GetR(), style::menuBg.GetG(), style::menuBg.GetB()), 0);
    SendMessageW(tip, TTM_SETTIPTEXTCOLOR, RGB(style::text.GetR(), style::text.GetG(), style::text.GetB()), 0);
    RECT m = { 6, 4, 6, 4 };
    SendMessageW(tip, TTM_SETMARGIN, 0, (LPARAM)&m);
}

// Menús nativos que siguen existiendo (menú contextual de extensiones de shell).
inline void SetNativeMenuMode(bool dark) {
    static HMODULE ux = LoadLibraryW(L"uxtheme.dll");
    if (!ux) return;
    typedef int(WINAPI* PFN_SetPreferredAppMode)(int);
    typedef void(WINAPI* PFN_FlushMenuThemes)();
    auto setMode = (PFN_SetPreferredAppMode)GetProcAddress(ux, MAKEINTRESOURCEA(135));
    auto flush = (PFN_FlushMenuThemes)GetProcAddress(ux, MAKEINTRESOURCEA(136));
    if (setMode) setMode(dark ? 2 : 3); // ForceDark / ForceLight
    if (flush) flush();
}

inline void ApplyTheme(const ThemeSettings& t) {
    using namespace style;
    g_theme = t;
    dark = t.dark;
    transparency = t.transparency;
    Color a = FromRef(t.accent);
    Color white(255, 255, 255, 255), black(255, 0, 0, 0);
    float lum = (0.299f * a.GetR() + 0.587f * a.GetG() + 0.114f * a.GetB()) / 255.0f;

    if (dark) {
        windowBg = Color(transparency ? 247 : 255, 17, 20, 27);
        titleBg = RGBc(23, 27, 36);
        cardBg = RGBc(27, 31, 41);
        cardBorder = C(40, 255, 255, 255);
        text = RGBc(232, 236, 244);
        textDim = RGBc(148, 160, 178);
        textFaint = RGBc(105, 116, 136);
        danger = RGBc(248, 113, 113);
        success = RGBc(74, 222, 128);
        warning = RGBc(251, 191, 36);
        rowHover = RGBc(34, 39, 51);
        controlBg = RGBc(35, 40, 52);
        controlHover = RGBc(44, 50, 64);
        controlBorder = C(45, 255, 255, 255);
        separator = C(30, 255, 255, 255);
        navBg = RGBc(19, 22, 30);
        dialogBg = RGBc(20, 23, 31);
        menuBg = RGBc(30, 34, 44);
        menuHover = RGBc(48, 54, 68);
        menuBorder = C(60, 255, 255, 255);
        taskbarBg = Color(transparency ? 150 : 255, 16, 18, 24);
        taskButton = RGBc(28, 31, 40);
        taskButtonHover = RGBc(38, 42, 54);
        taskButtonActive = RGBc(46, 51, 66);
        flyoutBg = Color(transparency ? 246 : 255, 21, 24, 32);
        flyoutBorder = C(65, 255, 255, 255);
        graphBg = RGBc(22, 26, 35);
        gridLine = C(18, 255, 255, 255);
        track = C(60, 255, 255, 255);
        editBg = RGB(35, 40, 52);
        editFg = RGB(232, 236, 244);
        taskbarTint = 0xE614161C;
        flyoutTint = 0xF214161C;
        accent = Mix(a, white, 0.18f);
        accentHover = Mix(a, white, 0.12f);
        rowSelected = WithAlpha(a, 90);
    } else {
        windowBg = Color(transparency ? 250 : 255, 247, 248, 250);
        titleBg = RGBc(237, 239, 243);
        cardBg = RGBc(255, 255, 255);
        cardBorder = C(36, 0, 0, 0);
        text = RGBc(26, 30, 38);
        textDim = RGBc(78, 88, 104);
        textFaint = RGBc(124, 132, 146);
        danger = RGBc(220, 38, 38);
        success = RGBc(22, 163, 74);
        warning = RGBc(202, 138, 4);
        rowHover = RGBc(230, 233, 239);
        controlBg = RGBc(255, 255, 255);
        controlHover = RGBc(234, 237, 242);
        controlBorder = C(48, 0, 0, 0);
        separator = C(26, 0, 0, 0);
        navBg = RGBc(240, 242, 245);
        dialogBg = RGBc(249, 250, 251);
        menuBg = RGBc(252, 252, 253);
        menuHover = RGBc(229, 233, 240);
        menuBorder = C(45, 0, 0, 0);
        taskbarBg = Color(transparency ? 175 : 255, 238, 240, 244);
        taskButton = RGBc(226, 229, 235);
        taskButtonHover = RGBc(214, 219, 227);
        taskButtonActive = RGBc(250, 251, 252);
        flyoutBg = Color(transparency ? 244 : 255, 248, 249, 251);
        flyoutBorder = C(45, 0, 0, 0);
        graphBg = RGBc(255, 255, 255);
        gridLine = C(20, 0, 0, 0);
        track = C(45, 0, 0, 0);
        editBg = RGB(255, 255, 255);
        editFg = RGB(26, 30, 38);
        taskbarTint = 0xD8F4F2F0;
        flyoutTint = 0xEEFAF9F8;
        accent = Mix(a, black, 0.12f);
        accentHover = Mix(a, black, 0.08f);
        rowSelected = WithAlpha(a, 60);
    }
    accentSolid = a;
    if (!dark) accentHover = Mix(a, black, 0.1f);
    else accentHover = Mix(a, white, 0.12f);
    onAccent = lum > 0.68f ? RGBc(20, 20, 24) : white;
    focusRing = WithAlpha(accent, 150);
    closeHover = RGBc(196, 43, 43);

    if (hbrEdit) DeleteObject(hbrEdit);
    hbrEdit = CreateSolidBrush(editBg);
    SetNativeMenuMode(dark);
    auto& tips = ThemedTips();
    tips.erase(std::remove_if(tips.begin(), tips.end(), [](HWND h) { return !IsWindow(h); }), tips.end());
    for (HWND tip : tips) ThemeTooltip(tip);
}

// Mensaje que se difunde a todas las ventanas cuando cambia el tema.
inline UINT ThemeMessage() {
    static UINT m = RegisterWindowMessageW(L"OptimizedShell.ThemeChanged");
    return m;
}

// Relee theme.ini; true si ha cambiado algo.
inline bool ReloadTheme() {
    ThemeSettings t = LoadThemeSettings();
    if (t == g_theme && hbrEdit) return false;
    ApplyTheme(t);
    return true;
}

// Guarda, aplica en este proceso y avisa al resto de ventanas de OptimizedShell.
inline void SetTheme(const ThemeSettings& t) {
    SaveThemeSettings(t);
    ApplyTheme(t);
    SendNotifyMessageW(HWND_BROADCAST, ThemeMessage(), 0, 0);
}

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

    INITCOMMONCONTROLSEX icc = { sizeof(icc), ICC_STANDARD_CLASSES | ICC_BAR_CLASSES };
    InitCommonControlsEx(&icc);

    ApplyTheme(LoadThemeSettings());
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

inline void SetAccentPolicy(HWND hwnd, ACCENT_STATE state, DWORD gradient) {
    static PFN_SetWindowCompositionAttribute setAttr = nullptr;
    if (!setAttr) {
        HMODULE user32 = GetModuleHandleW(L"user32.dll");
        if (user32) setAttr = (PFN_SetWindowCompositionAttribute)GetProcAddress(user32, "SetWindowCompositionAttribute");
    }
    if (!setAttr) return;
    ACCENT_POLICY policy = {};
    policy.AccentState = state;
    policy.AccentFlags = 2;
    policy.GradientColor = gradient;
    WINDOWCOMPOSITIONATTRIBDATA data = {};
    data.Attribute = 19; // WCA_ACCENT_POLICY
    data.Data = &policy;
    data.SizeOfData = sizeof(policy);
    setAttr(hwnd, &data);
}

// Tipos de marco: opaco (ventanas con controles), acrílico (barra/paneles) o menú.
enum FrameKind { FrameNone = 0, FrameOpaque, FrameTaskbar, FrameFlyout, FrameMenu };

inline void ApplyFrame(HWND hwnd, FrameKind kind) {
    if (!hwnd || kind == FrameNone) return;
    BOOL dark = style::dark;
    DwmSetWindowAttribute(hwnd, 20, &dark, sizeof(dark)); // DWMWA_USE_IMMERSIVE_DARK_MODE
    int pref = kind == FrameTaskbar ? 1 : (kind == FrameMenu ? 3 : 2); // sin redondear / pequeño / normal
    DwmSetWindowAttribute(hwnd, 33, &pref, sizeof(pref));
    if (kind == FrameTaskbar || kind == FrameFlyout) {
        bool acrylic = style::transparency;
        SetAccentPolicy(hwnd, acrylic ? ACCENT_ENABLE_ACRYLICBLURBEHIND : ACCENT_DISABLED,
                        kind == FrameTaskbar ? style::taskbarTint : style::flyoutTint);
    }
}

// Compatibilidad: marco opaco con el tema actual.
inline void EnableDarkFrame(HWND hwnd) { ApplyFrame(hwnd, FrameOpaque); }

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
    SetWindowTheme(e, style::dark ? L"DarkMode_CFD" : L"CFD", nullptr);
    if (cue) SendMessageW(e, EM_SETCUEBANNER, TRUE, (LPARAM)cue);
    SendMessageW(e, EM_SETMARGINS, EC_LEFTMARGIN | EC_RIGHTMARGIN, MAKELPARAM(S(2), S(2)));
    return e;
}

// Responde a WM_CTLCOLOREDIT con colores del tema oscuro.
inline LRESULT CtlColorEdit(WPARAM wp) {
    HDC dc = (HDC)wp;
    SetTextColor(dc, style::editFg);
    SetBkColor(dc, style::editBg);
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
        ThemeTooltip(tip);
        ThemedTips().push_back(tip);
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

// HBITMAP (32 bpp con alfa premultiplicado, p. ej. de IShellItemImageFactory) -> Bitmap PARGB.
inline Bitmap* HBitmapToBitmap(HBITMAP hb) {
    if (!hb) return nullptr;
    BITMAP bm = {};
    if (!GetObjectW(hb, sizeof(bm), &bm) || bm.bmWidth <= 0 || bm.bmHeight == 0) return nullptr;
    int w = bm.bmWidth, h = std::abs(bm.bmHeight);
    std::vector<DWORD> px((size_t)w * h);
    BITMAPINFO bi = {};
    bi.bmiHeader.biSize = sizeof(bi.bmiHeader);
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    HDC dc = GetDC(nullptr);
    int lines = GetDIBits(dc, hb, 0, h, px.data(), &bi, DIB_RGB_COLORS);
    ReleaseDC(nullptr, dc);
    if (lines <= 0) return nullptr;
    bool alpha = false;
    for (DWORD p : px) if (p >> 24) { alpha = true; break; }
    if (!alpha) for (DWORD& p : px) p |= 0xFF000000;
    Bitmap* out = new Bitmap(w, h, PixelFormat32bppPARGB);
    BitmapData bd = {};
    Rect r(0, 0, w, h);
    if (out->LockBits(&r, ImageLockModeWrite, PixelFormat32bppPARGB, &bd) == Ok) {
        for (int y = 0; y < h; ++y)
            memcpy((BYTE*)bd.Scan0 + (size_t)y * bd.Stride, px.data() + (size_t)y * w, (size_t)w * 4);
        out->UnlockBits(&bd);
    }
    return out;
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
    DrawRounded(g, track, track.Width / 2, style::separator);
    float thumbH = std::max((float)S(26), track.Height * (float)visible / (float)total);
    float thumbY = track.Y + (track.Height - thumbH) * (float)offset / (float)std::max(1, total - visible);
    DrawRounded(g, RectF(track.X, thumbY, track.Width, thumbH), track.Width / 2, WithAlpha(style::textFaint, 170));
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
// Estilo de ventana inspirado en el Explorador de Windows 11: esquinas
// redondeadas, barra de título propia y botones de ventana dibujados a mano.
// Todos los colores salen de los tokens del tema (ver ApplyTheme).
namespace style {

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
    DrawRounded(g, RectF(0, 0, (float)w, (float)h), maximized ? 0.0f : SF(10), windowBg, cardBorder);
    SolidBrush tb(titleBg);
    g.FillRectangle(&tb, 0.0f, 0.0f, (float)w, titleH);
    HLine(g, 0.0f, (float)w, titleH - 1, separator);

    RectF ic((float)S(14), (float)S(11), (float)S(20), (float)S(20));
    DrawRounded(g, ic, SF(4), accentSolid);
    DrawTextStr(g, glyph, fontIcon, onAccent, ic, StringAlignmentCenter);
    DrawTextStr(g, title, fontBold, text, RectF((float)S(42), 0, (float)w - S(200), titleH));

    auto button = [&](const RectF& rc, const wchar_t* gl, int id, bool close) {
        bool hov = hoverBtn == id;
        if (hov) {
            SolidBrush b(close ? closeHover : controlHover);
            g.FillRectangle(&b, rc);
        }
        DrawTextStr(g, gl, fontIcon, hov && close ? RGBc(255, 255, 255) : textDim, rc, StringAlignmentCenter);
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
    DrawRounded(g, RectF(0, 0, (float)w, (float)h), SF(12), flyoutBg, flyoutBorder);

    RectF ic((float)S(16), (float)S(11), (float)S(20), (float)S(20));
    DrawRounded(g, ic, SF(5), accentSolid);
    DrawTextStr(g, glyph, fontIcon, onAccent, ic, StringAlignmentCenter);
    DrawTextStr(g, title, fontBold, text, RectF((float)S(44), (float)S(6), (float)w - S(90), (float)S(30)));
    if (hoverClose) DrawRounded(g, c.close, SF(5), closeHover);
    DrawTextStr(g, L"", fontIcon, hoverClose ? RGBc(255, 255, 255) : textDim, c.close, StringAlignmentCenter);
    return c;
}

inline void DrawSwitch(Graphics& g, const RectF& rc, bool on, bool hover) {
    DrawRounded(g, rc, rc.Height / 2.0f, on ? (hover ? accentHover : accentSolid) : (hover ? controlHover : controlBg),
                on ? Color(0, 0, 0, 0) : WithAlpha(textDim, 160));
    float knob = rc.Height - SF(6);
    float x = on ? rc.X + rc.Width - knob - SF(3) : rc.X + SF(3);
    FillCircle(g, x + knob / 2, rc.Y + rc.Height / 2, knob / 2, on ? onAccent : textDim);
}

inline void DrawLink(Graphics& g, const RectF& rc, const std::wstring& label, bool hover) {
    DrawTextStr(g, label, fontSmall, hover ? accent : textDim, rc, StringAlignmentCenter);
}

inline void DrawProgress(Graphics& g, const RectF& rc, float progress, const Color& color) {
    DrawRounded(g, rc, rc.Height / 2.0f, track);
    if (progress < 0) progress = 0;
    if (progress > 1) progress = 1;
    if (progress > 0)
        DrawRounded(g, RectF(rc.X, rc.Y, rc.Width * progress, rc.Height), rc.Height / 2.0f, color);
}

// Casilla de verificación con etiqueta.
inline void DrawCheckbox(Graphics& g, const RectF& r, bool on, const std::wstring& label) {
    RectF b(r.X, r.Y + (r.Height - S(18)) / 2, (float)S(18), (float)S(18));
    DrawRounded(g, b, SF(4), on ? accentSolid : controlBg, on ? Color(0, 0, 0, 0) : WithAlpha(textDim, 170));
    if (on) DrawCheck(g, b.X + b.Width / 2, b.Y + b.Height / 2, b.Width * 0.6f, onAccent, SF(1.8f));
    DrawTextStr(g, label, fontSmall, text, RectF(b.X + S(26), r.Y, r.Width - S(26), r.Height));
}

// Botón de barra de herramientas: glifo MDL2 + texto opcional.
inline void DrawToolButton(Graphics& g, const RectF& rc, const wchar_t* glyph, const std::wstring& label,
                           bool hover, bool enabled = true, bool active = false) {
    if (active) DrawRounded(g, rc, SF(5), rowSelected);
    else if (hover && enabled) DrawRounded(g, rc, SF(5), controlHover);
    Color fg = enabled ? (active ? text : textDim) : WithAlpha(textFaint, 140);
    if (hover && enabled) fg = text;
    if (label.empty()) {
        DrawTextStr(g, glyph, fontIcon, fg, rc, StringAlignmentCenter);
    } else {
        DrawTextStr(g, glyph, fontIcon, fg, RectF(rc.X + S(8), rc.Y, (float)S(18), rc.Height), StringAlignmentCenter);
        DrawTextStr(g, label, fontSmall, fg, RectF(rc.X + S(30), rc.Y, rc.Width - S(34), rc.Height));
    }
}

inline void DrawButton(Graphics& g, const RectF& rc, const std::wstring& label, bool hover,
                       bool primary = false, bool danger_ = false, bool disabled = false, bool focused = false) {
    Color fill = hover ? controlHover : controlBg;
    Color border = controlBorder;
    Color fg = text;
    if (primary) { fill = hover ? accentHover : accentSolid; border = Color(0, 0, 0, 0); fg = onAccent; }
    if (danger_) {
        fill = hover ? Mix(controlBg, danger, 0.28f) : Mix(controlBg, danger, 0.14f);
        border = WithAlpha(danger, 130);
        fg = danger;
    }
    if (disabled) { fill = controlBg; border = separator; fg = WithAlpha(textFaint, 150); }
    DrawRounded(g, rc, SF(6), fill, border);
    if (focused && !disabled)
        DrawRounded(g, RectF(rc.X - 2, rc.Y - 2, rc.Width + 4, rc.Height + 4), SF(8), Color(0, 0, 0, 0), focusRing, 1.5f);
    DrawTextStr(g, label, fontBody, fg, rc, StringAlignmentCenter);
}

// Caja de texto (el texto lo pinta un EDIT nativo encima).
inline void DrawField(Graphics& g, const RectF& rc, bool focused = false, float radius = 6) {
    DrawRounded(g, rc, SF(radius), controlBg, focused ? accent : controlBorder, focused ? 1.3f : 1.0f);
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

    // Tipo de marco: se vuelve a aplicar cuando cambia el tema.
    void SetFrame(FrameKind k) {
        frameKind = k;
        ApplyFrame(hwnd, k);
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
    FrameKind frameKind = FrameNone;

    virtual void OnThemeChanged() {}

    static BOOL CALLBACK ThemeChildProc(HWND child, LPARAM) {
        wchar_t cls[32] = {};
        GetClassNameW(child, cls, 32);
        if (_wcsicmp(cls, L"Edit") == 0) SetWindowTheme(child, style::dark ? L"DarkMode_CFD" : L"CFD", nullptr);
        return TRUE;
    }

    void HandleThemeChange() {
        ReloadTheme();
        ApplyFrame(hwnd, frameKind);
        EnumChildWindows(hwnd, ThemeChildProc, 0);
        OnThemeChanged();
        ReleaseBuffers(); // repintado completo
        RedrawWindow(hwnd, nullptr, nullptr, RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_FRAME);
    }

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
        if (msg == ThemeMessage()) {
            HandleThemeChange();
            return 0;
        }
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

// ------------------------------------------------------------ menús con el tema propio
// Sustituto de TrackPopupMenu: lee un HMENU normal (texto, atajos tras '\t',
// marcas, deshabilitados, submenús) y lo dibuja con los colores del tema.
struct MenuEntry {
    UINT id = 0;
    std::wstring text, shortcut;
    bool sep = false, checked = false, grayed = false, isDefault = false, radio = false;
    HMENU sub = nullptr;
};

inline std::vector<MenuEntry> ReadMenuEntries(HMENU m) {
    std::vector<MenuEntry> out;
    int n = GetMenuItemCount(m);
    for (int i = 0; i < n; ++i) {
        wchar_t buf[512] = {};
        MENUITEMINFOW mii = { sizeof(mii) };
        mii.fMask = MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING;
        mii.dwTypeData = buf;
        mii.cch = 511;
        if (!GetMenuItemInfoW(m, i, TRUE, &mii)) continue;
        MenuEntry e;
        e.sep = (mii.fType & MFT_SEPARATOR) != 0;
        e.radio = (mii.fType & MFT_RADIOCHECK) != 0;
        e.id = mii.wID;
        e.sub = mii.hSubMenu;
        e.checked = (mii.fState & MFS_CHECKED) != 0;
        e.grayed = (mii.fState & (MFS_GRAYED | MFS_DISABLED)) != 0;
        e.isDefault = (mii.fState & MFS_DEFAULT) != 0;
        std::wstring t = buf;
        size_t tab = t.find(L'\t');
        if (tab != std::wstring::npos) {
            e.shortcut = t.substr(tab + 1);
            t.resize(tab);
        }
        for (size_t k = 0; k < t.size(); ++k) { // quitar '&' de los mnemónicos
            if (t[k] == L'&') {
                if (k + 1 < t.size() && t[k + 1] == L'&') { e.text += L'&'; ++k; }
                continue;
            }
            e.text += t[k];
        }
        out.push_back(std::move(e));
    }
    return out;
}

class MenuPopup : public WindowBase {
public:
    std::vector<MenuEntry> items;
    std::vector<RectF> rects;
    int hover = -1;
    int width = 0, height = 0;

    bool Open(HMENU m, int x, int y, UINT flags, const RECT* parentItem) {
        items = ReadMenuEntries(m);
        if (items.empty()) return false;
        Measure();
        POINT anchor = { x, y };
        if (parentItem) anchor = { parentItem->right, parentItem->top };
        MONITORINFO mi = { sizeof(mi) };
        GetMonitorInfoW(MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST), &mi);
        RECT wa = mi.rcWork;
        int px = x, py = y;
        if (parentItem) {
            px = parentItem->right - S(2);
            py = parentItem->top - S(4);
            if (px + width > wa.right) px = parentItem->left - width + S(2);
            if (py + height > wa.bottom) py = wa.bottom - height;
        } else {
            if (flags & TPM_RIGHTALIGN) px -= width;
            else if (flags & TPM_CENTERALIGN) px -= width / 2;
            if (flags & TPM_BOTTOMALIGN) py -= height;
            else if (flags & TPM_VCENTERALIGN) py -= height / 2;
            if (px + width > wa.right) px = wa.right - width;
            if (py + height > wa.bottom) py = std::max((int)wa.top, y - height);
        }
        px = std::max((int)wa.left, px);
        py = std::max((int)wa.top, py);
        if (!Create(L"OShellCppMenu", WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE, WS_POPUP, px, py, width,
                    height, L"", CS_DROPSHADOW))
            return false;
        SetFrame(FrameMenu);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        UpdateWindow(hwnd);
        return true;
    }

    bool Selectable(int i) const { return i >= 0 && i < (int)items.size() && !items[i].sep && !items[i].grayed; }

    // Índice del elemento bajo un punto de pantalla; -1 = dentro pero sin elemento, -2 = fuera.
    int ItemAtScreen(POINT pt) const {
        RECT wr;
        GetWindowRect(hwnd, &wr);
        if (!PtInRect(&wr, pt)) return -2;
        int x = pt.x - wr.left, y = pt.y - wr.top;
        for (int i = 0; i < (int)items.size(); ++i)
            if (!items[i].sep && InRect(rects[i], x, y)) return i;
        return -1;
    }

    RECT ItemScreenRect(int i) const {
        RECT wr;
        GetWindowRect(hwnd, &wr);
        const RectF& r = rects[i];
        return RECT{ wr.left, wr.top + (LONG)r.Y, wr.right, wr.top + (LONG)(r.Y + r.Height) };
    }

    void SetHover(int i) {
        if (i == hover) return;
        if (hover >= 0) InvalidateRectF(rects[hover]);
        hover = i;
        if (i >= 0) InvalidateRectF(rects[i]);
    }

protected:
    void Measure() {
        float textW = 0, keyW = 0;
        HDC dc = GetDC(nullptr);
        {
            Graphics g(dc);
            for (auto& it : items) {
                if (it.sep) continue;
                RectF b;
                g.MeasureString(it.text.c_str(), -1, it.isDefault ? fontBold : fontBody, PointF(0, 0), &b);
                textW = std::max(textW, b.Width);
                if (!it.shortcut.empty()) {
                    g.MeasureString(it.shortcut.c_str(), -1, fontSmall, PointF(0, 0), &b);
                    keyW = std::max(keyW, b.Width);
                }
            }
        }
        ReleaseDC(nullptr, dc);
        width = std::max(S(180), (int)(S(40) + textW + (keyW > 0 ? S(30) + keyW : 0) + S(36)));
        int y = S(4);
        rects.clear();
        for (auto& it : items) {
            int h = it.sep ? S(9) : S(32);
            rects.push_back(RectF((float)S(4), (float)y, (float)width - S(8), (float)h));
            y += h;
        }
        height = y + S(4);
    }

    void OnPaint(Graphics& g, int w, int h) override {
        g.Clear(style::menuBg);
        DrawRounded(g, RectF(0, 0, (float)w - 1, (float)h - 1), SF(6), Color(0, 0, 0, 0), style::menuBorder);
        for (size_t i = 0; i < items.size(); ++i) {
            const MenuEntry& it = items[i];
            const RectF& r = rects[i];
            if (it.sep) {
                HLine(g, r.X + S(8), r.X + r.Width - S(8), r.Y + r.Height / 2, style::separator);
                continue;
            }
            if ((int)i == hover && !it.grayed) DrawRounded(g, r, SF(4), style::menuHover);
            Color fg = it.grayed ? WithAlpha(style::textFaint, 150) : style::text;
            if (it.checked) {
                if (it.radio) FillCircle(g, r.X + S(17), r.Y + r.Height / 2, SF(3.5f), it.grayed ? fg : style::accent);
                else DrawTextStr(g, L"", fontIcon, it.grayed ? fg : style::accent,
                                 RectF(r.X + S(6), r.Y, (float)S(22), r.Height), StringAlignmentCenter);
            }
            DrawTextStr(g, it.text, it.isDefault ? fontBold : fontBody, fg,
                        RectF(r.X + S(34), r.Y, r.Width - S(34) - S(28), r.Height));
            if (!it.shortcut.empty())
                DrawTextStr(g, it.shortcut, fontSmall, style::textFaint, RectF(r.X, r.Y, r.Width - S(26), r.Height),
                            StringAlignmentFar);
            if (it.sub)
                DrawTextStr(g, L"", fontIcon, fg, RectF(r.X + r.Width - S(24), r.Y, (float)S(20), r.Height),
                            StringAlignmentCenter);
        }
    }

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        if (msg == WM_MOUSEACTIVATE) return MA_NOACTIVATE;
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
};

// Misma firma que TrackPopupMenu. Con TPM_RETURNCMD devuelve el id elegido (0 = nada).
inline int ThemedMenu(HMENU menu, UINT flags, int x, int y, int, HWND owner, const RECT* = nullptr) {
    if (!menu || GetMenuItemCount(menu) <= 0) return 0;
    std::vector<std::unique_ptr<MenuPopup>> chain; // [0] = raíz, back() = submenú más profundo
    std::vector<int> openedFrom;                  // índice del elemento del que cuelga cada nivel
    {
        auto root = std::make_unique<MenuPopup>();
        if (!root->Open(menu, x, y, flags, nullptr)) return 0;
        chain.push_back(std::move(root));
        openedFrom.push_back(-1);
    }
    HWND cap = chain[0]->Hwnd();
    SetCapture(cap);
    SetTimer(cap, 77, 150, nullptr);
    HWND fgStart = GetForegroundWindow();

    auto closeFrom = [&](size_t level) {
        while (chain.size() > level) {
            chain.pop_back();
            openedFrom.pop_back();
        }
    };
    auto openSub = [&](size_t level, int idx) -> bool {
        MenuPopup* p = chain[level].get();
        if (!p->Selectable(idx) || !p->items[idx].sub) return false;
        if (chain.size() > level + 1 && openedFrom[level + 1] == idx) return true;
        closeFrom(level + 1);
        RECT r = p->ItemScreenRect(idx);
        auto sub = std::make_unique<MenuPopup>();
        if (!sub->Open(p->items[idx].sub, 0, 0, 0, &r)) return false;
        chain.push_back(std::move(sub));
        openedFrom.push_back(idx);
        return true;
    };
    auto hoverFirst = [](MenuPopup* p) {
        for (int i = 0; i < (int)p->items.size(); ++i)
            if (p->Selectable(i)) { p->SetHover(i); break; }
    };

    int result = 0;
    bool done = false;
    MSG msg;
    while (!done) {
        if (!GetMessageW(&msg, nullptr, 0, 0)) { PostQuitMessage((int)msg.wParam); break; }
        if (GetCapture() != cap) break; // otra ventana tomó el ratón (Alt+Tab, etc.)
        if (msg.message == WM_TIMER && msg.hwnd == cap && msg.wParam == 77) {
            if (GetForegroundWindow() != fgStart) break;
            continue;
        }
        if (msg.message >= WM_MOUSEFIRST && msg.message <= WM_MOUSELAST) {
            int level = -1, idx = -2;
            for (int k = (int)chain.size() - 1; k >= 0; --k) {
                int i = chain[k]->ItemAtScreen(msg.pt);
                if (i != -2) { level = k; idx = i; break; }
            }
            bool inside = level >= 0;
            switch (msg.message) {
            case WM_MOUSEMOVE:
                if (inside) {
                    MenuPopup* p = chain[level].get();
                    p->SetHover(p->Selectable(idx) ? idx : -1);
                    if (p->Selectable(idx) && p->items[idx].sub) openSub(level, idx);
                    else if (idx >= 0 && (int)chain.size() > level + 1) closeFrom(level + 1);
                }
                break;
            case WM_LBUTTONDOWN: case WM_RBUTTONDOWN: case WM_MBUTTONDOWN:
                if (!inside) done = true;
                break;
            case WM_LBUTTONUP: case WM_RBUTTONUP:
                if (inside && chain[level]->Selectable(idx)) {
                    const MenuEntry& it = chain[level]->items[idx];
                    if (it.sub) openSub(level, idx);
                    else { result = (int)it.id; done = true; }
                }
                break;
            }
            continue;
        }
        if (msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN) {
            MenuPopup* p = chain.back().get();
            int n = (int)p->items.size();
            auto move = [&](int dir) {
                int i = p->hover < 0 ? (dir > 0 ? -1 : n) : p->hover;
                for (int k = 0; k < n; ++k) {
                    i = (i + dir + n) % n;
                    if (p->Selectable(i)) { p->SetHover(i); break; }
                }
            };
            switch (msg.wParam) {
            case VK_ESCAPE: if (chain.size() > 1) closeFrom(chain.size() - 1); else done = true; break;
            case VK_MENU: case VK_F10: done = true; break;
            case VK_UP: move(-1); break;
            case VK_DOWN: move(1); break;
            case VK_LEFT: if (chain.size() > 1) closeFrom(chain.size() - 1); break;
            case VK_RIGHT: case VK_RETURN: case VK_SPACE:
                if (p->Selectable(p->hover)) {
                    const MenuEntry& it = p->items[p->hover];
                    if (it.sub) { if (openSub(chain.size() - 1, p->hover)) hoverFirst(chain.back().get()); }
                    else if (msg.wParam != VK_RIGHT) { result = (int)it.id; done = true; }
                }
                break;
            }
            continue;
        }
        if (msg.message == WM_CHAR || msg.message == WM_SYSCHAR || msg.message == WM_KEYUP || msg.message == WM_SYSKEYUP)
            continue;
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    KillTimer(cap, 77);
    if (GetCapture() == cap) ReleaseCapture();
    chain.clear();
    if (result && !(flags & TPM_RETURNCMD) && owner) PostMessageW(owner, WM_COMMAND, result, 0);
    return (flags & TPM_RETURNCMD) ? result : (result ? TRUE : FALSE);
}

// ------------------------------------------------------------ cuadros de mensaje con el tema propio
class MessageDialog : public WindowBase {
public:
    int Run(HWND owner, const std::wstring& msgText, const std::wstring& msgTitle, UINT flags, UINT timeoutSec = 0) {
        remaining = timeoutSec;
        text = msgText;
        title = msgTitle;
        switch (flags & MB_TYPEMASK) {
        case MB_OKCANCEL: btns = { { IDOK, L"Aceptar" }, { IDCANCEL, L"Cancelar" } }; cancelId = IDCANCEL; break;
        case MB_YESNO: btns = { { IDYES, L"Sí" }, { IDNO, L"No" } }; cancelId = IDNO; break;
        case MB_YESNOCANCEL: btns = { { IDYES, L"Sí" }, { IDNO, L"No" }, { IDCANCEL, L"Cancelar" } }; cancelId = IDCANCEL; break;
        case MB_RETRYCANCEL: btns = { { IDRETRY, L"Reintentar" }, { IDCANCEL, L"Cancelar" } }; cancelId = IDCANCEL; break;
        default: btns = { { IDOK, L"Aceptar" } }; cancelId = IDOK; break;
        }
        focus = std::min((int)((flags & MB_DEFMASK) >> 8), (int)btns.size() - 1);
        switch (flags & MB_ICONMASK) {
        case MB_ICONERROR: glyph = L""; beep = MB_ICONERROR; break;
        case MB_ICONQUESTION: glyph = L""; beep = MB_ICONQUESTION; break;
        case MB_ICONWARNING: glyph = L""; beep = MB_ICONWARNING; break;
        case MB_ICONINFORMATION: glyph = L""; beep = MB_ICONINFORMATION; break;
        default: glyph = nullptr; break;
        }
        iconKind = flags & MB_ICONMASK;

        int w = S(470);
        float textW = (float)(w - S(48) - (glyph ? S(48) : 0));
        float textH = 0;
        HDC dc = GetDC(nullptr);
        {
            Graphics g(dc);
            RectF b;
            g.MeasureString(text.c_str(), -1, fontBody, RectF(0, 0, textW, 4000),
                            CachedFormat(StringAlignmentNear, StringAlignmentNear, true), &b);
            textH = b.Height;
        }
        ReleaseDC(nullptr, dc);
        int h = S(62) + std::max((int)textH, glyph ? S(10) : 0) + S(28) + S(62) + (timeoutSec ? S(24) : 0);

        RECT ref = {};
        if (owner && IsWindowVisible(owner) && !IsIconic(owner)) GetWindowRect(owner, &ref);
        else SystemParametersInfoW(SPI_GETWORKAREA, 0, &ref, 0);
        int x = ref.left + ((ref.right - ref.left) - w) / 2;
        int y = ref.top + ((ref.bottom - ref.top) - h) / 2;
        MONITORINFO mi = { sizeof(mi) };
        GetMonitorInfoW(MonitorFromRect(&ref, MONITOR_DEFAULTTONEAREST), &mi);
        x = std::max((int)mi.rcWork.left, std::min(x, (int)mi.rcWork.right - w));
        y = std::max((int)mi.rcWork.top, std::min(y, (int)mi.rcWork.bottom - h));

        DWORD ex = owner ? 0 : (WS_EX_TOPMOST | WS_EX_APPWINDOW);
        if (!Create(L"OShellCppMessage", ex, WS_POPUP, x, y, w, h, title.c_str()))
            return MessageBoxW(owner, text.c_str(), title.c_str(), flags);
        if (owner) SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, (LONG_PTR)owner);
        SetFrame(FrameOpaque);
        bool ownerWasEnabled = owner && IsWindowEnabled(owner);
        if (owner) EnableWindow(owner, FALSE);
        ShowWindow(hwnd, SW_SHOW);
        ForceForeground(hwnd);
        SetFocus(hwnd);
        if (glyph) MessageBeep(beep);
        if (remaining) SetTimer(hwnd, 1, 1000, nullptr);

        MSG msg;
        while (!done && IsWindow(hwnd)) {
            if (!GetMessageW(&msg, nullptr, 0, 0)) { PostQuitMessage((int)msg.wParam); break; }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if (owner) {
            if (ownerWasEnabled) EnableWindow(owner, TRUE);
            SetForegroundWindow(owner);
        }
        Close();
        return result ? result : cancelId;
    }

protected:
    struct Btn {
        int id;
        std::wstring label;
        RectF rc;
    };
    std::wstring text, title;
    std::vector<Btn> btns;
    const wchar_t* glyph = nullptr;
    UINT beep = 0, iconKind = 0;
    int cancelId = IDOK, focus = 0, hoverBtn = -1, result = 0;
    bool done = false;
    UINT remaining = 0; // segundos hasta elegir automáticamente la opción de cancelar

    void Finish(int id) {
        result = id;
        done = true;
    }

    void OnPaint(Graphics& g, int w, int h) override {
        g.Clear(style::dialogBg);
        float barH = (float)S(62);
        FillRect(g, RectF(0, h - barH, (float)w, barH), style::titleBg);
        HLine(g, 0, (float)w, h - barH, style::separator);
        DrawRounded(g, RectF(0, 0, (float)w - 1, (float)h - 1), 0, Color(0, 0, 0, 0), style::cardBorder);

        float tx = (float)S(24);
        if (glyph) {
            Color c = iconKind == MB_ICONERROR ? style::danger : (iconKind == MB_ICONWARNING ? style::warning : style::accent);
            RectF ic((float)S(24), (float)S(22), (float)S(34), (float)S(34));
            DrawRounded(g, ic, ic.Width / 2, WithAlpha(c, style::dark ? 40 : 30));
            DrawTextStr(g, glyph, fontIcon, c, ic, StringAlignmentCenter);
            tx += S(48);
        }
        DrawTextStr(g, title, fontBold, style::text, RectF(tx, (float)S(20), w - tx - S(24), (float)S(26)));
        if (remaining)
            DrawTextStr(g, L"Se revertirá automáticamente en " + std::to_wstring(remaining) + L" s.", fontSmall, style::warning,
                        RectF(tx, h - barH - S(30), w - tx - S(24), (float)S(22)));
        DrawTextV(g, text, fontBody, style::textDim, RectF(tx, (float)S(52), w - tx - S(24), h - barH - S(60)),
                  StringAlignmentNear, true);

        float bw = (float)S(108), bh = (float)S(34);
        float bx = w - S(20) - bw * btns.size() - S(10) * (btns.size() - 1);
        for (size_t i = 0; i < btns.size(); ++i) {
            btns[i].rc = RectF(bx + i * (bw + S(10)), h - barH + (barH - bh) / 2, bw, bh);
            style::DrawButton(g, btns[i].rc, btns[i].label, (int)i == hoverBtn, (int)i == focus, false, false,
                              (int)i == focus);
        }
    }

    void OnMouseMove(int x, int y) override {
        int hv = -1;
        for (size_t i = 0; i < btns.size(); ++i)
            if (InRect(btns[i].rc, x, y)) hv = (int)i;
        if (hv != hoverBtn) { hoverBtn = hv; Invalidate(); }
    }

    void OnMouseDown(int x, int y) override {
        for (auto& b : btns)
            if (InRect(b.rc, x, y)) { Finish(b.id); return; }
    }

    void OnKeyDown(int vk) override {
        int n = (int)btns.size();
        switch (vk) {
        case VK_ESCAPE: Finish(cancelId); break;
        case VK_RETURN: case VK_SPACE: Finish(btns[focus].id); break;
        case VK_TAB: focus = (focus + (GetKeyState(VK_SHIFT) < 0 ? n - 1 : 1)) % n; Invalidate(); break;
        case VK_LEFT: focus = (focus + n - 1) % n; Invalidate(); break;
        case VK_RIGHT: focus = (focus + 1) % n; Invalidate(); break;
        case 'S': case 'Y': for (auto& b : btns) if (b.id == IDYES) Finish(IDYES); break;
        case 'N': for (auto& b : btns) if (b.id == IDNO) Finish(IDNO); break;
        case 'C':
            if (GetKeyState(VK_CONTROL) < 0) CopyTextToClipboard(hwnd, title + L"\r\n\r\n" + text);
            break;
        }
    }

    LRESULT OnHitTest(int x, int y) override {
        for (auto& b : btns)
            if (InRect(b.rc, x, y)) return HTCLIENT;
        return y < S(50) ? HTCAPTION : 0;
    }

    void OnTimer(UINT_PTR) override {
        if (remaining && --remaining == 0) Finish(cancelId);
        Invalidate();
    }

    bool CanClose() override {
        Finish(cancelId);
        return false;
    }
};

// Misma firma y valores de retorno que MessageBox, con el tema propio.
inline int MsgBox(HWND owner, const std::wstring& text, const std::wstring& title = L"OptimizedShell", UINT flags = MB_OK,
                  UINT timeoutSec = 0) {
    if (!fontBody) return MessageBoxW(owner, text.c_str(), title.c_str(), flags); // UI sin inicializar
    MessageDialog d;
    return d.Run(owner, text, title, flags, timeoutSec);
}

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
