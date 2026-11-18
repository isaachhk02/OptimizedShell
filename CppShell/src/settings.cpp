// settings.cpp - Configuración de OptimizedShell.
// Se ejecuta con: OptimizedShellCpp.exe --settings [personalizacion|fondo|pantalla|bluetooth]
//
// Páginas:
//  - Personalización: modo claro/oscuro, color de acento y transparencia. Se guarda en
//    %APPDATA%\OptimizedShell\theme.ini y se aplica al momento en todas las ventanas.
//  - Fondo de escritorio: galería de imágenes (Windows + Imágenes), "Examinar..." y ajuste.
//  - Pantalla: resolución, frecuencia y orientación (con confirmación y vuelta atrás a los
//    15 s), escala y brillo de la pantalla integrada.
//  - Bluetooth: encender/apagar la radio, dispositivos emparejados, agregar y quitar.
// Todo lo lento (miniaturas, WMI, Bluetooth) se hace en hilos de trabajo.
#include "ui.h"
#include "winrt_bridge.h"

#include <shobjidl.h>
#include <wbemidl.h>
#include <oleauto.h>
#include <thread>
#include <map>
#include <set>

#pragma comment(lib, "wbemuuid.lib")
#pragma comment(lib, "oleaut32.lib")

int RunSettingsMode();

namespace {

using namespace Gdiplus;
using namespace ui;

const UINT WM_THUMB = WM_APP + 50;    // lParam = ThumbResult*
const UINT WM_BTSTATE = WM_APP + 51;  // lParam = BtState*
const UINT WM_BRIGHT = WM_APP + 52;   // wParam = brillo (-1 = no disponible)
const UINT WM_WALLDONE = WM_APP + 53;
const UINT WM_BTDONE = WM_APP + 54;   // wParam = 1 si la operación fue bien
const UINT WM_GOTOPAGE = WM_APP + 90; // wParam = página

enum Page { PgTheme = 0, PgWallpaper, PgDisplay, PgBluetooth, PgCount };

int PageFromName(const std::wstring& n) {
    std::wstring s = ToLower(n);
    if (s == L"fondo" || s == L"wallpaper") return PgWallpaper;
    if (s == L"pantalla" || s == L"display") return PgDisplay;
    if (s == L"bluetooth") return PgBluetooth;
    return PgTheme;
}

constexpr int kPresetCount = (int)(sizeof(kAccentPresets) / sizeof(kAccentPresets[0]));

// ---------------------------------------------------------------- brillo (WMI)
// Solo pantallas integradas (portátiles): WmiMonitorBrightness en ROOT\WMI.
struct ComInit {
    HRESULT hr;
    ComInit() {
        hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
        // Seguridad por defecto del proceso; si ya estaba fijada se ignora.
        CoInitializeSecurity(nullptr, -1, nullptr, nullptr, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
                             nullptr, EOAC_NONE, nullptr);
    }
    ~ComInit() {
        if (SUCCEEDED(hr)) CoUninitialize();
    }
};

IWbemServices* ConnectWmi() {
    IWbemLocator* loc = nullptr;
    if (FAILED(CoCreateInstance(CLSID_WbemLocator, nullptr, CLSCTX_INPROC_SERVER, IID_IWbemLocator, (void**)&loc)))
        return nullptr;
    IWbemServices* svc = nullptr;
    BSTR ns = SysAllocString(L"ROOT\\WMI");
    HRESULT hr = loc->ConnectServer(ns, nullptr, nullptr, nullptr, 0, nullptr, nullptr, &svc);
    SysFreeString(ns);
    loc->Release();
    if (FAILED(hr)) return nullptr;
    CoSetProxyBlanket(svc, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, nullptr, RPC_C_AUTHN_LEVEL_CALL,
                      RPC_C_IMP_LEVEL_IMPERSONATE, nullptr, EOAC_NONE);
    return svc;
}

IWbemClassObject* WmiFirst(IWbemServices* svc, const wchar_t* query) {
    IEnumWbemClassObject* en = nullptr;
    BSTR lang = SysAllocString(L"WQL");
    BSTR q = SysAllocString(query);
    IWbemClassObject* obj = nullptr;
    if (SUCCEEDED(svc->ExecQuery(lang, q, WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, nullptr, &en)) && en) {
        ULONG n = 0;
        if (en->Next(5000, 1, &obj, &n) != S_OK || !n) obj = nullptr;
        en->Release();
    }
    SysFreeString(lang);
    SysFreeString(q);
    return obj;
}

int WmiGetBrightness() {
    ComInit com;
    IWbemServices* svc = ConnectWmi();
    if (!svc) return -1;
    int val = -1;
    if (IWbemClassObject* o = WmiFirst(svc, L"SELECT CurrentBrightness FROM WmiMonitorBrightness WHERE Active=TRUE")) {
        VARIANT v;
        VariantInit(&v);
        if (SUCCEEDED(o->Get(L"CurrentBrightness", 0, &v, nullptr, nullptr)))
            val = v.vt == VT_UI1 ? v.bVal : (v.vt == VT_I4 ? v.lVal : -1);
        VariantClear(&v);
        o->Release();
    }
    svc->Release();
    return val;
}

bool WmiSetBrightness(int pct) {
    ComInit com;
    IWbemServices* svc = ConnectWmi();
    if (!svc) return false;
    bool ok = false;
    if (IWbemClassObject* o = WmiFirst(svc, L"SELECT * FROM WmiMonitorBrightnessMethods WHERE Active=TRUE")) {
        VARIANT path;
        VariantInit(&path);
        IWbemClassObject* cls = nullptr;
        IWbemClassObject* inDef = nullptr;
        IWbemClassObject* in = nullptr;
        BSTR clsName = SysAllocString(L"WmiMonitorBrightnessMethods");
        BSTR method = SysAllocString(L"WmiSetBrightness");
        if (SUCCEEDED(o->Get(L"__PATH", 0, &path, nullptr, nullptr)) && path.vt == VT_BSTR &&
            SUCCEEDED(svc->GetObject(clsName, 0, nullptr, &cls, nullptr)) &&
            SUCCEEDED(cls->GetMethod(L"WmiSetBrightness", 0, &inDef, nullptr)) && inDef &&
            SUCCEEDED(inDef->SpawnInstance(0, &in))) {
            VARIANT t;
            VariantInit(&t);
            t.vt = VT_I4;
            t.lVal = 0;
            VARIANT b;
            VariantInit(&b);
            b.vt = VT_UI1;
            b.bVal = (BYTE)std::max(0, std::min(100, pct));
            in->Put(L"Timeout", 0, &t, 0);
            in->Put(L"Brightness", 0, &b, 0);
            ok = SUCCEEDED(svc->ExecMethod(path.bstrVal, method, 0, nullptr, in, nullptr, nullptr));
        }
        if (in) in->Release();
        if (inDef) inDef->Release();
        if (cls) cls->Release();
        SysFreeString(clsName);
        SysFreeString(method);
        VariantClear(&path);
        o->Release();
    }
    svc->Release();
    return ok;
}

// ---------------------------------------------------------------- pantallas
struct DisplayInfo {
    std::wstring device; // \\.\DISPLAY1
    std::wstring name;   // nombre del monitor
    DEVMODEW mode = {};  // modo actual
    bool primary = false;
};

struct Mode {
    DWORD w, h, hz;
};

std::map<std::wstring, std::wstring> MonitorFriendlyNames() {
    std::map<std::wstring, std::wstring> out;
    UINT32 np = 0, nm = 0;
    if (GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &np, &nm) != ERROR_SUCCESS) return out;
    std::vector<DISPLAYCONFIG_PATH_INFO> paths(np);
    std::vector<DISPLAYCONFIG_MODE_INFO> modes(nm);
    if (QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &np, paths.data(), &nm, modes.data(), nullptr) != ERROR_SUCCESS)
        return out;
    for (UINT32 i = 0; i < np; ++i) {
        DISPLAYCONFIG_SOURCE_DEVICE_NAME src = {};
        src.header.type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        src.header.size = sizeof(src);
        src.header.adapterId = paths[i].sourceInfo.adapterId;
        src.header.id = paths[i].sourceInfo.id;
        DISPLAYCONFIG_TARGET_DEVICE_NAME tgt = {};
        tgt.header.type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        tgt.header.size = sizeof(tgt);
        tgt.header.adapterId = paths[i].targetInfo.adapterId;
        tgt.header.id = paths[i].targetInfo.id;
        if (DisplayConfigGetDeviceInfo(&src.header) == ERROR_SUCCESS &&
            DisplayConfigGetDeviceInfo(&tgt.header) == ERROR_SUCCESS) {
            std::wstring name = tgt.monitorFriendlyDeviceName;
            if (name.empty())
                name = tgt.outputTechnology == DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL ||
                               tgt.outputTechnology == DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED ||
                               tgt.outputTechnology == DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED
                           ? L"Pantalla integrada"
                           : L"Pantalla";
            out[src.viewGdiDeviceName] = name;
        }
    }
    return out;
}

