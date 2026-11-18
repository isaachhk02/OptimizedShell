// shell.cpp - OptimizedShell: escritorio, barra de tareas, menú Inicio, bandeja,
// flyouts (volumen, Wi-Fi, batería) y diálogo Ejecutar. Puede sustituir a
// explorer.exe como shell del sistema (modo shell) o ejecutarse encima de él.
//
// Diseño orientado a consumir lo mínimo:
//  - Todo es por eventos: shell hooks para las ventanas, notificaciones de energía
//    para la batería, cambios de carpeta para el escritorio. No hay sondeos.
//  - El reloj se despierta una vez por minuto y repinta solo su rectángulo.
//  - Las ventanas ocultas (flyouts, menú Inicio) no tienen timers y liberan su buffer.
//  - El fondo de escritorio lo pinta el sistema (PaintDesktop): no se carga la imagen.
//
// Modos: (sin argumentos) shell | --taskmgr | --explorer [ruta] | --logon | --lock
//        --install / --uninstall (shell del usuario actual)
#include "ui.h"
#include "startup.h"
#include "winrt_bridge.h"

#include <powrprof.h>
#include <mmdeviceapi.h>
#include <endpointvolume.h>
#include <wlanapi.h>
#include <thread>
#include <mutex>
#include <condition_variable>
#include <deque>
#include <functional>
#include <map>
#include <memory>

#pragma comment(lib, "powrprof.lib")
#pragma comment(lib, "wlanapi.lib")
#pragma comment(lib, "uuid.lib")

using namespace Gdiplus;
using namespace ui;

int RunLogonUIMode(bool demo, bool lockMode);
int RunTaskManagerMode();
int RunExplorerMode();
int RunSettingsMode();

namespace {

bool g_shellMode = false;
HWND g_taskbarHwnd = nullptr;
bool g_restoreExplorer = false;

const UINT WM_SHELLACTION = WM_APP + 20;   // wParam = ShellAction
const UINT WM_APPSLOADED = WM_APP + 30;    // lParam = std::vector<AppEntry>*
const UINT WM_APPICON = WM_APP + 31;       // lParam = IconResult*
const UINT WM_CLOAKCHANGED = WM_APP + 32;  // lParam = HWND

enum ShellAction { ActStart = 1, ActExplorer, ActRun, ActTaskMgr, ActShowDesktop };

// ---------------------------------------------------------------- hilo de trabajo
// Un único hilo en segundo plano (COM STA, EcoQoS) para tareas lentas del shell:
// enumerar aplicaciones, extraer iconos, ejecutar programas de inicio.
class Worker {
public:
    ~Worker() {
        {
            std::lock_guard<std::mutex> lk(mu);
            stop = true;
        }
        cv.notify_all();
        if (th.joinable()) th.join();
    }

    void Post(std::function<void()> fn) {
        {
            std::lock_guard<std::mutex> lk(mu);
            queue.push_back(std::move(fn));
            if (!th.joinable()) th = std::thread([this] { Loop(); });
        }
        cv.notify_one();
    }

private:
    std::thread th;
    std::mutex mu;
    std::condition_variable cv;
    std::deque<std::function<void()>> queue;
    bool stop = false;

    void Loop() {
        CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
        SetThreadEco();
        for (;;) {
            std::function<void()> fn;
            {
                std::unique_lock<std::mutex> lk(mu);
                cv.wait(lk, [this] { return stop || !queue.empty(); });
                if (stop) break;
                fn = std::move(queue.front());
                queue.pop_front();
            }
            fn();
        }
        CoUninitialize();
    }
};

Worker& BackgroundWorker() {
    static Worker w;
    return w;
}


// ---------------------------------------------------------------- iconos vectoriales
void DrawSpeaker(Graphics& g, float x, float y, float size, const Color& c) {
    SolidBrush b(c);
    g.FillRectangle(&b, x, y + size * 0.30f, size * 0.22f, size * 0.40f);
    PointF pts[4] = {
        PointF(x + size * 0.22f, y + size * 0.22f),
        PointF(x + size * 0.55f, y + size * 0.08f),
        PointF(x + size * 0.55f, y + size * 0.92f),
        PointF(x + size * 0.22f, y + size * 0.78f),
    };
    g.FillPolygon(&b, pts, 4);
    Pen p(c, size * 0.07f);
    g.DrawArc(&p, x + size * 0.55f, y + size * 0.28f, size * 0.35f, size * 0.44f, -60.0f, 120.0f);
}

void DrawNetwork(Graphics& g, float x, float y, float size, const Color& c) {
    SolidBrush b(c);
    for (int i = 0; i < 3; ++i) {
        float h = size * (0.28f + i * 0.22f);
        g.FillRectangle(&b, x + i * size * 0.30f, y + size - h, size * 0.20f, h);
    }
}

void DrawBattery(Graphics& g, float x, float y, float size, const Color& c, int percent, bool charging) {
    SolidBrush b(c);
    g.FillRectangle(&b, x, y + size * 0.10f, size * 0.82f, size * 0.80f);
    g.FillRectangle(&b, x + size * 0.82f, y + size * 0.32f, size * 0.10f, size * 0.36f);
    SolidBrush bg(WithAlpha(style::taskbarBg, 255));
    g.FillRectangle(&bg, x + size * 0.06f, y + size * 0.16f, size * 0.70f, size * 0.68f);
    percent = std::max(0, std::min(100, percent));
    Color lvl = charging ? style::accent : (percent < 20 ? style::danger : style::success);
    SolidBrush lb(lvl);
    g.FillRectangle(&lb, x + size * 0.06f, y + size * 0.16f, size * 0.70f * percent / 100.0f, size * 0.68f);
}

// ---------------------------------------------------------------- escritorio
struct DeskItem {
    std::wstring name;
    std::wstring path;
    bool folder = false;
    int iconIndex = -1;
};

class DesktopWindow : public WindowBase {
public:
    bool Start() {
        int sw = GetSystemMetrics(SM_CXSCREEN);
        int sh = GetSystemMetrics(SM_CYSCREEN);
        if (!Create(L"OShellCppDesktop", WS_EX_TOOLWINDOW, WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN,
                    0, 0, sw, sh, L"Escritorio"))
            return false;
        SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        LoadItems();
        StartWatching();
        return true;
    }

    ~DesktopWindow() override {
        for (HANDLE h : watch) FindCloseChangeNotification(h);
    }

    const std::vector<HANDLE>& WatchHandles() const { return watch; }

    // Una de las carpetas del escritorio cambió: recargar con un pequeño retardo
    // para agrupar ráfagas de cambios (p. ej. al copiar muchos archivos).
    void OnWatchSignaled(size_t i) {
        if (i < watch.size()) FindNextChangeNotification(watch[i]);
        SetTimer(hwnd, 1, 400, nullptr);
    }

    void FitToScreen() {
        SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN),
                     SWP_NOACTIVATE);
        Invalidate();
    }

protected:
    std::vector<DeskItem> items;
    std::vector<HANDLE> watch;
    int selected = -1;

    void StartWatching() {
        for (int csidl : { CSIDL_DESKTOPDIRECTORY, CSIDL_COMMON_DESKTOPDIRECTORY }) {
            wchar_t dir[MAX_PATH] = {};
            if (SHGetFolderPathW(nullptr, csidl, nullptr, 0, dir) != S_OK) continue;
            HANDLE h = FindFirstChangeNotificationW(dir, FALSE,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_ATTRIBUTES);
            if (h != INVALID_HANDLE_VALUE) watch.push_back(h);
        }
    }

    void LoadItems() {
        std::wstring selPath = (selected >= 0 && selected < (int)items.size()) ? items[selected].path : L"";
        items.clear();
        for (int csidl : { CSIDL_DESKTOPDIRECTORY, CSIDL_COMMON_DESKTOPDIRECTORY }) {
            wchar_t dir[MAX_PATH] = {};
            if (SHGetFolderPathW(nullptr, csidl, nullptr, 0, dir) == S_OK) ScanDir(dir);
        }
        std::sort(items.begin(), items.end(), [](const DeskItem& a, const DeskItem& b) {
            if (a.folder != b.folder) return a.folder > b.folder;
            return StrCmpLogicalW(a.name.c_str(), b.name.c_str()) < 0;
        });
        selected = -1;
        for (size_t i = 0; i < items.size(); ++i)
            if (!selPath.empty() && items[i].path == selPath) selected = (int)i;
        Invalidate();
    }

    void ScanDir(const std::wstring& dir) {
        WIN32_FIND_DATAW fd;
        HANDLE h = FindFirstFileExW((dir + L"\\*").c_str(), FindExInfoBasic, &fd, FindExSearchNameMatch, nullptr,
                                    FIND_FIRST_EX_LARGE_FETCH);
        if (h == INVALID_HANDLE_VALUE) return;
        do {
            if (wcscmp(fd.cFileName, L".") == 0 || wcscmp(fd.cFileName, L"..") == 0) continue;
            if (fd.dwFileAttributes & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM)) continue;
            DeskItem it;
            it.path = dir + L"\\" + fd.cFileName;
            it.folder = (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
            it.name = fd.cFileName;
            if (!it.folder) {
                for (const wchar_t* ext : { L".lnk", L".url" })
                    if (EndsWithNoCase(it.name, ext)) it.name.resize(it.name.size() - wcslen(ext));
            }
            it.iconIndex = SysIconIndex(it.path);
            items.push_back(it);
        } while (FindNextFileW(h, &fd));
        FindClose(h);
    }

    // Rejilla por columnas, como el escritorio de Windows.
    int Rows() const {
        RECT rc;
        GetClientRect(hwnd, &rc);
        return std::max(1, (int)(rc.bottom - S(16) - S(56)) / S(104));
    }

    RectF CellRect(int i) const {
        int rows = Rows();
        int col = i / rows, row = i % rows;
        return RectF((float)(S(12) + col * S(96)), (float)(S(12) + row * S(104)), (float)S(96), (float)S(104));
    }

    int HitTest(int x, int y) const {
        for (size_t i = 0; i < items.size(); ++i)
            if (InRect(CellRect((int)i), x, y)) return (int)i;
        return -1;
    }

    void Select(int idx) {
        if (idx == selected) return;
        if (selected >= 0 && selected < (int)items.size()) InvalidateRectF(CellRect(selected));
        selected = idx;
        if (selected >= 0) InvalidateRectF(CellRect(selected));
    }

    void OnPaintBackground(HDC dc, const RECT& rc) override {
        // El sistema ya tiene el fondo en memoria: lo reutilizamos en vez de decodificar la imagen.
        if (!PaintDesktop(dc)) {
            HBRUSH b = CreateSolidBrush(RGB(15, 23, 42));
            ::FillRect(dc, &rc, b);
            DeleteObject(b);
        }
    }

    void OnPaint(Graphics& g, int, int h) override {
        int icon = S(48);
        for (size_t i = 0; i < items.size(); ++i) {
            RectF cell = CellRect((int)i);
            if (cell.Y + cell.Height > h) continue;
            const DeskItem& it = items[i];
            bool sel = ((int)i == selected);
            if (sel)
                DrawRounded(g, RectF(cell.X + 4, cell.Y + 2, cell.Width - 8, cell.Height - 4), SF(6),
                            WithAlpha(style::accentSolid, 130), WithAlpha(style::accent, 200));
            Bitmap* bmp = SysIconBitmap(it.iconIndex, icon);
            float ix = cell.X + (cell.Width - icon) / 2;
            if (bmp) g.DrawImage(bmp, ix, cell.Y + 6, (float)icon, (float)icon);
            RectF label(cell.X + 2, cell.Y + icon + SF(8), cell.Width - 4, (float)S(36));
            DrawTextV(g, it.name, fontSmall, C(200, 0, 0, 0), RectF(label.X + 1, label.Y + 1, label.Width, label.Height),
                      StringAlignmentCenter, true);
            DrawTextV(g, it.name, fontSmall, RGBc(245, 247, 250), label, StringAlignmentCenter, true);
        }
    }

    void Open(int idx) {
        if (idx < 0 || idx >= (int)items.size()) return;
        if (items[idx].folder) OpenInExplorer(items[idx].path);
        else Launch(items[idx].path);
    }

    void DeleteSelected() {
        if (selected < 0 || selected >= (int)items.size()) return;
        std::wstring from = items[selected].path;
        from.push_back(L'\0'); // doble terminador
        SHFILEOPSTRUCTW op = {};
        op.hwnd = hwnd;
        op.wFunc = FO_DELETE;
        op.pFrom = from.c_str();
        op.fFlags = FOF_ALLOWUNDO;
        SHFileOperationW(&op);
    }

    void OnMouseDown(int x, int y) override { Select(HitTest(x, y)); }

    void OnDoubleClick(int x, int y) override { Open(HitTest(x, y)); }

    void OnKeyDown(int vk) override {
        if (vk == VK_F5) { LoadItems(); return; }
        if (items.empty()) return;
        if (vk == VK_RETURN) { Open(selected); return; }
        if (vk == VK_DELETE) { DeleteSelected(); return; }
        int rows = Rows();
        int s = selected < 0 ? 0 : selected;
        int n = (int)items.size();
        if (vk == VK_UP) s = std::max(0, s - 1);
        else if (vk == VK_DOWN) s = std::min(n - 1, s + 1);
        else if (vk == VK_LEFT) s = std::max(0, s - rows);
        else if (vk == VK_RIGHT) s = std::min(n - 1, s + rows);
        else return;
        Select(s);
    }

    void OnTimer(UINT_PTR id) override {
        if (id == 1) { KillTimer(hwnd, 1); LoadItems(); }
    }

    void OnSettingsChange() override { Invalidate(); } // fondo cambiado

    void ShowItemMenu(int idx, POINT pt) {
        const DeskItem& it = items[idx];
        HMENU m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING | MF_DEFAULT, 1, L"Abrir");
        if (!it.folder) AppendMenuW(m, MF_STRING, 2, L"Abrir ubicación del archivo");
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING, 3, L"Eliminar");
        AppendMenuW(m, MF_STRING, 4, L"Propiedades");
        SetForegroundWindow(hwnd);
        int cmd = ui::ThemedMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        switch (cmd) {
        case 1: Open(idx); break;
        case 2: OpenInExplorer(it.path, true); break;
        case 3: DeleteSelected(); break;
        case 4: SHObjectProperties(hwnd, SHOP_FILEPATH, it.path.c_str(), nullptr); break;
        }
    }

    void ShowDesktopMenu(POINT pt) {
        HMENU m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, 1, L"Actualizar\tF5");
        AppendMenuW(m, MF_STRING, 2, L"Abrir el Escritorio en Archivos");
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING, 3, L"Cambiar fondo de escritorio");
        AppendMenuW(m, MF_STRING, 4, L"Configuración de pantalla");
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING, 5, L"Administrador de tareas");
        AppendMenuW(m, MF_STRING, 6, L"Personalizar OptimizedShell");
        SetForegroundWindow(hwnd);
        int cmd = ui::ThemedMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        wchar_t dir[MAX_PATH] = {};
        switch (cmd) {
        case 1: LoadItems(); break;
        case 2:
            if (SHGetFolderPathW(nullptr, CSIDL_DESKTOPDIRECTORY, nullptr, 0, dir) == S_OK) OpenInExplorer(dir);
            break;
        case 3: LaunchOwn(L"--settings fondo"); break;
        case 4: LaunchOwn(L"--settings pantalla"); break;
        case 5: LaunchOwn(L"--taskmgr"); break;
        case 6: LaunchOwn(L"--settings"); break;
        }
    }

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        switch (msg) {
        case WM_RBUTTONUP: {
            int x = GET_X_LPARAM(lp), y = GET_Y_LPARAM(lp);
            int idx = HitTest(x, y);
            Select(idx);
            POINT pt = { x, y };
            ClientToScreen(hwnd, &pt);
            if (idx >= 0) ShowItemMenu(idx, pt);
            else ShowDesktopMenu(pt);
            return 0;
        }
        case WM_WINDOWPOSCHANGING: {
            // El escritorio siempre al fondo, aunque se haga clic en él.
            auto* wpos = (WINDOWPOS*)lp;
            if (!(wpos->flags & SWP_NOZORDER)) wpos->hwndInsertAfter = HWND_BOTTOM;
            return 0;
        }
        case WM_DISPLAYCHANGE:
            FitToScreen();
            return 0;
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }

    bool CanClose() override { return false; }
};

