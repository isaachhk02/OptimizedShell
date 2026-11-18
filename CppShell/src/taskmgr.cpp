// taskmgr.cpp - Administrador de tareas de OptimizedShell.
// Pestañas: Procesos · Rendimiento · Inicio · Servicios.
// Se ejecuta con: OptimizedShellCpp.exe --taskmgr [--tab procesos|rendimiento|inicio|servicios]
//
// Para gastar poco:
//  - Una sola llamada a NtQuerySystemInformation trae CPU, memoria, E/S, hilos y
//    proceso padre de todos los procesos (sin abrir cada proceso en cada refresco).
//  - La ruta e icono de un proceso se piden una vez y solo si su fila se ve.
//  - Solo se muestrea lo que necesita la pestaña visible (disco/red solo en Rendimiento).
//  - Minimizado no hay timers: el proceso queda en EcoQoS y devuelve su memoria.
#include "ui.h"
#include "startup.h"

#include <winsock2.h>
#include <ws2ipdef.h>
#include <iphlpapi.h>
#include <winternl.h>
#include <pdh.h>
#include <powrprof.h>
#include <thread>
#include <functional>
#include <map>
#include <set>
#include <unordered_map>
#include <cstdio>

#pragma comment(lib, "pdh.lib")
#pragma comment(lib, "iphlpapi.lib")
#pragma comment(lib, "powrprof.lib")

namespace {

using namespace Gdiplus;
using namespace ui;

const Color kGreen(255, 74, 222, 128);
const Color kAmber(255, 251, 191, 36);
const Color kRed(255, 248, 113, 113);
const Color kPurple(255, 167, 139, 250);
const Color kTeal(255, 45, 212, 191);

const UINT WM_TASKRESULT = WM_APP + 70; // lParam = std::wstring* (mensaje) o nullptr; wParam = acción

// ---------------------------------------------------------------- ntdll
struct SysProcInfo {
    ULONG NextEntryOffset;
    ULONG NumberOfThreads;
    LARGE_INTEGER WorkingSetPrivateSize;
    ULONG HardFaultCount;
    ULONG NumberOfThreadsHighWatermark;
    ULONGLONG CycleTime;
    LARGE_INTEGER CreateTime;
    LARGE_INTEGER UserTime;
    LARGE_INTEGER KernelTime;
    UNICODE_STRING ImageName;
    LONG BasePriority;
    HANDLE UniqueProcessId;
    HANDLE InheritedFromUniqueProcessId;
    ULONG HandleCount;
    ULONG SessionId;
    ULONG_PTR UniqueProcessKey;
    SIZE_T PeakVirtualSize;
    SIZE_T VirtualSize;
    ULONG PageFaultCount;
    SIZE_T PeakWorkingSetSize;
    SIZE_T WorkingSetSize;
    SIZE_T QuotaPeakPagedPoolUsage;
    SIZE_T QuotaPagedPoolUsage;
    SIZE_T QuotaPeakNonPagedPoolUsage;
    SIZE_T QuotaNonPagedPoolUsage;
    SIZE_T PagefileUsage;
    SIZE_T PeakPagefileUsage;
    SIZE_T PrivatePageCount;
    LARGE_INTEGER ReadOperationCount;
    LARGE_INTEGER WriteOperationCount;
    LARGE_INTEGER OtherOperationCount;
    LARGE_INTEGER ReadTransferCount;
    LARGE_INTEGER WriteTransferCount;
    LARGE_INTEGER OtherTransferCount;
};

struct CpuPerf {
    LARGE_INTEGER IdleTime;
    LARGE_INTEGER KernelTime;
    LARGE_INTEGER UserTime;
    LARGE_INTEGER DpcTime;
    LARGE_INTEGER InterruptTime;
    ULONG InterruptCount;
};

struct ProcessorPowerInfo {
    ULONG Number;
    ULONG MaxMhz;
    ULONG CurrentMhz;
    ULONG MhzLimit;
    ULONG MaxIdleState;
    ULONG CurrentIdleState;
};

typedef NTSTATUS(NTAPI* PFN_NtQSI)(ULONG, PVOID, ULONG, PULONG);

PFN_NtQSI NtQSI() {
    static PFN_NtQSI fn = (PFN_NtQSI)GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "NtQuerySystemInformation");
    return fn;
}

// ---------------------------------------------------------------- datos
struct Proc {
    DWORD pid = 0, ppid = 0;
    std::wstring name;
    ULONGLONG created = 0;
    double cpu = 0;
    ULONGLONG ws = 0;     // conjunto de trabajo privado (lo que muestra Windows como "Memoria")
    ULONGLONG commit = 0; // bytes privados confirmados
    double io = 0;        // bytes/s de E/S
    DWORD threads = 0, handles = 0;
    int depth = 0;
};

struct ProcCache {
    ULONGLONG created = 0;
    ULONGLONG cpuTime = 0;
    ULONGLONG ioBytes = 0;
    bool queried = false;
    std::wstring path;
    int icon = -1;
};

struct ServiceRow {
    std::wstring name, display;
    DWORD state = 0, pid = 0, startType = 0xFFFFFFFF;
};

const wchar_t* ServiceState(DWORD s) {
    switch (s) {
    case SERVICE_RUNNING: return L"En ejecución";
    case SERVICE_STOPPED: return L"Detenido";
    case SERVICE_START_PENDING: return L"Iniciando";
    case SERVICE_STOP_PENDING: return L"Deteniendo";
    case SERVICE_PAUSED: return L"En pausa";
    default: return L"—";
    }
}

const wchar_t* StartTypeName(DWORD t) {
    switch (t) {
    case SERVICE_AUTO_START: return L"Automático";
    case SERVICE_DEMAND_START: return L"Manual";
    case SERVICE_DISABLED: return L"Deshabilitado";
    case SERVICE_BOOT_START: case SERVICE_SYSTEM_START: return L"Sistema";
    default: return L"";
    }
}

std::wstring ErrText(DWORD err) {
    wchar_t* buf = nullptr;
    FormatMessageW(FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
                   nullptr, err, 0, (LPWSTR)&buf, 0, nullptr);
    std::wstring s = buf ? Trim(buf) : L"Error " + std::to_wstring(err);
    if (buf) LocalFree(buf);
    return s;
}

std::wstring FormatRate(double bytesPerSec) {
    if (bytesPerSec < 1) return L"0 KB/s";
    return FormatBytes((ULONGLONG)bytesPerSec) + L"/s";
}

// Historial circular de 60 muestras.
struct History {
    float v[60] = {};
    int count = 0, head = 0;
    void Push(float x) {
        v[head] = x;
        head = (head + 1) % 60;
        if (count < 60) ++count;
    }
    float At(int i) const { return v[(head - count + i + 60) % 60]; } // 0 = más antiguo
    float Last() const { return count ? At(count - 1) : 0; }
    float Max() const {
        float m = 0;
        for (int i = 0; i < count; ++i) m = std::max(m, At(i));
        return m;
    }
};

// ---------------------------------------------------------------- tabla genérica
struct Column {
    const wchar_t* title;
    float width; // 0 = ocupa el resto
    bool right;
};

struct Table {
    std::vector<Column> cols;
    int sortCol = 0;
    bool sortDesc = false;
    int scroll = 0;
    int hover = -1;
    std::wstring selKey;
    RectF header, body;
    std::vector<float> x; // borde izquierdo de cada columna + fin

    int RowH() const { return S(28); }
    int Visible() const { return std::max(1, (int)(body.Height / RowH())); }

    void Layout(const RectF& area) {
        header = RectF(area.X, area.Y, area.Width, (float)S(30));
        body = RectF(area.X, area.Y + S(30), area.Width - S(10), area.Height - S(30));
        float fixed = 0;
        for (auto& c : cols) fixed += S((int)c.width);
        float flex = std::max((float)S(120), body.Width - fixed - S(8));
        x.assign(cols.size() + 1, 0);
        float cx = body.X + S(8);
        for (size_t i = 0; i < cols.size(); ++i) {
            x[i] = cx;
            cx += cols[i].width == 0 ? flex : S((int)cols[i].width);
        }
        x[cols.size()] = cx;
    }

    void Clamp(int count) {
        scroll = std::max(0, std::min(scroll, std::max(0, count - Visible())));
    }

    int RowAt(int px, int py, int count) const {
        if (!InRect(body, px, py)) return -1;
        int r = scroll + (int)((py - body.Y) / RowH());
        return r < count ? r : -1;
    }

    int ColAt(int px, int py) const {
        if (!InRect(header, px, py)) return -1;
        for (size_t i = 0; i < cols.size(); ++i)
            if (px >= x[i] && px < x[i + 1]) return (int)i;
        return -1;
    }

    void ClickHeader(int c) {
        if (c < 0) return;
        if (sortCol == c) sortDesc = !sortDesc;
        else { sortCol = c; sortDesc = cols[c].right; } // números: de mayor a menor
    }

    void EnsureVisible(int row, int count) {
        if (row < 0) return;
        if (row < scroll) scroll = row;
        if (row >= scroll + Visible()) scroll = row - Visible() + 1;
        Clamp(count);
    }

    struct Source {
        std::function<std::wstring(int, int)> cell;
        std::function<std::wstring(int)> key;
        std::function<Bitmap*(int)> icon;
        std::function<Color(int, int)> color;
        std::function<int(int)> indent;
    };

    void Paint(Graphics& g, int count, const Source& src, const wchar_t* emptyMsg) {
        for (size_t i = 0; i < cols.size(); ++i) {
            RectF r(x[i] + S(2), header.Y, x[i + 1] - x[i] - S(10), header.Height);
            DrawTextStr(g, cols[i].title, fontSmallBold, (int)i == sortCol ? style::text : style::textDim, r,
                        cols[i].right ? StringAlignmentFar : StringAlignmentNear);
            if ((int)i == sortCol) {
                float ax = cols[i].right ? x[i] + S(4) : x[i + 1] - S(20);
                DrawTextStr(g, sortDesc ? L"" : L"", fontIcon, style::accent,
                            RectF(ax, header.Y, (float)S(14), header.Height), StringAlignmentCenter);
            }
        }
        HLine(g, header.X, header.X + header.Width, header.Y + header.Height - 1, C(30, 255, 255, 255));
        Clamp(count);
        int vis = Visible();
        for (int row = 0; row < vis; ++row) {
            int i = scroll + row;
            if (i >= count) break;
            RectF rr(body.X + S(2), body.Y + row * RowH(), body.Width - S(2), (float)RowH() - S(2));
            bool sel = !selKey.empty() && src.key(i) == selKey;
            if (sel) DrawRounded(g, rr, SF(5), style::rowSelected);
            else if (i == hover) DrawRounded(g, rr, SF(5), style::rowHover);
            for (size_t c = 0; c < cols.size(); ++c) {
                float cx = x[c] + S(2);
                float cw = x[c + 1] - x[c] - S(10);
                if (c == 0) {
                    float ind = src.indent ? (float)src.indent(i) * S(16) : 0;
                    cx += ind;
                    cw -= ind;
                    if (src.icon) {
                        if (Bitmap* b = src.icon(i))
                            g.DrawImage(b, cx, rr.Y + (rr.Height - S(16)) / 2, (float)S(16), (float)S(16));
                        cx += S(24);
                        cw -= S(24);
                    }
                }
                Color col = src.color ? src.color(i, (int)c) : (c == 0 ? style::text : style::textDim);
                DrawTextStr(g, src.cell(i, (int)c), c == 0 ? fontBody : fontSmall, col, RectF(cx, rr.Y, cw, rr.Height),
                            cols[c].right ? StringAlignmentFar : StringAlignmentNear);
            }
        }
        if (count == 0 && emptyMsg)
            DrawTextStr(g, emptyMsg, fontBody, style::textFaint, RectF(body.X, body.Y + S(20), body.Width, (float)S(28)),
                        StringAlignmentCenter);
        DrawScrollbar(g, RectF(body.X + body.Width + S(3), body.Y + S(2), (float)S(4), body.Height - S(4)), count, vis, scroll);
    }
};

