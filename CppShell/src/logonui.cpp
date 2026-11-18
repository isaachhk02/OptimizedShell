// logonui.cpp - Pantalla de inicio de sesión / bloqueo de OptimizedShell.
// Pantalla completa, fondo desenfocado, autenticación real y modo demo.
//   --lock           bloquea la sesión actual (no lanza otro shell)
//   --logon          autentica y lanza el shell como el usuario indicado
//   --logon --demo   modo de prueba: no cierra sesión ni lanza procesos
#include "ui.h"

#include <lm.h>
#include <userenv.h>
#include <powrprof.h>
#include <thread>
#include <cstdio>
#include <cwctype>

#pragma comment(lib, "netapi32.lib")
#pragma comment(lib, "userenv.lib")
#pragma comment(lib, "powrprof.lib")

namespace {

using namespace Gdiplus;
using namespace ui;

const Color kText(255, 238, 242, 248);
const Color kTextDim(255, 158, 169, 188);
const Color kTextFaint(255, 112, 124, 146);
const Color kAccent(255, 96, 165, 250);
const Color kRed(255, 252, 165, 165);
const Color kAmber(255, 253, 224, 71);
const Color kGreen(255, 74, 222, 128);

// --------------------------------------------------------------- usuarios
std::vector<std::wstring> EnumLocalUsers() {
    std::vector<std::wstring> users;
    LPUSER_INFO_0 buf = nullptr;
    DWORD read = 0, total = 0, resume = 0;
    NET_API_STATUS st = NetUserEnum(nullptr, 0, FILTER_NORMAL_ACCOUNT, (LPBYTE*)&buf,
                                    MAX_PREFERRED_LENGTH, &read, &total, &resume);
    if (st == NERR_Success && buf) {
        for (DWORD i = 0; i < read; ++i) {
            if (buf[i].usri0_name && buf[i].usri0_name[0] &&
                buf[i].usri0_name[wcslen(buf[i].usri0_name) - 1] != L'$')
                users.push_back(buf[i].usri0_name);
        }
        NetApiBufferFree(buf);
    }

    wchar_t current[256] = {};
    DWORD size = 256;
    if (GetUserNameW(current, &size) && current[0]) {
        bool exists = false;
        for (auto& u : users)
            if (_wcsicmp(u.c_str(), current) == 0) { exists = true; break; }
        if (!exists) users.insert(users.begin(), current);
    }
    std::sort(users.begin(), users.end(), [](const std::wstring& a, const std::wstring& b) {
        return _wcsicmp(a.c_str(), b.c_str()) < 0;
    });
    return users;
}

std::wstring InitialOf(const std::wstring& s) {
    for (wchar_t c : s) {
        if (iswalnum(c)) {
            wchar_t up[2] = { (wchar_t)towupper(c), 0 };
            return up;
        }
    }
    return L"?";
}

std::wstring AccountDomain(const std::wstring& username, std::wstring& userOnly) {
    size_t bs = username.find(L'\\');
    if (bs != std::wstring::npos) {
        userOnly = username.substr(bs + 1);
        return username.substr(0, bs);
    }
    userOnly = username;
    if (username.find(L'@') != std::wstring::npos) return L"MicrosoftAccount";
    return L".";
}

std::wstring AuthErrorText(DWORD err) {
    switch (err) {
    case ERROR_LOGON_FAILURE: return L"El usuario o la contraseña no son correctos.";
    case ERROR_ACCOUNT_RESTRICTION: return L"La cuenta tiene restricciones de inicio de sesión.";
    case ERROR_ACCOUNT_DISABLED: return L"La cuenta está deshabilitada.";
    case ERROR_ACCOUNT_LOCKED_OUT: return L"La cuenta está bloqueada.";
    case ERROR_PASSWORD_EXPIRED: return L"La contraseña ha caducado.";
    case ERROR_PASSWORD_MUST_CHANGE: return L"Debes cambiar la contraseña antes de iniciar sesión.";
    case ERROR_LOGON_TYPE_NOT_GRANTED: return L"El tipo de inicio de sesión no está permitido.";
    case ERROR_LOGON_NOT_GRANTED: return L"No tienes permiso para iniciar sesión en este equipo.";
    case ERROR_NO_LOGON_SERVERS: return L"No se pudo contactar con el servidor de inicio de sesión.";
    case ERROR_ACCOUNT_EXPIRED: return L"La cuenta ha caducado.";
    default: {
        wchar_t buf[160] = {};
        swprintf_s(buf, L"No se pudo iniciar sesión (error %lu).", err);
        return buf;
    }
    }
}

bool LaunchShellAsUser(const std::wstring& username, const std::wstring& password) {
    std::wstring userOnly;
    std::wstring domain = AccountDomain(username, userOnly);
    std::wstring cmd = L"\"" + GetOwnPath() + L"\"";

    STARTUPINFOW si = {};
    si.cb = sizeof(si);
    si.lpDesktop = const_cast<LPWSTR>(L"winsta0\\default");
    PROCESS_INFORMATION pi = {};

    if (CreateProcessWithLogonW(userOnly.c_str(), domain.c_str(), password.c_str(),
                                LOGON_WITH_PROFILE, nullptr, cmd.data(),
                                CREATE_UNICODE_ENVIRONMENT, nullptr, nullptr, &si, &pi)) {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        return true;
    }

    HANDLE token = nullptr;
    if (!LogonUserW(userOnly.c_str(), domain.c_str(), password.c_str(),
                    LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, &token))
        return false;

    LPVOID env = nullptr;
    CreateEnvironmentBlock(&env, token, FALSE);
    BOOL ok = CreateProcessAsUserW(token, nullptr, cmd.data(), nullptr, nullptr, FALSE,
                                   CREATE_UNICODE_ENVIRONMENT, env, nullptr, &si, &pi);
    if (env) DestroyEnvironmentBlock(env);
    CloseHandle(token);
    if (!ok) return false;
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    return true;
}

// Bloquea atajos que permitirían eludir la pantalla de bloqueo.
HHOOK g_kbHook = nullptr;

LRESULT CALLBACK LockKeyboardProc(int nCode, WPARAM wp, LPARAM lp) {
    if (nCode == HC_ACTION && (wp == WM_KEYDOWN || wp == WM_SYSKEYDOWN)) {
        auto* k = (KBDLLHOOKSTRUCT*)lp;
        bool alt = (GetAsyncKeyState(VK_MENU) & 0x8000) != 0;
        bool ctrl = (GetAsyncKeyState(VK_CONTROL) & 0x8000) != 0;
        bool shift = (GetAsyncKeyState(VK_SHIFT) & 0x8000) != 0;
        if (k->vkCode == VK_LWIN || k->vkCode == VK_RWIN || k->vkCode == VK_APPS) return 1;
        if (alt && (k->vkCode == VK_TAB || k->vkCode == VK_ESCAPE || k->vkCode == VK_F4)) return 1;
        if (ctrl && shift && k->vkCode == VK_ESCAPE) return 1;
        if (ctrl && k->vkCode == VK_ESCAPE) return 1;
    }
    return CallNextHookEx(g_kbHook, nCode, wp, lp);
}

struct AuthResult {
    BOOL ok = FALSE;
    DWORD error = 0;
    HANDLE token = nullptr;
};

// --------------------------------------------------------------- ventana
class LogonWindow : public WindowBase {
public:
    LogonWindow(bool demo, bool lockMode) : demo(demo), lockMode(lockMode) {}

