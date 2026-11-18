// explorer.cpp - Explorador de archivos ligero de OptimizedShell.
// Se ejecuta con: OptimizedShellCpp.exe --explorer [ruta] | --explorer --select <archivo>
//
// Para usar poca CPU/RAM:
//  - Un solo proceso para todas las ventanas (las siguientes llamadas le pasan la ruta).
//  - Las carpetas se leen en un hilo aparte y llegan por lotes: la UI nunca se congela.
//  - Lista virtualizada: solo se dibujan (y solo piden icono) las filas visibles.
//  - Iconos de la lista de imágenes del sistema, en caché por índice/extensión.
//  - La carpeta abierta se vigila con ReadDirectoryChangesW y se actualiza por diferencias.
//  - Copiar/mover/eliminar con IFileOperation en un hilo aparte (diálogo estándar).
//  - El menú contextual nativo (extensiones de shell) solo se carga si se pide.
#include "ui.h"

#include <shobjidl.h>
#include <oleidl.h>
#include <thread>
#include <mutex>
#include <condition_variable>
#include <deque>
#include <atomic>
#include <memory>
#include <unordered_map>

int RunExplorerMode();

namespace {

using namespace Gdiplus;
using namespace ui;

const UINT WM_ENUMBATCH = WM_APP + 40;  // wParam = generación, lParam = std::vector<FileItem>*
const UINT WM_ENUMDONE = WM_APP + 41;   // wParam = generación, lParam = código de error
const UINT WM_ICONREADY = WM_APP + 42;  // wParam = generación, lParam = IconReady*
const UINT WM_DIRCHANGES = WM_APP + 43; // wParam = generación, lParam = std::vector<DirChange>*
const UINT WM_OPDONE = WM_APP + 44;
const wchar_t* kHostClass = L"OShellCppExplorerHost";
const ULONG_PTR kCopyDataId = 0x4F53;

struct FileItem {
    std::wstring name;    // nombre del archivo (o etiqueta de la unidad en "Este equipo")
    std::wstring path;    // solo unidades: "C:\"
    DWORD attrs = 0;
    ULONGLONG size = 0;
    FILETIME mtime = {};
    int icon = -1;        // índice de la lista de imágenes del sistema; -2 = pendiente
    bool requested = false;
    bool selected = false;
    ULONGLONG total = 0, freeBytes = 0;
    UINT driveType = 0;
    bool IsDir() const { return (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0; }
};

struct DirChange {
    DWORD action;
    std::wstring name;
};

struct IconReady {
    std::wstring name;
    int index;
};

// ---------------------------------------------------------------- utilidades
std::wstring JoinPath(const std::wstring& dir, const std::wstring& name) {
    if (dir.empty()) return name;
    return dir.back() == L'\\' ? dir + name : dir + L"\\" + name;
}

std::wstring NormalizeDir(std::wstring p) {
    p = Trim(p);
    if (p.size() >= 2 && p.front() == L'"' && p.back() == L'"') p = p.substr(1, p.size() - 2);
    if (p.empty()) return p;
    p = ExpandEnv(p);
    std::replace(p.begin(), p.end(), L'/', L'\\');
    wchar_t full[MAX_PATH * 2] = {};
    if (GetFullPathNameW(p.c_str(), MAX_PATH * 2, full, nullptr)) p = full;
    while (p.size() > 3 && p.back() == L'\\') p.pop_back();
    if (p.size() == 2 && p[1] == L':') p += L"\\";
    return p;
}

std::wstring ExtOf(const std::wstring& name) {
    size_t dot = name.find_last_of(L'.');
    if (dot == std::wstring::npos || dot == 0) return L"";
    return ToLower(name.substr(dot));
}

bool NeedsOwnIcon(const std::wstring& ext) {
    static const wchar_t* list[] = { L".exe", L".lnk", L".ico", L".url", L".cur", L".ani",
                                     L".msc", L".appref-ms", L".scr", L".cpl" };
    for (auto* e : list)
        if (ext == e) return true;
    return false;
}

std::wstring FormatTime(const FILETIME& ft) {
    if (!ft.dwHighDateTime && !ft.dwLowDateTime) return L"";
    FILETIME local;
    SYSTEMTIME st;
    if (!FileTimeToLocalFileTime(&ft, &local) || !FileTimeToSystemTime(&local, &st)) return L"";
    wchar_t d[64] = {}, t[64] = {};
    GetDateFormatEx(LOCALE_NAME_USER_DEFAULT, DATE_SHORTDATE, &st, nullptr, d, 64, nullptr);
    GetTimeFormatEx(LOCALE_NAME_USER_DEFAULT, TIME_NOSECONDS, &st, nullptr, t, 64);
    return std::wstring(d) + L" " + t;
}

std::wstring ErrorText(DWORD err) {
    wchar_t* buf = nullptr;
    FormatMessageW(FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
                   nullptr, err, 0, (LPWSTR)&buf, 0, nullptr);
    std::wstring s = buf ? Trim(buf) : L"Error " + std::to_wstring(err);
    if (buf) LocalFree(buf);
    return s;
}

bool IsDirectory(const std::wstring& p) {
    DWORD a = GetFileAttributesW(p.c_str());
    return a != INVALID_FILE_ATTRIBUTES && (a & FILE_ATTRIBUTE_DIRECTORY);
}

// Cachés del hilo de UI: icono y nombre de tipo por extensión.
int FolderIconIndex() {
    static int idx = SysIconIndex(L"folder", FILE_ATTRIBUTE_DIRECTORY, true);
    return idx;
}

int ExtIconIndex(const std::wstring& ext) {
    static std::unordered_map<std::wstring, int> cache;
    auto it = cache.find(ext);
    if (it != cache.end()) return it->second;
    int idx = SysIconIndex(L"file" + ext, FILE_ATTRIBUTE_NORMAL, true);
    cache[ext] = idx;
    return idx;
}

const std::wstring& TypeName(const FileItem& it) {
    static std::unordered_map<std::wstring, std::wstring> cache;
    std::wstring key = it.IsDir() ? L"<dir>" : ExtOf(it.name);
    auto f = cache.find(key);
    if (f != cache.end()) return f->second;
    SHFILEINFOW sfi = {};
    std::wstring probe = it.IsDir() ? L"folder" : L"file" + key;
    SHGetFileInfoW(probe.c_str(), it.IsDir() ? FILE_ATTRIBUTE_DIRECTORY : FILE_ATTRIBUTE_NORMAL, &sfi, sizeof(sfi),
                   SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES);
    std::wstring name = sfi.szTypeName;
    if (name.empty()) name = key.empty() ? L"Archivo" : L"Archivo " + key.substr(1);
    return cache[key] = name;
}

const wchar_t* DriveTypeName(UINT t) {
    switch (t) {
    case DRIVE_FIXED: return L"Disco local";
    case DRIVE_REMOVABLE: return L"Unidad extraíble";
    case DRIVE_REMOTE: return L"Unidad de red";
    case DRIVE_CDROM: return L"Unidad de CD/DVD";
    case DRIVE_RAMDISK: return L"Disco RAM";
    default: return L"Unidad";
    }
}

// ---------------------------------------------------------------- portapapeles
bool SetClipboardFiles(HWND owner, const std::vector<std::wstring>& files, DWORD effect) {
    if (files.empty()) return false;
    size_t chars = 1;
    for (auto& f : files) chars += f.size() + 1;
    HGLOBAL h = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, sizeof(DROPFILES) + chars * sizeof(wchar_t));
    HGLOBAL he = GlobalAlloc(GMEM_MOVEABLE, sizeof(DWORD));
    if (!h || !he) { if (h) GlobalFree(h); if (he) GlobalFree(he); return false; }
    auto* df = (DROPFILES*)GlobalLock(h);
    df->pFiles = sizeof(DROPFILES);
    df->fWide = TRUE;
    wchar_t* p = (wchar_t*)(df + 1);
    for (auto& f : files) {
        memcpy(p, f.c_str(), f.size() * sizeof(wchar_t));
        p += f.size() + 1;
    }
    GlobalUnlock(h);
    *(DWORD*)GlobalLock(he) = effect;
    GlobalUnlock(he);
    if (!OpenClipboard(owner)) { GlobalFree(h); GlobalFree(he); return false; }
    EmptyClipboard();
    SetClipboardData(CF_HDROP, h);
    SetClipboardData(RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT), he);
    CloseClipboard();
    return true;
}

std::vector<std::wstring> FilesFromHDrop(HDROP drop) {
    std::vector<std::wstring> out;
    UINT n = DragQueryFileW(drop, 0xFFFFFFFF, nullptr, 0);
    for (UINT i = 0; i < n; ++i) {
        UINT len = DragQueryFileW(drop, i, nullptr, 0);
        std::wstring s(len + 1, L'\0');
        DragQueryFileW(drop, i, &s[0], len + 1);
        s.resize(len);
        out.push_back(s);
    }
    return out;
}

bool GetClipboardFiles(HWND owner, std::vector<std::wstring>& files, DWORD& effect) {
    effect = DROPEFFECT_COPY;
    if (!IsClipboardFormatAvailable(CF_HDROP) || !OpenClipboard(owner)) return false;
    if (HANDLE h = GetClipboardData(CF_HDROP)) files = FilesFromHDrop((HDROP)h);
    if (HANDLE he = GetClipboardData(RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT))) {
        if (DWORD* e = (DWORD*)GlobalLock(he)) {
            effect = *e;
            GlobalUnlock(he);
        }
    }
    CloseClipboard();
    return !files.empty();
}

// ---------------------------------------------------------------- operaciones de archivo
std::atomic<int> g_activeOps{ 0 };
HWND g_host = nullptr;

enum class FileOp { Copy, Move, Recycle, Delete };

// IFileOperation en su propio hilo: muestra el progreso y los conflictos estándar
// de Windows y admite deshacer, sin bloquear la ventana.
void RunFileOp(FileOp op, std::vector<std::wstring> sources, std::wstring dest, HWND owner) {
    if (sources.empty()) return;
    ++g_activeOps;
    std::thread([op, sources, dest, owner] {
        if (SUCCEEDED(CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED))) {
            IFileOperation* fo = nullptr;
            if (SUCCEEDED(CoCreateInstance(CLSID_FileOperation, nullptr, CLSCTX_ALL, IID_PPV_ARGS(&fo)))) {
                DWORD flags = FOF_NOCONFIRMMKDIR;
                if (op == FileOp::Recycle) flags |= FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION;
                if (op == FileOp::Copy || op == FileOp::Move) {
                    flags |= FOF_ALLOWUNDO;
                    for (auto& s : sources)
                        if (_wcsicmp(ParentDir(s).c_str(), dest.c_str()) == 0) flags |= FOF_RENAMEONCOLLISION;
                }
                fo->SetOperationFlags(flags);
                fo->SetOwnerWindow(owner);
                std::vector<PIDLIST_ABSOLUTE> pidls;
                for (auto& s : sources)
                    if (PIDLIST_ABSOLUTE p = ILCreateFromPathW(s.c_str())) pidls.push_back(p);
                IShellItemArray* arr = nullptr;
                if (!pidls.empty() &&
                    SUCCEEDED(SHCreateShellItemArrayFromIDLists((UINT)pidls.size(), (PCIDLIST_ABSOLUTE_ARRAY)pidls.data(), &arr))) {
                    HRESULT hr = E_FAIL;
                    if (op == FileOp::Copy || op == FileOp::Move) {
                        IShellItem* d = nullptr;
                        if (SUCCEEDED(SHCreateItemFromParsingName(dest.c_str(), nullptr, IID_PPV_ARGS(&d)))) {
                            hr = op == FileOp::Copy ? fo->CopyItems(arr, d) : fo->MoveItems(arr, d);
                            d->Release();
                        }
                    } else {
                        hr = fo->DeleteItems(arr);
                    }
                    if (SUCCEEDED(hr)) fo->PerformOperations();
                    arr->Release();
                }
                for (auto p : pidls) ILFree(p);
                fo->Release();
            }
            CoUninitialize();
        }
        --g_activeOps;
        if (g_host) PostMessageW(g_host, WM_OPDONE, 0, 0);
    }).detach();
}