// ---------------------------------------------------------------- diálogos
// Ventana emergente oscura con título, botones Aceptar/Cancelar y contenido propio.
class DialogBase : public WindowBase {
public:
    std::function<void()> onClosed;

protected:
    HWND owner = nullptr;
    std::wstring title;
    RectF okRc, cancelRc;
    int hoverBtn = 0;

    bool OpenDialog(HWND own, const std::wstring& t, int w, int h) {
        owner = own;
        title = t;
        RECT orc = {};
        GetWindowRect(own, &orc);
        int x = orc.left + ((orc.right - orc.left) - w) / 2;
        int y = orc.top + ((orc.bottom - orc.top) - h) / 2;
        if (!Create(L"OShellCppTaskDialog", WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP | WS_CLIPCHILDREN, x, y, w, h, t.c_str()))
            return false;
        EnableDarkFrame(hwnd);
        okRc = RectF((float)w - S(20) - S(104), (float)h - S(20) - S(34), (float)S(104), (float)S(34));
        cancelRc = RectF(okRc.X - S(10) - S(104), okRc.Y, (float)S(104), (float)S(34));
        EnableWindow(owner, FALSE);
        return true;
    }

    void Show() {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }

    void PaintFrame(Graphics& g, int w, int h, const std::wstring& subtitle) {
        g.Clear(RGBc(20, 23, 31));
        DrawRounded(g, RectF(0, 0, (float)w - 1, (float)h - 1), 0, Color(0, 0, 0, 0), C(60, 255, 255, 255));
        DrawTextStr(g, title, fontTitle, style::text, RectF((float)S(20), (float)S(16), (float)w - S(40), (float)S(30)));
        if (!subtitle.empty())
            DrawTextStr(g, subtitle, fontSmall, style::textDim, RectF((float)S(20), (float)S(46), (float)w - S(40), (float)S(20)));
        style::DrawButton(g, cancelRc, L"Cancelar", hoverBtn == 2);
        style::DrawButton(g, okRc, L"Aceptar", hoverBtn == 1, true);
    }

    virtual void Accept() = 0;

    void Finish() {
        if (owner) { EnableWindow(owner, TRUE); SetForegroundWindow(owner); }
        Close();
    }

    void OnMouseMove(int x, int y) override {
        int hv = InRect(okRc, x, y) ? 1 : (InRect(cancelRc, x, y) ? 2 : 0);
        if (hv != hoverBtn) { hoverBtn = hv; InvalidateRectF(okRc); InvalidateRectF(cancelRc); }
    }

    bool HandleButtons(int x, int y) {
        if (InRect(okRc, x, y)) { Accept(); return true; }
        if (InRect(cancelRc, x, y)) { Finish(); return true; }
        return false;
    }

    void OnKeyDown(int vk) override {
        if (vk == VK_ESCAPE) Finish();
        else if (vk == VK_RETURN) Accept();
    }

    LRESULT OnHitTest(int x, int y) override {
        return (y < S(46) && !InRect(okRc, x, y)) ? HTCAPTION : 0;
    }

    void OnDestroy() override {
        if (owner) EnableWindow(owner, TRUE);
        if (onClosed) {
            auto cb = onClosed;
            onClosed = nullptr;
            cb();
        }
    }
};

class NewTaskDialog : public DialogBase, public EditKeySink {
public:
    bool Open(HWND own) {
        if (!OpenDialog(own, L"Crear nueva tarea", S(500), S(220))) return false;
        field = RectF((float)S(20), (float)S(76), (float)S(460), (float)S(36));
        adminRc = RectF((float)S(20), (float)S(122), (float)S(300), (float)S(24));
        edit = CreateDarkEdit(hwnd, 100, L"Programa, carpeta, documento o sitio web");
        MoveWindow(edit, (int)field.X + S(10), (int)field.Y + S(8), (int)field.Width - S(20), (int)field.Height - S(14), FALSE);
        HookEditKeys(edit, this);
        Show();
        SetFocus(edit);
        return true;
    }

    bool OnEditKey(HWND, int vk) override {
        if (vk == VK_RETURN) { Accept(); return true; }
        if (vk == VK_ESCAPE) { Finish(); return true; }
        return false;
    }

protected:
    HWND edit = nullptr;
    RectF field, adminRc;
    bool admin = false;

    void OnPaint(Graphics& g, int w, int h) override {
        PaintFrame(g, w, h, L"Escribe el nombre de un programa, carpeta, documento o recurso de Internet.");
        DrawRounded(g, field, SF(6), C(255, 35, 40, 52), style::accent, 1.2f);
        RectF box(adminRc.X, adminRc.Y + S(3), (float)S(18), (float)S(18));
        DrawRounded(g, box, SF(4), admin ? style::accentSolid : C(255, 35, 40, 52), C(90, 255, 255, 255));
        if (admin) DrawCheck(g, box.X + box.Width / 2, box.Y + box.Height / 2, box.Width * 0.6f, RGBc(255, 255, 255), SF(1.8f));
        DrawTextStr(g, L"Crear esta tarea con privilegios de administrador", fontSmall, style::text,
                    RectF(adminRc.X + S(28), adminRc.Y, (float)S(320), adminRc.Height));
    }

    void OnMouseDown(int x, int y) override {
        if (HandleButtons(x, y)) return;
        if (InRect(RectF(adminRc.X, adminRc.Y, (float)S(360), adminRc.Height), x, y)) { admin = !admin; InvalidateRectF(adminRc); }
        SetFocus(edit);
    }

    void Accept() override {
        std::wstring cmd = Trim(GetWindowString(edit));
        if (cmd.empty()) return;
        bool ok;
        if (admin) {
            std::vector<wchar_t> file(cmd.begin(), cmd.end());
            file.resize(cmd.size() + MAX_PATH, L'\0');
            PathRemoveArgsW(file.data());
            PathUnquoteSpacesW(file.data());
            const wchar_t* args = PathGetArgsW(cmd.c_str());
            SHELLEXECUTEINFOW sei = { sizeof(sei) };
            sei.hwnd = hwnd;
            sei.lpVerb = L"runas";
            sei.lpFile = file.data();
            sei.lpParameters = (args && *args) ? args : nullptr;
            sei.nShow = SW_SHOWNORMAL;
            ok = ShellExecuteExW(&sei) != FALSE || GetLastError() == ERROR_CANCELLED;
        } else {
            ok = RunCommandLine(cmd, hwnd);
        }
        if (ok) Finish();
        else MessageBoxW(hwnd, (L"No se encuentra \"" + cmd + L"\".").c_str(), L"Crear nueva tarea", MB_ICONWARNING);
    }
};

class AffinityDialog : public DialogBase {
public:
    bool Open(HWND own, DWORD pid, const std::wstring& name) {
        this->pid = pid;
        HANDLE hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        DWORD_PTR sys = 0;
        if (!hp || !GetProcessAffinityMask(hp, &mask, &sys)) {
            if (hp) CloseHandle(hp);
            MessageBoxW(own, L"No se puede consultar la afinidad de este proceso (puede requerir administrador).",
                        L"Establecer afinidad", MB_ICONWARNING);
            return false;
        }
        CloseHandle(hp);
        systemMask = sys;
        cpus = 0;
        for (int i = 0; i < 64; ++i)
            if (sys & ((DWORD_PTR)1 << i)) cpus = i + 1;
        int rows = (cpus + 7) / 8;
        int h = S(150) + rows * S(34) + S(40);
        if (!OpenDialog(own, L"Afinidad de " + name, S(520), h)) return false;
        Show();
        return true;
    }

protected:
    DWORD pid = 0;
    DWORD_PTR mask = 0, systemMask = 0;
    int cpus = 0;

    RectF Cell(int i) const {
        return RectF((float)S(20) + (i % 8) * S(60), (float)S(110) + (i / 8) * S(34), (float)S(56), (float)S(28));
    }
    RectF AllRc() const { return RectF((float)S(20), (float)S(76), (float)S(220), (float)S(26)); }

    void OnPaint(Graphics& g, int w, int h) override {
        PaintFrame(g, w, h, L"Elige en qué procesadores lógicos puede ejecutarse.");
        bool all = (mask & systemMask) == systemMask;
        auto box = [&](const RectF& r, bool on, const std::wstring& label) {
            RectF b(r.X, r.Y + (r.Height - S(18)) / 2, (float)S(18), (float)S(18));
            DrawRounded(g, b, SF(4), on ? style::accentSolid : C(255, 35, 40, 52), C(90, 255, 255, 255));
            if (on) DrawCheck(g, b.X + b.Width / 2, b.Y + b.Height / 2, b.Width * 0.6f, RGBc(255, 255, 255), SF(1.8f));
            DrawTextStr(g, label, fontSmall, style::text, RectF(b.X + S(24), r.Y, r.Width - S(24), r.Height));
        };
        box(AllRc(), all, L"Todos los procesadores");
        for (int i = 0; i < cpus; ++i) box(Cell(i), (mask >> i) & 1, L"CPU " + std::to_wstring(i));
    }

    void OnMouseDown(int x, int y) override {
        if (HandleButtons(x, y)) return;
        if (InRect(AllRc(), x, y)) {
            mask = ((mask & systemMask) == systemMask) ? 0 : systemMask;
            Invalidate();
            return;
        }
        for (int i = 0; i < cpus; ++i)
            if (InRect(Cell(i), x, y)) { mask ^= ((DWORD_PTR)1 << i); Invalidate(); return; }
    }

    void Accept() override {
        if (!(mask & systemMask)) {
            MessageBoxW(hwnd, L"Selecciona al menos un procesador.", L"Afinidad", MB_ICONINFORMATION);
            return;
        }
        HANDLE hp = OpenProcess(PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        bool ok = hp && SetProcessAffinityMask(hp, mask & systemMask);
        DWORD err = GetLastError();
        if (hp) CloseHandle(hp);
        if (!ok) {
            MessageBoxW(hwnd, (L"No se pudo cambiar la afinidad:\n" + ErrText(err)).c_str(), L"Afinidad", MB_ICONWARNING);
            return;
        }
        Finish();
    }
};

// ---------------------------------------------------------------- ventana principal
enum Tab { TabProc = 0, TabPerf, TabStartup, TabServices, TabCount };
enum PerfKind { PerfCpu = 0, PerfMem, PerfDisk, PerfNet, PerfCount };

enum Btn {
    BNone = 0, BMin, BMax, BClose,
    BTab0 = 10,
    BEnd = 20, BNewTask, BTree, BFreeRam, BToggleStartup, BStartupLocation, BSvcStart, BSvcStop, BSvcRestart,
    BSvcOpen, BElevate, BRefresh,
};

class TaskManagerWindow : public WindowBase, public EditKeySink {
public:
    bool Start(int initialTab) {
        customFrame = true;
        tab = initialTab;
        RECT wa = {};
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &wa, 0);
        int w = std::min(S(1060), (int)(wa.right - wa.left) - S(40));
        int h = std::min(S(700), (int)(wa.bottom - wa.top) - S(40));
        int x = wa.left + (wa.right - wa.left - w) / 2;
        int y = wa.top + (wa.bottom - wa.top - h) / 2;
        if (!Create(L"OShellCppTaskMgr", WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, x, y, w, h,
                    L"Administrador de tareas"))
            return false;
        EnableDarkFrame(hwnd);
        SetWindowPos(hwnd, nullptr, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        SHSTOCKICONINFO sii = { sizeof(sii) };
        if (SUCCEEDED(SHGetStockIconInfo(SIID_DESKTOPPC, SHGSI_ICON | SHGSI_SMALLICON, &sii))) {
            appIcon = sii.hIcon;
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL, (LPARAM)appIcon);
            SendMessageW(hwnd, WM_SETICON, ICON_BIG, (LPARAM)appIcon);
        }
        elevated = IsElevated();
        cpus = std::max(1, (int)GetActiveProcessorCount(ALL_PROCESSOR_GROUPS));