// ---------------------------------------------------------------- diálogo Ejecutar
class RunDialog : public WindowBase, public EditKeySink {
public:
    static void Show() {
        static RunDialog* inst = nullptr;
        if (inst && inst->Hwnd() && IsWindow(inst->Hwnd())) {
            ForceForeground(inst->Hwnd());
            return;
        }
        delete inst;
        inst = new RunDialog();
        if (!inst->Open()) { delete inst; inst = nullptr; }
    }

    bool OnEditKey(HWND, int vk) override {
        if (vk == VK_RETURN) { Submit(); return true; }
        if (vk == VK_ESCAPE) { Close(); return true; }
        return false;
    }

protected:
    HWND edit = nullptr;
    RectF ok, cancel, field;
    int hover = 0;

    bool Open() {
        int w = S(480), h = S(190);
        RECT wa = {};
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &wa, 0);
        int x = wa.left + S(12);
        int y = wa.bottom - h - S(12);
        if (!Create(L"OShellCppRun", WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP | WS_CLIPCHILDREN,
                    x, y, w, h, L"Ejecutar"))
            return false;
        SetFrame(FrameOpaque);
        field = RectF((float)S(20), (float)S(74), (float)w - S(40), (float)S(36));
        ok = RectF((float)w - S(20) - S(100), (float)h - S(20) - S(32), (float)S(100), (float)S(32));
        cancel = RectF(ok.X - S(10) - S(100), ok.Y, (float)S(100), (float)S(32));
        edit = CreateDarkEdit(hwnd, 100, L"Programa, carpeta, documento o sitio web");
        MoveWindow(edit, (int)field.X + S(10), (int)field.Y + S(8), (int)field.Width - S(20), (int)field.Height - S(14), FALSE);
        HookEditKeys(edit, this);
        ShowWindow(hwnd, SW_SHOW);
        ForceForeground(hwnd);
        SetFocus(edit);
        return true;
    }

    void Submit() {
        std::wstring cmd = Trim(GetWindowString(edit));
        if (cmd.empty()) return;
        if (RunCommandLine(cmd, hwnd)) Close();
        else ui::MsgBox(hwnd, (L"No se encuentra \"" + cmd + L"\".").c_str(), L"Ejecutar", MB_ICONWARNING);
    }

    void OnPaint(Graphics& g, int w, int h) override {
        g.Clear(style::dialogBg);
        DrawRounded(g, RectF(0, 0, (float)w - 1, (float)h - 1), 0, Color(0, 0, 0, 0), style::cardBorder);
        DrawTextStr(g, L"Ejecutar", fontTitle, style::text, RectF((float)S(20), (float)S(16), (float)w - S(40), (float)S(30)));
        DrawTextStr(g, L"Escribe el nombre de un programa, carpeta, documento o recurso de Internet.",
                    fontSmall, style::textDim, RectF((float)S(20), (float)S(46), (float)w - S(40), (float)S(20)));
        style::DrawField(g, field, true);
        style::DrawButton(g, cancel, L"Cancelar", hover == 2);
        style::DrawButton(g, ok, L"Aceptar", hover == 1, true);
    }

    void OnMouseDown(int x, int y) override {
        if (InRect(ok, x, y)) Submit();
        else if (InRect(cancel, x, y)) Close();
        else if (InRect(field, x, y)) SetFocus(edit);
    }

    void OnMouseMove(int x, int y) override {
        int hv = InRect(ok, x, y) ? 1 : (InRect(cancel, x, y) ? 2 : 0);
        if (hv != hover) { hover = hv; InvalidateRectF(ok); InvalidateRectF(cancel); }
    }

    void OnKeyDown(int vk) override {
        if (vk == VK_ESCAPE) Close();
        else if (vk == VK_RETURN) Submit();
    }

    LRESULT OnHitTest(int x, int y) override {
        return (y < S(70) && !InRect(field, x, y)) ? HTCAPTION : 0;
    }
};

// ---------------------------------------------------------------- menú Inicio
struct AppEntry {
    std::wstring name;
    std::wstring parse; // nombre de análisis dentro de shell:AppsFolder (AUMID o ruta)
    std::wstring lower;
};

struct IconResult {
    std::wstring key;
    HBITMAP bmp = nullptr;
};

// Enumera shell:AppsFolder: aplicaciones de escritorio y de la Store en una sola lista.
std::vector<AppEntry>* EnumerateApps() {
    auto* out = new std::vector<AppEntry>();
    IShellItem* folder = nullptr;
    if (SUCCEEDED(SHGetKnownFolderItem(FOLDERID_AppsFolder, KF_FLAG_DEFAULT, nullptr, IID_PPV_ARGS(&folder)))) {
        IEnumShellItems* en = nullptr;
        if (SUCCEEDED(folder->BindToHandler(nullptr, BHID_EnumItems, IID_PPV_ARGS(&en)))) {
            IShellItem* it = nullptr;
            while (en->Next(1, &it, nullptr) == S_OK) {
                LPWSTR name = nullptr, parse = nullptr;
                if (SUCCEEDED(it->GetDisplayName(SIGDN_NORMALDISPLAY, &name)) &&
                    SUCCEEDED(it->GetDisplayName(SIGDN_PARENTRELATIVEPARSING, &parse))) {
                    AppEntry a;
                    a.name = name;
                    a.parse = parse;
                    a.lower = ToLower(a.name);
                    bool skip = a.lower.rfind(L"uninstall", 0) == 0 || a.lower.rfind(L"desinstalar", 0) == 0 ||
                                a.parse.rfind(L"http", 0) == 0;
                    if (!skip) out->push_back(std::move(a));
                }
                if (name) CoTaskMemFree(name);
                if (parse) CoTaskMemFree(parse);
                it->Release();
            }
            en->Release();
        }
        folder->Release();
    }
    std::sort(out->begin(), out->end(), [](const AppEntry& a, const AppEntry& b) {
        return CompareStringEx(LOCALE_NAME_USER_DEFAULT, LINGUISTIC_IGNORECASE, a.name.c_str(), -1,
                               b.name.c_str(), -1, nullptr, nullptr, 0) == CSTR_LESS_THAN;
    });
    return out;
}

class StartMenuWindow : public WindowBase, public EditKeySink {
public:
    std::function<void(int)> onAction; // acciones que necesita el shell (salir, restaurar Explorer)

    bool Start() {
        int w = S(440), h = S(600);
        if (!Create(L"OShellCppStartMenu", WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP | WS_CLIPCHILDREN,
                    0, 0, w, h, L"Inicio"))
            return false;
        SetFrame(FrameOpaque);
        wchar_t user[256] = {};
        DWORD un = 256;
        if (!GetUserNameExW_(user, &un)) GetUserNameW(user, &un);
        userName = user;
        search = CreateDarkEdit(hwnd, 200, L"Escribe para buscar aplicaciones");
        HookEditKeys(search, this);
        return true;
    }

    void ShowAt(int x, int bottomY) {
        RECT rc;
        GetWindowRect(hwnd, &rc);
        SetWindowTextW(search, L"");
        filter.clear();
        selected = 0;
        scroll = 0;
        Rebuild();
        SetWindowPos(hwnd, HWND_TOPMOST, x, bottomY - (rc.bottom - rc.top) - S(8), 0, 0, SWP_NOSIZE | SWP_SHOWWINDOW);
        ForceForeground(hwnd);
        SetFocus(search);
        Invalidate();
        RequestApps();
    }

    void Hide() {
        if (!IsWindowVisible(hwnd)) return;
        ShowWindow(hwnd, SW_HIDE);
        lastHide = GetTickCount64();
        ReleaseBuffers();
    }

    bool IsVisible() const { return IsWindowVisible(hwnd) != 0; }
    ULONGLONG LastHide() const { return lastHide; }

    bool OnEditKey(HWND, int vk) override {
        switch (vk) {
        case 0: return false;
        case VK_ESCAPE: Hide(); return true;
        case VK_RETURN: LaunchIndex(selected); return true;
        case VK_UP: MoveSel(-1); return true;
        case VK_DOWN: MoveSel(1); return true;
        case VK_PRIOR: MoveSel(-RowsVisible()); return true;
        case VK_NEXT: MoveSel(RowsVisible()); return true;
        }
        return false;
    }

protected:
    std::wstring userName;
    HWND search = nullptr;
    std::vector<AppEntry> apps;
    std::vector<int> view;
    std::wstring filter;
    int selected = 0;
    int scroll = 0;
    int hoverRow = -1;
    int hoverBtn = 0;
    bool loading = false;
    ULONGLONG lastHide = 0;
    ULONGLONG lastLoad = 0;
    std::map<std::wstring, Bitmap*> icons;   // por nombre de análisis
    std::map<std::wstring, bool> requested;

    RectF searchRc, listRc, btns[5];
    static constexpr int kBtnCount = 6;

    static BOOL GetUserNameExW_(wchar_t* buf, DWORD* len) {
        // Nombre para mostrar si existe (cuenta Microsoft / dominio); si no, el de inicio de sesión.
        typedef BOOLEAN(WINAPI* PFN)(int, LPWSTR, PULONG);
        HMODULE sec = LoadLibraryW(L"secur32.dll");
        if (!sec) return FALSE;
        auto fn = (PFN)GetProcAddress(sec, "GetUserNameExW");
        ULONG l = *len;
        BOOL ok = fn && fn(3 /*NameDisplay*/, buf, &l) && buf[0];
        FreeLibrary(sec);
        return ok;
    }

    int RowH() const { return S(38); }
    int RowsVisible() const { return std::max(1, (int)(listRc.Height / RowH())); }

    void Layout(int w, int h) {
        searchRc = RectF((float)S(16), (float)S(64), (float)w - S(32), (float)S(36));
        listRc = RectF((float)S(8), (float)S(110), (float)w - S(16), (float)h - S(110) - S(62));
        float bw = (float)S(44), y = (float)h - S(52);
        for (int i = 0; i < kBtnCount - 1; ++i) btns[i] = RectF((float)S(12) + i * (bw + S(4)), y, bw, bw);
        btns[kBtnCount - 1] = RectF((float)w - S(12) - bw, y, bw, bw);
    }

    void OnSize(UINT, int w, int h) override {
        Layout(w, h);
        if (search)
            MoveWindow(search, (int)searchRc.X + S(36), (int)searchRc.Y + S(8), (int)searchRc.Width - S(46),
                       (int)searchRc.Height - S(14), TRUE);
    }

    void OnCreate() override {
        RECT rc;
        GetClientRect(hwnd, &rc);
        Layout(rc.right, rc.bottom);
    }

    void RequestApps() {
        ULONGLONG now = GetTickCount64();
        if (loading || (!apps.empty() && now - lastLoad < 30000)) return;
        loading = true;
        HWND target = hwnd;
        BackgroundWorker().Post([target] {
            auto* list = EnumerateApps();
            if (!PostMessageW(target, WM_APPSLOADED, 0, (LPARAM)list)) delete list;
        });
    }

    void RequestIcon(const std::wstring& parse) {
        if (requested[parse]) return;
        requested[parse] = true;
        HWND target = hwnd;
        int size = S(24);
        BackgroundWorker().Post([target, parse, size] {
            auto* r = new IconResult();
            r->key = parse;
            IShellItemImageFactory* f = nullptr;
            std::wstring full = L"shell:AppsFolder\\" + parse;
            if (SUCCEEDED(SHCreateItemFromParsingName(full.c_str(), nullptr, IID_PPV_ARGS(&f)))) {
                f->GetImage(SIZE{ size, size }, SIIGBF_ICONONLY, &r->bmp);
                f->Release();
            }
            if (!PostMessageW(target, WM_APPICON, 0, (LPARAM)r)) {
                if (r->bmp) DeleteObject(r->bmp);
                delete r;
            }
        });
    }

    void Rebuild() {
        view.clear();
        std::wstring q = ToLower(filter);
        // Primero las que empiezan por el texto, luego las que lo contienen.
        for (int pass = 0; pass < 2; ++pass)
            for (int i = 0; i < (int)apps.size(); ++i) {
                size_t pos = q.empty() ? 0 : apps[i].lower.find(q);
                if (pos == std::wstring::npos) continue;
                if ((pass == 0) != (pos == 0)) continue;
                view.push_back(i);
            }
        if (selected >= (int)view.size()) selected = std::max(0, (int)view.size() - 1);
        if (!q.empty()) { selected = 0; scroll = 0; }
        ClampScroll();
    }

    void ClampScroll() {
        int vis = RowsVisible();
        if (selected < scroll) scroll = selected;
        if (selected >= scroll + vis) scroll = selected - vis + 1;
        scroll = std::max(0, std::min(scroll, std::max(0, (int)view.size() - vis)));
    }

    void MoveSel(int d) {
        if (view.empty()) return;
        selected = std::max(0, std::min((int)view.size() - 1, selected + d));
        ClampScroll();
        InvalidateRectF(listRc);
    }

