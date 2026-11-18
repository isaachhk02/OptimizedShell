// winrt_bridge.cpp - implementación con C++/WinRT de winrt_bridge.h.
// Cada operación se ejecuta en un apartamento MTA propio: así .get() puede
// esperar a las operaciones asíncronas sin bloquear (ni romper) un hilo STA.
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <unknwn.h>
#include <thread>
#include <set>

#include <winrt/base.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.Devices.Geolocation.h>
#include <winrt/Windows.Devices.Radios.h>
#include <winrt/Windows.Devices.Enumeration.h>
#include <winrt/Windows.Devices.Bluetooth.h>

#include "winrt_bridge.h"

#pragma comment(lib, "windowsapp.lib")

using namespace winrt;
using namespace winrt::Windows::Devices;

namespace {

template <class R, class F>
R RunMta(R fallback, F&& fn) {
    R result = fallback;
    std::thread t([&] {
        try {
            init_apartment(apartment_type::multi_threaded);
            result = fn();
        } catch (...) {
            result = fallback;
        }
        uninit_apartment();
    });
    t.join();
    return result;
}

Radios::Radio FindBluetoothRadio() {
    for (auto const& r : Radios::Radio::GetRadiosAsync().get())
        if (r.Kind() == Radios::RadioKind::Bluetooth) return r;
    return nullptr;
}

std::wstring KindFromClass(Bluetooth::BluetoothMajorClass c) {
    switch (c) {
    case Bluetooth::BluetoothMajorClass::AudioVideo: return L"audio";
    case Bluetooth::BluetoothMajorClass::Peripheral: return L"input";
    case Bluetooth::BluetoothMajorClass::Phone: return L"phone";
    case Bluetooth::BluetoothMajorClass::Computer: return L"computer";
    default: return L"other";
    }
}

std::wstring KindFromAppearance(uint16_t category) {
    // Categorías GAP: 0x0F HID, 0x29/0x2A audio (auriculares/altavoces), 0x01 teléfono, 0x02 equipo
    switch (category) {
    case 0x0F: return L"input";
    case 0x29: case 0x2A: case 0x2B: return L"audio";
    case 0x01: return L"phone";
    case 0x02: return L"computer";
    default: return L"other";
    }
}

} // namespace

namespace wrt {

Access RequestLocation() {
    return RunMta(Access::Error, [] {
        auto status = Geolocation::Geolocator::RequestAccessAsync().get();
        switch (status) {
        case Geolocation::GeolocationAccessStatus::Allowed: return Access::Allowed;
        case Geolocation::GeolocationAccessStatus::Denied: return Access::Denied;
        default: return Access::Unspecified;
        }
    });
}

int BluetoothRadioState() {
    return RunMta(-1, [] {
        auto r = FindBluetoothRadio();
        if (!r) return -1;
        return r.State() == Radios::RadioState::On ? 1 : 0;
    });
}

bool SetBluetoothRadio(bool on) {
    return RunMta(false, [on] {
        if (Radios::Radio::RequestAccessAsync().get() != Radios::RadioAccessStatus::Allowed) return false;
        auto r = FindBluetoothRadio();
        if (!r) return false;
        auto st = r.SetStateAsync(on ? Radios::RadioState::On : Radios::RadioState::Off).get();
        return st == Radios::RadioAccessStatus::Allowed;
    });
}

std::vector<BtDevice> PairedBluetoothDevices() {
    return RunMta(std::vector<BtDevice>(), [] {
        std::vector<BtDevice> out;
        std::set<std::wstring> names;
        // Bluetooth clásico (auriculares, teclados, teléfonos...)
        for (auto const& di : Enumeration::DeviceInformation::FindAllAsync(
                 Bluetooth::BluetoothDevice::GetDeviceSelectorFromPairingState(true)).get()) {
            BtDevice d;
            d.id = di.Id().c_str();
            d.name = di.Name().c_str();
            try {
                if (auto dev = Bluetooth::BluetoothDevice::FromIdAsync(di.Id()).get()) {
                    d.connected = dev.ConnectionStatus() == Bluetooth::BluetoothConnectionStatus::Connected;
                    d.kind = KindFromClass(dev.ClassOfDevice().MajorClass());
                }
            } catch (...) {
            }
            if (d.kind.empty()) d.kind = L"other";
            names.insert(d.name);
            out.push_back(d);
        }
        // Bluetooth de bajo consumo (ratones, relojes, auriculares LE...)
        for (auto const& di : Enumeration::DeviceInformation::FindAllAsync(
                 Bluetooth::BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)).get()) {
            BtDevice d;
            d.id = di.Id().c_str();
            d.name = di.Name().c_str();
            d.lowEnergy = true;
            if (names.count(d.name)) continue; // mismo dispositivo con perfil clásico y LE
            try {
                if (auto dev = Bluetooth::BluetoothLEDevice::FromIdAsync(di.Id()).get()) {
                    d.connected = dev.ConnectionStatus() == Bluetooth::BluetoothConnectionStatus::Connected;
                    d.kind = KindFromAppearance(dev.Appearance().Category());
                }
            } catch (...) {
            }
            if (d.kind.empty()) d.kind = L"other";
            out.push_back(d);
        }
        return out;
    });
}

bool UnpairBluetooth(const std::wstring& id) {
    return RunMta(false, [&id] {
        auto di = Enumeration::DeviceInformation::CreateFromIdAsync(hstring(id)).get();
        auto r = di.Pairing().UnpairAsync().get();
        return r.Status() == Enumeration::DeviceUnpairingResultStatus::Unpaired ||
               r.Status() == Enumeration::DeviceUnpairingResultStatus::AlreadyUnpaired;
    });
}

} // namespace wrt
