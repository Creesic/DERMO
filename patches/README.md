# Patches for BootCommander (OpenBLT) macOS build

The `openblt-darwin` patch adds a Darwin (macOS) port to LibOpenBLT so BootCommander
can be built natively on macOS. Serial (xcp_rs232), USB (xcp_usb), and TCP/IP (xcp_net)
work; CAN (xcp_can) is not supported (no SocketCAN on macOS).

## Applying the patch

If you clone OpenBLT fresh and need the Darwin port:

```bash
git clone --depth 1 https://github.com/feaser/openblt.git /path/to/openblt
cd /path/to/openblt
git apply /path/to/DERMO/patches/openblt-darwin.patch
```

Or use the project's `ext/openblt` which already has the patch applied.