    bool Start() {
        int x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        int y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        int w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        int h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if (w <= 0) w = GetSystemMetrics(SM_CXSCREEN);
        if (h <= 0) h = GetSystemMetrics(SM_CYSCREEN);

        if (!Create(L"OShellCppLogon", WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                    WS_POPUP | WS_VISIBLE, x, y, w, h, L"OptimizedShell - Inicio de sesión"))
            return false;

        users = EnumLocalUsers();
        wchar_t current[256] = {};
        DWORD size = 256;
        if (GetUserNameW(current, &size)) username = current;
        if (users.empty()) users.push_back(username.empty() ? L"Usuario" : username);
        if (username.empty()) username = users[0];

        BuildBackground(w, h);
        SetTimer(hwnd, 1, 1000, nullptr);
        SetTimer(hwnd, 2, 530, nullptr);
        SetTimer(hwnd, 3, 2000, nullptr);
        UpdateClock();

        if (!demo)
            g_kbHook = SetWindowsHookExW(WH_KEYBOARD_LL, LockKeyboardProc,
                                          GetModuleHandleW(nullptr), 0);

        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        Invalidate();
        return true;
    }

    ~LogonWindow() override {
        delete background;
    }

protected:
    bool demo = false;
    bool lockMode = false;
    std::vector<std::wstring> users;
    std::wstring username;
    std::wstring password;
    std::wstring passwordAttempt;
    std::wstring lastUser;
    std::wstring error;
    std::wstring timeText;
    std::wstring dateText;
    std::wstring hostName;