    void LaunchIndex(int viewIdx) {
        if (viewIdx < 0 || viewIdx >= (int)view.size()) {
            // Sin coincidencias: intentar ejecutar el texto como comando.
            std::wstring t = Trim(filter);
            if (!t.empty() && RunCommandLine(t, hwnd)) Hide();
            return;
        }
        const AppEntry& a = apps[view[viewIdx]];
        Launch(L"shell:AppsFolder\\" + a.parse);
        Hide();
    }

    void OnPaint(Graphics& g, int w, int h) override {
        g.Clear(style::dialogBg);
        DrawRounded(g, RectF(0, 0, (float)w - 1, (float)h - 1), 0, Color(0, 0, 0, 0), style::cardBorder);

        // Cabecera: avatar con inicial + nombre
        RectF av((float)S(16), (float)S(14), (float)S(36), (float)S(36));
        DrawRounded(g, av, av.Width / 2, style::accentSolid);
        DrawTextStr(g, userName.substr(0, 1), fontBold, style::onAccent, av, StringAlignmentCenter);
        DrawTextStr(g, userName, fontBold, style::text, RectF((float)S(62), (float)S(14), (float)w - S(80), (float)S(20)));
        DrawTextStr(g, g_shellMode ? L"OptimizedShell · shell del sistema" : L"OptimizedShell", fontSmall,
                    style::textFaint, RectF((float)S(62), (float)S(32), (float)w - S(80), (float)S(18)));

        // Buscador (el texto lo pinta el EDIT nativo)
        DrawRounded(g, searchRc, SF(18), style::controlBg, style::controlBorder);
        DrawTextStr(g, L"\uE721", fontIcon, style::textDim,
                    RectF(searchRc.X + S(10), searchRc.Y, (float)S(20), searchRc.Height), StringAlignmentCenter);

        // Lista de aplicaciones
        int vis = RowsVisible();
        if (view.empty()) {
            std::wstring msg = loading && apps.empty() ? L"Cargando aplicaciones..."
                             : (filter.empty() ? L"No hay aplicaciones" : L"Sin coincidencias · Enter para ejecutarlo como comando");
            DrawTextStr(g, msg, fontSmall, style::textFaint,
                        RectF(listRc.X, listRc.Y + S(10), listRc.Width, (float)S(24)), StringAlignmentCenter);
        }
        for (int row = 0; row < vis; ++row) {
            int vi = scroll + row;
            if (vi >= (int)view.size()) break;
            const AppEntry& a = apps[view[vi]];
            RectF rr(listRc.X, listRc.Y + row * RowH(), listRc.Width - S(8), (float)RowH() - S(2));
            if (vi == selected) DrawRounded(g, rr, SF(6), style::rowSelected);
            else if (vi == hoverRow) DrawRounded(g, rr, SF(6), style::rowHover);
            int isz = S(24);
            auto it = icons.find(a.parse);
            if (it != icons.end() && it->second) {
                g.DrawImage(it->second, rr.X + S(10), rr.Y + (rr.Height - isz) / 2, (float)isz, (float)isz);
            } else {
                DrawRounded(g, RectF(rr.X + S(10), rr.Y + (rr.Height - isz) / 2, (float)isz, (float)isz), SF(5),
                            style::controlHover);
                if (it == icons.end()) RequestIcon(a.parse);
            }
            DrawTextStr(g, a.name, fontBody, style::text,
                        RectF(rr.X + S(44), rr.Y, rr.Width - S(52), rr.Height));
        }
        DrawScrollbar(g, RectF(listRc.X + listRc.Width - S(5), listRc.Y, (float)S(3), vis * (float)RowH()),
                      (int)view.size(), vis, scroll);

        // Barra inferior
        HLine(g, 0, (float)w, (float)h - S(62), style::separator);
        const wchar_t* glyphs[kBtnCount] = { L"\uEC50", L"\uE713", L"\uE9D9", L"\uE7C4", L"\uE771", L"\uE7E8" };
        for (int i = 0; i < kBtnCount; ++i)
            style::DrawToolButton(g, btns[i], glyphs[i], L"", hoverBtn == i + 1);
    }

    int RowAt(int x, int y) const {
        if (!InRect(listRc, x, y)) return -1;
        int vi = scroll + (int)((y - listRc.Y) / RowH());
        return vi < (int)view.size() ? vi : -1;
    }

    int BtnAt(int x, int y) const {
        for (int i = 0; i < kBtnCount; ++i)
            if (InRect(btns[i], x, y)) return i + 1;
        return 0;
    }

    void PowerMenu() {
        HMENU m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, 1, L"Bloquear");
        AppendMenuW(m, MF_STRING, 2, L"Cerrar sesión");
        AppendMenuW(m, MF_STRING, 3, L"Suspender");
        AppendMenuW(m, MF_STRING, 4, L"Reiniciar");
        AppendMenuW(m, MF_STRING, 5, L"Apagar");
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        if (g_shellMode) AppendMenuW(m, MF_STRING, 6, L"Restaurar Explorer (salir de OptimizedShell)");
        else AppendMenuW(m, MF_STRING, 7, L"Salir de OptimizedShell");
        POINT pt = { (LONG)btns[kBtnCount - 1].X, (LONG)btns[kBtnCount - 1].Y };
        ClientToScreen(hwnd, &pt);
        int cmd = ui::ThemedMenu(m, TPM_RETURNCMD | TPM_BOTTOMALIGN | TPM_RIGHTALIGN, pt.x + S(44), pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        if (!cmd) return;
        Hide();
        switch (cmd) {
        case 1: LockWorkStation(); break;
        case 2: ExitWindowsEx(EWX_LOGOFF, SHTDN_REASON_FLAG_PLANNED); break;
        case 3: SetSuspendState(FALSE, FALSE, FALSE); break;
        case 4: ExitWindowsEx(EWX_REBOOT, SHTDN_REASON_FLAG_PLANNED); break;
        case 5: ExitWindowsEx(EWX_POWEROFF, SHTDN_REASON_FLAG_PLANNED); break;
        case 6: if (onAction) onAction(1); break;
        case 7: if (onAction) onAction(2); break;
        }
    }

    void OnMouseDown(int x, int y) override {
        if (InRect(searchRc, x, y)) { SetFocus(search); return; }
        int vi = RowAt(x, y);
        if (vi >= 0) { LaunchIndex(vi); return; }
        switch (BtnAt(x, y)) {
        case 1: Hide(); OpenInExplorer(L""); break;
        case 2: Hide(); Launch(L"ms-settings:"); break;
        case 3: Hide(); LaunchOwn(L"--taskmgr"); break;
        case 4: Hide(); RunDialog::Show(); break;
        case 5: Hide(); LaunchOwn(L"--settings"); break;
        case 6: PowerMenu(); break;
        }
        SetFocus(search);
    }

    void OnMouseMove(int x, int y) override {
        int r = RowAt(x, y);
        int b = BtnAt(x, y);
        if (r != hoverRow) { hoverRow = r; InvalidateRectF(listRc); }
        if (b != hoverBtn) {
            hoverBtn = b;
            for (auto& br : btns) InvalidateRectF(br);
            static const wchar_t* tips[] = { L"", L"Archivos", L"Configuración", L"Administrador de tareas", L"Ejecutar", L"Personalización", L"Inicio/apagado" };
            tip.Set(b ? btns[b - 1] : RectF(), tips[b]);
        }
        if (r >= 0 || b) SetCursor(LoadCursorW(nullptr, IDC_HAND));
    }

    void OnWheel(int delta) override {
        int vis = RowsVisible();
        scroll -= (delta / WHEEL_DELTA) * 3;
        scroll = std::max(0, std::min(scroll, std::max(0, (int)view.size() - vis)));
        InvalidateRectF(listRc);
    }

    void OnCommand(int id, int code) override {
        if (id == 200 && code == EN_CHANGE) {
            filter = GetWindowString(search);
            Rebuild();
            InvalidateRectF(listRc);
        }
    }

    void OnActivate(bool active) override {
        if (!active) Hide();
    }

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        if (msg == WM_APPSLOADED) {
            std::unique_ptr<std::vector<AppEntry>> list((std::vector<AppEntry>*)lp);
            apps = std::move(*list);
            loading = false;
            lastLoad = GetTickCount64();
            Rebuild();
            if (IsVisible()) InvalidateRectF(listRc);
            return 0;
        }
        if (msg == WM_APPICON) {
            std::unique_ptr<IconResult> r((IconResult*)lp);
            Bitmap* b = r->bmp ? HBitmapToBitmap(r->bmp) : nullptr;
            if (r->bmp) DeleteObject(r->bmp);
            auto it = icons.find(r->key);
            if (it != icons.end()) delete it->second;
            icons[r->key] = b;
            if (IsVisible()) InvalidateRectF(listRc);
            return 0;
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }

    bool CanClose() override { return false; }

public:
    HoverTip tip;
};

// ---------------------------------------------------------------- flyouts de bandeja
class TrayFlyout;
TrayFlyout* g_openFlyout = nullptr;
int g_lastHiddenType = 0;
ULONGLONG g_lastHiddenTick = 0;

class TrayFlyout : public WindowBase {
public:
    int type = 0;

    bool Start(const wchar_t* cls, int w, int h, UINT refreshMs) {
        width = w;
        height = h;
        refreshInterval = refreshMs;
        if (!Create(cls, WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP, 0, 0, w, h, L"")) return false;
        SetFrame(FrameFlyout);
        return true;
    }

    void ShowAt(int anchorX, int taskbarTop) {
        int sw = GetSystemMetrics(SM_CXSCREEN);
        int x = anchorX - width + S(24);
        x = std::max(S(8), std::min(sw - width - S(8), x));
        int y = taskbarTop - height - S(8);
        OnShow();
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_SHOWWINDOW);
        ForceForeground(hwnd);
        // El timer solo existe mientras el flyout está visible.
        if (refreshInterval) SetTimer(hwnd, 1, refreshInterval, nullptr);
        Invalidate();
    }

    void Hide() {
        if (!hwnd) return;
        if (GetCapture() == hwnd) ReleaseCapture();
        KillTimer(hwnd, 1);
        ShowWindow(hwnd, SW_HIDE);
        ReleaseBuffers();
        if (g_openFlyout == this) {
            g_openFlyout = nullptr;
            g_lastHiddenType = type;
            g_lastHiddenTick = GetTickCount64();
        }
    }

    bool IsVisible() const { return hwnd && IsWindowVisible(hwnd); }

    virtual void Refresh() {}
    virtual void OnShow() { Refresh(); }

protected:
    int width = 0;
    int height = 0;
    UINT refreshInterval = 0;
    int hoverClose = 0;
    RectF closeRect;

    void PaintFrame(Graphics& g, int w, int h, const std::wstring& title, const wchar_t* glyph) {
        auto c = style::DrawFlyoutChrome(g, w, h, title, glyph, hoverClose);
        closeRect = c.close;
    }

    bool FrameClick(int x, int y) {
        if (InRect(closeRect, x, y)) {
            Hide();
            return true;
        }
        return false;
    }

    void OnTimer(UINT_PTR id) override {
        if (id == 1 && IsVisible()) Refresh();
    }

    void OnKeyDown(int vk) override {
        if (vk == VK_ESCAPE) Hide();
    }

    void OnActivate(bool active) override {
        if (!active) Hide();
    }

    bool CanClose() override { return false; }
};

// ---------------------------------------------------------------- volumen
class VolumeFlyout : public TrayFlyout {
public:
    bool Start() {
        type = 1;
        return TrayFlyout::Start(L"OShellCppVolumeFlyout", S(368), S(238), 1500);
    }

    ~VolumeFlyout() override { ReleaseAudio(); }

    void OnShow() override {
        ReleaseAudio(); // el dispositivo predeterminado puede haber cambiado
        Refresh();
    }

    void Refresh() override {
        EnsureAudio();
        float v = 0.0f;
        BOOL m = FALSE;
        if (vol) {
            vol->GetMasterVolumeLevelScalar(&v);
            vol->GetMute(&m);
        }
        bool changed = fabs(v - volume) > 0.001f || (m != FALSE) != muted;
        volume = v;
        muted = m != FALSE;
        if (changed && IsVisible()) Invalidate();
    }

    // Teclas multimedia (solo en modo shell; normalmente las gestiona Explorer).
    void Step(int dir) {
        EnsureAudio();
        if (!vol) return;
        HRESULT hr = dir > 0 ? vol->VolumeStepUp(nullptr) : vol->VolumeStepDown(nullptr);
        if (FAILED(hr)) { ReleaseAudio(); return; }
        if (dir > 0) vol->SetMute(FALSE, nullptr);
        Refresh();
    }

    void ToggleMute() {
        EnsureAudio();
        if (!vol) return;
        BOOL m = FALSE;
        vol->GetMute(&m);
        vol->SetMute(!m, nullptr);
        muted = !m;
        if (IsVisible()) Invalidate();
    }

protected:
    IMMDeviceEnumerator* enumr = nullptr;
    IAudioEndpointVolume* vol = nullptr;
    float volume = 0.0f;
    bool muted = false;
    bool dragging = false;
    int hoverMute = 0;
    int hoverMixer = 0;
    int hoverSound = 0;

    RectF slider, speaker, muteBtn, mixerBtn, soundBtn;

    void ReleaseAudio() {
        if (vol) { vol->Release(); vol = nullptr; }
    }