        procTable.cols = { { L"Nombre", 0, false }, { L"PID", 76, true }, { L"CPU", 76, true },
                           { L"Memoria", 104, true }, { L"E/S", 96, true }, { L"Hilos", 64, true } };
        procTable.sortCol = 2;
        procTable.sortDesc = true;
        startupTable.cols = { { L"Nombre", 220, false }, { L"Estado", 110, false }, { L"Origen", 190, false },
                              { L"Comando", 0, false } };
        svcTable.cols = { { L"Nombre", 190, false }, { L"Descripción", 0, false }, { L"PID", 70, true },
                          { L"Estado", 120, false }, { L"Inicio", 110, false } };
        svcTable.sortCol = 3;
        svcTable.sortDesc = false;

        search = CreateDarkEdit(hwnd, 100, L"Buscar");
        HookEditKeys(search, this);
        tip.Create(hwnd);
        RECT rc;
        GetClientRect(hwnd, &rc);
        Layout(rc.right, rc.bottom);

        RefreshProcesses();
        SwitchTab(tab, true);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        StartTimers();
        return true;
    }

    bool OnEditKey(HWND, int vk) override {
        if (vk == VK_ESCAPE) {
            if (GetWindowTextLengthW(search)) SetWindowTextW(search, L"");
            else SetFocus(hwnd);
            return true;
        }
        if (vk == VK_RETURN || vk == VK_DOWN) { SetFocus(hwnd); return true; }
        return false;
    }

    ~TaskManagerWindow() override {
        ClosePdh();
        if (appIcon) DestroyIcon(appIcon);
    }

protected:
    int tab = TabProc;
    bool elevated = false;
    bool minimized = false;
    int cpus = 1;
    HICON appIcon = nullptr;
    HWND search = nullptr;
    std::wstring filter;
    HoverTip tip;
    int hoverBtn = BNone;
    std::unique_ptr<DialogBase> dialog;

    // Procesos
    std::vector<BYTE> spiBuf;
    std::vector<Proc> procs;
    std::vector<int> procView;
    std::unordered_map<DWORD, ProcCache> cache;
    std::unordered_map<std::wstring, int> iconByPath;
    ULONGLONG lastSample = 0;
    bool tree = false;
    Table procTable;
    ULONGLONG procCount = 0, threadCount = 0, handleCount = 0;

    // Sistema
    std::vector<CpuPerf> prevCpu;
    std::vector<float> coreUsage;
    double cpuTotal = 0;
    MEMORYSTATUSEX mem = { sizeof(mem) };
    PERFORMANCE_INFORMATION perf = { sizeof(perf) };
    History hist[PerfCount];
    double diskRead = 0, diskWrite = 0, diskActive = 0;
    double netIn = 0, netOut = 0;
    ULONGLONG netPrevIn = 0, netPrevOut = 0, netPrevTick = 0;
    PDH_HQUERY pdhQuery = nullptr;
    PDH_HCOUNTER pdhIdle = nullptr, pdhRead = nullptr, pdhWrite = nullptr;
    int perfSel = PerfCpu;
    ULONG baseMhz = 0;
    std::wstring freeRamMsg;

    // Inicio
    std::vector<startup::Entry> startupItems;
    std::vector<int> startupView;
    Table startupTable;

    // Servicios
    std::vector<ServiceRow> services;
    std::vector<int> svcView;
    Table svcTable;
    bool startTypesLoaded = false;

    // Layout
    RectF btnRc[64];
    RectF tabRc[TabCount];
    RectF searchRc, contentRc, perfListRc, perfDetailRc, perfItemRc[PerfCount];
    static constexpr int kCaption = 42;

    // ------------------------------------------------------------ layout
    void Layout(int w, int h) {
        auto ch = style::ChromeRects(w);
        btnRc[BMin] = ch.min;
        btnRc[BMax] = ch.max;
        btnRc[BClose] = ch.close;
        const wchar_t* names[TabCount] = { L"Procesos", L"Rendimiento", L"Inicio", L"Servicios" };
        float tx = (float)S(12);
        for (int i = 0; i < TabCount; ++i) {
            RectF m;
            float tw = S(24) + (float)wcslen(names[i]) * S(8);
            tabRc[i] = RectF(tx, (float)S(kCaption) + S(4), tw, (float)S(36));
            tx += tw + S(4);
        }
        float y = (float)S(kCaption) + S(48);
        float bh = (float)S(32);
        searchRc = RectF((float)S(14), y, (float)S(260), bh);
        // Botones de la barra de cada pestaña, alineados a la derecha.
        for (int i = BEnd; i <= BRefresh; ++i) btnRc[i] = RectF();
        float rx = (float)w - S(14);
        auto right = [&](int id, float width) {
            rx -= width;
            btnRc[id] = RectF(rx, y, width, bh);
            rx -= S(8);
        };
        switch (tab) {
        case TabProc:
            right(BEnd, (float)S(132));
            right(BNewTask, (float)S(124));
            right(BTree, (float)S(96));
            break;
        case TabPerf:
            right(BNewTask, (float)S(124));
            right(BFreeRam, (float)S(122));
            break;
        case TabStartup:
            right(BToggleStartup, (float)S(132));
            right(BStartupLocation, (float)S(140));
            right(BRefresh, (float)S(104));
            if (!elevated) right(BElevate, (float)S(170));
            break;
        case TabServices:
            right(BSvcRestart, (float)S(104));
            right(BSvcStop, (float)S(96));
            right(BSvcStart, (float)S(96));
            right(BSvcOpen, (float)S(150));
            if (!elevated) right(BElevate, (float)S(170));
            break;
        }
        contentRc = RectF((float)S(8), y + bh + S(10), (float)w - S(16), h - (y + bh + S(10)) - S(8));
        procTable.Layout(contentRc);
        startupTable.Layout(contentRc);
        svcTable.Layout(contentRc);
        perfListRc = RectF(contentRc.X, contentRc.Y, (float)S(250), contentRc.Height);
        for (int i = 0; i < PerfCount; ++i)
            perfItemRc[i] = RectF(perfListRc.X + S(4), perfListRc.Y + S(4) + i * S(72), perfListRc.Width - S(8), (float)S(66));
        perfDetailRc = RectF(perfListRc.X + perfListRc.Width + S(16), contentRc.Y,
                             contentRc.Width - perfListRc.Width - S(20), contentRc.Height);
        if (search) {
            ShowWindow(search, tab == TabPerf ? SW_HIDE : SW_SHOW);
            MoveWindow(search, (int)searchRc.X + S(32), (int)searchRc.Y + S(7), (int)searchRc.Width - S(40),
                       (int)searchRc.Height - S(13), TRUE);
        }
    }

    void Relayout() {
        RECT rc;
        GetClientRect(hwnd, &rc);
        Layout(rc.right, rc.bottom);
        Invalidate();
    }

    void OnSize(UINT type, int w, int h) override {
        if (type == SIZE_MINIMIZED) {
            // Minimizado: sin trabajo periódico, EcoQoS y memoria devuelta.
            minimized = true;
            KillTimer(hwnd, 1);
            ClosePdh();
            SetProcessEco(true);
            TrimWorkingSet();
            return;
        }
        if (minimized) {
            minimized = false;
            SetProcessEco(false);
            Tick();
            StartTimers();
        }
        Layout(w, h);
    }

    void StartTimers() {
        if (!minimized) SetTimer(hwnd, 1, tab == TabStartup ? 4000 : (tab == TabServices ? 3000 : 1000), nullptr);
    }

    void SwitchTab(int t, bool force = false) {
        if (t == tab && !force) return;
        tab = t;
        if (tab != TabPerf) ClosePdh();
        if (search) {
            SetWindowTextW(search, L"");
            filter.clear();
        }
        if (tab == TabStartup) LoadStartup();
        if (tab == TabServices) LoadServices(true);
        if (tab == TabPerf) OpenPdh();
        Relayout();
        StartTimers();
    }

    // ------------------------------------------------------------ muestreo
    void Tick() {
        SampleSystem();
        switch (tab) {
        case TabProc: RefreshProcesses(); break;
        case TabPerf: SamplePerfExtras(); break;
        case TabServices: LoadServices(false); break;
        default: break;
        }
        Invalidate();
    }

    void SampleSystem() {
        // CPU total y por núcleo
        auto fn = NtQSI();
        if (fn) {
            std::vector<CpuPerf> cur(cpus);
            ULONG len = 0;
            if (fn(8 /*SystemProcessorPerformanceInformation*/, cur.data(), (ULONG)(cur.size() * sizeof(CpuPerf)), &len) >= 0) {
                int n = std::min(cpus, (int)(len / sizeof(CpuPerf)));
                cur.resize(n);
                if ((int)prevCpu.size() == n) {
                    coreUsage.assign(n, 0);
                    ULONGLONG sumIdle = 0, sumTotal = 0;
                    for (int i = 0; i < n; ++i) {
                        ULONGLONG idle = cur[i].IdleTime.QuadPart - prevCpu[i].IdleTime.QuadPart;
                        ULONGLONG total = (cur[i].KernelTime.QuadPart - prevCpu[i].KernelTime.QuadPart) +
                                          (cur[i].UserTime.QuadPart - prevCpu[i].UserTime.QuadPart);
                        coreUsage[i] = total ? (float)std::max(0.0, std::min(100.0, 100.0 * (1.0 - (double)idle / total))) : 0;
                        sumIdle += idle;
                        sumTotal += total;
                    }
                    cpuTotal = sumTotal ? std::max(0.0, std::min(100.0, 100.0 * (1.0 - (double)sumIdle / sumTotal))) : 0;
                    hist[PerfCpu].Push((float)cpuTotal);
                }
                prevCpu = cur;
            }
        }
        GlobalMemoryStatusEx(&mem);
        GetPerformanceInfo(&perf, sizeof(perf));
        hist[PerfMem].Push((float)mem.dwMemoryLoad);
    }

    void OpenPdh() {
        if (pdhQuery) return;
        if (PdhOpenQueryW(nullptr, 0, &pdhQuery) != ERROR_SUCCESS) { pdhQuery = nullptr; return; }
        // Nombres en inglés: funcionan en cualquier idioma de Windows.
        PdhAddEnglishCounterW(pdhQuery, L"\\PhysicalDisk(_Total)\\% Idle Time", 0, &pdhIdle);
        PdhAddEnglishCounterW(pdhQuery, L"\\PhysicalDisk(_Total)\\Disk Read Bytes/sec", 0, &pdhRead);
        PdhAddEnglishCounterW(pdhQuery, L"\\PhysicalDisk(_Total)\\Disk Write Bytes/sec", 0, &pdhWrite);
        PdhCollectQueryData(pdhQuery);
        if (!baseMhz) {
            std::vector<ProcessorPowerInfo> ppi(cpus);
            if (CallNtPowerInformation(ProcessorInformation, nullptr, 0, ppi.data(),
                                       (ULONG)(ppi.size() * sizeof(ProcessorPowerInfo))) == 0)
                baseMhz = ppi[0].MaxMhz;
        }
    }

    void ClosePdh() {
        if (pdhQuery) PdhCloseQuery(pdhQuery);
        pdhQuery = nullptr;
        pdhIdle = pdhRead = pdhWrite = nullptr;
        netPrevTick = 0;
    }