// IShellFolder del directorio + PIDL relativos de los nombres indicados.
struct ChildItems {
    IShellFolder* folder = nullptr;
    std::vector<PITEMID_CHILD> children;
    ~ChildItems() {
        for (auto c : children) CoTaskMemFree(c);
        if (folder) folder->Release();
    }
    bool Load(HWND hwnd, const std::wstring& dir, const std::vector<std::wstring>& names) {
        PIDLIST_ABSOLUTE pidl = ILCreateFromPathW(dir.c_str());
        if (!pidl) return false;
        IShellFolder* desktop = nullptr;
        bool ok = SUCCEEDED(SHGetDesktopFolder(&desktop)) &&
                  SUCCEEDED(desktop->BindToObject(pidl, nullptr, IID_PPV_ARGS(&folder)));
        if (desktop) desktop->Release();
        ILFree(pidl);
        if (!ok) return false;
        for (auto& n : names) {
            PIDLIST_RELATIVE child = nullptr;
            if (SUCCEEDED(folder->ParseDisplayName(hwnd, nullptr, (LPWSTR)n.c_str(), nullptr, &child, nullptr)))
                children.push_back((PITEMID_CHILD)child);
        }
        return !children.empty();
    }
};

// ---------------------------------------------------------------- hilo de iconos
// Un único hilo para todo el proceso. Resuelve los iconos que dependen del
// propio archivo (exe, lnk, ico...) solo para las filas que se ven.
class IconWorker {
public:
    static IconWorker& Get() {
        static IconWorker w;
        return w;
    }

    void Request(HWND target, UINT gen, const std::wstring& path, const std::wstring& name) {
        {
            std::lock_guard<std::mutex> lk(mu);
            // Descartar peticiones de carpetas que ya no se están mostrando.
            jobs.erase(std::remove_if(jobs.begin(), jobs.end(),
                                      [&](const Job& j) { return j.target == target && j.gen != gen; }),
                       jobs.end());
            jobs.push_back({ target, gen, path, name });
            if (!th.joinable()) th = std::thread([this] { Loop(); });
        }
        cv.notify_one();
    }

    void Stop() {
        {
            std::lock_guard<std::mutex> lk(mu);
            stop = true;
        }
        cv.notify_all();
        if (th.joinable()) th.join();
    }

private:
    struct Job {
        HWND target;
        UINT gen;
        std::wstring path, name;
    };
    std::thread th;
    std::mutex mu;
    std::condition_variable cv;
    std::deque<Job> jobs;
    bool stop = false;

    void Loop() {
        CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
        SetThreadEco();
        for (;;) {
            Job j;
            {
                std::unique_lock<std::mutex> lk(mu);
                cv.wait(lk, [this] { return stop || !jobs.empty(); });
                if (stop) break;
                j = jobs.front();
                jobs.pop_front();
            }
            auto* r = new IconReady{ j.name, SysIconIndex(j.path) };
            if (!PostMessageW(j.target, WM_ICONREADY, j.gen, (LPARAM)r)) delete r;
        }
        CoUninitialize();
    }
};

// ---------------------------------------------------------------- vigilancia de carpeta
class DirWatcher {
public:
    ~DirWatcher() { Stop(); }

    void Start(const std::wstring& dir, HWND target, UINT gen) {
        Stop();
        HANDLE h = CreateFileW(dir.c_str(), FILE_LIST_DIRECTORY, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                               nullptr, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED, nullptr);
        if (h == INVALID_HANDLE_VALUE) return;
        dirHandle = h;
        stopEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        th = std::thread([this, target, gen] { Loop(target, gen); });
    }

    void Stop() {
        if (stopEvent) SetEvent(stopEvent);
        if (th.joinable()) th.join();
        if (dirHandle) { CloseHandle(dirHandle); dirHandle = nullptr; }
        if (stopEvent) { CloseHandle(stopEvent); stopEvent = nullptr; }
    }

private:
    HANDLE dirHandle = nullptr;
    HANDLE stopEvent = nullptr;
    std::thread th;

    void Loop(HWND target, UINT gen) {
        std::vector<DWORD> buf(16 * 1024);
        OVERLAPPED ov = {};
        ov.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        for (;;) {
            ResetEvent(ov.hEvent);
            if (!ReadDirectoryChangesW(dirHandle, buf.data(), (DWORD)(buf.size() * sizeof(DWORD)), FALSE,
                                       FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME |
                                           FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE |
                                           FILE_NOTIFY_CHANGE_ATTRIBUTES,
                                       nullptr, &ov, nullptr))
                break;
            HANDLE waits[2] = { ov.hEvent, stopEvent };
            DWORD r = WaitForMultipleObjects(2, waits, FALSE, INFINITE);
            if (r != WAIT_OBJECT_0) {
                CancelIoEx(dirHandle, &ov);
                DWORD ignored = 0;
                GetOverlappedResult(dirHandle, &ov, &ignored, TRUE);
                break;
            }
            DWORD bytes = 0;
            if (!GetOverlappedResult(dirHandle, &ov, &bytes, FALSE)) break;
            auto* changes = new std::vector<DirChange>();
            if (bytes == 0) {
                changes->push_back({ 0, L"" }); // desbordamiento: releer la carpeta
            } else {
                BYTE* p = (BYTE*)buf.data();
                for (;;) {
                    auto* fni = (FILE_NOTIFY_INFORMATION*)p;
                    changes->push_back({ fni->Action, std::wstring(fni->FileName, fni->FileNameLength / sizeof(wchar_t)) });
                    if (!fni->NextEntryOffset) break;
                    p += fni->NextEntryOffset;
                }
            }
            if (!PostMessageW(target, WM_DIRCHANGES, gen, (LPARAM)changes)) delete changes;
        }
        CloseHandle(ov.hEvent);
    }
};

// ---------------------------------------------------------------- ventana
class ExplorerWindow;
std::vector<ExplorerWindow*> g_windows;

void MaybeQuit() {
    if (g_windows.empty() && g_activeOps == 0) PostQuitMessage(0);
}

class DropTarget;

enum ButtonId {
    BNone = 0,
    BMin = 1, BMax, BClose,
    BBack = 10, BFwd, BUp, BRefresh,
    BNewFolder = 20, BCut, BCopy, BPaste, BRename, BDelete, BProps, BSort, BHidden, BTerminal,
};

struct NavEntry {
    std::wstring label;
    std::wstring path;
    int icon = -1;
    bool header = false;
    RectF rc;
};

class ExplorerWindow : public WindowBase, public EditKeySink {
public:
    static ExplorerWindow* Open(const std::wstring& path, const std::wstring& selectName = L"") {
        auto* w = new ExplorerWindow();
        if (!w->Start(path, selectName)) {
            delete w;
            return nullptr;
        }
        return w;
    }

    // Interfaz para DropTarget
    std::wstring DropFolderAt(int x, int y) {
        if (cur.empty()) return L"";
        int vi = RowAt(x, y);
        if (vi >= 0 && items[view[vi]].IsDir()) return JoinPath(cur, items[view[vi]].name);
        return InRect(listRc, x, y) || InRect(headerRc, x, y) ? cur : L"";
    }
    void SetDropHighlight(int x, int y) {
        int vi = (x < 0) ? -1 : RowAt(x, y);
        if (vi >= 0 && !items[view[vi]].IsDir()) vi = -1;
        if (vi != dropRow) { dropRow = vi; InvalidateRectF(listRc); }
    }
    void DropFiles(const std::vector<std::wstring>& files, const std::wstring& dest, DWORD effect) {
        RunFileOp(effect & DROPEFFECT_MOVE ? FileOp::Move : FileOp::Copy, files, dest, hwnd);
    }

    bool OnEditKey(HWND edit, int vk) override {
        if (edit == renameEdit) {
            if (vk == VK_RETURN || vk == 0) { CommitRename(); return vk != 0; }
            if (vk == VK_ESCAPE) { CancelRename(); return true; }
            return false;
        }
        if (edit == addr) {
            if (vk == VK_RETURN) {
                std::wstring typed = Trim(GetWindowString(addr));
                SetFocus(hwnd);
                GoToTyped(typed);
                return true;
            }
            if (vk == VK_ESCAPE) { UpdateAddress(); SetFocus(hwnd); return true; }
            if (vk == 0) UpdateAddress();
            return false;
        }
        if (edit == search) {
            if (vk == VK_ESCAPE) {
                if (GetWindowTextLengthW(search)) SetWindowTextW(search, L"");
                else SetFocus(hwnd);
                return true;
            }
            if (vk == VK_RETURN || vk == VK_DOWN) {
                SetFocus(hwnd);
                if (!view.empty() && FocusIndex() < 0) SetFocusRow(0, false, false);
                return true;
            }
        }
        return false;
    }

    ~ExplorerWindow() override;

protected:
    std::wstring cur;                       // "" = Este equipo
    std::vector<std::wstring> backStack, fwdStack;
    std::vector<FileItem> items;
    std::vector<int> view;                  // índices en items, filtrados y ordenados
    std::wstring focusName, anchorName, pendingSelect;
    UINT gen = 0;
    bool loading = false;
    DWORD loadError = 0;
    bool showHidden = false;
    int sortCol = 0;
    bool sortDesc = false;
    std::wstring filter;
    int scroll = 0;
    DirWatcher watcher;

    HWND addr = nullptr, search = nullptr, renameEdit = nullptr;
    std::wstring renaming;
    bool committing = false;

    std::vector<NavEntry> nav;
    int hoverBtn = BNone, hoverRow = -1, hoverNav = -1, hoverCol = -1;
    int dropRow = -1;
    HoverTip tip;

    std::wstring typeBuffer;
    ULONGLONG typeTick = 0;

    bool mouseDownOnSel = false;
    POINT downPt = {};

    IContextMenu2* cm2 = nullptr;
    IContextMenu3* cm3 = nullptr;
    DropTarget* dropTarget = nullptr;
    HICON iconBig = nullptr, iconSmall = nullptr;

    // Layout
    RectF btns[64];
    RectF addrRc, searchRc, navRc, headerRc, listRc, statusRc;
    float colX[5] = {}; // inicio de columnas: nombre, fecha, tipo, tamaño, fin

    static constexpr int kCaption = 42;
    int RowH() const { return S(28); }
    int RowsVisible() const { return std::max(1, (int)(listRc.Height / RowH())); }

    // ------------------------------------------------------------ creación
    bool Start(const std::wstring& path, const std::wstring& selectName);