    enum Field { None, UserField, PassField };
    Field active = PassField;
    bool caretOn = true;
    bool busy = false;
    bool success = false;
    bool userMenuOpen = false;

    Bitmap* background = nullptr;
    int hoverLogin = 0;
    int hoverPower = -1;
    bool hoverAvatar = false;
    bool hoverChange = false;

    struct Layout {
        RectF card, avatar, user, change, pass, message, login;
        RectF power[3];
        RectF hint;
    } L;

    // ----------------------------------------------------------- fondo
    void BuildBackground(int w, int h) {
        delete background;
        background = new Bitmap(w, h, PixelFormat32bppPARGB);
        Graphics g(background);
        g.SetSmoothingMode(SmoothingModeAntiAlias);

        Bitmap* blur = CreateBlurredWallpaper(w, h, 22);
        if (blur) {
            g.SetInterpolationMode(InterpolationModeHighQualityBicubic);
            g.SetPixelOffsetMode(PixelOffsetModeHighQuality);
            g.DrawImage(blur, 0, 0, w, h);
            delete blur;
        } else {
            LinearGradientBrush base(Rect(0, 0, w, h), C(255, 18, 27, 48),
                                     C(255, 5, 8, 16), LinearGradientModeVertical);
            g.FillRectangle(&base, 0, 0, w, h);
        }

        LinearGradientBrush overlay(Rect(0, 0, w, h), C(120, 5, 8, 18),
                                    C(232, 3, 5, 11), LinearGradientModeVertical);
        g.FillRectangle(&overlay, 0, 0, w, h);

        // Resplandores suaves
        SolidBrush glow1(C(30, 59, 130, 246));
        g.FillEllipse(&glow1, (float)(w * 0.52), (float)(-h * 0.28), (float)(w * 0.62), (float)(h * 0.9));
        SolidBrush glow2(C(18, 139, 92, 246));
        g.FillEllipse(&glow2, (float)(-w * 0.18), (float)(h * 0.45), (float)(w * 0.55), (float)(h * 0.8));

        // Viñeta inferior y superior
        LinearGradientBrush top(Rect(0, 0, w, h / 3), C(150, 0, 0, 0), C(0, 0, 0, 0),
                                LinearGradientModeVertical);
        g.FillRectangle(&top, 0, 0, w, h / 3);
        LinearGradientBrush bottom(Rect(0, h * 2 / 3, w, h / 3), C(0, 0, 0, 0), C(160, 0, 0, 0),
                                   LinearGradientModeVertical);
        g.FillRectangle(&bottom, 0, h * 2 / 3, w, h / 3);
    }

