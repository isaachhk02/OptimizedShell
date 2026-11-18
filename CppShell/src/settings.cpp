// settings.cpp - Personalización de OptimizedShell: modo claro/oscuro, color de
// acento y transparencia. Se ejecuta con: OptimizedShellCpp.exe --settings
//
// Los cambios se guardan en %APPDATA%\OptimizedShell\theme.ini y se difunden al
// momento a todas las ventanas de OptimizedShell (shell, explorador, Administrador
// de tareas...), que se repintan con el nuevo tema sin reiniciarse.
#include "ui.h"

int RunSettingsMode();

namespace {

using namespace Gdiplus;
using namespace ui;

constexpr int kPresetCount = (int)(sizeof(kAccentPresets) / sizeof(kAccentPresets[0]));

enum Hit {
    HNone = 0, HMin, HMax, HClose,
    HDark = 10, HLight, HWindowsAccent, HTransparency, HReset,
    HSwatch0 = 100,
};

class SettingsWindow : public WindowBase {
public:
    bool Start() {
        customFrame = true;
        int w = S(620), h = S(600);
        RECT wa = {};
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &wa, 0);
        int x = wa.left + (wa.right - wa.left - w) / 2;
        int y = wa.top + (wa.bottom - wa.top - h) / 2;
        if (!Create(L"OShellCppSettings", WS_EX_APPWINDOW, WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX, x, y, w, h,
                    L"Personalización"))
            return false;
        SetFrame(FrameOpaque);
        SetWindowPos(hwnd, nullptr, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        tip.Create(hwnd);
        Layout(w, h);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        ForceForeground(hwnd);
        return true;
    }

protected:
    RectF rc[128];
    int hover = HNone;
    HoverTip tip;

    static constexpr int kCaption = 42;

    void Layout(int w, int) {
        auto ch = style::ChromeRects(w);
        rc[HMin] = ch.min;
        rc[HMax] = RectF();
        rc[HClose] = ch.close;
        float x0 = (float)S(28), cw = w - x0 * 2;
        float cardW = (cw - S(16)) / 2;
        rc[HDark] = RectF(x0, (float)S(96), cardW, (float)S(156));
        rc[HLight] = RectF(x0 + cardW + S(16), (float)S(96), cardW, (float)S(156));
        float sw = (float)S(36), gap = (float)S(10);
        for (int i = 0; i < kPresetCount; ++i) rc[HSwatch0 + i] = RectF(x0 + i * (sw + gap), (float)S(304), sw, sw);
        rc[HWindowsAccent] = RectF(x0, (float)S(356), (float)S(236), (float)S(34));
        rc[HTransparency] = RectF(x0, (float)S(438), cw, (float)S(64));
        rc[HReset] = RectF(w - x0 - S(150), (float)S(540), (float)S(150), (float)S(34));
    }

    void OnSize(UINT, int w, int h) override { Layout(w, h); }

    void SectionTitle(Graphics& g, const wchar_t* t, float y, int w) {
        DrawTextStr(g, t, fontBold, style::text, RectF((float)S(28), y, (float)w - S(56), (float)S(24)));
    }

    // Miniatura de una ventana con los colores de cada modo.
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
            DrawRounded(g, RectF(p.X + S(12), p.Y + S(30) + i * S(14), p.Width * (i == 1 ? 0.45f : 0.6f), (float)S(6)),
                        SF(3), line);
        DrawRounded(g, RectF(p.X + S(12), p.Y + S(30) + 1 * S(14), p.Width * 0.3f, (float)S(6)), SF(3),
                    style::accentSolid);
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