    void EnsureAudio() {
        if (vol) return;
        if (!enumr)
            CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
                             __uuidof(IMMDeviceEnumerator), (void**)&enumr);
        if (!enumr) return;
        IMMDevice* dev = nullptr;
        if (FAILED(enumr->GetDefaultAudioEndpoint(eRender, eConsole, &dev))) return;
        dev->Activate(__uuidof(IAudioEndpointVolume), CLSCTX_ALL, nullptr, (void**)&vol);
        dev->Release();
    }

    void SetVolume(float v) {
        v = std::max(0.0f, std::min(1.0f, v));
        if (vol) vol->SetMasterVolumeLevelScalar(v, nullptr);
        volume = v;
        if (vol) {
            BOOL m = FALSE;
            vol->GetMute(&m);
            muted = m != FALSE;
        }
        Invalidate();
    }

    void LayoutAll(int w, int h) {
        speaker = RectF((float)S(20), (float)S(52), (float)S(48), (float)S(48));
        slider = RectF((float)S(22), (float)S(124), (float)w - S(44), (float)S(8));
        muteBtn = RectF((float)S(20), (float)h - S(44), (float)S(150), (float)S(30));
        soundBtn = RectF((float)w - S(126), (float)h - S(44), (float)S(106), (float)S(30));
        mixerBtn = RectF((float)w - S(236), (float)h - S(44), (float)S(100), (float)S(30));
    }

    void OnSize(UINT, int w, int h) override { LayoutAll(w, h); }

    void OnPaint(Graphics& g, int w, int h) override {
        LayoutAll(w, h);
        PaintFrame(g, w, h, L"Volumen", L"\uE767");

        DrawSpeaker(g, speaker.X, speaker.Y, speaker.Width, muted ? style::textFaint : style::text);
        if (muted) {
            Pen p(style::danger, SF(2.2f));
            p.SetStartCap(LineCapRound);
            p.SetEndCap(LineCapRound);
            g.DrawLine(&p, speaker.X + SF(3), speaker.Y + speaker.Height - SF(3),
                       speaker.X + speaker.Width - SF(2), speaker.Y + SF(2));
        }

        wchar_t buf[32] = {};
        swprintf_s(buf, L"%d %%", (int)(volume * 100.0f + 0.5f));
        DrawTextStr(g, buf, fontBig, style::text,
                    RectF(speaker.X + speaker.Width + S(12), speaker.Y, (float)w - S(40) - speaker.Width, speaker.Height),
                    StringAlignmentFar);
        DrawTextStr(g, muted ? L"Silenciado" : L"Salida de audio predeterminada", fontSmall,
                    muted ? style::danger : style::textDim,
                    RectF(speaker.X + speaker.Width + S(12), speaker.Y + S(30),
                          (float)w - S(40) - speaker.Width, (float)S(18)),
                    StringAlignmentFar);

        style::DrawProgress(g, slider, volume, muted ? style::danger : style::accent);
        float thumbX = slider.X + slider.Width * volume;
        FillCircle(g, thumbX, slider.Y + slider.Height / 2, SF(9), muted ? style::danger : style::accentSolid);
        DrawCircle(g, thumbX, slider.Y + slider.Height / 2, SF(9), RGBc(255, 255, 255), 1.2f);

        DrawTextStr(g, muted ? L"Activar sonido" : L"Silenciar", fontSmall,
                    hoverMute ? style::accent : style::textDim, muteBtn, StringAlignmentNear);
        DrawTextStr(g, L"Mezclador", fontSmall, hoverMixer ? style::accent : style::textDim, mixerBtn,
                    StringAlignmentCenter);
        DrawTextStr(g, L"Sonido", fontSmall, hoverSound ? style::accent : style::textDim, soundBtn,
                    StringAlignmentCenter);
    }

    void OnMouseDown(int x, int y) override {
        if (FrameClick(x, y)) return;
        RectF sliderHit(slider.X, slider.Y - S(14), slider.Width, slider.Height + S(28));
        if (InRect(sliderHit, x, y)) {
            dragging = true;
            SetVolume((x - slider.X) / slider.Width);
            SetCapture(hwnd);
            return;
        }
        if (InRect(speaker, x, y) || InRect(muteBtn, x, y)) { ToggleMute(); return; }
        if (InRect(mixerBtn, x, y)) { Launch(L"sndvol.exe"); Hide(); return; }
        if (InRect(soundBtn, x, y)) { Launch(L"mmsys.cpl"); Hide(); return; }
    }

    void OnMouseUp(int, int) override {
        dragging = false;
        ReleaseCapture();
    }

    void OnMouseMove(int x, int y) override {
        if (dragging) {
            if (GetKeyState(VK_LBUTTON) & 0x8000) SetVolume((x - slider.X) / slider.Width);
            else { dragging = false; ReleaseCapture(); }
            return;
        }
        int nc = InRect(closeRect, x, y) ? 1 : 0;
        int nm = (InRect(speaker, x, y) || InRect(muteBtn, x, y)) ? 1 : 0;
        int nx = InRect(mixerBtn, x, y) ? 1 : 0;
        int ns = InRect(soundBtn, x, y) ? 1 : 0;
        bool hand = nc || nm || nx || ns ||
                    (x >= slider.X && x <= slider.X + slider.Width && y >= slider.Y - S(14) && y <= slider.Y + S(22));
        if (hand) SetCursor(LoadCursorW(nullptr, IDC_HAND));
        if (nc != hoverClose || nm != hoverMute || nx != hoverMixer || ns != hoverSound) {
            hoverClose = nc; hoverMute = nm; hoverMixer = nx; hoverSound = ns;
            Invalidate();
        }
    }

    void OnWheel(int delta) override { SetVolume(volume + (delta / WHEEL_DELTA) * 0.03f); }
};

// ---------------------------------------------------------------- wi-fi
struct WifiNetwork {
    std::wstring ssid;
    int quality = 0;
    bool secure = false;
    bool connected = false;
    bool hasProfile = false;
};

class WifiFlyout : public TrayFlyout {
public:
    bool Start() {
        type = 2;
        return TrayFlyout::Start(L"OShellCppWifiFlyout", S(392), S(408), 2500);
    }

    ~WifiFlyout() override {
        if (handle) WlanCloseHandle(handle, nullptr);
    }

    void OnShow() override {
        needScan = true;
        Refresh();
    }

    void Refresh() override {
        if (!Open()) return;
        ULONGLONG now = GetTickCount64();

        PWLAN_INTERFACE_INFO_LIST list = nullptr;
        if (WlanEnumInterfaces(handle, nullptr, &list) == ERROR_SUCCESS && list) {
            haveInterface = list->dwNumberOfItems > 0;
            if (haveInterface) {
                info = list->InterfaceInfo[0];
                radioOn = QueryRadio();
            }
            WlanFreeMemory(list);
        } else {
            haveInterface = false;
        }

        connectedSsid.clear();
        networks.clear();

        if (haveInterface) {
            DWORD size = 0;
            PWLAN_CONNECTION_ATTRIBUTES attr = nullptr;
            if (WlanQueryInterface(handle, &info.InterfaceGuid, wlan_intf_opcode_current_connection, nullptr,
                                   &size, (PVOID*)&attr, nullptr) == ERROR_SUCCESS && attr) {
                if (attr->isState == wlan_interface_state_connected)
                    connectedSsid = SsidToString(attr->wlanAssociationAttributes.dot11Ssid);
                WlanFreeMemory(attr);
            }

            if (radioOn && (needScan || now - lastScan > 15000)) {
                WlanScan(handle, &info.InterfaceGuid, nullptr, nullptr, nullptr);
                lastScan = now;
                needScan = false;
            }

            PWLAN_AVAILABLE_NETWORK_LIST avail = nullptr;
            DWORD listErr = WlanGetAvailableNetworkList(handle, &info.InterfaceGuid,
                                                        WLAN_AVAILABLE_NETWORK_INCLUDE_ALL_MANUAL_HIDDEN_PROFILES,
                                                        nullptr, &avail);
            // Desde Windows 11 24H2 las aplicaciones de escritorio necesitan permiso de
            // ubicación para ver las redes: sin él la API devuelve "acceso denegado".
            needLocation = listErr == ERROR_ACCESS_DENIED;
            if (listErr == ERROR_SUCCESS && avail) {
                for (DWORD i = 0; i < avail->dwNumberOfItems; ++i) {
                    const WLAN_AVAILABLE_NETWORK& n = avail->Network[i];
                    std::wstring ssid = SsidToString(n.dot11Ssid);
                    if (ssid.empty()) continue;
                    auto it = std::find_if(networks.begin(), networks.end(),
                                           [&](const WifiNetwork& x) { return x.ssid == ssid; });
                    if (it == networks.end()) {
                        WifiNetwork wn;
                        wn.ssid = ssid;
                        wn.quality = n.wlanSignalQuality;
                        wn.secure = n.bSecurityEnabled != 0;
                        wn.hasProfile = (n.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE) != 0;
                        wn.connected = (ssid == connectedSsid);
                        networks.push_back(wn);
                    } else {
                        it->quality = std::max(it->quality, (int)n.wlanSignalQuality);
                        it->secure = it->secure || n.bSecurityEnabled;
                        it->hasProfile = it->hasProfile || (n.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE);
                    }
                }
                WlanFreeMemory(avail);
            }
        }

        std::sort(networks.begin(), networks.end(), [](const WifiNetwork& a, const WifiNetwork& b) {
            if (a.connected != b.connected) return a.connected;
            return a.quality > b.quality;
        });
        if (scrollOffset > (int)networks.size()) scrollOffset = 0;
        if (IsVisible()) Invalidate();
    }

protected:
    HANDLE handle = nullptr;
    WLAN_INTERFACE_INFO info = {};
    bool haveInterface = false;
    bool radioOn = false;
    bool needScan = true;
    bool needLocation = false;   // Windows deniega la lista de redes sin permiso de ubicación
    bool requestingLocation = false;
    std::wstring locationMsg;
    RectF locationBtn;
    int hoverLocation = 0;