    void UpdateClock() {
        SYSTEMTIME st;
        GetLocalTime(&st);
        wchar_t buf[96] = {};
        if (GetTimeFormatW(LOCALE_NAME_USER_DEFAULT, TIME_NOSECONDS, &st, nullptr, buf, 96) > 0)
            timeText = buf;
        if (GetDateFormatW(LOCALE_NAME_USER_DEFAULT, DATE_LONGDATE, &st, nullptr, buf, 96) > 0)
            dateText = buf;
        if (hostName.empty()) {
            wchar_t host[256] = {};
            DWORD size = 256;
            if (GetComputerNameW(host, &size)) hostName = host;
        }
        Invalidate();
    }

    // ----------------------------------------------------------- layout
    void LayoutAll(int w, int h) {
        float cardW = (float)S(420);
        float cardH = (float)S(412);
        float cx = (float)w / 2.0f;
        float cardY = std::max((float)S(220), ((float)h - cardH) / 2.0f + (float)S(26));
        L.card = RectF(cx - cardW / 2.0f, cardY, cardW, cardH);
        L.avatar = RectF(cx - S(38), cardY + S(22), S(76), S(76));
        L.user = RectF(L.card.X + S(40), cardY + S(112), cardW - S(80), (float)S(44));
        L.change = RectF(L.card.X + S(40), cardY + S(158), cardW - S(80), (float)S(18));
        L.pass = RectF(L.card.X + S(40), cardY + S(182), cardW - S(80), (float)S(44));
        L.message = RectF(L.card.X + S(40), cardY + S(230), cardW - S(80), (float)S(20));
        L.login = RectF(L.card.X + S(40), cardY + S(254), cardW - S(80), (float)S(46));
        L.hint = RectF(L.card.X + S(24), cardY + cardH - S(46), cardW - S(48), (float)S(20));

        float pw = (float)S(96), ph = (float)S(30), gap = (float)S(8);
        float py = (float)h - S(50);
        for (int i = 0; i < 3; ++i)
            L.power[i] = RectF((float)w - S(18) - (pw + gap) * (3 - i) + gap, py, pw, ph);
    }

    // ----------------------------------------------------------- pintura
    void DrawField(Graphics& g, const RectF& rc, const std::wstring& value,
                   const std::wstring& placeholder, bool isPassword, bool focused) {
        DrawRounded(g, rc, SF(7), C(255, 30, 35, 46),
                    focused ? kAccent : C(55, 255, 255, 255), focused ? 1.3f : 1.0f);
        std::wstring shown = value;
        if (isPassword && !value.empty()) shown.assign(value.size(), L'\u25CF');

        g.SetClip(rc);
        Color fg = value.empty() ? kTextFaint : RGBc(246, 248, 252);
        std::wstring text = value.empty() ? placeholder : shown;
        float tx = rc.X + SF(13);
        float textW = 0;
        if (!text.empty()) {
            RectF measured;
            g.MeasureString(text.c_str(), (INT)text.size(), fontBody, PointF(0, 0), &measured);
            textW = measured.Width;
            float avail = rc.Width - SF(26);
            if (textW > avail) tx = rc.X + rc.Width - SF(13) - textW;
        }
        DrawTextStr(g, text, fontBody, fg,
                    RectF(tx, rc.Y, textW + SF(30), rc.Height));
        if (focused && caretOn && !busy) {
            float cx = tx + (value.empty() ? 0 : textW) + SF(1);
            float maxX = rc.X + rc.Width - SF(9);
            if (cx > maxX) cx = maxX;
            Pen p(kAccent, 1.4f);
            g.DrawLine(&p, cx, rc.Y + SF(10), cx, rc.Y + rc.Height - SF(10));
        }
        g.ResetClip();
    }

    void DrawAvatar(Graphics& g, float cx, float cy, float r, const std::wstring& name) {
        LinearGradientBrush grad(PointF(cx - r, cy - r), PointF(cx + r, cy + r),
                                 C(255, 96, 165, 250), C(255, 37, 99, 235));
        g.FillEllipse(&grad, cx - r, cy - r, r * 2, r * 2);
        DrawCircle(g, cx, cy, r, hoverAvatar ? C(180, 255, 255, 255) : C(90, 255, 255, 255), 1.4f);
        DrawTextStr(g, InitialOf(name), fontAvatar,
                    RGBc(255, 255, 255),
                    RectF(cx - r, cy - r - SF(2), r * 2, r * 2), StringAlignmentCenter);
    }

