# rusEFI Wideband CAN Protocol Documentation

> Extracted from rusEFI firmware for standalone program development
> Date: 2026-02-28

---

## Table of Contents

1. [Overview](#overview)
2. [CAN Bus Configuration](#can-bus-configuration)
3. [CAN Message IDs](#can-message-ids)
4. [Data Frame Structures](#data-frame-structures)
5. [Timing & Protocol Behavior](#timing--protocol-behavior)
6. [Sensor Types](#sensor-types)
7. [Implementation Guide](#implementation-guide)
8. [Code Examples](#code-examples)

---

## Overview

rusEFI wideband controllers communicate via CAN bus using:
- **Extended 29-bit CAN IDs** for commands (ECU → Controller)
- **Standard 11-bit CAN IDs** for sensor data (Controller → ECU)
- **Little-endian** byte order for multi-byte values
- **100 Hz** data rate (10 ms intervals)

### Protocol Constants

```c
#define WB_ACK               0x727573  // ASCII "rus" - ACK response ID
#define WB_BL_HEADER         0x0EF     // Bootloader command header
#define WB_DATA_BASE_ADDR    0x190     // First sensor data ID
#define WBO_TX_PERIOD_MS     10        // 10ms transmission period
#define RUSEFI_WIDEBAND_VERSION 0xA0   // Current protocol version
```

### Transmit (Wideband → ECU)

| Message | CAN ID | DLC | Rate | Protocol | Enable Flag |
|---------|--------|-----|------|----------|-------------|
| **WidebandStandardData** | `RusEfiBaseId` + 0 | 8 | 100 Hz | RusEfi (little-endian) | `RusEfiTx` |
| **WidebandDiagData** | `RusEfiBaseId` + 1 | 8 | 100 Hz | RusEfi (little-endian) | `RusEfiTxDiag` |
| **AEMNet UEGO** | 0x180 + `AemNetIdOffset` | 8 | 100 Hz | AEMNet (29-bit ext, big-endian) | `AemNetTx` |
| **AEMNet EGT** | 0x0A0305 + `AemNetIdOffset` | 8 | 20 Hz | AEMNet (29-bit ext, big-endian) | `egt[ch].AemNetTx` |
| **Pong (WB_ACK)** | 0x727573 (EID) | 8 | On request | Response to Ping | — |

### Receive (ECU → Wideband)

All received messages use 29-bit extended IDs with header `0xEF`:

| Message | CAN ID | DLC | Purpose |
|---------|--------|-----|---------|
| **Bootloader Enter** | 0xEF00000 | 0 or 1 | Reboot to bootloader |
| **SetIndex** | 0xEF40000 | 1, 2, or 3 | Set CAN base ID for RusEFI format |
| **WidebandControl (ECU Status)** | 0xEF50000 | ≥2 | Battery voltage, heater enable; optional pump gain if DLC ≥ 3 |
| **Ping** | 0xEF60000 | 1 or 2 | Request version/build date; payload = base CAN ID (DLC 1: low byte; DLC 2: [low, high]); controller replies with Pong if it matches |
| **SetSensorType** | 0xEF70000 | ≥2 | Set sensor type (0=LSU4.9, 1=LSU4.2, 2=LSU ADV, 3=FAE LSU4.9) |
| **HeaterConfig** | 0xEF80000 | ≥3 | Heater thresholds and preheat time; stored in flash |

---

## CAN Bus Configuration

| Parameter | Value |
|-----------|-------|
| Bitrate | 500 kbps (typical) |
| Bus Selection | CAN1 or CAN2 (configurable via `widebandOnSecondBus`) |
| Message Format | Extended (29-bit) for commands, Standard (11-bit) for data |
| Category ID | 5 (WBO_SERVICE) |

---

## CAN Message IDs

### ECU → Controller Commands (Extended 29-bit IDs)

Extended ID format: `(WB_BL_HEADER << 4 | opcode) << 16 | extra`

| Command | CAN ID (hex) | DLC | Byte Layout | Description |
|---------|-------------|-----|-------------|-------------|
| Enter Bootloader | `0x0EF00000` | 0-1 | `[index?]` | Reboot controller to bootloader mode |
| Flash Erase | `0x0EF15A5A` | 0 | - | Erase firmware flash area |
| Flash Write | `0x0EF20000 + N` | 8 | `[8 bytes]` | Write 8 bytes at flash offset N |
| Reboot to App | `0x0EF30000` | 0 | - | Exit bootloader, run application |
| SetIndex | `0x0EF40000` | 1, 2, or 3 | See SetIndex section | Set CAN base ID for RusEFI format; persists to flash |
| ECU Status (WidebandControl) | `0x0EF50000` | ≥2 | `[V_batt×10, flags, pumpGain?]` | Battery, heater enable; optional pump gain if DLC≥3 |
| Ping | `0x0EF60000` | 1 or 2 | base CAN ID (DLC 1: low; DLC 2: [low,high]) | Request version/build date; controller replies with Pong if it matches |
| Set Sensor Type | `0x0EF70000` | ≥2 | `[index, type]` | Set LSU sensor type (0=LSU4.9, 1=LSU4.2, 2=LSU ADV, 3=FAE LSU4.9) |
| HeaterConfig | `0x0EF80000` | ≥3 | Heater thresholds, preheat time | Stored in flash; byte layout TBD |

### Controller → ECU Replies

| CAN ID (hex) | DLC | Description |
|-------------|-----|-------------|
| `0x727573` | 0 | Simple ACK acknowledgment |
| `0x727573` | 8 | Pong response with version/build date info |

### Sensor Data Messages (Standard 11-bit IDs)

**ID Formula:** `0x190 + (2 × index)` where index = 0-15

Each sensor uses **two consecutive CAN IDs**:

| ID Type | Formula | Example (index=0) | Description |
|---------|---------|-------------------|-------------|
| Standard Data | `0x190 + (2 × index)` | `0x190` | Lambda and temperature |
| Diagnostic Data | `0x190 + (2 × index) + 1` | `0x191` | ESR, nernst, status |

### AEMNet Messages (29-bit extended, big-endian)

| Message | CAN ID | DLC | Rate | Enable Flag |
|---------|--------|-----|------|-------------|
| AEMNet UEGO | `0x180 + AemNetIdOffset` | 8 | 100 Hz | `AemNetTx` |
| AEMNet EGT | `0x0A0305 + AemNetIdOffset` | 8 | 20 Hz | `egt[ch].AemNetTx` |

**Transmit schedule:** AFR at 100 Hz, EGT every 5th cycle (20 Hz). `SendCanForChannel()` calls both `SendRusefiFormat()` and `SendAemNetUEGOFormat()`.

### SetIndex (0xEF40000)

Sets the base CAN ID for RusEFI format. Full ID format (DLC 2/3) is the current way:

| DLC | Format | Bytes | Example |
|-----|--------|-------|---------|
| 1 | Legacy offset (deprecated) | `[0]` = offset from 0x190 | `[5]` → base 0x195 |
| 2 | Full ID | `[0]=low, [1]=high` (little-endian) | `[0xA0, 0x01]` → base 0x1A0 |
| 3 | Full ID + hwIdx | `[0]=low, [1]=high, [2]=hwIdx]` | `[0xA0, 0x01, 0]` → base 0x1A0 for hwIdx 0 |

- Base ID must be 11-bit standard (0–0x7FF).
- AFR channels: `RusEfiBaseId = baseId + ch * 2` (StandardData at base, DiagData at base+1).
- EGT channels: `RusEfiIdOffset` derived from base.
- Persists to flash.

### Pong Response

Layout: `baseId` (low, high), Version (0xA0), year, month, day, reserved

**Complete ID Map (RusEfi):**

| Index | Standard ID | Diagnostic ID |
|-------|-------------|---------------|
| 0 | 0x190 | 0x191 |
| 1 | 0x192 | 0x193 |
| 2 | 0x194 | 0x195 |
| 3 | 0x196 | 0x197 |
| 4 | 0x198 | 0x199 |
| 5 | 0x19A | 0x19B |
| 6 | 0x19C | 0x19D |
| 7 | 0x19E | 0x19F |
| 8 | 0x1A0 | 0x1A1 |
| 9 | 0x1A2 | 0x1A3 |
| 10 | 0x1A4 | 0x1A5 |
| 11 | 0x1A6 | 0x1A7 |
| 12 | 0x1A8 | 0x1A9 |
| 13 | 0x1AA | 0x1AB |
| 14 | 0x1AC | 0x1AD |
| 15 | 0x1AE | 0x1AF |

---

## Data Frame Structures

### Standard Data Frame (8 bytes) - Even CAN ID

Received on CAN ID: `0x190 + (2 × index)`

```
┌─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┐
│ Byte 0  │ Byte 1  │ Byte 2  │ Byte 3  │ Byte 4  │ Byte 5  │ Byte 6  │ Byte 7  │
├─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ Version │ Valid   │ Lambda LSB│Lambda MSB│Temp LSB │Temp MSB │ Pad     │ Pad     │
│ (0xA0)  │ (0/1)   │         │         │         │         │ (0x00)  │ (0x00)  │
└─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┘
```

| Field | Bytes | Type | Unit | Resolution |
|-------|-------|------|------|------------|
| Version | 0 | uint8 | - | Protocol version (0xA0 = current) |
| Valid | 1 | uint8 | - | 0 = invalid, 1 = valid data |
| Lambda | 2-3 | uint16 LE | λ | × 10000 (0.0001 resolution) |
| Temperature | 4-5 | int16 LE | °C | 1°C resolution |
| Pad | 6-7 | - | - | Reserved (0x00) |

**Decoding Examples:**

| Raw Lambda Value | λ Value | AFR (gasoline, 14.7) |
|------------------|---------|---------------------|
| 10000 | 1.000 | 14.7 |
| 10500 | 1.050 | 15.4 |
| 8500 | 0.850 | 12.5 |
| 12000 | 1.200 | 17.6 |

### Diagnostic Data Frame (8 bytes) - Odd CAN ID

Received on CAN ID: `0x190 + (2 × index) + 1`

```
┌─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┐
│ Byte 0  │ Byte 1  │ Byte 2  │ Byte 3  │ Byte 4  │ Byte 5  │ Byte 6  │ Byte 7  │
├─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ESR LSB  │ESR MSB  │Nernst LSB│Nernst MSB│Pump    │Status  │Heater  │ Pad     │
│         │         │         │         │Duty    │        │Duty    │ (0x00)  │
└─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┘
```

| Field | Bytes | Type | Unit | Resolution |
|-------|-------|------|------|------------|
| ESR | 0-1 | uint16 LE | Ω | 1 ohm resolution |
| Nernst DC | 2-3 | uint16 LE | V | × 1000 (0.001V resolution) |
| Pump Duty | 4 | uint8 | % | × 2.55 (raw 0-255 = 0-100%) |
| Status | 5 | uint8 | - | See status codes table |
| Heater Duty | 6 | uint8 | % | × 2.55 (raw 0-255 = 0-100%) |
| Pad | 7 | - | - | Reserved (0x00) |

**Status Codes:** (from WBO::Status in wideband_can.h, matches TunerStudio AfrFaultList)

| Value | Name | Description |
|-------|------|-------------|
| 0 | Preheat | Heater preheating |
| 1 | Warmup | Sensor warming up |
| 2 | Running | Closed-loop, valid data |
| 3 | Failed to heat | Sensor didn't heat within 30s |
| 4 | Overheat | Sensor overheated |
| 5 | Underheat | Sensor unexpectedly cold |
| 6 | No supply | No heater supply voltage |

### ECU Status Command (WidebandControl, TX to controller)

Send to CAN ID: `0x0EF50000` (extended)

```
┌─────────┬─────────┬─────────┐
│ Byte 0  │ Byte 1  │ Byte 2  │ (optional)
├─────────┼─────────┼─────────┤
│Voltage×10│Flags   │PumpGain │
└─────────┴─────────┴─────────┘
```

| Field | Byte | Description |
|-------|------|-------------|
| Voltage | 0 | Battery voltage × 10 (e.g., 135 = 13.5V) |
| Flags | 1 | Bit 0: Heater enable (1 = on, 0 = off) |
| PumpGain | 2 | Optional; 0–200 = 0–200% pump gain; only when DLC ≥ 3 |

**Important:** The controller requires this message to be sent periodically (~10ms) to enable the heater and start producing valid data.

### Ping Response (Pong)

Received on CAN ID: `0x727573` (extended) with DLC = 8

```
┌─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┐
│ Byte 0  │ Byte 1  │ Byte 2  │ Byte 3  │ Byte 4  │ Byte 5  │ Byte 6  │ Byte 7  │
├─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ hwId    │ Version │ year    │ month   │ day     │ Reserved│ Reserved│ Reserved│
│         │ (0xA0)  │         │         │         │         │         │         │
└─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┘
```

### AEMNet UEGO (0x180+offset, big-endian)

| Byte | Field | Encoding |
|------|-------|----------|
| 0–1 | Lambda | Big-endian, λ × 10000 |
| 2–3 | Oxygen | Big-endian, 0.001 %/bit (currently 0) |
| 4 | SystemVolts | 0.1 V/bit |
| 5 | reserved | 0 |
| 6 | Flags | Bit 7: lambda valid, Bit 1: LSU4.9 |
| 7 | Faults | Currently 0 |

### AEMNet EGT (0x0A0305+offset, big-endian)

| Byte | Field | Encoding |
|------|-------|----------|
| 0–1 | Temperature | Big-endian, °C × 10 |
| 2–7 | reserved | 0 |

---

## Timing & Protocol Behavior

### Transmission Rates

| Message Type | Rate | Interval |
|--------------|------|----------|
| Sensor Data (Standard) | 100 Hz | 10 ms |
| Sensor Data (Diagnostic) | 100 Hz | 10 ms |
| ECU Status (TX) | 100 Hz | 10 ms |
| ACK Response | Immediate | - |

### Timeout Values

| Operation | Timeout |
|-----------|---------|
| Standard ACK wait | 25 ms |
| Flash erase | 1000 ms |
| Flash write | 100 ms |
| Reboot | 500 ms |

### Startup Sequence

1. Controller powers up and waits for ECU status messages
2. ECU sends status with heater enable bit set
3. Controller begins heating sensor (warm-up takes ~10-30 seconds)
4. Once heated, controller starts sending valid lambda data

### Communication Flow

```
┌─────────┐                              ┌─────────────┐
│   ECU   │                              │  Wideband   │
│         │                              │ Controller  │
└────┬────┘                              └──────┬──────┘
     │                                          │
     │  ECU Status (0x0EF50000) [V×10, 0x01]   │
     │─────────────────────────────────────────>│
     │                                          │
     │         Sensor Data (0x190) [8 bytes]    │
     │<─────────────────────────────────────────│
     │                                          │
     │      Diagnostic (0x191) [8 bytes]        │
     │<─────────────────────────────────────────│
     │                                          │
     │              (repeat every 10ms)         │
     │                                          │
```

---

## Configuration

- **RusEfiBaseId** (AFR) — Full 11-bit base CAN ID per channel (StandardData at base, DiagData at base+1). Default 0x190 for ch0, 0x192 for ch1.
- **AemNetIdOffset** — Base 0x180 for UEGO, 0x0A0305 for EGT
- **RusEfiIdOffset** (EGT) — Offset from 0x190 for EGT channels

---

## Sensor Types

Supported LSU sensor types for `Set Sensor Type` command (0x0EF70000):

| Value | Sensor | Description |
|-------|--------|-------------|
| 0 | Bosch LSU 4.9 | Current standard, recommended |
| 1 | Bosch LSU 4.2 | Older sensor type |
| 2 | Bosch LSU ADV | Advanced sensor |
| 3 | FAE LSU 4.9 | FAE brand 4.9 compatible |

---

## Implementation Guide

### Prerequisites for Standalone Program

1. **CAN Interface**: Any CAN adapter (SocketCAN, PCAN, etc.)
2. **Bitrate**: 500 kbps (match your rusEFI config)
3. **Periodic TX**: Must send ECU status every 10ms to enable heater

### Critical Implementation Notes

1. **Heater Enable**: Controller won't produce valid data until it receives ECU status with heater bit set
2. **Warm-up Time**: Allow 10-30 seconds after enabling heater for sensor to reach operating temperature
3. **Valid Flag**: Always check byte 1 of standard data frame before using lambda value
4. **Dual IDs**: Each sensor uses two CAN IDs - don't ignore the diagnostic frames

### Minimal Implementation Steps

1. Initialize CAN bus at 500 kbps
2. Start periodic timer to send ECU status every 10ms
3. Receive and decode sensor data frames
4. Check valid flag before using data
5. Monitor diagnostic frames for error detection

---

## Code Examples

### Python Implementation

```python
#!/usr/bin/env python3
"""
rusEFI Wideband CAN Interface
Standalone implementation for reading rusEFI wideband controllers
"""

import can
import time
from dataclasses import dataclass
from typing import Optional

# CAN IDs
WB_ACK_ID = 0x727573
WB_DATA_BASE = 0x190
WB_CMD_ECU_STATUS = 0x0EF50000
WB_CMD_PING = 0x0EF60000
WB_CMD_SET_INDEX = 0x0EF40000
WB_CMD_SET_SENSOR_TYPE = 0x0EF70000

# Protocol version
RUSEFI_WIDEBAND_VERSION = 0xA0

# Sensor types
SENSOR_LSU49 = 0
SENSOR_LSU42 = 1
SENSOR_LSU_ADV = 2
SENSOR_FAE_LSU49 = 3


@dataclass
class WidebandData:
    """Standard sensor data"""
    index: int
    valid: bool
    lambda_value: float
    afr_gasoline: float
    temp_c: int


@dataclass
class WidebandDiagnostics:
    """Diagnostic sensor data"""
    index: int
    esr_ohms: int
    nernst_v: float
    pump_duty_pct: float
    heater_duty_pct: float
    status: int
    status_name: str


STATUS_NAMES = {
    0: "OK",
    1: "HEATER_FAULT",
    2: "SENSOR_ERROR"
}


def decode_standard_frame(can_id: int, data: bytes) -> Optional[WidebandData]:
    """Decode lambda and temperature from standard sensor data frame"""
    if len(data) != 8:
        return None

    index = (can_id - WB_DATA_BASE) // 2

    version = data[0]
    if version != RUSEFI_WIDEBAND_VERSION:
        print(f"Warning: Unknown protocol version 0x{version:02X}")

    valid = data[1] == 1
    lambda_raw = int.from_bytes(data[2:4], 'little')
    temp_raw = int.from_bytes(data[4:6], 'little', signed=True)

    lambda_value = lambda_raw / 10000.0
    afr = lambda_value * 14.7  # Gasoline stoichiometric AFR

    return WidebandData(
        index=index,
        valid=valid,
        lambda_value=lambda_value,
        afr_gasoline=afr,
        temp_c=temp_raw
    )


def decode_diagnostic_frame(can_id: int, data: bytes) -> Optional[WidebandDiagnostics]:
    """Decode diagnostic data from sensor"""
    if len(data) != 8:
        return None

    index = (can_id - WB_DATA_BASE - 1) // 2

    esr = int.from_bytes(data[0:2], 'little')
    nernst_mv = int.from_bytes(data[2:4], 'little') / 1000.0
    pump_duty = data[4] / 2.55
    status = data[5]
    heater_duty = data[6] / 2.55

    return WidebandDiagnostics(
        index=index,
        esr_ohms=esr,
        nernst_v=nernst_mv,
        pump_duty_pct=pump_duty,
        heater_duty_pct=heater_duty,
        status=status,
        status_name=STATUS_NAMES.get(status, "UNKNOWN")
    )


def send_ecu_status(bus: can.Bus, voltage: float, heater_enable: bool) -> None:
    """Send periodic ECU status to enable heater operation"""
    msg = can.Message(
        arbitration_id=WB_CMD_ECU_STATUS,
        is_extended_id=True,
        data=[
            int(voltage * 10),
            0x01 if heater_enable else 0x00
        ]
    )
    bus.send(msg)


def send_ping(bus: can.Bus, index: int = 0) -> None:
    """Ping controller to request version info"""
    msg = can.Message(
        arbitration_id=WB_CMD_PING,
        is_extended_id=True,
        data=[index]
    )
    bus.send(msg)


class WidebandController:
    """High-level interface for rusEFI wideband controller"""

    def __init__(self, can_interface: str = 'socketcan', can_channel: str = 'can0'):
        self.bus = can.Bus(interface=can_interface, channel=can_channel, bitrate=500000)
        self.sensor_data: dict[int, WidebandData] = {}
        self.diagnostics: dict[int, WidebandDiagnostics] = {}
        self._running = False

    def start_heater(self, voltage: float = 13.5):
        """Enable heater with specified voltage"""
        send_ecu_status(self.bus, voltage, heater_enable=True)

    def process_message(self, msg: can.Message):
        """Process incoming CAN message"""
        can_id = msg.arbitration_id

        # Check for sensor data (standard ID range 0x190-0x1AF)
        if WB_DATA_BASE <= can_id <= WB_DATA_BASE + 31:
            if can_id % 2 == 0:
                # Standard data frame (even ID)
                data = decode_standard_frame(can_id, msg.data)
                if data:
                    self.sensor_data[data.index] = data
            else:
                # Diagnostic frame (odd ID)
                diag = decode_diagnostic_frame(can_id, msg.data)
                if diag:
                    self.diagnostics[diag.index] = diag

        # Check for ACK/pong response
        elif can_id == WB_ACK_ID:
            if len(msg.data) == 8:
                print(f"Pong received: Version={msg.data[0]}, Build={msg.data[1]}/{msg.data[2]}/{msg.data[3]}")
            else:
                print("ACK received")

    def get_lambda(self, index: int = 0) -> Optional[float]:
        """Get current lambda value for sensor"""
        if index in self.sensor_data and self.sensor_data[index].valid:
            return self.sensor_data[index].lambda_value
        return None

    def get_afr(self, index: int = 0, stoich: float = 14.7) -> Optional[float]:
        """Get current AFR value for sensor"""
        lam = self.get_lambda(index)
        if lam is not None:
            return lam * stoich
        return None


def main():
    """Example usage"""
    print("rusEFI Wideband CAN Monitor")
    print("============================")

    # Initialize controller
    wb = WidebandController(can_interface='socketcan', can_channel='can0')

    print("Starting heater...")
    wb.start_heater(voltage=13.5)

    print("Listening for sensor data (Ctrl+C to stop)...")

    try:
        while True:
            msg = wb.bus.recv(timeout=1.0)
            if msg:
                wb.process_message(msg)

                # Print sensor data
                for idx, data in wb.sensor_data.items():
                    if data.valid:
                        print(f"Sensor {idx}: λ={data.lambda_value:.3f} "
                              f"AFR={data.afr_gasoline:.1f} "
                              f"Temp={data.temp_c}°C")

                # Print diagnostics if errors
                for idx, diag in wb.diagnostics.items():
                    if diag.status != 0:
                        print(f"  [WARN] Sensor {idx}: {diag.status_name}")

            # Keep sending heater status
            wb.start_heater(voltage=13.5)

    except KeyboardInterrupt:
        print("\nStopping...")


if __name__ == "__main__":
    main()
```

### C Implementation

```c
/*
 * rusEFI Wideband CAN Interface (C)
 * For bare-metal or embedded systems
 */

#include <stdint.h>
#include <stdbool.h>

// CAN IDs
#define WB_ACK_ID           0x727573
#define WB_DATA_BASE        0x190
#define WB_CMD_ECU_STATUS   0x0EF50000
#define WB_CMD_PING         0x0EF60000

// Protocol version
#define RUSEFI_WIDEBAND_VERSION 0xA0

// Sensor types
typedef enum {
    SENSOR_LSU49 = 0,
    SENSOR_LSU42 = 1,
    SENSOR_LSU_ADV = 2,
    SENSOR_FAE_LSU49 = 3
} SensorType;

// Status codes
typedef enum {
    STATUS_OK = 0,
    STATUS_HEATER_FAULT = 1,
    STATUS_SENSOR_ERROR = 2
} WidebandStatus;

// Data structures
typedef struct {
    uint8_t index;
    bool valid;
    uint16_t lambda_raw;    // Lambda * 10000
    int16_t temp_c;
} WidebandData;

typedef struct {
    uint8_t index;
    uint16_t esr_ohms;
    uint16_t nernst_mv;     // mV * 1000
    uint8_t pump_duty;      // 0-255 = 0-100%
    uint8_t status;
    uint8_t heater_duty;    // 0-255 = 0-100%
} WidebandDiagnostics;

// Decode standard data frame
bool decode_standard_frame(uint32_t can_id, uint8_t *data, uint8_t len, WidebandData *out) {
    if (len != 8) return false;
    if (can_id < WB_DATA_BASE || can_id > WB_DATA_BASE + 30) return false;
    if (can_id % 2 != 0) return false;  // Must be even ID

    out->index = (can_id - WB_DATA_BASE) / 2;
    out->valid = (data[1] == 1);
    out->lambda_raw = data[2] | (data[3] << 8);  // Little-endian
    out->temp_c = data[4] | (data[5] << 8);

    return true;
}

// Decode diagnostic frame
bool decode_diagnostic_frame(uint32_t can_id, uint8_t *data, uint8_t len, WidebandDiagnostics *out) {
    if (len != 8) return false;
    if (can_id < WB_DATA_BASE + 1 || can_id > WB_DATA_BASE + 31) return false;
    if (can_id % 2 != 1) return false;  // Must be odd ID

    out->index = (can_id - WB_DATA_BASE - 1) / 2;
    out->esr_ohms = data[0] | (data[1] << 8);
    out->nernst_mv = data[2] | (data[3] << 8);
    out->pump_duty = data[4];
    out->status = data[5];
    out->heater_duty = data[6];

    return true;
}

// Convert raw lambda to floating point
float lambda_to_float(uint16_t lambda_raw) {
    return (float)lambda_raw / 10000.0f;
}

// Convert lambda to AFR (gasoline)
float lambda_to_afr(uint16_t lambda_raw) {
    return lambda_to_float(lambda_raw) * 14.7f;
}

// Build ECU status message
void build_ecu_status(uint8_t *data, float voltage, bool heater_enable) {
    data[0] = (uint8_t)(voltage * 10.0f);
    data[1] = heater_enable ? 0x01 : 0x00;
}

// Build ping message
void build_ping(uint8_t *data, uint8_t index) {
    data[0] = index;
}

/*
 * Example CAN TX function (platform-specific implementation required):
 *
 * void can_tx_extended(uint32_t id, uint8_t *data, uint8_t len);
 *
 * Usage:
 *   uint8_t status_data[2];
 *   build_ecu_status(status_data, 13.5, true);
 *   can_tx_extended(WB_CMD_ECU_STATUS, status_data, 2);
 */
```

---

## Troubleshooting

### No Data Received

1. Check CAN bus termination (120Ω at each end)
2. Verify bitrate matches (typically 500 kbps)
3. Ensure ECU status is being sent with heater bit enabled
4. Wait for warm-up period (10-30 seconds)

### Invalid Data Flag Set

1. Sensor not warmed up - wait longer
2. Sensor not connected - check wiring
3. Heater fault - check diagnostic frame status

### Erratic Readings

1. Check ESR value in diagnostics (should be 80-300Ω for LSU4.9)
2. Check heater duty cycle (should stabilize after warm-up)
3. Verify sensor type matches actual sensor

---

## References

- Source: rusEFI firmware (`firmware/controllers/sensors/wideband/`)
- Key files: `wideband_can.h`, `AemXSeriesLambda.cpp`, `CanTxMessage.h`
- rusEFI Wiki: https://wiki.rusefi.com/

---

*Document generated from rusEFI firmware analysis*