    // Pide el permiso en un hilo aparte (WinRT bloqueante); el resultado llega por mensaje.
    void RequestLocation() {
        if (requestingLocation) return;
        requestingLocation = true;
        locationMsg.clear();
        Invalidate();
        HWND target = hwnd;
        std::thread([target] {
            wrt::Access a = wrt::RequestLocation();
            PostMessageW(target, WM_APP + 33, (WPARAM)a, 0);
        }).detach();
    }

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        if (msg == WM_APP + 33) {
            requestingLocation = false;
            if ((wrt::Access)wp == wrt::Access::Allowed) {
                needScan = true;
                Refresh();
            } else {
                // Ubicación desactivada en el sistema o denegada: abrir la página de Windows.
                locationMsg = L"Activa \"Servicios de ubicación\" y \"Permitir que las aplicaciones de escritorio accedan a tu ubicación\".";
                Launch(L"ms-settings:privacy-location");
            }
            Invalidate();
            return 0;
        }
        return TrayFlyout::OnMessage(msg, wp, lp);
    }
    ULONGLONG lastScan = 0;
    std::wstring connectedSsid;
    std::vector<WifiNetwork> networks;
    int scrollOffset = 0;
    int hoverRow = -1;
    int hoverManage = 0;
    int hoverRadio = 0;
    int hoverRefresh = 0;

    RectF switchRect, rowArea, manageBtn, refreshBtn;

    bool Open() {
        if (handle) return true;
        DWORD neg = 0;
        if (WlanOpenHandle(2, nullptr, &neg, &handle) != ERROR_SUCCESS) {
            handle = nullptr;
            return false;
        }
        return true;
    }

    bool QueryRadio() {
        DWORD size = 0;
        PWLAN_RADIO_STATE state = nullptr;
        if (WlanQueryInterface(handle, &info.InterfaceGuid, wlan_intf_opcode_radio_state, nullptr,
                               &size, (PVOID*)&state, nullptr) != ERROR_SUCCESS || !state)
            return false;
        bool on = state->dwNumberOfPhys > 0 &&
                  state->PhyRadioState[0].dot11SoftwareRadioState == dot11_radio_state_on;
        WlanFreeMemory(state);
        return on;
    }

    void SetRadio(bool on) {
        if (!haveInterface) return;
        WLAN_PHY_RADIO_STATE rs = {};
        rs.dwPhyIndex = 0;
        rs.dot11SoftwareRadioState = on ? dot11_radio_state_on : dot11_radio_state_off;
        rs.dot11HardwareRadioState = dot11_radio_state_unknown;
        WlanSetInterface(handle, &info.InterfaceGuid, wlan_intf_opcode_radio_state, sizeof(rs), &rs, nullptr);
        radioOn = on;
        needScan = on;
        Sleep(120);
        Refresh();
    }

    static std::wstring SsidToString(const DOT11_SSID& ssid) {
        int len = (int)ssid.uSSIDLength;
        if (len <= 0) return L"";
        int wlen = MultiByteToWideChar(CP_UTF8, 0, (const char*)ssid.ucSSID, len, nullptr, 0);
        if (wlen <= 0) return L"";
        std::wstring out(wlen, L'\0');
        MultiByteToWideChar(CP_UTF8, 0, (const char*)ssid.ucSSID, len, &out[0], wlen);
        return out;
    }

    void Connect(const WifiNetwork& n) {
        if (!haveInterface) return;
        if (n.connected) {
            WlanDisconnect(handle, &info.InterfaceGuid, nullptr);
            connectedSsid.clear();
            Sleep(150);
            Refresh();
            return;
        }
        if (n.hasProfile) {
            WLAN_CONNECTION_PARAMETERS cp = {};
            cp.wlanConnectionMode = wlan_connection_mode_profile;
            cp.strProfile = n.ssid.c_str();
            cp.dot11BssType = dot11_BSS_type_any;
            if (WlanConnect(handle, &info.InterfaceGuid, &cp, nullptr) == ERROR_SUCCESS) {
                Sleep(350);
                Refresh();
                return;
            }
        }
        Launch(L"ms-settings:network-wifi");
        Hide();
    }

    int RowsVisible() const { return std::max(1, (int)(rowArea.Height / S(54))); }

    void LayoutAll(int w, int h) {
        switchRect = RectF((float)w - S(62), (float)S(52), (float)S(42), (float)S(22));
        rowArea = RectF((float)S(12), (float)S(116), (float)w - S(24), (float)h - S(168));
        manageBtn = RectF((float)S(16), (float)h - S(40), (float)S(150), (float)S(26));
        refreshBtn = RectF((float)w - S(126), (float)h - S(40), (float)S(110), (float)S(26));
    }

    void OnSize(UINT, int w, int h) override { LayoutAll(w, h); }

    void DrawSignal(Graphics& g, float x, float y, int quality, const Color& c) {
        for (int i = 0; i < 4; ++i) {
            float bh = SF(4.0f + i * 3.0f);
            bool on = quality >= (i + 1) * 22;
            SolidBrush bb(on ? c : style::track);
            g.FillRectangle(&bb, x + i * SF(5), y + SF(14) - bh, SF(3.2f), bh);
        }
    }

    void DrawLock(Graphics& g, float x, float y, const Color& c) {
        Pen p(c, SF(1.4f));
        g.DrawArc(&p, x + SF(2), y, SF(8), SF(8), 180.0f, 180.0f);
        SolidBrush b(c);
        g.FillRectangle(&b, x, y + SF(5), SF(12), SF(8));
    }

    void OnPaint(Graphics& g, int w, int h) override {
        LayoutAll(w, h);
        PaintFrame(g, w, h, L"Wi-Fi", L"\uE701");

        DrawTextStr(g, L"Wi-Fi", fontBody, style::text, RectF((float)S(20), (float)S(50), (float)S(160), (float)S(24)));
        std::wstring state = haveInterface ? (radioOn ? L"Activado" : L"Desactivado") : L"Sin adaptador";
        DrawTextStr(g, state, fontSmall, style::textFaint, RectF((float)S(20), (float)S(72), (float)S(160), (float)S(18)));
        style::DrawSwitch(g, switchRect, radioOn && haveInterface, hoverRadio != 0);
        HLine(g, (float)S(16), (float)w - S(16), (float)S(102), style::separator);

        RectF msgRc((float)S(16), (float)S(130), (float)w - S(32), (float)S(28));
        if (!haveInterface) {
            DrawTextStr(g, L"No se encontró un adaptador Wi-Fi.", fontBody, style::textDim, msgRc, StringAlignmentCenter);
        } else if (!radioOn) {
            DrawTextStr(g, L"Wi-Fi desactivado", fontBody, style::textDim, msgRc, StringAlignmentCenter);
            DrawTextStr(g, L"Actívalo para buscar redes cercanas.", fontSmall, style::textFaint,
                        RectF((float)S(16), (float)S(160), (float)w - S(32), (float)S(22)), StringAlignmentCenter);
        } else {
            int visible = RowsVisible();
            int maxOff = std::max(0, (int)networks.size() - visible);
            if (scrollOffset > maxOff) scrollOffset = maxOff;
            locationBtn = RectF();
            if (needLocation && networks.empty()) {
                DrawTextV(g, L"Windows necesita el permiso de ubicación para mostrar las redes Wi-Fi cercanas.",
                          fontBody, style::textDim, RectF((float)S(24), (float)S(122), (float)w - S(48), (float)S(48)),
                          StringAlignmentCenter, true);
                locationBtn = RectF((float)(w - S(200)) / 2, (float)S(180), (float)S(200), (float)S(34));
                style::DrawButton(g, locationBtn, requestingLocation ? L"Solicitando..." : L"Permitir ubicación",
                                  hoverLocation != 0, true, false, requestingLocation);
                if (!locationMsg.empty())
                    DrawTextV(g, locationMsg, fontSmall, style::textFaint,
                              RectF((float)S(24), (float)S(224), (float)w - S(48), (float)S(54)), StringAlignmentCenter, true);
            } else if (networks.empty()) {
                DrawTextStr(g, L"Buscando redes...", fontBody, style::textFaint, msgRc, StringAlignmentCenter);
            }
            for (int row = 0; row < visible; ++row) {
                int idx = scrollOffset + row;
                if (idx >= (int)networks.size()) break;
                const WifiNetwork& n = networks[idx];
                RectF rr(rowArea.X, rowArea.Y + row * S(54), rowArea.Width, (float)S(48));
                if (idx == hoverRow || n.connected)
                    DrawRounded(g, rr, SF(6), n.connected ? WithAlpha(style::success, 45) : style::rowHover);
                DrawSignal(g, rr.X + S(10), rr.Y + S(10), n.quality, n.connected ? style::success : style::textDim);
                float textX = rr.X + S(42);
                DrawTextStr(g, n.ssid, fontBody, style::text, RectF(textX, rr.Y + S(5), rr.Width - S(160), (float)S(22)));
                std::wstring sub;
                if (n.connected) sub = L"Conectado · clic para desconectar";
                else if (!n.hasProfile) sub = L"Sin perfil · abrir configuración";
                else if (n.secure) sub = L"Red protegida";
                else sub = L"Red abierta";
                DrawTextStr(g, sub, fontSmall, n.connected ? style::success : style::textFaint,
                            RectF(textX, rr.Y + S(24), rr.Width - S(110), (float)S(18)));
                if (n.secure) DrawLock(g, rr.X + rr.Width - S(28), rr.Y + S(17), style::textFaint);
                if (n.connected) {
                    wchar_t buf[16] = {};
                    swprintf_s(buf, L"%d %%", n.quality);
                    DrawTextStr(g, buf, fontSmall, style::success,
                                RectF(rr.X + rr.Width - S(76), rr.Y, (float)S(44), (float)S(44)), StringAlignmentFar);
                }
            }
            DrawScrollbar(g, RectF((float)w - S(10), rowArea.Y, (float)S(3), std::min(rowArea.Height, (float)visible * S(54))),
                          (int)networks.size(), visible, scrollOffset);
        }

        DrawTextStr(g, L"Administrar redes", fontSmall, hoverManage ? style::accent : style::textDim, manageBtn,
                    StringAlignmentNear);
        DrawTextStr(g, L"Actualizar", fontSmall, hoverRefresh ? style::accent : style::textDim, refreshBtn,
                    StringAlignmentFar);
    }

    void OnMouseDown(int x, int y) override {
        if (FrameClick(x, y)) return;
        if (InRect(switchRect, x, y)) { SetRadio(!radioOn); return; }
        if (InRect(manageBtn, x, y)) { Launch(L"ms-settings:network-wifi"); Hide(); return; }
        if (InRect(refreshBtn, x, y)) { needScan = true; Refresh(); return; }
        if (locationBtn.Width > 0 && InRect(locationBtn, x, y)) { RequestLocation(); return; }
        if (InRect(rowArea, x, y)) {
            int idx = scrollOffset + (int)((y - rowArea.Y) / S(54));
            if (idx >= 0 && idx < (int)networks.size()) Connect(networks[idx]);
        }
    }

    void OnMouseMove(int x, int y) override {
        int nc = InRect(closeRect, x, y) ? 1 : 0;
        int nr = InRect(switchRect, x, y) ? 1 : 0;
        int nm = InRect(manageBtn, x, y) ? 1 : 0;
        int nf = InRect(refreshBtn, x, y) ? 1 : 0;
        int nl = (locationBtn.Width > 0 && InRect(locationBtn, x, y)) ? 1 : 0;
        if (nl != hoverLocation) { hoverLocation = nl; Invalidate(); }
        if (nl) SetCursor(LoadCursorW(nullptr, IDC_HAND));
        int nrow = -1;
        if (InRect(rowArea, x, y) && !needLocation) {
            int row = (int)((y - rowArea.Y) / S(54));
            if (scrollOffset + row < (int)networks.size()) nrow = scrollOffset + row;
        }
        if (nc || nr || nm || nf || nrow >= 0) SetCursor(LoadCursorW(nullptr, IDC_HAND));
        if (nc != hoverClose || nr != hoverRadio || nm != hoverManage || nf != hoverRefresh || nrow != hoverRow) {
            hoverClose = nc; hoverRadio = nr; hoverManage = nm; hoverRefresh = nf; hoverRow = nrow;
            Invalidate();
        }
    }

    void OnWheel(int delta) override {
        int maxOff = std::max(0, (int)networks.size() - RowsVisible());
        scrollOffset -= (delta / WHEEL_DELTA) * 2;
        scrollOffset = std::max(0, std::min(maxOff, scrollOffset));
        Invalidate();
    }
};

// ---------------------------------------------------------------- batería
class BatteryFlyout : public TrayFlyout {
public:
    bool Start() {
        type = 3;
        return TrayFlyout::Start(L"OShellCppBatteryFlyout", S(372), S(300), 10000);
    }

    void Refresh() override {
        SYSTEM_POWER_STATUS s = {};
        if (GetSystemPowerStatus(&s)) {
            status = s;
            if (IsVisible()) Invalidate();
        }
    }

protected:
    SYSTEM_POWER_STATUS status = {};
    int hoverPower = 0;
    int hoverSaver = 0;
    RectF powerBtn, saverBtn;

    bool NoBattery() const { return status.BatteryFlag == 128 || status.BatteryLifePercent == 255; }

    std::wstring TimeLeft() const {
        if (NoBattery()) return L"—";
        if (status.BatteryLifeTime == (DWORD)-1 || status.BatteryLifeTime == 0)
            return status.ACLineStatus == 1 ? L"Conectado a la corriente" : L"Calculando...";
        DWORD sec = status.BatteryLifeTime;
        wchar_t buf[64] = {};
        swprintf_s(buf, L"%lu h %02lu min", sec / 3600, (sec % 3600) / 60);
        return buf;
    }

    std::wstring StateText() const {
        if (NoBattery()) return L"Sin batería";
        if (status.BatteryFlag & 8) return L"Cargando";
        if (status.ACLineStatus == 1) return L"Conectado a la corriente";
        return L"Usando la batería";
    }

    void DrawBigBattery(Graphics& g, const RectF& rc, int percent, bool charging, bool missing) {
        Color base = missing ? style::textFaint
                             : (percent <= 15 ? style::danger : (percent <= 35 ? style::warning : style::success));
        DrawRounded(g, rc, SF(6), style::track, WithAlpha(style::textDim, 160), 1.4f);
        RectF cap(rc.X + rc.Width, rc.Y + rc.Height * 0.30f, rc.Width * 0.06f, rc.Height * 0.40f);
        DrawRounded(g, cap, SF(2), WithAlpha(style::textDim, 160));
        if (!missing) {
            float inset = SF(5);
            RectF fill(rc.X + inset, rc.Y + inset, (rc.Width - inset * 2) * (percent / 100.0f), rc.Height - inset * 2);
            DrawRounded(g, fill, SF(4), base);
        }
        if (charging) {
            SolidBrush b(C(255, 255, 255, 255));
            PointF pts[3] = {
                PointF(rc.X + rc.Width * 0.48f, rc.Y + rc.Height * 0.13f),
                PointF(rc.X + rc.Width * 0.32f, rc.Y + rc.Height * 0.54f),
                PointF(rc.X + rc.Width * 0.48f, rc.Y + rc.Height * 0.54f),
            };
            PointF pts2[3] = {
                PointF(rc.X + rc.Width * 0.52f, rc.Y + rc.Height * 0.87f),
                PointF(rc.X + rc.Width * 0.68f, rc.Y + rc.Height * 0.46f),
                PointF(rc.X + rc.Width * 0.52f, rc.Y + rc.Height * 0.46f),
            };
            g.FillPolygon(&b, pts, 3);
            g.FillPolygon(&b, pts2, 3);
        }
    }

    void LayoutAll(int w, int h) {
        powerBtn = RectF((float)S(20), (float)h - S(46), (float)S(170), (float)S(32));
        saverBtn = RectF((float)w - S(186), (float)h - S(46), (float)S(166), (float)S(32));
    }

    void OnSize(UINT, int w, int h) override { LayoutAll(w, h); }

    void OnPaint(Graphics& g, int w, int h) override {
        LayoutAll(w, h);
        PaintFrame(g, w, h, L"Batería", L"\uE83F");

        int pct = status.BatteryLifePercent == 255 ? 100 : (int)status.BatteryLifePercent;
        bool missing = NoBattery();
        bool charging = (status.BatteryFlag & 8) != 0;
        DrawBigBattery(g, RectF((float)S(24), (float)S(60), (float)S(104), (float)S(50)), pct, charging, missing);

        wchar_t buf[32] = {};
        if (missing) wcscpy_s(buf, L"—");
        else swprintf_s(buf, L"%d %%", pct);
        DrawTextStr(g, buf, fontBig, style::text, RectF((float)S(148), (float)S(54), (float)w - S(172), (float)S(38)));
        DrawTextStr(g, StateText(), fontSmall, charging ? style::success : style::textDim,
                    RectF((float)S(148), (float)S(90), (float)w - S(172), (float)S(20)));
        HLine(g, (float)S(16), (float)w - S(16), (float)S(128), style::separator);

        struct Row { const wchar_t* label; std::wstring value; };
        Row rows[] = {
            { L"Tiempo restante", TimeLeft() },
            { L"Fuente de energía", status.ACLineStatus == 1 ? L"Corriente alterna" : L"Batería" },
            { L"Ahorro de batería", status.SystemStatusFlag == 1 ? L"Activado" : L"Desactivado" },
        };
        float y = (float)S(140);
        for (auto& r : rows) {
            DrawTextStr(g, r.label, fontSmall, style::textFaint, RectF((float)S(20), y, (float)S(160), (float)S(22)));
            DrawTextStr(g, r.value, fontSmall, style::text, RectF((float)S(170), y, (float)w - S(190), (float)S(22)),
                        StringAlignmentFar);
            y += S(26);
        }

        style::DrawButton(g, powerBtn, L"Opciones de energía", hoverPower != 0);
        style::DrawButton(g, saverBtn, L"Ahorro de batería", hoverSaver != 0);
    }

    void OnMouseDown(int x, int y) override {
        if (FrameClick(x, y)) return;
        if (InRect(powerBtn, x, y)) { Launch(L"powercfg.cpl"); Hide(); return; }
        if (InRect(saverBtn, x, y)) { Launch(L"ms-settings:batterysaver"); Hide(); return; }
    }

    void OnMouseMove(int x, int y) override {
        int nc = InRect(closeRect, x, y) ? 1 : 0;
        int np = InRect(powerBtn, x, y) ? 1 : 0;
        int ns = InRect(saverBtn, x, y) ? 1 : 0;
        if (nc || np || ns) SetCursor(LoadCursorW(nullptr, IDC_HAND));
        if (nc != hoverClose || np != hoverPower || ns != hoverSaver) {
            hoverClose = nc; hoverPower = np; hoverSaver = ns;
            Invalidate();
        }
    }
};

// ---------------------------------------------------------------- barra de tareas
struct TaskItem {
    HWND hwnd = nullptr;
    std::wstring title;
    HICON hicon = nullptr;
    Bitmap* icon = nullptr;
    bool flash = false;
    RectF rc;
};

// Estructura que Shell_NotifyIcon envía por WM_COPYDATA (dwData = 1). Los handles
// viajan como DWORD para que sea igual en procesos de 32 y 64 bits.
struct NotifyIconData32 {
    DWORD cbSize;
    DWORD hWnd;
    UINT uID;
    UINT uFlags;
    UINT uCallbackMessage;
    DWORD hIcon;
    WCHAR szTip[128];
    DWORD dwState;
    DWORD dwStateMask;
    WCHAR szInfo[256];
    UINT uVersion;
    WCHAR szInfoTitle[64];
    DWORD dwInfoFlags;
    GUID guidItem;
    DWORD hBalloonIcon;
};

struct TrayNotifyData {
    DWORD signature;
    DWORD message;
    NotifyIconData32 nid;
};

struct TrayIcon {
    HWND owner = nullptr;
    UINT id = 0;
    bool hasGuid = false;
    GUID guid = {};
    UINT callback = 0;
    UINT version = 0;
    HICON hicon = nullptr; // copia propia
    Bitmap* bmp = nullptr;
    std::wstring tip;
    bool hidden = false;
    RectF rc;
};

enum HoverKind { HNone, HStart, HTask, HTray, HVol, HNet, HBat, HClock, HDesk };

class TaskbarWindow : public WindowBase {
public:
    static TaskbarWindow* instance;
    StartMenuWindow startMenu;