std::vector<DisplayInfo> EnumerateDisplays() {
    std::vector<DisplayInfo> out;
    auto names = MonitorFriendlyNames();
    DISPLAY_DEVICEW dd = { sizeof(dd) };
    for (DWORD i = 0; EnumDisplayDevicesW(nullptr, i, &dd, 0); ++i, dd = { sizeof(dd) }) {
        if (!(dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP)) continue;
        DisplayInfo d;
        d.device = dd.DeviceName;
        d.primary = (dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE) != 0;
        d.mode.dmSize = sizeof(DEVMODEW);
        if (!EnumDisplaySettingsExW(dd.DeviceName, ENUM_CURRENT_SETTINGS, &d.mode, 0)) continue;
        auto f = names.find(d.device);
        d.name = f != names.end() ? f->second : std::wstring(dd.DeviceString);
        out.push_back(d);
    }
    std::sort(out.begin(), out.end(), [](const DisplayInfo& a, const DisplayInfo& b) { return a.primary > b.primary; });
    return out;
}

bool Portrait(DWORD orientation) { return orientation == DMDO_90 || orientation == DMDO_270; }

// Modos disponibles en orientación horizontal (ancho >= alto), 32 bpp.
std::vector<Mode> ModesOf(const std::wstring& device) {
    std::vector<Mode> out;
    DEVMODEW dm = { };
    dm.dmSize = sizeof(dm);
    for (DWORD i = 0; EnumDisplaySettingsExW(device.c_str(), i, &dm, 0); ++i) {
        if (dm.dmBitsPerPel != 32) continue;
        Mode m = { dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency };
        bool dup = false;
        for (auto& o : out)
            if (o.w == m.w && o.h == m.h && o.hz == m.hz) dup = true;
        if (!dup) out.push_back(m);
    }
    std::sort(out.begin(), out.end(), [](const Mode& a, const Mode& b) {
        if (a.w * a.h != b.w * b.h) return a.w * a.h > b.w * b.h;
        return a.hz > b.hz;
    });
    return out;
}

// ---------------------------------------------------------------- fondo de escritorio
struct FitMode {
    const wchar_t* label;
    int style;
    int tile;
};

const FitMode kFits[] = {
    { L"Rellenar", 10, 0 }, { L"Ajustar", 6, 0 }, { L"Expandir", 2, 0 },
    { L"Mosaico", 0, 1 },   { L"Centro", 0, 0 },  { L"Extender", 22, 0 },
};
constexpr int kFitCount = 6;

int CurrentFit() {
    int st = _wtoi(RegString(HKEY_CURRENT_USER, L"Control Panel\\Desktop", L"WallpaperStyle").c_str());
    int tile = _wtoi(RegString(HKEY_CURRENT_USER, L"Control Panel\\Desktop", L"TileWallpaper").c_str());
    for (int i = 0; i < kFitCount; ++i)
        if (kFits[i].style == st && kFits[i].tile == tile) return i;
    return 0;
}

bool IsImage(const std::wstring& name) {
    for (const wchar_t* e : { L".jpg", L".jpeg", L".png", L".bmp", L".jfif", L".webp" })
        if (EndsWithNoCase(name, e)) return true;
    return false;
}

void CollectImages(std::vector<std::wstring>& out, const std::wstring& dir, int depth, size_t limit) {
    WIN32_FIND_DATAW fd;
    HANDLE h = FindFirstFileExW((dir + L"\\*").c_str(), FindExInfoBasic, &fd, FindExSearchNameMatch, nullptr, 0);
    if (h == INVALID_HANDLE_VALUE) return;
    std::vector<std::wstring> subdirs;
    do {
        if (fd.cFileName[0] == L'.') continue;
        std::wstring full = dir + L"\\" + fd.cFileName;
        if (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) subdirs.push_back(full);
        else if (IsImage(fd.cFileName) && out.size() < limit) out.push_back(full);
    } while (FindNextFileW(h, &fd));
    FindClose(h);
    if (depth > 0)
        for (auto& s : subdirs) CollectImages(out, s, depth - 1, limit);
}

struct ThumbResult {
    std::wstring path;
    HBITMAP bmp = nullptr;
};

// Miniaturas en un hilo aparte (caché de miniaturas de Windows vía IShellItemImageFactory).
void LoadThumbs(HWND target, std::vector<std::wstring> paths, SIZE size) {
    std::thread([target, paths, size] {
        CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
        SetThreadEco();
        for (auto& p : paths) {
            auto* r = new ThumbResult{ p, nullptr };
            IShellItemImageFactory* f = nullptr;
            if (SUCCEEDED(SHCreateItemFromParsingName(p.c_str(), nullptr, IID_PPV_ARGS(&f)))) {
                f->GetImage(size, SIIGBF_BIGGERSIZEOK, &r->bmp);
                f->Release();
            }
            if (!PostMessageW(target, WM_THUMB, 0, (LPARAM)r)) {
                if (r->bmp) DeleteObject(r->bmp);
                delete r;
                break; // la ventana se cerró
            }
        }
        CoUninitialize();
    }).detach();
}

// Dibuja una imagen recortada para cubrir el rectángulo (como "Rellenar").
void DrawCover(Graphics& g, Bitmap* b, const RectF& r) {
    if (!b) return;
    float iw = (float)b->GetWidth(), ih = (float)b->GetHeight();
    if (iw <= 0 || ih <= 0) return;
    float scale = std::max(r.Width / iw, r.Height / ih);
    float sw = r.Width / scale, sh = r.Height / scale;
    g.DrawImage(b, r, (iw - sw) / 2, (ih - sh) / 2, sw, sh, UnitPixel);
}

// ---------------------------------------------------------------- Bluetooth
struct BtState {
    int radio = -1;
    std::vector<wrt::BtDevice> devices;
};

void LoadBluetooth(HWND target) {
    std::thread([target] {
        auto* st = new BtState();
        st->radio = wrt::BluetoothRadioState();
        if (st->radio == 1) st->devices = wrt::PairedBluetoothDevices();
        if (!PostMessageW(target, WM_BTSTATE, 0, (LPARAM)st)) delete st;
    }).detach();
}

const wchar_t* BtGlyph(const std::wstring& kind) {
    if (kind == L"audio") return L"\uE7F6";
    if (kind == L"input") return L"\uE765";
    if (kind == L"phone") return L"\uE8EA";
    if (kind == L"computer") return L"\uE7F8";
    return L"\uE702";
}

// ---------------------------------------------------------------- ventana
enum Hit {
    HNone = 0, HMin, HClose,
    HNav0 = 20,
    HDark = 30, HLight, HWindowsAccent, HBlur, HTaskOpacity, HWinOpacity, HReset,
    HBrowse = 40, HFit0 = 41,
    HMonitor0 = 50, HRes = 60, HHz, HOrient, HScale, HBright, HAdvanced,
    HBtSwitch = 70, HBtAdd, HBtMore, HBtRefresh,
    HSwatch0 = 100,
    HWall0 = 1000,     // + índice de imagen
    HBtRemove0 = 3000, // + índice de dispositivo
};

class SettingsWindow : public WindowBase {
public:
    bool Start(int startPage) {
        customFrame = true;
        page = startPage;
        int w = S(940), h = S(680);
        RECT wa = {};
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &wa, 0);
        w = std::min(w, (int)(wa.right - wa.left));
        h = std::min(h, (int)(wa.bottom - wa.top));
        int x = wa.left + (wa.right - wa.left - w) / 2;
        int y = wa.top + (wa.bottom - wa.top - h) / 2;
        if (!Create(L"OShellCppSettings", WS_EX_APPWINDOW, WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX, x, y,
                    w, h, L"Configuración"))
            return false;
        SetFrame(FrameOpaque);
        SetWindowPos(hwnd, nullptr, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        tip.Create(hwnd);
        RECT rc;
        GetClientRect(hwnd, &rc);
        Layout(rc.right, rc.bottom);
        EnterPage(page);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        ForceForeground(hwnd);
        return true;
    }