    static double PdhValue(PDH_HCOUNTER c) {
        PDH_FMT_COUNTERVALUE v = {};
        if (c && PdhGetFormattedCounterValue(c, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, nullptr, &v) == ERROR_SUCCESS &&
            v.CStatus == ERROR_SUCCESS)
            return v.doubleValue;
        return 0;
    }

    void SamplePerfExtras() {
        if (pdhQuery && PdhCollectQueryData(pdhQuery) == ERROR_SUCCESS) {
            diskActive = std::max(0.0, std::min(100.0, 100.0 - PdhValue(pdhIdle)));
            diskRead = PdhValue(pdhRead);
            diskWrite = PdhValue(pdhWrite);
            hist[PerfDisk].Push((float)diskActive);
        }
        PMIB_IF_TABLE2 table = nullptr;
        if (GetIfTable2(&table) == NO_ERROR && table) {
            ULONGLONG in = 0, out = 0;
            for (ULONG i = 0; i < table->NumEntries; ++i) {
                const MIB_IF_ROW2& r = table->Table[i];
                if (!r.InterfaceAndOperStatusFlags.HardwareInterface || r.InterfaceAndOperStatusFlags.FilterInterface) continue;
                if (r.OperStatus != IfOperStatusUp || r.Type == IF_TYPE_SOFTWARE_LOOPBACK) continue;
                in += r.InOctets;
                out += r.OutOctets;
            }
            FreeMibTable(table);
            ULONGLONG now = GetTickCount64();
            if (netPrevTick && now > netPrevTick) {
                double secs = (now - netPrevTick) / 1000.0;
                netIn = in >= netPrevIn ? (in - netPrevIn) / secs : 0;
                netOut = out >= netPrevOut ? (out - netPrevOut) / secs : 0;
                hist[PerfNet].Push((float)((netIn + netOut) * 8 / 1000.0)); // kbit/s
            }
            netPrevIn = in;
            netPrevOut = out;
            netPrevTick = now;
        }
    }

    void RefreshProcesses() {
        auto fn = NtQSI();
        if (!fn) return;
        if (spiBuf.empty()) spiBuf.resize(512 * 1024);
        ULONG needed = 0;
        NTSTATUS st;
        while ((st = fn(5 /*SystemProcessInformation*/, spiBuf.data(), (ULONG)spiBuf.size(), &needed)) == (NTSTATUS)0xC0000004L)
            spiBuf.resize(std::max<size_t>(spiBuf.size() * 2, needed + 64 * 1024));
        if (st < 0) return;

        ULONGLONG now = GetTickCount64();
        double wall100ns = lastSample ? (double)(now - lastSample) * 10000.0 : 0;
        double wallSec = lastSample ? (now - lastSample) / 1000.0 : 0;
        lastSample = now;

        std::vector<Proc> list;
        list.reserve(procs.size() + 16);
        std::set<DWORD> alive;
        procCount = threadCount = handleCount = 0;
        BYTE* p = spiBuf.data();
        for (;;) {
            auto* e = (SysProcInfo*)p;
            DWORD pid = (DWORD)(ULONG_PTR)e->UniqueProcessId;
            ++procCount;
            threadCount += e->NumberOfThreads;
            handleCount += e->HandleCount;
            if (pid != 0) {
                Proc pr;
                pr.pid = pid;
                pr.ppid = (DWORD)(ULONG_PTR)e->InheritedFromUniqueProcessId;
                pr.created = e->CreateTime.QuadPart;
                pr.name = e->ImageName.Buffer ? std::wstring(e->ImageName.Buffer, e->ImageName.Length / sizeof(wchar_t))
                                              : (pid == 4 ? L"System" : L"(desconocido)");
                if (EndsWithNoCase(pr.name, L".exe")) pr.name.resize(pr.name.size() - 4);
                pr.ws = e->WorkingSetPrivateSize.QuadPart;
                pr.commit = e->PrivatePageCount;
                pr.threads = e->NumberOfThreads;
                pr.handles = e->HandleCount;
                ULONGLONG cpuTime = e->KernelTime.QuadPart + e->UserTime.QuadPart;
                ULONGLONG io = e->ReadTransferCount.QuadPart + e->WriteTransferCount.QuadPart + e->OtherTransferCount.QuadPart;
                ProcCache& c = cache[pid];
                if (c.created != pr.created) c = ProcCache{ pr.created, cpuTime, io };
                else if (wall100ns > 0) {
                    pr.cpu = std::min(100.0, 100.0 * (double)(cpuTime - c.cpuTime) / (wall100ns * cpus));
                    pr.io = (double)(io - c.ioBytes) / wallSec;
                }
                c.cpuTime = cpuTime;
                c.ioBytes = io;
                alive.insert(pid);
                list.push_back(std::move(pr));
            }
            if (!e->NextEntryOffset) break;
            p += e->NextEntryOffset;
        }
        // Olvidar procesos que ya no existen.
        for (auto it = cache.begin(); it != cache.end();) {
            if (!alive.count(it->first)) it = cache.erase(it);
            else ++it;
        }
        procs.swap(list);
        RebuildProcView();
    }