    bool Start() {
        instance = this;
        int sw = GetSystemMetrics(SM_CXSCREEN);
        int sh = GetSystemMetrics(SM_CYSCREEN);
        h = S(48);
        // En modo shell la clase debe llamarse Shell_TrayWnd: así la encuentran
        // Shell_NotifyIcon y las aplicaciones que buscan la barra de tareas.
        const wchar_t* cls = g_shellMode ? L"Shell_TrayWnd" : L"OShellCppTaskbar";
        if (!Create(cls, WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP | WS_VISIBLE, 0, sh - h, sw, h,
                    L"Barra de tareas"))
            return false;
        g_taskbarHwnd = hwnd;
        SetFrame(FrameTaskbar);

        shellHookMsg = RegisterWindowMessageW(L"SHELLHOOK");
        RegisterShellHookWindow(hwnd);
        cloakHook = SetWinEventHook(EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED, nullptr, CloakProc, 0, 0,
                                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        batteryNotify = RegisterPowerSettingNotification(hwnd, &GUID_BATTERY_PERCENTAGE_REMAINING,
                                                         DEVICE_NOTIFY_WINDOW_HANDLE);
        ReadPower();
        UpdateClock();
        tip.Create(hwnd);

        // Lista inicial de ventanas (una sola vez; después todo llega por el shell hook).
        EnumWindows([](HWND w, LPARAM lp) -> BOOL {
            auto* self = (TaskbarWindow*)lp;
            if (self->IsTaskWindow(w)) self->AddTask(w);
            return TRUE;
        }, (LPARAM)this);
        activeHwnd = GetForegroundWindow();

        volumeFlyout.Start();
        wifiFlyout.Start();
        batteryFlyout.Start();
        startMenu.Start();
        startMenu.tip.Create(startMenu.Hwnd());
        startMenu.onAction = [this](int a) {
            if (a == 1) g_restoreExplorer = true;
            PostQuitMessage(0);
        };
        Relayout();
        return true;
    }

    ~TaskbarWindow() override {
        if (cloakHook) UnhookWinEvent(cloakHook);
        if (batteryNotify) UnregisterPowerSettingNotification(batteryNotify);
        if (hwnd) DeregisterShellHookWindow(hwnd);
        for (auto& t : tasks) delete t.icon;
        for (auto& t : tray) { delete t.bmp; if (t.hicon) DestroyIcon(t.hicon); }
        instance = nullptr;
    }

    int Height() const { return h; }

    void DoAction(int action) {
        switch (action) {
        case ActStart: ToggleStart(); break;
        case ActExplorer: OpenInExplorer(L""); break;
        case ActRun: RunDialog::Show(); break;
        case ActTaskMgr: LaunchOwn(L"--taskmgr"); break;
        case ActShowDesktop: ShowDesktop(); break;
        }
    }

    void FitToScreen() {
        int sw = GetSystemMetrics(SM_CXSCREEN), sh = GetSystemMetrics(SM_CYSCREEN);
        SetWindowPos(hwnd, HWND_TOPMOST, 0, sh - h, sw, h, SWP_NOACTIVATE);
        if (g_shellMode) {
            RECT wa = { 0, 0, sw, sh - h };
            SystemParametersInfoW(SPI_SETWORKAREA, 0, &wa, SPIF_SENDCHANGE);
        }
    }

protected:
    int h = 0;
    UINT shellHookMsg = 0;
    HWINEVENTHOOK cloakHook = nullptr;
    HPOWERNOTIFY batteryNotify = nullptr;
    std::vector<TaskItem> tasks;
    std::vector<TrayIcon> tray;
    std::vector<HWND> minimizedByUs;
    HWND activeHwnd = nullptr;
    bool topmost = true;
    HoverTip tip;

    RectF startRc, volRc, netRc, batRc, clockRc, deskRc;
    std::wstring timeText, dateText;
    SYSTEM_POWER_STATUS power = {};
    HoverKind hoverKind = HNone;
    int hoverIndex = -1;
    int pressedTray = -1;

    VolumeFlyout volumeFlyout;
    WifiFlyout wifiFlyout;
    BatteryFlyout batteryFlyout;

    // ------------------------------------------------------------ ventanas
    static void CALLBACK CloakProc(HWINEVENTHOOK, DWORD, HWND w, LONG idObject, LONG idChild, DWORD, DWORD) {
        if (idObject == OBJID_WINDOW && idChild == CHILDID_SELF && instance && instance->hwnd)
            PostMessageW(instance->hwnd, WM_CLOAKCHANGED, 0, (LPARAM)w);
    }

    bool IsTaskWindow(HWND w) const {
        if (!IsWindow(w) || !IsWindowVisible(w)) return false;
        DWORD pid = 0;
        GetWindowThreadProcessId(w, &pid);
        if (pid == GetCurrentProcessId()) return false;
        LONG ex = GetWindowLongW(w, GWL_EXSTYLE);
        if (!(ex & WS_EX_APPWINDOW)) {
            if (ex & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE)) return false;
            if (GetWindow(w, GW_OWNER)) return false;
        }
        DWORD cloaked = 0;
        if (SUCCEEDED(DwmGetWindowAttribute(w, DWMWA_CLOAKED, &cloaked, sizeof(cloaked))) && cloaked) return false;
        return true;
    }

    TaskItem* FindTask(HWND w) {
        for (auto& t : tasks)
            if (t.hwnd == w) return &t;
        return nullptr;
    }

    static std::wstring TitleOf(HWND w) {
        // Para ventanas de otros procesos GetWindowText lee el título guardado
        // sin enviar mensajes: no se bloquea si la aplicación está colgada.
        wchar_t buf[512] = {};
        GetWindowTextW(w, buf, 512);
        return buf;
    }

    void AddTask(HWND w) {
        if (FindTask(w)) return;
        TaskItem t;
        t.hwnd = w;
        t.title = TitleOf(w);
        tasks.push_back(t);
        RequestIcon(w, 0);
    }

    void RemoveTask(HWND w) {
        for (size_t i = 0; i < tasks.size(); ++i)
            if (tasks[i].hwnd == w) {
                delete tasks[i].icon;
                tasks.erase(tasks.begin() + i);
                break;
            }
        minimizedByUs.erase(std::remove(minimizedByUs.begin(), minimizedByUs.end(), w), minimizedByUs.end());
    }

    // El icono se pide de forma asíncrona: una aplicación colgada nunca bloquea la barra.
    static void CALLBACK IconReply(HWND w, UINT, ULONG_PTR stage, LRESULT result) {
        if (instance) instance->OnIconReply(w, (int)stage, (HICON)result);
    }

    void RequestIcon(HWND w, int stage) {
        WPARAM kind = stage == 0 ? ICON_SMALL2 : (stage == 1 ? ICON_SMALL : ICON_BIG);
        if (!SendMessageCallbackW(w, WM_GETICON, kind, 0, IconReply, (ULONG_PTR)stage))
            OnIconReply(w, 3, nullptr);
    }

    void OnIconReply(HWND w, int stage, HICON hi) {
        TaskItem* t = FindTask(w);
        if (!t) return;
        if (!hi && stage < 2) { RequestIcon(w, stage + 1); return; }
        if (!hi) hi = (HICON)GetClassLongPtrW(w, GCLP_HICONSM);
        if (!hi) hi = (HICON)GetClassLongPtrW(w, GCLP_HICON);
        if (!hi) hi = LoadIconW(nullptr, IDI_APPLICATION);
        if (hi == t->hicon && t->icon) return;
        delete t->icon;
        t->icon = IconToBitmap(hi, S(20));
        t->hicon = hi;
        InvalidateRectF(t->rc);
    }

    void OnShellHook(WPARAM code, LPARAM lp) {
        HWND w = (HWND)lp;
        switch (code & 0x7FFF) {
        case HSHELL_WINDOWCREATED:
            if (IsTaskWindow(w)) { AddTask(w); Relayout(); }
            break;
        case HSHELL_WINDOWDESTROYED:
            if (FindTask(w)) { RemoveTask(w); Relayout(); }
            PruneTray();
            break;
        case HSHELL_WINDOWACTIVATED: { // incluye HSHELL_RUDEAPPACTIVATED (bit alto)
            if (w && !FindTask(w) && IsTaskWindow(w)) { AddTask(w); Relayout(); }
            HWND old = activeHwnd;
            if (w) activeHwnd = w;
            if (TaskItem* t = FindTask(w)) { t->flash = false; InvalidateRectF(t->rc); }
            if (TaskItem* o = FindTask(old)) InvalidateRectF(o->rc);
            UpdateTopmost(w);
            break;
        }
        case HSHELL_REDRAW:
            if (TaskItem* t = FindTask(w)) {
                if (code & 0x8000) t->flash = true; // HSHELL_FLASH
                std::wstring title = TitleOf(w);
                if (title != t->title) { t->title = title; }
                RequestIcon(w, 0);
                InvalidateRectF(t->rc);
                if (hoverKind == HTask && hoverIndex >= 0 && hoverIndex < (int)tasks.size() && tasks[hoverIndex].hwnd == w)
                    tip.Set(t->rc, t->title);
            }
            break;
        case HSHELL_WINDOWREPLACED:
            if (TaskItem* t = FindTask(w)) RemoveTask(t->hwnd), Relayout();
            break;
        case HSHELL_APPCOMMAND:
            if (g_shellMode) {
                switch (GET_APPCOMMAND_LPARAM(lp)) {
                case APPCOMMAND_VOLUME_MUTE: volumeFlyout.ToggleMute(); break;
                case APPCOMMAND_VOLUME_DOWN: volumeFlyout.Step(-1); break;
                case APPCOMMAND_VOLUME_UP: volumeFlyout.Step(1); break;
                }
            }
            break;
        }
    }

    // Una aplicación a pantalla completa (vídeo, juego) no debe quedar tapada por la barra.
    void UpdateTopmost(HWND fg) {
        bool full = false;
        if (fg && FindTask(fg)) {
            RECT wr;
            MONITORINFO mi = { sizeof(mi) };
            if (GetWindowRect(fg, &wr) && GetMonitorInfoW(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST), &mi))
                full = wr.left <= mi.rcMonitor.left && wr.top <= mi.rcMonitor.top &&
                       wr.right >= mi.rcMonitor.right && wr.bottom >= mi.rcMonitor.bottom;
        }
        if (full == !topmost) return;
        topmost = !full;
        SetWindowPos(hwnd, topmost ? HWND_TOPMOST : HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }

    void ActivateTask(HWND w) {
        if (IsIconic(w)) {
            ShowWindowAsync(w, SW_RESTORE);
            ForceForeground(w);
        } else if (w == activeHwnd && GetForegroundWindow() == w) {
            ShowWindowAsync(w, SW_MINIMIZE);
        } else if (w == activeHwnd && GetForegroundWindow() == hwnd) {
            // Al hacer clic la barra toma el foco: la ventana activa era 'w'.
            ShowWindowAsync(w, SW_MINIMIZE);
        } else {
            ForceForeground(w);
        }
    }

    void ShowDesktop() {
        bool allStillMin = !minimizedByUs.empty();
        for (HWND w : minimizedByUs)
            if (IsWindow(w) && !IsIconic(w)) allStillMin = false;
        if (allStillMin) {
            for (HWND w : minimizedByUs)
                if (IsWindow(w)) ShowWindowAsync(w, SW_RESTORE);
            minimizedByUs.clear();
            return;
        }
        minimizedByUs.clear();
        for (auto& t : tasks)
            if (!IsIconic(t.hwnd)) {
                ShowWindowAsync(t.hwnd, SW_MINIMIZE);
                minimizedByUs.push_back(t.hwnd);
            }
    }

    void TaskMenu(int i, POINT pt) {
        HWND w = tasks[i].hwnd;
        HMENU m = CreatePopupMenu();
        bool iconic = IsIconic(w) != FALSE, zoomed = IsZoomed(w) != FALSE;
        AppendMenuW(m, MF_STRING | (iconic || zoomed ? 0 : MF_GRAYED), 1, L"Restaurar");
        AppendMenuW(m, MF_STRING | (iconic ? MF_GRAYED : 0), 2, L"Minimizar");
        AppendMenuW(m, MF_STRING | (zoomed ? MF_GRAYED : 0), 3, L"Maximizar");
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING, 4, L"Cerrar ventana");
        SetForegroundWindow(hwnd);
        int cmd = ui::ThemedMenu(m, TPM_RETURNCMD | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        switch (cmd) {
        case 1: ShowWindowAsync(w, SW_RESTORE); ForceForeground(w); break;
        case 2: ShowWindowAsync(w, SW_MINIMIZE); break;
        case 3: ShowWindowAsync(w, SW_MAXIMIZE); ForceForeground(w); break;
        case 4: PostMessageW(w, WM_SYSCOMMAND, SC_CLOSE, 0); break;
        }
    }

    // ------------------------------------------------------------ bandeja (modo shell)
    TrayIcon* FindTray(const NotifyIconData32& n, bool useGuid) {
        for (auto& t : tray) {
            if (useGuid) { if (t.hasGuid && IsEqualGUID(t.guid, n.guidItem)) return &t; }
            else if (t.owner == (HWND)(UINT_PTR)n.hWnd && t.id == n.uID) return &t;
        }
        return nullptr;
    }

    BOOL OnTrayMessage(const TrayNotifyData* d, DWORD size) {
        if (size < sizeof(DWORD) * 2 + offsetof(NotifyIconData32, szTip)) return FALSE;
        const NotifyIconData32& n = d->nid;
        DWORD cb = std::min<DWORD>(n.cbSize, size - sizeof(DWORD) * 2);
        auto has = [&](size_t endOffset) { return cb >= endOffset; };
        bool useGuid = (n.uFlags & NIF_GUID) && has(offsetof(NotifyIconData32, guidItem) + sizeof(GUID));
        TrayIcon* t = FindTray(n, useGuid);

        switch (d->message) {
        case NIM_ADD:
        case NIM_MODIFY: {
            if (!t) {
                if (d->message == NIM_MODIFY) return FALSE;
                tray.push_back(TrayIcon());
                t = &tray.back();
                t->owner = (HWND)(UINT_PTR)n.hWnd;
                t->id = n.uID;
                if (useGuid) { t->hasGuid = true; t->guid = n.guidItem; }
            }
            if (n.uFlags & NIF_MESSAGE) t->callback = n.uCallbackMessage;
            if (n.uFlags & NIF_ICON) {
                HICON src = (HICON)(UINT_PTR)n.hIcon;
                if (t->hicon) DestroyIcon(t->hicon);
                t->hicon = src ? CopyIcon(src) : nullptr;
                delete t->bmp;
                t->bmp = t->hicon ? IconToBitmap(t->hicon, S(16)) : nullptr;
            }
            if (n.uFlags & NIF_TIP) {
                size_t maxChars = has(offsetof(NotifyIconData32, dwState)) ? 128 : 64;
                t->tip.assign(n.szTip, wcsnlen(n.szTip, maxChars));
            }
            if ((n.uFlags & NIF_STATE) && has(offsetof(NotifyIconData32, dwStateMask) + sizeof(DWORD)) &&
                (n.dwStateMask & NIS_HIDDEN))
                t->hidden = (n.dwState & NIS_HIDDEN) != 0;
            Relayout();
            return TRUE;
        }
        case NIM_DELETE:
            if (!t) return FALSE;
            if (t->hicon) DestroyIcon(t->hicon);
            delete t->bmp;
            tray.erase(tray.begin() + (t - tray.data()));
            Relayout();
            return TRUE;
        case NIM_SETVERSION:
            if (!t) return FALSE;
            t->version = has(offsetof(NotifyIconData32, uVersion) + sizeof(UINT)) ? n.uVersion : 0;
            return TRUE;
        case NIM_SETFOCUS:
            return t ? TRUE : FALSE;
        }
        return FALSE;
    }