    void Layout(int w, int h) {
        auto chrome = style::ChromeRects(w);
        btns[BMin] = chrome.min;
        btns[BMax] = chrome.max;
        btns[BClose] = chrome.close;
        float y0 = (float)S(kCaption) + S(6);
        float bs = (float)S(34);
        for (int i = 0; i < 4; ++i) btns[BBack + i] = RectF((float)S(8) + i * (bs + S(4)), y0, bs, bs);
        float ax = btns[BRefresh].X + bs + S(8);
        float sw = std::min((float)S(260), w * 0.28f);
        searchRc = RectF((float)w - S(10) - sw, y0, sw, bs);
        addrRc = RectF(ax, y0, searchRc.X - S(8) - ax, bs);

        float y1 = y0 + bs + S(6);
        float x = (float)S(8);
        auto place = [&](int id, float width) {
            btns[id] = RectF(x, y1, width, (float)S(32));
            x += width + S(2);
        };
        place(BNewFolder, (float)S(132));
        x += S(6);
        for (int id : { BCut, BCopy, BPaste, BRename, BDelete, BProps }) place(id, (float)S(36));
        x += S(6);
        place(BTerminal, (float)S(36));
        float rx = (float)w - S(10);
        btns[BHidden] = RectF(rx - S(132), y1, (float)S(132), (float)S(32));
        btns[BSort] = RectF(btns[BHidden].X - S(4) - S(112), y1, (float)S(112), (float)S(32));

        float top = y1 + S(32) + S(8);
        float statusH = (float)S(28);
        navRc = RectF(0, top, (float)S(220), h - top - statusH);
        float lx = navRc.Width + 1;
        headerRc = RectF(lx, top, w - lx, (float)S(30));
        listRc = RectF(lx, top + S(30), w - lx - S(10), h - top - S(30) - statusH);
        statusRc = RectF(0, h - statusH, (float)w, statusH);

        float sizeW = (float)S(100), typeW = (float)S(160), dateW = (float)S(150);
        colX[0] = lx + S(8);
        colX[4] = listRc.X + listRc.Width;
        colX[3] = colX[4] - sizeW;
        colX[2] = colX[3] - typeW;
        colX[1] = colX[2] - dateW;
        if (colX[1] - colX[0] < S(160)) { // ventana estrecha: solo nombre y tamaño
            colX[1] = colX[2] = colX[3];
        }

        // Panel de navegación
        float ny = navRc.Y + S(8);
        for (auto& e : nav) {
            float eh = e.header ? (float)S(28) : (float)S(30);
            e.rc = RectF((float)S(6), ny, navRc.Width - S(12), eh);
            ny += eh;
        }
    }

    void PositionEdits() {
        if (addr)
            MoveWindow(addr, (int)addrRc.X + S(10), (int)addrRc.Y + S(8), (int)addrRc.Width - S(20), (int)addrRc.Height - S(14), TRUE);
        if (search)
            MoveWindow(search, (int)searchRc.X + S(32), (int)searchRc.Y + S(8), (int)searchRc.Width - S(40),
                       (int)searchRc.Height - S(14), TRUE);
    }

    void OnSize(UINT type, int w, int h) override {
        if (type == SIZE_MINIMIZED) { TrimWorkingSet(); return; }
        CancelRename();
        Layout(w, h);
        PositionEdits();
        ClampScroll();
    }

    void BuildNav() {
        nav.clear();
        nav.push_back({ L"Acceso rápido", L"", -1, true });
        struct KF { const KNOWNFOLDERID* id; const wchar_t* label; };
        KF kfs[] = {
            { &FOLDERID_Desktop, L"Escritorio" }, { &FOLDERID_Downloads, L"Descargas" },
            { &FOLDERID_Documents, L"Documentos" }, { &FOLDERID_Pictures, L"Imágenes" },
            { &FOLDERID_Music, L"Música" }, { &FOLDERID_Videos, L"Vídeos" },
        };
        for (auto& k : kfs) {
            PWSTR p = nullptr;
            if (SUCCEEDED(SHGetKnownFolderPath(*k.id, 0, nullptr, &p))) {
                nav.push_back({ k.label, p, SysIconIndex(p), false });
                CoTaskMemFree(p);
            }
        }
        nav.push_back({ L"Este equipo", L"", -1, true });
        nav.push_back({ L"Este equipo", L"::thispc", SysIconIndex(L"C:\\"), false });
        DWORD mask = GetLogicalDrives();
        for (int i = 0; i < 26; ++i) {
            if (!(mask & (1u << i))) continue;
            std::wstring root = std::wstring(1, (wchar_t)(L'A' + i)) + L":\\";
            UINT type = GetDriveTypeW(root.c_str());
            nav.push_back({ DriveLabel(root, type), root,
                            (type == DRIVE_FIXED || type == DRIVE_REMOVABLE) ? SysIconIndex(root)
                                                                             : SysIconIndex(root, FILE_ATTRIBUTE_DIRECTORY, true),
                            false });
        }
        RECT rc;
        GetClientRect(hwnd, &rc);
        Layout(rc.right, rc.bottom);
    }

    static std::wstring DriveLabel(const std::wstring& root, UINT type) {
        wchar_t label[MAX_PATH + 1] = {};
        // Solo se consulta el volumen en unidades locales: las de red o un lector
        // vacío pueden tardar segundos en responder.
        if (type == DRIVE_FIXED || type == DRIVE_REMOVABLE || type == DRIVE_RAMDISK)
            GetVolumeInformationW(root.c_str(), label, MAX_PATH + 1, nullptr, nullptr, nullptr, nullptr, 0);
        std::wstring name = label[0] ? label : DriveTypeName(type);
        return name + L" (" + root.substr(0, 2) + L")";
    }

    // ------------------------------------------------------------ navegación
    std::wstring DisplayPath() const { return cur.empty() ? L"Este equipo" : cur; }

    void UpdateAddress() {
        if (addr) SetWindowTextW(addr, DisplayPath().c_str());
        std::wstring title = cur.empty() ? L"Este equipo" : BaseName(cur);
        if (title.empty()) title = cur;
        SetWindowTextW(hwnd, title.c_str());
    }

    bool Navigate(std::wstring path, bool addHistory = true, const std::wstring& select = L"") {
        if (path == L"::thispc") path.clear();
        if (!path.empty()) {
            path = NormalizeDir(path);
            DWORD a = GetFileAttributesW(path.c_str());
            if (a == INVALID_FILE_ATTRIBUTES || !(a & FILE_ATTRIBUTE_DIRECTORY)) {
                ui::MsgBox(hwnd, (L"No se puede acceder a \"" + path + L"\".\n\n" + ErrorText(GetLastError())).c_str(),
                            L"Archivos", MB_ICONWARNING);
                return false;
            }
        }
        CancelRename();
        if (addHistory && path != cur) {
            backStack.push_back(cur);
            fwdStack.clear();
        }
        cur = path;
        filter.clear();
        if (search) SetWindowTextW(search, L"");
        scroll = 0;
        focusName.clear();
        anchorName.clear();
        pendingSelect = select;
        UpdateAddress();
        Load();
        return true;
    }

    void GoToTyped(const std::wstring& typed) {
        if (typed.empty() || _wcsicmp(typed.c_str(), L"Este equipo") == 0) { Navigate(L""); return; }
        std::wstring p = NormalizeDir(typed);
        DWORD a = GetFileAttributesW(p.c_str());
        if (a != INVALID_FILE_ATTRIBUTES && (a & FILE_ATTRIBUTE_DIRECTORY)) { Navigate(p); return; }
        if (a != INVALID_FILE_ATTRIBUTES) { Launch(p); UpdateAddress(); return; }
        // shell:downloads, URLs, comandos...
        if (!RunCommandLine(typed, hwnd))
            ui::MsgBox(hwnd, (L"No se encuentra \"" + typed + L"\".").c_str(), L"Archivos", MB_ICONWARNING);
        UpdateAddress();
    }

    void GoBack() {
        if (backStack.empty()) return;
        fwdStack.push_back(cur);
        std::wstring prev = cur;
        std::wstring target = backStack.back();
        backStack.pop_back();
        if (!Navigate(target, false, BaseName(prev))) fwdStack.pop_back();
    }

    void GoForward() {
        if (fwdStack.empty()) return;
        backStack.push_back(cur);
        std::wstring target = fwdStack.back();
        fwdStack.pop_back();
        if (!Navigate(target, false)) backStack.pop_back();
    }

    void GoUp() {
        if (cur.empty()) return;
        std::wstring child = cur;
        std::wstring parent = (cur.size() <= 3) ? L"" : ParentDir(cur);
        Navigate(parent, true, cur.size() <= 3 ? DriveLabel(cur, GetDriveTypeW(cur.c_str())) : BaseName(child));
    }

    void Load() {
        ++gen;
        watcher.Stop();
        items.clear();
        view.clear();
        loadError = 0;
        dropRow = hoverRow = -1;
        if (cur.empty()) {
            LoadDrives();
        } else {
            loading = true;
            StartEnum();
            watcher.Start(cur, hwnd, gen);
        }
        Invalidate();
    }

    void LoadDrives() {
        DWORD mask = GetLogicalDrives();
        for (int i = 0; i < 26; ++i) {
            if (!(mask & (1u << i))) continue;
            FileItem it;
            it.path = std::wstring(1, (wchar_t)(L'A' + i)) + L":\\";
            it.driveType = GetDriveTypeW(it.path.c_str());
            it.name = DriveLabel(it.path, it.driveType);
            it.attrs = FILE_ATTRIBUTE_DIRECTORY;
            bool local = it.driveType == DRIVE_FIXED || it.driveType == DRIVE_REMOVABLE || it.driveType == DRIVE_RAMDISK;
            if (local) {
                ULARGE_INTEGER freeB, total;
                if (GetDiskFreeSpaceExW(it.path.c_str(), &freeB, &total, nullptr)) {
                    it.freeBytes = freeB.QuadPart;
                    it.total = total.QuadPart;
                }
                it.icon = SysIconIndex(it.path);
            } else {
                it.icon = SysIconIndex(it.path, FILE_ATTRIBUTE_DIRECTORY, true);
            }
            items.push_back(it);
        }
        loading = false;
        RebuildView();
        ApplyPendingSelect();
    }

    void StartEnum() {
        std::wstring dir = cur;
        UINT g = gen;
        HWND target = hwnd;
        std::thread([dir, g, target] {
            WIN32_FIND_DATAW fd;
            HANDLE h = FindFirstFileExW(JoinPath(dir, L"*").c_str(), FindExInfoBasic, &fd, FindExSearchNameMatch,
                                        nullptr, FIND_FIRST_EX_LARGE_FETCH);
            if (h == INVALID_HANDLE_VALUE) {
                PostMessageW(target, WM_ENUMDONE, g, GetLastError());
                return;
            }
            auto* batch = new std::vector<FileItem>();
            ULONGLONG last = GetTickCount64();
            do {
                if (fd.cFileName[0] == L'.' && (!fd.cFileName[1] || (fd.cFileName[1] == L'.' && !fd.cFileName[2]))) continue;
                FileItem it;
                it.name = fd.cFileName;
                it.attrs = fd.dwFileAttributes;
                it.size = ((ULONGLONG)fd.nFileSizeHigh << 32) | fd.nFileSizeLow;
                it.mtime = fd.ftLastWriteTime;
                batch->push_back(std::move(it));
                ULONGLONG now = GetTickCount64();
                if (batch->size() >= 4000 || now - last > 120) {
                    if (!PostMessageW(target, WM_ENUMBATCH, g, (LPARAM)batch)) { delete batch; FindClose(h); return; }
                    batch = new std::vector<FileItem>();
                    last = now;
                }
            } while (FindNextFileW(h, &fd));
            FindClose(h);
            if (batch->empty() || !PostMessageW(target, WM_ENUMBATCH, g, (LPARAM)batch)) delete batch;
            PostMessageW(target, WM_ENUMDONE, g, 0);
        }).detach();
    }

    void ResolveIconFast(FileItem& it) {
        if (it.IsDir()) { it.icon = FolderIconIndex(); return; }
        std::wstring ext = ExtOf(it.name);
        it.icon = NeedsOwnIcon(ext) ? -2 : ExtIconIndex(ext);
    }

    // ------------------------------------------------------------ vista
    bool Visible(const FileItem& it, const std::wstring& q) const {
        if (!showHidden && (it.attrs & FILE_ATTRIBUTE_HIDDEN) && !cur.empty()) return false;
        return q.empty() || ToLower(it.name).find(q) != std::wstring::npos;
    }