    ~SettingsWindow() override {
        for (auto& kv : thumbs) delete kv.second;
    }

protected:
    int page = PgTheme;
    int hover = HNone;
    HoverTip tip;
    std::map<int, RectF> rc; // rectángulos fijos por id
    float x0 = 0, cw = 0;    // zona de contenido
    int winW = 0, winH = 0;

    // Opacidad (barra de tareas y ventanas) con deslizadores arrastrables.
    RectF taskOpTrack, winOpTrack;
    int dragOpacity = 0; // 0 ninguno, 1 barra, 2 ventanas
    int liveOpacity = 0; // valor mientras se arrastra
    static constexpr int kCaption = 42;

    // Fondo
    std::vector<std::wstring> wallpapers;
    std::map<std::wstring, Bitmap*> thumbs;
    std::set<std::wstring> thumbRequested;
    std::wstring currentWall;
    int currentFit = 0;
    RectF galleryRc, previewRc;
    float wallScroll = 0;
    bool wallsLoaded = false;
    bool applyingWall = false;

    // Pantalla
    std::vector<DisplayInfo> displays;
    int displaySel = 0;
    std::vector<Mode> modes;
    int brightness = -2; // -2 cargando, -1 no disponible
    bool dragBright = false;
    RectF brightTrack;

    // Bluetooth
    BtState bt;
    bool btLoading = false, btBusy = false;
    std::vector<RectF> btRows;

    // ------------------------------------------------------------ layout
    void Layout(int w, int h) {
        winW = w;
        winH = h;
        auto ch = style::ChromeRects(w);
        rc[HMin] = ch.min;
        rc[HClose] = ch.close;
        for (int i = 0; i < PgCount; ++i)
            rc[HNav0 + i] = RectF((float)S(10), (float)S(58) + i * S(42), (float)S(206), (float)S(38));
        x0 = (float)S(250);
        cw = w - x0 - S(28);

        // Personalización
        float cardW = (cw - S(16)) / 2;
        rc[HDark] = RectF(x0, (float)S(126), cardW, (float)S(156));
        rc[HLight] = RectF(x0 + cardW + S(16), (float)S(126), cardW, (float)S(156));
        float sw = (float)S(34), gap = (float)S(10);
        for (int i = 0; i < kPresetCount; ++i) rc[HSwatch0 + i] = RectF(x0 + i * (sw + gap), (float)S(330), sw, sw);
        rc[HWindowsAccent] = RectF(x0, (float)S(380), (float)S(236), (float)S(34));
        rc[HBlur] = RectF(x0, (float)S(446), cw, (float)S(56));
        rc[HTaskOpacity] = RectF(x0, (float)S(510), cw, (float)S(46));
        rc[HWinOpacity] = RectF(x0, (float)S(562), cw, (float)S(46));
        taskOpTrack = RectF(rc[HTaskOpacity].X + S(16), rc[HTaskOpacity].Y + S(30), rc[HTaskOpacity].Width - S(32), (float)S(6));
        winOpTrack = RectF(rc[HWinOpacity].X + S(16), rc[HWinOpacity].Y + S(30), rc[HWinOpacity].Width - S(32), (float)S(6));
        rc[HReset] = RectF(x0 + cw - S(150), (float)h - S(50), (float)S(150), (float)S(34));

        // Fondo
        previewRc = RectF(x0, (float)S(110), (float)S(320), (float)S(180));
        float fx = previewRc.X + previewRc.Width + S(24), fw = (cw - previewRc.Width - S(24) - S(16)) / 3;
        for (int i = 0; i < kFitCount; ++i)
            rc[HFit0 + i] = RectF(fx + (i % 3) * (fw + S(8)), (float)S(136) + (i / 3) * S(42), fw, (float)S(34));
        rc[HBrowse] = RectF(fx, (float)S(240), (float)S(170), (float)S(34));
        galleryRc = RectF(x0, (float)S(372), cw, h - S(372) - S(16));

        // Pantalla
        float my = (float)S(108);
        for (int i = 0; i < 8; ++i) rc[HMonitor0 + i] = RectF(x0 + i * S(176), my, (float)S(168), (float)S(40));
        float ry = displays.size() > 1 ? (float)S(164) : (float)S(112);
        for (int id : { HRes, HHz, HOrient, HScale }) {
            rc[id] = RectF(x0, ry, cw, (float)S(56));
            ry += S(64);
        }
        rc[HBright] = RectF(x0, ry, cw, (float)S(72));
        brightTrack = RectF(x0 + S(160), ry + S(32), cw - S(240), (float)S(8));
        rc[HAdvanced] = RectF(x0, ry + S(96), (float)S(300), (float)S(34));

        // Bluetooth
        rc[HBtSwitch] = RectF(x0, (float)S(108), cw, (float)S(72));
        rc[HBtAdd] = RectF(x0, (float)S(196), (float)S(190), (float)S(34));
        rc[HBtMore] = RectF(x0 + S(200), (float)S(196), (float)S(210), (float)S(34));
        rc[HBtRefresh] = RectF(x0 + cw - S(130), (float)S(196), (float)S(130), (float)S(34));
    }

    void OnSize(UINT, int w, int h) override { Layout(w, h); }

    // ------------------------------------------------------------ páginas
    void EnterPage(int p) {
        page = p;
        hover = HNone;
        KillTimer(hwnd, 1);
        switch (page) {
        case PgWallpaper: LoadWallpaperPage(); break;
        case PgDisplay: LoadDisplayPage(); break;
        case PgBluetooth:
            RefreshBluetooth();
            SetTimer(hwnd, 1, 6000, nullptr); // estado de conexión, solo mientras se ve la página
            break;
        }
        const wchar_t* titles[PgCount] = { L"Personalización", L"Fondo de escritorio", L"Pantalla", L"Bluetooth" };
        SetWindowTextW(hwnd, (std::wstring(titles[page]) + L" — Configuración").c_str());
        Invalidate();
    }

    void LoadWallpaperPage() {
        currentWall = GetWallpaperPath();
        currentFit = CurrentFit();
        if (!wallsLoaded) {
            wallsLoaded = true;
            wchar_t win[MAX_PATH] = {};
            GetWindowsDirectoryW(win, MAX_PATH);
            CollectImages(wallpapers, std::wstring(win) + L"\\Web\\Wallpaper", 1, 60);
            PWSTR pics = nullptr;
            if (SUCCEEDED(SHGetKnownFolderPath(FOLDERID_Pictures, 0, nullptr, &pics))) {
                CollectImages(wallpapers, pics, 1, 120);
                CoTaskMemFree(pics);
            }
        }
        std::vector<std::wstring> need;
        if (!currentWall.empty() && !thumbRequested.count(currentWall)) need.push_back(currentWall);
        for (auto& p : wallpapers)
            if (!thumbRequested.count(p)) need.push_back(p);
        for (auto& p : need) thumbRequested.insert(p);
        if (!need.empty()) LoadThumbs(hwnd, need, SIZE{ S(320), S(180) });
    }

    void LoadDisplayPage() {
        displays = EnumerateDisplays();
        if (displaySel >= (int)displays.size()) displaySel = 0;
        modes = displays.empty() ? std::vector<Mode>() : ModesOf(displays[displaySel].device);
        RECT r;
        GetClientRect(hwnd, &r);
        Layout(r.right, r.bottom);
        if (brightness == -2) {
            HWND target = hwnd;
            std::thread([target] { PostMessageW(target, WM_BRIGHT, (WPARAM)WmiGetBrightness(), 0); }).detach();
        }
    }

    void RefreshBluetooth() {
        if (btLoading) return;
        btLoading = true;
        LoadBluetooth(hwnd);
    }

    // ------------------------------------------------------------ acciones
    void ApplyTheme_(const ThemeSettings& t) {
        SetTheme(t);
        Invalidate();
    }