    void PruneTray() {
        bool changed = false;
        for (size_t i = 0; i < tray.size();) {
            if (!IsWindow(tray[i].owner)) {
                if (tray[i].hicon) DestroyIcon(tray[i].hicon);
                delete tray[i].bmp;
                tray.erase(tray.begin() + i);
                changed = true;
            } else {
                ++i;
            }
        }
        if (changed) Relayout();
    }

    void SendTrayMouse(TrayIcon& t, UINT mouseMsg) {
        if (!IsWindow(t.owner)) { PruneTray(); return; }
        if (!t.callback) return;
        DWORD pid = 0;
        GetWindowThreadProcessId(t.owner, &pid);
        AllowSetForegroundWindow(pid);
        POINT pt;
        GetCursorPos(&pt);
        if (t.version >= 4) {
            PostMessageW(t.owner, t.callback, MAKEWPARAM(pt.x, pt.y), MAKELPARAM(mouseMsg, t.id));
            if (mouseMsg == WM_LBUTTONUP)
                PostMessageW(t.owner, t.callback, MAKEWPARAM(pt.x, pt.y), MAKELPARAM(NIN_SELECT, t.id));
            else if (mouseMsg == WM_RBUTTONUP)
                PostMessageW(t.owner, t.callback, MAKEWPARAM(pt.x, pt.y), MAKELPARAM(WM_CONTEXTMENU, t.id));
        } else {
            PostMessageW(t.owner, t.callback, t.id, mouseMsg);
        }
    }

    int TrayAt(int x, int y) const {
        for (size_t i = 0; i < tray.size(); ++i)
            if (!tray[i].hidden && InRect(tray[i].rc, x, y)) return (int)i;
        return -1;
    }

    // ------------------------------------------------------------ energía y reloj
    void ReadPower() {
        GetSystemPowerStatus(&power);
    }

    bool HasBattery() const { return !(power.BatteryFlag & 128) && power.BatteryLifePercent != 255; }

    void UpdateClock() {
        SYSTEMTIME st;
        GetLocalTime(&st);
        wchar_t buf[80] = {};
        if (GetTimeFormatEx(LOCALE_NAME_USER_DEFAULT, TIME_NOSECONDS, &st, nullptr, buf, 80) > 0) timeText = buf;
        if (GetDateFormatEx(LOCALE_NAME_USER_DEFAULT, DATE_SHORTDATE, &st, nullptr, buf, 80, nullptr) > 0) dateText = buf;
        InvalidateRectF(clockRc);
        // Despertar justo en el siguiente cambio de minuto (y no cada segundo).
        UINT ms = (UINT)((60 - st.wSecond) * 1000 - st.wMilliseconds + 30);
        SetTimer(hwnd, 1, ms, nullptr);
    }

    // ------------------------------------------------------------ layout y pintado
    void Relayout() {
        RECT rc;
        GetClientRect(hwnd, &rc);
        float w = (float)rc.right, fh = (float)h;
        startRc = RectF(0, 0, (float)S(56), fh);
        deskRc = RectF(w - S(8), 0, (float)S(8), fh);
        clockRc = RectF(deskRc.X - S(104), 0, (float)S(100), fh);
        float sx = clockRc.X - S(4);
        float sysW = (float)S(30);
        if (HasBattery()) { sx -= sysW; batRc = RectF(sx, 0, sysW, fh); } else batRc = RectF();
        sx -= sysW; netRc = RectF(sx, 0, sysW, fh);
        sx -= sysW; volRc = RectF(sx, 0, sysW, fh);

        // Iconos de bandeja de otras aplicaciones, de derecha a izquierda.
        float tx = sx - S(6);
        for (int i = (int)tray.size() - 1; i >= 0; --i) {
            if (tray[i].hidden) { tray[i].rc = RectF(); continue; }
            tx -= S(28);
            tray[i].rc = RectF(tx, (float)S(8), (float)S(28), fh - S(16));
        }

        float left = startRc.X + startRc.Width + S(4);
        float right = tx - S(10);
        int n = (int)tasks.size();
        float bw = n ? std::min((float)S(190), (right - left) / n - S(4)) : 0;
        bw = std::max(bw, (float)S(44));
        for (int i = 0; i < n; ++i) {
            float x = left + i * (bw + S(4));
            tasks[i].rc = (x + bw <= right) ? RectF(x, (float)S(5), bw, fh - S(10)) : RectF();
        }
        Invalidate();
    }

    void OnSize(UINT, int, int) override { Relayout(); }

    void OnPaint(Graphics& g, int w, int hh) override {
        g.Clear(style::taskbarBg);
        HLine(g, 0, (float)w, 0, style::separator);

        // Inicio: cuatro cuadrados de acento
        if (hoverKind == HStart || startMenu.IsVisible())
            DrawRounded(g, RectF(startRc.X + S(6), (float)S(5), startRc.Width - S(12), (float)hh - S(10)), SF(6),
                        style::taskButtonHover);
        {
            float d = SF(7), gap = SF(4);
            float total = d * 2 + gap;
            float sx = startRc.X + (startRc.Width - total) / 2;
            float sy = (hh - total) / 2;
            SolidBrush b(style::accent);
            for (int r = 0; r < 2; ++r)
                for (int c = 0; c < 2; ++c)
                    g.FillRectangle(&b, sx + c * (d + gap), sy + r * (d + gap), d, d);
        }

        // Tareas
        for (size_t i = 0; i < tasks.size(); ++i) {
            const TaskItem& t = tasks[i];
            if (t.rc.Width <= 0) continue;
            bool active = t.hwnd == activeHwnd;
            bool hov = hoverKind == HTask && (int)i == hoverIndex;
            Color bg = t.flash ? Mix(style::taskButton, style::warning, 0.45f)
                     : (active ? style::taskButtonActive : (hov ? style::taskButtonHover : style::taskButton));
            DrawRounded(g, t.rc, SF(6), bg);
            if (active)
                DrawRounded(g, RectF(t.rc.X + t.rc.Width / 2 - S(10), t.rc.Y + t.rc.Height - S(3), (float)S(20), (float)S(3)),
                            SF(1.5f), style::accent);
            int isz = S(20);
            bool iconOnly = t.rc.Width < S(80);
            float ix = iconOnly ? t.rc.X + (t.rc.Width - isz) / 2 : t.rc.X + S(10);
            float iy = t.rc.Y + (t.rc.Height - isz) / 2;
            if (t.icon) g.DrawImage(t.icon, ix, iy, (float)isz, (float)isz);
            if (!iconOnly)
                DrawTextStr(g, t.title, fontSmall, style::text,
                            RectF(ix + isz + S(8), t.rc.Y, t.rc.X + t.rc.Width - (ix + isz + S(14)), t.rc.Height));
        }

        // Bandeja de aplicaciones
        for (size_t i = 0; i < tray.size(); ++i) {
            const TrayIcon& t = tray[i];
            if (t.hidden || t.rc.Width <= 0) continue;
            if (hoverKind == HTray && (int)i == hoverIndex) DrawRounded(g, t.rc, SF(5), style::taskButtonHover);
            int isz = S(16);
            if (t.bmp) g.DrawImage(t.bmp, t.rc.X + (t.rc.Width - isz) / 2, t.rc.Y + (t.rc.Height - isz) / 2,
                                   (float)isz, (float)isz);
        }

        // Iconos del sistema
        auto sysHover = [&](HoverKind k, const RectF& r) {
            if (hoverKind == k) DrawRounded(g, RectF(r.X + S(1), (float)S(8), r.Width - S(2), (float)hh - S(16)), SF(5),
                                            style::taskButtonHover);
        };
        Color ic = style::text;
        float iy = (hh - SF(16)) / 2;
        sysHover(HVol, volRc);
        DrawSpeaker(g, volRc.X + SF(7), iy, SF(16), ic);
        sysHover(HNet, netRc);
        DrawNetwork(g, netRc.X + SF(8), iy + SF(1), SF(14), ic);
        if (batRc.Width > 0) {
            sysHover(HBat, batRc);
            DrawBattery(g, batRc.X + SF(7), iy, SF(16), ic, power.BatteryLifePercent, (power.BatteryFlag & 8) != 0);
        }

        // Reloj
        if (hoverKind == HClock) DrawRounded(g, RectF(clockRc.X, (float)S(5), clockRc.Width, (float)hh - S(10)), SF(5),
                                             style::taskButtonHover);
        DrawTextStr(g, timeText, fontBody, style::text, RectF(clockRc.X, SF(4), clockRc.Width, SF(22)), StringAlignmentCenter);
        DrawTextStr(g, dateText, fontSmall, style::textDim, RectF(clockRc.X, SF(24), clockRc.Width, SF(18)),
                    StringAlignmentCenter);

        // Mostrar escritorio
        Pen sep(hoverKind == HDesk ? style::accent : style::track, 1.0f);
        g.DrawLine(&sep, deskRc.X, (float)S(12), deskRc.X, (float)hh - S(12));
    }

    // ------------------------------------------------------------ ratón
    RectF HoverRect(HoverKind k, int idx) const {
        switch (k) {
        case HStart: return startRc;
        case HTask: return (idx >= 0 && idx < (int)tasks.size()) ? tasks[idx].rc : RectF();
        case HTray: return (idx >= 0 && idx < (int)tray.size()) ? tray[idx].rc : RectF();
        case HVol: return volRc;
        case HNet: return netRc;
        case HBat: return batRc;
        case HClock: return clockRc;
        case HDesk: return deskRc;
        default: return RectF();
        }
    }

    void HitTest(int x, int y, HoverKind& k, int& idx) const {
        idx = -1;
        k = HNone;
        if (x < 0) return;
        if (InRect(startRc, x, y)) { k = HStart; return; }
        for (size_t i = 0; i < tasks.size(); ++i)
            if (tasks[i].rc.Width > 0 && InRect(tasks[i].rc, x, y)) { k = HTask; idx = (int)i; return; }
        int t = TrayAt(x, y);
        if (t >= 0) { k = HTray; idx = t; return; }
        if (InRect(volRc, x, y)) k = HVol;
        else if (InRect(netRc, x, y)) k = HNet;
        else if (batRc.Width > 0 && InRect(batRc, x, y)) k = HBat;
        else if (InRect(clockRc, x, y)) k = HClock;
        else if (InRect(deskRc, x, y)) k = HDesk;
    }

    std::wstring TipFor(HoverKind k, int idx) const {
        wchar_t buf[96] = {};
        switch (k) {
        case HStart: return L"Inicio";
        case HTask: return tasks[idx].title;
        case HTray: return tray[idx].tip;
        case HVol: return L"Volumen";
        case HNet: return L"Red";
        case HBat:
            swprintf_s(buf, L"Batería: %d %%%s", (int)power.BatteryLifePercent, (power.BatteryFlag & 8) ? L" (cargando)" : L"");
            return buf;
        case HClock: return dateText;
        case HDesk: return L"Mostrar escritorio";
        default: return L"";
        }
    }

    void OnMouseMove(int x, int y) override {
        HoverKind k;
        int idx;
        HitTest(x, y, k, idx);
        if (k == HTray && idx >= 0) SendTrayMouse(tray[idx], WM_MOUSEMOVE);
        if (k == hoverKind && idx == hoverIndex) return;
        InvalidateRectF(HoverRect(hoverKind, hoverIndex));
        hoverKind = k;
        hoverIndex = idx;
        InvalidateRectF(HoverRect(k, idx));
        tip.Set(HoverRect(k, idx), k == HNone ? L"" : TipFor(k, idx));
    }

    void ToggleFlyout(TrayFlyout& f, const RectF& icon) {
        POINT pt = { (LONG)(icon.X + icon.Width / 2), 0 };
        ClientToScreen(hwnd, &pt);
        if (g_openFlyout) {
            bool same = (g_openFlyout == &f);
            g_openFlyout->Hide();
            if (same) return;
        }
        // El clic que cerró el flyout (al perder el foco) no debe volver a abrirlo.
        if (f.type == g_lastHiddenType && GetTickCount64() - g_lastHiddenTick < 400) {
            g_lastHiddenType = 0;
            return;
        }
        f.ShowAt(pt.x, pt.y);
        g_openFlyout = &f;
    }

    void ToggleStart() {
        if (startMenu.IsVisible()) { startMenu.Hide(); InvalidateRectF(startRc); return; }
        if (GetTickCount64() - startMenu.LastHide() < 300) return; // mismo clic que lo cerró
        RECT wr;
        GetWindowRect(hwnd, &wr);
        startMenu.ShowAt(wr.left + S(8), wr.top);
        InvalidateRectF(startRc);
    }

    void OnMouseDown(int x, int y) override {
        HoverKind k;
        int idx;
        HitTest(x, y, k, idx);
        if (k == HTray) { pressedTray = idx; SendTrayMouse(tray[idx], WM_LBUTTONDOWN); }
    }

    void OnMouseUp(int x, int y) override {
        HoverKind k;
        int idx;
        HitTest(x, y, k, idx);
        switch (k) {
        case HStart: ToggleStart(); break;
        case HTask: ActivateTask(tasks[idx].hwnd); break;
        case HTray: if (idx == pressedTray) SendTrayMouse(tray[idx], WM_LBUTTONUP); break;
        case HVol: ToggleFlyout(volumeFlyout, volRc); break;
        case HNet: ToggleFlyout(wifiFlyout, netRc); break;
        case HBat: ToggleFlyout(batteryFlyout, batRc); break;
        case HClock: Launch(L"timedate.cpl"); break;
        case HDesk: ShowDesktop(); break;
        default: break;
        }
        pressedTray = -1;
    }

    void OnDoubleClick(int x, int y) override {
        HoverKind k;
        int idx;
        HitTest(x, y, k, idx);
        if (k == HTray) SendTrayMouse(tray[idx], WM_LBUTTONDBLCLK);
    }