    void RebuildView() {
        std::wstring q = ToLower(filter);
        view.clear();
        view.reserve(items.size());
        for (int i = 0; i < (int)items.size(); ++i) {
            if (Visible(items[i], q)) view.push_back(i);
            else items[i].selected = false;
        }
        bool drives = cur.empty();
        std::sort(view.begin(), view.end(), [&](int a, int b) {
            const FileItem& x = items[a];
            const FileItem& y = items[b];
            if (!drives && x.IsDir() != y.IsDir()) return x.IsDir();
            int c = 0;
            switch (sortCol) {
            case 1: c = drives ? (x.freeBytes < y.freeBytes ? -1 : x.freeBytes > y.freeBytes)
                               : CompareFileTime(&x.mtime, &y.mtime); break;
            case 2: c = drives ? (int)x.driveType - (int)y.driveType : _wcsicmp(TypeName(x).c_str(), TypeName(y).c_str()); break;
            case 3: { ULONGLONG xs = drives ? x.total : x.size, ys = drives ? y.total : y.size; c = xs < ys ? -1 : xs > ys; break; }
            }
            if (c == 0) c = StrCmpLogicalW(x.name.c_str(), y.name.c_str());
            return sortDesc ? c > 0 : c < 0;
        });
        ClampScroll();
        Invalidate();
    }

    void ClampScroll() {
        int maxOff = std::max(0, (int)view.size() - RowsVisible());
        scroll = std::max(0, std::min(scroll, maxOff));
    }

    int FindView(const std::wstring& name) const {
        if (name.empty()) return -1;
        for (int i = 0; i < (int)view.size(); ++i)
            if (_wcsicmp(items[view[i]].name.c_str(), name.c_str()) == 0) return i;
        return -1;
    }

    int FocusIndex() const { return FindView(focusName); }

    void EnsureVisible(int vi) {
        if (vi < 0) return;
        int vis = RowsVisible();
        if (vi < scroll) scroll = vi;
        else if (vi >= scroll + vis) scroll = vi - vis + 1;
        ClampScroll();
    }

    void ApplyPendingSelect() {
        if (pendingSelect.empty()) return;
        int vi = FindView(pendingSelect);
        if (vi >= 0) {
            pendingSelect.clear();
            SetFocusRow(vi, false, false);
        }
    }

    void ClearSelection() {
        for (auto& it : items) it.selected = false;
    }

    void SetFocusRow(int vi, bool shift, bool ctrl) {
        if (vi < 0 || vi >= (int)view.size()) return;
        if (shift) {
            int a = FindView(anchorName);
            if (a < 0) a = vi;
            if (!ctrl) ClearSelection();
            for (int i = std::min(a, vi); i <= std::max(a, vi); ++i) items[view[i]].selected = true;
        } else if (ctrl) {
            items[view[vi]].selected = !items[view[vi]].selected;
            anchorName = items[view[vi]].name;
        } else {
            ClearSelection();
            items[view[vi]].selected = true;
            anchorName = items[view[vi]].name;
        }
        focusName = items[view[vi]].name;
        EnsureVisible(vi);
        Invalidate();
    }

    std::vector<int> SelectedRows() const {
        std::vector<int> r;
        for (int i = 0; i < (int)view.size(); ++i)
            if (items[view[i]].selected) r.push_back(i);
        return r;
    }

    std::wstring FullPath(const FileItem& it) const { return cur.empty() ? it.path : JoinPath(cur, it.name); }

    std::vector<std::wstring> SelectedPaths() const {
        std::vector<std::wstring> out;
        for (int vi : SelectedRows()) out.push_back(FullPath(items[view[vi]]));
        return out;
    }

    std::vector<std::wstring> SelectedNames() const {
        std::vector<std::wstring> out;
        for (int vi : SelectedRows()) out.push_back(items[view[vi]].name);
        return out;
    }

    // ------------------------------------------------------------ cambios en disco
    void ApplyChanges(const std::vector<DirChange>& changes) {
        for (auto& c : changes)
            if (c.action == 0) { // desbordamiento
                pendingSelect = focusName;
                Load();
                return;
            }
        std::unordered_map<std::wstring, size_t> index;
        index.reserve(items.size());
        for (size_t i = 0; i < items.size(); ++i) index[ToLower(items[i].name)] = i;
        std::vector<bool> removed(items.size(), false);
        bool any = false;
        for (auto& c : changes) {
            if (c.name.find(L'\\') != std::wstring::npos) continue;
            std::wstring key = ToLower(c.name);
            auto it = index.find(key);
            switch (c.action) {
            case FILE_ACTION_REMOVED:
            case FILE_ACTION_RENAMED_OLD_NAME:
                if (it != index.end() && !removed[it->second]) {
                    removed[it->second] = true;
                    index.erase(it);
                    any = true;
                }
                break;
            case FILE_ACTION_ADDED:
            case FILE_ACTION_RENAMED_NEW_NAME:
            case FILE_ACTION_MODIFIED: {
                WIN32_FILE_ATTRIBUTE_DATA fa;
                if (!GetFileAttributesExW(JoinPath(cur, c.name).c_str(), GetFileExInfoStandard, &fa)) break;
                FileItem* target = nullptr;
                if (it != index.end() && !removed[it->second]) {
                    target = &items[it->second];
                } else {
                    if (c.action == FILE_ACTION_MODIFIED) break;
                    items.push_back(FileItem());
                    removed.push_back(false);
                    target = &items.back();
                    target->name = c.name;
                    index[key] = items.size() - 1;
                }
                bool wasDir = target->IsDir();
                target->attrs = fa.dwFileAttributes;
                target->size = ((ULONGLONG)fa.nFileSizeHigh << 32) | fa.nFileSizeLow;
                target->mtime = fa.ftLastWriteTime;
                if (target->icon == -1 || wasDir != target->IsDir()) { target->requested = false; ResolveIconFast(*target); }
                any = true;
                break;
            }
            }
        }
        if (!any) return;
        std::vector<FileItem> kept;
        kept.reserve(items.size());
        for (size_t i = 0; i < items.size(); ++i)
            if (!removed[i]) kept.push_back(std::move(items[i]));
        items.swap(kept);
        RebuildView();
        ApplyPendingSelect();
    }

    // ------------------------------------------------------------ acciones
    void OpenRow(int vi, bool newWindow = false) {
        if (vi < 0 || vi >= (int)view.size()) return;
        const FileItem& it = items[view[vi]];
        std::wstring full = FullPath(it);
        if (it.IsDir()) {
            if (newWindow) Open(full);
            else Navigate(full);
            return;
        }
        if (EndsWithNoCase(it.name, L".lnk")) {
            // Accesos directos a carpetas: navegar aquí en vez de abrir otra ventana.
            std::wstring target = ResolveLink(full);
            if (!target.empty() && IsDirectory(target)) {
                if (newWindow) Open(target);
                else Navigate(target);
                return;
            }
        }
        SHELLEXECUTEINFOW sei = { sizeof(sei) };
        sei.hwnd = hwnd;
        sei.lpFile = full.c_str();
        sei.lpDirectory = cur.c_str();
        sei.nShow = SW_SHOWNORMAL;
        ShellExecuteExW(&sei);
    }