    void OnPaint(Graphics& g, int w, int h) override {
        style::DrawChrome(g, w, h, L"Personalización", L"", false, hover <= HClose ? hover : 0);
        // Sin maximizar: tapar su botón.
        auto ch = style::ChromeRects(w);
        FillRect(g, ch.max, style::titleBg);

        SectionTitle(g, L"Modo", (float)S(62), w);
        ModeCard(g, HDark, true, L"Oscuro");
        ModeCard(g, HLight, false, L"Claro");

        SectionTitle(g, L"Color de acento", (float)S(270), w);
        for (int i = 0; i < kPresetCount; ++i) {
            const RectF& r = rc[HSwatch0 + i];
            bool sel = g_theme.accent == kAccentPresets[i].color;
            float cx = r.X + r.Width / 2, cy = r.Y + r.Height / 2;
            if (sel || hover == HSwatch0 + i)
                DrawCircle(g, cx, cy, r.Width / 2 + SF(3), sel ? style::text : style::textFaint, sel ? SF(2) : 1.0f);
            FillCircle(g, cx, cy, r.Width / 2 - SF(2), FromRef(kAccentPresets[i].color));
            if (sel) {
                Color mark = FromRef(kAccentPresets[i].color);
                float lum = (0.299f * mark.GetR() + 0.587f * mark.GetG() + 0.114f * mark.GetB()) / 255.0f;
                DrawCheck(g, cx, cy, r.Width * 0.4f, lum > 0.68f ? RGBc(20, 20, 24) : RGBc(255, 255, 255), SF(2));
            }
        }
        bool custom = true;
        for (int i = 0; i < kPresetCount; ++i)
            if (g_theme.accent == kAccentPresets[i].color) custom = false;
        style::DrawButton(g, rc[HWindowsAccent], L"Usar el color de Windows", hover == HWindowsAccent);
        if (custom)
            DrawTextStr(g, L"Color personalizado en uso", fontSmall, style::textFaint,
                        RectF(rc[HWindowsAccent].X + rc[HWindowsAccent].Width + S(14), rc[HWindowsAccent].Y, (float)S(240),
                              rc[HWindowsAccent].Height));

        SectionTitle(g, L"Efectos", (float)S(410), w);
        const RectF& t = rc[HTransparency];
        DrawRounded(g, t, SF(8), hover == HTransparency ? style::controlHover : style::cardBg, style::controlBorder);
        DrawTextStr(g, L"Transparencia", fontBody, style::text, RectF(t.X + S(16), t.Y + S(10), t.Width - S(100), (float)S(22)));
        DrawTextStr(g, L"Fondo acrílico en la barra de tareas y los paneles. Desactivarla ahorra GPU.", fontSmall,
                    style::textDim, RectF(t.X + S(16), t.Y + S(32), t.Width - S(100), (float)S(20)));
        style::DrawSwitch(g, RectF(t.X + t.Width - S(62), t.Y + (t.Height - S(22)) / 2, (float)S(44), (float)S(22)),
                          g_theme.transparency, hover == HTransparency);

        DrawTextV(g, L"Los cambios se aplican al momento en todas las ventanas de OptimizedShell.", fontSmall,
                  style::textFaint, RectF((float)S(28), (float)S(546), (float)w - S(220), (float)S(36)), StringAlignmentNear, true);
        style::DrawButton(g, rc[HReset], L"Restablecer", hover == HReset);
    }

    int HitAt(int x, int y) const {
        for (int id : { HMin, HClose, HDark, HLight, HWindowsAccent, HTransparency, HReset })
            if (rc[id].Width > 0 && InRect(rc[id], x, y)) return id;
        for (int i = 0; i < kPresetCount; ++i)
            if (InRect(rc[HSwatch0 + i], x, y)) return HSwatch0 + i;
        return HNone;
    }

    void Apply(const ThemeSettings& t) {
        SetTheme(t); // guarda, aplica aquí y avisa a las demás ventanas
        Invalidate();
    }

    void OnMouseDown(int x, int y) override {
        ThemeSettings t = g_theme;
        int id = HitAt(x, y);
        switch (id) {
        case HMin: ShowWindow(hwnd, SW_MINIMIZE); return;
        case HClose: Close(); return;
        case HDark: t.dark = true; break;
        case HLight: t.dark = false; break;
        case HWindowsAccent: t.accent = WindowsAccent(); break;
        case HTransparency: t.transparency = !t.transparency; break;
        case HReset: t = ThemeSettings(); break;
        default:
            if (id >= HSwatch0 && id < HSwatch0 + kPresetCount) t.accent = kAccentPresets[id - HSwatch0].color;
            else return;
        }
        if (!(t == g_theme)) Apply(t);
    }

    void OnMouseMove(int x, int y) override {
        int id = HitAt(x, y);
        if (id == hover) return;
        hover = id;
        Invalidate();
        std::wstring t;
        if (id >= HSwatch0 && id < HSwatch0 + kPresetCount) t = kAccentPresets[id - HSwatch0].name;
        tip.Set(id ? rc[id] : RectF(), t);
        if (id) SetCursor(LoadCursorW(nullptr, IDC_HAND));
    }

    void OnKeyDown(int vk) override {
        if (vk == VK_ESCAPE) Close();
    }

    // Tamaño fijo: solo la barra de título (sin bordes de redimensión).
    LRESULT OnHitTest(int x, int y) override {
        if (y >= S(kCaption) || InRect(rc[HMin], x, y) || InRect(rc[HClose], x, y)) return HTCLIENT;
        return HTCAPTION;
    }

    void OnDestroy() override { PostQuitMessage(0); }

    LRESULT OnMessage(UINT msg, WPARAM wp, LPARAM lp) override {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
};

} // namespace

int RunSettingsMode() {
    if (HWND existing = FindWindowW(L"OShellCppSettings", nullptr)) {
        if (IsIconic(existing)) ShowWindow(existing, SW_RESTORE);
        SetForegroundWindow(existing);
        return 0;
    }
    InitUi(GetModuleHandleW(nullptr));
    {
        SettingsWindow w;
        if (w.Start()) RunMessageLoop();
    }
    ShutdownUi();
    return 0;
}