    void EnsureProcDetails(DWORD pid) {
        auto it = cache.find(pid);
        if (it == cache.end() || it->second.queried) return;
        ProcCache& c = it->second;
        c.queried = true;
        if (HANDLE hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)) {
            wchar_t full[MAX_PATH * 2] = {};
            DWORD len = MAX_PATH * 2;
            if (QueryFullProcessImageNameW(hp, 0, full, &len)) c.path = full;
            CloseHandle(hp);
        }
        if (!c.path.empty()) {
            std::wstring key = ToLower(c.path);
            auto f = iconByPath.find(key);
            c.icon = f != iconByPath.end() ? f->second : (iconByPath[key] = SysIconIndex(c.path));
        } else {
            static int generic = SysIconIndex(L"x.exe", FILE_ATTRIBUTE_NORMAL, true);
            c.icon = generic;
        }
    }

    void RebuildProcView() {
        std::wstring q = ToLower(filter);
        auto cmp = [&](const Proc& a, const Proc& b) {
            int c = 0;
            switch (procTable.sortCol) {
            case 0: c = _wcsicmp(a.name.c_str(), b.name.c_str()); break;
            case 1: c = (a.pid > b.pid) - (a.pid < b.pid); break;
            case 2: c = (a.cpu > b.cpu) - (a.cpu < b.cpu); break;
            case 3: c = (a.ws > b.ws) - (a.ws < b.ws); break;
            case 4: c = (a.io > b.io) - (a.io < b.io); break;
            case 5: c = (a.threads > b.threads) - (a.threads < b.threads); break;
            }
            if (c == 0) c = _wcsicmp(a.name.c_str(), b.name.c_str());
            if (c == 0) c = (a.pid > b.pid) - (a.pid < b.pid);
            return procTable.sortDesc ? c > 0 : c < 0;
        };
        procView.clear();
        if (!tree || !q.empty()) {
            for (int i = 0; i < (int)procs.size(); ++i) {
                procs[i].depth = 0;
                if (!q.empty() && ToLower(procs[i].name).find(q) == std::wstring::npos &&
                    std::to_wstring(procs[i].pid).find(q) == std::wstring::npos)
                    continue;
                procView.push_back(i);
            }
            std::sort(procView.begin(), procView.end(), [&](int a, int b) { return cmp(procs[a], procs[b]); });
            return;
        }
        // Árbol: el padre debe existir y ser más antiguo que el hijo (los PID se reutilizan).
        std::unordered_map<DWORD, int> byPid;
        for (int i = 0; i < (int)procs.size(); ++i) byPid[procs[i].pid] = i;
        std::unordered_map<int, std::vector<int>> children;
        std::vector<int> roots;
        for (int i = 0; i < (int)procs.size(); ++i) {
            auto f = byPid.find(procs[i].ppid);
            if (f != byPid.end() && f->second != i && procs[f->second].created <= procs[i].created)
                children[f->second].push_back(i);
            else
                roots.push_back(i);
        }
        std::function<void(std::vector<int>&, int)> walk = [&](std::vector<int>& level, int depth) {
            std::sort(level.begin(), level.end(), [&](int a, int b) { return cmp(procs[a], procs[b]); });
            for (int i : level) {
                procs[i].depth = depth;
                procView.push_back(i);
                auto c = children.find(i);
                if (c != children.end() && depth < 24) walk(c->second, depth + 1);
            }
        };
        walk(roots, 0);
    }

    const Proc* SelectedProc() const {
        if (procTable.selKey.empty()) return nullptr;
        for (auto& p : procs)
            if (std::to_wstring(p.pid) == procTable.selKey) return &p;
        return nullptr;
    }

    // ------------------------------------------------------------ inicio / servicios
    void LoadStartup() {
        startupItems = startup::Enumerate();
        RebuildStartupView();
    }

    void RebuildStartupView() {
        std::wstring q = ToLower(filter);
        startupView.clear();
        for (int i = 0; i < (int)startupItems.size(); ++i)
            if (q.empty() || ToLower(startupItems[i].name + L" " + startupItems[i].command).find(q) != std::wstring::npos)
                startupView.push_back(i);
        std::sort(startupView.begin(), startupView.end(), [&](int a, int b) {
            const auto& x = startupItems[a];
            const auto& y = startupItems[b];
            int c = 0;
            switch (startupTable.sortCol) {
            case 0: c = _wcsicmp(x.name.c_str(), y.name.c_str()); break;
            case 1: c = (int)y.enabled - (int)x.enabled; break;
            case 2: c = _wcsicmp(x.location.c_str(), y.location.c_str()); break;
            case 3: c = _wcsicmp(x.command.c_str(), y.command.c_str()); break;
            }
            if (c == 0) c = _wcsicmp(x.name.c_str(), y.name.c_str());
            return startupTable.sortDesc ? c > 0 : c < 0;
        });
    }

    std::wstring StartupKey(const startup::Entry& e) const { return e.location + L"|" + e.name; }

    startup::Entry* SelectedStartup() {
        for (auto& e : startupItems)
            if (StartupKey(e) == startupTable.selKey) return &e;
        return nullptr;
    }

    void LoadServices(bool withStartTypes) {
        SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE);
        if (!scm) return;
        DWORD needed = 0, count = 0, resume = 0;
        EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, nullptr, 0, &needed, &count,
                              &resume, nullptr);
        std::vector<BYTE> buf(needed + 4096);
        resume = 0;
        std::map<std::wstring, DWORD> oldStart;
        for (auto& s : services) oldStart[s.name] = s.startType;
        if (EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, buf.data(),
                                  (DWORD)buf.size(), &needed, &count, &resume, nullptr)) {
            auto* arr = (ENUM_SERVICE_STATUS_PROCESSW*)buf.data();
            std::vector<ServiceRow> list;
            list.reserve(count);
            for (DWORD i = 0; i < count; ++i) {
                ServiceRow r;
                r.name = arr[i].lpServiceName;
                r.display = arr[i].lpDisplayName;
                r.state = arr[i].ServiceStatusProcess.dwCurrentState;
                r.pid = arr[i].ServiceStatusProcess.dwProcessId;
                auto f = oldStart.find(r.name);
                if (f != oldStart.end()) r.startType = f->second;
                list.push_back(std::move(r));
            }
            services.swap(list);
        }
        if (withStartTypes || !startTypesLoaded) {
            // Tipo de inicio: una consulta por servicio, solo al abrir la pestaña.
            std::vector<BYTE> cfg(8192);
            for (auto& s : services) {
                SC_HANDLE h = OpenServiceW(scm, s.name.c_str(), SERVICE_QUERY_CONFIG);
                if (!h) continue;
                DWORD n = 0;
                if (QueryServiceConfigW(h, (QUERY_SERVICE_CONFIGW*)cfg.data(), (DWORD)cfg.size(), &n))
                    s.startType = ((QUERY_SERVICE_CONFIGW*)cfg.data())->dwStartType;
                CloseServiceHandle(h);
            }
            startTypesLoaded = true;
        }
        CloseServiceHandle(scm);
        RebuildSvcView();
    }

    void RebuildSvcView() {
        std::wstring q = ToLower(filter);
        svcView.clear();
        for (int i = 0; i < (int)services.size(); ++i)
            if (q.empty() || ToLower(services[i].name + L" " + services[i].display).find(q) != std::wstring::npos)
                svcView.push_back(i);
        std::sort(svcView.begin(), svcView.end(), [&](int a, int b) {
            const auto& x = services[a];
            const auto& y = services[b];
            int c = 0;
            switch (svcTable.sortCol) {
            case 0: c = _wcsicmp(x.name.c_str(), y.name.c_str()); break;
            case 1: c = _wcsicmp(x.display.c_str(), y.display.c_str()); break;
            case 2: c = (x.pid > y.pid) - (x.pid < y.pid); break;
            case 3: c = (x.state == SERVICE_RUNNING ? 0 : 1) - (y.state == SERVICE_RUNNING ? 0 : 1); break;
            case 4: c = (int)x.startType - (int)y.startType; break;
            }
            if (c == 0) c = _wcsicmp(x.display.c_str(), y.display.c_str());
            return svcTable.sortDesc ? c > 0 : c < 0;
        });
    }

    ServiceRow* SelectedService() {
        for (auto& s : services)
            if (s.name == svcTable.selKey) return &s;
        return nullptr;
    }

    // Las operaciones de servicios pueden tardar (esperar a que se detenga): en un hilo.
    void ServiceAction(int action, const std::wstring& name, DWORD startType = 0) {
        HWND target = hwnd;
        std::thread([target, action, name, startType] {
            std::wstring* err = nullptr;
            SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_CONNECT);
            DWORD access = SERVICE_QUERY_STATUS | (action == 0 ? SERVICE_START : action == 1 ? SERVICE_STOP
                                                  : action == 2 ? (SERVICE_STOP | SERVICE_START) : SERVICE_CHANGE_CONFIG);
            SC_HANDLE h = scm ? OpenServiceW(scm, name.c_str(), access) : nullptr;
            DWORD e = 0;
            if (!h) {
                e = GetLastError();
            } else {
                SERVICE_STATUS ss = {};
                if (action == 1 || action == 2) {
                    if (!ControlService(h, SERVICE_CONTROL_STOP, &ss) && GetLastError() != ERROR_SERVICE_NOT_ACTIVE)
                        e = GetLastError();
                    for (int i = 0; i < 100 && !e && QueryServiceStatus(h, &ss) && ss.dwCurrentState != SERVICE_STOPPED; ++i)
                        Sleep(100);
                }
                if (!e && (action == 0 || action == 2) && !StartServiceW(h, 0, nullptr) &&
                    GetLastError() != ERROR_SERVICE_ALREADY_RUNNING)
                    e = GetLastError();
                if (action == 3 && !ChangeServiceConfigW(h, SERVICE_NO_CHANGE, startType, SERVICE_NO_CHANGE, nullptr,
                                                         nullptr, nullptr, nullptr, nullptr, nullptr, nullptr))
                    e = GetLastError();
                CloseServiceHandle(h);
            }
            if (scm) CloseServiceHandle(scm);
            if (e) err = new std::wstring(e == ERROR_ACCESS_DENIED ? L"Acceso denegado: hace falta ejecutar el Administrador "
                                                                     L"de tareas como administrador."
                                                                   : ErrText(e));
            if (!PostMessageW(target, WM_TASKRESULT, 1, (LPARAM)err)) delete err;
        }).detach();
    }

    // ------------------------------------------------------------ acciones de procesos
    static bool IsCritical(const std::wstring& name) {
        static const wchar_t* list[] = { L"system", L"smss", L"csrss", L"wininit", L"winlogon", L"services",
                                         L"lsass", L"fontdrvhost", L"registry", L"memory compression", L"dwm" };
        std::wstring l = ToLower(name);
        for (auto* n : list)
            if (l == n) return true;
        return false;
    }

    std::vector<DWORD> Descendants(DWORD root) const {
        std::vector<DWORD> out;
        std::vector<DWORD> stack{ root };
        std::unordered_map<DWORD, ULONGLONG> created;
        for (auto& p : procs) created[p.pid] = p.created;
        while (!stack.empty()) {
            DWORD cur = stack.back();
            stack.pop_back();
            for (auto& p : procs)
                if (p.ppid == cur && p.pid != cur && created[cur] <= p.created &&
                    std::find(out.begin(), out.end(), p.pid) == out.end()) {
                    out.push_back(p.pid);
                    stack.push_back(p.pid);
                }
        }
        return out;
    }

    void EndProcess(bool wholeTree) {
        const Proc* p = SelectedProc();
        if (!p) return;
        // Copias: durante el MessageBox el timer puede refrescar (y reemplazar) la lista.
        const DWORD pid = p->pid;
        const std::wstring name = p->name;
        if (pid == GetCurrentProcessId()) return;
        if (IsCritical(name)) {
            MessageBoxW(hwnd, (L"\"" + name + L"\" es un proceso esencial de Windows. Finalizarlo bloquearía o reiniciaría el equipo.").c_str(),
                        L"Administrador de tareas", MB_ICONWARNING);
            return;
        }
        std::vector<DWORD> targets;
        if (wholeTree) targets = Descendants(pid);
        std::wstring msg = wholeTree
            ? L"¿Finalizar \"" + name + L"\" y sus " + std::to_wstring(targets.size()) + L" procesos secundarios?"
            : L"¿Finalizar \"" + name + L"\" (PID " + std::to_wstring(pid) + L")?";
        msg += L"\n\nSe perderán los datos sin guardar.";
        if (MessageBoxW(hwnd, msg.c_str(), L"Finalizar tarea", MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) != IDYES) return;
        std::reverse(targets.begin(), targets.end()); // hijos primero
        targets.push_back(pid);
        DWORD failed = 0, lastErr = 0;
        for (DWORD pid : targets) {
            HANDLE hp = OpenProcess(PROCESS_TERMINATE, FALSE, pid);
            if (!hp || !TerminateProcess(hp, 1)) { ++failed; lastErr = GetLastError(); }
            if (hp) CloseHandle(hp);
        }
        if (failed)
            MessageBoxW(hwnd, (L"No se pudo finalizar " + std::to_wstring(failed) + L" proceso(s):\n" + ErrText(lastErr) +
                               (elevated ? L"" : L"\n\nPrueba a reiniciar el Administrador de tareas como administrador."))
                                  .c_str(),
                        L"Finalizar tarea", MB_ICONWARNING);
        procTable.selKey.clear();
        Sleep(80);
        RefreshProcesses();
        Invalidate();
    }

    static bool GetEfficiency(DWORD pid, DWORD priority) {
        bool eco = false;
        if (HANDLE hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)) {
            PROCESS_POWER_THROTTLING_STATE st = {};
            st.Version = PROCESS_POWER_THROTTLING_CURRENT_VERSION;
            if (GetProcessInformation(hp, ProcessPowerThrottling, &st, sizeof(st)))
                eco = (st.ControlMask & st.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED) != 0;
            CloseHandle(hp);
        }
        return eco && priority == IDLE_PRIORITY_CLASS;
    }

    void SetEfficiency(DWORD pid, bool on) {
        HANDLE hp = OpenProcess(PROCESS_SET_INFORMATION, FALSE, pid);
        bool ok = false;
        if (hp) {
            PROCESS_POWER_THROTTLING_STATE st = {};
            st.Version = PROCESS_POWER_THROTTLING_CURRENT_VERSION;
            st.ControlMask = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
            st.StateMask = on ? PROCESS_POWER_THROTTLING_EXECUTION_SPEED : 0;
            ok = SetProcessInformation(hp, ProcessPowerThrottling, &st, sizeof(st)) &&
                 SetPriorityClass(hp, on ? IDLE_PRIORITY_CLASS : NORMAL_PRIORITY_CLASS);
            CloseHandle(hp);
        }
        if (!ok) MessageBoxW(hwnd, (L"No se pudo cambiar el modo eficiencia:\n" + ErrText(GetLastError())).c_str(),
                             L"Modo eficiencia", MB_ICONWARNING);
    }

    void SetPriority(DWORD pid, DWORD cls) {
        if (cls == REALTIME_PRIORITY_CLASS &&
            MessageBoxW(hwnd, L"La prioridad en tiempo real puede dejar el sistema sin respuesta. ¿Continuar?",
                        L"Cambiar prioridad", MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) != IDYES)
            return;
        HANDLE hp = OpenProcess(PROCESS_SET_INFORMATION, FALSE, pid);
        bool ok = hp && SetPriorityClass(hp, cls);
        DWORD err = GetLastError();
        if (hp) CloseHandle(hp);
        if (!ok) MessageBoxW(hwnd, (L"No se pudo cambiar la prioridad:\n" + ErrText(err)).c_str(), L"Cambiar prioridad",
                             MB_ICONWARNING);
    }

    void TrimProcess(DWORD pid) {
        HANDLE hp = OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if (hp) {
            EmptyWorkingSet(hp);
            CloseHandle(hp);
        }
    }

    // Recorta los conjuntos de trabajo de todos los procesos accesibles (en segundo plano).
    void FreeRam() {
        std::vector<DWORD> pids;
        for (auto& p : procs) pids.push_back(p.pid);
        HWND target = hwnd;
        freeRamMsg = L"Liberando memoria...";
        Invalidate();
        std::thread([pids, target] {
            SetThreadEco();
            MEMORYSTATUSEX before = { sizeof(before) };
            GlobalMemoryStatusEx(&before);
            int done = 0;
            for (DWORD pid : pids) {
                HANDLE hp = OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
                if (!hp) continue;
                if (EmptyWorkingSet(hp)) ++done;
                CloseHandle(hp);
            }
            Sleep(400);
            MEMORYSTATUSEX after = { sizeof(after) };
            GlobalMemoryStatusEx(&after);
            LONGLONG freed = (LONGLONG)after.ullAvailPhys - (LONGLONG)before.ullAvailPhys;
            auto* msg = new std::wstring(L"Liberados " + FormatBytes(freed > 0 ? (ULONGLONG)freed : 0) + L" de " +
                                         std::to_wstring(done) + L" procesos");
            if (!PostMessageW(target, WM_TASKRESULT, 2, (LPARAM)msg)) delete msg;
        }).detach();
    }

    void ShowProcMenu(POINT pt) {
        const Proc* p = SelectedProc();
        if (!p) return;
        DWORD pid = p->pid;
        EnsureProcDetails(pid);
        std::wstring path = cache[pid].path;
        std::wstring name = p->name;
        DWORD prio = 0;
        if (HANDLE hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)) {
            prio = GetPriorityClass(hp);
            CloseHandle(hp);
        }
        bool eco = GetEfficiency(pid, prio);

        HMENU m = CreatePopupMenu();
        HMENU pm = CreatePopupMenu();
        struct { DWORD cls; const wchar_t* label; } prios[] = {
            { REALTIME_PRIORITY_CLASS, L"Tiempo real" }, { HIGH_PRIORITY_CLASS, L"Alta" },
            { ABOVE_NORMAL_PRIORITY_CLASS, L"Superior a lo normal" }, { NORMAL_PRIORITY_CLASS, L"Normal" },
            { BELOW_NORMAL_PRIORITY_CLASS, L"Inferior a lo normal" }, { IDLE_PRIORITY_CLASS, L"Baja" },
        };
        for (int i = 0; i < 6; ++i)
            AppendMenuW(pm, MF_STRING | (prio == prios[i].cls ? MF_CHECKED : 0), 100 + i, prios[i].label);
        AppendMenuW(m, MF_STRING, 1, L"Finalizar tarea\tSupr");
        AppendMenuW(m, MF_STRING, 2, L"Finalizar árbol de procesos");
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING | (eco ? MF_CHECKED : 0), 3, L"Modo eficiencia");
        AppendMenuW(m, MF_POPUP | (prio ? 0 : MF_GRAYED), (UINT_PTR)pm, L"Establecer prioridad");
        AppendMenuW(m, MF_STRING, 4, L"Establecer afinidad...");
        AppendMenuW(m, MF_STRING, 5, L"Liberar memoria del proceso");
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING | (path.empty() ? MF_GRAYED : 0), 6, L"Abrir ubicación del archivo");
        AppendMenuW(m, MF_STRING | (path.empty() ? MF_GRAYED : 0), 7, L"Propiedades");
        AppendMenuW(m, MF_STRING, 8, L"Copiar detalles");
        int cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        switch (cmd) {
        case 1: EndProcess(false); break;
        case 2: EndProcess(true); break;
        case 3: SetEfficiency(pid, !eco); break;
        case 4: OpenAffinity(pid, name); break;
        case 5: TrimProcess(pid); break;
        case 6: OpenInExplorer(path, true); break;
        case 7: SHObjectProperties(hwnd, SHOP_FILEPATH, path.c_str(), nullptr); break;
        case 8: {
            const Proc* q = SelectedProc();
            if (q) {
                wchar_t buf[256];
                swprintf_s(buf, L"%s\tPID %lu\tCPU %.1f %%\t%s\t%lu hilos", q->name.c_str(), q->pid, q->cpu,
                           FormatBytes(q->ws).c_str(), q->threads);
                CopyTextToClipboard(hwnd, std::wstring(buf) + (path.empty() ? L"" : L"\t" + path));
            }
            break;
        }
        default:
            if (cmd >= 100 && cmd < 106) SetPriority(pid, prios[cmd - 100].cls);
        }
    }

    void OpenAffinity(DWORD pid, const std::wstring& name) {
        if (dialog) return;
        auto* d = new AffinityDialog();
        if (d->Open(hwnd, pid, name)) {
            dialog.reset(d);
            d->onClosed = [this] { PostMessageW(hwnd, WM_TASKRESULT, 3, 0); };
        } else {
            delete d;
        }
    }

    void OpenNewTask() {
        if (dialog) return;
        auto* d = new NewTaskDialog();
        if (d->Open(hwnd)) {
            dialog.reset(d);
            d->onClosed = [this] { PostMessageW(hwnd, WM_TASKRESULT, 3, 0); };
        } else {
            delete d;
        }
    }

    void RestartElevated() {
        static const wchar_t* tabs[] = { L"procesos", L"rendimiento", L"inicio", L"servicios" };
        std::wstring args = std::wstring(L"--taskmgr --tab ") + tabs[tab];
        SHELLEXECUTEINFOW sei = { sizeof(sei) };
        sei.hwnd = hwnd;
        sei.lpVerb = L"runas";
        std::wstring exe = GetOwnPath();
        sei.lpFile = exe.c_str();
        sei.lpParameters = args.c_str();
        sei.nShow = SW_SHOWNORMAL;
        if (ShellExecuteExW(&sei)) Close();
    }

    void ToggleStartupItem() {
        startup::Entry* e = SelectedStartup();
        if (!e) return;
        LSTATUS st = startup::SetEnabled(*e, !e->enabled);
        if (st != ERROR_SUCCESS) {
            std::wstring msg = L"No se pudo cambiar el estado de \"" + e->name + L"\":\n" + ErrText(st);
            if (st == ERROR_ACCESS_DENIED && !elevated) {
                msg += L"\n\n¿Reiniciar el Administrador de tareas como administrador?";
                if (MessageBoxW(hwnd, msg.c_str(), L"Inicio", MB_YESNO | MB_ICONWARNING) == IDYES) RestartElevated();
            } else {
                MessageBoxW(hwnd, msg.c_str(), L"Inicio", MB_ICONWARNING);
            }
            return;
        }
        e->enabled = !e->enabled;
        RebuildStartupView();
        Invalidate();
    }

    void ShowStartupMenu(POINT pt) {
        startup::Entry* e = SelectedStartup();
        if (!e) return;
        HMENU m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, 1, e->enabled ? L"Deshabilitar" : L"Habilitar");
        AppendMenuW(m, MF_STRING, 2, L"Abrir ubicación del archivo");
        AppendMenuW(m, MF_STRING, 3, L"Copiar comando");
        int cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        if (cmd == 1) ToggleStartupItem();
        else if (cmd == 2) OpenInExplorer(startup::TargetPath(*e), true);
        else if (cmd == 3) CopyTextToClipboard(hwnd, e->command);
    }

    void ShowServiceMenu(POINT pt) {
        ServiceRow* s = SelectedService();
        if (!s) return;
        // Copias: durante el menú el timer puede recargar la lista de servicios.
        const std::wstring name = s->name;
        const DWORD pid = s->pid;
        bool running = s->state == SERVICE_RUNNING;
        HMENU m = CreatePopupMenu();
        HMENU st = CreatePopupMenu();
        AppendMenuW(st, MF_STRING | (s->startType == SERVICE_AUTO_START ? MF_CHECKED : 0), 10, L"Automático");
        AppendMenuW(st, MF_STRING | (s->startType == SERVICE_DEMAND_START ? MF_CHECKED : 0), 11, L"Manual");
        AppendMenuW(st, MF_STRING | (s->startType == SERVICE_DISABLED ? MF_CHECKED : 0), 12, L"Deshabilitado");
        AppendMenuW(m, MF_STRING | (running ? MF_GRAYED : 0), 1, L"Iniciar");
        AppendMenuW(m, MF_STRING | (running ? 0 : MF_GRAYED), 2, L"Detener");
        AppendMenuW(m, MF_STRING | (running ? 0 : MF_GRAYED), 3, L"Reiniciar");
        AppendMenuW(m, MF_POPUP, (UINT_PTR)st, L"Tipo de inicio");
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING | (pid ? 0 : MF_GRAYED), 4, L"Ir al proceso");
        AppendMenuW(m, MF_STRING, 5, L"Copiar nombre");
        int cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        switch (cmd) {
        case 1: ServiceAction(0, name); break;
        case 2: ServiceAction(1, name); break;
        case 3: ServiceAction(2, name); break;
        case 4:
            SwitchTab(TabProc);
            procTable.selKey = std::to_wstring(pid);
            RefreshProcesses();
            for (int i = 0; i < (int)procView.size(); ++i)
                if (procs[procView[i]].pid == pid) procTable.EnsureVisible(i, (int)procView.size());
            Invalidate();
            break;
        case 5: CopyTextToClipboard(hwnd, name); break;
        case 10: ServiceAction(3, name, SERVICE_AUTO_START); break;
        case 11: ServiceAction(3, name, SERVICE_DEMAND_START); break;
        case 12: ServiceAction(3, name, SERVICE_DISABLED); break;
        }
    }

    // ------------------------------------------------------------ pintado
    Table* CurTable() {
        switch (tab) {
        case TabProc: return &procTable;
        case TabStartup: return &startupTable;
        case TabServices: return &svcTable;
        default: return nullptr;
        }
    }

    int CurCount() const {
        switch (tab) {
        case TabProc: return (int)procView.size();
        case TabStartup: return (int)startupView.size();
        case TabServices: return (int)svcView.size();
        default: return 0;
        }
    }

    std::wstring ButtonLabel(int id) const {
        switch (id) {
        case BEnd: return L"Finalizar tarea";
        case BNewTask: return L"Nueva tarea";
        case BTree: return tree ? L"Lista" : L"Árbol";
        case BFreeRam: return L"Liberar RAM";
        case BToggleStartup: {
            for (auto& e : startupItems)
                if (StartupKey(e) == startupTable.selKey) return e.enabled ? L"Deshabilitar" : L"Habilitar";
            return L"Deshabilitar";
        }
        case BStartupLocation: return L"Abrir ubicación";
        case BRefresh: return L"Actualizar";
        case BSvcStart: return L"Iniciar";
        case BSvcStop: return L"Detener";
        case BSvcRestart: return L"Reiniciar";
        case BSvcOpen: return L"Abrir Servicios";
        case BElevate: return L"Ejecutar como admin.";
        default: return L"";
        }
    }

    bool ButtonEnabled(int id) {
        switch (id) {
        case BEnd: return SelectedProc() != nullptr;
        case BToggleStartup: case BStartupLocation: return !startupTable.selKey.empty();
        case BSvcStart: { auto* s = SelectedService(); return s && s->state == SERVICE_STOPPED; }
        case BSvcStop: case BSvcRestart: { auto* s = SelectedService(); return s && s->state == SERVICE_RUNNING; }
        default: return true;
        }
    }

    void OnPaint(Graphics& g, int w, int h) override {
        style::DrawChrome(g, w, h, L"Administrador de tareas", L"", IsZoomed(hwnd) != FALSE,
                          hoverBtn >= BMin && hoverBtn <= BClose ? hoverBtn : 0);

        // Pestañas
        const wchar_t* names[TabCount] = { L"Procesos", L"Rendimiento", L"Inicio", L"Servicios" };
        for (int i = 0; i < TabCount; ++i) {
            bool act = i == tab, hov = hoverBtn == BTab0 + i;
            if (hov && !act) DrawRounded(g, tabRc[i], SF(6), style::rowHover);
            DrawTextStr(g, names[i], act ? fontBold : fontBody, act ? style::text : style::textDim, tabRc[i],
                        StringAlignmentCenter);
            if (act)
                DrawRounded(g, RectF(tabRc[i].X + S(12), tabRc[i].Y + tabRc[i].Height - S(3), tabRc[i].Width - S(24), (float)S(3)),
                            SF(1.5f), style::accent);
        }
        wchar_t summary[160];
        swprintf_s(summary, L"CPU %.0f %%   ·   Memoria %s / %s (%lu %%)   ·   %llu procesos%s", cpuTotal,
                   FormatBytes(mem.ullTotalPhys - mem.ullAvailPhys).c_str(), FormatBytes(mem.ullTotalPhys).c_str(),
                   mem.dwMemoryLoad, procCount, elevated ? L"   ·   Administrador" : L"");
        DrawTextStr(g, summary, fontSmall, style::textDim,
                    RectF(tabRc[TabCount - 1].X + tabRc[TabCount - 1].Width + S(16), tabRc[0].Y,
                          (float)w - (tabRc[TabCount - 1].X + tabRc[TabCount - 1].Width + S(32)), tabRc[0].Height),
                    StringAlignmentFar);
        HLine(g, 0, (float)w, tabRc[0].Y + tabRc[0].Height + S(2), C(30, 255, 255, 255));

        // Barra de herramientas
        if (tab != TabPerf) {
            DrawRounded(g, searchRc, SF(6), C(255, 35, 40, 52), C(45, 255, 255, 255));
            DrawTextStr(g, L"", fontIcon, style::textDim,
                        RectF(searchRc.X + S(8), searchRc.Y, (float)S(20), searchRc.Height), StringAlignmentCenter);
        } else {
            DrawTextStr(g, L"Rendimiento", fontTitle, style::text, RectF(searchRc.X, searchRc.Y, (float)S(300), searchRc.Height));
        }
        for (int id = BEnd; id <= BRefresh; ++id) {
            if (btnRc[id].Width <= 0) continue;
            bool en = ButtonEnabled(id);
            style::DrawButton(g, btnRc[id], ButtonLabel(id), hoverBtn == id && en, id == BNewTask,
                              id == BEnd, !en);
        }

        switch (tab) {
        case TabProc: PaintProcesses(g); break;
        case TabPerf: PaintPerformance(g); break;
        case TabStartup: PaintStartup(g); break;
        case TabServices: PaintServices(g); break;
        }
    }

    void PaintProcesses(Graphics& g) {
        Table::Source src;
        src.key = [&](int i) { return std::to_wstring(procs[procView[i]].pid); };
        src.indent = [&](int i) { return procs[procView[i]].depth; };
        src.icon = [&](int i) {
            DWORD pid = procs[procView[i]].pid;
            EnsureProcDetails(pid);
            return SysIconBitmap(cache[pid].icon, S(16));
        };
        src.cell = [&](int i, int c) -> std::wstring {
            const Proc& p = procs[procView[i]];
            wchar_t buf[48];
            switch (c) {
            case 0: return p.name;
            case 1: return std::to_wstring(p.pid);
            case 2: swprintf_s(buf, p.cpu >= 0.05 ? L"%.1f %%" : L"0 %%", p.cpu); return buf;
            case 3: return FormatBytes(p.ws);
            case 4: return p.io >= 1024 ? FormatRate(p.io) : L"0 KB/s";
            case 5: return std::to_wstring(p.threads);
            }
            return L"";
        };
        src.color = [&](int i, int c) {
            const Proc& p = procs[procView[i]];
            if (c == 2) return p.cpu >= 50 ? kRed : (p.cpu >= 15 ? kAmber : style::textDim);
            if (c == 3) return p.ws >= 1024ull * 1024 * 1024 ? kAmber : style::textDim;
            return c == 0 ? style::text : style::textDim;
        };
        procTable.Paint(g, (int)procView.size(), src, filter.empty() ? L"Cargando..." : L"Ningún proceso coincide.");
    }

    void PaintStartup(Graphics& g) {
        Table::Source src;
        src.key = [&](int i) { return StartupKey(startupItems[startupView[i]]); };
        src.icon = [&](int i) {
            static std::unordered_map<std::wstring, int> icons;
            const auto& e = startupItems[startupView[i]];
            std::wstring t = startup::TargetPath(e);
            auto f = icons.find(t);
            int idx = f != icons.end() ? f->second : (icons[t] = SysIconIndex(t));
            return SysIconBitmap(idx, S(16));
        };
        src.cell = [&](int i, int c) -> std::wstring {
            const auto& e = startupItems[startupView[i]];
            switch (c) {
            case 0: return e.name;
            case 1: return e.enabled ? L"Habilitado" : L"Deshabilitado";
            case 2: return e.location;
            case 3: return e.command;
            }
            return L"";
        };
        src.color = [&](int i, int c) {
            const auto& e = startupItems[startupView[i]];
            if (c == 1) return e.enabled ? kGreen : style::textFaint;
            if (c == 0) return e.enabled ? style::text : style::textDim;
            return style::textDim;
        };
        startupTable.Paint(g, (int)startupView.size(), src, L"No hay programas de inicio.");
    }

    void PaintServices(Graphics& g) {
        Table::Source src;
        src.key = [&](int i) { return services[svcView[i]].name; };
        src.cell = [&](int i, int c) -> std::wstring {
            const auto& s = services[svcView[i]];
            switch (c) {
            case 0: return s.name;
            case 1: return s.display;
            case 2: return s.pid ? std::to_wstring(s.pid) : L"";
            case 3: return ServiceState(s.state);
            case 4: return StartTypeName(s.startType);
            }
            return L"";
        };
        src.color = [&](int i, int c) {
            const auto& s = services[svcView[i]];
            if (c == 3) return s.state == SERVICE_RUNNING ? kGreen : style::textFaint;
            return c == 0 ? style::text : style::textDim;
        };
        svcTable.Paint(g, (int)svcView.size(), src, L"No hay servicios que mostrar.");
    }

    void DrawGraph(Graphics& g, const RectF& rc, const History& h, float maxV, const Color& color, bool grid) {
        DrawRounded(g, rc, SF(6), C(255, 22, 26, 35), C(40, 255, 255, 255));
        if (grid) {
            for (int i = 1; i < 4; ++i) HLine(g, rc.X, rc.X + rc.Width, rc.Y + rc.Height * i / 4, C(18, 255, 255, 255));
            Pen vp(C(18, 255, 255, 255), 1.0f);
            for (int i = 1; i < 6; ++i) g.DrawLine(&vp, rc.X + rc.Width * i / 6, rc.Y, rc.X + rc.Width * i / 6, rc.Y + rc.Height);
        }
        if (h.count < 2 || maxV <= 0) return;
        std::vector<PointF> pts;
        float step = rc.Width / 59.0f;
        float x0 = rc.X + rc.Width - (h.count - 1) * step;
        for (int i = 0; i < h.count; ++i) {
            float v = std::min(1.0f, h.At(i) / maxV);
            pts.push_back(PointF(x0 + i * step, rc.Y + rc.Height - 2 - v * (rc.Height - 4)));
        }
        std::vector<PointF> poly(pts);
        poly.push_back(PointF(pts.back().X, rc.Y + rc.Height - 1));
        poly.push_back(PointF(pts.front().X, rc.Y + rc.Height - 1));
        SolidBrush fill(Color(50, color.GetR(), color.GetG(), color.GetB()));
        g.FillPolygon(&fill, poly.data(), (INT)poly.size());
        Pen line(color, SF(1.5f));
        g.DrawLines(&line, pts.data(), (INT)pts.size());
    }

    struct PerfInfo {
        const wchar_t* title;
        std::wstring value, sub;
        Color color;
        float maxV;
    };

    PerfInfo Info(int k) const {
        wchar_t buf[96];
        switch (k) {
        case PerfCpu:
            swprintf_s(buf, L"%.0f %%", cpuTotal);
            return { L"CPU", buf, std::to_wstring(cpus) + L" procesadores lógicos", style::accent, 100 };
        case PerfMem:
            return { L"Memoria", FormatBytes(mem.ullTotalPhys - mem.ullAvailPhys) + L" / " + FormatBytes(mem.ullTotalPhys),
                     std::to_wstring(mem.dwMemoryLoad) + L" % en uso", kPurple, 100 };
        case PerfDisk:
            swprintf_s(buf, L"%.0f %%", diskActive);
            return { L"Disco", buf, L"L: " + FormatRate(diskRead) + L"  E: " + FormatRate(diskWrite), kGreen, 100 };
        default: {
            float mx = std::max(1000.0f, hist[PerfNet].Max() * 1.15f);
            return { L"Red", L"↓ " + FormatRate(netIn), L"↑ " + FormatRate(netOut), kAmber, mx };
        }
        }
    }

    void PaintPerformance(Graphics& g) {
        for (int k = 0; k < PerfCount; ++k) {
            PerfInfo in = Info(k);
            RectF r = perfItemRc[k];
            if (k == perfSel) DrawRounded(g, r, SF(6), C(60, 59, 130, 246));
            else if (hoverBtn == 40 + k) DrawRounded(g, r, SF(6), style::rowHover);
            RectF spark(r.X + S(8), r.Y + S(8), (float)S(70), r.Height - S(16));
            DrawGraph(g, spark, hist[k], in.maxV, in.color, false);
            float tx = spark.X + spark.Width + S(12);
            DrawTextStr(g, in.title, fontBold, style::text, RectF(tx, r.Y + S(6), r.Width - (tx - r.X) - S(4), (float)S(20)));
            DrawTextStr(g, in.value, fontSmall, style::text, RectF(tx, r.Y + S(26), r.Width - (tx - r.X) - S(4), (float)S(18)));
            DrawTextStr(g, in.sub, fontSmall, style::textFaint, RectF(tx, r.Y + S(43), r.Width - (tx - r.X) - S(4), (float)S(18)));
        }
        Pen sep(C(30, 255, 255, 255), 1.0f);
        g.DrawLine(&sep, perfListRc.X + perfListRc.Width + S(8), perfListRc.Y, perfListRc.X + perfListRc.Width + S(8),
                   perfListRc.Y + perfListRc.Height);

        RectF d = perfDetailRc;
        PerfInfo in = Info(perfSel);
        DrawTextStr(g, in.title, fontBig, style::text, RectF(d.X, d.Y, d.Width, (float)S(40)));
        DrawTextStr(g, in.value, fontTitle, in.color, RectF(d.X, d.Y, d.Width, (float)S(40)), StringAlignmentFar);
        float graphH = std::max((float)S(120), d.Height * 0.42f);
        RectF graph(d.X, d.Y + S(48), d.Width, graphH);
        DrawGraph(g, graph, hist[perfSel], in.maxV, in.color, true);
        DrawTextStr(g, L"60 segundos", fontSmall, style::textFaint, RectF(graph.X, graph.Y + graph.Height + S(2), (float)S(120), (float)S(18)));

        // Estadísticas en dos columnas
        std::vector<std::pair<std::wstring, std::wstring>> stats;
        auto pages = [&](SIZE_T n) { return FormatBytes((ULONGLONG)n * perf.PageSize); };
        ULONGLONG up = GetTickCount64() / 1000;
        wchar_t upBuf[64];
        swprintf_s(upBuf, L"%llu d %02llu:%02llu:%02llu", up / 86400, (up / 3600) % 24, (up / 60) % 60, up % 60);
        switch (perfSel) {
        case PerfCpu:
            stats = { { L"Uso", in.value },
                      { L"Velocidad base", baseMhz ? std::to_wstring(baseMhz / 1000) + L"," + std::to_wstring((baseMhz % 1000) / 10) + L" GHz" : L"—" },
                      { L"Procesos", std::to_wstring(perf.ProcessCount) },
                      { L"Subprocesos", std::to_wstring(perf.ThreadCount) },
                      { L"Identificadores", std::to_wstring(perf.HandleCount) },
                      { L"Tiempo activo", upBuf } };
            break;
        case PerfMem:
            stats = { { L"En uso", FormatBytes(mem.ullTotalPhys - mem.ullAvailPhys) },
                      { L"Disponible", FormatBytes(mem.ullAvailPhys) },
                      { L"Confirmada", pages(perf.CommitTotal) + L" / " + pages(perf.CommitLimit) },
                      { L"En caché", pages(perf.SystemCache) },
                      { L"Bloque paginado", pages(perf.KernelPaged) },
                      { L"Bloque no paginado", pages(perf.KernelNonpaged) } };
            break;
        case PerfDisk:
            stats = { { L"Tiempo activo", in.value }, { L"Velocidad de lectura", FormatRate(diskRead) },
                      { L"Velocidad de escritura", FormatRate(diskWrite) } };
            break;
        default:
            stats = { { L"Recepción", FormatRate(netIn) }, { L"Envío", FormatRate(netOut) },
                      { L"Pico (60 s)", FormatRate(hist[PerfNet].Max() * 1000 / 8) } };
            break;
        }
        float sy = graph.Y + graph.Height + S(28);
        float colW = d.Width / 3;
        for (size_t i = 0; i < stats.size(); ++i) {
            float sx = d.X + (i % 3) * colW;
            float yy = sy + (i / 3) * S(52);
            DrawTextStr(g, stats[i].first, fontSmall, style::textFaint, RectF(sx, yy, colW - S(10), (float)S(18)));
            DrawTextStr(g, stats[i].second, fontBold, style::text, RectF(sx, yy + S(18), colW - S(10), (float)S(24)));
        }
        float after = sy + ((stats.size() + 2) / 3) * S(52) + S(6);

        if (perfSel == PerfCpu && !coreUsage.empty()) {
            // Uso por núcleo como barras compactas.
            int n = (int)coreUsage.size();
            int perRow = std::max(1, std::min(n, (int)(d.Width / S(56))));
            float cw = d.Width / perRow;
            float bh = (float)S(36);
            for (int i = 0; i < n; ++i) {
                float cx = d.X + (i % perRow) * cw;
                float cy = after + (i / perRow) * (bh + S(22));
                if (cy + bh > d.Y + d.Height) break;
                RectF bar(cx, cy, cw - S(6), bh);
                DrawRounded(g, bar, SF(4), C(255, 22, 26, 35), C(40, 255, 255, 255));
                float fillH = bar.Height * coreUsage[i] / 100.0f;
                if (fillH > 1) DrawRounded(g, RectF(bar.X, bar.Y + bar.Height - fillH, bar.Width, fillH), SF(4),
                                           Color(170, 76, 141, 255));
                DrawTextStr(g, std::to_wstring(i), fontSmall, style::textFaint, RectF(bar.X, cy + bh, bar.Width, (float)S(18)),
                            StringAlignmentCenter);
            }
        }
        if (perfSel == PerfMem && !freeRamMsg.empty())
            DrawTextStr(g, freeRamMsg, fontSmall, kGreen, RectF(d.X, after, d.Width, (float)S(20)));
        if (perfSel == PerfMem)
            DrawTextV(g, L"\"Liberar RAM\" recorta la memoria en uso de cada proceso. Windows la reutiliza para otras "
                         L"aplicaciones; si un proceso vuelve a necesitarla, la recupera del disco o de la caché.",
                      fontSmall, style::textFaint, RectF(d.X, after + S(24), d.Width, (float)S(40)), StringAlignmentNear, true);
    }

    // ------------------------------------------------------------ entrada
    int ButtonAt(int x, int y) const {
        for (int id : { BMin, BMax, BClose })
            if (InRect(btnRc[id], x, y)) return id;
        for (int i = 0; i < TabCount; ++i)
            if (InRect(tabRc[i], x, y)) return BTab0 + i;
        for (int id = BEnd; id <= BRefresh; ++id)
            if (btnRc[id].Width > 0 && InRect(btnRc[id], x, y)) return id;
        if (tab == TabPerf)
            for (int k = 0; k < PerfCount; ++k)
                if (InRect(perfItemRc[k], x, y)) return 40 + k;
        return BNone;
    }

    void Click(int id) {
        if (id >= BEnd && id <= BRefresh && !ButtonEnabled(id)) return;
        switch (id) {
        case BMin: ShowWindow(hwnd, SW_MINIMIZE); break;
        case BMax: ShowWindow(hwnd, IsZoomed(hwnd) ? SW_RESTORE : SW_MAXIMIZE); break;
        case BClose: Close(); break;
        case BEnd: EndProcess(false); break;
        case BNewTask: OpenNewTask(); break;
        case BTree: tree = !tree; RebuildProcView(); Invalidate(); break;
        case BFreeRam: FreeRam(); break;
        case BToggleStartup: ToggleStartupItem(); break;
        case BStartupLocation: if (auto* e = SelectedStartup()) OpenInExplorer(startup::TargetPath(*e), true); break;
        case BRefresh: LoadStartup(); Invalidate(); break;
        case BSvcStart: if (auto* s = SelectedService()) ServiceAction(0, s->name); break;
        case BSvcStop: if (auto* s = SelectedService()) ServiceAction(1, s->name); break;
        case BSvcRestart: if (auto* s = SelectedService()) ServiceAction(2, s->name); break;
        case BSvcOpen: Launch(L"services.msc"); break;
        case BElevate: RestartElevated(); break;
        default:
            if (id >= BTab0 && id < BTab0 + TabCount) SwitchTab(id - BTab0);
            else if (id >= 40 && id < 40 + PerfCount) { perfSel = id - 40; Invalidate(); }
        }
    }

    std::wstring RowKey(int row) {
        switch (tab) {
        case TabProc: return std::to_wstring(procs[procView[row]].pid);
        case TabStartup: return StartupKey(startupItems[startupView[row]]);
        case TabServices: return services[svcView[row]].name;
        default: return L"";
        }
    }

    void OnMouseDown(int x, int y) override {
        int b = ButtonAt(x, y);
        if (b) { Click(b); return; }
        if (InRect(searchRc, x, y) && tab != TabPerf) { SetFocus(search); return; }
        Table* t = CurTable();
        if (!t) return;
        int c = t->ColAt(x, y);
        if (c >= 0) {
            t->ClickHeader(c);
            if (tab == TabProc) RebuildProcView();
            else if (tab == TabStartup) RebuildStartupView();
            else RebuildSvcView();
            Invalidate();
            return;
        }
        int r = t->RowAt(x, y, CurCount());
        t->selKey = r >= 0 ? RowKey(r) : L"";
        Invalidate();
    }

    void OnDoubleClick(int x, int y) override {
        Table* t = CurTable();
        if (!t || t->RowAt(x, y, CurCount()) < 0) return;
        if (tab == TabStartup) ToggleStartupItem();
        else if (tab == TabProc) {
            if (const Proc* p = SelectedProc()) {
                EnsureProcDetails(p->pid);
                std::wstring path = cache[p->pid].path;
                if (!path.empty()) SHObjectProperties(hwnd, SHOP_FILEPATH, path.c_str(), nullptr);
            }
        }
    }

    void OnMouseMove(int x, int y) override {
        int b = ButtonAt(x, y);
        if (b != hoverBtn) {
            hoverBtn = b;
            Invalidate();
            std::wstring t;
            if (b == BTree) t = L"Agrupar los procesos bajo el proceso que los creó";
            else if (b == BFreeRam) t = L"Recorta la memoria en uso de todos los procesos";
            else if (b == BElevate) t = L"Necesario para cambiar servicios y entradas de inicio del equipo";
            tip.Set(b ? (b < 40 ? btnRc[b] : perfItemRc[b - 40]) : RectF(), t);
        }
        if (Table* t = CurTable()) {
            int r = t->RowAt(x, y, CurCount());
            if (r != t->hover) { t->hover = r; InvalidateRectF(t->body); }
        }
    }

    void OnWheel(int delta) override {
        if (Table* t = CurTable()) {
            t->scroll -= (delta / WHEEL_DELTA) * 3;
            t->Clamp(CurCount());
            InvalidateRectF(t->body);
        }
    }

    void OnKeyDown(int vk) override {
        bool ctrl = GetKeyState(VK_CONTROL) < 0;
        if (ctrl && vk == VK_TAB) { SwitchTab((tab + (GetKeyState(VK_SHIFT) < 0 ? TabCount - 1 : 1)) % TabCount); return; }
        if (ctrl && vk >= '1' && vk <= '4') { SwitchTab(vk - '1'); return; }
        if (ctrl && vk == 'F') { SetFocus(search); return; }
        if (ctrl && vk == 'N') { OpenNewTask(); return; }
        if (vk == VK_F5) { Tick(); if (tab == TabStartup) LoadStartup(); return; }
        Table* t = CurTable();
        if (!t) return;
        int n = CurCount();
        int cur = -1;
        for (int i = 0; i < n && !t->selKey.empty(); ++i)
            if (RowKey(i) == t->selKey) { cur = i; break; }
        int next = cur;
        switch (vk) {
        case VK_UP: next = std::max(0, cur - 1); break;
        case VK_DOWN: next = std::min(n - 1, cur + 1); break;
        case VK_PRIOR: next = std::max(0, cur - t->Visible()); break;
        case VK_NEXT: next = std::min(n - 1, std::max(0, cur) + t->Visible()); break;
        case VK_HOME: next = 0; break;
        case VK_END: next = n - 1; break;
        case VK_DELETE: if (tab == TabProc) EndProcess(GetKeyState(VK_SHIFT) < 0); return;
        case VK_SPACE: if (tab == TabStartup) ToggleStartupItem(); return;
        case VK_APPS: {
            POINT pt = { (LONG)t->body.X + S(60), (LONG)t->body.Y + S(30) };
            ClientToScreen(hwnd, &pt);
            ContextMenuAt(pt);
            return;
        }
        default: return;
        }
        if (next >= 0 && next < n) {
            t->selKey = RowKey(next);
            t->EnsureVisible(next, n);
            Invalidate();
        }
    }

    void ContextMenuAt(POINT pt) {
        if (tab == TabProc) ShowProcMenu(pt);
        else if (tab == TabStartup) ShowStartupMenu(pt);
        else if (tab == TabServices) ShowServiceMenu(pt);
    }

    void OnTimer(UINT_PTR id) override {
        if (id == 1) Tick();
    }

    void OnCommand(int id, int code) override {
        if (id == 100 && code == EN_CHANGE) {
            filter = GetWindowString(search);
            if (tab == TabProc) RebuildProcView();
            else if (tab == TabStartup) RebuildStartupView();
            else if (tab == TabServices) RebuildSvcView();
            if (Table* t = CurTable()) t->scroll = 0;
            Invalidate();
        }
    }

    LRESULT OnHitTest(int x, int y) override {
        return FrameHitTest(x, y, S(kCaption), { btnRc[BMin], btnRc[BMax], btnRc[BClose] });
    }

    void OnDestroy() override {
        KillTimer(hwnd, 1);
        dialog.reset();
        PostQuitMessage(0);
    }

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        switch (msg) {
        case WM_RBUTTONUP: {
            int x = GET_X_LPARAM(lp), y = GET_Y_LPARAM(lp);
            Table* t = CurTable();
            if (!t) return 0;
            int r = t->RowAt(x, y, CurCount());
            if (r < 0) return 0;
            t->selKey = RowKey(r);
            Invalidate();
            POINT pt = { x, y };
            ClientToScreen(hwnd, &pt);
            ContextMenuAt(pt);
            return 0;
        }
        case WM_TASKRESULT: {
            std::unique_ptr<std::wstring> text((std::wstring*)lp);
            if (wp == 1) { // servicio
                if (text) MessageBoxW(hwnd, text->c_str(), L"Servicios", MB_ICONWARNING);
                LoadServices(false);
                Invalidate();
            } else if (wp == 2) { // liberar RAM
                freeRamMsg = text ? *text : L"";
                SampleSystem();
                Invalidate();
            } else if (wp == 3) { // diálogo cerrado
                dialog.reset();
            }
            return 0;
        }
        case WM_GETMINMAXINFO: {
            auto* mm = (MINMAXINFO*)lp;
            mm->ptMinTrackSize = { S(760), S(480) };
            return 0;
        }
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
};

} // namespace