    void DrawPower(Graphics& g, int index, const wchar_t* glyph, const wchar_t* label) {
        const RectF& rc = L.power[index];
        bool hov = hoverPower == index;
        DrawRounded(g, rc, SF(6), hov ? C(255, 38, 44, 58) : C(235, 20, 24, 33),
                    C(50, 255, 255, 255));
        DrawTextStr(g, glyph, fontIcon, kTextDim,
                    RectF(rc.X + S(8), rc.Y, S(18), rc.Height), StringAlignmentCenter);
        DrawTextStr(g, label, fontSmall, hov ? kText : kTextDim,
                    RectF(rc.X + S(26), rc.Y, rc.Width - S(32), rc.Height));
    }

    void OnPaint(Graphics& g, int w, int h) override {
        LayoutAll(w, h);
        if (background) g.DrawImage(background, 0, 0, w, h);
        else {
            SolidBrush b(C(255, 10, 14, 24));
            g.FillRectangle(&b, 0, 0, w, h);
        }

        // Reloj y fecha
        DrawTextStr(g, timeText, fontHuge, RGBc(255, 255, 255),
                    RectF(0, (float)S(48), (float)w, (float)S(88)), StringAlignmentCenter);
        DrawTextStr(g, dateText, fontBody, C(230, 205, 214, 230),
                    RectF(0, (float)S(136), (float)w, (float)S(26)), StringAlignmentCenter);

        // Sombra y tarjeta
        DrawRounded(g, RectF(L.card.X + S(2), L.card.Y + S(7), L.card.Width, L.card.Height),
                    SF(16), C(95, 0, 0, 0));
        DrawRounded(g, L.card, SF(16), C(232, 22, 27, 37), C(62, 255, 255, 255));

        if (success) {
            float cx = L.card.X + L.card.Width / 2;
            float cy = L.card.Y + S(154);
            FillCircle(g, cx, cy, S(46), C(255, 30, 58, 46));
            DrawCircle(g, cx, cy, S(46), kGreen, 2.0f);
            DrawCheck(g, cx, cy, S(46), kGreen, 4.0f);
            DrawTextStr(g, L"Bienvenido", fontTitle, kText,
                        RectF(L.card.X, cy + S(58), L.card.Width, S(34)), StringAlignmentCenter);
            DrawTextStr(g, username, fontBody, kTextDim,
                        RectF(L.card.X, cy + S(90), L.card.Width, S(24)), StringAlignmentCenter);
            DrawTextStr(g, demo ? L"Acceso concedido (modo demo)" : L"Iniciando sesión...",
                        fontSmall, kTextFaint,
                        RectF(L.card.X, cy + S(116), L.card.Width, S(22)), StringAlignmentCenter);
            DrawFooter(g, w, h);
            return;
        }

        // Avatar
        DrawAvatar(g, L.avatar.X + L.avatar.Width / 2, L.avatar.Y + L.avatar.Height / 2,
                   L.avatar.Width / 2, username);

        // Campos
        DrawField(g, L.user, username, L"Usuario", false, active == UserField);
        DrawTextStr(g, L"Cambiar usuario", fontSmall, hoverChange ? kAccent : kTextFaint,
                    L.change, StringAlignmentFar);
        DrawField(g, L.pass, password, L"Contraseña", true, active == PassField);

        // Mensajes
        bool caps = (GetKeyState(VK_CAPITAL) & 1) != 0;
        if (!error.empty())
            DrawTextStr(g, error, fontSmall, kRed, L.message);
        else if (caps && active == PassField)
            DrawTextStr(g, L"Bloq Mayús está activado", fontSmall, kAmber, L.message);
        else if (demo)
            DrawTextStr(g, L"Modo demo: escribe cualquier contraseña o pulsa Esc para salir.",
                        fontSmall, kTextFaint, L.message);

        // Botón principal
        {
            bool hov = hoverLogin > 0 && !busy;
            Color c1 = busy ? C(255, 48, 61, 82) : (hov ? C(255, 96, 165, 250) : C(255, 59, 130, 246));
            Color c2 = busy ? C(255, 40, 51, 69) : (hov ? C(255, 59, 130, 246) : C(255, 37, 99, 235));
            LinearGradientBrush grad(PointF(L.login.X, L.login.Y),
                                     PointF(L.login.X + L.login.Width, L.login.Y + L.login.Height),
                                     c1, c2);
            GraphicsPath path;
            float d = SF(7) * 2;
            path.AddArc(L.login.X, L.login.Y, d, d, 180, 90);
            path.AddArc(L.login.X + L.login.Width - d, L.login.Y, d, d, 270, 90);
            path.AddArc(L.login.X + L.login.Width - d, L.login.Y + L.login.Height - d, d, d, 0, 90);
            path.AddArc(L.login.X, L.login.Y + L.login.Height - d, d, d, 90, 90);
            path.CloseFigure();
            g.FillPath(&grad, &path);
            std::wstring label = busy ? L"Comprobando..." : L"Iniciar sesión";
            DrawTextStr(g, label, fontBold, RGBc(255, 255, 255), L.login, StringAlignmentCenter);
            if (busy) {
                float t = (float)((GetTickCount64() / 12) % 360);
                Pen p(RGBc(255, 255, 255), 2.0f);
                g.DrawArc(&p, L.login.X + L.login.Width / 2 - S(60), L.login.Y + S(12),
                          (float)S(22), (float)S(22), t, 250.0f);
            }
        }

        // Pie de tarjeta
        DrawTextStr(g, demo ? L"ESC sale del modo demo · Intro inicia sesión"
                            : L"Intro inicia sesión · Ctrl+Alt+Supr para opciones de seguridad",
                    fontSmall, kTextFaint, L.hint, StringAlignmentCenter);
        DrawFooter(g, w, h);
    }

