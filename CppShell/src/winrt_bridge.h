// winrt_bridge.h - funciones de Windows Runtime (ubicación y Bluetooth) expuestas
// como C++ normal, para no arrastrar las cabeceras de C++/WinRT al resto del código.
// Todas son bloqueantes: llámalas desde un hilo de trabajo, nunca desde el de la UI.
#pragma once
#include <string>
#include <vector>

namespace wrt {

enum class Access { Allowed, Denied, Unspecified, Error };

// Pide a Windows permiso de ubicación (necesario desde Windows 11 24H2 para que
// una aplicación de escritorio vea las redes Wi-Fi). Puede mostrar un aviso del sistema.
Access RequestLocation();

// -1 = no hay adaptador, 0 = apagado, 1 = encendido
int BluetoothRadioState();
bool SetBluetoothRadio(bool on);

struct BtDevice {
    std::wstring id;
    std::wstring name;
    std::wstring kind; // "audio", "input", "phone", "computer", "other"
    bool connected = false;
    bool lowEnergy = false;
};

std::vector<BtDevice> PairedBluetoothDevices();
bool UnpairBluetooth(const std::wstring& id);

} // namespace wrt