    void BarMenu(POINT pt) {
        HMENU m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, 1, L"Administrador de tareas");
        AppendMenuW(m, MF_STRING, 2, L"Archivos");
        AppendMenuW(m, MF_STRING, 3, L"Mostrar escritorio");
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING, 4, L"Sonido");
        AppendMenuW(m, MF_STRING, 5, L"Red");
        AppendMenuW(m, MF_STRING, 6, L"Energía");
        AppendMenuW(m, MF_STRING, 7, L"Fecha y hora");
        AppendMenuW(m, MF_STRING, 8, L"Bluetooth");
        AppendMenuW(m, MF_STRING, 9, L"Pantalla");
        SetForegroundWindow(hwnd);
        int cmd = ui::ThemedMenu(m, TPM_RETURNCMD | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        switch (cmd) {
        case 1: LaunchOwn(L"--taskmgr"); break;
        case 2: OpenInExplorer(L""); break;
        case 3: ShowDesktop(); break;
        case 4: Launch(L"mmsys.cpl"); break;
        case 5: Launch(L"ncpa.cpl"); break;
        case 6: Launch(L"powercfg.cpl"); break;
        case 7: Launch(L"timedate.cpl"); break;
        case 8: LaunchOwn(L"--settings bluetooth"); break;
        case 9: LaunchOwn(L"--settings pantalla"); break;
        }
    }

    void OnTimer(UINT_PTR id) override {
        if (id == 1) UpdateClock();
    }

    void OnSettingsChange() override { Invalidate(); }

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        if (msg == shellHookMsg && shellHookMsg) {
            if ((wp & 0x7FFF) == HSHELL_GETMINRECT) {
                // Animación de minimizar hacia el botón de la tarea.
                auto* info = (SHELLHOOKINFO*)lp;
                if (TaskItem* t = FindTask(info->hwnd)) {
                    POINT a = { (LONG)t->rc.X, (LONG)t->rc.Y };
                    POINT b = { (LONG)(t->rc.X + t->rc.Width), (LONG)(t->rc.Y + t->rc.Height) };
                    ClientToScreen(hwnd, &a);
                    ClientToScreen(hwnd, &b);
                    info->rc = RECT{ a.x, a.y, b.x, b.y };
                    return TRUE;
                }
                return FALSE;
            }
            OnShellHook(wp, lp);
            return 0;
        }
        switch (msg) {
        case WM_COPYDATA: {
            auto* cds = (COPYDATASTRUCT*)lp;
            if (g_shellMode && cds && cds->dwData == 1 && cds->lpData && cds->cbData >= sizeof(DWORD) * 2)
                return OnTrayMessage((const TrayNotifyData*)cds->lpData, cds->cbData);
            return FALSE;
        }
        case WM_RBUTTONDOWN: {
            HoverKind k;
            int idx;
            HitTest(GET_X_LPARAM(lp), GET_Y_LPARAM(lp), k, idx);
            if (k == HTray) SendTrayMouse(tray[idx], WM_RBUTTONDOWN);
            return 0;
        }
        case WM_RBUTTONUP: {
            int x = GET_X_LPARAM(lp), y = GET_Y_LPARAM(lp);
            HoverKind k;
            int idx;
            HitTest(x, y, k, idx);
            POINT pt = { x, y };
            ClientToScreen(hwnd, &pt);
            if (k == HTray) SendTrayMouse(tray[idx], WM_RBUTTONUP);
            else if (k == HTask) TaskMenu(idx, pt);
            else BarMenu(pt);
            return 0;
        }
        case WM_SHELLACTION:
            DoAction((int)wp);
            return 0;
        case WM_CLOAKCHANGED: {
            HWND w = (HWND)lp;
            bool want = IsTaskWindow(w);
            bool has = FindTask(w) != nullptr;
            if (want && !has) { AddTask(w); Relayout(); }
            else if (!want && has) { RemoveTask(w); Relayout(); }
            return 0;
        }
        case WM_POWERBROADCAST:
            if (wp == PBT_APMPOWERSTATUSCHANGE || wp == PBT_POWERSETTINGCHANGE) {
                bool had = HasBattery();
                ReadPower();
                if (had != HasBattery()) Relayout();
                else InvalidateRectF(batRc);
            }
            return TRUE;
        case WM_TIMECHANGE:
            UpdateClock();
            return 0;
        case WM_DISPLAYCHANGE:
            FitToScreen();
            return 0;
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }

    bool CanClose() override { return false; }
};

TaskbarWindow* TaskbarWindow::instance = nullptr;

// ---------------------------------------------------------------- teclado global (modo shell)
// Sin Explorer nadie atiende la tecla Windows ni sus atajos: los recogemos aquí.
HHOOK g_kbHook = nullptr;
bool g_winDown = false;
bool g_winCombo = false;

LRESULT CALLBACK LowLevelKeyboard(int code, WPARAM wp, LPARAM lp) {
    if (code == HC_ACTION && g_taskbarHwnd) {
        auto* k = (KBDLLHOOKSTRUCT*)lp;
        bool down = wp == WM_KEYDOWN || wp == WM_SYSKEYDOWN;
        bool up = wp == WM_KEYUP || wp == WM_SYSKEYUP;
        DWORD vk = k->vkCode;
        if (vk == VK_LWIN || vk == VK_RWIN) {
            if (down && !g_winDown) { g_winDown = true; g_winCombo = false; }
            if (up) {
                if (g_winDown && !g_winCombo) PostMessageW(g_taskbarHwnd, WM_SHELLACTION, ActStart, 0);
                g_winDown = false;
            }
        } else if (down) {
            if (g_winDown) {
                g_winCombo = true;
                int act = vk == 'E' ? ActExplorer : vk == 'R' ? ActRun : vk == 'D' ? ActShowDesktop : 0;
                if (act) {
                    PostMessageW(g_taskbarHwnd, WM_SHELLACTION, act, 0);
                    return 1;
                }
            }
            if (vk == VK_ESCAPE && (GetAsyncKeyState(VK_CONTROL) & 0x8000) && (GetAsyncKeyState(VK_SHIFT) & 0x8000)) {
                PostMessageW(g_taskbarHwnd, WM_SHELLACTION, ActTaskMgr, 0);
                return 1;
            }
        }
    }
    return CallNextHookEx(g_kbHook, code, wp, lp);
}

// ---------------------------------------------------------------- app
// Programas de inicio: una sola vez por sesión (una clave volátil del registro
// desaparece al cerrar sesión, así que reiniciar el shell no los repite).
bool FirstShellStartThisSession() {
    DWORD sid = 0;
    ProcessIdToSessionId(GetCurrentProcessId(), &sid);
    std::wstring key = L"Software\\OptimizedShell\\Session" + std::to_wstring(sid);
    HKEY k = nullptr;
    DWORD disp = 0;
    if (RegCreateKeyExW(HKEY_CURRENT_USER, key.c_str(), 0, nullptr, REG_OPTION_VOLATILE, KEY_READ, nullptr, &k,
                        &disp) != ERROR_SUCCESS)
        return false;
    RegCloseKey(k);
    return disp == REG_CREATED_NEW_KEY;
}

class ShellApp {
public:
    bool Start() {
        if (!desktop.Start()) return false;
        if (!taskbar.Start()) return false;

        if (g_shellMode) {
            typedef BOOL(WINAPI* PFN_SetShellWindow)(HWND);
            typedef BOOL(WINAPI* PFN_SetTaskmanWindow)(HWND);
            HMODULE u32 = GetModuleHandleW(L"user32.dll");
            if (auto setShell = (PFN_SetShellWindow)GetProcAddress(u32, "SetShellWindow")) setShell(desktop.Hwnd());
            if (auto setTaskman = (PFN_SetTaskmanWindow)GetProcAddress(u32, "SetTaskmanWindow")) setTaskman(taskbar.Hwnd());

            // Ventanas minimizadas ocultas (no como iconos sobre el escritorio).
            MINIMIZEDMETRICS mm = { sizeof(mm) };
            SystemParametersInfoW(SPI_GETMINIMIZEDMETRICS, sizeof(mm), &mm, 0);
            mm.iArrange |= ARW_HIDE;
            SystemParametersInfoW(SPI_SETMINIMIZEDMETRICS, sizeof(mm), &mm, 0);

            taskbar.FitToScreen(); // reserva el área de trabajo
            SendNotifyMessageW(HWND_BROADCAST, RegisterWindowMessageW(L"TaskbarCreated"), 0, 0);
            g_kbHook = SetWindowsHookExW(WH_KEYBOARD_LL, LowLevelKeyboard, GetModuleHandleW(nullptr), 0);

            if (FirstShellStartThisSession())
                BackgroundWorker().Post([] { Sleep(1500); startup::RunAll(); });
        }
        // Tras arrancar, devolver la memoria de inicialización que ya no se usa.
        SetTimer(nullptr, 0, 8000, [](HWND, UINT, UINT_PTR id, DWORD) { KillTimer(nullptr, id); TrimWorkingSet(); });
        return true;
    }

    // Bucle de mensajes que además espera las notificaciones de cambios del escritorio.
    void Run() {
        const auto& handles = desktop.WatchHandles();
        for (;;) {
            DWORD n = (DWORD)handles.size();
            DWORD r = MsgWaitForMultipleObjectsEx(n, n ? handles.data() : nullptr, INFINITE, QS_ALLINPUT,
                                                  MWMO_INPUTAVAILABLE);
            if (r < WAIT_OBJECT_0 + n) {
                desktop.OnWatchSignaled(r - WAIT_OBJECT_0);
                continue;
            }
            MSG msg;
            while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
                if (msg.message == WM_QUIT) return;
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    void Shutdown() {
        if (g_kbHook) { UnhookWindowsHookEx(g_kbHook); g_kbHook = nullptr; }
        if (g_shellMode) {
            RECT full = { 0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN) };
            SystemParametersInfoW(SPI_SETWORKAREA, 0, &full, SPIF_SENDCHANGE);
        }
    }

private:
    DesktopWindow desktop;
    TaskbarWindow taskbar;
};

// ---------------------------------------------------------------- instalación
const wchar_t* kWinlogonKey = L"Software\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon";

int InstallShell(bool install) {
    if (install) {
        std::wstring msg =
            L"OptimizedShell sustituirá a Explorer como shell de tu usuario a partir del próximo inicio de sesión.\n\n"
            L"Para volver atrás:\n"
            L"  • Menú Inicio > botón de apagado > Restaurar Explorer y luego ejecutar \"OptimizedShellCpp.exe --uninstall\", o\n"
            L"  • Ctrl+Alt+Supr > Administrador de tareas > Ejecutar nueva tarea > explorer.exe\n\n"
            L"¿Continuar?";
        if (ui::MsgBox(nullptr, msg.c_str(), L"Instalar OptimizedShell", MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2) != IDYES)
            return 1;
        std::wstring value = Quote(GetOwnPath());
        LSTATUS st = RegSetKeyValueW(HKEY_CURRENT_USER, kWinlogonKey, L"Shell", REG_SZ, value.c_str(),
                                     (DWORD)((value.size() + 1) * sizeof(wchar_t)));
        ui::MsgBox(nullptr, st == ERROR_SUCCESS ? L"Instalado. Cierra sesión y vuelve a entrar para usarlo."
                                                 : L"No se pudo escribir en el registro.",
                    L"OptimizedShell", st == ERROR_SUCCESS ? MB_ICONINFORMATION : MB_ICONERROR);
        return st == ERROR_SUCCESS ? 0 : 1;
    }
    if (ui::MsgBox(nullptr, L"¿Volver a usar Explorer como shell a partir del próximo inicio de sesión?",
                    L"Desinstalar OptimizedShell", MB_YESNO | MB_ICONQUESTION) != IDYES)
        return 1;
    LSTATUS st = RegDeleteKeyValueW(HKEY_CURRENT_USER, kWinlogonKey, L"Shell");
    bool ok = st == ERROR_SUCCESS || st == ERROR_FILE_NOT_FOUND;
    ui::MsgBox(nullptr, ok ? L"Listo. Explorer volverá a ser el shell en el próximo inicio de sesión."
                            : L"No se pudo modificar el registro.",
                L"OptimizedShell", ok ? MB_ICONINFORMATION : MB_ICONERROR);
    return ok ? 0 : 1;
}

} // namespace

int WINAPI wWinMain(HINSTANCE hInst, HINSTANCE, PWSTR, int) {
    if (HasArg(L"--logon") || HasArg(L"--lock")) return RunLogonUIMode(HasArg(L"--demo"), HasArg(L"--lock"));
    if (HasArg(L"--taskmgr")) return RunTaskManagerMode();
    if (HasArg(L"--explorer")) return RunExplorerMode();
    if (HasArg(L"--settings")) return RunSettingsMode();
    if (HasArg(L"--install") || HasArg(L"--uninstall")) {
        InitUi(hInst); // los diálogos usan el tema propio
        int r = InstallShell(HasArg(L"--install"));
        ShutdownUi();
        return r;
    }

    // Un solo shell por sesión.
    HANDLE single = CreateMutexW(nullptr, TRUE, L"Local\\OptimizedShell.Shell");
    if (GetLastError() == ERROR_ALREADY_EXISTS) {
        HWND tb = FindWindowW(L"OShellCppTaskbar", nullptr);
        if (!tb) tb = FindWindowW(L"Shell_TrayWnd", nullptr);
        if (tb) PostMessageW(tb, WM_SHELLACTION, ActStart, 0);
        return 0;
    }

    // Modo shell: no hay otro shell (Explorer) en la sesión, o se fuerza con --shell.
    g_shellMode = HasArg(L"--shell") ||
                  (!GetShellWindow() && !FindWindowW(L"Shell_TrayWnd", nullptr) && !HasArg(L"--noshell"));

    HRESULT comHr = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    InitUi(hInst);

    int rc = 0;
    {
        ShellApp app;
        if (!app.Start()) {
            ui::MsgBox(nullptr, L"No se pudo iniciar OptimizedShell.", L"OptimizedShell", MB_ICONERROR | MB_OK);
            rc = 1;
        } else {
            app.Run();
            app.Shutdown();
        }
    }
    if (g_restoreExplorer) {
        // Nuestras ventanas ya no existen: Explorer arrancará como shell.
        STARTUPINFOW si = { sizeof(si) };
        PROCESS_INFORMATION pi = {};
        wchar_t cmd[MAX_PATH] = L"explorer.exe";
        if (CreateProcessW(nullptr, cmd, nullptr, nullptr, FALSE, 0, nullptr, nullptr, &si, &pi)) {
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
        }
    }
    ShutdownUi();
    if (SUCCEEDED(comHr)) CoUninitialize();
    if (single) CloseHandle(single);
    return rc;
}