    void DrawFooter(Graphics& g, int w, int h) {
        std::wstring host = hostName.empty() ? L"Este equipo" : hostName;
        DrawTextStr(g, host + L"  ·  OptimizedShell", fontSmall, C(190, 190, 200, 216),
                    RectF((float)S(20), (float)h - S(38), (float)S(380), (float)S(22)));
        DrawPower(g, 0, L"\uE7E8", L"Apagar");
        DrawPower(g, 1, L"\uE777", L"Reiniciar");
        DrawPower(g, 2, L"\uE708", L"Suspender");
        DrawTextStr(g, L"OptimizedShell LogonUI", fontSmall, C(130, 160, 170, 188),
                    RectF((float)w - S(220), (float)h - S(70), (float)S(200), (float)S(18)),
                    StringAlignmentFar);
        (void)w;
    }

    // ----------------------------------------------------------- acciones
    void ShowUserMenu() {
        if (users.empty() || userMenuOpen) return;
        userMenuOpen = true;
        HMENU menu = CreatePopupMenu();
        for (size_t i = 0; i < users.size(); ++i)
            AppendMenuW(menu, MF_STRING | (_wcsicmp(users[i].c_str(), username.c_str()) == 0 ? MF_CHECKED : 0),
                        100 + (UINT)i, users[i].c_str());
        POINT pt;
        GetCursorPos(&pt);
        SetForegroundWindow(hwnd);
        int cmd = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
                                 pt.x, pt.y, 0, hwnd, nullptr);
        DestroyMenu(menu);
        userMenuOpen = false;
        if (cmd >= 100 && cmd < 100 + (int)users.size()) {
            username = users[cmd - 100];
            password.clear();
            error.clear();
            active = PassField;
            Invalidate();
        }
    }