int RunTaskManagerMode() {
    // Una sola instancia: si ya está abierto, traerlo al frente.
    HANDLE mutex = CreateMutexW(nullptr, FALSE, L"Local\\OptimizedShell.TaskMgr");
    if (GetLastError() == ERROR_ALREADY_EXISTS && !ui::HasArg(L"--tab")) {
        if (HWND w = FindWindowW(L"OShellCppTaskMgr", nullptr)) {
            if (IsIconic(w)) ShowWindow(w, SW_RESTORE);
            SetForegroundWindow(w);
            CloseHandle(mutex);
            return 0;
        }
    }

    CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    ui::InitUi(GetModuleHandleW(nullptr));
    int tab = TabProc;
    std::wstring t = ui::ToLower(ui::ArgValue(L"--tab"));
    if (t == L"rendimiento" || t == L"performance") tab = TabPerf;
    else if (t == L"inicio" || t == L"startup") tab = TabStartup;
    else if (t == L"servicios" || t == L"services") tab = TabServices;

    {
        TaskManagerWindow wnd;
        if (!wnd.Start(tab)) {
            MessageBoxW(nullptr, L"No se pudo abrir el Administrador de tareas.", L"OptimizedShell", MB_ICONERROR | MB_OK);
        } else {
            ui::RunMessageLoop();
        }
    }
    ui::ShutdownUi();
    CoUninitialize();
    if (mutex) CloseHandle(mutex);
    return 0;
}