    static std::wstring ResolveLink(const std::wstring& lnk) {
        std::wstring out;
        IShellLinkW* sl = nullptr;
        if (SUCCEEDED(CoCreateInstance(CLSID_ShellLink, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&sl)))) {
            IPersistFile* pf = nullptr;
            if (SUCCEEDED(sl->QueryInterface(IID_PPV_ARGS(&pf)))) {
                if (SUCCEEDED(pf->Load(lnk.c_str(), STGM_READ))) {
                    wchar_t buf[MAX_PATH] = {};
                    if (sl->GetPath(buf, MAX_PATH, nullptr, SLGP_RAWPATH) == S_OK) out = ExpandEnv(buf);
                }
                pf->Release();
            }
            sl->Release();
        }
        return out;
    }

    void OpenSelection() {
        auto rows = SelectedRows();
        if (rows.empty()) { OpenRow(FocusIndex()); return; }
        if (rows.size() == 1) { OpenRow(rows[0]); return; }
        for (int vi : rows)
            if (!items[view[vi]].IsDir()) OpenRow(vi);
    }

    void CopySelection(bool cut) {
        if (cur.empty()) return;
        SetClipboardFiles(hwnd, SelectedPaths(), cut ? DROPEFFECT_MOVE : DROPEFFECT_COPY);
    }

    void Paste() {
        if (cur.empty()) return;
        std::vector<std::wstring> files;
        DWORD effect = DROPEFFECT_COPY;
        if (!GetClipboardFiles(hwnd, files, effect)) return;
        bool move = (effect & DROPEFFECT_MOVE) != 0;
        RunFileOp(move ? FileOp::Move : FileOp::Copy, files, cur, hwnd);
        if (move && OpenClipboard(hwnd)) { EmptyClipboard(); CloseClipboard(); } // cortar se consume al pegar
    }

    void DeleteSelection(bool permanent) {
        auto paths = SelectedPaths();
        if (paths.empty() || cur.empty()) return;
        if (permanent) {
            std::wstring msg = paths.size() == 1 ? L"¿Eliminar permanentemente \"" + BaseName(paths[0]) + L"\"?"
                                                 : L"¿Eliminar permanentemente estos " + std::to_wstring(paths.size()) + L" elementos?";
            if (ui::MsgBox(hwnd, msg.c_str(), L"Eliminar", MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) != IDYES) return;
        }
        RunFileOp(permanent ? FileOp::Delete : FileOp::Recycle, paths, L"", hwnd);
    }

    void NewFolder() {
        if (cur.empty()) return;
        std::wstring base = L"Nueva carpeta", name = base;
        for (int n = 2; GetFileAttributesW(JoinPath(cur, name).c_str()) != INVALID_FILE_ATTRIBUTES; ++n)
            name = base + L" (" + std::to_wstring(n) + L")";
        if (!CreateDirectoryW(JoinPath(cur, name).c_str(), nullptr)) {
            ui::MsgBox(hwnd, ErrorText(GetLastError()).c_str(), L"Nueva carpeta", MB_ICONWARNING);
            return;
        }
        // Añadirla ya (el vigilante también avisará y se ignorará el duplicado).
        ApplyChanges({ { FILE_ACTION_ADDED, name } });
        filter.clear();
        int vi = FindView(name);
        if (vi >= 0) {
            SetFocusRow(vi, false, false);
            StartRename(vi);
        }
    }

    void ShowProperties() {
        auto paths = SelectedPaths();
        std::wstring target = paths.empty() ? cur : paths[0];
        if (target.empty()) return;
        SHObjectProperties(hwnd, SHOP_FILEPATH, target.c_str(), nullptr);
    }

    void OpenTerminal() {
        if (cur.empty()) return;
        // Windows Terminal si existe; si no, el símbolo del sistema.
        SHELLEXECUTEINFOW sei = { sizeof(sei) };
        sei.fMask = SEE_MASK_FLAG_NO_UI;
        sei.hwnd = hwnd;
        sei.lpFile = L"wt.exe";
        std::wstring args = L"-d " + Quote(cur);
        sei.lpParameters = args.c_str();
        sei.nShow = SW_SHOWNORMAL;
        if (!ShellExecuteExW(&sei)) {
            SHELLEXECUTEINFOW s2 = { sizeof(s2) };
            s2.hwnd = hwnd;
            s2.lpFile = L"cmd.exe";
            s2.lpDirectory = cur.c_str();
            s2.nShow = SW_SHOWNORMAL;
            ShellExecuteExW(&s2);
        }
    }

    // ------------------------------------------------------------ renombrar
    void StartRename(int vi) {
        if (cur.empty() || vi < 0 || vi >= (int)view.size()) return;
        EnsureVisible(vi);
        Invalidate();
        const FileItem& it = items[view[vi]];
        renaming = it.name;
        if (!renameEdit) {
            renameEdit = CreateDarkEdit(hwnd, 300, nullptr);
            HookEditKeys(renameEdit, this);
        }
        float y = listRc.Y + (vi - scroll) * RowH();
        float x = colX[0] + S(26);
        float w = std::max((float)S(160), colX[1] - x - S(8));
        SetWindowPos(renameEdit, HWND_TOP, (int)x, (int)y + S(3), (int)w, RowH() - S(6), SWP_SHOWWINDOW);
        SetWindowTextW(renameEdit, it.name.c_str());
        size_t dot = it.IsDir() ? std::wstring::npos : it.name.find_last_of(L'.');
        SendMessageW(renameEdit, EM_SETSEL, 0, (dot == std::wstring::npos || dot == 0) ? -1 : (LPARAM)dot);
        SetFocus(renameEdit);
    }

    void CancelRename() {
        if (!renameEdit || !IsWindowVisible(renameEdit)) return;
        renaming.clear();
        ShowWindow(renameEdit, SW_HIDE);
        SetFocus(hwnd);
    }

    void CommitRename() {
        if (committing || !renameEdit || !IsWindowVisible(renameEdit) || renaming.empty()) return;
        committing = true;
        std::wstring oldName = renaming;
        std::wstring newName = Trim(GetWindowString(renameEdit));
        renaming.clear();
        ShowWindow(renameEdit, SW_HIDE);
        SetFocus(hwnd);
        if (!newName.empty() && newName != oldName) {
            if (newName.find_first_of(L"\\/:*?\"<>|") != std::wstring::npos) {
                ui::MsgBox(hwnd, L"El nombre no puede contener ninguno de estos caracteres:\n\\ / : * ? \" < > |",
                            L"Cambiar nombre", MB_ICONWARNING);
            } else if (!MoveFileW(JoinPath(cur, oldName).c_str(), JoinPath(cur, newName).c_str())) {
                ui::MsgBox(hwnd, ErrorText(GetLastError()).c_str(), L"Cambiar nombre", MB_ICONWARNING);
            } else {
                for (auto& it : items)
                    if (_wcsicmp(it.name.c_str(), oldName.c_str()) == 0) {
                        it.name = newName;
                        it.requested = false;
                        ResolveIconFast(it);
                    }
                focusName = anchorName = newName;
                RebuildView();
                EnsureVisible(FocusIndex());
            }
        }
        committing = false;
    }

    // ------------------------------------------------------------ menús
    void ShowNativeMenu(POINT pt, bool extended) {
        auto names = SelectedNames();
        if (names.empty() || cur.empty()) return;
        ChildItems ci;
        if (!ci.Load(hwnd, cur, names)) return;
        IContextMenu* cm = nullptr;
        if (FAILED(ci.folder->GetUIObjectOf(hwnd, (UINT)ci.children.size(), (PCUITEMID_CHILD_ARRAY)ci.children.data(),
                                            IID_IContextMenu, nullptr, (void**)&cm)))
            return;
        HMENU m = CreatePopupMenu();
        if (SUCCEEDED(cm->QueryContextMenu(m, 0, 1, 0x7FFF, CMF_NORMAL | (extended ? CMF_EXTENDEDVERBS : 0)))) {
            cm->QueryInterface(IID_PPV_ARGS(&cm3));
            cm->QueryInterface(IID_PPV_ARGS(&cm2));
            int cmd = TrackPopupMenuEx(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, hwnd, nullptr);
            if (cm3) { cm3->Release(); cm3 = nullptr; }
            if (cm2) { cm2->Release(); cm2 = nullptr; }
            if (cmd > 0) {
                wchar_t verb[64] = {};
                cm->GetCommandString(cmd - 1, GCS_VERBW, nullptr, (LPSTR)verb, 64);
                if (_wcsicmp(verb, L"rename") == 0) {
                    StartRename(FocusIndex() >= 0 ? FocusIndex() : SelectedRows().front());
                } else {
                    CMINVOKECOMMANDINFOEX ici = { sizeof(ici) };
                    ici.fMask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
                    ici.hwnd = hwnd;
                    ici.lpVerb = MAKEINTRESOURCEA(cmd - 1);
                    ici.lpVerbW = MAKEINTRESOURCEW(cmd - 1);
                    ici.lpDirectoryW = cur.c_str();
                    ici.nShow = SW_SHOWNORMAL;
                    ici.ptInvoke = pt;
                    cm->InvokeCommand((LPCMINVOKECOMMANDINFO)&ici);
                }
            }
        }
        DestroyMenu(m);
        cm->Release();
    }

    void ShowItemMenu(POINT pt) {
        auto rows = SelectedRows();
        if (rows.empty()) return;
        bool single = rows.size() == 1;
        bool dir = single && items[view[rows[0]]].IsDir();
        bool drives = cur.empty();
        HMENU m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING | MF_DEFAULT, 1, L"Abrir");
        if (dir) AppendMenuW(m, MF_STRING, 2, L"Abrir en una ventana nueva");
        if (!drives) {
            AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
            AppendMenuW(m, MF_STRING, 3, L"Cortar\tCtrl+X");
            AppendMenuW(m, MF_STRING, 4, L"Copiar\tCtrl+C");
            AppendMenuW(m, MF_STRING, 5, L"Copiar como ruta\tCtrl+Mayús+C");
            AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
            AppendMenuW(m, MF_STRING | (single ? 0 : MF_GRAYED), 6, L"Cambiar nombre\tF2");
            AppendMenuW(m, MF_STRING, 7, L"Eliminar\tSupr");
        }
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING, 8, L"Propiedades\tAlt+Entrar");
        if (!drives) AppendMenuW(m, MF_STRING, 9, L"Más opciones...");
        int cmd = ui::ThemedMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        switch (cmd) {
        case 1: OpenSelection(); break;
        case 2: OpenRow(rows[0], true); break;
        case 3: CopySelection(true); break;
        case 4: CopySelection(false); break;
        case 5: CopyPaths(); break;
        case 6: StartRename(rows[0]); break;
        case 7: DeleteSelection(GetKeyState(VK_SHIFT) < 0); break;
        case 8: ShowProperties(); break;
        case 9: ShowNativeMenu(pt, GetKeyState(VK_SHIFT) < 0); break;
        }
    }

    void CopyPaths() {
        std::wstring text;
        for (auto& p : SelectedPaths()) text += (text.empty() ? L"" : L"\r\n") + Quote(p);
        if (!text.empty()) CopyTextToClipboard(hwnd, text);
    }

    void ShowSortMenu(POINT pt) {
        HMENU m = CreatePopupMenu();
        const wchar_t* labels[4] = { L"Nombre", cur.empty() ? L"Espacio libre" : L"Fecha de modificación", L"Tipo",
                                     cur.empty() ? L"Tamaño total" : L"Tamaño" };
        for (int i = 0; i < 4; ++i) AppendMenuW(m, MF_STRING | (sortCol == i ? MF_CHECKED : 0), 1 + i, labels[i]);
        AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(m, MF_STRING | (!sortDesc ? MF_CHECKED : 0), 10, L"Ascendente");
        AppendMenuW(m, MF_STRING | (sortDesc ? MF_CHECKED : 0), 11, L"Descendente");
        int cmd = ui::ThemedMenu(m, TPM_RETURNCMD, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        if (cmd >= 1 && cmd <= 4) { sortCol = cmd - 1; RebuildView(); }
        else if (cmd == 10 || cmd == 11) { sortDesc = cmd == 11; RebuildView(); }
    }

    void ShowBackgroundMenu(POINT pt) {
        HMENU m = CreatePopupMenu();
        HMENU sort = CreatePopupMenu();
        const wchar_t* labels[4] = { L"Nombre", L"Fecha de modificación", L"Tipo", L"Tamaño" };
        for (int i = 0; i < 4; ++i) AppendMenuW(sort, MF_STRING | (sortCol == i ? MF_CHECKED : 0), 20 + i, labels[i]);
        AppendMenuW(m, MF_POPUP, (UINT_PTR)sort, L"Ordenar por");
        AppendMenuW(m, MF_STRING | (showHidden ? MF_CHECKED : 0), 1, L"Mostrar elementos ocultos");
        AppendMenuW(m, MF_STRING, 2, L"Actualizar\tF5");
        if (!cur.empty()) {
            AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
            AppendMenuW(m, MF_STRING | (IsClipboardFormatAvailable(CF_HDROP) ? 0 : MF_GRAYED), 3, L"Pegar\tCtrl+V");
            AppendMenuW(m, MF_STRING, 4, L"Nueva carpeta\tCtrl+Mayús+N");
            AppendMenuW(m, MF_SEPARATOR, 0, nullptr);
            AppendMenuW(m, MF_STRING, 5, L"Abrir en Terminal");
            AppendMenuW(m, MF_STRING, 6, L"Copiar ruta de la carpeta");
            AppendMenuW(m, MF_STRING, 7, L"Propiedades");
        }
        int cmd = ui::ThemedMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(m);
        switch (cmd) {
        case 1: showHidden = !showHidden; RebuildView(); break;
        case 2: pendingSelect = focusName; Load(); break;
        case 3: Paste(); break;
        case 4: NewFolder(); break;
        case 5: OpenTerminal(); break;
        case 6: CopyTextToClipboard(hwnd, cur); break;
        case 7: SHObjectProperties(hwnd, SHOP_FILEPATH, cur.c_str(), nullptr); break;
        default:
            if (cmd >= 20 && cmd < 24) { sortCol = cmd - 20; RebuildView(); }
        }
    }

    // ------------------------------------------------------------ pintado
    bool ButtonEnabled(int id) const {
        bool sel = !SelectedRows().empty();
        switch (id) {
        case BBack: return !backStack.empty();
        case BFwd: return !fwdStack.empty();
        case BUp: return !cur.empty();
        case BNewFolder: case BTerminal: return !cur.empty();
        case BCut: case BCopy: case BDelete: return sel && !cur.empty();
        case BRename: return SelectedRows().size() == 1 && !cur.empty();
        case BPaste: return !cur.empty() && IsClipboardFormatAvailable(CF_HDROP);
        case BProps: return sel || !cur.empty();
        default: return true;
        }
    }

    static const wchar_t* ButtonTip(int id) {
        switch (id) {
        case BBack: return L"Atrás (Alt+←)";
        case BFwd: return L"Adelante (Alt+→)";
        case BUp: return L"Subir un nivel (Alt+↑)";
        case BRefresh: return L"Actualizar (F5)";
        case BNewFolder: return L"Nueva carpeta (Ctrl+Mayús+N)";
        case BCut: return L"Cortar (Ctrl+X)";
        case BCopy: return L"Copiar (Ctrl+C)";
        case BPaste: return L"Pegar (Ctrl+V)";
        case BRename: return L"Cambiar nombre (F2)";
        case BDelete: return L"Eliminar (Supr) · Mayús+Supr elimina permanentemente";
        case BProps: return L"Propiedades (Alt+Entrar)";
        case BTerminal: return L"Abrir en Terminal";
        case BSort: return L"Ordenar";
        case BHidden: return L"Mostrar u ocultar elementos ocultos";
        default: return L"";
        }
    }

    void OnPaint(Graphics& g, int w, int h) override {
        bool zoomed = IsZoomed(hwnd) != FALSE;
        std::wstring title = cur.empty() ? L"Este equipo" : (BaseName(cur).empty() ? cur : BaseName(cur));
        style::DrawChrome(g, w, h, title + L" — Archivos", L"\uEC50", zoomed,
                          hoverBtn >= BMin && hoverBtn <= BClose ? hoverBtn : 0);

        // Barra de navegación
        const wchar_t* navGlyphs[4] = { L"\uE72B", L"\uE72A", L"\uE74A", L"\uE72C" };
        for (int i = 0; i < 4; ++i)
            style::DrawToolButton(g, btns[BBack + i], navGlyphs[i], L"", hoverBtn == BBack + i, ButtonEnabled(BBack + i));
        style::DrawField(g, addrRc, GetFocus() == addr);
        style::DrawField(g, searchRc, GetFocus() == search);
        DrawTextStr(g, L"\uE721", fontIcon, style::textDim, RectF(searchRc.X + S(8), searchRc.Y, (float)S(20), searchRc.Height),
                    StringAlignmentCenter);

        // Barra de comandos
        style::DrawToolButton(g, btns[BNewFolder], L"\uE8F4", L"Nueva carpeta", hoverBtn == BNewFolder, ButtonEnabled(BNewFolder));
        const std::pair<int, const wchar_t*> cmdGlyphs[] = { { BCut, L"\uE8C6" }, { BCopy, L"\uE8C8" }, { BPaste, L"\uE77F" },
                                                             { BRename, L"\uE8AC" }, { BDelete, L"\uE74D" }, { BProps, L"\uE946" },
                                                             { BTerminal, L"\uE756" } };
        for (auto& cg : cmdGlyphs) style::DrawToolButton(g, btns[cg.first], cg.second, L"", hoverBtn == cg.first, ButtonEnabled(cg.first));
        style::DrawToolButton(g, btns[BSort], L"\uE8CB", L"Ordenar", hoverBtn == BSort);
        style::DrawToolButton(g, btns[BHidden], showHidden ? L"\uE7B3" : L"\uED1A", L"Ver ocultos", hoverBtn == BHidden, true, showHidden);
        HLine(g, 0, (float)w, navRc.Y - 1, style::separator);

        PaintNav(g);
        PaintList(g);

        // Barra de estado
        HLine(g, 0, (float)w, statusRc.Y, style::separator);
        std::wstring status = std::to_wstring(view.size()) + (view.size() == 1 ? L" elemento" : L" elementos");
        auto rows = SelectedRows();
        if (!rows.empty()) {
            ULONGLONG total = 0;
            bool files = true;
            for (int vi : rows) {
                if (items[view[vi]].IsDir()) files = false;
                total += items[view[vi]].size;
            }
            status += L"   |   " + std::to_wstring(rows.size()) + (rows.size() == 1 ? L" seleccionado" : L" seleccionados");
            if (files && !cur.empty()) status += L"  " + FormatBytes(total);
        }
        if (loading) status += L"   |   Cargando...";
        DrawTextStr(g, status, fontSmall, style::textDim, RectF((float)S(12), statusRc.Y, statusRc.Width - S(24), statusRc.Height));
    }

    void PaintNav(Graphics& g) {
        FillRect(g, navRc, style::navBg);
        for (int i = 0; i < (int)nav.size(); ++i) {
            const NavEntry& e = nav[i];
            if (e.rc.Y + e.rc.Height > navRc.Y + navRc.Height) break;
            if (e.header) {
                DrawTextStr(g, e.label, fontSmallBold, style::textFaint,
                            RectF(e.rc.X + S(8), e.rc.Y + S(6), e.rc.Width - S(8), e.rc.Height - S(6)));
                continue;
            }
            bool active = (e.path == L"::thispc") ? cur.empty() : (!cur.empty() && _wcsicmp(e.path.c_str(), cur.c_str()) == 0);
            if (active) DrawRounded(g, e.rc, SF(5), style::rowSelected);
            else if (i == hoverNav) DrawRounded(g, e.rc, SF(5), style::rowHover);
            if (Bitmap* b = SysIconBitmap(e.icon, S(16)))
                g.DrawImage(b, e.rc.X + S(10), e.rc.Y + (e.rc.Height - S(16)) / 2, (float)S(16), (float)S(16));
            DrawTextStr(g, e.label, fontSmall, active ? style::text : style::textDim,
                        RectF(e.rc.X + S(34), e.rc.Y, e.rc.Width - S(40), e.rc.Height));
        }
        Pen sep(style::separator, 1.0f);
        g.DrawLine(&sep, navRc.X + navRc.Width, navRc.Y, navRc.X + navRc.Width, navRc.Y + navRc.Height);
    }

    void PaintList(Graphics& g) {
        bool drives = cur.empty();
        const wchar_t* heads[4] = { L"Nombre", drives ? L"Espacio libre" : L"Fecha de modificación",
                                    drives ? L"Tipo" : L"Tipo", drives ? L"Tamaño total" : L"Tamaño" };
        bool narrow = colX[1] == colX[3];
        for (int c = 0; c < 4; ++c) {
            if (narrow && (c == 1 || c == 2)) continue;
            RectF r(colX[c] + S(4), headerRc.Y, colX[c + 1] - colX[c] - S(10), headerRc.Height);
            if (hoverCol == c) DrawRounded(g, RectF(colX[c], headerRc.Y + S(3), colX[c + 1] - colX[c], headerRc.Height - S(6)), SF(4), style::rowHover);
            DrawTextStr(g, heads[c], fontSmallBold, sortCol == c ? style::text : style::textDim, r,
                        c == 3 ? StringAlignmentFar : StringAlignmentNear);
            if (sortCol == c) {
                float ax = c == 3 ? colX[c] + S(2) : colX[c + 1] - S(18);
                DrawTextStr(g, sortDesc ? L"\uE70D" : L"\uE70E", fontIcon, style::accent,
                            RectF(ax, headerRc.Y, (float)S(14), headerRc.Height), StringAlignmentCenter);
            }
        }
        HLine(g, headerRc.X, headerRc.X + headerRc.Width, headerRc.Y + headerRc.Height - 1, style::separator);

        int vis = RowsVisible();
        int focus = FocusIndex();
        float isz = (float)S(16);
        GraphicsState clipState = g.Save();
        g.IntersectClip(RectF(listRc.X, listRc.Y, listRc.Width + S(10), listRc.Height));
        for (int row = 0; row <= vis; ++row) {
            int vi = scroll + row;
            if (vi >= (int)view.size()) break;
            FileItem& it = items[view[vi]];
            RectF rr(listRc.X + S(4), listRc.Y + row * RowH(), listRc.Width - S(4), (float)RowH() - S(2));
            if (rr.Y >= listRc.Y + listRc.Height) break;
            if (it.selected) DrawRounded(g, rr, SF(5), style::rowSelected);
            else if (vi == dropRow) DrawRounded(g, rr, SF(5), WithAlpha(style::success, 50), style::success);
            else if (vi == hoverRow) DrawRounded(g, rr, SF(5), style::rowHover);
            if (vi == focus && GetFocus() == hwnd) DrawRounded(g, rr, SF(5), Color(0, 0, 0, 0), style::focusRing);

            if (it.icon == -2 && !it.requested) {
                it.requested = true;
                IconWorker::Get().Request(hwnd, gen, FullPath(it), it.name);
            }
            int idx = it.icon >= 0 ? it.icon : (it.IsDir() ? FolderIconIndex() : ExtIconIndex(L""));
            if (Bitmap* b = SysIconBitmap(idx, (int)isz))
                g.DrawImage(b, colX[0], rr.Y + (rr.Height - isz) / 2, isz, isz);
            bool hiddenAttr = (it.attrs & FILE_ATTRIBUTE_HIDDEN) && !drives;
            Color nameColor = hiddenAttr ? style::textFaint : style::text;
            DrawTextStr(g, it.name, fontBody, nameColor, RectF(colX[0] + S(26), rr.Y, colX[1] - colX[0] - S(32), rr.Height));
            if (drives) {
                if (it.total) {
                    float bw = colX[2] - colX[1] - S(16);
                    float used = 1.0f - (float)((double)it.freeBytes / (double)it.total);
                    style::DrawProgress(g, RectF(colX[1] + S(4), rr.Y + S(6), bw, (float)S(5)), used,
                                        used > 0.9f ? style::danger : style::accent);
                    DrawTextStr(g, FormatBytes(it.freeBytes) + L" libres", fontSmall, style::textDim,
                                RectF(colX[1] + S(4), rr.Y + S(12), bw, rr.Height - S(12)));
                }
                DrawTextStr(g, DriveTypeName(it.driveType), fontSmall, style::textDim,
                            RectF(colX[2] + S(4), rr.Y, colX[3] - colX[2] - S(8), rr.Height));
                if (it.total)
                    DrawTextStr(g, FormatBytes(it.total), fontSmall, style::textDim,
                                RectF(colX[3], rr.Y, colX[4] - colX[3] - S(8), rr.Height), StringAlignmentFar);
            } else {
                if (!narrow) {
                    DrawTextStr(g, FormatTime(it.mtime), fontSmall, style::textDim,
                                RectF(colX[1] + S(4), rr.Y, colX[2] - colX[1] - S(8), rr.Height));
                    DrawTextStr(g, TypeName(it), fontSmall, style::textDim,
                                RectF(colX[2] + S(4), rr.Y, colX[3] - colX[2] - S(8), rr.Height));
                }
                if (!it.IsDir())
                    DrawTextStr(g, FormatBytes(it.size), fontSmall, style::textDim,
                                RectF(colX[3], rr.Y, colX[4] - colX[3] - S(8), rr.Height), StringAlignmentFar);
            }
        }

        g.Restore(clipState);

        RectF msgRc(listRc.X, listRc.Y + S(24), listRc.Width, (float)S(30));
        if (view.empty() && !loading) {
            std::wstring msg = loadError == ERROR_ACCESS_DENIED ? L"Acceso denegado a esta carpeta."
                             : loadError && loadError != ERROR_FILE_NOT_FOUND ? ErrorText(loadError)
                             : !filter.empty() ? L"Ningún elemento coincide con la búsqueda."
                             : L"Esta carpeta está vacía.";
            DrawTextStr(g, msg, fontBody, style::textFaint, msgRc, StringAlignmentCenter);
        } else if (view.empty() && loading) {
            DrawTextStr(g, L"Cargando...", fontBody, style::textFaint, msgRc, StringAlignmentCenter);
        }
        DrawScrollbar(g, RectF(listRc.X + listRc.Width + S(3), listRc.Y + S(2), (float)S(4), listRc.Height - S(4)),
                      (int)view.size(), vis, scroll);
    }

    // ------------------------------------------------------------ entrada
    int RowAt(int x, int y) const {
        if (!InRect(listRc, x, y)) return -1;
        int vi = scroll + (int)((y - listRc.Y) / RowH());
        return vi < (int)view.size() ? vi : -1;
    }

    int ButtonAt(int x, int y) const {
        for (int id : { BMin, BMax, BClose, BBack, BFwd, BUp, BRefresh, BNewFolder, BCut, BCopy, BPaste, BRename,
                        BDelete, BProps, BTerminal, BSort, BHidden })
            if (InRect(btns[id], x, y)) return id;
        return BNone;
    }

    int NavAt(int x, int y) const {
        for (int i = 0; i < (int)nav.size(); ++i)
            if (!nav[i].header && InRect(nav[i].rc, x, y)) return i;
        return -1;
    }

    int ColAt(int x, int y) const {
        if (!InRect(headerRc, x, y)) return -1;
        for (int c = 0; c < 4; ++c)
            if (x >= colX[c] && x < colX[c + 1]) return c;
        return -1;
    }

    void ClickButton(int id) {
        if (!ButtonEnabled(id)) return;
        POINT pt = { (LONG)btns[id].X, (LONG)(btns[id].Y + btns[id].Height) };
        ClientToScreen(hwnd, &pt);
        switch (id) {
        case BMin: ShowWindow(hwnd, SW_MINIMIZE); break;
        case BMax: ShowWindow(hwnd, IsZoomed(hwnd) ? SW_RESTORE : SW_MAXIMIZE); break;
        case BClose: Close(); break;
        case BBack: GoBack(); break;
        case BFwd: GoForward(); break;
        case BUp: GoUp(); break;
        case BRefresh: pendingSelect = focusName; Load(); break;
        case BNewFolder: NewFolder(); break;
        case BCut: CopySelection(true); break;
        case BCopy: CopySelection(false); break;
        case BPaste: Paste(); break;
        case BRename: { auto r = SelectedRows(); if (r.size() == 1) StartRename(r[0]); break; }
        case BDelete: DeleteSelection(GetKeyState(VK_SHIFT) < 0); break;
        case BProps: ShowProperties(); break;
        case BTerminal: OpenTerminal(); break;
        case BSort: ShowSortMenu(pt); break;
        case BHidden: showHidden = !showHidden; RebuildView(); break;
        }
    }

    void OnMouseDown(int x, int y) override {
        CommitRename();
        mouseDownOnSel = false;
        int b = ButtonAt(x, y);
        if (b) { ClickButton(b); return; }
        if (InRect(addrRc, x, y)) { SetFocus(addr); SendMessageW(addr, EM_SETSEL, 0, -1); return; }
        if (InRect(searchRc, x, y)) { SetFocus(search); return; }
        int n = NavAt(x, y);
        if (n >= 0) { Navigate(nav[n].path); return; }
        int c = ColAt(x, y);
        if (c >= 0) {
            if (sortCol == c) sortDesc = !sortDesc;
            else { sortCol = c; sortDesc = (c == 1 || c == 3); }
            RebuildView();
            return;
        }
        int vi = RowAt(x, y);
        bool ctrl = GetKeyState(VK_CONTROL) < 0, shift = GetKeyState(VK_SHIFT) < 0;
        if (vi >= 0) {
            if (items[view[vi]].selected && !ctrl && !shift) {
                // Puede ser el inicio de un arrastre: la selección se reduce al soltar.
                mouseDownOnSel = true;
                downPt = { x, y };
                focusName = items[view[vi]].name;
                Invalidate();
            } else {
                SetFocusRow(vi, shift, ctrl);
                mouseDownOnSel = true;
                downPt = { x, y };
            }
        } else if (InRect(listRc, x, y) && !ctrl) {
            ClearSelection();
            Invalidate();
        }
    }

    void OnMouseUp(int x, int y) override {
        if (mouseDownOnSel) {
            int vi = RowAt(x, y);
            bool ctrl = GetKeyState(VK_CONTROL) < 0, shift = GetKeyState(VK_SHIFT) < 0;
            if (vi >= 0 && !ctrl && !shift && SelectedRows().size() > 1) SetFocusRow(vi, false, false);
        }
        mouseDownOnSel = false;
    }

    void OnDoubleClick(int x, int y) override {
        int vi = RowAt(x, y);
        if (vi >= 0) OpenRow(vi);
        else if (InRect(listRc, x, y) && !cur.empty() && view.empty()) {}
    }

    void BeginDrag() {
        mouseDownOnSel = false;
        auto names = SelectedNames();
        if (names.empty() || cur.empty()) return;
        ChildItems ci;
        if (!ci.Load(hwnd, cur, names)) return;
        IDataObject* data = nullptr;
        if (FAILED(ci.folder->GetUIObjectOf(hwnd, (UINT)ci.children.size(), (PCUITEMID_CHILD_ARRAY)ci.children.data(),
                                            IID_IDataObject, nullptr, (void**)&data)))
            return;
        DWORD effect = 0;
        SHDoDragDrop(hwnd, data, nullptr, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK, &effect);
        data->Release();
    }

    void OnMouseMove(int x, int y) override {
        if (mouseDownOnSel && (GetKeyState(VK_LBUTTON) & 0x8000) &&
            (abs(x - downPt.x) > GetSystemMetrics(SM_CXDRAG) || abs(y - downPt.y) > GetSystemMetrics(SM_CYDRAG))) {
            BeginDrag();
            return;
        }
        int b = ButtonAt(x, y);
        int r = RowAt(x, y);
        int n = NavAt(x, y);
        int c = ColAt(x, y);
        if (b != hoverBtn) {
            if (hoverBtn) InvalidateRectF(btns[hoverBtn]);
            if (b) InvalidateRectF(btns[b]);
            hoverBtn = b;
            tip.Set(b ? btns[b] : RectF(), b ? ButtonTip(b) : L"");
        }
        if (r != hoverRow) {
            for (int v : { hoverRow, r })
                if (v >= scroll) InvalidateRectF(RectF(listRc.X, listRc.Y + (v - scroll) * RowH(), listRc.Width, (float)RowH()));
            hoverRow = r;
        }
        if (n != hoverNav) {
            if (hoverNav >= 0 && hoverNav < (int)nav.size()) InvalidateRectF(nav[hoverNav].rc);
            if (n >= 0) InvalidateRectF(nav[n].rc);
            hoverNav = n;
        }
        if (c != hoverCol) { hoverCol = c; InvalidateRectF(headerRc); }
    }

    void OnWheel(int delta) override {
        CancelRename();
        scroll -= (delta / WHEEL_DELTA) * 3;
        ClampScroll();
        InvalidateRectF(listRc);
    }

    void OnKeyDown(int vk) override {
        bool ctrl = GetKeyState(VK_CONTROL) < 0, shift = GetKeyState(VK_SHIFT) < 0;
        int focus = FocusIndex();
        int n = (int)view.size();
        if (ctrl) {
            switch (vk) {
            case 'A': for (int vi : view) items[vi].selected = true; Invalidate(); return;
            case 'C': if (shift) CopyPaths(); else CopySelection(false); return;
            case 'X': CopySelection(true); return;
            case 'V': Paste(); return;
            case 'N': if (shift) NewFolder(); else Open(cur); return;
            case 'W': Close(); return;
            case 'L': SetFocus(addr); SendMessageW(addr, EM_SETSEL, 0, -1); return;
            case 'F': SetFocus(search); return;
            case 'R': pendingSelect = focusName; Load(); return;
            }
        }
        switch (vk) {
        case VK_UP: SetFocusRow(std::max(0, focus - 1), shift, ctrl && !shift); return;
        case VK_DOWN: SetFocusRow(focus < 0 ? 0 : std::min(n - 1, focus + 1), shift, ctrl && !shift); return;
        case VK_PRIOR: SetFocusRow(std::max(0, focus - RowsVisible()), shift, false); return;
        case VK_NEXT: SetFocusRow(std::min(n - 1, std::max(0, focus) + RowsVisible()), shift, false); return;
        case VK_HOME: SetFocusRow(0, shift, false); return;
        case VK_END: SetFocusRow(n - 1, shift, false); return;
        case VK_RETURN: OpenSelection(); return;
        case VK_BACK: GoBack(); return;
        case VK_DELETE: DeleteSelection(shift); return;
        case VK_F2: { auto r = SelectedRows(); if (r.size() == 1) StartRename(r[0]); return; }
        case VK_F3: SetFocus(search); return;
        case VK_F4: SetFocus(addr); SendMessageW(addr, EM_SETSEL, 0, -1); return;
        case VK_F5: pendingSelect = focusName; Load(); return;
        case VK_ESCAPE: ClearSelection(); Invalidate(); return;
        case VK_APPS: {
            RECT rc;
            GetClientRect(hwnd, &rc);
            POINT pt = { (LONG)listRc.X + S(40), (LONG)(listRc.Y + std::max(0, focus - scroll) * RowH() + RowH()) };
            ClientToScreen(hwnd, &pt);
            if (SelectedRows().empty()) ShowBackgroundMenu(pt);
            else ShowItemMenu(pt);
            return;
        }
        }
    }

    // Escribir para saltar al primer elemento que empieza por esas letras.
    void OnChar(wchar_t ch) override {
        if (ch < 32 || GetKeyState(VK_CONTROL) < 0) return;
        ULONGLONG now = GetTickCount64();
        if (now - typeTick > 900) typeBuffer.clear();
        typeTick = now;
        typeBuffer.push_back((wchar_t)towlower(ch));
        int n = (int)view.size();
        int start = FocusIndex();
        bool repeat = typeBuffer.size() > 1 && typeBuffer.find_first_not_of(typeBuffer[0]) == std::wstring::npos;
        std::wstring q = repeat ? typeBuffer.substr(0, 1) : typeBuffer;
        int from = (repeat || typeBuffer.size() == 1) ? start + 1 : std::max(0, start);
        for (int k = 0; k < n; ++k) {
            int vi = (from + k) % n;
            if (ToLower(items[view[vi]].name).rfind(q, 0) == 0) { SetFocusRow(vi, false, false); return; }
        }
    }

    void OnActivate(bool) override { InvalidateRectF(listRc); }

    LRESULT OnHitTest(int x, int y) override {
        return FrameHitTest(x, y, S(kCaption), { btns[BMin], btns[BMax], btns[BClose] });
    }

    void OnCommand(int id, int code) override {
        if (id == 201 && code == EN_CHANGE) {
            filter = GetWindowString(search);
            RebuildView();
            Invalidate();
        }
        // Borde de acento en la caja con el foco.
        if (code == EN_SETFOCUS || code == EN_KILLFOCUS) {
            InvalidateRectF(addrRc);
            InvalidateRectF(searchRc);
        }
    }

    void OnDestroy() override;

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        // Submenús del menú contextual nativo ("Abrir con", "Enviar a"...).
        if (cm3 || cm2) {
            if (msg == WM_INITMENUPOPUP || msg == WM_DRAWITEM || msg == WM_MEASUREITEM || msg == WM_MENUCHAR) {
                LRESULT r = 0;
                if (cm3 && SUCCEEDED(cm3->HandleMenuMsg2(msg, wp, lp, &r))) return r;
                if (cm2 && SUCCEEDED(cm2->HandleMenuMsg(msg, wp, lp))) return 0;
            }
        }
        switch (msg) {
        case WM_ENUMBATCH: {
            std::unique_ptr<std::vector<FileItem>> batch((std::vector<FileItem>*)lp);
            if ((UINT)wp != gen) return 0;
            for (auto& it : *batch) {
                ResolveIconFast(it);
                items.push_back(std::move(it));
            }
            RebuildView();
            ApplyPendingSelect();
            return 0;
        }
        case WM_ENUMDONE:
            if ((UINT)wp == gen) {
                loading = false;
                loadError = (DWORD)lp;
                ApplyPendingSelect();
                pendingSelect.clear();
                Invalidate();
            }
            return 0;
        case WM_ICONREADY: {
            std::unique_ptr<IconReady> r((IconReady*)lp);
            if ((UINT)wp != gen) return 0;
            for (auto& it : items)
                if (it.name == r->name) { it.icon = r->index; break; }
            InvalidateRectF(listRc);
            return 0;
        }
        case WM_DIRCHANGES: {
            std::unique_ptr<std::vector<DirChange>> ch((std::vector<DirChange>*)lp);
            if ((UINT)wp == gen) ApplyChanges(*ch);
            return 0;
        }
        case WM_RBUTTONUP: {
            int x = GET_X_LPARAM(lp), y = GET_Y_LPARAM(lp);
            CommitRename();
            POINT pt = { x, y };
            ClientToScreen(hwnd, &pt);
            int vi = RowAt(x, y);
            if (vi >= 0) {
                if (!items[view[vi]].selected) SetFocusRow(vi, false, false);
                if (GetKeyState(VK_SHIFT) < 0) ShowNativeMenu(pt, true);
                else ShowItemMenu(pt);
            } else if (InRect(listRc, x, y)) {
                ClearSelection();
                Invalidate();
                ShowBackgroundMenu(pt);
            } else if (NavAt(x, y) >= 0) {
                int ni = NavAt(x, y);
                HMENU m = CreatePopupMenu();
                AppendMenuW(m, MF_STRING, 1, L"Abrir");
                AppendMenuW(m, MF_STRING, 2, L"Abrir en una ventana nueva");
                int cmd = ui::ThemedMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, nullptr);
                DestroyMenu(m);
                if (cmd == 1) Navigate(nav[ni].path);
                else if (cmd == 2) Open(nav[ni].path == L"::thispc" ? L"" : nav[ni].path);
            }
            return 0;
        }
        case WM_XBUTTONUP:
            if (GET_XBUTTON_WPARAM(wp) == XBUTTON1) GoBack();
            else GoForward();
            return TRUE;
        case WM_SYSKEYDOWN:
            if (wp == VK_LEFT) { GoBack(); return 0; }
            if (wp == VK_RIGHT) { GoForward(); return 0; }
            if (wp == VK_UP) { GoUp(); return 0; }
            if (wp == VK_RETURN) { ShowProperties(); return 0; }
            if (wp == 'D') { SetFocus(addr); SendMessageW(addr, EM_SETSEL, 0, -1); return 0; }
            break;
        case WM_GETMINMAXINFO: {
            auto* mm = (MINMAXINFO*)lp;
            mm->ptMinTrackSize = { S(640), S(420) };
            return 0;
        }
        case WM_DEVICECHANGE:
            if (wp == 0x8000 /*DBT_DEVICEARRIVAL*/ || wp == 0x8004 /*DBT_DEVICEREMOVECOMPLETE*/) {
                BuildNav();
                if (cur.empty()) Load();
                Invalidate();
            }
            return TRUE;
        case WM_SETFOCUS:
        case WM_KILLFOCUS:
            InvalidateRectF(listRc);
            break;
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
};