    void Submit() {
        if (busy || success) return;
        if (username.empty()) {
            error = L"Escribe un nombre de usuario.";
            Invalidate();
            return;
        }
        error.clear();
        busy = true;
        passwordAttempt = password;
        Invalidate();

        if (demo) {
            SetTimer(hwnd, 5, 650, nullptr);
            return;
        }

        std::wstring user = username;
        std::wstring pass = password;
        std::thread([this, user, pass]() {
            std::wstring userOnly;
            std::wstring domain = AccountDomain(user, userOnly);
            HANDLE token = nullptr;
            BOOL ok = LogonUserW(userOnly.c_str(), domain.c_str(), pass.c_str(),
                                 LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, &token);
            DWORD err = ok ? 0 : GetLastError();
            auto* r = new AuthResult{ ok, err, ok ? token : nullptr };
            if (!PostMessageW(hwnd, WM_APP + 10, 0, (LPARAM)r)) delete r;
        }).detach();
    }

    void OnAuthenticated(AuthResult* r) {
        busy = false;
        if (!r->ok) {
            error = AuthErrorText(r->error);
            if (r->token) CloseHandle(r->token);
            delete r;
            active = PassField;
            password.clear();
            Invalidate();
            return;
        }
        if (r->token) CloseHandle(r->token);
        delete r;

        success = true;
        error.clear();
        Invalidate();

        if (demo) {
            SetTimer(hwnd, 6, 1000, nullptr);
        } else if (lockMode) {
            SetTimer(hwnd, 6, 750, nullptr);
        } else if (LaunchShellAsUser(username, passwordAttempt)) {
            SetTimer(hwnd, 6, 850, nullptr);
        } else {
            success = false;
            busy = false;
            error = L"Las credenciales son válidas, pero no se pudo iniciar el escritorio.";
            Invalidate();
        }
    }

    void DoPower(int index) {
        if (index == 0) {
            if (MessageBoxW(hwnd, L"¿Apagar el equipo?", L"Energía",
                            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2) == IDYES)
                ExitWindowsEx(EWX_POWEROFF | EWX_FORCEIFHUNG, 0);
        } else if (index == 1) {
            if (MessageBoxW(hwnd, L"¿Reiniciar el equipo?", L"Energía",
                            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2) == IDYES)
                ExitWindowsEx(EWX_REBOOT | EWX_FORCEIFHUNG, 0);
        } else {
            SetSuspendState(FALSE, FALSE, FALSE);
        }
    }

    // ----------------------------------------------------------- eventos
    void OnCreate() override {}

    void OnDestroy() override {
        KillTimer(hwnd, 1);
        KillTimer(hwnd, 2);
        KillTimer(hwnd, 3);
        KillTimer(hwnd, 5);
        KillTimer(hwnd, 6);
        if (g_kbHook) {
            UnhookWindowsHookEx(g_kbHook);
            g_kbHook = nullptr;
        }
        PostQuitMessage(0);
    }

    void OnTimer(UINT_PTR id) override {
        if (id == 1) {
            UpdateClock();
        } else if (id == 2) {
            if (active != None && !success) {
                caretOn = !caretOn;
                Invalidate();
            }
        } else if (id == 3) {
            if (!success)
                SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0,
                             SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        } else if (id == 5) {
            KillTimer(hwnd, 5);
            success = true;
            busy = false;
            Invalidate();
            SetTimer(hwnd, 6, 1000, nullptr);
        } else if (id == 6) {
            KillTimer(hwnd, 6);
            Close();
        }
    }

    void OnMouseDown(int x, int y) override {
        if (success) return;

        if (InRect(L.avatar, x, y) || InRect(L.change, x, y)) {
            ShowUserMenu();
            return;
        }
        if (InRect(L.user, x, y)) {
            active = UserField;
            caretOn = true;
            Invalidate();
            return;
        }
        if (InRect(L.pass, x, y)) {
            active = PassField;
            caretOn = true;
            Invalidate();
            return;
        }
        if (InRect(L.login, x, y)) {
            Submit();
            return;
        }
        for (int i = 0; i < 3; ++i) {
            if (InRect(L.power[i], x, y)) {
                DoPower(i);
                return;
            }
        }
        active = None;
        Invalidate();
    }