    void SetWallpaper(const std::wstring& path, int fit) {
        if (applyingWall) return;
        applyingWall = true;
        currentWall = path;
        currentFit = fit;
        Invalidate();
        HWND target = hwnd;
        int st = kFits[fit].style, tile = kFits[fit].tile;
        // SPI_SETDESKWALLPAPER avisa a todas las ventanas y puede tardar: en un hilo.
        std::thread([target, path, st, tile] {
            std::wstring s = std::to_wstring(st), t = std::to_wstring(tile);
            RegSetKeyValueW(HKEY_CURRENT_USER, L"Control Panel\\Desktop", L"WallpaperStyle", REG_SZ, s.c_str(),
                            (DWORD)((s.size() + 1) * sizeof(wchar_t)));
            RegSetKeyValueW(HKEY_CURRENT_USER, L"Control Panel\\Desktop", L"TileWallpaper", REG_SZ, t.c_str(),
                            (DWORD)((t.size() + 1) * sizeof(wchar_t)));
            SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, (PVOID)path.c_str(), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
            PostMessageW(target, WM_WALLDONE, 0, 0);
        }).detach();
    }

    void BrowseWallpaper() {
        IFileOpenDialog* dlg = nullptr;
        if (FAILED(CoCreateInstance(CLSID_FileOpenDialog, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&dlg)))) return;
        COMDLG_FILTERSPEC filters[] = { { L"Imágenes", L"*.jpg;*.jpeg;*.png;*.bmp;*.jfif;*.webp" },
                                        { L"Todos los archivos", L"*.*" } };
        dlg->SetFileTypes(2, filters);
        dlg->SetTitle(L"Elegir fondo de escritorio");
        PWSTR pics = nullptr;
        if (SUCCEEDED(SHGetKnownFolderPath(FOLDERID_Pictures, 0, nullptr, &pics))) {
            IShellItem* folder = nullptr;
            if (SUCCEEDED(SHCreateItemFromParsingName(pics, nullptr, IID_PPV_ARGS(&folder)))) {
                dlg->SetFolder(folder);
                folder->Release();
            }
            CoTaskMemFree(pics);
        }
        if (SUCCEEDED(dlg->Show(hwnd))) {
            IShellItem* item = nullptr;
            if (SUCCEEDED(dlg->GetResult(&item))) {
                PWSTR path = nullptr;
                if (SUCCEEDED(item->GetDisplayName(SIGDN_FILESYSPATH, &path))) {
                    std::wstring p = path;
                    CoTaskMemFree(path);
                    if (std::find(wallpapers.begin(), wallpapers.end(), p) == wallpapers.end())
                        wallpapers.insert(wallpapers.begin(), p);
                    if (!thumbRequested.count(p)) {
                        thumbRequested.insert(p);
                        LoadThumbs(hwnd, { p }, SIZE{ S(320), S(180) });
                    }
                    SetWallpaper(p, currentFit);
                }
                item->Release();
            }
        }
        dlg->Release();
    }

    // Cambia el modo de la pantalla seleccionada y pide confirmación; si no se confirma
    // en 15 s (p. ej. la pantalla se quedó en negro) se vuelve al modo anterior.
    void ApplyMode(DEVMODEW dm) {
        if (displays.empty()) return;
        const DisplayInfo& d = displays[displaySel];
        DEVMODEW old = d.mode;
        std::wstring dev = d.device;
        LONG r = ChangeDisplaySettingsExW(dev.c_str(), &dm, nullptr, CDS_TEST, nullptr);
        if (r == DISP_CHANGE_SUCCESSFUL) r = ChangeDisplaySettingsExW(dev.c_str(), &dm, nullptr, CDS_UPDATEREGISTRY, nullptr);
        if (r != DISP_CHANGE_SUCCESSFUL) {
            MsgBox(hwnd, L"La pantalla no admite esta configuración.", L"Pantalla", MB_ICONWARNING);
            return;
        }
        LoadDisplayPage();
        Invalidate();
        UpdateWindow(hwnd);
        int answer = MsgBox(hwnd, L"¿Quieres mantener esta configuración de pantalla?", L"Pantalla",
                            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2, 15);
        if (answer != IDYES) {
            old.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY | DM_DISPLAYORIENTATION;
            ChangeDisplaySettingsExW(dev.c_str(), &old, nullptr, CDS_UPDATEREGISTRY, nullptr);
        }
        LoadDisplayPage();
        Invalidate();
    }

    void ResolutionMenu() {
        if (displays.empty()) return;
        const DEVMODEW& cur = displays[displaySel].mode;
        bool portrait = Portrait(cur.dmDisplayOrientation);
        DWORD cw_ = portrait ? cur.dmPelsHeight : cur.dmPelsWidth, ch_ = portrait ? cur.dmPelsWidth : cur.dmPelsHeight;
        std::vector<std::pair<DWORD, DWORD>> res;
        for (auto& m : modes)
            if (std::find(res.begin(), res.end(), std::make_pair(m.w, m.h)) == res.end()) res.push_back({ m.w, m.h });
        HMENU menu = CreatePopupMenu();
        for (size_t i = 0; i < res.size() && i < 40; ++i) {
            std::wstring label = std::to_wstring(res[i].first) + L" × " + std::to_wstring(res[i].second) +
                                 (i == 0 ? L"  (recomendada)" : L"");
            AppendMenuW(menu, MF_STRING | (res[i].first == cw_ && res[i].second == ch_ ? MF_CHECKED : 0), 1 + i, label.c_str());
        }
        int cmd = PopupAt(menu, rc[HRes]);
        if (cmd <= 0) return;
        auto sel = res[cmd - 1];
        // La mejor frecuencia disponible para esa resolución.
        DWORD hz = 0;
        for (auto& m : modes)
            if (m.w == sel.first && m.h == sel.second) hz = std::max(hz, m.hz);
        DEVMODEW dm = cur;
        dm.dmPelsWidth = portrait ? sel.second : sel.first;
        dm.dmPelsHeight = portrait ? sel.first : sel.second;
        dm.dmDisplayFrequency = hz;
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        ApplyMode(dm);
    }

    void RefreshMenu() {
        if (displays.empty()) return;
        const DEVMODEW& cur = displays[displaySel].mode;
        bool portrait = Portrait(cur.dmDisplayOrientation);
        DWORD cw_ = portrait ? cur.dmPelsHeight : cur.dmPelsWidth, ch_ = portrait ? cur.dmPelsWidth : cur.dmPelsHeight;
        std::vector<DWORD> rates;
        for (auto& m : modes)
            if (m.w == cw_ && m.h == ch_ && std::find(rates.begin(), rates.end(), m.hz) == rates.end()) rates.push_back(m.hz);
        HMENU menu = CreatePopupMenu();
        for (size_t i = 0; i < rates.size(); ++i)
            AppendMenuW(menu, MF_STRING | (rates[i] == cur.dmDisplayFrequency ? MF_CHECKED : 0), 1 + i,
                        (std::to_wstring(rates[i]) + L" Hz").c_str());
        int cmd = PopupAt(menu, rc[HHz]);
        if (cmd <= 0) return;
        DEVMODEW dm = cur;
        dm.dmDisplayFrequency = rates[cmd - 1];
        dm.dmFields = DM_DISPLAYFREQUENCY;
        ApplyMode(dm);
    }

    void OrientationMenu() {
        if (displays.empty()) return;
        const DEVMODEW& cur = displays[displaySel].mode;
        const wchar_t* labels[4] = { L"Horizontal", L"Vertical", L"Horizontal (volteada)", L"Vertical (volteada)" };
        HMENU menu = CreatePopupMenu();
        for (int i = 0; i < 4; ++i)
            AppendMenuW(menu, MF_STRING | (cur.dmDisplayOrientation == (DWORD)i ? MF_CHECKED : 0), 1 + i, labels[i]);
        int cmd = PopupAt(menu, rc[HOrient]);
        if (cmd <= 0 || (DWORD)(cmd - 1) == cur.dmDisplayOrientation) return;
        DEVMODEW dm = cur;
        DWORD o = (DWORD)(cmd - 1);
        if (Portrait(o) != Portrait(cur.dmDisplayOrientation)) std::swap(dm.dmPelsWidth, dm.dmPelsHeight);
        dm.dmDisplayOrientation = o;
        dm.dmFields = DM_DISPLAYORIENTATION | DM_PELSWIDTH | DM_PELSHEIGHT;
        ApplyMode(dm);
    }

    int PopupAt(HMENU menu, const RectF& anchor) {
        POINT pt = { (LONG)(anchor.X + anchor.Width - S(260)), (LONG)(anchor.Y + anchor.Height - S(6)) };
        ClientToScreen(hwnd, &pt);
        int cmd = ThemedMenu(menu, TPM_RETURNCMD, pt.x, pt.y, 0, hwnd);
        DestroyMenu(menu);
        return cmd;
    }

    void SetBrightnessFromX(int x) {
        float t = (x - brightTrack.X) / brightTrack.Width;
        brightness = (int)std::round(std::max(0.0f, std::min(1.0f, t)) * 100);
        InvalidateRectF(rc[HBright]);
    }

    void CommitBrightness() {
        int v = brightness;
        if (v < 0) return;
        std::thread([v] { WmiSetBrightness(v); }).detach();
    }

    void ToggleBluetooth() {
        if (btBusy || bt.radio < 0) return;
        btBusy = true;
        bool on = bt.radio != 1;
        HWND target = hwnd;
        Invalidate();
        std::thread([target, on] {
            bool ok = wrt::SetBluetoothRadio(on);
            PostMessageW(target, WM_BTDONE, ok ? 1 : 0, 0);
        }).detach();
    }

    void RemoveDevice(int i) {
        if (btBusy || i < 0 || i >= (int)bt.devices.size()) return;
        wrt::BtDevice d = bt.devices[i];
        if (MsgBox(hwnd, L"¿Quitar \"" + d.name + L"\"? Tendrás que volver a emparejarlo para usarlo.", L"Bluetooth",
                   MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2) != IDYES)
            return;
        btBusy = true;
        Invalidate();
        HWND target = hwnd;
        std::thread([target, d] {
            bool ok = wrt::UnpairBluetooth(d.id);
            PostMessageW(target, WM_BTDONE, ok ? 1 : 0, 1);
        }).detach();
    }

    void AddDevice() {
        // Asistente clásico de Windows para emparejar; si no existe, la página de Configuración.
        if (!RunCommandLine(L"DevicePairingWizard.exe", hwnd)) Launch(L"ms-settings:bluetooth");
    }

    // ------------------------------------------------------------ pintado
    void SectionTitle(Graphics& g, const wchar_t* t, float y) {
        DrawTextStr(g, t, fontBold, style::text, RectF(x0, y, cw, (float)S(24)));
    }

    void PageTitle(Graphics& g, const wchar_t* t) {
        DrawTextStr(g, t, fontTitle, style::text, RectF(x0, (float)S(56), cw, (float)S(36)));
    }

    void Card(Graphics& g, const RectF& r, bool hov) {
        DrawRounded(g, r, SF(8), hov ? style::controlHover : style::cardBg, style::controlBorder);
    }

    int OpacityFromX(const RectF& track, int x) const {
        float t = (x - track.X) / track.Width;
        return (int)std::round(std::max(0.0f, std::min(1.0f, t)) * 100);
    }

    // Fila con etiqueta, valor en porcentaje y deslizador (track + tirador).
    void OpacitySlider(Graphics& g, int id, const RectF& track, const wchar_t* label, int value) {
        const RectF& r = rc[id];
        Card(g, r, hover == id || (dragOpacity && ((id == HTaskOpacity && dragOpacity == 1) ||
                                                    (id == HWinOpacity && dragOpacity == 2))));
        DrawTextStr(g, label, fontBody, style::text, RectF(r.X + S(16), r.Y + S(4), r.Width - S(90), (float)S(22)));
        wchar_t buf[16] = {};
        swprintf_s(buf, L"%d %%", value);
        DrawTextStr(g, buf, fontSmall, style::textDim, RectF(r.X + r.Width - S(74), r.Y + S(6), (float)S(58), (float)S(20)),
                    StringAlignmentFar);
        style::DrawProgress(g, track, value / 100.0f, style::accentSolid);
        float tx = track.X + track.Width * value / 100.0f;
        FillCircle(g, tx, track.Y + track.Height / 2, SF(8), style::accentSolid);
        DrawCircle(g, tx, track.Y + track.Height / 2, SF(8), RGBc(255, 255, 255), 1.2f);
    }

    void OnPaint(Graphics& g, int w, int h) override {
        style::DrawChrome(g, w, h, L"Configuración", L"\uE713", false, hover == HMin ? 1 : (hover == HClose ? 3 : 0));
        FillRect(g, style::ChromeRects(w).max, style::titleBg); // sin maximizar

        // Navegación
        FillRect(g, RectF(0, (float)S(kCaption), (float)S(226), (float)h - S(kCaption)), style::navBg);
        const wchar_t* glyphs[PgCount] = { L"\uE771", L"\uE91B", L"\uE7F4", L"\uE702" };
        const wchar_t* names[PgCount] = { L"Personalización", L"Fondo de escritorio", L"Pantalla", L"Bluetooth" };
        for (int i = 0; i < PgCount; ++i) {
            const RectF& r = rc[HNav0 + i];
            if (i == page) DrawRounded(g, r, SF(6), style::rowSelected);
            else if (hover == HNav0 + i) DrawRounded(g, r, SF(6), style::rowHover);
            if (i == page) DrawRounded(g, RectF(r.X, r.Y + S(10), (float)S(3), r.Height - S(20)), SF(1.5f), style::accent);
            DrawTextStr(g, glyphs[i], fontIcon, i == page ? style::accent : style::textDim,
                        RectF(r.X + S(10), r.Y, (float)S(24), r.Height), StringAlignmentCenter);
            DrawTextStr(g, names[i], fontBody, style::text, RectF(r.X + S(44), r.Y, r.Width - S(48), r.Height));
        }

        switch (page) {
        case PgTheme: PaintTheme(g, w, h); break;
        case PgWallpaper: PaintWallpaper(g); break;
        case PgDisplay: PaintDisplay(g); break;
        case PgBluetooth: PaintBluetooth(g); break;
        }
    }

    void DrawPreview(Graphics& g, const RectF& r, bool darkMode) {
        Color bg = darkMode ? RGBc(17, 20, 27) : RGBc(247, 248, 250);
        Color title = darkMode ? RGBc(23, 27, 36) : RGBc(237, 239, 243);
        Color line = darkMode ? C(70, 255, 255, 255) : C(55, 0, 0, 0);
        Color bar = darkMode ? RGBc(16, 18, 24) : RGBc(226, 229, 235);
        RectF p(r.X + S(12), r.Y + S(12), r.Width - S(24), r.Height - S(52));
        DrawRounded(g, p, SF(6), bg, line);
        DrawRounded(g, RectF(p.X, p.Y, p.Width, (float)S(18)), SF(6), title);
        DrawRounded(g, RectF(p.X + S(8), p.Y + S(5), (float)S(8), (float)S(8)), SF(2), style::accentSolid);
        for (int i = 0; i < 3; ++i)
            DrawRounded(g, RectF(p.X + S(12), p.Y + S(30) + i * S(14), p.Width * (i == 1 ? 0.45f : 0.6f), (float)S(6)), SF(3), line);
        DrawRounded(g, RectF(p.X + S(12), p.Y + S(44), p.Width * 0.3f, (float)S(6)), SF(3), style::accentSolid);
        FillRect(g, RectF(p.X + 1, p.Y + p.Height - S(14), p.Width - 2, (float)S(13)), bar);
    }

    void ModeCard(Graphics& g, int id, bool darkMode, const wchar_t* label) {
        const RectF& r = rc[id];
        bool selected = g_theme.dark == darkMode;
        DrawRounded(g, r, SF(10), hover == id ? style::controlHover : style::cardBg,
                    selected ? style::accentSolid : style::controlBorder, selected ? SF(2) : 1.0f);
        DrawPreview(g, r, darkMode);
        DrawTextStr(g, label, fontBody, style::text, RectF(r.X + S(14), r.Y + r.Height - S(36), r.Width - S(50), (float)S(28)));
        if (selected) {
            RectF badge(r.X + r.Width - S(34), r.Y + r.Height - S(32), (float)S(20), (float)S(20));
            DrawRounded(g, badge, badge.Width / 2, style::accentSolid);
            DrawCheck(g, badge.X + badge.Width / 2, badge.Y + badge.Height / 2, badge.Width * 0.55f, style::onAccent, SF(1.8f));
        }
    }

    void PaintTheme(Graphics& g, int, int h) {
        PageTitle(g, L"Personalización");
        SectionTitle(g, L"Modo", (float)S(96));
        ModeCard(g, HDark, true, L"Oscuro");
        ModeCard(g, HLight, false, L"Claro");

        SectionTitle(g, L"Color de acento", (float)S(298));
        bool custom = true;
        for (int i = 0; i < kPresetCount; ++i) {
            const RectF& r = rc[HSwatch0 + i];
            bool sel = g_theme.accent == kAccentPresets[i].color;
            if (sel) custom = false;
            float cx = r.X + r.Width / 2, cy = r.Y + r.Height / 2;
            if (sel || hover == HSwatch0 + i)
                DrawCircle(g, cx, cy, r.Width / 2 + SF(3), sel ? style::text : style::textFaint, sel ? SF(2) : 1.0f);
            Color c = FromRef(kAccentPresets[i].color);
            FillCircle(g, cx, cy, r.Width / 2 - SF(2), c);
            if (sel) {
                float lum = (0.299f * c.GetR() + 0.587f * c.GetG() + 0.114f * c.GetB()) / 255.0f;
                DrawCheck(g, cx, cy, r.Width * 0.4f, lum > 0.68f ? RGBc(20, 20, 24) : RGBc(255, 255, 255), SF(2));
            }
        }
        style::DrawButton(g, rc[HWindowsAccent], L"Usar el color de Windows", hover == HWindowsAccent);
        if (custom)
            DrawTextStr(g, L"Color personalizado en uso", fontSmall, style::textFaint,
                        RectF(rc[HWindowsAccent].X + rc[HWindowsAccent].Width + S(14), rc[HWindowsAccent].Y, (float)S(240),
                              rc[HWindowsAccent].Height));

        SectionTitle(g, L"Efectos", (float)S(418));
        const RectF& b = rc[HBlur];
        Card(g, b, hover == HBlur);
        DrawTextStr(g, L"Desenfoque (efecto cristal)", fontBody, style::text,
                    RectF(b.X + S(16), b.Y + S(8), b.Width - S(100), (float)S(22)));
        DrawTextStr(g, L"Fondo acrílico en la barra de tareas, los paneles y las ventanas. Desactivarlo ahorra GPU.",
                    fontSmall, style::textDim, RectF(b.X + S(16), b.Y + S(30), b.Width - S(100), (float)S(20)));
        style::DrawSwitch(g, RectF(b.X + b.Width - S(62), b.Y + (b.Height - S(22)) / 2, (float)S(44), (float)S(22)),
                          g_theme.blur, hover == HBlur);

        int taskOp = dragOpacity == 1 ? liveOpacity : g_theme.taskbarOpacity;
        int winOp = dragOpacity == 2 ? liveOpacity : g_theme.windowOpacity;
        OpacitySlider(g, HTaskOpacity, taskOpTrack, L"Opacidad de la barra de tareas", taskOp);
        OpacitySlider(g, HWinOpacity, winOpTrack, L"Opacidad de las ventanas", winOp);

        DrawTextV(g, L"Los cambios se aplican al momento en todas las ventanas de OptimizedShell.", fontSmall,
                  style::textFaint, RectF(x0, (float)h - S(54), cw - S(170), (float)S(36)), StringAlignmentNear, true);
        style::DrawButton(g, rc[HReset], L"Restablecer", hover == HReset);
    }

    RectF WallRect(int i) const {
        float tw = (float)S(150), th = (float)S(86), gap = (float)S(12);
        int cols = std::max(1, (int)((galleryRc.Width + gap) / (tw + gap)));
        return RectF(galleryRc.X + (i % cols) * (tw + gap), galleryRc.Y + (i / cols) * (th + gap) - wallScroll, tw, th);
    }

    float WallContentHeight() const {
        if (wallpapers.empty()) return 0;
        RectF last = WallRect((int)wallpapers.size() - 1);
        return last.Y + last.Height + wallScroll - galleryRc.Y;
    }

    void PaintWallpaper(Graphics& g) {
        PageTitle(g, L"Fondo de escritorio");
        // Vista previa con marco de monitor
        DrawRounded(g, RectF(previewRc.X - S(6), previewRc.Y - S(6), previewRc.Width + S(12), previewRc.Height + S(12)), SF(10),
                    RGBc(20, 22, 28), style::controlBorder);
        auto it = thumbs.find(currentWall);
        if (it != thumbs.end() && it->second) DrawCover(g, it->second, previewRc);
        else DrawTextStr(g, currentWall.empty() ? L"Sin imagen" : L"Cargando...", fontSmall, RGBc(160, 168, 182), previewRc,
                         StringAlignmentCenter);
        if (applyingWall)
            DrawTextStr(g, L"Aplicando...", fontSmall, style::textFaint,
                        RectF(previewRc.X, previewRc.Y + previewRc.Height + S(10), previewRc.Width, (float)S(20)));
        else
            DrawTextStr(g, BaseName(currentWall), fontSmall, style::textFaint,
                        RectF(previewRc.X, previewRc.Y + previewRc.Height + S(10), previewRc.Width, (float)S(20)));

        float fx = rc[HFit0].X;
        DrawTextStr(g, L"Ajuste", fontBold, style::text, RectF(fx, (float)S(104), (float)S(200), (float)S(24)));
        for (int i = 0; i < kFitCount; ++i)
            style::DrawButton(g, rc[HFit0 + i], kFits[i].label, hover == HFit0 + i, i == currentFit);
        style::DrawButton(g, rc[HBrowse], L"Examinar...", hover == HBrowse);

        DrawTextStr(g, L"Imágenes", fontBold, style::text, RectF(x0, galleryRc.Y - S(30), cw, (float)S(24)));
        GraphicsState st = g.Save();
        g.IntersectClip(galleryRc);
        for (int i = 0; i < (int)wallpapers.size(); ++i) {
            RectF r = WallRect(i);
            if (r.Y + r.Height < galleryRc.Y || r.Y > galleryRc.Y + galleryRc.Height) continue;
            auto t = thumbs.find(wallpapers[i]);
            DrawRounded(g, r, SF(6), style::controlBg);
            if (t != thumbs.end() && t->second) DrawCover(g, t->second, r);
            bool sel = _wcsicmp(wallpapers[i].c_str(), currentWall.c_str()) == 0;
            if (sel) DrawRounded(g, RectF(r.X - 2, r.Y - 2, r.Width + 4, r.Height + 4), SF(7), Color(0, 0, 0, 0), style::accentSolid, SF(3));
            else if (hover == HWall0 + i) DrawRounded(g, r, SF(6), Color(0, 0, 0, 0), style::accent, SF(2));
        }
        g.Restore(st);
        if (wallpapers.empty())
            DrawTextStr(g, L"No se encontraron imágenes. Usa \"Examinar...\".", fontSmall, style::textFaint,
                        RectF(galleryRc.X, galleryRc.Y, galleryRc.Width, (float)S(30)));
        float total = WallContentHeight();
        if (total > galleryRc.Height)
            DrawScrollbar(g, RectF(galleryRc.X + galleryRc.Width + S(10), galleryRc.Y, (float)S(4), galleryRc.Height),
                          (int)total, (int)galleryRc.Height, (int)wallScroll);
    }

    void ValueRow(Graphics& g, int id, const wchar_t* label, const std::wstring& value, bool clickable) {
        const RectF& r = rc[id];
        Card(g, r, clickable && hover == id);
        DrawTextStr(g, label, fontBody, style::text, RectF(r.X + S(16), r.Y, r.Width / 2, r.Height));
        DrawTextStr(g, value, fontBody, clickable ? style::text : style::textDim,
                    RectF(r.X + r.Width / 2, r.Y, r.Width / 2 - (clickable ? S(44) : S(16)), r.Height), StringAlignmentFar);
        if (clickable)
            DrawTextStr(g, L"\uE70D", fontIcon, style::textDim, RectF(r.X + r.Width - S(36), r.Y, (float)S(20), r.Height),
                        StringAlignmentCenter);
    }

    void PaintDisplay(Graphics& g) {
        PageTitle(g, L"Pantalla");
        if (displays.empty()) {
            DrawTextStr(g, L"No se encontraron pantallas.", fontBody, style::textDim, RectF(x0, (float)S(110), cw, (float)S(30)));
            return;
        }
        if (displays.size() > 1)
            for (int i = 0; i < (int)displays.size() && i < 8; ++i) {
                const RectF& r = rc[HMonitor0 + i];
                bool sel = i == displaySel;
                DrawRounded(g, r, SF(6), sel ? style::rowSelected : (hover == HMonitor0 + i ? style::controlHover : style::controlBg),
                            sel ? style::accentSolid : style::controlBorder);
                DrawTextStr(g, std::to_wstring(i + 1) + L". " + displays[i].name, fontSmall, style::text,
                            RectF(r.X + S(10), r.Y, r.Width - S(20), r.Height));
            }
        const DEVMODEW& m = displays[displaySel].mode;
        bool portrait = Portrait(m.dmDisplayOrientation);
        DWORD w = portrait ? m.dmPelsHeight : m.dmPelsWidth, hh = portrait ? m.dmPelsWidth : m.dmPelsHeight;
        const wchar_t* orient[4] = { L"Horizontal", L"Vertical", L"Horizontal (volteada)", L"Vertical (volteada)" };
        std::wstring scale = displays[displaySel].primary ? std::to_wstring(MulDiv(g_dpi, 100, 96)) + L" %" : L"—";
        ValueRow(g, HRes, L"Resolución", std::to_wstring(w) + L" × " + std::to_wstring(hh), true);
        ValueRow(g, HHz, L"Frecuencia de actualización", std::to_wstring(m.dmDisplayFrequency) + L" Hz", true);
        ValueRow(g, HOrient, L"Orientación", orient[std::min<DWORD>(m.dmDisplayOrientation, 3)], true);
        ValueRow(g, HScale, L"Escala (se cambia en Windows)", scale, true);

        const RectF& b = rc[HBright];
        Card(g, b, false);
        DrawTextStr(g, L"Brillo", fontBody, style::text, RectF(b.X + S(16), b.Y + S(10), (float)S(140), (float)S(24)));
        if (brightness >= 0) {
            style::DrawProgress(g, brightTrack, brightness / 100.0f, style::accentSolid);
            float tx = brightTrack.X + brightTrack.Width * brightness / 100.0f;
            FillCircle(g, tx, brightTrack.Y + brightTrack.Height / 2, SF(9), style::accentSolid);
            DrawCircle(g, tx, brightTrack.Y + brightTrack.Height / 2, SF(9), RGBc(255, 255, 255), 1.2f);
            DrawTextStr(g, std::to_wstring(brightness) + L" %", fontBody, style::text,
                        RectF(brightTrack.X + brightTrack.Width + S(16), b.Y, (float)S(60), b.Height));
        } else {
            DrawTextStr(g, brightness == -2 ? L"Leyendo..." : L"No disponible para esta pantalla (solo pantallas integradas).",
                        fontSmall, style::textFaint, RectF(brightTrack.X, b.Y, cw - S(176), b.Height));
        }
        style::DrawButton(g, rc[HAdvanced], L"Más opciones de pantalla de Windows", hover == HAdvanced);
    }

    void PaintBluetooth(Graphics& g) {
        PageTitle(g, L"Bluetooth");
        const RectF& s = rc[HBtSwitch];
        Card(g, s, hover == HBtSwitch && bt.radio >= 0);
        DrawTextStr(g, L"\uE702", fontIcon, style::accent, RectF(s.X + S(16), s.Y, (float)S(24), s.Height), StringAlignmentCenter);
        DrawTextStr(g, L"Bluetooth", fontBody, style::text, RectF(s.X + S(52), s.Y + S(14), s.Width - S(140), (float)S(22)));
        std::wstring state = btLoading && bt.radio == -1 ? L"Comprobando..."
                           : bt.radio < 0 ? L"No se encontró un adaptador Bluetooth"
                           : btBusy ? L"Aplicando..."
                           : bt.radio == 1 ? L"Activado · visible para dispositivos emparejados" : L"Desactivado";
        DrawTextStr(g, state, fontSmall, style::textDim, RectF(s.X + S(52), s.Y + S(38), s.Width - S(140), (float)S(20)));
        if (bt.radio >= 0)
            style::DrawSwitch(g, RectF(s.X + s.Width - S(62), s.Y + (s.Height - S(22)) / 2, (float)S(44), (float)S(22)),
                              bt.radio == 1, hover == HBtSwitch);

        bool on = bt.radio == 1;
        style::DrawButton(g, rc[HBtAdd], L"Agregar dispositivo", hover == HBtAdd, on, false, !on);
        style::DrawButton(g, rc[HBtMore], L"Más opciones de Bluetooth", hover == HBtMore);
        style::DrawButton(g, rc[HBtRefresh], L"Actualizar", hover == HBtRefresh);

        float y = (float)S(256);
        DrawTextStr(g, L"Dispositivos emparejados", fontBold, style::text, RectF(x0, y, cw, (float)S(24)));
        y += S(34);
        btRows.clear();
        if (!on) {
            DrawTextStr(g, bt.radio < 0 ? L"" : L"Activa Bluetooth para ver tus dispositivos.", fontSmall, style::textFaint,
                        RectF(x0, y, cw, (float)S(24)));
            return;
        }
        if (bt.devices.empty())
            DrawTextStr(g, btLoading ? L"Buscando..." : L"No hay dispositivos emparejados.", fontSmall, style::textFaint,
                        RectF(x0, y, cw, (float)S(24)));
        for (int i = 0; i < (int)bt.devices.size(); ++i) {
            const auto& d = bt.devices[i];
            RectF r(x0, y, cw, (float)S(56));
            if (r.Y + r.Height > winH - S(8)) break;
            btRows.push_back(r);
            Card(g, r, false);
            DrawTextStr(g, BtGlyph(d.kind), fontIcon, d.connected ? style::accent : style::textDim,
                        RectF(r.X + S(16), r.Y, (float)S(24), r.Height), StringAlignmentCenter);
            DrawTextStr(g, d.name.empty() ? L"(sin nombre)" : d.name, fontBody, style::text,
                        RectF(r.X + S(52), r.Y + S(8), r.Width - S(170), (float)S(22)));
            DrawTextStr(g, d.connected ? L"Conectado" : L"Emparejado", fontSmall, d.connected ? style::success : style::textFaint,
                        RectF(r.X + S(52), r.Y + S(30), r.Width - S(170), (float)S(18)));
            RectF rb(r.X + r.Width - S(100), r.Y + S(11), (float)S(86), (float)S(34));
            style::DrawButton(g, rb, L"Quitar", hover == HBtRemove0 + i, false, true, btBusy);
            y += S(64);
        }
    }

    // ------------------------------------------------------------ entrada
    RectF RemoveRect(int i) const {
        const RectF& r = btRows[i];
        return RectF(r.X + r.Width - S(100), r.Y + S(11), (float)S(86), (float)S(34));
    }

    int HitAt(int x, int y) const {
        for (int id : { HMin, HClose }) if (InRect(rc.at(id), x, y)) return id;
        for (int i = 0; i < PgCount; ++i) if (InRect(rc.at(HNav0 + i), x, y)) return HNav0 + i;
        switch (page) {
        case PgTheme:
            for (int id : { HDark, HLight, HWindowsAccent, HBlur, HTaskOpacity, HWinOpacity, HReset })
                if (InRect(rc.at(id), x, y)) return id;
            for (int i = 0; i < kPresetCount; ++i) if (InRect(rc.at(HSwatch0 + i), x, y)) return HSwatch0 + i;
            break;
        case PgWallpaper:
            for (int i = 0; i < kFitCount; ++i) if (InRect(rc.at(HFit0 + i), x, y)) return HFit0 + i;
            if (InRect(rc.at(HBrowse), x, y)) return HBrowse;
            if (InRect(galleryRc, x, y))
                for (int i = 0; i < (int)wallpapers.size(); ++i) if (InRect(WallRect(i), x, y)) return HWall0 + i;
            break;
        case PgDisplay:
            if (displays.size() > 1)
                for (int i = 0; i < (int)displays.size() && i < 8; ++i) if (InRect(rc.at(HMonitor0 + i), x, y)) return HMonitor0 + i;
            for (int id : { HRes, HHz, HOrient, HScale, HAdvanced }) if (InRect(rc.at(id), x, y)) return id;
            if (brightness >= 0 && InRect(RectF(brightTrack.X - S(10), brightTrack.Y - S(14), brightTrack.Width + S(20), brightTrack.Height + S(28)), x, y))
                return HBright;
            break;
        case PgBluetooth:
            for (int id : { HBtSwitch, HBtAdd, HBtMore, HBtRefresh }) if (InRect(rc.at(id), x, y)) return id;
            for (int i = 0; i < (int)btRows.size(); ++i) if (InRect(RemoveRect(i), x, y)) return HBtRemove0 + i;
            break;
        }
        return HNone;
    }

    void OnMouseDown(int x, int y) override {
        int id = HitAt(x, y);
        if (id == HMin) { ShowWindow(hwnd, SW_MINIMIZE); return; }
        if (id == HClose) { Close(); return; }
        if (id >= HNav0 && id < HNav0 + PgCount) { EnterPage(id - HNav0); return; }
        ThemeSettings t = g_theme;
        switch (id) {
        // Personalización
        case HDark: t.dark = true; break;
        case HLight: t.dark = false; break;
        case HWindowsAccent: t.accent = WindowsAccent(); break;
        case HBlur: t.blur = !t.blur; break;
        case HTaskOpacity:
            dragOpacity = 1; SetCapture(hwnd); liveOpacity = OpacityFromX(taskOpTrack, x);
            InvalidateRectF(rc[HTaskOpacity]); return;
        case HWinOpacity:
            dragOpacity = 2; SetCapture(hwnd); liveOpacity = OpacityFromX(winOpTrack, x);
            InvalidateRectF(rc[HWinOpacity]); return;
        case HReset: t = ThemeSettings(); break;
        // Fondo
        case HBrowse: BrowseWallpaper(); return;
        // Pantalla
        case HRes: ResolutionMenu(); return;
        case HHz: RefreshMenu(); return;
        case HOrient: OrientationMenu(); return;
        case HScale: case HAdvanced: Launch(L"ms-settings:display"); return;
        case HBright: dragBright = true; SetCapture(hwnd); SetBrightnessFromX(x); return;
        // Bluetooth
        case HBtSwitch: ToggleBluetooth(); return;
        case HBtAdd: if (bt.radio == 1) AddDevice(); return;
        case HBtMore: Launch(L"ms-settings:bluetooth"); return;
        case HBtRefresh: RefreshBluetooth(); return;
        default:
            if (id >= HSwatch0 && id < HSwatch0 + kPresetCount) { t.accent = kAccentPresets[id - HSwatch0].color; break; }
            if (id >= HFit0 && id < HFit0 + kFitCount) { if (!currentWall.empty()) SetWallpaper(currentWall, id - HFit0); return; }
            if (id >= HMonitor0 && id < HMonitor0 + 8) { displaySel = id - HMonitor0; LoadDisplayPage(); Invalidate(); return; }
            if (id >= HWall0 && id < HBtRemove0) { SetWallpaper(wallpapers[id - HWall0], currentFit); return; }
            if (id >= HBtRemove0) { RemoveDevice(id - HBtRemove0); return; }
            return;
        }
        if (!(t == g_theme)) ApplyTheme_(t);
    }

    void OnMouseUp(int, int) override {
        if (dragOpacity) {
            ReleaseCapture();
            ThemeSettings t = g_theme;
            if (dragOpacity == 1) t.taskbarOpacity = liveOpacity;
            else t.windowOpacity = liveOpacity;
            dragOpacity = 0;
            if (!(t == g_theme)) ApplyTheme_(t);
            else Invalidate();
            return;
        }
        if (dragBright) {
            dragBright = false;
            ReleaseCapture();
            CommitBrightness();
        }
    }

    void OnMouseMove(int x, int y) override {
        if (dragOpacity) {
            liveOpacity = OpacityFromX(dragOpacity == 1 ? taskOpTrack : winOpTrack, x);
            InvalidateRectF(rc[dragOpacity == 1 ? HTaskOpacity : HWinOpacity]);
            return;
        }
        if (dragBright) { SetBrightnessFromX(x); return; }
        int id = HitAt(x, y);
        if (id == hover) return;
        hover = id;
        Invalidate();
        std::wstring t;
        if (id >= HSwatch0 && id < HSwatch0 + kPresetCount) t = kAccentPresets[id - HSwatch0].name;
        else if (id >= HWall0 && id < HBtRemove0) t = BaseName(wallpapers[id - HWall0]);
        RectF tr = id >= HWall0 && id < HBtRemove0 ? WallRect(id - HWall0) : (rc.count(id) ? rc[id] : RectF());
        tip.Set(tr, t);
        if (id) SetCursor(LoadCursorW(nullptr, IDC_HAND));
    }

    void OnWheel(int delta) override {
        if (page == PgWallpaper) {
            float maxScroll = std::max(0.0f, WallContentHeight() - galleryRc.Height);
            wallScroll = std::max(0.0f, std::min(maxScroll, wallScroll - (delta / (float)WHEEL_DELTA) * S(90)));
            InvalidateRectF(RectF(galleryRc.X, galleryRc.Y, galleryRc.Width + S(20), galleryRc.Height));
        } else if (page == PgDisplay && brightness >= 0) {
            brightness = std::max(0, std::min(100, brightness + (delta > 0 ? 5 : -5)));
            InvalidateRectF(rc[HBright]);
            SetTimer(hwnd, 2, 250, nullptr); // aplicar al dejar de girar la rueda
        }
    }

    void OnTimer(UINT_PTR id) override {
        if (id == 1 && page == PgBluetooth && !IsIconic(hwnd)) RefreshBluetooth();
        if (id == 2) { KillTimer(hwnd, 2); CommitBrightness(); }
    }

    void OnKeyDown(int vk) override {
        if (vk == VK_ESCAPE) Close();
        else if (GetKeyState(VK_CONTROL) < 0 && vk >= '1' && vk < '1' + PgCount) EnterPage(vk - '1');
    }

    LRESULT OnHitTest(int x, int y) override {
        if (y >= S(kCaption) || InRect(rc[HMin], x, y) || InRect(rc[HClose], x, y)) return HTCLIENT;
        return HTCAPTION;
    }

    void OnDestroy() override { PostQuitMessage(0); }

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        switch (msg) {
        case WM_THUMB: {
            std::unique_ptr<ThumbResult> r((ThumbResult*)lp);
            Bitmap* b = r->bmp ? HBitmapToBitmap(r->bmp) : nullptr;
            if (r->bmp) DeleteObject(r->bmp);
            delete thumbs[r->path];
            thumbs[r->path] = b;
            if (page == PgWallpaper) Invalidate();
            return 0;
        }
        case WM_WALLDONE:
            applyingWall = false;
            currentWall = GetWallpaperPath();
            if (!thumbRequested.count(currentWall)) {
                thumbRequested.insert(currentWall);
                LoadThumbs(hwnd, { currentWall }, SIZE{ S(320), S(180) });
            }
            Invalidate();
            return 0;
        case WM_BRIGHT:
            brightness = (int)(INT_PTR)wp;
            if (page == PgDisplay) Invalidate();
            return 0;
        case WM_BTSTATE: {
            std::unique_ptr<BtState> st((BtState*)lp);
            bt = *st;
            btLoading = false;
            if (page == PgBluetooth) Invalidate();
            return 0;
        }
        case WM_BTDONE:
            btBusy = false;
            if (!wp)
                MsgBox(hwnd, lp ? L"No se pudo quitar el dispositivo." :
                                  L"No se pudo cambiar el estado de Bluetooth. Comprueba que el modo avión está desactivado.",
                       L"Bluetooth", MB_ICONWARNING);
            RefreshBluetooth();
            Invalidate();
            return 0;
        case WM_GOTOPAGE:
            if (IsIconic(hwnd)) ShowWindow(hwnd, SW_RESTORE);
            EnterPage((int)wp);
            ForceForeground(hwnd);
            return 0;
        case WM_DISPLAYCHANGE:
            if (page == PgDisplay) { LoadDisplayPage(); Invalidate(); }
            return 0;
        case WM_SETTINGCHANGE:
            break;
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
};

} // namespace

int RunSettingsMode() {
    std::wstring arg = ArgValue(L"--settings");
    int pg = (arg.empty() || arg.rfind(L"--", 0) == 0) ? PgTheme : PageFromName(arg);
    if (HWND existing = FindWindowW(L"OShellCppSettings", nullptr)) {
        DWORD pid = 0;
        GetWindowThreadProcessId(existing, &pid);
        AllowSetForegroundWindow(pid);
        PostMessageW(existing, WM_GOTOPAGE, pg, 0);
        return 0;
    }
    CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    InitUi(GetModuleHandleW(nullptr));
    {
        SettingsWindow w;
        if (w.Start(pg)) RunMessageLoop();
    }
    ShutdownUi();
    CoUninitialize();
    return 0;
}