// ---------------------------------------------------------------- soltar archivos
class DropTarget : public IDropTarget {
public:
    explicit DropTarget(ExplorerWindow* w, HWND h) : wnd(w), hwnd(h) {
        CoCreateInstance(CLSID_DragDropHelper, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&helper));
    }
    virtual ~DropTarget() {
        if (helper) helper->Release();
    }

    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void** ppv) override {
        if (riid == IID_IUnknown || riid == IID_IDropTarget) {
            *ppv = static_cast<IDropTarget*>(this);
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&ref); }
    ULONG STDMETHODCALLTYPE Release() override {
        LONG r = InterlockedDecrement(&ref);
        if (!r) delete this;
        return r;
    }

    HRESULT STDMETHODCALLTYPE DragEnter(IDataObject* data, DWORD keys, POINTL pt, DWORD* effect) override {
        files.clear();
        FORMATETC fe = { CF_HDROP, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL };
        STGMEDIUM sm = {};
        if (SUCCEEDED(data->GetData(&fe, &sm))) {
            files = FilesFromHDrop((HDROP)sm.hGlobal);
            ReleaseStgMedium(&sm);
        }
        *effect = Effect(keys, pt);
        if (helper) { POINT p = { pt.x, pt.y }; helper->DragEnter(hwnd, data, &p, *effect); }
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE DragOver(DWORD keys, POINTL pt, DWORD* effect) override {
        *effect = Effect(keys, pt);
        if (helper) { POINT p = { pt.x, pt.y }; helper->DragOver(&p, *effect); }
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE DragLeave() override {
        wnd->SetDropHighlight(-1, -1);
        if (helper) helper->DragLeave();
        files.clear();
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE Drop(IDataObject* data, DWORD keys, POINTL pt, DWORD* effect) override {
        *effect = Effect(keys, pt);
        std::wstring dest = Dest(pt);
        if (helper) { POINT p = { pt.x, pt.y }; helper->Drop(data, &p, *effect); }
        wnd->SetDropHighlight(-1, -1);
        if (*effect != DROPEFFECT_NONE && !dest.empty()) wnd->DropFiles(files, dest, *effect);
        files.clear();
        return S_OK;
    }

private:
    LONG ref = 1;
    ExplorerWindow* wnd;
    HWND hwnd;
    IDropTargetHelper* helper = nullptr;
    std::vector<std::wstring> files;

    std::wstring Dest(POINTL pt) {
        POINT p = { pt.x, pt.y };
        ScreenToClient(hwnd, &p);
        return wnd->DropFolderAt(p.x, p.y);
    }

    DWORD Effect(DWORD keys, POINTL pt) {
        POINT p = { pt.x, pt.y };
        ScreenToClient(hwnd, &p);
        wnd->SetDropHighlight(p.x, p.y);
        std::wstring dest = wnd->DropFolderAt(p.x, p.y);
        if (files.empty() || dest.empty()) return DROPEFFECT_NONE;
        for (auto& f : files)
            if (_wcsicmp(f.c_str(), dest.c_str()) == 0 || _wcsicmp(ParentDir(f).c_str(), dest.c_str()) == 0)
                return DROPEFFECT_NONE; // soltar en su propia carpeta
        if (keys & MK_CONTROL) return DROPEFFECT_COPY;
        if (keys & MK_SHIFT) return DROPEFFECT_MOVE;
        // Como Explorer: mover dentro de la misma unidad, copiar entre unidades.
        bool sameDrive = _wcsnicmp(files[0].c_str(), dest.c_str(), 2) == 0;
        return sameDrive ? DROPEFFECT_MOVE : DROPEFFECT_COPY;
    }
};

// ---------------------------------------------------------------- implementación diferida
bool ExplorerWindow::Start(const std::wstring& path, const std::wstring& selectName) {
    customFrame = true;
    RECT wa = {};
    SystemParametersInfoW(SPI_GETWORKAREA, 0, &wa, 0);
    int w = std::min(S(1100), (int)(wa.right - wa.left) - S(40));
    int h = std::min(S(700), (int)(wa.bottom - wa.top) - S(40));
    int offset = (int)(g_windows.size() % 6) * S(28);
    int x = wa.left + (wa.right - wa.left - w) / 2 + offset;
    int y = wa.top + (wa.bottom - wa.top - h) / 2 + offset;
    if (!Create(L"OShellCppExplorer", WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, x, y, w, h, L"Archivos"))
        return false;
    SetFrame(FrameOpaque);
    SetWindowPos(hwnd, nullptr, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);

    SHSTOCKICONINFO sii = { sizeof(sii) };
    if (SUCCEEDED(SHGetStockIconInfo(SIID_FOLDER, SHGSI_ICON | SHGSI_LARGEICON, &sii))) iconBig = sii.hIcon;
    if (SUCCEEDED(SHGetStockIconInfo(SIID_FOLDER, SHGSI_ICON | SHGSI_SMALLICON, &sii))) iconSmall = sii.hIcon;
    if (iconBig) SendMessageW(hwnd, WM_SETICON, ICON_BIG, (LPARAM)iconBig);
    if (iconSmall) SendMessageW(hwnd, WM_SETICON, ICON_SMALL, (LPARAM)iconSmall);

    addr = CreateDarkEdit(hwnd, 200, nullptr);
    search = CreateDarkEdit(hwnd, 201, L"Buscar en esta carpeta");
    HookEditKeys(addr, this);
    HookEditKeys(search, this);
    tip.Create(hwnd);
    BuildNav();
    RECT rc;
    GetClientRect(hwnd, &rc);
    Layout(rc.right, rc.bottom);
    PositionEdits();

    dropTarget = new DropTarget(this, hwnd);
    RegisterDragDrop(hwnd, dropTarget);

    g_windows.push_back(this);
    if (!Navigate(path, false, selectName)) Navigate(L"", false);
    ShowWindow(hwnd, SW_SHOWNORMAL);
    ForceForeground(hwnd);
    SetFocus(hwnd);
    return true;
}

void ExplorerWindow::OnDestroy() {
    watcher.Stop();
    RevokeDragDrop(hwnd);
    if (dropTarget) { dropTarget->Release(); dropTarget = nullptr; }
    g_windows.erase(std::remove(g_windows.begin(), g_windows.end(), this), g_windows.end());
    // La ventana se libera después de procesar todos sus mensajes.
    ExplorerWindow* self = this;
    PostMessageW(g_host, WM_APP + 60, 0, (LPARAM)self);
}

ExplorerWindow::~ExplorerWindow() {
    if (iconBig) DestroyIcon(iconBig);
    if (iconSmall) DestroyIcon(iconSmall);
}

// ---------------------------------------------------------------- proceso único
void OpenFromArgs(const std::wstring& rawPath, bool select) {
    std::wstring path = rawPath.empty() ? L"" : NormalizeDir(rawPath);
    if (select && !path.empty()) {
        ExplorerWindow::Open(ParentDir(path), BaseName(path));
        return;
    }
    if (!path.empty() && !IsDirectory(path)) {
        DWORD a = GetFileAttributesW(path.c_str());
        if (a != INVALID_FILE_ATTRIBUTES) { // un archivo: abrir su carpeta seleccionándolo
            ExplorerWindow::Open(ParentDir(path), BaseName(path));
            return;
        }
    }
    ExplorerWindow::Open(path);
}

LRESULT CALLBACK HostProc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_COPYDATA: {
        auto* cds = (COPYDATASTRUCT*)lp;
        if (!cds || cds->dwData != kCopyDataId || cds->cbData < sizeof(wchar_t)) return FALSE;
        std::wstring payload((const wchar_t*)cds->lpData, cds->cbData / sizeof(wchar_t));
        payload = payload.c_str(); // hasta el primer nulo
        if (payload.empty()) return FALSE;
        OpenFromArgs(payload.substr(1), payload[0] == L'S');
        MaybeQuit();
        return TRUE;
    }
    case WM_APP + 60:
        delete (ExplorerWindow*)lp;
        MaybeQuit();
        return 0;
    case WM_OPDONE:
        MaybeQuit();
        return 0;
    }
    return DefWindowProcW(h, msg, wp, lp);
}

} // namespace