    void OnMouseMove(int x, int y) override {
        int newLogin = InRect(L.login, x, y) ? 1 : 0;
        int newPower = -1;
        for (int i = 0; i < 3; ++i)
            if (InRect(L.power[i], x, y)) newPower = i;
        bool newAvatar = InRect(L.avatar, x, y);
        bool newChange = InRect(L.change, x, y);

        bool hand = newLogin || newPower >= 0 || newAvatar || newChange ||
                    InRect(L.user, x, y) || InRect(L.pass, x, y);
        SetCursor(LoadCursorW(nullptr, hand ? IDC_HAND : IDC_ARROW));

        if (newLogin != hoverLogin || newPower != hoverPower ||
            newAvatar != hoverAvatar || newChange != hoverChange) {
            hoverLogin = newLogin;
            hoverPower = newPower;
            hoverAvatar = newAvatar;
            hoverChange = newChange;
            Invalidate();
        }
    }

    void OnKeyDown(int vk) override {
        if (success) return;
        if (vk == VK_ESCAPE) {
            if (demo) Close();
            return;
        }
        if (busy) return;

        if (vk == VK_RETURN) {
            Submit();
        } else if (vk == VK_TAB) {
            active = (active == UserField) ? PassField : UserField;
            caretOn = true;
            Invalidate();
        } else if (vk == VK_BACK) {
            std::wstring& field = (active == UserField) ? username : password;
            if (!field.empty()) field.pop_back();
            error.clear();
            caretOn = true;
            Invalidate();
        } else if (vk == VK_UP || vk == VK_DOWN) {
            if (!users.empty()) {
                int idx = -1;
                for (size_t i = 0; i < users.size(); ++i)
                    if (_wcsicmp(users[i].c_str(), username.c_str()) == 0) idx = (int)i;
                if (idx < 0) idx = 0;
                idx = (idx + (vk == VK_DOWN ? 1 : -1) + (int)users.size()) % (int)users.size();
                username = users[idx];
                password.clear();
                Invalidate();
            }
        } else if (vk == VK_CAPITAL) {
            Invalidate();
        }
    }

    void OnChar(wchar_t ch) override {
        if (success || busy) return;
        if (ch < 32 || ch == 127) return;
        if (active == None) active = PassField;
        std::wstring& field = (active == UserField) ? username : password;
        size_t limit = (active == UserField) ? 256 : 128;
        if (field.size() < limit) {
            field.push_back(ch);
            error.clear();
            caretOn = true;
            Invalidate();
            SetTimer(hwnd, 2, 530, nullptr);
        }
    }

    bool CanClose() override { return demo; }

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        if (msg == WM_APP + 10) {
            OnAuthenticated((AuthResult*)lp);
            return 0;
        }
        if (msg == WM_SYSCOMMAND) {
            UINT cmd = (UINT)(wp & 0xFFF0);
            if (cmd == SC_CLOSE || cmd == SC_MINIMIZE) return 0;
        }
        if (msg == WM_SYSKEYDOWN) {
            // Alt+F4 y similares
            if (wp == VK_F4 && (GetKeyState(VK_MENU) & 0x8000)) return 0;
        }
        return WindowBase::OnMessage(msg, wp, lp);
    }
};

} // namespace

int RunLogonUIMode(bool demo, bool lockMode) {
    ui::InitUi(GetModuleHandleW(nullptr));

    int result = 0;
    {
        LogonWindow wnd(demo, lockMode);
        if (!wnd.Start()) {
            ui::ShutdownUi();
            MessageBoxW(nullptr, L"No se pudo abrir la pantalla de inicio de sesión.",
                        L"OptimizedShell", MB_ICONERROR | MB_OK);
            return 1;
        }
        MSG msg;
        while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    ui::ShutdownUi();
    return result;
}
