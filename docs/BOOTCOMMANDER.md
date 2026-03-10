# BootCommander for F1 Wideband Flashing

BootCommander (from [OpenBLT](https://github.com/feaser/openblt)) is required to flash firmware on F1 (STM32F103) wideband boards via XCP over CAN or RS232. The DERMO wideband plugin uses it when you click "Flash via BootCommander (F1)...".

## Getting BootCommander

### Option 1: Build with the provided script (recommended)

**On Linux** (native build):

```bash
# Install deps (Debian/Ubuntu)
sudo apt-get install cmake build-essential libusb-1.0-0-dev

./scripts/build-bootcommander.sh
# Binary: ./dist/BootCommander
```

**On macOS** (native build; uses Darwin port in `ext/openblt`):

```bash
# Requires: cmake, libusb (brew install libusb)
./scripts/build-bootcommander.sh
# Binary: ./dist/BootCommander
# Note: xcp_rs232 (serial), xcp_usb, xcp_net work. xcp_can not supported on macOS.
```

### Option 2: Build from OpenBLT source manually

1. Clone [OpenBLT](https://github.com/feaser/openblt)
2. Build LibOpenBLT and BootCommander (see OpenBLT docs)
3. On Linux: build natively. On macOS/Windows: use a Linux VM or Docker.

### Option 3: Use a Linux machine or VM

If you have access to a Linux box, build there and copy the `BootCommander` binary (and `libopenblt.so`) to your Mac. Put them in a directory and add it to PATH.

## Using BootCommander with DERMO

Add the directory containing `BootCommander` to your PATH:

```bash
export PATH="/path/to/dist:$PATH"
```

Or copy `BootCommander` and `libopenblt.so` to a directory already in PATH (e.g. `/usr/local/bin` on macOS, though the `.so` typically goes in `/usr/local/lib` or next to the binary).

## Interface names

- **CAN (Linux SocketCAN):** `can0`, `can1`, etc.
- **CAN (Windows PCAN):** `peak_pcanusb`
- **Serial (RS232):** `/dev/ttyUSB0` (Linux), `COM3` (Windows)

For serial CAN adapters (e.g. SLCAN), create a virtual CAN interface first:

```bash
slcand -o -s6 /dev/ttyUSB0 can0
ip link set can0 up
```