int RunExplorerMode() {
    // Argumentos: --explorer [--select] [ruta]
    int argc = 0;
    LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    std::wstring path;
    bool select = false;
    for (int i = 1; argv && i < argc; ++i) {
        if (_wcsicmp(argv[i], L"--select") == 0) select = true;
        else if (wcsncmp(argv[i], L"--", 2) != 0 && path.empty()) path = argv[i];
    }
    if (argv) LocalFree(argv);

    HANDLE mutex = CreateMutexW(nullptr, FALSE, L"Local\\OptimizedShell.Explorer");
    if (GetLastError() == ERROR_ALREADY_EXISTS) {
        HWND host = FindWindowExW(HWND_MESSAGE, nullptr, kHostClass, nullptr);
        if (host) {
            std::wstring payload = std::wstring(select ? L"S" : L"O") + path;
            COPYDATASTRUCT cds = { kCopyDataId, (DWORD)((payload.size() + 1) * sizeof(wchar_t)), (void*)payload.c_str() };
            DWORD pid = 0;
            GetWindowThreadProcessId(host, &pid);
            AllowSetForegroundWindow(pid);
            DWORD_PTR res = 0;
            if (SendMessageTimeoutW(host, WM_COPYDATA, 0, (LPARAM)&cds, SMTO_ABORTIFHUNG, 5000, &res) && res) {
                CloseHandle(mutex);
                return 0;
            }
        }
    }

    OleInitialize(nullptr); // necesario para arrastrar y soltar
    InitUi(GetModuleHandleW(nullptr));

    WNDCLASSEXW wc = { sizeof(wc) };
    wc.lpfnWndProc = HostProc;
    wc.hInstance = g_hInst;
    wc.lpszClassName = kHostClass;
    RegisterClassExW(&wc);
    g_host = CreateWindowExW(0, kHostClass, L"", 0, 0, 0, 0, 0, HWND_MESSAGE, nullptr, g_hInst, nullptr);

    OpenFromArgs(path, select);
    if (!g_windows.empty()) RunMessageLoop();

    IconWorker::Get().Stop();
    ShutdownUi();
    OleUninitialize();
    if (mutex) CloseHandle(mutex);
    return 0;
}
